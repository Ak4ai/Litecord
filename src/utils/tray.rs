use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};
use tray_icon::{
    menu::{Menu, MenuItem, MenuId},
    Icon, TrayIcon, TrayIconBuilder,
};
use log::{info, warn};

thread_local! {
    // The icon must be touched only on the thread that created it (the UI thread).
    static TRAY_ICON: RefCell<Option<TrayIcon>> = RefCell::new(None);
    static TRAY_MENU: RefCell<Option<Menu>> = RefCell::new(None);
}

// Presence bits of the tray/taskbar icon.  Green means "in a call" (voice
// channel or DM), red means "broadcasting" (camera or screen share); muted and
// deafened are drawn as a small badge on top of either colour.
const PRESENCE_VOICE: u8 = 1 << 0;
const PRESENCE_STREAM: u8 = 1 << 1;
const PRESENCE_MUTE: u8 = 1 << 2;
const PRESENCE_DEAF: u8 = 1 << 3;

static PRESENCE: AtomicU8 = AtomicU8::new(0);

const ICON_VOICE: &[u8] = include_bytes!("../../assets/status_voice.png");
const ICON_STREAM: &[u8] = include_bytes!("../../assets/status_stream.png");
const ICON_MUTE: &[u8] = include_bytes!("../../assets/status_overlay_mute.png");
const ICON_DEAF: &[u8] = include_bytes!("../../assets/status_overlay_deafen.png");
const ICON_DEFAULT: &[u8] = include_bytes!("../../assets/app_icon.png");

fn compose_presence_icon(bits: u8) -> Option<image::RgbaImage> {
    let base = if bits & PRESENCE_STREAM != 0 {
        ICON_STREAM
    } else if bits & PRESENCE_VOICE != 0 {
        ICON_VOICE
    } else {
        ICON_DEFAULT
    };
    let mut image = image::load_from_memory(base).ok()?.into_rgba8();
    // The bundled app icon is 512×512 while the presence badges are 64×64;
    // normalise so the overlay lands in the bottom-right corner in both cases.
    if image.dimensions() != (64, 64) {
        image = image::imageops::resize(&image, 64, 64, image::imageops::FilterType::Lanczos3);
    }
    let overlay = if bits & PRESENCE_DEAF != 0 {
        Some(ICON_DEAF)
    } else if bits & PRESENCE_MUTE != 0 {
        Some(ICON_MUTE)
    } else {
        None
    };
    if let Some(overlay) = overlay {
        if let Ok(badge) = image::load_from_memory(overlay) {
            let badge = badge.into_rgba8();
            image::imageops::overlay(&mut image, &badge, 0, 0);
        }
    }
    Some(image)
}

/// Updates the tray and taskbar presence icon.  Returns `true` when the icon
/// actually changed, so the window icon is only touched on transitions.
pub fn set_presence(in_voice: bool, streaming: bool, muted: bool, deafened: bool) -> bool {
    let mut bits = 0u8;
    if in_voice {
        bits |= PRESENCE_VOICE;
    }
    if streaming {
        bits |= PRESENCE_STREAM;
    }
    if muted {
        bits |= PRESENCE_MUTE;
    }
    if deafened {
        bits |= PRESENCE_DEAF;
    }
    if PRESENCE.swap(bits, Ordering::SeqCst) == bits {
        return false;
    }

    #[cfg(target_os = "linux")]
    linux_tray::refresh();

    #[cfg(not(target_os = "linux"))]
    if let Some(image) = compose_presence_icon(bits) {
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(icon) = Icon::from_rgba(rgba, width, height) {
                TRAY_ICON.with(|cell| {
                    if let Some(tray) = cell.borrow().as_ref() {
                        let _ = tray.set_icon(Some(icon));
                    }
                });
            }
        });
    }

    true
}

/// RGBA pixels of the current presence icon, for the taskbar/dock window icon.
pub fn presence_icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let image = compose_presence_icon(PRESENCE.load(Ordering::SeqCst))?;
    let (width, height) = image.dimensions();
    Some((image.into_raw(), width, height))
}

/// Receives `true` for show/hide toggle and `false` for quit. Linux uses this
/// from the StatusNotifierItem thread; other platforms from the tray callback.
static ACTION_TX: std::sync::OnceLock<std::sync::Mutex<Option<std::sync::mpsc::Sender<bool>>>> =
    std::sync::OnceLock::new();

/// Must be called before `SystemTrayManager::setup()`.
pub fn set_action_sender(tx: std::sync::mpsc::Sender<bool>) {
    let slot = ACTION_TX.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(tx);
    }
}

fn send_action(toggle: bool) {
    if let Some(slot) = ACTION_TX.get() {
        if let Ok(guard) = slot.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(toggle);
            }
        }
    }
}

/// Sets the tray tooltip from any thread. Runs on the UI thread only when called,
/// so nothing polls for tooltip changes.
pub fn update_tray_tooltip(tooltip: &str) {
    #[cfg(target_os = "linux")]
    {
        linux_tray::set_tooltip(tooltip);
    }
    let tip = tooltip.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        TRAY_ICON.with(|icon| {
            if let Some(icon) = icon.borrow().as_ref() {
                let _ = icon.set_tooltip(Some(tip));
            }
        });
    });
}

/// KDE Plasma / Wayland: `tray-icon` uses libappindicator, which only exposes a
/// menu and never reports a left click. ksni speaks StatusNotifierItem directly,
/// so `activate()` fires on left click and can toggle the window.
#[cfg(target_os = "linux")]
mod linux_tray {
    use super::*;
    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;
    use ksni::{MenuItem, ToolTip, Tray};
    use std::sync::{Arc, Mutex};

    pub struct LitecordTray {
        tooltip: Arc<Mutex<String>>,
    }

    static HANDLE: Mutex<Option<ksni::blocking::Handle<LitecordTray>>> = Mutex::new(None);
    static TOOLTIP: Mutex<Option<Arc<Mutex<String>>>> = Mutex::new(None);

    fn icon_pixmaps() -> Vec<ksni::Icon> {
        // Plasma asks for the item properties periodically (roughly once a
        // second).  The presence icon changes only on state transitions, so
        // cache each variant instead of decoding the PNG on every GetAll.
        static CACHE: Mutex<Option<(u8, Vec<ksni::Icon>)>> = Mutex::new(None);
        let bits = PRESENCE.load(Ordering::SeqCst);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached_bits, icons)) = cache.as_ref() {
            if *cached_bits == bits {
                return icons.clone();
            }
        }
        let icons = compose_presence_icon(bits)
            .map(|image| {
                let (width, height) = image.dimensions();
                // StatusNotifierItem expects ARGB32 in network byte order.
                let mut data = Vec::with_capacity((width * height * 4) as usize);
                for pixel in image.pixels() {
                    data.push(pixel[3]);
                    data.push(pixel[0]);
                    data.push(pixel[1]);
                    data.push(pixel[2]);
                }
                vec![ksni::Icon { width: width as i32, height: height as i32, data }]
            })
            .unwrap_or_default();
        *cache = Some((bits, icons.clone()));
        icons
    }

    impl Tray for LitecordTray {
        fn id(&self) -> String {
            "litecord".into()
        }

        fn title(&self) -> String {
            "Litecord".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            icon_pixmaps()
        }

        fn tool_tip(&self) -> ToolTip {
            ToolTip {
                title: "Litecord".into(),
                description: self.tooltip.lock().map(|t| t.clone()).unwrap_or_default(),
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            super::send_action(true);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                StandardItem {
                    label: "Mostrar/Ocultar Litecord".into(),
                    activate: Box::new(|_t| super::send_action(true)),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Sair".into(),
                    activate: Box::new(|_t| super::send_action(false)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    static SPAWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub fn spawn() {
        if SPAWNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        std::thread::Builder::new()
            .name("ksni-tray".into())
            .spawn(|| {
                let tooltip = Arc::new(Mutex::new(
                    "Litecord - Discord Client Ultra-Leve".to_string(),
                ));
                if let Ok(mut slot) = TOOLTIP.lock() {
                    *slot = Some(Arc::clone(&tooltip));
                }
                let tray = LitecordTray { tooltip };
                match tray.assume_sni_available(true).spawn() {
                    Ok(handle) => {
                        info!("Ícone roxo ativado na bandeja (StatusNotifierItem).");
                        if let Ok(mut slot) = HANDLE.lock() {
                            *slot = Some(handle);
                        }
                    }
                    Err(e) => warn!("System Tray (SNI) indisponível: {:?}", e),
                }
            })
            .ok();
    }

    pub fn set_enabled(enabled: bool) {
        if enabled {
            spawn();
        } else if let Ok(mut slot) = HANDLE.lock() {
            if let Some(handle) = slot.take() {
                let _ = handle.shutdown();
                SPAWNED.store(false, std::sync::atomic::Ordering::SeqCst);
                info!("Ícone da bandeja (SNI) desativado.");
            }
        }
    }

    pub fn set_tooltip(tooltip: &str) {
        if let Ok(slot) = TOOLTIP.lock() {
            if let Some(shared) = slot.as_ref() {
                if let Ok(mut t) = shared.lock() {
                    *t = tooltip.to_string();
                }
            }
        }
        if let Ok(slot) = HANDLE.lock() {
            if let Some(handle) = slot.as_ref() {
                handle.update(|_t| {});
            }
        }
    }

    /// Republishes the item so the host fetches the new `icon_pixmap`.
    pub fn refresh() {
        if let Ok(slot) = HANDLE.lock() {
            if let Some(handle) = slot.as_ref() {
                handle.update(|_t| {});
            }
        }
    }
}

fn build_tray_icon(menu: Menu) -> Option<TrayIcon> {
    let icon = create_default_icon();
    match TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .with_tooltip("Litecord - Discord Client Ultra-Leve")
        .build()
    {
        Ok(icon) => {
            info!("Ícone roxo ativado na bandeja do sistema (System Tray).");
            Some(icon)
        }
        Err(e) => {
            warn!("Não foi possível inicializar o ícone do System Tray ({:?}), continuando modo janela.", e);
            None
        }
    }
}

/// Shows or removes the system tray icon at runtime (must be called on or dispatched to the UI thread).
pub fn set_tray_enabled(enabled: bool) {
    let _ = slint::invoke_from_event_loop(move || {
        #[cfg(target_os = "linux")]
        {
            linux_tray::set_enabled(enabled);
            return;
        }
        #[cfg(not(target_os = "linux"))]
        TRAY_ICON.with(|icon_cell| {
            if enabled {
                if let Some(existing) = icon_cell.borrow().as_ref() {
                    let _ = existing.set_visible(true);
                    return;
                }
                let menu_opt = TRAY_MENU.with(|m| m.borrow().clone());
                if let Some(menu) = menu_opt {
                    *icon_cell.borrow_mut() = build_tray_icon(menu);
                }
            } else if let Some(existing) = icon_cell.borrow_mut().take() {
                let _ = existing.set_visible(false);
                drop(existing);
                info!("Ícone da bandeja do sistema (System Tray) desativado.");
            }
        });
    });
}

pub struct SystemTrayManager {
    pub show_item_id: MenuId,
    pub quit_item_id: MenuId,
}


fn create_default_icon() -> Icon {
    let icon_bytes = include_bytes!("../../assets/app_icon.png");
    if let Ok(img) = image::load_from_memory(icon_bytes) {
        let rgba_img = img.into_rgba8();
        let (width, height) = rgba_img.dimensions();
        if let Ok(icon) = Icon::from_rgba(rgba_img.into_raw(), width, height) {
            return icon;
        }
    }

    let width = 32u32;
    let height = 32u32;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);

    for y in 0..height {
        for x in 0..width {
            let is_border = x == 0 || x == width - 1 || y == 0 || y == height - 1;
            if is_border {
                rgba.extend_from_slice(&[0x11, 0x12, 0x14, 0xFF]);
            } else {
                rgba.extend_from_slice(&[0x58, 0x65, 0xF2, 0xFF]);
            }
        }
    }

    Icon::from_rgba(rgba, width, height).expect("Falha ao gerar ícone do tray")
}

impl SystemTrayManager {
    /// Call on the UI thread.
    pub fn setup() -> Self {
        #[cfg(target_os = "linux")]
        {
            let _ = gtk::init();
        }

        let tray_menu = Menu::new();
        let show_item = MenuItem::new("Exibir Litecord", true, None);
        let quit_item = MenuItem::new("Sair", true, None);

        let show_id = show_item.id().clone();
        let quit_id = quit_item.id().clone();

        let _ = tray_menu.append(&show_item);
        let _ = tray_menu.append(&quit_item);

        TRAY_MENU.with(|m| *m.borrow_mut() = Some(tray_menu.clone()));

        #[cfg(target_os = "linux")]
        {
            if super::video_settings::get_enable_tray_icon() {
                linux_tray::spawn();
            } else {
                info!("System Tray desativado nas configurações iniciais.");
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let tray_icon = if super::video_settings::get_enable_tray_icon() {
                build_tray_icon(tray_menu)
            } else {
                info!("System Tray desativado nas configurações iniciais.");
                None
            };
            TRAY_ICON.with(|icon| *icon.borrow_mut() = tray_icon);
        }

        Self {
            show_item_id: show_id,
            quit_item_id: quit_id,
        }
    }
}


