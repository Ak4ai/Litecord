mod gateway;
mod http;
mod tray;

use gateway::{GatewayClient, GatewayEvent, GatewayCommand, GuildData, ChannelData, format_discord_author, format_discord_message};
use http::DiscordHttpClient;
use tray::SystemTrayManager;

use slint::{SharedString, Model, Image};
use std::sync::{Arc, Mutex};
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
            ui.set_connection_status(format!("🛡️ Servidor: {} | Gateway v9 (Online)", g_name_top).into());
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
            let ui_msgs: Vec<ChatMessage> = if msgs_val.is_empty() {
                vec![ChatMessage {
                    author: "Litecord System".into(),
                    content: "Este canal está vazio ou não possui mensagens recentes.".into(),
                    timestamp: "Agora".into(),
                }]
            } else {
                msgs_val.iter().rev().map(|m| {
                    let author = format_discord_author(m);
                    let content = format_discord_message(m);
                    ChatMessage {
                        author: author.into(),
                        content: content.into(),
                        timestamp: "Agora".into(),
                    }
                }).collect()
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    info!("Iniciando Litecord v0.1.0...");

    let app = AppWindow::new()?;
    let app_weak = app.as_weak();

    let hwnd_store: Arc<Mutex<Option<isize>>> = Arc::new(Mutex::new(None));
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
                        unsafe {
                            ShowWindow(hwnd as _, SW_SHOW);
                            ShowWindow(hwnd as _, SW_RESTORE);
                            SetForegroundWindow(hwnd as _);
                        }
                    }
                } else if event.id == quit_id {
                    std::process::exit(0);
                }
            }

            if let Ok(event) = tray_rx.try_recv() {
                if matches!(event, TrayIconEvent::DoubleClick { .. } | TrayIconEvent::Click { button: MouseButton::Left, .. }) {
                    if let Some(hwnd) = *hwnd_store_tray.lock().unwrap() {
                        unsafe {
                            ShowWindow(hwnd as _, SW_SHOW);
                            ShowWindow(hwnd as _, SW_RESTORE);
                            SetForegroundWindow(hwnd as _);
                        }
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
    let app_weak_level = app_weak.clone();
    tokio::spawn(async move {
        while let Some(level) = level_rx.recv().await {
            let app_w_inner = app_weak_level.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w_inner.upgrade() {
                    ui.set_mic_level(level);
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

    // Auto-Login if saved token file exists
    if let Ok(saved_token) = std::fs::read_to_string(".litecord_token") {
        let saved_token = saved_token.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();
        if !saved_token.is_empty() {
            info!("Token salvo encontrado em '.litecord_token'. Autoconectando...");
            *last_token.lock().unwrap() = saved_token.clone();
            let http = DiscordHttpClient::new(saved_token.clone());
            *http_client.lock().unwrap() = Some(http.clone());
            app.set_connection_status("Autoconectando à Gateway v9...".into());

            let event_tx_gw = event_tx.clone();
            let app_weak_auto = app_weak.clone();
            let guilds_map_auto = Arc::clone(&guilds_map);
            let active_guild_auto = Arc::clone(&active_guild_id);
            let active_channel_auto = Arc::clone(&active_channel_id);
            let cmd_tx_auto = Arc::clone(&cmd_tx_store);

            tokio::spawn(async move {
                match http.get_current_user().await {
                    Ok(user_info) => {
                        let username = user_info["username"].as_str().unwrap_or("User");
                        info!("Token salvo é VÁLIDO! Usuário: {}", username);

                        let app_w = app_weak_auto.clone();
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
                            app_weak_auto.clone(),
                            guilds_map_auto,
                            active_guild_auto,
                            active_channel_auto,
                        ).await;

                        let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(100);
                        *cmd_tx_auto.lock().unwrap() = Some(cmd_tx);

                        let gw = Arc::new(GatewayClient::new(saved_token, event_tx_gw));
                        gw.start(cmd_rx).await;
                    }
                    Err(err_msg) => {
                        error!("Token salvo é inválido: {}", err_msg);
                        let _ = std::fs::remove_file(".litecord_token");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_weak_auto.upgrade() {
                                ui.set_connection_status(format!("❌ {}", err_msg).into());
                            }
                        });
                    }
                }
            });
        }
    }

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

                // 2. Send REST PATCH to /guilds/{guild_id}/voice-states/@me to clear suppression
                let http_opt = http_client_chan_select.lock().unwrap().as_ref().cloned();
                let gid_rest = gid.clone();
                let ch_id_rest = ch_id.clone();
                if let Some(http) = http_opt {
                    tokio::spawn(async move {
                        let _ = http.update_my_voice_state(&gid_rest, &ch_id_rest).await;
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
                GatewayEvent::MessageCreated { author, content, timestamp, .. } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak_inner.upgrade() {
                            let mut current_msgs: Vec<ChatMessage> = ui.get_messages().iter().collect();
                            current_msgs.push(ChatMessage {
                                author: author.into(),
                                content: content.into(),
                                timestamp: timestamp.into(),
                            });
                            
                            let model = std::rc::Rc::new(slint::VecModel::from(current_msgs));
                            ui.set_messages(model.into());
                        }
                    });
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
