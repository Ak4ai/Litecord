#![allow(dead_code)]


use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize}};
use log::{info, warn};

use crate::{
    AppWindow, ChannelItem, MessageLine, MessageBlock, MessageButton, MessageAttachment,
    CommandSuggestion, SettingOptionItem, LanguageItem,
};
use crate::http::DiscordHttpClient;
use crate::gateway::{self, ChannelData};
use crate::i18n;
use crate::video_settings;
use crate::emoji_cache;
use crate::attachment_cache;

pub fn request_chat_scroll_to_bottom(app_weak: slint::Weak<AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || {
        let delays = [15, 45, 100, 200, 400];
        for delay in delays {
            let app_w = app_weak.clone();
            slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
                if let Some(ui) = app_w.upgrade() {
                    ui.invoke_scroll_chat_to_bottom();
                }
            });
        }
    });
}

pub fn parse_hex_color(hex: &str) -> slint::Color {
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
pub static APP_IS_VISIBLE: AtomicBool = AtomicBool::new(true);

/// Set to true by the tray restore paths to signal a REST refresh is needed.
pub static NEED_UI_REFRESH: AtomicBool = AtomicBool::new(false);

/// Number of messages received while the window was hidden.
/// Cleared when the window is restored and a REST refresh is triggered.
pub static PENDING_MESSAGES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
pub struct RawGuildItem {
    id: String,
    name: String,
    icon: String,
    icon_path: Option<String>,
}

pub fn apply_i18n_translations(ui: &AppWindow, lang: i18n::Language) {
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
    #[cfg(target_os = "windows")]
    {
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
    }
    #[cfg(not(target_os = "windows"))]
    {
        ui.set_tr_settings_btn_test_start(match resolved {
            i18n::Language::Portuguese => "Testar Microfone (\"Se Ouvir\")".into(),
            i18n::Language::Spanish => "Probar Micrófono (\"Escucharse\")".into(),
            i18n::Language::German => "Mikrofon testen (\"Sich hören\")".into(),
            i18n::Language::French => "Tester le Micro (\"S'entendre\")".into(),
            i18n::Language::Russian => "Проверить микрофон (\"Слышать себя\")".into(),
            i18n::Language::Japanese => "マイクをテスト (自分の声を聞く)".into(),
            _ => "Test Microphone (\"Hear Yourself\")".into(),
        });
        ui.set_tr_settings_btn_test_stop(match resolved {
            i18n::Language::Portuguese => "Parar Teste (Ouvindo...)".into(),
            i18n::Language::Spanish => "Detener Prueba (Escuchando...)".into(),
            i18n::Language::German => "Test beenden (Hören...)".into(),
            i18n::Language::French => "Arrêter le Test (Écoute...)".into(),
            i18n::Language::Russian => "Остановить тест (Слушаем...)".into(),
            i18n::Language::Japanese => "テスト停止 (聴取中...)".into(),
            _ => "Stop Testing (Listening...)".into(),
        });
    }
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
    ui.set_tr_settings_tab_voice(tr.settings_tab_voice.into());
    ui.set_tr_settings_tab_language(tr.settings_tab_language.into());
    ui.set_tr_settings_tab_keybinds(tr.settings_tab_keybinds.into());
    ui.set_tr_settings_tab_security(tr.settings_tab_security.into());
    ui.set_tr_settings_tab_updates(tr.settings_tab_updates.into());
    ui.set_tr_keybind_mute_title(tr.keybind_mute_title.into());
    ui.set_tr_keybind_mute_desc(tr.keybind_mute_desc.into());
    ui.set_tr_keybind_deafen_title(tr.keybind_deafen_title.into());
    ui.set_tr_keybind_deafen_desc(tr.keybind_deafen_desc.into());
    ui.set_tr_keybind_recording_prompt(tr.keybind_recording_prompt.into());
    ui.set_tr_keybind_btn_record(tr.keybind_btn_record.into());
    ui.set_tr_keybind_btn_clear(tr.keybind_btn_clear.into());
    ui.set_tr_keybind_guide_title(tr.keybind_guide_title.into());
    ui.set_tr_keybind_guide_desc(tr.keybind_guide_desc.into());

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

pub fn populate_video_interface_settings(ui: &AppWindow) {
    let enc = video_settings::get_video_encoder();
    let vcap = video_settings::get_video_capture_backend();
    let aloop = video_settings::get_audio_loopback_backend();
    let notice = video_settings::get_enable_self_preview_notice();

    ui.set_selected_video_encoder(enc.clone().into());
    ui.set_selected_video_capture_backend(vcap.clone().into());
    ui.set_selected_audio_loopback_backend(aloop.clone().into());
    ui.set_enable_self_preview_notice(notice);

    #[cfg(target_os = "windows")]
    let enc_items = vec![
        SettingOptionItem {
            id: "auto".into(),
            name: "Automático (Recomendado)".into(),
            desc: "Prioriza aceleração por hardware GPU disponível com fallback para CPU.".into(),
            is_selected: enc == "auto",
        },
        SettingOptionItem {
            id: "nvenc".into(),
            name: "NVIDIA NVENC".into(),
            desc: "Codificação dedicada ultrarrápida para GPUs NVIDIA GeForce / RTX.".into(),
            is_selected: enc == "nvenc",
        },
        SettingOptionItem {
            id: "amf".into(),
            name: "AMD AMF (Zero-Copy)".into(),
            desc: "Aceleração nativa Direct3D 11 / VCE para placas AMD Radeon.".into(),
            is_selected: enc == "amf",
        },
        SettingOptionItem {
            id: "ffmpeg".into(),
            name: "FFmpeg Hardware".into(),
            desc: "Pipeline GPU unificado FFmpeg (NVENC, AMF ou Intel QuickSync).".into(),
            is_selected: enc == "ffmpeg",
        },
        SettingOptionItem {
            id: "wmf".into(),
            name: "Windows Media Foundation".into(),
            desc: "Codificador Direct3D 11 nativo do Windows (H.264 MFT).".into(),
            is_selected: enc == "wmf",
        },
        SettingOptionItem {
            id: "openh264".into(),
            name: "Cisco OpenH264 (CPU)".into(),
            desc: "Codificação universal por software multithread com otimizações SIMD.".into(),
            is_selected: enc == "openh264",
        },
    ];

    #[cfg(not(target_os = "windows"))]
    let enc_items = vec![
        SettingOptionItem {
            id: "auto".into(),
            name: "Automático (Recomendado)".into(),
            desc: "Prioriza aceleração por hardware GPU disponível com fallback para CPU.".into(),
            is_selected: enc == "auto",
        },
        SettingOptionItem {
            id: "nvenc".into(),
            name: "NVIDIA NVENC".into(),
            desc: "Codificação acelerada por hardware via GPU NVIDIA GeForce / RTX.".into(),
            is_selected: enc == "nvenc",
        },
        SettingOptionItem {
            id: "ffmpeg".into(),
            name: "FFmpeg Hardware (VA-API / QSV)".into(),
            desc: "Pipeline acelerado por GPU via FFmpeg (VA-API, QSV, NVENC).".into(),
            is_selected: enc == "ffmpeg",
        },
        SettingOptionItem {
            id: "openh264".into(),
            name: "Cisco OpenH264 (CPU)".into(),
            desc: "Codificação universal por software multithread com otimizações SIMD.".into(),
            is_selected: enc == "openh264",
        },
    ];

    #[cfg(target_os = "windows")]
    let vcap_items = vec![
        SettingOptionItem {
            id: "auto".into(),
            name: "Automático (Recomendado)".into(),
            desc: "Tenta PrintWindow para janelas DirectX e usa BitBlt GDI para desktop e fallback.".into(),
            is_selected: vcap == "auto",
        },
        SettingOptionItem {
            id: "printwindow".into(),
            name: "PrintWindow (DirectX / DWM)".into(),
            desc: "Captura direta de janelas em primeiro ou segundo plano via DWM.".into(),
            is_selected: vcap == "printwindow",
        },
        SettingOptionItem {
            id: "bitblt".into(),
            name: "BitBlt GDI (Desktop Crop)".into(),
            desc: "Captura clássica de alto desempenho diretamente do framebuffer do desktop.".into(),
            is_selected: vcap == "bitblt",
        },
    ];

    #[cfg(not(target_os = "windows"))]
    let vcap_items = vec![
        SettingOptionItem {
            id: "auto".into(),
            name: "Automático (Recomendado)".into(),
            desc: "Utiliza o seletor nativo do sistema via XDG Desktop Portal e PipeWire.".into(),
            is_selected: vcap == "auto",
        },
        SettingOptionItem {
            id: "portal".into(),
            name: "XDG Desktop Portal + PipeWire".into(),
            desc: "Captura de tela moderna de baixa latência compatível com Wayland e Flatpak.".into(),
            is_selected: vcap == "portal",
        },
        SettingOptionItem {
            id: "x11".into(),
            name: "X11 Fallback".into(),
            desc: "Captura legada direta de tela em sessões gráficas X11.".into(),
            is_selected: vcap == "x11",
        },
    ];

    #[cfg(target_os = "windows")]
    let aloop_items = vec![
        SettingOptionItem {
            id: "auto".into(),
            name: "Automático (Recomendado)".into(),
            desc: "Captura com isolamento de processo WASAPI (exclui o Litecord do áudio da stream).".into(),
            is_selected: aloop == "auto",
        },
        SettingOptionItem {
            id: "wasapi_isolated".into(),
            name: "WASAPI Process-Loopback Isolado".into(),
            desc: "Captura nativa que remove vozes dos amigos e sons do Litecord da sua transmissão.".into(),
            is_selected: aloop == "wasapi_isolated",
        },
        SettingOptionItem {
            id: "cpal".into(),
            name: "CPAL WASAPI Loopback".into(),
            desc: "Captura direta do endpoint de saída padrão do sistema operacional.".into(),
            is_selected: aloop == "cpal",
        },
    ];

    #[cfg(not(target_os = "windows"))]
    let aloop_items = vec![
        SettingOptionItem {
            id: "auto".into(),
            name: "Automático (Recomendado)".into(),
            desc: "Captura monitor do sink padrão PulseAudio/PipeWire via GStreamer pulsesrc.".into(),
            is_selected: aloop == "auto",
        },
        SettingOptionItem {
            id: "pulsesrc".into(),
            name: "GStreamer pulsesrc (Monitor)".into(),
            desc: "Pipeline GStreamer dedicado conectado ao sink monitor do PipeWire.".into(),
            is_selected: aloop == "pulsesrc",
        },
        SettingOptionItem {
            id: "cpal".into(),
            name: "CPAL Audio Loopback".into(),
            desc: "Captura padrão via interface de áudio CPAL.".into(),
            is_selected: aloop == "cpal",
        },
    ];

    ui.set_video_encoder_options(std::rc::Rc::new(slint::VecModel::from(enc_items)).into());
    ui.set_video_capture_options(std::rc::Rc::new(slint::VecModel::from(vcap_items)).into());
    ui.set_audio_loopback_options(std::rc::Rc::new(slint::VecModel::from(aloop_items)).into());
}


pub fn get_guild_initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() >= 2 {
        words.iter().filter_map(|w| w.chars().next()).take(3).collect::<String>().to_uppercase()
    } else {
        name.chars().take(2).collect::<String>().to_uppercase()
    }
}


#[derive(Clone, Debug, Default)]
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

pub async fn load_guild_command_index(
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

pub fn map_message_lines(lines: &[gateway::MessageLineData], channel_id: &str, app_weak: &slint::Weak<AppWindow>) -> slint::ModelRc<MessageLine> {
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

pub fn map_message_attachments(
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

pub fn map_message_buttons(
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


pub fn trim_process_memory() {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        K32EmptyWorkingSet(GetCurrentProcess());
    }
    #[cfg(target_os = "linux")]
    {
        trim_memory_if_linux();
    }
}


#[cfg(target_os = "linux")]
pub fn trim_memory_if_linux() {
    unsafe {
        extern "C" {
            pub fn malloc_trim(pad: usize) -> i32;
            pub fn mi_collect(force: bool);
        }
        malloc_trim(0);
        mi_collect(true);
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn trim_memory_if_linux() {}


pub fn build_ui_channels(
    channels_data: &[ChannelData],
    collapsed_cats: &std::collections::HashSet<String>,
) -> Vec<ChannelItem> {
    let mut ui_channels: Vec<ChannelItem> = Vec::new();
    let mut category_count = 0;
    for ch in channels_data {
        if ch.is_category {
            category_count += 1;
            let is_collapsed = collapsed_cats.contains(&ch.id);
            let has_separator = category_count > 1;
            ui_channels.push(ChannelItem {
                id: ch.id.clone().into(),
                name: ch.name.clone().into(),
                is_voice: false,
                voice_count: 0,
                is_category: true,
                has_parent: false,
                is_collapsed,
                has_separator,
            });
        } else {
            if let Some(ref pid) = ch.parent_id {
                if collapsed_cats.contains(pid) {
                    continue;
                }
            }
            let vcount = if ch.is_voice { gateway::get_voice_channel_participant_count(&ch.id) } else { 0 };
            ui_channels.push(ChannelItem {
                id: ch.id.clone().into(),
                name: ch.name.clone().into(),
                is_voice: ch.is_voice,
                voice_count: vcount,
                is_category: false,
                has_parent: ch.parent_id.is_some(),
                is_collapsed: false,
                has_separator: false,
            });
        }
    }
    ui_channels
}


#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub fn set_dark_titlebar_color(_hwnd: isize) {}

#[cfg(target_os = "windows")]
pub fn set_dark_titlebar_color(hwnd: isize) {
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


#[cfg(target_os = "linux")]
pub fn set_linux_window_keep_above(is_pinned: bool) {
    let pin_val = if is_pinned { "true" } else { "false" };
    let script_code = format!(
        "var list = workspace.windowList ? workspace.windowList() : (workspace.clientList ? workspace.clientList() : []);\nfor (var i = 0; i < list.length; ++i) {{\n    var w = list[i];\n    if (w.caption.indexOf(\"Transmiss\\u00e3o Desanexada\") !== -1 || (w.caption.indexOf(\"Litecord\") !== -1 && w.caption.indexOf(\"Ultra-Lightweight\") === -1)) {{\n        w.keepAbove = {};\n    }}\n}}\n",
        pin_val
    );

    let temp_base = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let script_path_buf = temp_base.join(format!("litecord_pin_{}.js", std::process::id()));
    let script_path = script_path_buf.to_string_lossy().to_string();
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
