#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gateway;
mod http;
mod tray;
mod i18n;
mod remote_auth;
mod updater;
mod screen_capture;
mod emoji_cache;
mod attachment_cache;

use gateway::{GatewayClient, GatewayEvent, GatewayCommand, GuildData, ChannelData, format_discord_author, format_discord_message_parts};
use http::DiscordHttpClient;
use tray::SystemTrayManager;
use remote_auth::RemoteAuthEvent;
use screen_capture::ScreenCaptureManager;

use slint::{SharedString, Model, Image, SharedPixelBuffer, Rgba8Pixel};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::collections::{HashMap, VecDeque};
use tokio::sync::mpsc;
use log::{info, warn, error};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
use tray_icon::{TrayIconEvent, menu::MenuEvent, MouseButton};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ShowWindow, SetForegroundWindow, SetWindowPos,
    SW_HIDE, SW_SHOW, SW_RESTORE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, HWND_TOP
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

fn apply_i18n_translations(ui: &AppWindow, lang: i18n::Language) {
    let resolved = match lang {
        i18n::Language::Auto => i18n::detect_os_language(),
        specific => specific,
    };

    let tr = match resolved {
        i18n::Language::Portuguese => &i18n::PT_TRANSLATIONS,
        i18n::Language::Spanish => &i18n::ES_TRANSLATIONS,
        i18n::Language::German => &i18n::DE_TRANSLATIONS,
        i18n::Language::French => &i18n::FR_TRANSLATIONS,
        i18n::Language::Russian => &i18n::RU_TRANSLATIONS,
        i18n::Language::Japanese => &i18n::JA_TRANSLATIONS,
        _ => &i18n::EN_TRANSLATIONS,
    };

    ui.set_current_language_code(lang.code().into());
    ui.set_tr_login_title(tr.login_title.into());
    ui.set_tr_login_desc(tr.login_desc.into());
    ui.set_tr_login_placeholder(tr.login_placeholder.into());
    ui.set_tr_login_btn_connect(tr.login_btn_connect.into());
    ui.set_tr_login_btn_detect(tr.login_btn_detect.into());
    ui.set_tr_server_channels(tr.server_channels.into());
    ui.set_tr_leave(tr.leave.into());
    ui.set_tr_view_text_chat(tr.view_text_chat.into());
    ui.set_tr_leave_call(tr.leave_call.into());
    ui.set_tr_voice_participants_title(tr.voice_participants_title.into());
    ui.set_tr_badge_you(tr.you.into());
    ui.set_tr_view_voice_room(tr.view_voice_room.into());
    ui.set_tr_replying_to(tr.replying_to.into());
    ui.set_tr_chat_placeholder_prefix(tr.chat_placeholder_prefix.into());
    ui.set_tr_send(tr.send.into());
    ui.set_tr_settings_title(tr.settings_title.into());
    ui.set_tr_settings_language_label(tr.settings_language_label.into());
    ui.set_tr_settings_input_device(tr.settings_input_device.into());
    ui.set_tr_settings_output_device(tr.settings_output_device.into());
    ui.set_tr_settings_threshold(tr.settings_threshold.into());
    ui.set_tr_settings_mic_level(tr.settings_mic_level.into());
    ui.set_tr_settings_btn_test_start(match resolved {
        i18n::Language::Portuguese => "🎧 Testar Microfone (\"Se Ouvir\")".into(),
        i18n::Language::Spanish => "🎧 Probar Micrófono (\"Escucharse\")".into(),
        i18n::Language::German => "🎧 Mikrofon testen (\"Sich hören\")".into(),
        i18n::Language::French => "🎧 Tester le Micro (\"S'entendre\")".into(),
        i18n::Language::Russian => "🎧 Проверить микрофон (\"Слышать себя\")".into(),
        i18n::Language::Japanese => "🎧 マイクをテスト (自分の声を聞く)".into(),
        _ => "🎧 Test Microphone (\"Hear Yourself\")".into(),
    });
    ui.set_tr_settings_btn_test_stop(match resolved {
        i18n::Language::Portuguese => "🎙️ Parar Teste (Ouvindo...)".into(),
        i18n::Language::Spanish => "🎙️ Detener Prueba (Escuchando...)".into(),
        i18n::Language::German => "🎙️ Test beenden (Hören...)".into(),
        i18n::Language::French => "🎙️ Arrêter le Test (Écoute...)".into(),
        i18n::Language::Russian => "🎙️ Остановить тест (Слушаем...)".into(),
        i18n::Language::Japanese => "🎙️ テスト停止 (聴取中...)".into(),
        _ => "🎙️ Stop Testing (Listening...)".into(),
    });
    ui.set_tr_settings_done(tr.settings_done.into());
    ui.set_tr_logout_title(tr.logout_title.into());
    ui.set_tr_logout_confirm_prefix(tr.logout_confirm_prefix.into());
    ui.set_tr_cancel(tr.cancel.into());
    ui.set_tr_confirm_logout(tr.confirm_logout.into());
    ui.set_tr_voice_connecting(tr.voice_connecting.into());
    ui.set_tr_voice_connecting_title(tr.voice_connecting_title.into());
    ui.set_tr_voice_connecting_desc(tr.voice_connecting_desc.into());
    ui.set_tr_voice_connected(tr.voice_connected.into());
    ui.set_tr_qr_title(tr.qr_title.into());
    ui.set_tr_qr_desc(tr.qr_desc.into());
    ui.set_tr_qr_confirm(tr.qr_confirm.into());

    let lang_items: Vec<LanguageItem> = i18n::Language::all_available().iter().map(|&l| {
        let (display, native) = l.display_info();
        LanguageItem {
            code: l.code().into(),
            name: display.into(),
            native_name: native.into(),
            is_selected: l == lang,
        }
    }).collect();
    ui.set_languages(std::rc::Rc::new(slint::VecModel::from(lang_items)).into());
}

static CACHED_AUDIO_DEVICES: std::sync::OnceLock<Arc<std::sync::Mutex<Option<(Vec<String>, Vec<String>)>>>> = std::sync::OnceLock::new();

fn get_audio_devices_cache() -> Arc<std::sync::Mutex<Option<(Vec<String>, Vec<String>)>>> {
    CACHED_AUDIO_DEVICES.get_or_init(|| Arc::new(std::sync::Mutex::new(None))).clone()
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

    let result = (inputs, outputs);
    if let Ok(mut cache) = get_audio_devices_cache().lock() {
        *cache = Some(result.clone());
    }
    result
}

fn push_to_mic_queues(q: &mut VecDeque<f32>, s: f32, level: f32) {
    if q.len() < 96000 { q.push_back(s); }
    if gateway::is_testing_mic() {
        if let Ok(mut loop_q) = gateway::get_mic_loopback_queue().lock() {
            // Apply VAD threshold to loopback test: play silence if level is below threshold!
            let sample_to_play = if level >= gateway::get_vad_threshold() { s } else { 0.0 };
            if loop_q.len() < 96000 { loop_q.push_back(sample_to_play); }
        }
    }
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
                    let num_channels = channels.max(1);
                    let frames = data.len() / num_channels;
                    if frames == 0 { return; }

                    let mut mono_samples = Vec::with_capacity(frames);
                    let mut sum_sq = 0.0f32;

                    for frame in data.chunks_exact(num_channels) {
                        let s = frame.iter().sum::<f32>() / num_channels as f32;
                        sum_sq += s * s;
                        mono_samples.push(s);
                    }

                    let rms = (sum_sq / frames as f32).sqrt();
                    let level = (rms * 6.0).min(1.0);
                    let _ = level_tx.try_send(level);

                    if let Ok(mut q) = q_arc.lock() {
                        if sample_rate == 48000 {
                            for &s in &mono_samples {
                                push_to_mic_queues(&mut q, s.clamp(-1.0, 1.0), level);
                            }
                        } else {
                            let ratio = 48000.0 / sample_rate as f64;
                            let out_len = (frames as f64 * ratio) as usize;
                            for i in 0..out_len {
                                let src_pos = i as f64 / ratio;
                                let src_idx = (src_pos as usize).min(frames.saturating_sub(1));
                                let frac = (src_pos - src_idx as f64) as f32;
                                
                                let s0 = mono_samples[src_idx];
                                let s1 = mono_samples[(src_idx + 1).min(frames.saturating_sub(1))];
                                let resampled = (s0 * (1.0 - frac) + s1 * frac).clamp(-1.0, 1.0);
                                push_to_mic_queues(&mut q, resampled, level);
                            }
                        }
                    }
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
                    let num_channels = channels.max(1);
                    let frames = data.len() / num_channels;
                    if frames == 0 { return; }

                    let mut mono_samples = Vec::with_capacity(frames);
                    let mut sum_sq = 0.0f32;

                    for frame in data.chunks_exact(num_channels) {
                        let s = frame.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / num_channels as f32;
                        sum_sq += s * s;
                        mono_samples.push(s);
                    }

                    let rms = (sum_sq / frames as f32).sqrt();
                    let level = (rms * 6.0).min(1.0);
                    let _ = level_tx.try_send(level);

                    if let Ok(mut q) = q_arc.lock() {
                        if sample_rate == 48000 {
                            for &s in &mono_samples {
                                push_to_mic_queues(&mut q, s.clamp(-1.0, 1.0), level);
                            }
                        } else {
                            let ratio = 48000.0 / sample_rate as f64;
                            let out_len = (frames as f64 * ratio) as usize;
                            for i in 0..out_len {
                                let src_pos = i as f64 / ratio;
                                let src_idx = (src_pos as usize).min(frames.saturating_sub(1));
                                let frac = (src_pos - src_idx as f64) as f32;
                                
                                let s0 = mono_samples[src_idx];
                                let s1 = mono_samples[(src_idx + 1).min(frames.saturating_sub(1))];
                                let resampled = (s0 * (1.0 - frac) + s1 * frac).clamp(-1.0, 1.0);
                                push_to_mic_queues(&mut q, resampled, level);
                            }
                        }
                    }
                },
                move |err| {
                    log::error!("Erro no Stream de Microfone I16: {:?}", err);
                },
                None,
            ).ok()?
        }
        cpal::SampleFormat::I32 => {
            let q_arc = Arc::clone(&pcm_queue);
            target_dev.build_input_stream(
                &config.into(),
                move |data: &[i32], _: &_| {
                    let num_channels = channels.max(1);
                    let frames = data.len() / num_channels;
                    if frames == 0 { return; }

                    let mut mono_samples = Vec::with_capacity(frames);
                    let mut sum_sq = 0.0f32;

                    for frame in data.chunks_exact(num_channels) {
                        let s = frame.iter().map(|&x| x as f32 / 2147483648.0).sum::<f32>() / num_channels as f32;
                        sum_sq += s * s;
                        mono_samples.push(s);
                    }

                    let rms = (sum_sq / frames as f32).sqrt();
                    let level = (rms * 6.0).min(1.0);
                    let _ = level_tx.try_send(level);

                    if let Ok(mut q) = q_arc.lock() {
                        if sample_rate == 48000 {
                            for &s in &mono_samples {
                                push_to_mic_queues(&mut q, s.clamp(-1.0, 1.0), level);
                            }
                        } else {
                            let ratio = 48000.0 / sample_rate as f64;
                            let out_len = (frames as f64 * ratio) as usize;
                            for i in 0..out_len {
                                let src_pos = i as f64 / ratio;
                                let src_idx = (src_pos as usize).min(frames.saturating_sub(1));
                                let frac = (src_pos - src_idx as f64) as f32;
                                
                                let s0 = mono_samples[src_idx];
                                let s1 = mono_samples[(src_idx + 1).min(frames.saturating_sub(1))];
                                let resampled = (s0 * (1.0 - frac) + s1 * frac).clamp(-1.0, 1.0);
                                push_to_mic_queues(&mut q, resampled, level);
                            }
                        }
                    }
                },
                move |err| {
                    log::error!("Erro no Stream de Microfone I32: {:?}", err);
                },
                None,
            ).ok()?
        }
        _ => return None,
    };

    if stream.play().is_ok() {
        info!("🎙️ Stream de Microfone INICIADO COM SUCESSO! Rate: {}Hz -> 48000Hz", sample_rate);
        Some(stream)
    } else {
        log::error!("Falha ao executar stream.play() no microfone!");
        None
    }
}

fn start_mic_loopback_stream(device_name: String) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let devices = host.output_devices().ok()?;

    let target_dev = if device_name.is_empty() || device_name.contains("Padrão") {
        host.default_output_device()?
    } else {
        devices.into_iter().find(|d| {
            d.name().map(|n| n == device_name).unwrap_or(false)
        })?
    };

    let config = target_dev.default_output_config().ok()?;
    let channels = config.channels() as usize;

    let loop_q_arc = gateway::get_mic_loopback_queue();

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let q_arc = Arc::clone(&loop_q_arc);
            target_dev.build_output_stream(
                &config.into(),
                move |output: &mut [f32], _| {
                    if !gateway::is_testing_mic() {
                        for s in output.iter_mut() { *s = 0.0; }
                        return;
                    }
                    if let Ok(mut loop_q) = q_arc.lock() {
                        for frame in output.chunks_mut(channels.max(1)) {
                            let sample = loop_q.pop_front().unwrap_or(0.0);
                            for s in frame.iter_mut() {
                                *s = sample;
                            }
                        }
                    } else {
                        for s in output.iter_mut() { *s = 0.0; }
                    }
                },
                move |err| { log::error!("Erro no Stream Loopback F32: {:?}", err); },
                None,
            ).ok()?
        }
        cpal::SampleFormat::I16 => {
            let q_arc = Arc::clone(&loop_q_arc);
            target_dev.build_output_stream(
                &config.into(),
                move |output: &mut [i16], _| {
                    if !gateway::is_testing_mic() {
                        for s in output.iter_mut() { *s = 0; }
                        return;
                    }
                    if let Ok(mut loop_q) = q_arc.lock() {
                        for frame in output.chunks_mut(channels.max(1)) {
                            let sample = loop_q.pop_front().unwrap_or(0.0);
                            let val = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
                            for s in frame.iter_mut() {
                                *s = val;
                            }
                        }
                    } else {
                        for s in output.iter_mut() { *s = 0; }
                    }
                },
                move |err| { log::error!("Erro no Stream Loopback I16: {:?}", err); },
                None,
            ).ok()?
        }
        _ => return None,
    };

    if stream.play().is_ok() {
        info!("🎧 Stream Loopback ('Se Ouvir') iniciado com sucesso!");
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
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
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
                    } else if let Some(ref hash) = g_icon_hash {
                        let icon_url = format!("https://cdn.discordapp.com/icons/{}/{}.png?size=128", g_id, hash);
                        pending_icon_downloads.push((icon_url, local_icon_path.to_string_lossy().to_string()));
                    }

                    guilds_map.lock().unwrap().insert(g_id.clone(), GuildData {
                        id: g_id.clone(),
                        name: g_name.clone(),
                        channels: Vec::new(),
                    });

                    raw_guilds.push(RawGuildItem {
                        id: g_id,
                        name: g_name,
                        icon: icon_str,
                        icon_path: icon_path_opt,
                    });
                }
            }

            let raw_guilds_clone = raw_guilds.clone();
            let app_w = app_weak.clone();
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
                    guilds_map.clone(),
                    active_guild_id.clone(),
                    active_channel_id.clone(),
                    cmd_tx_store.clone(),
                    &first_gid,
                ).await;
            }

            // Download custom server icons asynchronously in the background
            if !pending_icon_downloads.is_empty() {
                let http_dl = http.clone();
                let app_w_dl = app_weak.clone();

                tokio::spawn(async move {
                    for (url, save_path) in pending_icon_downloads {
                        if let Ok(bytes) = http_dl.download_image(&url).await {
                            let _ = std::fs::write(&save_path, &bytes);
                        }
                    }

                    // Reload UI with newly cached icons
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w_dl.upgrade() {
                            let old_guilds = ui.get_guilds();
                            let mut updated = Vec::new();
                            for i in 0..old_guilds.row_count() {
                                if let Some(mut item) = old_guilds.row_data(i) {
                                    let gid = item.id.to_string();
                                    let icon_path = cache_dir.join(format!("{}.png", gid));
                                    if icon_path.exists() {
                                        if let Ok(img) = Image::load_from_path(&icon_path) {
                                            item.has_image = true;
                                            item.icon_image = img;
                                        }
                                    }
                                    updated.push(item);
                                }
                            }
                            let model = std::rc::Rc::new(slint::VecModel::from(updated));
                            ui.set_guilds(model.into());
                        }
                    });
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
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
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

            // Subscribe via Gateway Opcode 14 for all voice channels in this guild to get live voice_states
            let voice_cids: Vec<String> = channels_data.iter().filter(|c| c.is_voice).map(|c| c.id.clone()).collect();
            if let Some(tx) = cmd_tx_store.lock().unwrap().as_ref() {
                let _ = tx.try_send(GatewayCommand::SubscribeGuild {
                    guild_id: guild_id.to_string(),
                    channel_ids: voice_cids,
                });
            }

            // Update channels in guilds_map
            if let Some(g_data) = guilds_map.lock().unwrap().get_mut(guild_id) {
                g_data.channels = channels_data.clone();
            }

            let ui_channels: Vec<ChannelItem> = channels_data.iter().map(|ch| {
                let vcount = if ch.is_voice { gateway::get_voice_channel_participant_count(&ch.id) } else { 0 };
                if ch.is_voice {
                    info!("📊 Canal de Voz {} ('{}'): vcount = {}", ch.id, ch.name, vcount);
                }
                ChannelItem {
                    id: ch.id.clone().into(),
                    name: ch.name.clone().into(),
                    is_voice: ch.is_voice,
                    voice_count: vcount,
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

            // Fetch and cache all guild members via REST API asynchronously
            if let Ok(members) = http.get_guild_members(guild_id).await {
                for m in members {
                    let uid_str = m["user"]["id"].as_str().unwrap_or("");
                    if let Ok(uid) = uid_str.parse::<u64>() {
                        let mut display_name = String::new();
                        if let Some(nick) = m["nick"].as_str() {
                            if !nick.is_empty() { display_name = nick.to_string(); }
                        }
                        if display_name.is_empty() {
                            if let Some(gname) = m["user"]["global_name"].as_str() {
                                if !gname.is_empty() { display_name = gname.to_string(); }
                            }
                        }
                        if display_name.is_empty() {
                            if let Some(uname) = m["user"]["username"].as_str() {
                                if !uname.is_empty() { display_name = uname.to_string(); }
                            }
                        }
                        if !display_name.is_empty() {
                            gateway::register_user_name(uid, display_name);
                        }
                    }
                }
                info!("✅ Membros e Bots do servidor {} pre-carregados via REST API com SUCESSO!", guild_id);
            }

            // Pre-load application commands for this guild (bots like music bots, 24/7, moderation)
            load_guild_command_index(http, app_weak.clone(), guild_id).await;

            // Try to find the first readable text channel automatically
            let text_channels: Vec<&ChannelData> = channels_data.iter().filter(|c| !c.is_voice).collect();
            let mut loaded_readable = false;

            for text_ch in text_channels {
                let ch_id = text_ch.id.clone();
                let ch_name = text_ch.name.clone();

                *active_channel_id.lock().unwrap() = ch_id.clone();
                if load_messages_for_channel(http, app_weak.clone(), &ch_id).await {
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
                *active_channel_id.lock().unwrap() = String::new();
                let app_w_none = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w_none.upgrade() {
                        ui.set_active_channel_name("Nenhum canal de texto acessível".into());
                        let empty_msgs = vec![ChatMessage {
                            id: "".into(),
                            author: "Litecord System".into(),
                            content: "🔒 Este servidor não possui canais de texto acessíveis para a sua conta.".into(),
                            commands: slint::ModelRc::default(),
                            content_lines: slint::ModelRc::default(),
                            embed_content: "".into(),
                            embed_lines: slint::ModelRc::default(),
                            embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                            embed_footer: "".into(),
                            code_block: "".into(),
                            reply_author: "".into(),
                            reply_content: "".into(),
                            reply_command: "".into(),
                            links: slint::ModelRc::default(),
                            buttons: slint::ModelRc::default(),
                            attachments: slint::ModelRc::default(),
                            timestamp: "Agora".into(),
                        }];
                        let model = std::rc::Rc::new(slint::VecModel::from(empty_msgs));
                        ui.set_messages(model.into());
                        ui.set_has_more_older_messages(false);
                    }
                });
            }
        }
        Err(e) => {
            error!("Erro ao buscar canais do servidor via REST: {}", e);
        }
    }
}

#[derive(Clone, Debug)]
pub struct CommandSuggestionItem {
    pub name: String,
    pub desc: String,
    pub usage: String,
    pub app_id: String,
    pub app_name: String,
    pub cmd_id: String,
    pub version: String,
    pub param_name: String,
    pub param_desc: String,
    pub is_required: bool,
}

pub static MASTER_COMMAND_SUGGESTIONS: std::sync::OnceLock<Arc<std::sync::Mutex<Vec<CommandSuggestionItem>>>> = std::sync::OnceLock::new();

pub fn get_master_command_suggestions() -> Arc<std::sync::Mutex<Vec<CommandSuggestionItem>>> {
    MASTER_COMMAND_SUGGESTIONS.get_or_init(|| {
        Arc::new(std::sync::Mutex::new(vec![
            CommandSuggestionItem {
                name: "/play".to_string(),
                desc: "Tocar música ou adicionar à fila".to_string(),
                usage: "/play <query>".to_string(),
                app_id: "".to_string(),
                app_name: "Música".to_string(),
                cmd_id: "".to_string(),
                version: "".to_string(),
                param_name: "query".to_string(),
                param_desc: "Nome da música ou link do YouTube/Spotify".to_string(),
                is_required: true,
            },
            CommandSuggestionItem {
                name: "/skip".to_string(),
                desc: "Pular para a próxima música".to_string(),
                usage: "/skip".to_string(),
                app_id: "".to_string(),
                app_name: "Música".to_string(),
                cmd_id: "".to_string(),
                version: "".to_string(),
                param_name: "".to_string(),
                param_desc: "".to_string(),
                is_required: false,
            },
            CommandSuggestionItem {
                name: "/stop".to_string(),
                desc: "Parar reprodução e limpar a fila".to_string(),
                usage: "/stop".to_string(),
                app_id: "".to_string(),
                app_name: "Música".to_string(),
                cmd_id: "".to_string(),
                version: "".to_string(),
                param_name: "".to_string(),
                param_desc: "".to_string(),
                is_required: false,
            },
            CommandSuggestionItem {
                name: "/pause".to_string(),
                desc: "Pausar a reprodução atual".to_string(),
                usage: "/pause".to_string(),
                app_id: "".to_string(),
                app_name: "Música".to_string(),
                cmd_id: "".to_string(),
                version: "".to_string(),
                param_name: "".to_string(),
                param_desc: "".to_string(),
                is_required: false,
            },
            CommandSuggestionItem {
                name: "/resume".to_string(),
                desc: "Retomar a reprodução pausada".to_string(),
                usage: "/resume".to_string(),
                app_id: "".to_string(),
                app_name: "Música".to_string(),
                cmd_id: "".to_string(),
                version: "".to_string(),
                param_name: "".to_string(),
                param_desc: "".to_string(),
                is_required: false,
            },
            CommandSuggestionItem {
                name: "/queue".to_string(),
                desc: "Ver a fila de músicas atual".to_string(),
                usage: "/queue".to_string(),
                app_id: "".to_string(),
                app_name: "Música".to_string(),
                cmd_id: "".to_string(),
                version: "".to_string(),
                param_name: "".to_string(),
                param_desc: "".to_string(),
                is_required: false,
            },
            CommandSuggestionItem {
                name: "/nowplaying".to_string(),
                desc: "Exibir a música tocando agora".to_string(),
                usage: "/nowplaying".to_string(),
                app_id: "".to_string(),
                app_name: "Música".to_string(),
                cmd_id: "".to_string(),
                version: "".to_string(),
                param_name: "".to_string(),
                param_desc: "".to_string(),
                is_required: false,
            },
            CommandSuggestionItem {
                name: "/247".to_string(),
                desc: "Manter o bot 24/7 conectado no canal de voz".to_string(),
                usage: "/247".to_string(),
                app_id: "".to_string(),
                app_name: "Música".to_string(),
                cmd_id: "".to_string(),
                version: "".to_string(),
                param_name: "".to_string(),
                param_desc: "".to_string(),
                is_required: false,
            },
        ]))
    }).clone()
}

async fn load_guild_command_index(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guild_id: &str,
) {
    if guild_id.is_empty() || guild_id == "@me" {
        return;
    }
    match http.get_guild_application_command_index(guild_id).await {
        Ok(data) => {
            // Build map of application_id -> Application / Bot Name
            let mut app_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            if let Some(apps) = data["applications"].as_array() {
                for app in apps {
                    let app_id = app["id"].as_str().unwrap_or("").to_string();
                    let app_name = app["name"].as_str()
                        .or_else(|| app["bot"]["global_name"].as_str())
                        .or_else(|| app["bot"]["username"].as_str())
                        .unwrap_or("")
                        .to_string();
                    if !app_id.is_empty() && !app_name.is_empty() {
                        app_names.insert(app_id, app_name);
                    }
                }
            }

            if let Some(cmds) = data["application_commands"].as_array() {
                let mut items: Vec<CommandSuggestionItem> = Vec::new();
                for cmd in cmds {
                    let name = cmd["name"].as_str().unwrap_or("");
                    if name.is_empty() { continue; }
                    let desc = cmd["description"].as_str().unwrap_or("");
                    let app_id = cmd["application_id"].as_str().unwrap_or("");
                    let cmd_id = cmd["id"].as_str().unwrap_or("");
                    let version = cmd["version"].as_str().unwrap_or("");
                    let app_name = app_names.get(app_id).cloned().unwrap_or_else(|| {
                        if !app_id.is_empty() { "Bot".to_string() } else { String::new() }
                    });
                    
                    let mut param_name = String::new();
                    let mut param_desc = String::new();
                    let mut is_required = false;
                    
                    if let Some(opts) = cmd["options"].as_array() {
                        // Find first required option, or fallback to first option if present
                        let required_opt = opts.iter().find(|o| o["required"].as_bool().unwrap_or(false));
                        if let Some(opt) = required_opt {
                            param_name = opt["name"].as_str().unwrap_or("").to_string();
                            param_desc = opt["description"].as_str().unwrap_or("").to_string();
                            is_required = true;
                        } else if let Some(opt) = opts.first() {
                            // Option is optional — do not force a required parameter pill
                            let opt_name = opt["name"].as_str().unwrap_or("");
                            let opt_desc = opt["description"].as_str().unwrap_or("");
                            param_desc = format!("Opcional: {} ({})", opt_name, opt_desc);
                            is_required = false;
                        }
                    }
                    
                    let usage = if param_name.is_empty() {
                        format!("/{}", name)
                    } else if is_required {
                        format!("/{} <{}>", name, param_name)
                    } else {
                        format!("/{} [{}]", name, param_name)
                    };
                    
                    items.push(CommandSuggestionItem {
                        name: format!("/{}", name),
                        desc: desc.to_string(),
                        usage,
                        app_id: app_id.to_string(),
                        app_name,
                        cmd_id: cmd_id.to_string(),
                        version: version.to_string(),
                        param_name,
                        param_desc,
                        is_required,
                    });
                }
                
                if !items.is_empty() {
                    let count = items.len();
                    if let Ok(mut master) = get_master_command_suggestions().lock() {
                        master.clear();
                        master.extend(items.clone());
                    }

                    let slint_suggestions: Vec<CommandSuggestion> = items.into_iter().map(|item| CommandSuggestion {
                        name: item.name.into(),
                        desc: item.desc.into(),
                        usage: item.usage.into(),
                        app_id: item.app_id.into(),
                        app_name: item.app_name.into(),
                        cmd_id: item.cmd_id.into(),
                        version: item.version.into(),
                        param_name: item.param_name.into(),
                        param_desc: item.param_desc.into(),
                        is_required: item.is_required,
                    }).collect();

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak.upgrade() {
                            let model = std::rc::Rc::new(slint::VecModel::from(slint_suggestions));
                            ui.set_command_suggestions(model.into());
                            info!("✅ Carregados {} comandos inteligentes reais dos bots do servidor com identificadores!", count);
                        }
                    });
                }
            }
        }
        Err(e) => {
            warn!("Não foi possível carregar o índice de comandos do servidor {}: {}", guild_id, e);
        }
    }
}

fn map_message_lines(lines: &[gateway::MessageLineData], channel_id: &str, app_weak: &slint::Weak<AppWindow>) -> slint::ModelRc<MessageLine> {
    let emoji_mgr = emoji_cache::get_emoji_cache();
    let slint_lines: Vec<MessageLine> = lines.iter().map(|line| {
        let slint_blocks: Vec<MessageBlock> = line.blocks.iter().map(|b| {
            let emoji_img = if b.is_emoji {
                if let Some(img) = emoji_mgr.get(&b.emoji_id) {
                    img
                } else {
                    emoji_mgr.fetch_priority_async(&b.emoji_id, channel_id, app_weak.clone());
                    slint::Image::default()
                }
            } else {
                slint::Image::default()
            };

            MessageBlock {
                text: b.text.clone().into(),
                is_link: b.is_link,
                is_command: b.is_command,
                is_emoji: b.is_emoji,
                emoji_id: b.emoji_id.clone().into(),
                emoji_img,
                url: b.url.clone().into(),
                command_name: b.command_name.clone().into(),
            }
        }).collect();
        MessageLine {
            blocks: slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(slint_blocks))),
        }
    }).collect();
    slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(slint_lines)))
}

fn map_message_attachments(
    attachments: &[gateway::MessageAttachmentData],
    app_weak: &slint::Weak<AppWindow>,
) -> slint::ModelRc<MessageAttachment> {
    let att_cache = attachment_cache::get_attachment_cache();
    let slint_atts: Vec<MessageAttachment> = attachments.iter().map(|a| {
        let (is_downloaded, full_img) = if let Some(img) = att_cache.get_full(&a.id, &a.filename) {
            (true, img)
        } else {
            (false, slint::Image::default())
        };

        let preview_img = if a.is_image && !is_downloaded {
            if let Some(p) = att_cache.get_preview(&a.id) {
                p
            } else {
                att_cache.fetch_preview_async(&a.id, &a.proxy_url, app_weak.clone());
                slint::Image::default()
            }
        } else {
            slint::Image::default()
        };

        let (width, height) = if a.width > 0 && a.height > 0 {
            (a.width, a.height)
        } else if is_downloaded {
            let sz = full_img.size();
            (sz.width as i32, sz.height as i32)
        } else {
            (a.width, a.height)
        };

        MessageAttachment {
            id: a.id.clone().into(),
            filename: a.filename.clone().into(),
            url: a.url.clone().into(),
            proxy_url: a.proxy_url.clone().into(),
            size_str: a.size_str.clone().into(),
            width,
            height,
            is_image: a.is_image,
            is_downloaded,
            is_loading: false,
            full_img,
            preview_img,
        }
    }).collect();
    slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(slint_atts)))
}

fn map_message_buttons(
    buttons: &[gateway::MessageButtonData],
    channel_id: &str,
    app_weak: &slint::Weak<AppWindow>,
) -> slint::ModelRc<MessageButton> {
    let emoji_mgr = emoji_cache::get_emoji_cache();
    let slint_buttons: Vec<MessageButton> = buttons.iter().map(|b| {
        let emoji_img = if !b.emoji_id.is_empty() {
            if let Some(img) = emoji_mgr.get(&b.emoji_id) {
                img
            } else {
                emoji_mgr.fetch_priority_async(&b.emoji_id, channel_id, app_weak.clone());
                slint::Image::default()
            }
        } else {
            slint::Image::default()
        };

        MessageButton {
            label: b.label.clone().into(),
            url: b.url.clone().into(),
            emoji: b.emoji.clone().into(),
            emoji_id: b.emoji_id.clone().into(),
            emoji_img,
            style_type: b.style_type,
            is_disabled: b.is_disabled,
        }
    }).collect();
    slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(slint_buttons)))
}

async fn load_messages_for_channel(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    channel_id: &str,
) -> bool {
    info!("Carregando mensagens do canal {}...", channel_id);
    emoji_cache::get_emoji_cache().set_active_channel(channel_id);
    let ch_id_for_emojis = channel_id.to_string();
    let app_weak_load = app_weak.clone();
    match http.get_channel_messages(channel_id).await {
        Ok(msgs_val) => {
            let has_more = msgs_val.len() >= 30;
            if let Some(oldest) = msgs_val.last() {
                if let Some(id_str) = oldest["id"].as_str() {
                    get_oldest_message_map().lock().unwrap().insert(channel_id.to_string(), id_str.to_string());
                }
            }

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    ui.set_has_more_older_messages(has_more);
                    ui.set_is_loading_older_messages(false);

                    let ui_msgs: Vec<ChatMessage> = if msgs_val.is_empty() {
                        vec![ChatMessage {
                            id: "".into(),
                            author: "Litecord System".into(),
                            content: "Este canal está vazio ou não possui mensagens recentes.".into(),
                            commands: slint::ModelRc::default(),
                            content_lines: slint::ModelRc::default(),
                            embed_content: "".into(),
                            embed_lines: slint::ModelRc::default(),
                            embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                            embed_footer: "".into(),
                            code_block: "".into(),
                            reply_author: "".into(),
                            reply_content: "".into(),
                            reply_command: "".into(),
                            links: slint::ModelRc::default(),
                            buttons: slint::ModelRc::default(),
                            attachments: slint::ModelRc::default(),
                            timestamp: "Agora".into(),
                        }]
                    } else {
                        msgs_val.iter().rev().map(|m| {
                            let msg_id = m["id"].as_str().unwrap_or("");
                            let author = format_discord_author(m);
                            let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(m);
                            
                            let slint_cmds: Vec<slint::SharedString> = commands.into_iter().map(|c| c.into()).collect();
                            let commands_model = std::rc::Rc::new(slint::VecModel::from(slint_cmds));

                            let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
                                label: l.label.clone().into(),
                                url: l.url.clone().into(),
                            }).collect();
                            let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

                            ChatMessage {
                                id: msg_id.into(),
                                author: author.into(),
                                content: content.into(),
                                commands: slint::ModelRc::from(commands_model),
                                content_lines: map_message_lines(&content_lines, &ch_id_for_emojis, &app_weak_load),
                                embed_content: embed_content.into(),
                                embed_lines: map_message_lines(&embed_lines, &ch_id_for_emojis, &app_weak_load),
                                embed_color: parse_hex_color(&embed_color),
                                embed_footer: embed_footer.into(),
                                code_block: code_block.into(),
                                reply_author: reply_author.into(),
                                reply_content: reply_content.into(),
                                reply_command: reply_command.into(),
                                links: slint::ModelRc::from(links_model),
                                buttons: map_message_buttons(&buttons, &ch_id_for_emojis, &app_weak_load),
                                attachments: map_message_attachments(&attachments, &app_weak_load),
                                timestamp: "Agora".into(),
                            }
                        }).collect()
                    };
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_msgs));
                    ui.set_messages(model.into());

                    let app_w_scroll = app_weak.clone();
                    slint::Timer::single_shot(std::time::Duration::from_millis(20), move || {
                        if let Some(ui) = app_w_scroll.upgrade() {
                            ui.invoke_scroll_chat_to_bottom();
                        }
                    });
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
                    ui.set_has_more_older_messages(false);
                    let ui_msgs = vec![ChatMessage {
                        id: "".into(),
                        author: "Litecord System".into(),
                        content: friendly_msg.into(),
                        commands: slint::ModelRc::default(),
                        content_lines: slint::ModelRc::default(),
                        embed_content: "".into(),
                        embed_lines: slint::ModelRc::default(),
                        embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                        embed_footer: "".into(),
                        code_block: "".into(),
                        reply_author: "".into(),
                        reply_content: "".into(),
                        reply_command: "".into(),
                        links: slint::ModelRc::default(),
                        buttons: slint::ModelRc::default(),
                        attachments: slint::ModelRc::default(),
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



struct AppLogger {
    file: Mutex<Option<std::fs::File>>,
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            use std::io::Write;
            let time_str = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => {
                    let total_secs = d.as_secs();
                    let millis = d.subsec_millis();
                    let secs = total_secs % 60;
                    let mins = (total_secs / 60) % 60;
                    let hours = (total_secs / 3600) % 24;
                    format!("{:02}:{:02}:{:02}.{:03}", hours, mins, secs, millis)
                }
                Err(_) => "00:00:00.000".to_string(),
            };

            let level_str = match record.level() {
                log::Level::Error => "\x1b[31mERROR\x1b[0m",
                log::Level::Warn => "\x1b[33mWARN \x1b[0m",
                log::Level::Info => "\x1b[32mINFO \x1b[0m",
                log::Level::Debug => "\x1b[36mDEBUG\x1b[0m",
                log::Level::Trace => "\x1b[35mTRACE\x1b[0m",
            };

            let plain_level = match record.level() {
                log::Level::Error => "ERROR",
                log::Level::Warn => "WARN ",
                log::Level::Info => "INFO ",
                log::Level::Debug => "DEBUG",
                log::Level::Trace => "TRACE",
            };

            let console_msg = format!("[{} {} {}] {}\n", time_str, level_str, record.target(), record.args());
            let file_msg = format!("[{} {} {}] {}\n", time_str, plain_level, record.target(), record.args());

            // 1. Output to attached console
            print!("{}", console_msg);
            let _ = std::io::stdout().flush();

            // 2. Persist to log file for user support diagnostics
            if let Ok(mut f_guard) = self.file.lock() {
                if let Some(ref mut f) = *f_guard {
                    let _ = f.write_all(file_msg.as_bytes());
                    let _ = f.flush();
                }
            }
        }
    }

    fn flush(&self) {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        if let Ok(mut f_guard) = self.file.lock() {
            if let Some(ref mut f) = *f_guard {
                let _ = f.flush();
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "desconhecido".to_string()
        };
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let msg = format!("💥 PANIC DETECTADO: {}\nLocation: {}\n", payload, location);
        eprintln!("{}", msg);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("panic_log.txt") {
            use std::io::Write;
            let _ = writeln!(f, "{}", msg);
        }
    }));

    attachment_cache::cleanup_temp_attachments();

    let log_file = std::fs::OpenOptions::new().create(true).append(true).open("litecord_app.log").ok();
    let logger = AppLogger { file: Mutex::new(log_file) };
    let _ = log::set_boxed_logger(Box::new(logger));
    log::set_max_level(log::LevelFilter::Info);

    #[cfg(target_os = "windows")]
    unsafe {
        #[link(name = "winmm")]
        extern "system" {
            fn timeBeginPeriod(uPeriod: u32) -> u32;
        }
        timeBeginPeriod(1);
    }

    info!("Iniciando Litecord v{} (Log persistente salvo em litecord_app.log)...", env!("CARGO_PKG_VERSION"));

    info!("🖥️ Inicializando Slint AppWindow...");
    let app = AppWindow::new()?;
    info!("🖥️ Exibindo janela principal (app.show())...");
    app.show()?;
    info!("🖥️ Janela Slint exibida com sucesso!");

    let initial_lang = i18n::load_persisted_language_config();
    apply_i18n_translations(&app, initial_lang);
    info!("🌐 Idioma inicial configurado: {:?}", initial_lang);

    #[cfg(target_os = "linux")]
    {
        app.set_is_linux(true);
    }

    let saved_audio_cfg = gateway::load_persisted_audio_config();
    app.set_vad_threshold(saved_audio_cfg.vad_threshold);
    app.set_app_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    let app_weak = app.as_weak();

    let popout_window = PopoutStreamWindow::new()?;
    let popout_weak = popout_window.as_weak();
    let popout_hwnd_store: Arc<Mutex<Option<isize>>> = Arc::new(Mutex::new(None));

    let hwnd_store: Arc<Mutex<Option<isize>>> = Arc::new(Mutex::new(None));

    use i_slint_backend_winit::WinitWindowAccessor;
    let app_weak_init = app_weak.clone();
    let hwnd_store_init = Arc::clone(&hwnd_store);
    let popout_weak_init = popout_weak.clone();
    let pop_hwnd_c = Arc::clone(&popout_hwnd_store);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_weak_init.upgrade() {
            ui.window().with_winit_window(|winit_win| {
                info!("🖥️ [EVENT LOOP] Configurando janela Winit no evento inicial...");
                let _ = winit_win.request_inner_size(winit::dpi::LogicalSize::new(980.0, 620.0));
                winit_win.set_outer_position(winit::dpi::LogicalPosition::new(100.0, 100.0));
                winit_win.set_visible(true);
                winit_win.set_minimized(false);
                winit_win.focus_window();
                winit_win.request_redraw();
                if let Some(pop_win) = popout_weak_init.upgrade() {
                    pop_win.window().with_winit_window(|winit_pop| {
                        winit_pop.set_decorations(false);
                        let _ = winit_pop.request_inner_size(winit::dpi::LogicalSize::new(640.0, 360.0));
                        winit_pop.set_min_inner_size(Some(winit::dpi::LogicalSize::new(320.0, 180.0)));
                        winit_pop.set_visible(false);
                        #[cfg(target_os = "windows")]
                        {
                            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                            if let Ok(handle) = winit_pop.window_handle() {
                                if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                                    let hwnd = win32_handle.hwnd.get() as isize;
                                    *pop_hwnd_c.lock().unwrap() = Some(hwnd);
                                    set_dark_titlebar_color(hwnd);
                                    unsafe {
                                        use windows_sys::Win32::UI::WindowsAndMessaging::{
                                            GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE, WS_THICKFRAME, WS_MINIMIZEBOX, WS_MAXIMIZEBOX
                                        };
                                        let style = GetWindowLongPtrW(hwnd as _, GWL_STYLE) as u32;
                                        SetWindowLongPtrW(hwnd as _, GWL_STYLE, (style | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX) as isize);
                                    }
                                }
                            }
                        }
                    });
                }

                // Set native window and taskbar/dock icon (Linux X11/Wayland & Windows)
                let icon_bytes = include_bytes!("../assets/app_icon.png");
                if let Ok(img) = image::load_from_memory(icon_bytes) {
                    let rgba = img.into_rgba8();
                    let (width, height) = rgba.dimensions();
                    if let Ok(winit_icon) = winit::window::Icon::from_rgba(rgba.into_raw(), width, height) {
                        winit_win.set_window_icon(Some(winit_icon));
                    }
                }

                #[cfg(target_os = "windows")]
                {
                    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = winit_win.window_handle() {
                        if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                            let hwnd = win32_handle.hwnd.get() as isize;
                            *hwnd_store_init.lock().unwrap() = Some(hwnd);
                            set_dark_titlebar_color(hwnd);
                        }
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    winit_win.set_decorations(true);
                }
            });
            ui.window().request_redraw();
        }
    });

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak_pulse = app_weak.clone();
        tokio::spawn(async move {
            for _ in 0..8 {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let app_w = app_weak_pulse.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w.upgrade() {
                        ui.window().request_redraw();
                    }
                });
            }
        });
    }

    let last_token: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let guilds_map: Arc<Mutex<HashMap<String, GuildData>>> = Arc::new(Mutex::new(HashMap::new()));
    let cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>> = Arc::new(Mutex::new(None));

    let selected_input: Arc<Mutex<String>> = Arc::new(Mutex::new(saved_audio_cfg.input_device.clone()));
    let selected_output: Arc<Mutex<String>> = Arc::new(Mutex::new(saved_audio_cfg.output_device.clone()));
    if !saved_audio_cfg.input_device.is_empty() {
        app.set_selected_input_device(saved_audio_cfg.input_device.into());
    }
    if !saved_audio_cfg.output_device.is_empty() {
        gateway::set_selected_output_device(saved_audio_cfg.output_device.clone());
        app.set_selected_output_device(saved_audio_cfg.output_device.into());
    }
    let active_mic_stream: Arc<Mutex<Option<cpal::Stream>>> = Arc::new(Mutex::new(None));
    let active_loopback_stream: Arc<Mutex<Option<cpal::Stream>>> = Arc::new(Mutex::new(None));

    let tray = SystemTrayManager::setup();
    let show_id = tray.show_item_id.clone();
    let quit_id = tray.quit_item_id.clone();

    // Spawn tray event listener thread
    let hwnd_store_tray = Arc::clone(&hwnd_store);
    let app_weak_tray = app_weak.clone();
    let show_id_c = show_id.clone();
    let quit_id_c = quit_id.clone();
    tokio::task::spawn_blocking(move || {
        let menu_rx = MenuEvent::receiver();
        let tray_rx = TrayIconEvent::receiver();

        loop {
            let mut should_restore = false;

            while let Ok(event) = menu_rx.try_recv() {
                info!("Tray MenuEvent recebido: {:?}", event);
                if event.id == show_id_c {
                    should_restore = true;
                } else if event.id == quit_id_c {
                    #[cfg(target_os = "linux")]
                    screen_capture::kill_portal_child();
                    std::process::exit(0);
                }
            }

            while let Ok(event) = tray_rx.try_recv() {
                info!("TrayIconEvent recebido: {:?}", event);
                if matches!(event, TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }) {
                    should_restore = true;
                }
            }

            if should_restore {
                APP_IS_VISIBLE.store(true, Ordering::Relaxed);
                NEED_UI_REFRESH.store(true, Ordering::Relaxed);
                info!("[DeepSleep] Restauração acionada via Tray — acordando UI.");

                let app_weak_inner = app_weak_tray.clone();
                let _hwnd_store_inner = Arc::clone(&hwnd_store_tray);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui_inner) = app_weak_inner.upgrade() {
                        let _ = ui_inner.window().with_winit_window(|winit_win| {
                            winit_win.set_visible(true);
                            winit_win.set_minimized(false);
                            winit_win.focus_window();
                        });
                    }
                    #[cfg(target_os = "windows")]
                    if let Some(hwnd) = *_hwnd_store_inner.lock().unwrap() {
                        unsafe {
                            ShowWindow(hwnd as _, SW_SHOW);
                            ShowWindow(hwnd as _, SW_RESTORE);
                            SetForegroundWindow(hwnd as _);
                            SetWindowPos(hwnd as _, HWND_TOP as _, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
                        }
                    }
                });
            }

            std::thread::sleep(std::time::Duration::from_millis(120));
        }
    });

    let http_client: Arc<Mutex<Option<DiscordHttpClient>>> = Arc::new(Mutex::new(None));
    let active_channel_id: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let active_guild_id: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    // Tokio MPSC Channel for Gateway Events -> Slint UI
    let (event_tx, mut event_rx) = mpsc::channel::<GatewayEvent>(100);

    // Initial background update check on app launch
        // Initial background update check on app launch
    trigger_update_check(app_weak.clone());

    // Background warmup of audio devices cache
    tokio::task::spawn_blocking(|| {
        let _ = enumerate_audio_devices();
    });

    // Tokio MPSC Channel for Microphone Volume Level (0.0 to 1.0)
    let (level_tx, mut level_rx) = mpsc::channel::<f32>(100);

    // Dispatch Microphone Volume Level to Slint UI Thread with high efficiency (throttled to 30 FPS / 33ms)
    let app_weak_level = app_weak.clone();
    tokio::spawn(async move {
        let mut last_send = std::time::Instant::now();
        let mut last_level = -1.0f32;
        while let Some(level) = level_rx.recv().await {
            gateway::set_self_mic_level(level);
            // Deep-sleep guard: drain the channel without invoking Slint
            if !APP_IS_VISIBLE.load(Ordering::Relaxed) {
                continue;
            }
            // Skip UI redraw if microphone was silent and is still silent
            if level < 0.005 && last_level < 0.005 {
                continue;
            }
            let now = std::time::Instant::now();
            let delta = (level - last_level).abs();
            // Cap visual UI property dispatch to 30 FPS (33ms) or significant transitions
            if now.duration_since(last_send).as_millis() >= 33 || (delta >= 0.05 && now.duration_since(last_send).as_millis() >= 16) {
                last_send = now;
                last_level = level;
                let app_w_inner = app_weak_level.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w_inner.upgrade() {
                        ui.set_mic_level(level);
                    }
                });
            }
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
        let url = url_str.trim().to_string();
        // Strict URL protocol validation: only allow HTTP and HTTPS
        if !url.starts_with("http://") && !url.starts_with("https://") {
            log::warn!("Tentativa de abrir link não-HTTP rejeitada por segurança: {}", url);
            return;
        }

        info!("Abrindo link seguro no navegador padrão: {}", url);
        #[cfg(target_os = "windows")]
        unsafe {
            let op: Vec<u16> = "open\0".encode_utf16().collect();
            let file: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
            ShellExecuteW(0, op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1 /* SW_SHOWNORMAL */);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        }
    });

    let app_weak_dl = app_weak.clone();
    app.on_download_attachment(move |att_id: SharedString, filename: SharedString, url: SharedString| {
        let cache = attachment_cache::get_attachment_cache();
        cache.download_full_async(att_id.as_str(), filename.as_str(), url.as_str(), app_weak_dl.clone());
    });

    // Popout Stream Window Callbacks & Window Manager
    let pop_w_cb = popout_weak.clone();
    let pop_hwnd_cb = Arc::clone(&popout_hwnd_store);
    let app_weak_pop = app_weak.clone();
    app.on_popout_stream_window(move |uid_str: slint::SharedString, uname_str: slint::SharedString| {
        let uid = uid_str.to_string();
        let uname = uname_str.to_string();
        info!("📺 Desanexando transmissão de vídeo para janela Popout! User ID: {}, Nome: {}", uid, uname);
        if let Some(ui) = app_weak_pop.upgrade() {
            ui.set_popped_out_stream_uid(uid.clone().into());
            let my_uid = gateway::get_my_user_id().to_string();
            let is_pop_self = uid == "self" || uid == my_uid;
            let cur_frame = if is_pop_self { ui.get_active_stream_frame() } else { ui.get_remote_stream_frame() };
            if let Some(pop_win) = pop_w_cb.upgrade() {
                pop_win.set_user_id(uid.clone().into());
                pop_win.set_username(if uname.is_empty() { ui.get_active_stream_user() } else { uname.into() });
                pop_win.set_stream_frame(cur_frame);
                pop_win.set_show_controls(true);
                let _ = pop_win.show();
                #[cfg(target_os = "windows")]
                if let Some(hwnd) = *pop_hwnd_cb.lock().unwrap() {
                    unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOW, SetForegroundWindow};
                        ShowWindow(hwnd as _, SW_SHOW);
                        SetForegroundWindow(hwnd as _);
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    let is_p = pop_win.get_is_pinned();
                    if is_p {
                        set_linux_window_keep_above(true);
                    }
                }
                let _ = pop_win.window().with_winit_window(|w| {
                    w.set_visible(true);
                    w.set_minimized(false);
                    w.focus_window();
                });
            }
        }
    });

    let pop_w_pin = popout_weak.clone();
    let pop_hwnd_pin = Arc::clone(&popout_hwnd_store);
    if let Some(pop_win) = popout_weak.upgrade() {
        pop_win.on_toggle_pin(move || {
            if let Some(pop) = pop_w_pin.upgrade() {
                let new_pinned = !pop.get_is_pinned();
                pop.set_is_pinned(new_pinned);
                info!("📌 Janela Popout Always-on-Top alterado para: {}", new_pinned);
                #[cfg(target_os = "windows")]
                if let Some(hwnd) = *pop_hwnd_pin.lock().unwrap() {
                    unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::{
                            SetWindowPos, HWND_TOPMOST, HWND_NOTOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
                        };
                        let insert_after = if new_pinned { HWND_TOPMOST } else { HWND_NOTOPMOST };
                        SetWindowPos(hwnd as _, insert_after as _, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    set_linux_window_keep_above(new_pinned);
                }
                let _ = pop.window().with_winit_window(move |w| {
                    w.set_window_level(if new_pinned {
                        winit::window::WindowLevel::AlwaysOnTop
                    } else {
                        winit::window::WindowLevel::Normal
                    });
                });
            }
        });
    }

    let pop_w_drag = popout_weak.clone();
    if let Some(pop_win) = popout_weak.upgrade() {
        pop_win.on_start_window_drag(move || {
            if let Some(pop) = pop_w_drag.upgrade() {
                static LAST_POP_DRAG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                let last = LAST_POP_DRAG.load(Ordering::Relaxed);
                if now.saturating_sub(last) < 200 {
                    return;
                }
                LAST_POP_DRAG.store(now, Ordering::Relaxed);

                pop.set_drag_area_enabled(false);

                let pop_weak_timer = pop.as_weak();
                slint::Timer::single_shot(std::time::Duration::from_millis(150), move || {
                    if let Some(pop_inst) = pop_weak_timer.upgrade() {
                        pop_inst.set_drag_area_enabled(true);
                    }
                });

                let _ = pop.window().with_winit_window(|winit_win| {
                    let _ = winit_win.drag_window();
                });
            }
        });
    }

    let pop_w_fs = popout_weak.clone();
    if let Some(pop_win) = popout_weak.upgrade() {
        pop_win.on_toggle_fullscreen(move || {
            if let Some(pop) = pop_w_fs.upgrade() {
                let current = pop.get_is_maximized();
                let next = !current;
                pop.set_is_maximized(next);
                pop.window().set_maximized(next);
            }
        });
    }

    let pop_w_min = popout_weak.clone();
    let pop_hwnd_min = Arc::clone(&popout_hwnd_store);
    if let Some(pop_win) = popout_weak.upgrade() {
        pop_win.on_minimize_popout(move || {
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = *pop_hwnd_min.lock().unwrap() {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MINIMIZE};
                    ShowWindow(hwnd as _, SW_MINIMIZE);
                }
            }
            if let Some(pop) = pop_w_min.upgrade() {
                pop.window().set_minimized(true);
            }
        });
    }

    let pop_w_resize = popout_weak.clone();
    if let Some(pop_win) = popout_weak.upgrade() {
        pop_win.on_drag_resize(move |edge: slint::SharedString| {
            if let Some(pop) = pop_w_resize.upgrade() {
                static LAST_POP_RESIZE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                let last = LAST_POP_RESIZE.load(Ordering::Relaxed);
                if now.saturating_sub(last) < 200 {
                    return;
                }
                LAST_POP_RESIZE.store(now, Ordering::Relaxed);

                pop.set_drag_area_enabled(false);

                let pop_weak_timer = pop.as_weak();
                slint::Timer::single_shot(std::time::Duration::from_millis(150), move || {
                    if let Some(pop_inst) = pop_weak_timer.upgrade() {
                        pop_inst.set_drag_area_enabled(true);
                    }
                });

                let _ = pop.window().with_winit_window(move |winit_win| {
                    let dir = match edge.as_str() {
                        "top" => winit::window::ResizeDirection::North,
                        "bottom" => winit::window::ResizeDirection::South,
                        "left" => winit::window::ResizeDirection::West,
                        "right" => winit::window::ResizeDirection::East,
                        "top-left" => winit::window::ResizeDirection::NorthWest,
                        "top-right" => winit::window::ResizeDirection::NorthEast,
                        "bottom-left" => winit::window::ResizeDirection::SouthWest,
                        "bottom-right" => winit::window::ResizeDirection::SouthEast,
                        _ => return,
                    };
                    let _ = winit_win.drag_resize_window(dir);
                });
            }
        });
    }

    // Intangible / Click-Through Ghost Mode Callbacks
    let pop_w_ghost = popout_weak.clone();
    let pop_hwnd_ghost = Arc::clone(&popout_hwnd_store);
    let app_weak_ghost = app_weak.clone();

    let toggle_ghost_fn = Arc::new(move || {
        let mut new_state = false;
        if let Some(pop) = pop_w_ghost.upgrade() {
            new_state = !pop.get_is_intangible();
            pop.set_is_intangible(new_state);
            let hittest = !new_state;
            let _ = pop.window().with_winit_window(move |w| {
                let _ = w.set_cursor_hittest(hittest);
            });
            if new_state {
                info!("👻 Modo Intangível ATIVADO no popup (cliques atravessam para os jogos/apps)!");
            } else {
                info!("🖱️ Modo Intangível DESATIVADO no popup (janela volta a receber cliques).");
            }
        }
        if let Some(ui) = app_weak_ghost.upgrade() {
            ui.set_is_popout_intangible(new_state);
        }
        #[cfg(target_os = "windows")]
        if let Some(hwnd) = *pop_hwnd_ghost.lock().unwrap() {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
                };
                let ex_style = GetWindowLongW(hwnd as _, GWL_EXSTYLE);
                if new_state {
                    SetWindowLongW(hwnd as _, GWL_EXSTYLE, ex_style | WS_EX_TRANSPARENT as i32 | WS_EX_LAYERED as i32);
                } else {
                    SetWindowLongW(hwnd as _, GWL_EXSTYLE, ex_style & !(WS_EX_TRANSPARENT as i32));
                }
            }
        }
    });

    if let Some(pop_win) = popout_weak.upgrade() {
        let tg1 = Arc::clone(&toggle_ghost_fn);
        pop_win.on_toggle_intangible(move || {
            tg1();
        });
    }

    let tg2 = Arc::clone(&toggle_ghost_fn);
    app.on_toggle_popout_intangible(move || {
        tg2();
    });

    let pop_w_close = popout_weak.clone();
    let pop_hwnd_close = Arc::clone(&popout_hwnd_store);
    let app_weak_pop_close = app_weak.clone();
    if let Some(pop_win) = popout_weak.upgrade() {
        pop_win.on_close_popout(move || {
            info!("📺 Fechando janela Popout e reanexando transmissão à sala principal!");
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = *pop_hwnd_close.lock().unwrap() {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        ShowWindow, SW_HIDE, GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_TRANSPARENT,
                    };
                    let ex_style = GetWindowLongW(hwnd as _, GWL_EXSTYLE);
                    SetWindowLongW(hwnd as _, GWL_EXSTYLE, ex_style & !(WS_EX_TRANSPARENT as i32));
                    ShowWindow(hwnd as _, SW_HIDE);
                }
            }
            if let Some(pop) = pop_w_close.upgrade() {
                pop.set_is_intangible(false);
                let _ = pop.window().with_winit_window(|w| {
                    let _ = w.set_cursor_hittest(true);
                });
                pop.set_user_id("".into());
                let _ = pop.hide();
            }
            if let Some(ui) = app_weak_pop_close.upgrade() {
                ui.set_is_popout_intangible(false);
                ui.set_popped_out_stream_uid("".into());
            }
        });
    }

    let pop_w_mouse = popout_weak.clone();
    let last_mouse_act = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let last_mouse_act_cb = Arc::clone(&last_mouse_act);
    if let Some(pop_win) = popout_weak.upgrade() {
        pop_win.on_mouse_activity(move || {
            if let Some(pop) = pop_w_mouse.upgrade() {
                if !pop.get_show_controls() {
                    pop.set_show_controls(true);
                }
                *last_mouse_act_cb.lock().unwrap() = std::time::Instant::now();
            }
        });

        pop_win.on_stream_volume_changed(move |uid_str: slint::SharedString, vol: f32| {
            let uid = uid_str.to_string().parse::<u64>().unwrap_or(0);
            if uid != 0 {
                screen_capture::set_stream_volume(uid, vol);
            }
        });
    }

    // Autohide top bar timer for Popout Window
    let pop_w_timer = popout_weak.clone();
    let last_mouse_act_timer = Arc::clone(&last_mouse_act);
    let _pop_autohide_timer = slint::Timer::default();
    _pop_autohide_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(300), move || {
        if let Some(pop) = pop_w_timer.upgrade() {
            if pop.get_show_controls() {
                let elapsed = last_mouse_act_timer.lock().unwrap().elapsed();
                if elapsed.as_millis() > 2200 {
                    pop.set_show_controls(false);
                }
            }
        }
    });

    // Screen Share / Live Stream Video Callbacks (P2P UDP Transmitter & Receiver)
    let screen_manager = Arc::new(ScreenCaptureManager::new());
    let sm_clone = Arc::clone(&screen_manager);
    let app_weak_ss = app_weak.clone();
    let pop_w_ss = popout_weak.clone();
    let pop_hwnd_ss = Arc::clone(&popout_hwnd_store);
    let hidden_streams: Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

    // Thread-safe Latest Frame Stores for Zero-Backlog Slint Event Loop Rendering
    let latest_rx_frame: Arc<Mutex<Option<(u64, String, String, SharedPixelBuffer<Rgba8Pixel>)>>> = Arc::new(Mutex::new(None));
    let is_rx_ui_pending = Arc::new(AtomicBool::new(false));

    let latest_tx_frame: Arc<Mutex<Option<SharedPixelBuffer<Rgba8Pixel>>>> = Arc::new(Mutex::new(None));
    let is_tx_ui_pending = Arc::new(AtomicBool::new(false));

    // Initialize P2P Video Receiver to receive streams from other users in real time!
    let app_weak_rx = app_weak.clone();
    let pop_w_rx = popout_weak.clone();
    let hidden_streams_rx = Arc::clone(&hidden_streams);
    let latest_rx_frame_cb = Arc::clone(&latest_rx_frame);
    let is_rx_ui_pending_cb = Arc::clone(&is_rx_ui_pending);

    screen_manager.start_receiver(
        {
            let app_w = app_weak_rx.clone();
            let pop_w = pop_w_rx.clone();
            move |uid, uname, quality, pixel_buf| {
                *latest_rx_frame_cb.lock().unwrap() = Some((uid, uname, quality, pixel_buf));
                if !is_rx_ui_pending_cb.swap(true, Ordering::AcqRel) {
                    let app_w2 = app_w.clone();
                    let pop_w2 = pop_w.clone();
                    let latest_slot = Arc::clone(&latest_rx_frame_cb);
                    let pending_flag = Arc::clone(&is_rx_ui_pending_cb);

                    let _ = slint::invoke_from_event_loop(move || {
                        pending_flag.store(false, Ordering::Release);
                        if let Some((uid, uname, quality, pixel_buf)) = latest_slot.lock().unwrap().take() {
                            let uid_str = uid.to_string();
                            let frame = Image::from_rgba8(pixel_buf);
                            let mut should_update_pop = false;
                            if let Some(ui) = app_w2.upgrade() {
                                let popped_uid = ui.get_popped_out_stream_uid().to_string();
                                if popped_uid == uid_str {
                                    should_update_pop = true;
                                }
                                ui.set_has_active_stream(true);
                                ui.set_active_stream_user_id(uid_str.clone().into());
                                ui.set_active_stream_user(uname.into());
                                ui.set_tr_stream_quality(quality.into());
                                ui.set_remote_stream_frame(frame.clone());
                            }
                            if should_update_pop {
                                if let Some(pop) = pop_w2.upgrade() {
                                    let pop_uid = pop.get_user_id().to_string();
                                    if pop_uid == uid_str || pop_uid.is_empty() {
                                        pop.set_stream_frame(frame);
                                    }
                                }
                            }
                        }
                    });
                }
            }
        },
        {
            let app_w = app_weak_rx.clone();
            let pop_w = pop_w_rx.clone();
            let hidden_streams_rx_cb = Arc::clone(&hidden_streams_rx);
            move |uid, is_streaming| {
                let uid_str = uid.to_string();
                let app_w2 = app_w.clone();
                let pop_w2 = pop_w.clone();
                let hidden_streams_inner = Arc::clone(&hidden_streams_rx_cb);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w2.upgrade() {
                        if !is_streaming {
                            hidden_streams_inner.lock().unwrap().remove(uid_str.as_str());
                            if ui.get_active_stream_user_id() == uid_str.as_str() {
                                ui.set_has_active_stream(false);
                                ui.set_active_stream_user("".into());
                                ui.set_active_stream_user_id("".into());
                            }
                            if ui.get_popped_out_stream_uid() == uid_str.as_str() {
                                ui.set_popped_out_stream_uid("".into());
                                if let Some(pop) = pop_w2.upgrade() {
                                    pop.set_user_id("".into());
                                    let _ = pop.hide();
                                }
                            }
                            if ui.get_focused_stream_uid() == uid_str.as_str() {
                                ui.set_focused_stream_uid("".into());
                            }
                        }
                    }
                });
            }
        }
    );

    app.on_toggle_screen_share(move || {
        let sm = Arc::clone(&sm_clone);
        if let Some(ui) = app_weak_ss.upgrade() {
            let active = sm.is_active();
            if active {
                info!("🛑 Encerrando transmissão de tela P2P...");
                sm.stop();
                ui.set_is_screen_sharing(false);
                ui.set_local_preview_fps("".into());
                let my_uid = gateway::get_my_user_id().to_string();
                if ui.get_popped_out_stream_uid() == "self" || ui.get_popped_out_stream_uid() == my_uid.as_str() {
                    ui.set_popped_out_stream_uid("".into());
                    if let Some(pop) = pop_w_ss.upgrade() {
                        pop.set_user_id("".into());
                        let _ = pop.hide();
                    }
                }
                if ui.get_focused_stream_uid() == "self" || ui.get_focused_stream_uid() == my_uid.as_str() {
                    ui.set_focused_stream_uid("".into());
                }
                #[cfg(target_os = "windows")]
                if let Some(hwnd) = *pop_hwnd_ss.lock().unwrap() {
                    unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
                        ShowWindow(hwnd as _, SW_HIDE);
                    }
                }
            } else {
                let my_uid = gateway::get_my_user_id();
                let my_uname = gateway::get_my_username();
                let cid = gateway::get_my_voice_channel_id();
                sm.set_context(cid, my_uid, &my_uname);

                let res_str = ui.get_stream_selected_res().to_string();
                let res_val: i32 = match res_str.as_str() {
                    "1080p" => 1080,
                    "480p" => 480,
                    _ => 720,
                };
                let fps_val = ui.get_stream_selected_fps();
                let target_fps: i32 = if fps_val <= 0 { 30 } else { fps_val };

                let source_tab = ui.get_stream_source_tab().to_string();
                let (target_hwnd, screen_index, camera_index, include_audio) = if source_tab == "cameras" {
                    let cam_id_str = ui.get_stream_selected_camera_id().to_string();
                    let cam_idx: Option<u32> = cam_id_str.parse().ok();
                    (0, 0, cam_idx, false)
                } else if source_tab == "windows" {
                    let hwnd: isize = ui.get_stream_selected_window_id().to_string().parse().unwrap_or(0);
                    (hwnd, 0, None, ui.get_stream_include_audio())
                } else {
                    let screen_idx = ui.get_stream_selected_source_index().max(0) as usize;
                    (0, screen_idx, None, ui.get_stream_include_audio())
                };

                #[cfg(target_os = "linux")]
                screen_capture::reset_wayland_portal_cancelled();

                info!("▶️ Iniciando transmissão P2P (tab={}, screen_idx={}, {} @ {} FPS, hwnd={}, cam={:?}, audio={})...", source_tab, screen_index, res_str, target_fps, target_hwnd, camera_index, include_audio);
                ui.set_tr_stream_quality(format!("{} {}fps", res_str, target_fps).into());

                let app_weak_frame = app_weak_ss.clone();
                let pop_w_frame = pop_w_ss.clone();
                let latest_tx_frame_cb = Arc::clone(&latest_tx_frame);
                let is_tx_ui_pending_cb = Arc::clone(&is_tx_ui_pending);

                sm.start(target_hwnd, screen_index, camera_index, res_val, target_fps, include_audio, move |pixel_buffer| {
                    *latest_tx_frame_cb.lock().unwrap() = Some(pixel_buffer);
                    if !is_tx_ui_pending_cb.swap(true, Ordering::AcqRel) {
                        let app_w = app_weak_frame.clone();
                        let pop_w = pop_w_frame.clone();
                        let latest_slot = Arc::clone(&latest_tx_frame_cb);
                        let pending_flag = Arc::clone(&is_tx_ui_pending_cb);

                        let _ = slint::invoke_from_event_loop(move || {
                            pending_flag.store(false, Ordering::Release);
                            if let Some(pixel_buf) = latest_slot.lock().unwrap().take() {
                                let frame = Image::from_rgba8(pixel_buf);
                                let tx_fps = screen_capture::get_tx_fps();
                                let fps_str = if tx_fps > 0.0 {
                                    format!("{:.1} FPS", tx_fps)
                                } else {
                                    format!("{} FPS", target_fps)
                                };

                                let mut is_pop_self = false;
                                if let Some(ui) = app_w.upgrade() {
                                    ui.set_active_stream_frame(frame.clone());
                                    ui.set_local_preview_fps(fps_str.clone().into());
                                    let popped = ui.get_popped_out_stream_uid().to_string();
                                    let my_uid = gateway::get_my_user_id().to_string();
                                    if popped == "self" || popped == my_uid {
                                        is_pop_self = true;
                                    }
                                }
                                if is_pop_self {
                                    if let Some(pop) = pop_w.upgrade() {
                                        pop.set_stream_frame(frame);
                                        pop.set_stream_fps(fps_str.into());
                                    }
                                }
                            }
                        });
                    }
                });
                ui.set_is_screen_sharing(true);
            }
        }
    });

    let app_weak_modal = app_weak.clone();
    app.on_open_stream_modal(move || {
        if let Some(ui) = app_weak_modal.upgrade() {
            let screens = screen_capture::list_screens();
            let ui_screens: Vec<ScreenSourceItem> = screens.into_iter().map(|s| {
                ScreenSourceItem {
                    id: s.id,
                    name: s.name.into(),
                    resolution: s.resolution.into(),
                    is_primary: s.is_primary,
                }
            }).collect();
            ui.set_available_screens(std::rc::Rc::new(slint::VecModel::from(ui_screens)).into());

            let windows = screen_capture::list_capturable_windows();
            let first_win_id = windows.first().map(|w| w.id.clone()).unwrap_or_default();
            let ui_windows: Vec<WindowSourceItem> = windows.into_iter().map(|w| {
                let mut has_icon = false;
                let mut icon_image = Image::default();
                if let Some((iw, ih, rgba)) = w.icon_rgba {
                    let mut pixel_buf = SharedPixelBuffer::<Rgba8Pixel>::new(iw, ih);
                    pixel_buf.make_mut_bytes().copy_from_slice(&rgba);
                    icon_image = Image::from_rgba8(pixel_buf);
                    has_icon = true;
                }
                WindowSourceItem {
                    id: w.id.into(),
                    title: w.title.into(),
                    app_name: w.app_name.into(),
                    has_icon,
                    icon_image,
                }
            }).collect();
            ui.set_available_windows(std::rc::Rc::new(slint::VecModel::from(ui_windows)).into());

            let cameras = screen_capture::list_cameras();
            let first_cam_id = cameras.first().map(|c| c.id.clone()).unwrap_or_default();
            let ui_cameras: Vec<CameraSourceItem> = cameras.into_iter().map(|c| {
                CameraSourceItem {
                    id: c.id.into(),
                    name: c.name.into(),
                }
            }).collect();
            ui.set_available_cameras(std::rc::Rc::new(slint::VecModel::from(ui_cameras)).into());

            ui.set_stream_selected_source_index(0);
            if ui.get_stream_source_tab() == "windows" {
                ui.set_stream_selected_window_id(first_win_id.into());
            } else if ui.get_stream_source_tab() == "cameras" {
                ui.set_stream_selected_camera_id(first_cam_id.into());
                ui.set_stream_include_audio(false);
            } else {
                ui.set_stream_selected_window_id("".into());
            }
            ui.set_show_stream_modal(true);
        }
    });

    let app_weak_fs = app_weak.clone();
    app.on_toggle_stream_fullscreen(move || {
        if let Some(ui) = app_weak_fs.upgrade() {
            let cur = ui.get_is_stream_fullscreen();
            ui.set_is_stream_fullscreen(!cur);
        }
    });

    let app_weak_focus_stream = app_weak.clone();
    app.on_focus_stream(move |uid_str: SharedString| {
        if let Some(ui) = app_weak_focus_stream.upgrade() {
            let cur = ui.get_focused_stream_uid();
            if cur == uid_str {
                ui.set_focused_stream_uid("".into());
            } else {
                ui.set_focused_stream_uid(uid_str);
            }
        }
    });

    let app_weak_hide = app_weak.clone();
    let hidden_streams_hide = Arc::clone(&hidden_streams);
    app.on_hide_stream(move |uid_str: SharedString| {
        let uid = uid_str.to_string();
        info!("👁️ Ocultando stream de vídeo do usuário {}", uid);
        hidden_streams_hide.lock().unwrap().insert(uid.clone());
        if let Some(ui) = app_weak_hide.upgrade() {
            if ui.get_focused_stream_uid() == uid.as_str() {
                ui.set_focused_stream_uid("".into());
            }
            let cur_model = ui.get_voice_participants();
            for i in 0..cur_model.row_count() {
                if let Some(mut p) = cur_model.row_data(i) {
                    if p.user_id == uid_str {
                        p.is_stream_hidden = true;
                        cur_model.set_row_data(i, p);
                        break;
                    }
                }
            }
        }
    });

    let app_weak_unhide = app_weak.clone();
    let hidden_streams_unhide = Arc::clone(&hidden_streams);
    let pop_w_unhide = popout_weak.clone();
    let pop_hwnd_unhide = Arc::clone(&popout_hwnd_store);
    app.on_unhide_stream(move |uid_str: SharedString| {
        let uid = uid_str.to_string();
        info!("👁️ Reexibindo stream de vídeo do usuário {}", uid);
        hidden_streams_unhide.lock().unwrap().remove(uid.as_str());
        if let Some(ui) = app_weak_unhide.upgrade() {
            if ui.get_popped_out_stream_uid() == uid.as_str() {
                ui.set_popped_out_stream_uid("".into());
                ui.set_is_popout_intangible(false);
                if let Some(pop) = pop_w_unhide.upgrade() {
                    pop.set_is_intangible(false);
                    let _ = pop.window().with_winit_window(|w| {
                        let _ = w.set_cursor_hittest(true);
                    });
                    pop.set_user_id("".into());
                    let _ = pop.hide();
                }
                #[cfg(target_os = "windows")]
                if let Some(hwnd) = *pop_hwnd_unhide.lock().unwrap() {
                    unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::{
                            ShowWindow, SW_HIDE, GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_TRANSPARENT,
                        };
                        let ex_style = GetWindowLongW(hwnd as _, GWL_EXSTYLE);
                        SetWindowLongW(hwnd as _, GWL_EXSTYLE, ex_style & !(WS_EX_TRANSPARENT as i32));
                        ShowWindow(hwnd as _, SW_HIDE);
                    }
                }
            }
            let cur_model = ui.get_voice_participants();
            for i in 0..cur_model.row_count() {
                if let Some(mut p) = cur_model.row_data(i) {
                    if p.user_id == uid_str {
                        p.is_stream_hidden = false;
                        cur_model.set_row_data(i, p);
                        break;
                    }
                }
            }
        }
    });

    let app_weak_mute = app_weak.clone();
    app.on_toggle_user_mute(move |uid_str: SharedString| {
        let uid = uid_str.to_string();
        let (is_m, _vol) = gateway::get_user_mute_volume(&uid);
        let new_m = !is_m;
        gateway::set_user_mute(&uid, new_m);
        info!("🎙️ Mute do participante {} alterado para: {}", uid, new_m);
        if let Some(ui) = app_weak_mute.upgrade() {
            let cur_model = ui.get_voice_participants();
            for i in 0..cur_model.row_count() {
                if let Some(mut p) = cur_model.row_data(i) {
                    if p.user_id == uid_str {
                        p.is_muted = new_m;
                        cur_model.set_row_data(i, p);
                        break;
                    }
                }
            }
        }
    });

    let app_weak_vol = app_weak.clone();
    app.on_set_user_volume(move |uid_str: SharedString, vol: f32| {
        let uid = uid_str.to_string();
        gateway::set_user_volume(&uid, vol);
        if let Some(ui) = app_weak_vol.upgrade() {
            let cur_model = ui.get_voice_participants();
            for i in 0..cur_model.row_count() {
                if let Some(mut p) = cur_model.row_data(i) {
                    if p.user_id == uid_str {
                        p.volume = vol;
                        cur_model.set_row_data(i, p);
                        break;
                    }
                }
            }
        }
    });

    let app_weak_stream_vol = app_weak.clone();
    app.on_set_stream_volume(move |uid_str: SharedString, vol: f32| {
        let uid = uid_str.to_string().parse::<u64>().unwrap_or(0);
        if uid != 0 {
            screen_capture::set_stream_volume(uid, vol);
        }
        if let Some(ui) = app_weak_stream_vol.upgrade() {
            ui.set_stream_remote_volume(vol);
        }
    });

    let app_weak_prio = app_weak.clone();
    app.on_set_user_priority(move |uid_str: SharedString, prio: i32| {
        let uid = uid_str.to_string();
        info!("👑 Prioridade de fala do participante {} alterada para: P:{}", uid, prio);
        gateway::set_user_priority(&uid, prio);
        if let Some(ui) = app_weak_prio.upgrade() {
            let cur_model = ui.get_voice_participants();
            for i in 0..cur_model.row_count() {
                if let Some(mut p) = cur_model.row_data(i) {
                    if p.user_id == uid_str {
                        p.priority = prio;
                        cur_model.set_row_data(i, p);
                        break;
                    }
                }
            }
        }
    });

    // Dispatch Live Voice Room Participants & Animated Volume Level Bars
    let app_weak_voice_loop = app_weak.clone();
    let http_client_voice_loop = Arc::clone(&http_client);
    let active_channel_voice_loop = Arc::clone(&active_channel_id);
    let active_guild_voice_loop = Arc::clone(&active_guild_id);
    let hidden_streams_voice_loop = Arc::clone(&hidden_streams);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            interval.tick().await;

            // Deep-sleep guard: skip when UI is hidden
            if !APP_IS_VISIBLE.load(Ordering::Relaxed) {
                continue;
            }

            let is_connected = gateway::is_connected_to_voice();

            // On-restore refresh: re-fetch active channel messages via REST
            if is_connected && NEED_UI_REFRESH.compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
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
            let http_client_voice_loop_inner = Arc::clone(&http_client_voice_loop);
            let active_guild_voice_loop_inner = Arc::clone(&active_guild_voice_loop);
            let hidden_streams_voice_loop_inner = Arc::clone(&hidden_streams_voice_loop);

            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = app_w_inner.upgrade() else { return; };
                if !ui.get_is_in_voice() { return; }

                if is_connected {
                    if ui.get_is_voice_connecting() {
                        ui.set_is_voice_connecting(false);
                    }
                } else {
                    return;
                }

                let active_parts = gateway::get_active_voice_participants_store();
                let queues_arc = gateway::get_speaker_pcm_queues();
                let my_uid = gateway::get_my_user_id();
                let self_level = gateway::get_self_mic_level();
                let mut participants = Vec::new();

                // Use try_lock() everywhere — never block the UI thread waiting for audio locks
                let parts_snapshot: Vec<(u32, u64)> = match active_parts.try_lock() {
                    Ok(map) => map.iter().filter(|(&s, _)| s != 999999).map(|(&s, &u)| (s, u)).collect(),
                    Err(_) => return, // Skip this frame; audio thread holds the lock
                };

                // Pre-compute audio levels using try_lock() so we never block on the CPAL-held speaker queue
                let mut ssrc_levels: HashMap<u32, f32> = HashMap::new();
                if let Ok(queues) = queues_arc.try_lock() {
                    for &(ssrc, user_id) in &parts_snapshot {
                        if user_id == my_uid && my_uid > 0 { continue; }
                        if let Some(q) = queues.get(&ssrc) {
                            let sample_cnt = q.len().min(480);
                            if sample_cnt > 0 {
                                let sum_sq: f32 = q.iter().take(sample_cnt).map(|&(l, r)| { let s = (l + r) * 0.5; s * s }).sum();
                                let rms = (sum_sq / sample_cnt as f32).sqrt();
                                ssrc_levels.insert(ssrc, if rms < 0.005 { 0.0 } else { (rms * 3.5).clamp(0.0, 1.0) });
                            }
                        }
                    }
                }
                // If try_lock failed, ssrc_levels stays empty → we just won't update bars this frame

                let mut user_best: HashMap<u64, (u32, f32, bool)> = HashMap::new();
                for (ssrc, user_id) in parts_snapshot {
                    if user_id == 999999 { continue; }
                    let (audio_level, is_speaking) = if user_id == my_uid && my_uid > 0 {
                        (self_level, self_level > 0.03)
                    } else {
                        let lvl = ssrc_levels.get(&ssrc).copied().unwrap_or(0.0);
                        (lvl, lvl > 0.03)
                    };
                    let entry = user_best.entry(user_id).or_insert((ssrc, audio_level, is_speaking));
                    if audio_level > entry.1 { *entry = (ssrc, audio_level, is_speaking); }
                }

                let my_user_id = gateway::get_my_user_id();
                let mut sorted_users: Vec<_> = user_best.into_iter().collect();
                sorted_users.sort_by(|(id_a, _), (id_b, _)| {
                    let is_self_a = (*id_a == my_user_id && my_user_id != 0) || *id_a == 999999;
                    let is_self_b = (*id_b == my_user_id && my_user_id != 0) || *id_b == 999999;
                    if is_self_a && !is_self_b {
                        std::cmp::Ordering::Less
                    } else if !is_self_a && is_self_b {
                        std::cmp::Ordering::Greater
                    } else {
                        let name_a = gateway::get_user_name(*id_a);
                        let name_b = gateway::get_user_name(*id_b);
                        name_a.cmp(&name_b)
                    }
                });

                let _total_users_count = sorted_users.len();
                for (user_id, (_ssrc, audio_level, is_speaking)) in sorted_users {
                    let uid_str = user_id.to_string();
                    let is_self = (user_id == my_user_id && my_user_id != 0) || user_id == 999999;

                    let username = if is_self {
                        let resolved = if my_user_id != 0 { gateway::get_user_name(my_user_id) } else { String::new() };
                        if resolved.is_empty() || resolved.starts_with("Participante #") {
                            "Você".to_string()
                        } else {
                            resolved
                        }
                    } else {
                        let uname = gateway::get_user_name(user_id);
                        if uname.starts_with("Participante #") {
                            if let Ok(http_opt) = http_client_voice_loop_inner.try_lock() {
                                if let Some(http) = http_opt.clone() {
                                    static FETCHING_USERS: std::sync::OnceLock<Arc<Mutex<std::collections::HashSet<u64>>>> = std::sync::OnceLock::new();
                                    let fetching_store = FETCHING_USERS.get_or_init(|| Arc::new(Mutex::new(std::collections::HashSet::new())));
                                    if let Ok(mut set) = fetching_store.try_lock() {
                                        if set.insert(user_id) {
                                            let uid_str_task = uid_str.clone();
                                            let active_gid = match active_guild_voice_loop_inner.try_lock() {
                                                Ok(g) => g.clone(),
                                                Err(_) => String::new(),
                                            };
                                            let store_arc = Arc::clone(fetching_store);
                                            tokio::spawn(async move {
                                                let mut resolved_name = String::new();

                                                // 1. Try GET /guilds/{guild_id}/members/{user_id} first (returns nick, user object)
                                                if !active_gid.is_empty() {
                                                    if let Ok(member_json) = http.get_guild_member(&active_gid, &uid_str_task).await {
                                                        if let Some(nick) = member_json["nick"].as_str() {
                                                            if !nick.is_empty() { resolved_name = nick.to_string(); }
                                                        }
                                                        if resolved_name.is_empty() {
                                                            if let Some(gname) = member_json["user"]["global_name"].as_str() {
                                                                if !gname.is_empty() { resolved_name = gname.to_string(); }
                                                            }
                                                        }
                                                        if resolved_name.is_empty() {
                                                            if let Some(uname) = member_json["user"]["username"].as_str() {
                                                                if !uname.is_empty() { resolved_name = uname.to_string(); }
                                                            }
                                                        }
                                                    }
                                                }

                                                // 2. Fallback to GET /users/{user_id}
                                                if resolved_name.is_empty() {
                                                    if let Ok(profile) = http.get_user_profile(&uid_str_task).await {
                                                        resolved_name = profile["global_name"].as_str()
                                                            .or_else(|| profile["username"].as_str())
                                                            .unwrap_or("")
                                                            .to_string();
                                                    }
                                                }

                                                if !resolved_name.is_empty() {
                                                    gateway::register_user_name(user_id, resolved_name.clone());
                                                    info!("✅ Nome de usuário/bot resolvido via REST API: {} -> {}", user_id, resolved_name);
                                                } else {
                                                    if let Ok(mut set) = store_arc.lock() {
                                                        set.remove(&user_id);
                                                    }
                                                }
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        uname
                    };

                    let avatar_text = username.chars().next().unwrap_or('U').to_uppercase().to_string();
                    let (is_muted_saved, vol, prio) = gateway::get_user_audio_settings(&uid_str);

                    let (final_audio_level, final_is_speaking, final_is_muted) = if is_self {
                        let mic_lvl = ui.get_mic_level();
                        let is_self_muted = ui.get_is_muted();
                        if is_self_muted {
                            (0.0, false, true)
                        } else {
                            let is_spk = mic_lvl >= gateway::get_vad_threshold();
                            let lvl = if is_spk { mic_lvl } else { 0.0 };
                            (lvl, is_spk, false)
                        }
                    } else {
                        (audio_level, is_speaking, is_muted_saved)
                    };

                    let is_streaming_peer = if is_self {
                        if ui.get_is_screen_sharing() {
                            true
                        } else {
                            let act_uid = ui.get_active_stream_user_id();
                            let act_uname = ui.get_active_stream_user();
                            ui.get_has_active_stream() && (
                                (!act_uid.is_empty() && act_uid.as_str() == uid_str.as_str())
                                || (!act_uname.is_empty() && act_uname.as_str() == username.as_str())
                            )
                        }
                    } else {
                        let act_uid = ui.get_active_stream_user_id();
                        let act_uname = ui.get_active_stream_user();
                        ui.get_has_active_stream() && (
                            (!act_uid.is_empty() && act_uid.as_str() == uid_str.as_str())
                            || (!act_uname.is_empty() && act_uname.as_str() == username.as_str())
                        )
                    };

                    let is_hidden = hidden_streams_voice_loop_inner.lock().map(|h| h.contains(&uid_str)).unwrap_or(false);

                    participants.push(VoiceParticipant {
                        user_id: uid_str.into(),
                        username: username.into(),
                        avatar_text: avatar_text.into(),
                        is_speaking: final_is_speaking,
                        audio_level: final_audio_level,
                        is_muted: final_is_muted,
                        volume: vol,
                        priority: prio,
                        is_self,
                        is_streaming: is_streaming_peer,
                        is_stream_hidden: is_hidden,
                    });
                }

                let cur_model = ui.get_voice_participants();
                let mut need_full_rebuild = cur_model.row_count() != participants.len();
                if !need_full_rebuild {
                    for (i, p) in participants.iter().enumerate() {
                        if let Some(old) = cur_model.row_data(i) {
                            if old.user_id != p.user_id || old.username != p.username || old.priority != p.priority || old.is_self != p.is_self {
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
                        if let Some(old) = cur_model.row_data(i) {
                            let level_diff = (old.audio_level - p.audio_level).abs();
                            let both_zero = old.audio_level < 0.01 && p.audio_level < 0.01;
                            let level_changed = !both_zero && level_diff >= 0.03;
                            let state_changed = old.is_speaking != p.is_speaking
                                || old.is_muted != p.is_muted
                                || old.volume != p.volume
                                || old.priority != p.priority
                                || old.username != p.username
                                || old.is_streaming != p.is_streaming
                                || old.is_stream_hidden != p.is_stream_hidden;
                            if level_changed || state_changed {
                                cur_model.set_row_data(i, p);
                            }
                        }
                    }
                }
            });
        }
    });

    // Periodic update for voice channel participant badges in sidebar (fingerprinted delta check)
    let app_weak_ch_badges = app_weak.clone();
    let guilds_map_badges = Arc::clone(&guilds_map);
    let active_guild_badges = Arc::clone(&active_guild_id);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));
        let mut last_fingerprint: Vec<(String, i32)> = Vec::new();
        let mut last_gid = String::new();
        loop {
            interval.tick().await;
            if !APP_IS_VISIBLE.load(Ordering::Relaxed) { continue; }
            let gid = active_guild_badges.lock().unwrap().clone();
            if gid.is_empty() { continue; }

            let channels_opt = {
                if let Ok(map) = guilds_map_badges.lock() {
                    map.get(&gid).map(|g| g.channels.clone())
                } else {
                    None
                }
            };

            if let Some(chans) = channels_opt {
                let mut current_fingerprint = Vec::with_capacity(chans.len());
                for ch in &chans {
                    let vcount = if ch.is_voice { gateway::get_voice_channel_participant_count(&ch.id) } else { 0 };
                    current_fingerprint.push((ch.id.clone(), vcount));
                }

                let guild_changed = gid != last_gid;
                if guild_changed || current_fingerprint != last_fingerprint {
                    last_gid = gid;
                    last_fingerprint = current_fingerprint;

                    let ui_channels: Vec<ChannelItem> = chans.iter().map(|ch| {
                        let vcount = if ch.is_voice { gateway::get_voice_channel_participant_count(&ch.id) } else { 0 };
                        ChannelItem {
                            id: ch.id.clone().into(),
                            name: ch.name.clone().into(),
                            is_voice: ch.is_voice,
                            voice_count: vcount,
                        }
                    }).collect();

                    let app_w = app_weak_ch_badges.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            let cur_chans = ui.get_channels();
                            let mut needs_rebuild = cur_chans.row_count() != ui_channels.len();
                            if !needs_rebuild {
                                for (i, new_c) in ui_channels.iter().enumerate() {
                                    if let Some(old_c) = cur_chans.row_data(i) {
                                        if old_c.voice_count != new_c.voice_count || old_c.id != new_c.id {
                                            cur_chans.set_row_data(i, new_c.clone());
                                        }
                                    } else {
                                        needs_rebuild = true;
                                        break;
                                    }
                                }
                            }
                            if needs_rebuild {
                                let model = std::rc::Rc::new(slint::VecModel::from(ui_channels));
                                ui.set_channels(model.into());
                            }
                        }
                    });
                }
            }
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
            // 1. Instantly display modal without blocking GUI thread!
            ui.set_show_settings_modal(true);
            ui.set_vad_threshold(gateway::get_vad_threshold());
            ui.set_is_testing_mic(gateway::is_testing_mic());

            let current_lang = i18n::load_persisted_language_config();
            let lang_items: Vec<LanguageItem> = i18n::Language::all_available().iter().map(|&l| {
                let (display, native) = l.display_info();
                LanguageItem {
                    code: l.code().into(),
                    name: display.into(),
                    native_name: native.into(),
                    is_selected: l == current_lang,
                }
            }).collect();
            ui.set_languages(std::rc::Rc::new(slint::VecModel::from(lang_items)).into());

            // 2. Populate devices immediately from cache if available
            let cur_input = selected_input_open.lock().unwrap().clone();
            let cur_output = selected_output_open.lock().unwrap().clone();

            let cached = get_audio_devices_cache().lock().unwrap().clone();
            if let Some((cached_inputs, cached_outputs)) = cached {
                let ui_inputs: Vec<AudioDeviceItem> = cached_inputs.iter().enumerate().map(|(idx, name)| {
                    let is_sel = if cur_input.is_empty() { idx == 0 } else { name == &cur_input };
                    AudioDeviceItem {
                        id: name.clone().into(),
                        name: name.clone().into(),
                        is_selected: is_sel,
                    }
                }).collect();

                let ui_outputs: Vec<AudioDeviceItem> = cached_outputs.iter().enumerate().map(|(idx, name)| {
                    let is_sel = if cur_output.is_empty() { idx == 0 } else { name == &cur_output };
                    AudioDeviceItem {
                        id: name.clone().into(),
                        name: name.clone().into(),
                        is_selected: is_sel,
                    }
                }).collect();

                ui.set_input_devices(std::rc::Rc::new(slint::VecModel::from(ui_inputs)).into());
                ui.set_output_devices(std::rc::Rc::new(slint::VecModel::from(ui_outputs)).into());
            }

            // 3. Start mic capture for volume meter
            let mic_name = cur_input.clone();
            if let Some(stream) = start_mic_capture(mic_name, level_tx_open.clone()) {
                *active_stream_open.lock().unwrap() = Some(stream);
            }

            // 4. Asynchronously refresh hardware devices in background without lag
            let app_w_async = app_weak_open_settings.clone();
            let sel_in = selected_input_open.clone();
            let sel_out = selected_output_open.clone();
            tokio::task::spawn_blocking(move || {
                let (inputs, outputs) = enumerate_audio_devices();
                let cur_in = sel_in.lock().unwrap().clone();
                let cur_out = sel_out.lock().unwrap().clone();

                let ui_inputs: Vec<AudioDeviceItem> = inputs.iter().enumerate().map(|(idx, name)| {
                    let is_sel = if cur_in.is_empty() { idx == 0 } else { name == &cur_in };
                    AudioDeviceItem {
                        id: name.clone().into(),
                        name: name.clone().into(),
                        is_selected: is_sel,
                    }
                }).collect();

                let ui_outputs: Vec<AudioDeviceItem> = outputs.iter().enumerate().map(|(idx, name)| {
                    let is_sel = if cur_out.is_empty() { idx == 0 } else { name == &cur_out };
                    AudioDeviceItem {
                        id: name.clone().into(),
                        name: name.clone().into(),
                        is_selected: is_sel,
                    }
                }).collect();

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui_inner) = app_w_async.upgrade() {
                        ui_inner.set_input_devices(std::rc::Rc::new(slint::VecModel::from(ui_inputs)).into());
                        ui_inner.set_output_devices(std::rc::Rc::new(slint::VecModel::from(ui_outputs)).into());
                    }
                });
            });
        }
    });

    let app_weak_close_settings = app_weak.clone();
    let active_stream_close = Arc::clone(&active_mic_stream);
    let active_loopback_close = Arc::clone(&active_loopback_stream);
    app.on_close_settings(move || {
        if let Some(ui) = app_weak_close_settings.upgrade() {
            ui.set_show_settings_modal(false);
            gateway::set_testing_mic(false);
            ui.set_is_testing_mic(false);
            *active_loopback_close.lock().unwrap() = None;
            if let Ok(mut q) = gateway::get_mic_loopback_queue().lock() {
                q.clear();
            }
            if !ui.get_is_in_voice() {
                *active_stream_close.lock().unwrap() = None; // Only stop test stream if not in voice
            }
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
        gateway::set_persisted_input_device(name_str.clone());

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
    let active_loopback_output = Arc::clone(&active_loopback_stream);
    app.on_select_output_device(move |dev_name: SharedString| {
        let name_str = dev_name.to_string();
        info!("Alto-falante selecionado: {}", name_str);
        *selected_output_store.lock().unwrap() = name_str.clone();
        gateway::set_persisted_output_device(name_str.clone());

        if let Some(ui) = app_weak_select_output.upgrade() {
            let current_devs: Vec<AudioDeviceItem> = ui.get_output_devices().iter().map(|mut item| {
                item.is_selected = item.name.as_str() == name_str;
                item
            }).collect();
            ui.set_output_devices(std::rc::Rc::new(slint::VecModel::from(current_devs)).into());
            ui.set_selected_output_device(name_str.clone().into());

            // If mic testing ("se ouvir") is active, restart output loopback stream for new device
            if gateway::is_testing_mic() {
                if let Some(stream) = start_mic_loopback_stream(name_str) {
                    *active_loopback_output.lock().unwrap() = Some(stream);
                }
            }
        }
    });

    app.on_set_vad_threshold(move |threshold: f32| {
        gateway::set_vad_threshold(threshold);
    });

    let app_weak_test_mic = app_weak.clone();
    let selected_output_test = Arc::clone(&selected_output);
    let active_loopback_test = Arc::clone(&active_loopback_stream);

    app.on_toggle_test_mic(move || {
        let now_testing = !gateway::is_testing_mic();
        gateway::set_testing_mic(now_testing);
        if let Some(ui) = app_weak_test_mic.upgrade() {
            ui.set_is_testing_mic(now_testing);
        }
        if now_testing {
            let cur_output = selected_output_test.lock().unwrap().clone();
            if let Some(stream) = start_mic_loopback_stream(cur_output) {
                *active_loopback_test.lock().unwrap() = Some(stream);
            }
        } else {
            *active_loopback_test.lock().unwrap() = None;
            if let Ok(mut q) = gateway::get_mic_loopback_queue().lock() {
                q.clear();
            }
        }
    });

    let app_weak_lang = app_weak.clone();
    app.on_change_language(move |code: SharedString| {
        let selected = i18n::Language::from_code(code.as_str());
        i18n::save_persisted_language_config(selected);
        if let Some(ui) = app_weak_lang.upgrade() {
            apply_i18n_translations(&ui, selected);
        }
        info!("🌐 Idioma alterado para: {:?} (código: {})", selected, code.as_str());
    });

    // Leave Voice Callback
    let cmd_tx_leave = Arc::clone(&cmd_tx_store);
    let active_guild_leave = Arc::clone(&active_guild_id);
    let app_weak_leave = app_weak.clone();
    let sm_leave = Arc::clone(&screen_manager);
    let hidden_streams_leave = Arc::clone(&hidden_streams);

    app.on_leave_voice(move || {
        info!("Usuário solicitou desconexão da sala de voz...");
        sm_leave.stop();
        sm_leave.set_context(0, 0, "");
        screen_capture::clear_stream_audio_queue();
        hidden_streams_leave.lock().unwrap().clear();
        if let Some(ui) = app_weak_leave.upgrade() {
            ui.set_is_in_voice(false);
            ui.set_is_voice_connecting(false);
            ui.set_current_voice_channel("".into());
            ui.set_is_screen_sharing(false);
            ui.set_local_preview_fps("".into());
            ui.set_has_active_stream(false);
            ui.set_popped_out_stream_uid("".into());
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
                id: "".into(),
                author: "Litecord Voice".into(),
                content: "🔴 Desconectado da sala de voz.".into(),
                commands: slint::ModelRc::default(),
                content_lines: slint::ModelRc::default(),
                embed_content: "".into(),
                embed_lines: slint::ModelRc::default(),
                embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                embed_footer: "".into(),
                code_block: "".into(),
                reply_author: "".into(),
                reply_content: "".into(),
                reply_command: "".into(),
                links: slint::ModelRc::default(),
                buttons: slint::ModelRc::default(),
                attachments: slint::ModelRc::default(),
                timestamp: "Agora".into(),
            });
            let model = std::rc::Rc::new(slint::VecModel::from(current_msgs));
            ui.set_messages(model.into());
        }
    });

    // Auto-Update Callbacks
    let app_weak_update_acc = app_weak.clone();
    app.on_update_app_accept(move || {
        let rel_opt = get_pending_update_store().lock().unwrap().clone();
        if let Some(rel) = rel_opt {
            if let Some(ui) = app_weak_update_acc.upgrade() {
                ui.set_is_updating(true);
                ui.set_update_progress(0.0);
                ui.set_update_status_text("Baixando atualização...".into());
            }

            let (prog_tx, mut prog_rx) = mpsc::channel::<f32>(50);
            let app_w_prog = app_weak_update_acc.clone();
            tokio::spawn(async move {
                while let Some(prog) = prog_rx.recv().await {
                    let app_w = app_w_prog.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_update_progress(prog);
                        }
                    });
                }
            });

            let app_w_fail = app_weak_update_acc.clone();
            tokio::spawn(async move {
                if let Err(e) = updater::download_and_install_update(rel.download_url, prog_tx).await {
                    error!("Erro ao atualizar Litecord: {}", e);
                    let app_w = app_w_fail.clone();
                    let err_msg = format!("Falha na atualização: {}", e);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_is_updating(false);
                            ui.set_update_status_text(err_msg.into());
                        }
                    });
                }
            });
        }
    });

    let app_weak_update_dism = app_weak.clone();
    app.on_update_app_dismiss(move || {
        if let Some(ui) = app_weak_update_dism.upgrade() {
            ui.set_show_update_dialog(false);
        }
    });

    let app_weak_update_ign = app_weak.clone();
    app.on_update_app_ignore(move || {
        if let Some(rel) = get_pending_update_store().lock().unwrap().as_ref() {
            updater::save_ignored_version(&rel.tag_name);
        }
        if let Some(ui) = app_weak_update_ign.upgrade() {
            ui.set_show_update_dialog(false);
        }
    });

    // Manual "Check for Updates" Button Callback
    let app_weak_manual_chk = app_weak.clone();
    app.on_check_for_updates(move || {
        let app_w = app_weak_manual_chk.clone();

        if let Some(ui) = app_w.upgrade() {
            ui.set_is_checking_update(true);
            ui.set_update_check_feedback("Verificando atualizações no GitHub...".into());
        }

        tokio::spawn(async move {
            let res = updater::check_for_updates_manual().await;
            let app_w_inner = app_w.clone();

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w_inner.upgrade() {
                    ui.set_is_checking_update(false);
                    match res {
                        Ok(Some(rel)) => {
                            *get_pending_update_store().lock().unwrap() = Some(rel.clone());
                            ui.set_update_check_feedback(format!("Nova versão disponível: v{}", rel.version).into());
                            ui.set_update_version(format!("v{}", rel.version).into());
                            ui.set_update_release_name(rel.release_name.into());
                            ui.set_show_update_dialog(true);
                        }
                        Ok(None) => {
                            ui.set_update_check_feedback(format!("✅ Você já está na versão mais recente (v{})!", env!("CARGO_PKG_VERSION")).into());
                        }
                        Err(e) => {
                            ui.set_update_check_feedback(format!("⚠️ {}", e).into());
                        }
                    }
                }
            });
        });
    });

    // Chat Pagination: Load Older Messages Callback
    let app_weak_older = app_weak.clone();
    let http_client_older = Arc::clone(&http_client);
    let active_ch_older = Arc::clone(&active_channel_id);
    app.on_load_older_messages(move || {
        let app_w = app_weak_older.clone();
        let http_opt = http_client_older.lock().unwrap().clone();
        let ch_id = active_ch_older.lock().unwrap().clone();

        if ch_id.is_empty() {
            return;
        }

        let oldest_id_opt = get_oldest_message_map().lock().unwrap().get(&ch_id).cloned();
        let oldest_id = match oldest_id_opt {
            Some(id) if !id.is_empty() => id,
            _ => return,
        };

        if let Some(http) = http_opt {
            if let Some(ui) = app_w.upgrade() {
                ui.set_is_loading_older_messages(true);
            }

            let app_w_inner = app_w.clone();
            let ch_id_clone = ch_id.clone();
            tokio::spawn(async move {
                match http.get_channel_messages_before(&ch_id_clone, &oldest_id).await {
                    Ok(msgs_val) => {
                        let has_more = msgs_val.len() >= 30;
                        if let Some(oldest) = msgs_val.last() {
                            if let Some(id_str) = oldest["id"].as_str() {
                                get_oldest_message_map().lock().unwrap().insert(ch_id_clone.clone(), id_str.to_string());
                            }
                        }

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_w_inner.upgrade() {
                                ui.set_is_loading_older_messages(false);
                                ui.set_has_more_older_messages(has_more);

                                let older_slint_msgs: Vec<ChatMessage> = msgs_val.iter().rev().map(|m| {
                                    let msg_id = m["id"].as_str().unwrap_or("");
                                    let author = format_discord_author(m);
                                    let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(m);
                                    
                                    let slint_cmds: Vec<slint::SharedString> = commands.into_iter().map(|c| c.into()).collect();
                                    let commands_model = std::rc::Rc::new(slint::VecModel::from(slint_cmds));

                                    let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
                                        label: l.label.clone().into(),
                                        url: l.url.clone().into(),
                                    }).collect();
                                    let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

                                    ChatMessage {
                                        id: msg_id.into(),
                                        author: author.into(),
                                        content: content.into(),
                                        commands: slint::ModelRc::from(commands_model),
                                        content_lines: map_message_lines(&content_lines, &ch_id_clone, &app_w_inner),
                                        embed_content: embed_content.into(),
                                        embed_lines: map_message_lines(&embed_lines, &ch_id_clone, &app_w_inner),
                                        embed_color: parse_hex_color(&embed_color),
                                        embed_footer: embed_footer.into(),
                                        code_block: code_block.into(),
                                        reply_author: reply_author.into(),
                                        reply_content: reply_content.into(),
                                        reply_command: reply_command.into(),
                                        links: slint::ModelRc::from(links_model),
                                        buttons: map_message_buttons(&buttons, &ch_id_clone, &app_w_inner),
                                        attachments: map_message_attachments(&attachments, &app_w_inner),
                                        timestamp: "Anterior".into(),
                                    }
                                }).collect();

                                let current_msgs: Vec<ChatMessage> = ui.get_messages().iter().collect();
                                let mut combined = older_slint_msgs;
                                combined.extend(current_msgs);
                                let model = std::rc::Rc::new(slint::VecModel::from(combined));
                                ui.set_messages(model.into());
                            }
                        });
                    }
                    Err(e) => {
                        error!("Erro ao carregar mensagens anteriores: {}", e);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_w_inner.upgrade() {
                                ui.set_is_loading_older_messages(false);
                            }
                        });
                    }
                }
            });
        }
    });

    // Active QR Code Remote Auth Session
    let (qr_tx, mut qr_rx) = mpsc::channel::<RemoteAuthEvent>(50);
    let active_qr_session: Arc<Mutex<Option<remote_auth::RemoteAuthSession>>> = Arc::new(Mutex::new(None));

    let qr_tx_refresh = qr_tx.clone();
    let active_qr_refresh = Arc::clone(&active_qr_session);
    let app_weak_refresh = app_weak.clone();
    app.on_refresh_qr_code(move || {
        info!("🔄 Atualizando / reiniciando QR Code de login...");
        if let Some(ui) = app_weak_refresh.upgrade() {
            ui.set_has_qr_code(false);
            ui.set_qr_scanned_user("".into());
        }
        if let Some(old) = active_qr_refresh.lock().unwrap().take() {
            old.cancel();
        }
        let session = remote_auth::RemoteAuthSession::start(qr_tx_refresh.clone());
        *active_qr_refresh.lock().unwrap() = Some(session);
    });

    // QR Code Event Processing Loop
    let app_weak_qr = app_weak.clone();
    let http_client_qr = Arc::clone(&http_client);
    let last_token_qr = Arc::clone(&last_token);
    let guilds_map_qr = Arc::clone(&guilds_map);
    let active_guild_qr = Arc::clone(&active_guild_id);
    let active_channel_qr = Arc::clone(&active_channel_id);
    let cmd_tx_qr = Arc::clone(&cmd_tx_store);
    let event_tx_qr = event_tx.clone();
    let active_qr_session_qr = Arc::clone(&active_qr_session);

    tokio::spawn(async move {
        while let Some(evt) = qr_rx.recv().await {
            match evt {
                RemoteAuthEvent::QrCodeUrl(url) => {
                    let app_w = app_weak_qr.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Ok(img) = remote_auth::generate_qr_image(&url) {
                            if let Some(ui) = app_w.upgrade() {
                                ui.set_qr_code_image(img);
                                ui.set_has_qr_code(true);
                                ui.set_qr_scanned_user("".into());
                            }
                        }
                    });
                }
                RemoteAuthEvent::UserScanned { username, .. } => {
                    let app_w = app_weak_qr.clone();
                    let uname = username.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_qr_scanned_user(uname.into());
                        }
                    });
                }
                RemoteAuthEvent::TokenReceived(token) => {
                    info!("🎉 Token de acesso recebido via QR Code com sucesso! Conectando...");
                    let app_w = app_weak_qr.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_has_qr_code(false);
                            ui.set_qr_scanned_user("".into());
                            ui.set_connection_status("Conectando via QR Code...".into());
                        }
                    });
                    if let Some(old) = active_qr_session_qr.lock().unwrap().take() {
                        old.cancel();
                    }
                    try_login_with_candidates(
                        vec![token],
                        app_weak_qr.clone(),
                        Arc::clone(&http_client_qr),
                        Arc::clone(&last_token_qr),
                        Arc::clone(&guilds_map_qr),
                        Arc::clone(&active_guild_qr),
                        Arc::clone(&active_channel_qr),
                        Arc::clone(&cmd_tx_qr),
                        event_tx_qr.clone(),
                    ).await;
                }
                RemoteAuthEvent::Cancelled => {
                    let app_w = app_weak_qr.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_has_qr_code(false);
                            ui.set_qr_scanned_user("".into());
                        }
                    });
                }
                RemoteAuthEvent::Error(err) => {
                    warn!("⚠️ Erro na sessão de QR Auth: {}", err);
                    let app_w = app_weak_qr.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_has_qr_code(false);
                        }
                    });
                }
            }
        }
    });

    // Logout Callback from UI
    let app_weak_logout = app_weak.clone();
    let http_client_logout = Arc::clone(&http_client);
    let last_token_logout = Arc::clone(&last_token);
    let guilds_map_logout = Arc::clone(&guilds_map);
    let active_guild_logout = Arc::clone(&active_guild_id);
    let active_channel_logout = Arc::clone(&active_channel_id);
    let cmd_tx_logout = Arc::clone(&cmd_tx_store);
    let active_mic_logout = Arc::clone(&active_mic_stream);
    let active_loopback_logout = Arc::clone(&active_loopback_stream);
    let qr_tx_logout = qr_tx.clone();
    let active_qr_logout = Arc::clone(&active_qr_session);

    app.on_logout(move || {
        info!("Usuário solicitou Logout da conta...");
        // 1. Remove saved token file so auto-login is cancelled
        let _ = std::fs::remove_file(".litecord_token");

        // 2. Disconnect voice session if active
        gateway::clear_voice_participants();
        gateway::CURRENT_VOICE_SESSION_ID.store(0, std::sync::atomic::Ordering::SeqCst);
        gateway::set_testing_mic(false);

        // 3. Send disconnect command if gateway is active
        let gid = active_guild_logout.lock().unwrap().clone();
        if let Some(cmd_tx) = cmd_tx_logout.lock().unwrap().as_ref() {
            let _ = cmd_tx.try_send(GatewayCommand::UpdateVoiceState {
                guild_id: gid,
                channel_id: None,
                self_mute: false,
                self_deaf: false,
            });
        }

        // 4. Reset backend states
        *cmd_tx_logout.lock().unwrap() = None;
        *http_client_logout.lock().unwrap() = None;
        *last_token_logout.lock().unwrap() = String::new();
        *guilds_map_logout.lock().unwrap() = std::collections::HashMap::new();
        *active_guild_logout.lock().unwrap() = String::new();
        *active_channel_logout.lock().unwrap() = String::new();
        *active_mic_logout.lock().unwrap() = None;
        *active_loopback_logout.lock().unwrap() = None;
        if let Ok(mut q) = gateway::get_mic_loopback_queue().lock() {
            q.clear();
        }

        // 5. Update Slint UI to login state
        if let Some(ui) = app_weak_logout.upgrade() {
            ui.set_is_logged_in(false);
            ui.set_user_tag("Não conectado".into());
            ui.set_connection_status("Você saiu da conta. Insira seu token ou escaneie o QR Code.".into());
            ui.set_is_in_voice(false);
            ui.set_current_voice_channel("".into());
            ui.set_show_user_menu(false);
            ui.set_show_settings_modal(false);
            ui.set_is_testing_mic(false);
            ui.set_guilds(std::rc::Rc::new(slint::VecModel::default()).into());
            ui.set_channels(std::rc::Rc::new(slint::VecModel::default()).into());
            ui.set_voice_participants(std::rc::Rc::new(slint::VecModel::default()).into());
            ui.set_has_qr_code(false);
            ui.set_qr_scanned_user("".into());
        }

        // 6. Start fresh QR Code session for fast re-login
        if let Some(old) = active_qr_logout.lock().unwrap().take() {
            old.cancel();
        }
        let session = remote_auth::RemoteAuthSession::start(qr_tx_logout.clone());
        *active_qr_logout.lock().unwrap() = Some(session);
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
    let active_qr_login = Arc::clone(&active_qr_session);

    app.on_login(move |token: SharedString| {
        let token_str = token.to_string().chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();
        info!("Token fornecido. Validando via API HTTP...");
        *last_token_login.lock().unwrap() = token_str.clone();

        if let Some(old) = active_qr_login.lock().unwrap().take() {
            old.cancel();
        }

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
                    save_secure_token(&token_str);

                    let app_w = app_weak_inner.clone();
                    let username_clone = username.to_string();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w.upgrade() {
                            ui.set_is_logged_in(true);
                            ui.set_connection_status(format!("Conectado como {}!", username_clone).into());
                        }
                    });

                    // Start Gateway WebSocket connection with Opcode 4 command channel
                    let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(100);
                    *cmd_tx_gw_store.lock().unwrap() = Some(cmd_tx);

                    // Fetch all servers and channels via HTTP REST immediately!
                    fetch_and_populate_guilds(
                        &http,
                        app_weak_inner.clone(),
                        guilds_map_in,
                        active_guild_in,
                        active_channel_in,
                        cmd_tx_gw_store,
                    ).await;

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
    let active_qr_auto = Arc::clone(&active_qr_session);

    app.on_auto_detect_token(move || {
        if let Some(old) = active_qr_auto.lock().unwrap().take() {
            old.cancel();
        }

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
    let qr_tx_startup = qr_tx.clone();
    let active_qr_startup = Arc::clone(&active_qr_session);

    tokio::spawn(async move {
        // Only auto-connect if user explicitly saved their token previously
        let saved_opt = load_secure_token();

        if let Some(saved) = saved_opt {
            let app_w_status = app_weak_startup.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w_status.upgrade() {
                    ui.set_connection_status("Autoconectando à Gateway v9...".into());
                }
            });

            let success = try_login_with_candidates(
                vec![saved],
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
                let session = remote_auth::RemoteAuthSession::start(qr_tx_startup.clone());
                *active_qr_startup.lock().unwrap() = Some(session);
            }
        } else {
            // No saved token (e.g. fresh install or after explicit logout): start QR session immediately!
            info!("Nenhum token salvo em disco. Iniciando sessão de QR Code...");
            let session = remote_auth::RemoteAuthSession::start(qr_tx_startup.clone());
            *active_qr_startup.lock().unwrap() = Some(session);
        }
    });



    // Select Guild Callback
    let guilds_map_select = Arc::clone(&guilds_map);
    let active_guild_select = Arc::clone(&active_guild_id);
    let app_weak_guild_select = app_weak.clone();
    let http_client_guild_select = Arc::clone(&http_client);
    let active_channel_guild_select = Arc::clone(&active_channel_id);
    let cmd_tx_guild_select = Arc::clone(&cmd_tx_store);

    app.on_select_guild(move |guild_id: SharedString| {
        let gid = guild_id.to_string();
        info!("Servidor selecionado pelo usuário: {}", gid);

        if let Some(tx) = cmd_tx_guild_select.lock().unwrap().as_ref() {
            let _ = tx.try_send(GatewayCommand::SubscribeGuild { guild_id: gid.clone(), channel_ids: Vec::new() });
        }

        let http_opt = http_client_guild_select.lock().unwrap().as_ref().cloned();
        let app_w = app_weak_guild_select.clone();
        let guilds_map_in = Arc::clone(&guilds_map_select);
        let active_g_in = Arc::clone(&active_guild_select);
        let active_c_in = Arc::clone(&active_channel_guild_select);
        let cmd_tx_in = Arc::clone(&cmd_tx_guild_select);

        if let Some(http) = http_opt {
            tokio::spawn(async move {
                fetch_and_populate_channels(
                    &http,
                    app_w,
                    guilds_map_in,
                    active_g_in,
                    active_c_in,
                    cmd_tx_in,
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
    let sm_chan_select = Arc::clone(&screen_manager);

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
                ui.set_is_voice_connecting(true);
                ui.set_is_voice_focused(true);
                ui.set_current_voice_channel(ch_name.clone().into());
                gateway::sync_voice_channel_participants(&ch_id);
                let muted = ui.get_is_muted();
                let deafened = ui.get_is_deafened();

                let cid_num = ch_id.parse::<u64>().unwrap_or(0);
                let my_uid = gateway::get_my_user_id();
                let my_uname = gateway::get_my_username();
                sm_chan_select.set_context(cid_num, my_uid, &my_uname);
                sm_chan_select.announce_presence();

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
                    id: "".into(),
                    author: "Litecord Voice".into(),
                    content: format!("🔊 Entrou no canal de voz: {}", ch_name).into(),
                    commands: slint::ModelRc::default(),
                    content_lines: slint::ModelRc::default(),
                    embed_content: "".into(),
                    embed_lines: slint::ModelRc::default(),
                    embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                    embed_footer: "".into(),
                    code_block: "".into(),
                    reply_author: "".into(),
                    reply_content: "".into(),
                    reply_command: "".into(),
                    links: slint::ModelRc::default(),
                    buttons: slint::ModelRc::default(),
                    attachments: slint::ModelRc::default(),
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
            gateway::set_self_deaf(new_deafened);

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

    // Click Command Callback (fills input with command parameter pill)
    let app_weak_cmd = app_weak.clone();
    app.on_click_command(move |cmd: SharedString| {
        if let Some(ui) = app_weak_cmd.upgrade() {
            let cmd_str = cmd.to_string();
            let clean_cmd = if cmd_str.starts_with('/') { cmd_str } else { format!("/{}", cmd_str) };
            
            let master = get_master_command_suggestions().lock().unwrap().clone();
            if let Some(found) = master.iter().find(|s| s.name.to_lowercase() == clean_cmd.to_lowercase()) {
                ui.set_active_command_name(found.name.clone().into());
                ui.set_active_param_name(found.param_name.clone().into());
                let pdesc = if found.param_desc.is_empty() {
                    if found.param_name.is_empty() { format!("Pressione Enter para executar {}", found.name) } else { found.desc.clone() }
                } else {
                    found.param_desc.clone()
                };
                ui.set_active_param_desc(pdesc.into());
                ui.set_active_command_app_id(found.app_id.clone().into());
                ui.set_active_command_cmd_id(found.cmd_id.clone().into());
                ui.set_active_command_version(found.version.clone().into());
            } else {
                ui.set_active_command_name(clean_cmd.clone().into());
                ui.set_active_param_name("".into());
                ui.set_active_param_desc(format!("Pressione Enter para executar {}", clean_cmd).into());
                ui.set_active_command_app_id("".into());
                ui.set_active_command_cmd_id("".into());
                ui.set_active_command_version("".into());
            }
            ui.set_command_arg_text("".into());
            ui.set_chat_input_text("".into());
            ui.set_show_command_suggestions(false);
        }
    });

    // Handle Input Change Callback (ranked real-time autocomplete filtering)
    let app_weak_inp = app_weak.clone();
    app.on_handle_input_change(move |text: SharedString| {
        if let Some(ui) = app_weak_inp.upgrade() {
            let t = text.to_string();
            let trimmed = t.trim();
            if trimmed.starts_with('/') {
                let query = trimmed.to_lowercase();
                let master = get_master_command_suggestions().lock().unwrap().clone();
                
                // If user typed "/play " (with trailing space and matched a command):
                if t.ends_with(' ') && t.starts_with('/') {
                    let cmd_word = t.trim();
                    if let Some(found) = master.iter().find(|c| c.name.to_lowercase() == cmd_word.to_lowercase()) {
                        ui.set_active_command_name(found.name.clone().into());
                        ui.set_active_param_name(found.param_name.clone().into());
                        let pdesc = if found.param_desc.is_empty() {
                            if found.param_name.is_empty() { format!("Pressione Enter para executar {}", found.name) } else { found.desc.clone() }
                        } else {
                            found.param_desc.clone()
                        };
                        ui.set_active_param_desc(pdesc.into());
                        ui.set_active_command_app_id(found.app_id.clone().into());
                        ui.set_active_command_cmd_id(found.cmd_id.clone().into());
                        ui.set_active_command_version(found.version.clone().into());
                        ui.set_command_arg_text("".into());
                        ui.set_chat_input_text("".into());
                        ui.set_show_command_suggestions(false);
                        return;
                    }
                }

                let mut scored: Vec<(i32, CommandSuggestionItem)> = master.into_iter().filter_map(|cmd| {
                    let name_lower = cmd.name.to_lowercase();
                    let desc_lower = cmd.desc.to_lowercase();
                    let app_lower = cmd.app_name.to_lowercase();
                    
                    if name_lower == query {
                        Some((1000, cmd))
                    } else if name_lower.starts_with(&query) {
                        Some((500 - (name_lower.len() as i32), cmd))
                    } else if name_lower.contains(&query) {
                        Some((200, cmd))
                    } else if app_lower.contains(&query.trim_start_matches('/')) {
                        Some((150, cmd))
                    } else if desc_lower.contains(&query.trim_start_matches('/')) {
                        Some((100, cmd))
                    } else if query == "/" {
                        Some((50, cmd))
                    } else {
                        None
                    }
                }).collect();

                scored.sort_by(|a, b| b.0.cmp(&a.0));

                let filtered_suggestions: Vec<CommandSuggestion> = scored.into_iter().map(|(_, item)| CommandSuggestion {
                    name: item.name.into(),
                    desc: item.desc.into(),
                    usage: item.usage.into(),
                    app_id: item.app_id.into(),
                    app_name: item.app_name.into(),
                    cmd_id: item.cmd_id.into(),
                    version: item.version.into(),
                    param_name: item.param_name.into(),
                    param_desc: item.param_desc.into(),
                    is_required: item.is_required,
                }).collect();

                let is_empty = filtered_suggestions.is_empty();
                let model = std::rc::Rc::new(slint::VecModel::from(filtered_suggestions));
                ui.set_command_suggestions(model.into());
                ui.set_show_command_suggestions(!is_empty);
            } else {
                ui.set_show_command_suggestions(false);
            }
        }
    });

    // Execute Active Command Callback (Discord Slash Command interaction or text message fallback)
    let http_client_cmd = Arc::clone(&http_client);
    let active_channel_cmd = Arc::clone(&active_channel_id);
    let active_guild_cmd = Arc::clone(&active_guild_id);
    app.on_execute_active_command(move |cmd_name: SharedString, arg_value: SharedString, app_id: SharedString, cmd_id: SharedString, version: SharedString, param_name: SharedString| {
        let cmd_name_str = cmd_name.to_string();
        let arg_val_str = arg_value.to_string();
        let app_id_str = app_id.to_string();
        let cmd_id_str = cmd_id.to_string();
        let version_str = version.to_string();
        let param_name_str = param_name.to_string();
        
        let channel_id = active_channel_cmd.lock().unwrap().clone();
        let guild_id = active_guild_cmd.lock().unwrap().clone();
        let http_opt = http_client_cmd.lock().unwrap().as_ref().cloned();

        if let Some(http) = http_opt {
            tokio::spawn(async move {
                let clean_name = cmd_name_str.trim_start_matches('/').to_string();
                if !app_id_str.is_empty() && !cmd_id_str.is_empty() {
                    let mut options = Vec::new();
                    if !arg_val_str.is_empty() {
                        let opt_name = if param_name_str.is_empty() { "query".to_string() } else { param_name_str };
                        options.push(serde_json::json!({
                            "type": 3,
                            "name": opt_name,
                            "value": arg_val_str
                        }));
                    }
                    if let Err(e) = http.send_slash_command_interaction(
                        &guild_id,
                        &channel_id,
                        &app_id_str,
                        &cmd_id_str,
                        &clean_name,
                        &version_str,
                        options,
                        ""
                    ).await {
                        warn!("Falha ao enviar slash command interaction (tentando fallback): {}", e);
                        let fallback_msg = if arg_val_str.is_empty() {
                            format!("/{}", clean_name)
                        } else {
                            format!("/{} {}", clean_name, arg_val_str)
                        };
                        let _ = http.send_message(&channel_id, &fallback_msg).await;
                    }
                } else {
                    let fallback_msg = if arg_val_str.is_empty() {
                        cmd_name_str
                    } else {
                        format!("{} {}", cmd_name_str, arg_val_str)
                    };
                    let _ = http.send_message(&channel_id, &fallback_msg).await;
                }
            });
        }
    });

    // Minimize to Tray Callback using Win32 ShowWindow(SW_HIDE)
    let app_weak_tray_min = app_weak.clone();
    let hwnd_store_minimize = Arc::clone(&hwnd_store);
    app.on_minimize_to_tray(move || {
        info!("Minimizando janela para a bandeja do sistema via Win32 SW_HIDE...");
        APP_IS_VISIBLE.store(false, Ordering::Relaxed);
        PENDING_MESSAGES.store(0, Ordering::Relaxed);
        info!("[DeepSleep] UI loops suspensos. Apenas áudio permanece ativo.");
        let app_w = app_weak_tray_min.clone();
        let hwnd_store_c = Arc::clone(&hwnd_store_minimize);
        slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
            let mut target_hwnd = *hwnd_store_c.lock().unwrap();
            if let Some(ui) = app_w.upgrade() {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                let _ = ui.window().with_winit_window(|winit_win| {
                    winit_win.set_visible(false);
                    if target_hwnd.is_none() {
                        if let Ok(handle) = winit_win.window_handle() {
                            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                                target_hwnd = Some(win32_handle.hwnd.get() as isize);
                            }
                        }
                    }
                });
            }
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = target_hwnd {
                unsafe {
                    ShowWindow(hwnd as _, SW_HIDE);
                    use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
                    use windows_sys::Win32::System::Threading::GetCurrentProcess;
                    K32EmptyWorkingSet(GetCurrentProcess());
                }
            }
        });
    });

    // Custom window controls for frameless mode
    let app_weak_drag = app_weak.clone();
    static LAST_DRAG_TS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    app.on_drag_window(move || {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let last = LAST_DRAG_TS.load(Ordering::Relaxed);
        if now.saturating_sub(last) < 250 {
            return;
        }
        LAST_DRAG_TS.store(now, Ordering::Relaxed);
        if let Some(app_instance) = app_weak_drag.upgrade() {
            app_instance.set_drag_area_enabled(false);
            let _ = app_instance.window().with_winit_window(|winit_window| {
                let _ = winit_window.drag_window();
            });
            let app_w = app_weak_drag.clone();
            slint::Timer::single_shot(std::time::Duration::from_millis(150), move || {
                if let Some(app_inst) = app_w.upgrade() {
                    app_inst.set_drag_area_enabled(true);
                }
            });
        }
    });

    let app_weak_resize = app_weak.clone();
    app.on_drag_resize(move |edge: SharedString| {
        if let Some(app_instance) = app_weak_resize.upgrade() {
            app_instance.set_drag_area_enabled(false);
            let _ = app_instance.window().with_winit_window(|winit_window| {
                let dir = match edge.as_str() {
                    "top" => winit::window::ResizeDirection::North,
                    "bottom" => winit::window::ResizeDirection::South,
                    "left" => winit::window::ResizeDirection::West,
                    "right" => winit::window::ResizeDirection::East,
                    "top-left" => winit::window::ResizeDirection::NorthWest,
                    "top-right" => winit::window::ResizeDirection::NorthEast,
                    "bottom-left" => winit::window::ResizeDirection::SouthWest,
                    "bottom-right" => winit::window::ResizeDirection::SouthEast,
                    _ => return,
                };
                let _ = winit_window.drag_resize_window(dir);
            });
            let app_w = app_weak_resize.clone();
            slint::Timer::single_shot(std::time::Duration::from_millis(150), move || {
                if let Some(app_inst) = app_w.upgrade() {
                    app_inst.set_drag_area_enabled(true);
                }
            });
        }
    });

    let app_weak_min = app_weak.clone();
    let hwnd_store_min = Arc::clone(&hwnd_store);
    app.on_minimize_window(move || {
        info!("🔕 Minimizando janela para a bandeja do sistema (System Tray) & entrando em DeepSleep...");
        APP_IS_VISIBLE.store(false, Ordering::Relaxed);
        PENDING_MESSAGES.store(0, Ordering::Relaxed);
        info!("[DeepSleep] UI loops suspensos. Apenas áudio permanece ativo.");
        let app_w = app_weak_min.clone();
        let hwnd_store_c = Arc::clone(&hwnd_store_min);
        slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
            let mut target_hwnd = *hwnd_store_c.lock().unwrap();
            if let Some(ui) = app_w.upgrade() {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                let _ = ui.window().with_winit_window(|winit_win| {
                    winit_win.set_visible(false);
                    winit_win.set_minimized(true);
                    if target_hwnd.is_none() {
                        if let Ok(handle) = winit_win.window_handle() {
                            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                                target_hwnd = Some(win32_handle.hwnd.get() as isize);
                            }
                        }
                    }
                });
            }
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = target_hwnd {
                unsafe {
                    ShowWindow(hwnd as _, SW_HIDE);
                    use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
                    use windows_sys::Win32::System::Threading::GetCurrentProcess;
                    K32EmptyWorkingSet(GetCurrentProcess());
                    info!("[DeepSleep] Memória RAM liberada via K32EmptyWorkingSet!");
                }
            }
        });
    });

    let app_weak_max = app_weak.clone();
    app.on_maximize_window(move || {
        if let Some(app_instance) = app_weak_max.upgrade() {
            let is_max = app_instance.window().is_maximized();
            app_instance.window().set_maximized(!is_max);
        }
    });

    let sm_for_close = Arc::clone(&screen_manager);
    app.on_close_window(move || {
        info!("Fechar clicado na barra superior: saindo do aplicativo...");
        sm_for_close.stop();
        #[cfg(target_os = "linux")]
        screen_capture::kill_portal_child();
        std::process::exit(0);
    });

    // Handle Gateway Events in Tokio Task and Dispatch to Slint UI Thread
    let app_weak_gw_events = app_weak.clone();
    let last_token_gw_save = Arc::clone(&last_token);
    let active_guild_gw_events = Arc::clone(&active_guild_id);
    let active_channel_gw_events = Arc::clone(&active_channel_id);
    let guilds_map_gw_events = Arc::clone(&guilds_map);
    let sm_gw = Arc::clone(&screen_manager);

    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let app_weak_inner = app_weak_gw_events.clone();
            let last_token_inner = Arc::clone(&last_token_gw_save);
            let active_guild_inner = Arc::clone(&active_guild_gw_events);
            let active_channel_inner = Arc::clone(&active_channel_gw_events);
            let guilds_map_inner = Arc::clone(&guilds_map_gw_events);
            let sm_gw_inner = Arc::clone(&sm_gw);

            match event {
                GatewayEvent::VoiceDisconnected => {
                    let app_w = app_weak.clone();
                    let sm_disc = Arc::clone(&sm_gw_inner);
                    let _ = slint::invoke_from_event_loop(move || {
                        sm_disc.stop();
                        sm_disc.set_context(0, 0, "");
                        screen_capture::clear_stream_audio_queue();
                        if let Some(ui) = app_w.upgrade() {
                            info!("🚪 Desconectando UI do canal de voz (sessão encerrada/deslocada por outro cliente)");
                            ui.set_is_in_voice(false);
                            ui.set_is_voice_connecting(false);
                            ui.set_is_screen_sharing(false);
                            ui.set_has_active_stream(false);
                            ui.set_popped_out_stream_uid("".into());
                            ui.set_voice_participants(slint::ModelRc::new(slint::VecModel::from(vec![])));
                        }
                    });
                }
                GatewayEvent::Connected { user_tag } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak_inner.upgrade() {
                            ui.set_is_logged_in(true);
                            ui.set_user_tag(user_tag.into());
                            if let Ok(tok) = last_token_inner.lock() {
                                save_secure_token(tok.as_str());
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
                GatewayEvent::MessageCreated { id, channel_id, author, content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments, timestamp } => {
                    let current_active_ch = active_channel_inner.lock().unwrap().clone();
                    if channel_id != current_active_ch {
                        // Message is for a different channel or different server — IGNORE from current chat UI!
                        continue;
                    }

                    if !APP_IS_VISIBLE.load(Ordering::Relaxed) {
                        // Window is hidden — count message but don't touch Slint.
                        // Messages will be re-fetched via REST when the window is restored.
                        let pending = PENDING_MESSAGES.fetch_add(1, Ordering::Relaxed) + 1;
                        info!("[DeepSleep] Mensagem recebida em background ({} pendente(s)) — UI ignorada.", pending);
                    } else {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_weak_inner.upgrade() {
                                let mut current_msgs: Vec<ChatMessage> = ui.get_messages().iter().collect();

                                let slint_cmds: Vec<slint::SharedString> = commands.into_iter().map(|c| c.into()).collect();
                                let commands_model = std::rc::Rc::new(slint::VecModel::from(slint_cmds));

                                let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
                                    label: l.label.clone().into(),
                                    url: l.url.clone().into(),
                                    }).collect();
                                let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

                                current_msgs.push(ChatMessage {
                                    id: id.into(),
                                    author: author.into(),
                                    content: content.into(),
                                    commands: slint::ModelRc::from(commands_model),
                                    content_lines: map_message_lines(&content_lines, &channel_id, &app_weak_inner),
                                    embed_content: embed_content.into(),
                                    embed_lines: map_message_lines(&embed_lines, &channel_id, &app_weak_inner),
                                    embed_color: parse_hex_color(&embed_color),
                                    embed_footer: embed_footer.into(),
                                    code_block: code_block.into(),
                                    reply_author: reply_author.into(),
                                    reply_content: reply_content.into(),
                                    reply_command: reply_command.into(),
                                    links: slint::ModelRc::from(links_model),
                                    buttons: map_message_buttons(&buttons, &channel_id, &app_weak_inner),
                                    attachments: map_message_attachments(&attachments, &app_weak_inner),
                                    timestamp: timestamp.into(),
                                });
                                let model = std::rc::Rc::new(slint::VecModel::from(current_msgs));
                                ui.set_messages(model.into());

                                let app_w_scroll = app_weak_inner.clone();
                                slint::Timer::single_shot(std::time::Duration::from_millis(20), move || {
                                    if let Some(ui) = app_w_scroll.upgrade() {
                                        ui.invoke_scroll_chat_to_bottom();
                                    }
                                });
                            }
                        });
                    }
                }
                GatewayEvent::MessageUpdated { id, channel_id, content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments } => {
                    let current_active_ch = active_channel_inner.lock().unwrap().clone();
                    if channel_id != current_active_ch {
                        continue;
                    }

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak_inner.upgrade() {
                            let mut current_msgs: Vec<ChatMessage> = ui.get_messages().iter().collect();

                            let slint_cmds: Vec<slint::SharedString> = commands.into_iter().map(|c| c.into()).collect();
                            let commands_model = std::rc::Rc::new(slint::VecModel::from(slint_cmds));

                            let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
                                label: l.label.clone().into(),
                                url: l.url.clone().into(),
                            }).collect();
                            let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

                            let mut found = false;
                            for msg in current_msgs.iter_mut() {
                                if !id.is_empty() && msg.id == id.as_str() {
                                    msg.content = content.clone().into();
                                    msg.commands = slint::ModelRc::from(commands_model.clone());
                                    msg.content_lines = map_message_lines(&content_lines, &channel_id, &app_weak_inner);
                                    msg.embed_content = embed_content.clone().into();
                                    msg.embed_lines = map_message_lines(&embed_lines, &channel_id, &app_weak_inner);
                                    msg.embed_color = parse_hex_color(&embed_color);
                                    msg.embed_footer = embed_footer.clone().into();
                                    msg.code_block = code_block.clone().into();
                                    if !reply_author.is_empty() {
                                        msg.reply_author = reply_author.clone().into();
                                    }
                                    if !reply_content.is_empty() {
                                        msg.reply_content = reply_content.clone().into();
                                    }
                                    if !reply_command.is_empty() {
                                        msg.reply_command = reply_command.clone().into();
                                    }
                                    msg.links = slint::ModelRc::from(links_model.clone());
                                    msg.buttons = map_message_buttons(&buttons, &channel_id, &app_weak_inner);
                                    msg.attachments = map_message_attachments(&attachments, &app_weak_inner);
                                    found = true;
                                    break;
                                }
                            }

                            if !found && (!content.is_empty() || !embed_content.is_empty() || !reply_command.is_empty() || !attachments.is_empty()) {
                                current_msgs.push(ChatMessage {
                                    id: id.into(),
                                    author: "Bot".into(),
                                    content: content.into(),
                                    commands: slint::ModelRc::from(commands_model),
                                    content_lines: map_message_lines(&content_lines, &channel_id, &app_weak_inner),
                                    embed_content: embed_content.into(),
                                    embed_lines: map_message_lines(&embed_lines, &channel_id, &app_weak_inner),
                                    embed_color: parse_hex_color(&embed_color),
                                    embed_footer: embed_footer.into(),
                                    code_block: code_block.into(),
                                    reply_author: reply_author.into(),
                                    reply_content: reply_content.into(),
                                    reply_command: reply_command.into(),
                                    links: slint::ModelRc::from(links_model),
                                    buttons: map_message_buttons(&buttons, &channel_id, &app_weak_inner),
                                    attachments: map_message_attachments(&attachments, &app_weak_inner),
                                    timestamp: "Agora".into(),
                                });
                            }

                            let model = std::rc::Rc::new(slint::VecModel::from(current_msgs));
                            ui.set_messages(model.into());
                        }
                    });
                }
                GatewayEvent::MessageDeleted { id, channel_id } => {
                    let current_active_ch = active_channel_inner.lock().unwrap().clone();
                    if channel_id != current_active_ch {
                        continue;
                    }

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_weak_inner.upgrade() {
                            let current_msgs: Vec<ChatMessage> = ui.get_messages().iter().filter(|m| m.id != id.as_str()).collect();
                            let model = std::rc::Rc::new(slint::VecModel::from(current_msgs));
                            ui.set_messages(model.into());
                        }
                    });
                }
                GatewayEvent::VoiceStatesUpdated => {
                    let gid = active_guild_inner.lock().unwrap().clone();
                    if !gid.is_empty() {
                        let channels_opt = {
                            if let Ok(map) = guilds_map_inner.lock() {
                                map.get(&gid).map(|g| g.channels.clone())
                            } else {
                                None
                            }
                        };

                        if let Some(chans) = channels_opt {
                            let ui_channels: Vec<ChannelItem> = chans.iter().map(|ch| {
                                let vcount = if ch.is_voice { gateway::get_voice_channel_participant_count(&ch.id) } else { 0 };
                                ChannelItem {
                                    id: ch.id.clone().into(),
                                    name: ch.name.clone().into(),
                                    is_voice: ch.is_voice,
                                    voice_count: vcount,
                                }
                            }).collect();

                            let app_w = app_weak_inner.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = app_w.upgrade() {
                                    let cur_chans = ui.get_channels();
                                    let mut needs_rebuild = cur_chans.row_count() != ui_channels.len();
                                    if !needs_rebuild {
                                        for (i, new_c) in ui_channels.iter().enumerate() {
                                            if let Some(old_c) = cur_chans.row_data(i) {
                                                if old_c.voice_count != new_c.voice_count || old_c.id != new_c.id {
                                                    cur_chans.set_row_data(i, new_c.clone());
                                                }
                                            } else {
                                                needs_rebuild = true;
                                                break;
                                            }
                                        }
                                    }
                                    if needs_rebuild {
                                        let model = std::rc::Rc::new(slint::VecModel::from(ui_channels));
                                        ui.set_channels(model.into());
                                    }
                                }
                            });
                        }
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

#[cfg(target_os = "windows")]
#[repr(C)]
#[allow(non_snake_case)]
struct DATA_BLOB {
    cbData: u32,
    pbData: *mut u8,
}

#[cfg(target_os = "windows")]
#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        pDataIn: *const DATA_BLOB,
        szDataDescr: *const u16,
        pOptionalEntropy: *const DATA_BLOB,
        pvReserved: *mut std::ffi::c_void,
        pPromptStruct: *mut std::ffi::c_void,
        dwFlags: u32,
        pDataOut: *mut DATA_BLOB,
    ) -> i32;
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

#[cfg(target_os = "windows")]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: isize,
        lpOperation: *const u16,
        lpFile: *const u16,
        lpParameters: *const u16,
        lpDirectory: *const u16,
        nShowCmd: i32,
    ) -> isize;
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[cfg(target_os = "windows")]
fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    let mut in_blob = DATA_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = DATA_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let res = unsafe {
        CryptProtectData(
            &mut in_blob,
            std::ptr::null(),
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

#[cfg(not(target_os = "windows"))]
fn dpapi_protect(_data: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "windows")]
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

#[cfg(not(target_os = "windows"))]
fn dpapi_unprotect(_data: &[u8]) -> Option<Vec<u8>> {
    None
}

fn save_secure_token(token_str: &str) {
    if let Some(encrypted) = dpapi_protect(token_str.as_bytes()) {
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &encrypted);
        let _ = std::fs::write(".litecord_token", format!("DPAPI:{}", b64));
    } else {
        let _ = std::fs::write(".litecord_token", token_str);
    }
}

fn load_secure_token() -> Option<String> {
    let raw = std::fs::read_to_string(".litecord_token").ok()?;
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("DPAPI:") {
        if let Ok(encrypted_bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, rest) {
            if let Some(decrypted_bytes) = dpapi_unprotect(&encrypted_bytes) {
                if let Ok(tok) = String::from_utf8(decrypted_bytes) {
                    let clean = tok.trim().to_string();
                    if !clean.is_empty() {
                        return Some(clean);
                    }
                }
            }
        }
    }
    // Fallback to plain string if legacy unencrypted format
    let clean = trimmed.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();
    if !clean.is_empty() {
        Some(clean)
    } else {
        None
    }
}

fn is_valid_token_chars(token: &str) -> bool {
    token.len() >= 50 && token.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
fn set_dark_titlebar_color(_hwnd: isize) {}

#[cfg(target_os = "windows")]
fn set_dark_titlebar_color(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
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

                save_secure_token(&token_str);
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

                trigger_update_check(app_weak.clone());

                let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(100);
                *cmd_tx_store.lock().unwrap() = Some(cmd_tx);

                fetch_and_populate_guilds(
                    &http,
                    app_weak.clone(),
                    guilds_map,
                    active_guild_id,
                    active_channel_id,
                    cmd_tx_store,
                ).await;

                let gw = Arc::new(GatewayClient::new(token_str, event_tx_gw));
                gw.start(cmd_rx).await;

                #[cfg(target_os = "windows")]
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    unsafe {
                        use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
                        use windows_sys::Win32::System::Threading::GetCurrentProcess;
                        K32EmptyWorkingSet(GetCurrentProcess());
                    }
                });

                return true;
            }
            Err(err_msg) => {
                info!("Candidato a token recusado pelo Discord ({}). Testando próximo candidato...", err_msg);
            }
        }
    }
    false
}

static PENDING_UPDATE_STORE: std::sync::OnceLock<Arc<Mutex<Option<updater::ReleaseInfo>>>> = std::sync::OnceLock::new();

pub fn get_pending_update_store() -> Arc<Mutex<Option<updater::ReleaseInfo>>> {
    PENDING_UPDATE_STORE.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}

static OLDEST_MESSAGE_MAP: std::sync::OnceLock<Arc<Mutex<HashMap<String, String>>>> = std::sync::OnceLock::new();

pub fn get_oldest_message_map() -> Arc<Mutex<HashMap<String, String>>> {
    OLDEST_MESSAGE_MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}

fn trigger_update_check(app_weak: slint::Weak<AppWindow>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        if let Some(rel) = updater::check_for_updates().await {
            info!("🔔 Atualização v{} pronta para ser exibida ao usuário!", rel.version);
            *get_pending_update_store().lock().unwrap() = Some(rel.clone());
            let app_w = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    ui.set_update_version(format!("v{}", rel.version).into());
                    ui.set_update_release_name(rel.release_name.into());
                    ui.set_show_update_dialog(true);
                }
            });
        }
    });
}

#[cfg(target_os = "linux")]
fn set_linux_window_keep_above(is_pinned: bool) {
    let pin_val = if is_pinned { "true" } else { "false" };
    let script_code = format!(
        "var list = workspace.windowList ? workspace.windowList() : (workspace.clientList ? workspace.clientList() : []);\nfor (var i = 0; i < list.length; ++i) {{\n    var w = list[i];\n    if (w.caption.indexOf(\"Transmiss\\u00e3o Desanexada\") !== -1 || (w.caption.indexOf(\"Litecord\") !== -1 && w.caption.indexOf(\"Ultra-Lightweight\") === -1)) {{\n        w.keepAbove = {};\n    }}\n}}\n",
        pin_val
    );

    let script_path = format!("/tmp/litecord_pin_{}.js", std::process::id());
    if std::fs::write(&script_path, script_code).is_ok() {
        let plugin_name = format!("litecord_pin_{}", std::process::id());
        let output = std::process::Command::new("busctl")
            .args(&[
                "--user",
                "call",
                "org.kde.KWin",
                "/Scripting",
                "org.kde.kwin.Scripting",
                "loadScript",
                "ss",
                &script_path,
                &plugin_name,
            ])
            .output();

        if let Ok(out) = output {
            let out_str = String::from_utf8_lossy(&out.stdout);
            if let Some(id_str) = out_str.trim().split_whitespace().last() {
                if let Ok(script_id) = id_str.parse::<i32>() {
                    let script_obj = format!("/Scripting/Script{}", script_id);
                    let _ = std::process::Command::new("busctl")
                        .args(&["--user", "call", "org.kde.KWin", &script_obj, "org.kde.kwin.Script", "run"])
                        .output();
                    let _ = std::process::Command::new("busctl")
                        .args(&["--user", "call", "org.kde.KWin", &script_obj, "org.kde.kwin.Script", "stop"])
                        .output();
                    let _ = std::process::Command::new("busctl")
                        .args(&["--user", "call", "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", "unloadScript", "s", &plugin_name])
                        .output();
                }
            }
        }
        let _ = std::fs::remove_file(&script_path);
    }
}
