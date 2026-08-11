mod gateway;
mod http;
mod tray;

use gateway::{GatewayClient, GatewayEvent, GatewayCommand, GuildData, ChannelData, format_discord_author, format_discord_message, format_discord_message_parts};
use http::DiscordHttpClient;
use tray::SystemTrayManager;

use slint::{SharedString, Model, Image};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::collections::HashMap;
use tokio::sync::mpsc;
use log::{info, error};
use tray_icon::{TrayIconEvent, menu::MenuEvent, MouseButton};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ShowWindow, SetForegroundWindow, GetForegroundWindow,
    SW_HIDE, SW_SHOW, SW_RESTORE
};

use cpal::traits::{HostTrait, DeviceTrait, StreamTrait};

slint::include_modules!();

fn parse_hex_color(hex: &str) -> slint::Color {
    if hex.starts_with('#') && hex.len() == 7 {
        if let Ok(r) = u8::from_str_radix(&hex[1..3], 16) {
            if let Ok(g) = u8::from_str_radix(&hex[3..5], 16) {
                if let Ok(b) = u8::from_str_radix(&hex[5..7], 16) {
                    return slint::Color::from_rgb_u8(r, g, b);
                }
            }
        }
    }
    slint::Color::from_rgb_u8(88, 101, 242) // default blurple
}

/// True when the window is visible; false when hidden to the system tray.
/// Guards all UI-rendering loops so they sleep when not needed.
static APP_IS_VISIBLE: AtomicBool = AtomicBool::new(true);

/// Set to true by the tray restore paths to signal a REST refresh is needed.
static NEED_UI_REFRESH: AtomicBool = AtomicBool::new(false);

/// Number of messages received while the window was hidden.
/// Cleared when the window is restored and a REST refresh is triggered.
static PENDING_MESSAGES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct RawGuildItem {
    id: String,
    name: String,
    icon: String,
    icon_path: Option<String>,
}

fn enumerate_audio_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();

    if let Ok(devices) = host.input_devices() {
        for dev in devices {
            if let Ok(name) = dev.name() {
                if !inputs.contains(&name) {
                    inputs.push(name);
                }
            }
        }
    }

    if let Ok(devices) = host.output_devices() {
        for dev in devices {
            if let Ok(name) = dev.name() {
                if !outputs.contains(&name) {
                    outputs.push(name);
                }
            }
        }
    }

    if inputs.is_empty() {
        inputs.push("Microfone Padrão do Sistema".to_string());
    }

    if outputs.is_empty() {
        outputs.push("Alto-falantes Padrão do Sistema".to_string());
    }

    (inputs, outputs)
}

fn start_mic_capture(
    device_name: String,
    level_tx: mpsc::Sender<f32>,
) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let devices = host.input_devices().ok()?;

    let target_dev = if device_name.is_empty() || device_name.contains("Padrão") {
        host.default_input_device()?
    } else {
        devices.into_iter().find(|d| {
            d.name().map(|n| n == device_name).unwrap_or(false)
        })?
    };

    let config = target_dev.default_input_config().ok()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let pcm_queue = gateway::get_mic_pcm_queue();

    info!("Capturando microfone: {}Hz, {} canal(is)", sample_rate, channels);

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let q_arc = Arc::clone(&pcm_queue);
            target_dev.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &_| {
                    let mut sum_sq = 0.0f32;
                    if let Ok(mut q) = q_arc.lock() {
                        // Downmix to mono and resample to 48000Hz
                        let mono_samples: Vec<f32> = data.chunks(channels.max(1)).map(|frame| {
                            let s = frame.iter().sum::<f32>() / frame.len() as f32;
                            sum_sq += s * s;
                            s.clamp(-1.0, 1.0)
                        }).collect();

                        // Linear interpolation resample to 48000Hz
                        if sample_rate == 48000 {
                            for s in mono_samples {
                                if q.len() < 96000 { q.push_back(s); }
                            }
                        } else {
                            let ratio = 48000.0 / sample_rate as f64;
                            let out_len = (mono_samples.len() as f64 * ratio) as usize;
                            for i in 0..out_len {
                                let src_pos = i as f64 / ratio;
                                let src_idx = src_pos as usize;
                                let frac = src_pos - src_idx as f64;
                                let s0 = mono_samples.get(src_idx).copied().unwrap_or(0.0);
                                let s1 = mono_samples.get(src_idx + 1).copied().unwrap_or(s0);
                                let resampled = (s0 * (1.0 - frac as f32) + s1 * frac as f32).clamp(-1.0, 1.0);
                                if q.len() < 96000 { q.push_back(resampled); }
                            }
                        }
                    }
                    let rms = (sum_sq / (data.len() / channels.max(1)).max(1) as f32).sqrt();
                    let level = (rms * 6.0).min(1.0);
                    let _ = level_tx.try_send(level);
                },
                move |err| {
                    log::error!("Erro no Stream de Microfone: {:?}", err);
                },
                None,
            ).ok()?
        }
        cpal::SampleFormat::I16 => {
            let q_arc = Arc::clone(&pcm_queue);
            target_dev.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &_| {
                    let mut sum_sq = 0.0f32;
                    if let Ok(mut q) = q_arc.lock() {
                        let mono_samples: Vec<f32> = data.chunks(channels.max(1)).map(|frame| {
                            let s = frame.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / frame.len() as f32;
                            sum_sq += s * s;
                            s.clamp(-1.0, 1.0)
                        }).collect();

                        if sample_rate == 48000 {
                            for s in mono_samples {
                                if q.len() < 96000 { q.push_back(s); }
                            }
                        } else {
                            let ratio = 48000.0 / sample_rate as f64;
                            let out_len = (mono_samples.len() as f64 * ratio) as usize;
                            for i in 0..out_len {
                                let src_pos = i as f64 / ratio;
                                let src_idx = src_pos as usize;
                                let frac = src_pos - src_idx as f64;
                                let s0 = mono_samples.get(src_idx).copied().unwrap_or(0.0);
                                let s1 = mono_samples.get(src_idx + 1).copied().unwrap_or(s0);
                                let resampled = (s0 * (1.0 - frac as f32) + s1 * frac as f32).clamp(-1.0, 1.0);
                                if q.len() < 96000 { q.push_back(resampled); }
                            }
                        }
                    }
                    let rms = (sum_sq / (data.len() / channels.max(1)).max(1) as f32).sqrt();
                    let level = (rms * 6.0).min(1.0);
                    let _ = level_tx.try_send(level);
                },
                move |err| {
                    log::error!("Erro no Stream de Microfone: {:?}", err);
                },
                None,
            ).ok()?
        }
        _ => return None,
    };

    if stream.play().is_ok() {
        info!("Stream de Microfone ativado com SUCESSO via WASAPI! Rate: {}Hz -> 48000Hz", sample_rate);
        Some(stream)
    } else {
        None
    }
}

fn get_guild_initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() >= 2 {
        words.iter().filter_map(|w| w.chars().next()).take(3).collect::<String>().to_uppercase()
    } else {
        name.chars().take(2).collect::<String>().to_uppercase()
    }
}

async fn fetch_and_populate_guilds(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
) {
    info!("Buscando servidores do usuário via REST API...");
    let cache_dir = std::path::Path::new(".litecord_cache/icons");
    let _ = std::fs::create_dir_all(cache_dir);

    match http.get_user_guilds().await {
        Ok(guilds_json) => {
            info!("{} servidores encontrados via REST!", guilds_json.len());
            let mut raw_guilds: Vec<RawGuildItem> = Vec::new();
            let mut first_guild_id_opt: Option<String> = None;
            let mut pending_icon_downloads: Vec<(String, String)> = Vec::new();

            for g in guilds_json {
                let g_id = g["id"].as_str().unwrap_or("").to_string();
                let g_name = g["name"].as_str().unwrap_or("Servidor").to_string();
                let g_icon_hash = g["icon"].as_str().map(|s| s.to_string());

                if !g_id.is_empty() {
                    if first_guild_id_opt.is_none() {
                        first_guild_id_opt = Some(g_id.clone());
                    }

                    let icon_str = get_guild_initials(&g_name);
                    let local_icon_path = cache_dir.join(format!("{}.png", g_id));

                    let mut icon_path_opt = None;
                    if local_icon_path.exists() {
                        icon_path_opt = Some(local_icon_path.to_string_lossy().to_string());
                    } else if let Some(ref icon_hash) = g_icon_hash {
                        let icon_url = format!("https://cdn.discordapp.com/icons/{}/{}.png?size=64", g_id, icon_hash);
                        pending_icon_downloads.push((g_id.clone(), icon_url));
                    }

                    raw_guilds.push(RawGuildItem {
                        id: g_id.clone(),
                        name: g_name.clone(),
                        icon: icon_str,
                        icon_path: icon_path_opt,
                    });

                    let g_data = GuildData {
                        id: g_id.clone(),
                        name: g_name,
                        channels: Vec::new(),
                    };
                    guilds_map.lock().unwrap().insert(g_id, g_data);
                }
            }

            let app_w = app_weak.clone();
            let raw_guilds_clone = raw_guilds.clone();

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    let ui_guilds: Vec<GuildItem> = raw_guilds_clone.into_iter().map(|raw| {
                        let mut has_image = false;
                        let mut icon_image = Image::default();
                        if let Some(ref path_str) = raw.icon_path {
                            if let Ok(img) = Image::load_from_path(std::path::Path::new(path_str)) {
                                has_image = true;
                                icon_image = img;
                            }
                        }
                        GuildItem {
                            id: raw.id.into(),
                            name: raw.name.into(),
                            icon: raw.icon.into(),
                            has_image,
                            icon_image,
                        }
                    }).collect();

                    let model = std::rc::Rc::new(slint::VecModel::from(ui_guilds));
                    ui.set_guilds(model.into());
                }
            });

            // Automatically select the first guild and fetch its channels & messages
            if let Some(first_gid) = first_guild_id_opt {
                fetch_and_populate_channels(
                    http,
                    app_weak.clone(),
                    guilds_map,
                    active_guild_id,
                    active_channel_id,
                    &first_gid,
                ).await;
            }

            // Download custom server icons asynchronously in the background
            if !pending_icon_downloads.is_empty() {
                let http_dl = http.clone();
                let app_w_dl = app_weak.clone();

                tokio::spawn(async move {
                    for (gid, url) in pending_icon_downloads {
                        if let Ok(bytes) = http_dl.download_image(&url).await {
                            let icon_file = std::path::Path::new(".litecord_cache/icons").join(format!("{}.png", gid));
                            let _ = std::fs::write(&icon_file, &bytes);

                            let app_w_inner = app_w_dl.clone();
                            let gid_clone = gid.clone();
                            let icon_path_str = icon_file.to_string_lossy().to_string();

                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = app_w_inner.upgrade() {
                                    if let Ok(img) = Image::load_from_path(std::path::Path::new(&icon_path_str)) {
                                        let mut current_g: Vec<GuildItem> = ui.get_guilds().iter().collect();
                                        if let Some(item) = current_g.iter_mut().find(|g| g.id == gid_clone) {
                                            item.has_image = true;
                                            item.icon_image = img;
                                        }
                                        let model = std::rc::Rc::new(slint::VecModel::from(current_g));
                                        ui.set_guilds(model.into());
                                    }
                                }
                            });
                        }
                    }
                });
            }
        }
        Err(e) => {
            error!("Erro ao buscar servidores via REST: {}", e);
        }
    }
}

async fn fetch_and_populate_channels(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    guild_id: &str,
) {
    info!("Buscando canais do servidor {} via REST API...", guild_id);
    *active_guild_id.lock().unwrap() = guild_id.to_string();

    let g_name = guilds_map.lock().unwrap()
        .get(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Servidor".to_string());

    let app_w_top = app_weak.clone();
    let g_name_top = g_name.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_w_top.upgrade() {
            ui.set_connection_status(format!("Servidor: {} | Gateway v9 (Online)", g_name_top).into());
        }
    });

    match http.get_guild_channels(guild_id).await {
        Ok(chans_json) => {
            info!("{} canais encontrados no servidor!", chans_json.len());
            let mut channels_data: Vec<ChannelData> = Vec::new();

            for ch in chans_json {
                let ch_id = ch["id"].as_str().unwrap_or("").to_string();
                let ch_name = ch["name"].as_str().unwrap_or("canal").to_string();
                let ch_type = ch["type"].as_u64().unwrap_or(0);

                // Ignore Category headers (type 4); include text (0), voice (2), news (5), stage (13), forum (15), threads (10, 11, 12)
                if ch_type != 4 {
                    let is_voice = ch_type == 2 || ch_type == 13;
                    channels_data.push(ChannelData {
                        id: ch_id,
                        name: ch_name,
                        is_voice,
                    });
                }
            }

            // Update channels in guilds_map
            if let Some(g_data) = guilds_map.lock().unwrap().get_mut(guild_id) {
                g_data.channels = channels_data.clone();
            }

            let ui_channels: Vec<ChannelItem> = channels_data.iter().map(|ch| {
                ChannelItem {
                    id: ch.id.clone().into(),
                    name: ch.name.clone().into(),
                    is_voice: ch.is_voice,
                }
            }).collect();

            let app_w = app_weak.clone();
            let ui_channels_clone = ui_channels.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_channels_clone));
                    ui.set_channels(model.into());
                }
            });

            // Try to find the first readable text channel automatically
            let text_channels: Vec<&ChannelData> = channels_data.iter().filter(|c| !c.is_voice).collect();
            let mut loaded_readable = false;

            for text_ch in text_channels {
                let ch_id = text_ch.id.clone();
                let ch_name = text_ch.name.clone();

                if load_messages_for_channel(http, app_weak.clone(), &ch_id).await {
                    *active_channel_id.lock().unwrap() = ch_id;
                    let app_w2 = app_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w2.upgrade() {
                            ui.set_active_channel_name(format!("# {}", ch_name).into());
                        }
                    });
                    loaded_readable = true;
                    break;
                }
            }

            if !loaded_readable {
                if let Some(ui) = app_weak.upgrade() {
                    ui.set_active_channel_name("Nenhum canal de texto acessível".into());
                }
            }
        }
        Err(e) => {
            error!("Erro ao buscar canais do servidor via REST: {}", e);
        }
    }
}

async fn load_messages_for_channel(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    channel_id: &str,
) -> bool {
    info!("Carregando mensagens do canal {}...", channel_id);
    match http.get_channel_messages(channel_id).await {
        Ok(msgs_val) => {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    let ui_msgs: Vec<ChatMessage> = if msgs_val.is_empty() {
                        vec![ChatMessage {
                            author: "Litecord System".into(),
                            content: "Este canal está vazio ou não possui mensagens recentes.".into(),
                            embed_content: "".into(),
                            embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                            embed_footer: "".into(),
                            code_block: "".into(),
                            links: slint::ModelRc::default(),
                            timestamp: "Agora".into(),
                        }]
                    } else {
                        msgs_val.iter().rev().map(|m| {
                            let author = format_discord_author(m);
                            let (content, embed_content, embed_color, embed_footer, code_block, links) = format_discord_message_parts(m);
                            
                            let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
                                label: l.label.clone().into(),
                                url: l.url.clone().into(),
                            }).collect();
                            let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

                            ChatMessage {
                                author: author.into(),
                                content: content.into(),
                                embed_content: embed_content.into(),
                                embed_color: parse_hex_color(&embed_color),
                                embed_footer: embed_footer.into(),
                                code_block: code_block.into(),
                                links: slint::ModelRc::from(links_model),
                                timestamp: "Agora".into(),
                            }
                        }).collect()
                    };
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_msgs));
                    ui.set_messages(model.into());
                }
            });
            true
        }
        Err(err) => {
            error!("Erro ao carregar mensagens do canal {}: {}", channel_id, err);
            let friendly_msg = if err.contains("403") || err.contains("Forbidden") {
                "🔒 Canal Privado\nEste canal é restrito e exige cargos específicos no servidor para visualizar as mensagens.".to_string()
            } else {
                format!("⚠️ Não foi possível carregar as mensagens ({})", err)
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    let ui_msgs = vec![ChatMessage {
                        author: "Litecord System".into(),
                        content: friendly_msg.into(),
                        embed_content: "".into(),
                        embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                        embed_footer: "".into(),
                        code_block: "".into(),
                        links: slint::ModelRc::default(),
                        timestamp: "Agora".into(),
                    }];
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_msgs));
                    ui.set_messages(model.into());
                }
            });
            false
        }
    }
}

struct MultiWriter {
    file: std::sync::Mutex<std::fs::File>,
}

impl std::io::Write for MultiWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stdout().write(buf);
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stdout().flush();
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(log_file) = std::fs::File::create("litecord_app.log") {
        let writer = MultiWriter { file: std::sync::Mutex::new(log_file) };
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .target(env_logger::Target::Pipe(Box::new(writer)))
            .init();
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }
    info!("Iniciando Litecord v0.1.0...");

    let app = AppWindow::new()?;
    let app_weak = app.as_weak();

    let hwnd_store: Arc<Mutex<Option<isize>>> = Arc::new(Mutex::new(None));

    use i_slint_backend_winit::WinitWindowAccessor;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    app.window().with_winit_window(|winit_win| {
        if let Ok(handle) = winit_win.window_handle() {
            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                let hwnd = win32_handle.hwnd.get() as isize;
                *hwnd_store.lock().unwrap() = Some(hwnd);
                set_dark_titlebar_color(hwnd);
            }
        }
    });

    let last_token: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let guilds_map: Arc<Mutex<HashMap<String, GuildData>>> = Arc::new(Mutex::new(HashMap::new()));
    let cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>> = Arc::new(Mutex::new(None));

    let selected_input: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let selected_output: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let active_mic_stream: Arc<Mutex<Option<cpal::Stream>>> = Arc::new(Mutex::new(None));

    let tray = SystemTrayManager::setup();
    let show_id = tray.show_item_id.clone();
    let quit_id = tray.quit_item_id.clone();

    // Spawn tray event listener thread
    let hwnd_store_tray = Arc::clone(&hwnd_store);
    tokio::task::spawn_blocking(move || {
        let menu_rx = MenuEvent::receiver();
        let tray_rx = TrayIconEvent::receiver();

        loop {
            if let Ok(event) = menu_rx.try_recv() {
                if event.id == show_id {
                    if let Some(hwnd) = *hwnd_store_tray.lock().unwrap() {
                        APP_IS_VISIBLE.store(true, Ordering::Relaxed);
                        NEED_UI_REFRESH.store(true, Ordering::Relaxed);
                        unsafe {
                            ShowWindow(hwnd as _, SW_SHOW);
                            ShowWindow(hwnd as _, SW_RESTORE);
                            SetForegroundWindow(hwnd as _);
                        }
                        info!("[DeepSleep] Janela restaurada — acordando UI.");
                    }
                } else if event.id == quit_id {
                    std::process::exit(0);
                }
            }

            if let Ok(event) = tray_rx.try_recv() {
                if matches!(event, TrayIconEvent::DoubleClick { .. } | TrayIconEvent::Click { button: MouseButton::Left, .. }) {
                    if let Some(hwnd) = *hwnd_store_tray.lock().unwrap() {
                        APP_IS_VISIBLE.store(true, Ordering::Relaxed);
                        NEED_UI_REFRESH.store(true, Ordering::Relaxed);
                        unsafe {
                            ShowWindow(hwnd as _, SW_SHOW);
                            ShowWindow(hwnd as _, SW_RESTORE);
                            SetForegroundWindow(hwnd as _);
                        }
                        info!("[DeepSleep] Janela restaurada via clique no tray — acordando UI.");
                    }
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });

    let http_client: Arc<Mutex<Option<DiscordHttpClient>>> = Arc::new(Mutex::new(None));
    let active_channel_id: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let active_guild_id: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    // Tokio MPSC Channel for Gateway Events -> Slint UI
    let (event_tx, mut event_rx) = mpsc::channel::<GatewayEvent>(100);

    // Tokio MPSC Channel for Microphone Volume Level (0.0 to 1.0)
    let (level_tx, mut level_rx) = mpsc::channel::<f32>(100);

    // Dispatch Microphone Volume Level to Slint UI Thread
    // Skipped when window is hidden to save CPU.
    let app_weak_level = app_weak.clone();
    tokio::spawn(async move {
        while let Some(level) = level_rx.recv().await {
            // Deep-sleep guard: drain the channel without invoking Slint
            if !APP_IS_VISIBLE.load(Ordering::Relaxed) {
                continue;
            }
            let app_w_inner = app_weak_level.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w_inner.upgrade() {
                    ui.set_mic_level(level);
                }
            });
        }
    });

    // Voice Room View Focus Toggle & Per-User Audio Callbacks
    let app_weak_voice_focus = app_weak.clone();
    app.on_toggle_voice_focus(move || {
        if let Some(ui) = app_weak_voice_focus.upgrade() {
            let cur = ui.get_is_voice_focused();
            ui.set_is_voice_focused(!cur);
        }
    });

    app.on_open_link(move |url_str: SharedString| {
        let url = url_str.to_string();
        info!("Abrindo link no navegador: {}", url);
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd")
            .args(&["/C", "start", &url])
            .spawn();
    });

    app.on_toggle_user_mute(move |uid_str: SharedString| {
        let uid = uid_str.to_string();
        let (is_m, _vol) = gateway::get_user_mute_volume(&uid);
        gateway::set_user_mute(&uid, !is_m);
        info!("🎙️ Mute do participante {} alterado para: {}", uid, !is_m);
    });

    app.on_set_user_volume(move |uid_str: SharedString, vol: f32| {
        let uid = uid_str.to_string();
        gateway::set_user_volume(&uid, vol);
    });

    // Dispatch Live Voice Room Participants & Animated Volume Level Bars
    let app_weak_voice_loop = app_weak.clone();
    let http_client_voice_loop = Arc::clone(&http_client);
    let active_channel_voice_loop = Arc::clone(&active_channel_id);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            interval.tick().await;

            // Deep-sleep guard: skip all RMS calculation and UI invoke when hidden.
            if !APP_IS_VISIBLE.load(Ordering::Relaxed) {
                continue;
            }

            // On-restore refresh: re-fetch active channel messages via REST
            if NEED_UI_REFRESH.compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                let pending = PENDING_MESSAGES.swap(0, Ordering::Relaxed);
                let ch_id = active_channel_voice_loop.lock().unwrap().clone();
                if !ch_id.is_empty() {
                    let http_opt = http_client_voice_loop.lock().unwrap().clone();
                    if let Some(http) = http_opt {
                        let app_w_refresh = app_weak_voice_loop.clone();
                        info!("[DeepSleep] Restaurado - re-sincronizando {} mensagem(s) pendente(s) do canal {}.", pending, ch_id);
                        tokio::spawn(async move {
                            load_messages_for_channel(&http, app_w_refresh, &ch_id).await;
                        });
                    }
                }
            }

            let app_w_inner = app_weak_voice_loop.clone();

            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = app_w_inner.upgrade() else { return; };
                if !ui.get_is_in_voice() { return; }

                let active_parts = gateway::get_active_voice_participants_store();
                let queues_arc = gateway::get_speaker_pcm_queues();
                let mut participants = Vec::new();

                if let Ok(parts_map) = active_parts.lock() {
                    let queues_guard = queues_arc.lock().ok();
                    let mut user_best: HashMap<u64, (u32, f32, bool)> = HashMap::new();

                    for (&ssrc, &user_id) in parts_map.iter() {
                        if ssrc == 999999 || user_id == 999999 { continue; }

                        let mut audio_level = 0.0f32;
                        let mut is_speaking = false;

                        if let Some(ref queues) = queues_guard {
                            if let Some(q) = queues.get(&ssrc) {
                                let sample_cnt = q.len().min(480);
                                if sample_cnt > 0 {
                                    let mut sum_sq = 0.0f32;
                                    for i in 0..sample_cnt {
                                        let (l, r) = q[i];
                                        let s = (l + r) * 0.5;
                                        sum_sq += s * s;
                                    }
                                    let rms = (sum_sq / sample_cnt as f32).sqrt();
                                    audio_level = if rms < 0.005 { 0.0 } else { (rms * 3.5).clamp(0.0, 1.0) };
                                    is_speaking = audio_level > 0.03;
                                }
                            }
                        }

                        let entry = user_best.entry(user_id).or_insert((ssrc, audio_level, is_speaking));
                        if audio_level > entry.1 {
                            *entry = (ssrc, audio_level, is_speaking);
                        }
                    }

                    let mut sorted_users: Vec<_> = user_best.into_iter().collect();
                    sorted_users.sort_by(|(id_a, _), (id_b, _)| {
                        let name_a = gateway::get_user_name(*id_a);
                        let name_b = gateway::get_user_name(*id_b);
                        name_a.cmp(&name_b)
                    });

                    for (user_id, (_ssrc, audio_level, is_speaking)) in sorted_users {
                        let uid_str = user_id.to_string();
                        let username = gateway::get_user_name(user_id);
                        let avatar_text = username.chars().next().unwrap_or('U').to_uppercase().to_string();
                        let (is_muted, vol) = gateway::get_user_mute_volume(&uid_str);

                        participants.push(VoiceParticipant {
                            user_id: uid_str.into(),
                            username: username.into(),
                            avatar_text: avatar_text.into(),
                            is_speaking,
                            audio_level,
                            is_muted,
                            volume: vol,
                        });
                    }
                }

                let cur_model = ui.get_voice_participants();
                let mut need_full_rebuild = cur_model.row_count() != participants.len();
                if !need_full_rebuild {
                    for (i, p) in participants.iter().enumerate() {
                        if let Some(old) = cur_model.row_data(i) {
                            if old.user_id != p.user_id || old.username != p.username {
                                need_full_rebuild = true;
                                break;
                            }
                        } else {
                            need_full_rebuild = true;
                            break;
                        }
                    }
                }

                if need_full_rebuild {
                    let model = std::rc::Rc::new(slint::VecModel::from(participants));
                    ui.set_voice_participants(model.into());
                } else {
                    for (i, p) in participants.into_iter().enumerate() {
                        cur_model.set_row_data(i, p);
                    }
                }
            });
        }
    });

    // Voice & Audio Settings Callbacks
    let app_weak_open_settings = app_weak.clone();
    let selected_input_open = Arc::clone(&selected_input);
    let selected_output_open = Arc::clone(&selected_output);
    let level_tx_open = level_tx.clone();
    let active_stream_open = Arc::clone(&active_mic_stream);

    app.on_open_settings(move || {
        if let Some(ui) = app_weak_open_settings.upgrade() {
            info!("Buscando dispositivos de áudio via cpal WASAPI...");
            let (inputs, outputs) = enumerate_audio_devices();

            let cur_input = selected_input_open.lock().unwrap().clone();
            let cur_output = selected_output_open.lock().unwrap().clone();

            let ui_inputs: Vec<AudioDeviceItem> = inputs.iter().enumerate().map(|(idx, name)| {
                let is_sel = if cur_input.is_empty() { idx == 0 } else { name == &cur_input };
                AudioDeviceItem {
                    id: name.clone().into(),
                    name: name.clone().into(),
                    is_selected: is_sel,
                }
            }).collect();

            let ui_outputs: Vec<AudioDeviceItem> = outputs.iter().enumerate().map(|(idx, name)| {
                let is_sel = if cur_output.is_empty() { idx == 0 } else { name == &cur_output };
                AudioDeviceItem {
                    id: name.clone().into(),
                    name: name.clone().into(),
                    is_selected: is_sel,
                }
            }).collect();

            ui.set_input_devices(std::rc::Rc::new(slint::VecModel::from(ui_inputs)).into());
            ui.set_output_devices(std::rc::Rc::new(slint::VecModel::from(ui_outputs)).into());
            ui.set_show_settings_modal(true);

            // Start hardware microphone stream capture for live volume meter
            let mic_name = if cur_input.is_empty() && !inputs.is_empty() { inputs[0].clone() } else { cur_input };
            if let Some(stream) = start_mic_capture(mic_name, level_tx_open.clone()) {
                *active_stream_open.lock().unwrap() = Some(stream);
            }
        }
    });

    let app_weak_close_settings = app_weak.clone();
    let active_stream_close = Arc::clone(&active_mic_stream);
    app.on_close_settings(move || {
        if let Some(ui) = app_weak_close_settings.upgrade() {
            ui.set_show_settings_modal(false);
            *active_stream_close.lock().unwrap() = None; // Stop test stream
        }
    });

    let app_weak_select_input = app_weak.clone();
    let selected_input_store = Arc::clone(&selected_input);
    let level_tx_select = level_tx.clone();
    let active_stream_select = Arc::clone(&active_mic_stream);

    app.on_select_input_device(move |dev_name: SharedString| {
        let name_str = dev_name.to_string();
        info!("Microfone selecionado: {}", name_str);
        *selected_input_store.lock().unwrap() = name_str.clone();

        if let Some(ui) = app_weak_select_input.upgrade() {
            let current_devs: Vec<AudioDeviceItem> = ui.get_input_devices().iter().map(|mut item| {
                item.is_selected = item.name.as_str() == name_str;
                item
            }).collect();
            ui.set_input_devices(std::rc::Rc::new(slint::VecModel::from(current_devs)).into());
            ui.set_selected_input_device(name_str.clone().into());

            // Restart hardware microphone stream for newly selected device
            if let Some(stream) = start_mic_capture(name_str, level_tx_select.clone()) {
                *active_stream_select.lock().unwrap() = Some(stream);
            }
        }
    });

    let app_weak_select_output = app_weak.clone();
    let selected_output_store = Arc::clone(&selected_output);
    app.on_select_output_device(move |dev_name: SharedString| {
        let name_str = dev_name.to_string();
        info!("Alto-falante selecionado: {}", name_str);
        *selected_output_store.lock().unwrap() = name_str.clone();
        gateway::set_selected_output_device(name_str.clone());

        if let Some(ui) = app_weak_select_output.upgrade() {
            let current_devs: Vec<AudioDeviceItem> = ui.get_output_devices().iter().map(|mut item| {
                item.is_selected = item.name.as_str() == name_str;
                item
            }).collect();
            ui.set_output_devices(std::rc::Rc::new(slint::VecModel::from(current_devs)).into());
            ui.set_selected_output_device(name_str.into());
        }
    });

    // Leave Voice Callback
    let cmd_tx_leave = Arc::clone(&cmd_tx_store);
    let active_guild_leave = Arc::clone(&active_guild_id);
    let app_weak_leave = app_weak.clone();

    app.on_leave_voice(move || {
        info!("Usuário solicitou desconexão da sala de voz...");
        if let Some(ui) = app_weak_leave.upgrade() {
            ui.set_is_in_voice(false);
            ui.set_current_voice_channel("".into());
            gateway::clear_voice_participants();

            let gid = active_guild_leave.lock().unwrap().clone();

            if let Some(cmd_tx_guard) = cmd_tx_leave.lock().unwrap().as_ref() {
                let _ = cmd_tx_guard.try_send(GatewayCommand::UpdateVoiceState {
                    guild_id: gid,
                    channel_id: None,
                    self_mute: false,
                    self_deaf: false,
                });
            }

            let mut current_msgs: Vec<ChatMessage> = ui.get_messages().iter().collect();
            current_msgs.push(ChatMessage {
                author: "Litecord Voice".into(),
                content: "🔴 Desconectado da sala de voz.".into(),
                embed_content: "".into(),
                embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                embed_footer: "".into(),
                code_block: "".into(),
                links: slint::ModelRc::default(),
                timestamp: "Agora".into(),
            });
            let model = std::rc::Rc::new(slint::VecModel::from(current_msgs));
            ui.set_messages(model.into());
        }
    });

    // Login Callback from UI
    let event_tx_clone = event_tx.clone();
    let http_client_clone = Arc::clone(&http_client);
    let app_weak_login = app_weak.clone();
    let last_token_login = Arc::clone(&last_token);
    let guilds_map_login = Arc::clone(&guilds_map);
    let active_guild_login = Arc::clone(&active_guild_id);
    let active_channel_login = Arc::clone(&active_channel_id);
    let cmd_tx_login = Arc::clone(&cmd_tx_store);

    app.on_login(move |token: SharedString| {
        let token_str = token.to_string().chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();
        info!("Token fornecido. Validando via API HTTP...");
        *last_token_login.lock().unwrap() = token_str.clone();

        let http = DiscordHttpClient::new(token_str.clone());
        *http_client_clone.lock().unwrap() = Some(http.clone());

        if let Some(ui) = app_weak_login.upgrade() {
            ui.set_connection_status("Validando Token na API do Discord...".into());
        }

        let event_tx_gw = event_tx_clone.clone();
        let app_weak_inner = app_weak_login.clone();
        let guilds_map_in = Arc::clone(&guilds_map_login);
        let active_guild_in = Arc::clone(&active_guild_login);
        let active_channel_in = Arc::clone(&active_channel_login);
        let cmd_tx_gw_store = Arc::clone(&cmd_tx_login);

        tokio::spawn(async move {
            match http.get_current_user().await {
                Ok(user_info) => {
                    let username = user_info["username"].as_str().unwrap_or("User");
                    info!("Token VÁLIDO! Usuário autenticado: {}", username);
                    let _ = std::fs::write(".litecord_token", &token_str);

                    let app_w = app_weak_inner.clone();
                    let username_clone = username.to_string();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_is_logged_in(true);
                            ui.set_connection_status(format!("Conectado como {}!", username_clone).into());
                        }
                    });

                    // Fetch all servers and channels via HTTP REST immediately!
                    fetch_and_populate_guilds(
                        &http,
                        app_weak_inner.clone(),
                        guilds_map_in,
                        active_guild_in,
                        active_channel_in,
                    ).await;

                    // Start Gateway WebSocket connection with Opcode 4 command channel
                    let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(100);
                    *cmd_tx_gw_store.lock().unwrap() = Some(cmd_tx);

                    let gw = Arc::new(GatewayClient::new(token_str, event_tx_gw));
                    gw.start(cmd_rx).await;
                }
                Err(err_msg) => {
                    error!("Falha na validação do Token: {}", err_msg);
                    let app_w = app_weak_inner.clone();
                    let err_clone = err_msg.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_connection_status(format!("❌ {}", err_clone).into());
                        }
                    });
                }
            }
        });
    });

    // Callback for Auto-Detect Token Button on Login UI
    let event_tx_auto_btn = event_tx.clone();
    let http_client_auto_btn = Arc::clone(&http_client);
    let app_weak_auto_btn = app_weak.clone();
    let last_token_auto_btn = Arc::clone(&last_token);
    let guilds_map_auto_btn = Arc::clone(&guilds_map);
    let active_guild_auto_btn = Arc::clone(&active_guild_id);
    let active_channel_auto_btn = Arc::clone(&active_channel_id);
    let cmd_tx_auto_btn = Arc::clone(&cmd_tx_store);

    app.on_auto_detect_token(move || {
        let app_w = app_weak_auto_btn.clone();
        let event_tx_gw = event_tx_auto_btn.clone();
        let http_client_inner = Arc::clone(&http_client_auto_btn);
        let last_token_inner = Arc::clone(&last_token_auto_btn);
        let guilds_map_inner = Arc::clone(&guilds_map_auto_btn);
        let active_g_inner = Arc::clone(&active_guild_auto_btn);
        let active_c_inner = Arc::clone(&active_channel_auto_btn);
        let cmd_tx_inner = Arc::clone(&cmd_tx_auto_btn);

        let app_w_status = app_w.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = app_w_status.upgrade() {
                ui.set_connection_status("Detectando token do Discord no sistema...".into());
            }
        });

        tokio::spawn(async move {
            let candidates = auto_detect_discord_tokens();
            if candidates.is_empty() {
                let app_w_ui = app_w.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w_ui.upgrade() {
                        ui.set_connection_status("⚠️ Nenhum token do Discord foi encontrado no sistema.".into());
                    }
                });
            } else {
                let success = try_login_with_candidates(
                    candidates,
                    app_w.clone(),
                    http_client_inner,
                    last_token_inner,
                    guilds_map_inner,
                    active_g_inner,
                    active_c_inner,
                    cmd_tx_inner,
                    event_tx_gw,
                ).await;

                if !success {
                    let app_w_ui = app_w.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w_ui.upgrade() {
                            ui.set_connection_status("⚠️ Nenhum dos tokens encontrados foi aceito pelo Discord.".into());
                        }
                    });
                }
            }
        });
    });

    // Auto-Login if saved token file exists or if auto-detectable from Discord Desktop
    let event_tx_startup = event_tx.clone();
    let app_weak_startup = app_weak.clone();
    let http_client_startup = Arc::clone(&http_client);
    let last_token_startup = Arc::clone(&last_token);
    let guilds_map_startup = Arc::clone(&guilds_map);
    let active_guild_startup = Arc::clone(&active_guild_id);
    let active_channel_startup = Arc::clone(&active_channel_id);
    let cmd_tx_startup = Arc::clone(&cmd_tx_store);

    tokio::spawn(async move {
        let mut candidates = Vec::new();
        if let Ok(saved_token) = std::fs::read_to_string(".litecord_token") {
            let saved = saved_token.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();
            if !saved.is_empty() {
                candidates.push(saved);
            }
        }

        let detected = auto_detect_discord_tokens();
        for d in detected {
            if !candidates.contains(&d) {
                candidates.push(d);
            }
        }

        if !candidates.is_empty() {
            let app_w_status = app_weak_startup.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w_status.upgrade() {
                    ui.set_connection_status("Autoconectando à Gateway v9...".into());
                }
            });

            let success = try_login_with_candidates(
                candidates,
                app_weak_startup.clone(),
                http_client_startup,
                last_token_startup,
                guilds_map_startup,
                active_guild_startup,
                active_channel_startup,
                cmd_tx_startup,
                event_tx_startup,
            ).await;

            if !success {
                let _ = std::fs::remove_file(".litecord_token");
            }
        }
    });



    // Select Guild Callback
    let guilds_map_select = Arc::clone(&guilds_map);
    let active_guild_select = Arc::clone(&active_guild_id);
    let app_weak_guild_select = app_weak.clone();
    let http_client_guild_select = Arc::clone(&http_client);
    let active_channel_guild_select = Arc::clone(&active_channel_id);

    app.on_select_guild(move |guild_id: SharedString| {
        let gid = guild_id.to_string();
        info!("Servidor selecionado pelo usuário: {}", gid);

        let http_opt = http_client_guild_select.lock().unwrap().as_ref().cloned();
        let app_w = app_weak_guild_select.clone();
        let guilds_map_in = Arc::clone(&guilds_map_select);
        let active_g_in = Arc::clone(&active_guild_select);
        let active_c_in = Arc::clone(&active_channel_guild_select);

        if let Some(http) = http_opt {
            tokio::spawn(async move {
                fetch_and_populate_channels(
                    &http,
                    app_w,
                    guilds_map_in,
                    active_g_in,
                    active_c_in,
                    &gid,
                ).await;
            });
        }
    });

    // Select Channel Callback
    let guilds_map_chan_select = Arc::clone(&guilds_map);
    let active_guild_chan_select = Arc::clone(&active_guild_id);
    let active_channel_chan_select = Arc::clone(&active_channel_id);
    let app_weak_chan_select = app_weak.clone();
    let http_client_chan_select = Arc::clone(&http_client);
    let cmd_tx_chan_select = Arc::clone(&cmd_tx_store);
    let selected_input_voice = Arc::clone(&selected_input);
    let active_mic_stream_voice = Arc::clone(&active_mic_stream);
    let level_tx_voice = level_tx.clone();

    app.on_select_channel(move |channel_id: SharedString| {
        let ch_id = channel_id.to_string();
        info!("Canal selecionado pelo usuário: {}", ch_id);
        *active_channel_chan_select.lock().unwrap() = ch_id.clone();

        let gid = active_guild_chan_select.lock().unwrap().clone();
        let target_channel = guilds_map_chan_select.lock().unwrap()
            .get(&gid)
            .and_then(|g| g.channels.iter().find(|c| c.id == ch_id).cloned());

        let is_voice = target_channel.as_ref().map(|c| c.is_voice).unwrap_or(false);
        let ch_name = target_channel.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| "canal".to_string());

        if let Some(ui) = app_weak_chan_select.upgrade() {
            if is_voice {
                ui.set_is_in_voice(true);
                ui.set_is_voice_focused(true);
                ui.set_current_voice_channel(format!("🔊 {}", ch_name).into());
                let muted = ui.get_is_muted();
                let deafened = ui.get_is_deafened();

                // 1. Send Opcode 4 VoiceStateUpdate to Discord Gateway WebSocket!
                if let Some(cmd_tx_guard) = cmd_tx_chan_select.lock().unwrap().as_ref() {
                    let _ = cmd_tx_guard.try_send(GatewayCommand::UpdateVoiceState {
                        guild_id: gid.clone(),
                        channel_id: Some(ch_id.clone()),
                        self_mute: muted,
                        self_deaf: deafened,
                    });
                }


                // 3. Auto-start microphone capture for voice channel (feeds PCM to gateway queue)
                let mic_name = selected_input_voice.lock().unwrap().clone();
                if let Some(stream) = start_mic_capture(mic_name, level_tx_voice.clone()) {
                    *active_mic_stream_voice.lock().unwrap() = Some(stream);
                    info!("Microfone ativado automaticamente para o canal de voz!");
                }
                
                let mut current_msgs: Vec<ChatMessage> = ui.get_messages().iter().collect();
                current_msgs.push(ChatMessage {
                    author: "Litecord Voice".into(),
                    content: format!("🔊 Entrou no canal de voz: {}", ch_name).into(),
                    embed_content: "".into(),
                    embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                    embed_footer: "".into(),
                    code_block: "".into(),
                    links: slint::ModelRc::default(),
                    timestamp: "Agora".into(),
                });
                let model = std::rc::Rc::new(slint::VecModel::from(current_msgs));
                ui.set_messages(model.into());
            } else {
                ui.set_active_channel_name(format!("# {}", ch_name).into());

                let http_opt = http_client_chan_select.lock().unwrap().as_ref().cloned();
                let app_w = app_weak_chan_select.clone();
                let ch_id_clone = ch_id.clone();

                if let Some(http) = http_opt {
                    tokio::spawn(async move {
                        load_messages_for_channel(&http, app_w, &ch_id_clone).await;
                    });
                }
            }
        }
    });

    // Mute Button Toggle Callback
    let cmd_tx_mute = Arc::clone(&cmd_tx_store);
    let active_guild_mute = Arc::clone(&active_guild_id);
    let active_channel_mute = Arc::clone(&active_channel_id);
    let app_weak_mute = app_weak.clone();

    app.on_toggle_mute(move || {
        if let Some(ui) = app_weak_mute.upgrade() {
            let new_muted = !ui.get_is_muted();
            ui.set_is_muted(new_muted);

            let gid = active_guild_mute.lock().unwrap().clone();
            let cid = active_channel_mute.lock().unwrap().clone();
            let deafened = ui.get_is_deafened();

            if let Some(cmd_tx_guard) = cmd_tx_mute.lock().unwrap().as_ref() {
                let _ = cmd_tx_guard.try_send(GatewayCommand::UpdateVoiceState {
                    guild_id: gid,
                    channel_id: Some(cid),
                    self_mute: new_muted,
                    self_deaf: deafened,
                });
            }
        }
    });

    // Deafen Button Toggle Callback
    let cmd_tx_deafen = Arc::clone(&cmd_tx_store);
    let active_guild_deafen = Arc::clone(&active_guild_id);
    let active_channel_deafen = Arc::clone(&active_channel_id);
    let app_weak_deafen = app_weak.clone();

    app.on_toggle_deafen(move || {
        if let Some(ui) = app_weak_deafen.upgrade() {
            let new_deafened = !ui.get_is_deafened();
            ui.set_is_deafened(new_deafened);

            let gid = active_guild_deafen.lock().unwrap().clone();
            let cid = active_channel_deafen.lock().unwrap().clone();
            let muted = ui.get_is_muted();

            if let Some(cmd_tx_guard) = cmd_tx_deafen.lock().unwrap().as_ref() {
                let _ = cmd_tx_guard.try_send(GatewayCommand::UpdateVoiceState {
                    guild_id: gid,
                    channel_id: Some(cid),
                    self_mute: muted,
                    self_deaf: new_deafened,
                });
            }
        }
    });

    // Send Message Callback from UI
    let http_client_msg = Arc::clone(&http_client);
    let active_channel_msg = Arc::clone(&active_channel_id);
    app.on_send_message(move |content: SharedString| {
        let content_str = content.to_string();
        let channel_id = active_channel_msg.lock().unwrap().clone();
        let http_opt = http_client_msg.lock().unwrap().as_ref().cloned();

        if let Some(http) = http_opt {
            tokio::spawn(async move {
                let _ = http.send_message(&channel_id, &content_str).await;
            });
        }
    });

    // Minimize to Tray Callback using Win32 ShowWindow(SW_HIDE)
    let hwnd_store_minimize = Arc::clone(&hwnd_store);
    app.on_minimize_to_tray(move || {
        info!("Minimizando janela para a bandeja do sistema via Win32 SW_HIDE...");
        // Signal all UI loops to enter deep sleep
        APP_IS_VISIBLE.store(false, Ordering::Relaxed);
        PENDING_MESSAGES.store(0, Ordering::Relaxed);
        info!("[DeepSleep] UI loops suspensos. Apenas áudio permanece ativo.");
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                *hwnd_store_minimize.lock().unwrap() = Some(hwnd as isize);
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    });

    // Handle Gateway Events in Tokio Task and Dispatch to Slint UI Thread
    let app_weak_gw_events = app_weak.clone();
    let last_token_gw_save = Arc::clone(&last_token);

    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let app_weak_inner = app_weak_gw_events.clone();
            let last_token_inner = Arc::clone(&last_token_gw_save);

            match event {
                GatewayEvent::Connected { user_tag } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak_inner.upgrade() {
                            ui.set_is_logged_in(true);
                            ui.set_user_tag(user_tag.into());
                            if let Ok(tok) = last_token_inner.lock() {
                                let _ = std::fs::write(".litecord_token", tok.as_str());
                            }
                        }
                    });
                }
                GatewayEvent::Disconnected { reason } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak_inner.upgrade() {
                            ui.set_is_logged_in(false);
                            ui.set_connection_status(format!("❌ Desconectado: {}", reason).into());
                        }
                    });
                }
                GatewayEvent::MessageCreated { author, content, embed_content, embed_color, embed_footer, code_block, links, timestamp, .. } => {
                    if !APP_IS_VISIBLE.load(Ordering::Relaxed) {
                        // Window is hidden — count message but don't touch Slint.
                        // Messages will be re-fetched via REST when the window is restored.
                        let pending = PENDING_MESSAGES.fetch_add(1, Ordering::Relaxed) + 1;
                        info!("[DeepSleep] Mensagem recebida em background ({} pendente(s)) — UI ignorada.", pending);
                    } else {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_weak_inner.upgrade() {
                                let mut current_msgs: Vec<ChatMessage> = ui.get_messages().iter().collect();

                                let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
                                    label: l.label.clone().into(),
                                    url: l.url.clone().into(),
                                }).collect();
                                let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

                                current_msgs.push(ChatMessage {
                                    author: author.into(),
                                    content: content.into(),
                                    embed_content: embed_content.into(),
                                    embed_color: parse_hex_color(&embed_color),
                                    embed_footer: embed_footer.into(),
                                    code_block: code_block.into(),
                                    links: slint::ModelRc::from(links_model),
                                    timestamp: timestamp.into(),
                                });
                                let model = std::rc::Rc::new(slint::VecModel::from(current_msgs));
                                ui.set_messages(model.into());
                            }
                        });
                    }
                }
                GatewayEvent::GuildLoaded { .. } => {
                    // Handled via HTTP REST instant fetching
                }
            }
        }
    });

    app.run()?;
    Ok(())
}

#[repr(C)]
struct DATA_BLOB {
    cbData: u32,
    pbData: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptUnprotectData(
        pDataIn: *const DATA_BLOB,
        ppszDataDescr: *mut *mut u16,
        pOptionalEntropy: *const DATA_BLOB,
        pvReserved: *mut std::ffi::c_void,
        pPromptStruct: *mut std::ffi::c_void,
        dwFlags: u32,
        pDataOut: *mut DATA_BLOB,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let mut in_blob = DATA_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = DATA_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let res = unsafe {
        CryptUnprotectData(
            &mut in_blob,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out_blob,
        )
    };

    if res != 0 && !out_blob.pbData.is_null() {
        let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
        let result = slice.to_vec();
        unsafe { LocalFree(out_blob.pbData as _) };
        Some(result)
    } else {
        None
    }
}

fn is_valid_token_chars(token: &str) -> bool {
    token.len() >= 50 && token.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
}

#[cfg(target_os = "windows")]
fn set_dark_titlebar_color(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };

    let dark_mode: u32 = 1;
    unsafe {
        // Attribute 20 (Win11 / Win10 20H1+)
        let _ = DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_USE_IMMERSIVE_DARK_MODE as _,
            &dark_mode as *const u32 as _,
            std::mem::size_of::<u32>() as u32,
        );

        // Attribute 19 (older Win10 1903-1909 builds)
        let _ = DwmSetWindowAttribute(
            hwnd as _,
            19,
            &dark_mode as *const u32 as _,
            std::mem::size_of::<u32>() as u32,
        );

        // Header color #111214 (BGR COLORREF: 0x00141211)
        let caption_color: u32 = 0x00141211;
        let _ = DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_CAPTION_COLOR as _,
            &caption_color as *const u32 as _,
            std::mem::size_of::<u32>() as u32,
        );

        // Header text color White #FFFFFF (BGR COLORREF: 0x00FFFFFF)
        let text_color: u32 = 0x00FFFFFF;
        let _ = DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_TEXT_COLOR as _,
            &text_color as *const u32 as _,
            std::mem::size_of::<u32>() as u32,
        );

        // Force DWM to re-calculate and redraw the non-client title bar frame immediately!
        SetWindowPos(
            hwnd as _,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
    }
}

fn auto_detect_discord_tokens() -> Vec<String> {
    use base64::Engine;
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    let mut tokens = Vec::new();
    let appdata = match std::env::var("APPDATA") {
        Ok(v) => v,
        Err(_) => return tokens,
    };
    let temp_dir = std::env::temp_dir();

    let discord_paths = vec![
        format!("{}/discord", appdata),
        format!("{}/discordcanary", appdata),
        format!("{}/discordptb", appdata),
        format!("{}/Lightcord", appdata),
    ];

    for path in &discord_paths {
        let local_state_path = format!("{}/Local State", path);
        if let Ok(content) = std::fs::read_to_string(&local_state_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(enc_key_b64) = v["os_crypt"]["encrypted_key"].as_str() {
                    if let Ok(key_raw) = base64::engine::general_purpose::STANDARD.decode(enc_key_b64) {
                        if key_raw.starts_with(b"DPAPI") {
                            if let Some(master_key) = dpapi_unprotect(&key_raw[5..]) {
                                if master_key.len() == 32 {
                                    let leveldb_dir = format!("{}/Local Storage/leveldb", path);
                                    if let Ok(entries) = std::fs::read_dir(&leveldb_dir) {
                                        let mut files: Vec<std::path::PathBuf> = entries
                                            .flatten()
                                            .map(|e| e.path())
                                            .filter(|p| {
                                                let fname = p.file_name().unwrap_or_default().to_string_lossy();
                                                fname.ends_with(".ldb") || fname.ends_with(".log")
                                            })
                                            .collect();

                                        files.sort_by(|a, b| {
                                            let time_a = a.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                            let time_b = b.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                            time_b.cmp(&time_a)
                                        });

                                        for file_path in files {
                                            let fname = file_path.file_name().unwrap_or_default().to_string_lossy().to_string();
                                            let temp_file = temp_dir.join(format!("litecord_tmp_{}", fname));
                                            if std::fs::copy(&file_path, &temp_file).is_ok() {
                                                if let Ok(bytes) = std::fs::read(&temp_file) {
                                                    let _ = std::fs::remove_file(&temp_file);
                                                    let text = String::from_utf8_lossy(&bytes);
                                                    for chunk in text.split("dQw4w9WgXcQ:") {
                                                        let enc_b64: String = chunk
                                                            .chars()
                                                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '=')
                                                            .collect();
                                                        if enc_b64.len() > 30 {
                                                            if let Ok(full_enc) = base64::engine::general_purpose::STANDARD.decode(&enc_b64) {
                                                                if full_enc.len() > 31 {
                                                                    let nonce = &full_enc[3..15];
                                                                    let ciphertext = &full_enc[15..];
                                                                    if let Ok(cipher) = Aes256Gcm::new_from_slice(&master_key) {
                                                                        let nonce_obj = Nonce::from_slice(nonce);
                                                                        if let Ok(decrypted) = cipher.decrypt(nonce_obj, ciphertext) {
                                                                            if let Ok(token) = String::from_utf8(decrypted) {
                                                                                let token_clean = token.trim().to_string();
                                                                                if is_valid_token_chars(&token_clean) && !tokens.contains(&token_clean) {
                                                                                    info!("Candidato a token do Discord encontrado!");
                                                                                    tokens.push(token_clean);
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    let _ = std::fs::remove_file(&temp_file);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    tokens
}

async fn try_login_with_candidates(
    candidates: Vec<String>,
    app_weak: slint::Weak<AppWindow>,
    http_client: Arc<Mutex<Option<DiscordHttpClient>>>,
    last_token: Arc<Mutex<String>>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
    event_tx_gw: mpsc::Sender<GatewayEvent>,
) -> bool {
    for token_str in candidates {
        info!("Testando candidato a token via Discord REST API...");
        let http = DiscordHttpClient::new(token_str.clone());
        match http.get_current_user().await {
            Ok(user_info) => {
                let username = user_info["username"].as_str().unwrap_or("User");
                info!("Token VÁLIDO ENCONTRADO! Conectado como {}", username);

                let _ = std::fs::write(".litecord_token", &token_str);
                *last_token.lock().unwrap() = token_str.clone();
                *http_client.lock().unwrap() = Some(http.clone());

                let app_w = app_weak.clone();
                let username_clone = username.to_string();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w.upgrade() {
                        ui.set_is_logged_in(true);
                        ui.set_connection_status(format!("Conectado como {}!", username_clone).into());
                    }
                });

                fetch_and_populate_guilds(
                    &http,
                    app_weak.clone(),
                    guilds_map,
                    active_guild_id,
                    active_channel_id,
                ).await;

                let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(100);
                *cmd_tx_store.lock().unwrap() = Some(cmd_tx);

                let gw = Arc::new(GatewayClient::new(token_str, event_tx_gw));
                gw.start(cmd_rx).await;

                return true;
            }
            Err(err_msg) => {
                info!("Candidato a token recusado pelo Discord ({}). Testando próximo candidato...", err_msg);
            }
        }
    }
    false
}


