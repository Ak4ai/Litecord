use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use log::info;

use std::path::PathBuf;

pub fn get_window_state_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(app_data) = std::env::var("APPDATA") {
            let path = PathBuf::from(app_data).join("Litecord");
            let _ = std::fs::create_dir_all(&path);
            return path.join("window_state.json");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let path = PathBuf::from(home).join("Library").join("Application Support").join("Litecord");
            let _ = std::fs::create_dir_all(&path);
            return path.join("window_state.json");
        }
    }
    // Linux / BSD / Fallback
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            let path = PathBuf::from(xdg).join("litecord");
            let _ = std::fs::create_dir_all(&path);
            return path.join("window_state.json");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let path = PathBuf::from(home).join(".config").join("litecord");
        let _ = std::fs::create_dir_all(&path);
        return path.join("window_state.json");
    }
    PathBuf::from(".litecord_window_state.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub is_maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: Some(100),
            y: Some(100),
            width: 980,
            height: 620,
            is_maximized: false,
        }
    }
}

pub fn load_window_state() -> WindowState {
    let path = get_window_state_path();
    // Try canonical path first, fallback to legacy relative files for backward compatibility
    let data = std::fs::read_to_string(&path)
        .or_else(|_| std::fs::read_to_string(".litecord_window_state.json"))
        .or_else(|_| {
            if let Ok(home) = std::env::var("HOME") {
                std::fs::read_to_string(PathBuf::from(home).join(".litecord_window_state.json"))
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
            }
        });

    if let Ok(content) = data {
        if let Ok(state) = serde_json::from_str::<WindowState>(&content) {
            // Guard against corrupted or zero/negative dimensions
            if state.width >= 400 && state.height >= 300 {
                info!("🪟 Estado e posição da janela carregados de {:?}: {:?}", path, state);
                return state;
            }
        }
    }
    WindowState::default()
}

pub fn save_window_state(state: &WindowState) {
    let path = get_window_state_path();
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(&path, json);
    }
}

static WINDOW_STATE_STORE: OnceLock<Mutex<WindowState>> = OnceLock::new();

pub fn get_window_state_store() -> &'static Mutex<WindowState> {
    WINDOW_STATE_STORE.get_or_init(|| Mutex::new(load_window_state()))
}

pub fn update_window_geometry(
    x: Option<i32>,
    y: Option<i32>,
    width: Option<u32>,
    height: Option<u32>,
    is_maximized: bool,
) {
    let mut store = get_window_state_store().lock().unwrap_or_else(|e| e.into_inner());
    store.is_maximized = is_maximized;

    // CRITICAL: When the window is maximized (or occupies the full monitor work area),
    // we strictly preserve store.width and store.height containing the last known floating/restored size.
    // Overwriting them with monitor dimensions (e.g. 1920x1080) would cause subsequent "unmaximize"
    // or next-launch restoration to forget the user's floating window size.
    //
    // NOTE ON WAYLAND (xdg_toplevel) COORDINATES:
    // Under native Wayland (KWin / Mutter), the xdg_toplevel protocol deliberately prevents client
    // applications from querying or setting global screen (x, y) coordinates for security and sandboxing.
    // Window placement is delegated entirely to the compositor (or KDE Window Rules).
    // Under Windows (Win32) and Linux (X11), (x, y) coordinates are faithfully set and restored.
    if !is_maximized {
        if let (Some(x_pos), Some(y_pos)) = (x, y) {
            // Filter out off-screen coordinates or invalid negative coordinates (-32000 on Windows when minimized)
            if x_pos >= -10000 && y_pos >= -10000 {
                store.x = Some(x_pos);
                store.y = Some(y_pos);
            }
        }
        if let (Some(w), Some(h)) = (width, height) {
            if w >= 400 && h >= 300 {
                store.width = w;
                store.height = h;
            }
        }
    }
    info!("🪟 Estado da janela salvo: is_maximized={}, size={}x{}, pos={:?},{:?}", store.is_maximized, store.width, store.height, store.x, store.y);
    save_window_state(&store);
}
