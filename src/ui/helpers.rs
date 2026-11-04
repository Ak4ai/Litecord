#![allow(dead_code)]


use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}};
use log::{info, warn};

use crate::{
    AppWindow, ChannelItem, MessageLine, MessageBlock, MessageButton, MessageAttachment,
    CommandSuggestion, SettingOptionItem, LanguageItem,
};
use crate::http::DiscordHttpClient;
use crate::gateway::{self, ChannelData};
use crate::utils::i18n;
use crate::utils::i18n_ui;
use crate::utils::video_settings;
use crate::utils::attachment_cache;
use crate::utils::avatar_cache;
use crate::utils::emoji_cache;
use std::sync::Mutex;
use slint::Model;
use std::collections::{HashMap, HashSet, VecDeque};

static DM_UNREAD_MAP: std::sync::LazyLock<Mutex<HashMap<String, i32>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

static PROCESSED_MSG_IDS: std::sync::LazyLock<Mutex<(HashSet<String>, VecDeque<String>)>> =
    std::sync::LazyLock::new(|| Mutex::new((HashSet::new(), VecDeque::new())));

pub fn get_dm_unread(ch_id: &str) -> i32 {
    DM_UNREAD_MAP.lock().unwrap().get(ch_id).copied().unwrap_or(0)
}

pub fn inc_dm_unread(ch_id: &str) -> i32 {
    let mut map = DM_UNREAD_MAP.lock().unwrap();
    let count = map.entry(ch_id.to_string()).or_insert(0);
    *count += 1;
    map.values().sum()
}

pub fn clear_dm_unread(ch_id: &str) -> i32 {
    let mut map = DM_UNREAD_MAP.lock().unwrap();
    map.remove(ch_id);
    map.values().sum()
}

pub fn get_total_dm_unreads() -> i32 {
    DM_UNREAD_MAP.lock().unwrap().values().sum()
}

pub fn mark_msg_seen(id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let mut guard = PROCESSED_MSG_IDS.lock().unwrap();
    if guard.0.contains(id) {
        return true;
    }
    guard.0.insert(id.to_string());
    guard.1.push_back(id.to_string());
    if guard.1.len() > 1000 {
        if let Some(old) = guard.1.pop_front() {
            guard.0.remove(&old);
        }
    }
    false
}

pub fn reset_unread_and_dedup_state() {
    DM_UNREAD_MAP.lock().unwrap().clear();
    let mut guard = PROCESSED_MSG_IDS.lock().unwrap();
    guard.0.clear();
    guard.1.clear();
}

/// Drops per-account memory: decoded images, message caches and user maps.
///
/// Called on logout and account switch. Preferences, window geometry and the
/// on-disk image caches are kept; a new account re-downloads what it needs.
pub fn clear_session_caches() {
    avatar_cache::get_avatar_cache().clear_session();
    emoji_cache::get_emoji_cache().clear_memory();
    emoji_cache::clear_registered_custom_emojis();
    attachment_cache::get_attachment_cache().clear_memory();
    crate::ui::data_loader::clear_session_caches();
    gateway::clear_user_directory();
    gateway::clear_guild_voice_states();
    gateway::channel_names::clear();
    crate::ui::invite_cards::clear_session();
    crate::ui::onboarding::clear_session();
    if let Ok(mut pending) = HIDDEN_CHANNEL_ACTIVITY.lock() {
        pending.clear();
    }
}

pub fn is_dm_channel(ch_id: &str, guilds_map: &Arc<Mutex<HashMap<String, crate::gateway::GuildData>>>) -> bool {
    if let Ok(map) = guilds_map.lock() {
        if let Some(dm_guild) = map.get("@me") {
            return dm_guild.channels.iter().any(|c| c.id == ch_id);
        }
    }
    false
}

struct ChatPinTimers {
    first: slint::Timer,
    follow_up: slint::Timer,
}

thread_local! {
    static CHAT_PIN_TIMERS: std::cell::RefCell<Option<std::rc::Rc<ChatPinTimers>>> =
        const { std::cell::RefCell::new(None) };
}

pub fn request_chat_scroll_to_bottom(app_weak: slint::Weak<AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || {
        // Images and embeds change the height after the first layout, so one follow-up
        // pin still lets a channel with pictures settle. Earlier requests are cancelled,
        // so a burst of messages coalesces into at most two scrolls instead of seven
        // timers per message.
        CHAT_PIN_TIMERS.with(|slot| {
            let existing = slot.borrow().clone();
            let timers = existing.unwrap_or_else(|| {
                let timers = std::rc::Rc::new(ChatPinTimers {
                    first: slint::Timer::default(),
                    follow_up: slint::Timer::default(),
                });
                *slot.borrow_mut() = Some(timers.clone());
                timers
            });
            timers.first.stop();
            timers.follow_up.stop();

            let weak_timers = std::rc::Rc::downgrade(&timers);
            let app_first = app_weak.clone();
            timers.first.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(80),
                move || {
                    if let Some(ui) = app_first.upgrade() {
                        ui.invoke_scroll_chat_to_bottom();
                    }
                    let Some(timers) = weak_timers.upgrade() else { return };
                    let app_follow = app_first.clone();
                    timers.follow_up.start(
                        slint::TimerMode::SingleShot,
                        std::time::Duration::from_millis(340),
                        move || {
                            if let Some(ui) = app_follow.upgrade() {
                                ui.invoke_scroll_chat_to_bottom();
                            }
                        },
                    );
                },
            );
        });
    });
}

static LAST_CHAT_PAINT_MS: AtomicU64 = AtomicU64::new(0);

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Marks that a channel batch was just painted in the chat.
pub fn mark_chat_painted() {
    LAST_CHAT_PAINT_MS.store(unix_millis(), Ordering::Relaxed);
}

/// True while a channel paint is recent: late model refreshes (images, emojis) may
/// re-pin the chat even if a rebuild already reset the scroll position.
pub fn chat_painted_within(max_age: std::time::Duration) -> bool {
    let last = LAST_CHAT_PAINT_MS.load(Ordering::Relaxed);
    last > 0 && unix_millis().saturating_sub(last) <= max_age.as_millis() as u64
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

/// Woken when the window comes back from the tray or the voice connection
/// changes, so UI loops can sleep instead of ticking while they have nothing to show.
pub static UI_LOOPS_WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

pub fn wake_ui_loops() {
    UI_LOOPS_WAKE.notify_waiters();
}

/// Set to true by the tray restore paths to signal a REST refresh is needed.
pub static NEED_UI_REFRESH: AtomicBool = AtomicBool::new(false);

/// Number of messages received while the window was hidden.
/// Cleared when the window is restored and a REST refresh is triggered.
pub static PENDING_MESSAGES: AtomicUsize = AtomicUsize::new(0);

/// Sidebar activity that arrived while the window was hidden: (channel id, new unread
/// messages), oldest first, one entry per channel. Applied when the window comes back.
static HIDDEN_CHANNEL_ACTIVITY: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());

/// True while the settings modal (with the microphone level meter) is open.
pub static MIC_METER_VISIBLE: AtomicBool = AtomicBool::new(false);

/// Set while hidden when voice states changed, so the channel list is refreshed on restore.
pub static VOICE_STATES_DIRTY: AtomicBool = AtomicBool::new(false);

pub fn record_hidden_channel_activity(channel_id: &str, new_unread: i32) {
    if let Ok(mut pending) = HIDDEN_CHANNEL_ACTIVITY.lock() {
        let earlier = pending
            .iter()
            .position(|(id, _)| id == channel_id)
            .map(|i| pending.remove(i).1)
            .unwrap_or(0);
        pending.push((channel_id.to_string(), earlier + new_unread));
    }
}

pub fn take_hidden_channel_activity() -> Vec<(String, i32)> {
    HIDDEN_CHANNEL_ACTIVITY.lock().map(|mut pending| std::mem::take(&mut *pending)).unwrap_or_default()
}

/// Applies sidebar activity in order: each channel's unread count grows by its new
/// messages (the open channel goes back to 0) and, in the DM list, it moves to the top.
pub fn apply_channel_activity(ui: &AppWindow, activity: &[(String, i32)], active_channel: &str, showing_dms: bool) {
    let current = ui.get_channels();
    let Some(model) = current.as_any().downcast_ref::<slint::VecModel<ChannelItem>>() else {
        // Unexpected model kind: keep the old rebuild path so the badge still updates.
        let mut channels: Vec<ChannelItem> = current.iter().collect();
        let mut changed = false;
        for (id, new_unread) in activity {
            let Some(pos) = channels.iter().position(|c| c.id == id.as_str()) else { continue };
            let unread = if id == active_channel { 0 } else { channels[pos].unread_count + new_unread };
            if channels[pos].unread_count != unread {
                channels[pos].unread_count = unread;
                changed = true;
            }
            if showing_dms && pos != 0 {
                let ch = channels.remove(pos);
                channels.insert(0, ch);
                changed = true;
            }
        }
        if changed {
            ui.set_channels(std::rc::Rc::new(slint::VecModel::from(channels)).into());
        }
        return;
    };

    // Touch only the rows that changed: replacing the whole model made the sidebar
    // rebuild every visible row (and restart hover/selection) for each message.
    for (id, new_unread) in activity {
        let Some(pos) = (0..model.row_count())
            .find(|&index| model.row_data(index).is_some_and(|c| c.id == id.as_str()))
        else {
            continue;
        };
        let Some(mut row) = model.row_data(pos) else { continue };
        let unread = if id == active_channel { 0 } else { row.unread_count + new_unread };
        let unread_changed = row.unread_count != unread;
        row.unread_count = unread;
        if showing_dms && pos != 0 {
            let _ = model.remove(pos);
            model.insert(0, row);
        } else if unread_changed {
            model.set_row_data(pos, row);
        }
    }
}

/// Clears the unread badge of one channel without rebuilding the sidebar model.
pub fn clear_channel_unread(ui: &AppWindow, channel_id: &str) {
    let current = ui.get_channels();
    if let Some(model) = current.as_any().downcast_ref::<slint::VecModel<ChannelItem>>() {
        for index in 0..model.row_count() {
            let Some(mut row) = model.row_data(index) else { continue };
            if row.id == channel_id {
                if row.unread_count != 0 {
                    row.unread_count = 0;
                    model.set_row_data(index, row);
                }
                return;
            }
        }
        return;
    }

    let mut channels: Vec<ChannelItem> = current.iter().collect();
    if let Some(channel) = channels.iter_mut().find(|c| c.id == channel_id) {
        channel.unread_count = 0;
    }
    ui.set_channels(std::rc::Rc::new(slint::VecModel::from(channels)).into());
}

/// Refreshes voice counts and participants from `fresh` (a `build_ui_channels` result).
/// Rows are matched by id, so the current order (recent DMs first) and unread counts stay.
pub fn update_channel_voice_rows(ui: &AppWindow, fresh: Vec<ChannelItem>) {
    let current = ui.get_channels();
    let fresh_by_id: HashMap<String, &ChannelItem> = fresh.iter().map(|c| (c.id.to_string(), c)).collect();
    let same_rows = current.row_count() == fresh.len()
        && (0..current.row_count()).all(|i| current.row_data(i).is_some_and(|c| fresh_by_id.contains_key(c.id.as_str())));
    if same_rows {
        for i in 0..current.row_count() {
            let Some(old) = current.row_data(i) else { continue };
            let new = fresh_by_id[old.id.as_str()];
            if old.voice_count != new.voice_count
                || old.name != new.name
                || old.is_speaking != new.is_speaking
                || old.is_streaming != new.is_streaming
                || old.is_muted != new.is_muted
                || old.is_deafened != new.is_deafened
            {
                let mut row = new.clone();
                row.unread_count = old.unread_count;
                current.set_row_data(i, row);
            }
        }
        return;
    }
    let unread: HashMap<String, i32> = current
        .iter()
        .filter(|c| c.unread_count > 0)
        .map(|c| (c.id.to_string(), c.unread_count))
        .collect();
    let rows: Vec<ChannelItem> = fresh
        .into_iter()
        .map(|mut c| {
            if let Some(&n) = unread.get(c.id.as_str()) {
                c.unread_count = n;
            }
            c
        })
        .collect();
    ui.set_channels(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

/// Updates the talking and streaming state of the voice member rows in place.
///
/// The voice member rows have a stable id (`voice-member:<channel>:<user>`), so
/// a change of speaker only touches that row: rebuilding the whole channel list
/// (as the old fingerprint did) re-created every row of the sidebar.
pub fn update_voice_member_rows(ui: &AppWindow, changed: &[(String, u64, bool, bool)]) {
    if changed.is_empty() {
        return;
    }
    let channels = ui.get_channels();
    let row_count = channels.row_count();

    // Index the rows once instead of scanning the list for every member.
    let mut row_of: HashMap<String, usize> = HashMap::with_capacity(changed.len());
    for index in 0..row_count {
        if let Some(row) = channels.row_data(index) {
            if row.is_voice_member {
                row_of.entry(row.id.to_string()).or_insert(index);
            }
        }
    }

    for (channel_id, user_id, speaking, streaming) in changed {
        let row_id = format!("voice-member:{channel_id}:{user_id}");
        let Some(index) = row_of.get(&row_id).copied() else { continue };
        let Some(mut row) = channels.row_data(index) else { continue };
        if row.is_speaking == *speaking && row.is_streaming == *streaming {
            continue;
        }
        row.is_speaking = *speaking;
        row.is_streaming = *streaming;
        channels.set_row_data(index, row);
    }
}

/// Repaints a member's role badge/color after a role was assigned or removed.
/// The role store is already updated; this only refreshes existing voice rows,
/// so the sidebar does not rebuild on every role toggle.
pub fn refresh_member_role_rows(ui: &AppWindow, guild_id: &str, user_id: u64) {
    let (role_name, role_color) = gateway::member_display_role(guild_id, user_id)
        .map(|(name, color)| (name, color))
        .unwrap_or_default();
    let marker = format!(":{user_id}");
    let channels = ui.get_channels();
    for index in 0..channels.row_count() {
        let Some(mut row) = channels.row_data(index) else { continue };
        if !row.is_voice_member || !row.id.ends_with(&marker) {
            continue;
        }
        row.role_name = role_name.as_str().into();
        row.role_color = slint::Color::from_rgb_u8(
            ((role_color >> 16) & 0xff) as u8,
            ((role_color >> 8) & 0xff) as u8,
            (role_color & 0xff) as u8,
        );
        channels.set_row_data(index, row);
    }
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
    i18n_ui::set_current(resolved);
    i18n_ui::apply_to_window(ui, i18n_ui::table_for(resolved));
    crate::ui::message_grouping::relabel_day_separators(&ui.get_messages());
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
    ui.set_tr_settings_tab_network_proxy(tr.settings_tab_network_proxy.into());
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
    #[cfg(windows)]
    populate_video_interface_settings(ui);
}

pub fn populate_video_interface_settings(ui: &AppWindow) {
    let enc = video_settings::get_video_encoder();
    let vcap = video_settings::get_video_capture_backend();
    let aloop = video_settings::get_audio_loopback_backend();
    let notice = video_settings::get_enable_self_preview_notice();
    let show_hw = video_settings::get_show_hardware_monitor();
    ui.set_use_gpu_renderer(video_settings::get_use_gpu_renderer());
    ui.set_enable_tray_icon(video_settings::get_enable_tray_icon());
    let theme_mode = video_settings::get_theme_mode();
    let use_light_theme = theme_mode == "light";

    ui.set_selected_video_encoder(enc.clone().into());
    ui.set_selected_video_capture_backend(vcap.clone().into());
    ui.set_selected_audio_loopback_backend(aloop.clone().into());
    ui.set_enable_self_preview_notice(notice);
    ui.set_show_hardware_monitor(show_hw);
    ui.set_theme_mode_name(theme_mode.as_str().into());
    ui.set_use_light_theme(use_light_theme);

    #[cfg(target_os = "windows")]
    let enc_items = {
        use crate::screen_capture::gpu_capture::{self, Choice};
        ui.set_gpu_selection_available(true);
        let text = i18n_ui::ui_text();
        let choice = gpu_capture::parse_choice(&enc);
        let gpus = gpu_capture::gpus();
        let selected = match choice {
            Choice::Gpu { vendor, device } => gpus.iter()
                .find(|g| g.vendor == vendor && (device == 0 || device == g.device)).map(|g| g.id()),
            _ => None,
        };
        let mut items = vec![SettingOptionItem {
            id: "auto".into(), name: text.gpu_auto.into(), desc: text.gpu_auto_desc.into(),
            is_selected: choice == Choice::Auto,
        }];
        for gpu in gpus {
            let codec = match gpu.vendor { 0x10de => "NVENC", 0x8086 => "Quick Sync", 0x1002 => "AMF", _ => "GPU" };
            items.push(SettingOptionItem {
                id: gpu.id().into(), name: gpu.name.clone().into(),
                desc: format!("{} · {}", codec, if gpu.drives_display { text.gpu_display_desc } else { text.gpu_other_desc }).into(),
                is_selected: selected.as_deref() == Some(gpu.id().as_str()),
            });
        }
        if matches!(choice, Choice::Gpu { .. }) && selected.is_none() {
            items.push(SettingOptionItem {
                id: enc.clone().into(), name: text.gpu_missing.into(), desc: text.gpu_next_stream.into(), is_selected: true,
            });
        }
        items.push(SettingOptionItem {
            id: "openh264".into(), name: text.gpu_cpu.into(), desc: text.gpu_cpu_desc.into(),
            is_selected: choice == Choice::Cpu,
        });
        refresh_stream_capture_status(ui);
        items
    };

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

#[cfg(target_os = "windows")]
pub fn refresh_stream_capture_status(ui: &AppWindow) {
    let text = i18n_ui::ui_text();
    let value = crate::screen_capture::gpu_capture::status().map(|(gpu, backend)| {
        let path = if crate::screen_capture::gpu_tx::active() { text.gpu_path } else { text.gpu_compat_path };
        format!("{}: {} · {} · {}", text.gpu_capture_now, gpu, backend, path)
    }).unwrap_or_default();
    ui.set_stream_capture_status(value.into());
}

pub fn populate_voice_processing_settings(ui: &AppWindow) {
    let ns = video_settings::get_noise_suppression();
    let aec = video_settings::get_echo_cancellation();
    let agc = video_settings::get_automatic_gain_control();

    ui.set_echo_cancellation(aec);
    ui.set_automatic_gain_control(agc);

    let lang = i18n::load_persisted_language_config();
    let (adv_name, adv_desc, std_name, std_desc, off_name, off_desc) = match lang {
        i18n::Language::English => (
            "Advanced Suppression",
            "Ideal for gaming & typing: blocks mechanical keyboards, pets, house noise & fans. (Avoid for musical instruments).",
            "Standard (RNNoise)",
            "For quiet rooms with steady hum (PC fans, AC). Preserves voice warmth, but does not block keyboard clicks.",
            "Disabled (Raw Audio)",
            "Pure unprocessed audio. Recommended for singing, playing guitar, and musical instruments."
        ),
        i18n::Language::Spanish => (
            "Supresión avanzada",
            "Ideal para gaming y llamadas: bloquea teclados mecánicos, mascotas, ruidos de casa y ventiladores. (Evitar para música).",
            "Estándar (RNNoise)",
            "Para ambientes tranquilos con zumbido constante (ventilador, AC). Mantiene voz natural, no quita teclas.",
            "Desactivado (Audio puro)",
            "Audio puro sin filtros ni cortes. Recomendado para cantar, tocar guitarra y transmitir instrumentos musicales."
        ),
        _ => (
            "Supressão Avançada",
            "Ideal para jogos e digitação: bloqueia teclado mecânico, pets, ruídos domésticos e ventoinhas. (Evitar para música).",
            "Padrão (RNNoise)",
            "Para ambientes calmos com ruído contínuo (ventoinha, AC). Mantém voz natural, mas não remove teclas.",
            "Desativado (Áudio puro)",
            "Áudio puro sem filtros ou cortes. Recomendado para cantar, tocar violão/guitarra e transmitir instrumentos."
        ),
    };

    let ns_items = vec![
        SettingOptionItem {
            id: "advanced".into(),
            name: adv_name.into(),
            desc: adv_desc.into(),
            is_selected: ns == "advanced" || ns == "krisp",
        },
        SettingOptionItem {
            id: "standard".into(),
            name: std_name.into(),
            desc: std_desc.into(),
            is_selected: ns == "standard",
        },
        SettingOptionItem {
            id: "none".into(),
            name: off_name.into(),
            desc: off_desc.into(),
            is_selected: ns == "none" || ns.is_empty(),
        },
    ];

    ui.set_noise_suppression_options(std::rc::Rc::new(slint::VecModel::from(ns_items)).into());
}

pub fn populate_network_proxy_settings(ui: &AppWindow) {
    let settings = crate::utils::get_network_settings();
    ui.set_selected_proxy_mode(settings.proxy_mode.clone().into());
    let cur_host = ui.get_proxy_host_input();
    if cur_host.trim().is_empty() {
        ui.set_proxy_host_input(settings.proxy_host.clone().into());
    }
    let cur_port = ui.get_proxy_port_input();
    if cur_port.trim().is_empty() {
        ui.set_proxy_port_input(settings.proxy_port.to_string().into());
    }
    ui.set_proxy_route_media_enabled(settings.route_media);

    let mode_items = vec![
        SettingOptionItem {
            id: "off".into(),
            name: "Desativado (Conexão Direta)".into(),
            desc: "Conexão direta aos servidores do Discord (Padrão).".into(),
            is_selected: settings.proxy_mode == "off",
        },
        SettingOptionItem {
            id: "http".into(),
            name: "Proxy HTTP / HTTPS".into(),
            desc: "Túnel HTTP CONNECT para redes restritas e proxies.".into(),
            is_selected: settings.proxy_mode == "http" || settings.proxy_mode == "https",
        },
        SettingOptionItem {
            id: "socks5".into(),
            name: "Proxy SOCKS5".into(),
            desc: "Túnel SOCKS5 compatível com Tor, Shadowsocks e SSH.".into(),
            is_selected: settings.proxy_mode == "socks5",
        },
        SettingOptionItem {
            id: "system".into(),
            name: "Proxy do Sistema".into(),
            desc: "Detecta automaticamente o proxy global do sistema.".into(),
            is_selected: settings.proxy_mode == "system",
        },
    ];

    ui.set_proxy_mode_options(std::rc::Rc::new(slint::VecModel::from(mode_items)).into());
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
    /// Discord application command option type: 3 string, 4 integer,
    /// 5 boolean, 6 user, 7 channel, 8 role, 9 mentionable, 10 number.
    pub option_type: i32,
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
                option_type: 3,
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
                option_type: 3,
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
                option_type: 3,
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
                option_type: 3,
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
                option_type: 3,
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
                option_type: 3,
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
                option_type: 3,
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
                option_type: 3,
            },
        ]))
    }).clone()
}

static COMMAND_INDEX_GENERATION: AtomicU64 = AtomicU64::new(0);

fn command_items_from_index(data: &serde_json::Value) -> Vec<CommandSuggestionItem> {
    use std::collections::HashSet;

    let mut app_names: HashMap<String, String> = HashMap::new();
    if let Some(apps) = data["applications"].as_array() {
        for app in apps {
            let app_id = app["id"].as_str().unwrap_or("").to_string();
            let app_name = app["name"]
                .as_str()
                .or_else(|| app["bot"]["global_name"].as_str())
                .or_else(|| app["bot"]["username"].as_str())
                .unwrap_or("")
                .to_string();
            if !app_id.is_empty() && !app_name.is_empty() {
                app_names.insert(app_id, app_name);
            }
        }
    }

    let mut items: Vec<CommandSuggestionItem> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    if let Some(commands) = data["application_commands"].as_array() {
        for command in commands {
            // 1 = CHAT_INPUT. USER/MESSAGE/ENTRY_POINT commands are invoked from
            // their own context menus, not from the composer.
            if command["type"].as_i64().unwrap_or(1) != 1 {
                continue;
            }
            let name = command["name"].as_str().unwrap_or("");
            let app_id = command["application_id"].as_str().unwrap_or("");
            let cmd_id = command["id"].as_str().unwrap_or("");
            if name.is_empty() || app_id.is_empty() || cmd_id.is_empty() {
                continue;
            }
            if !seen.insert((app_id.to_string(), cmd_id.to_string())) {
                continue;
            }

            let description = command["description"]
                .as_str()
                .or_else(|| command["description_default"].as_str())
                .unwrap_or("");
            let version = command["version"].as_str().unwrap_or("");
            let app_name = app_names
                .get(app_id)
                .cloned()
                .unwrap_or_else(|| "Bot".to_string());

            let mut param_name = String::new();
            let mut param_desc = String::new();
            let mut is_required = false;
            let mut option_type = 3;
            if let Some(options) = command["options"].as_array() {
                // The composer currently exposes one argument. Prefer the first
                // required scalar option; subcommands (types 1/2) need a richer
                // wizard and are intentionally not flattened into a wrong payload.
                let option = options
                    .iter()
                    .filter(|option| {
                        !matches!(option["type"].as_i64().unwrap_or(0), 1 | 2)
                    })
                    .find(|option| option["required"].as_bool().unwrap_or(false))
                    .or_else(|| {
                        options
                            .iter()
                            .find(|option| !matches!(option["type"].as_i64().unwrap_or(0), 1 | 2))
                    });
                if let Some(option) = option {
                    param_name = option["name"].as_str().unwrap_or("").to_string();
                    param_desc = option["description"].as_str().unwrap_or("").to_string();
                    is_required = option["required"].as_bool().unwrap_or(false);
                    option_type = option["type"].as_i64().unwrap_or(3) as i32;
                }
            }

            let usage = if param_name.is_empty() {
                format!("/{name}")
            } else if is_required {
                format!("/{name} <{param_name}>")
            } else {
                format!("/{name} [{param_name}]")
            };
            items.push(CommandSuggestionItem {
                name: format!("/{name}"),
                desc: description.to_string(),
                usage,
                app_id: app_id.to_string(),
                app_name,
                cmd_id: cmd_id.to_string(),
                version: version.to_string(),
                param_name,
                param_desc,
                is_required,
                option_type,
            });
        }
    }
    items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    items
}

fn publish_command_items(app_weak: &slint::Weak<AppWindow>, items: Vec<CommandSuggestionItem>) {
    let count = items.len();
    if let Ok(mut master) = get_master_command_suggestions().lock() {
        master.clear();
        master.extend(items.clone());
    }
    let slint_suggestions: Vec<CommandSuggestion> = items
        .into_iter()
        .map(|item| CommandSuggestion {
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
        })
        .collect();
    let app_weak = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_weak.upgrade() {
            let model = std::rc::Rc::new(slint::VecModel::from(slint_suggestions));
            ui.set_command_suggestions(model.into());
            if count == 0 {
                info!("No hay comandos de aplicación disponibles en este ámbito");
            } else {
                info!("✅ Carregados {} comandos de aplicación", count);
            }
        }
    });
}

/// Loads guild and user-installed application commands together. The guild
/// index covers commands installed in the server; the user index covers global
/// commands and apps installed only to the account (including DMs).
pub async fn load_guild_command_index(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guild_id: &str,
) {
    if guild_id.is_empty() {
        return;
    }
    let generation = COMMAND_INDEX_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let mut items = Vec::new();
    let mut loaded_any = false;

    if guild_id != "@me" {
        let (guild_index, user_index) = tokio::join!(
            http.get_guild_application_command_index(guild_id),
            http.get_user_application_command_index()
        );
        match guild_index {
            Ok(data) => {
                loaded_any = true;
                items.extend(command_items_from_index(&data));
            }
            Err(error) => {
                warn!("No se pudo cargar el índice del servidor {}: {}", guild_id, error);
            }
        }
        match user_index {
            Ok(data) => {
                loaded_any = true;
                items.extend(command_items_from_index(&data));
            }
            Err(error) => {
                warn!("No se pudo cargar el índice global de comandos: {}", error);
            }
        }
    } else {
        match http.get_user_application_command_index().await {
            Ok(data) => {
                loaded_any = true;
                items.extend(command_items_from_index(&data));
            }
            Err(error) => warn!("No se pudo cargar el índice global de comandos: {}", error),
        }
    }

    if generation != COMMAND_INDEX_GENERATION.load(Ordering::Acquire) {
        return;
    }
    if !loaded_any {
        // Both indexes failed. Clear instead of leaving commands from the
        // previous server active in the composer.
        publish_command_items(&app_weak, Vec::new());
        return;
    }
    // A command can appear in both indexes. Keep one entry per app/command id.
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert((item.app_id.clone(), item.cmd_id.clone())));
    publish_command_items(&app_weak, items);
}

pub fn map_message_lines(lines: &[gateway::MessageLineData], channel_id: &str, app_weak: &slint::Weak<AppWindow>) -> slint::ModelRc<MessageLine> {
    let emoji_mgr = emoji_cache::get_emoji_cache();
    let slint_lines: Vec<MessageLine> = lines.iter().map(|line| {
        let slint_blocks: Vec<MessageBlock> = line.blocks.iter().map(|b| {
            let emoji_img = if b.is_emoji {
                if let Some(img) = emoji_mgr.get_cached(&b.emoji_id) {
                    img
                } else {
                    // Never decode on the paint path: hydrate what is on disk and
                    // download only what is missing.
                    emoji_mgr.hydrate_from_disk_async(&b.emoji_id, app_weak.clone());
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
        let (is_downloaded, full_img) = if let Some(img) = att_cache.get_full_for_ui(&a.id) {
            (true, img)
        } else {
            // Painting never touches the disk: decode what is already downloaded on a
            // worker thread and update this row when it is ready.
            att_cache.hydrate_full_from_disk_async(&a.id, &a.filename, app_weak.clone());
            if a.id.starts_with("embed_img_") {
                att_cache.download_full_async(&a.id, &a.filename, &a.url, app_weak.clone());
            }
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

        let is_loading = a.id.starts_with("embed_img_") && !is_downloaded;

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
            is_loading,
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
            if let Some(img) = emoji_mgr.get_cached(&b.emoji_id) {
                img
            } else {
                emoji_mgr.hydrate_from_disk_async(&b.emoji_id, app_weak.clone());
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


/// The sidebar row a row dragged from `from` by `dy` logical pixels lands on: the one under
/// the dragged row's centre. Heights as drawn in channel_sidebar.slint (4px apart).
pub fn channel_drop_row(rows: &[ChannelItem], from: usize, dy: f32) -> usize {
    const SPACING: f32 = 4.0;
    let height = |r: &ChannelItem| {
        if r.is_category {
            if r.has_separator { 36.0 } else { 26.0 }
        } else if r.is_voice_member {
            26.0
        } else {
            34.0
        }
    };
    if from >= rows.len() {
        return from;
    }
    let mut top = 0.0;
    let mut tops = Vec::with_capacity(rows.len());
    for r in rows {
        tops.push(top);
        top += height(r) + SPACING;
    }
    let centre = tops[from] + height(&rows[from]) / 2.0 + dy;
    tops.iter().rposition(|&t| t <= centre).unwrap_or(0)
}

/// Resolves the row under a dragged voice member to a voice channel row. A
/// member row maps to its parent voice channel, so dropping anywhere over the
/// participant cluster works like Discord.
pub fn voice_member_drop_row(rows: &[ChannelItem], from: usize, dy: f32) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    let target = channel_drop_row(rows, from, dy);
    let row = rows.get(target)?;
    (row.is_voice || row.is_voice_member).then_some(target)
}

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
                name: crate::utils::styled_text::plain_display_name(&ch.name).as_ref().into(),
                is_voice: false,
                voice_count: 0,
                is_voice_member: false,
                parent_channel_id: "".into(),
                avatar_text: "".into(),
                avatar_url: "".into(),
                avatar_slot: 0,
                is_category: true,
                has_parent: false,
                is_collapsed,
                has_separator,
                unread_count: 0,
                ..crate::ui::helpers::channel_emoji_fields(&crate::utils::styled_text::plain_display_name(&ch.name))
            });
        } else {
            if let Some(ref pid) = ch.parent_id {
                if collapsed_cats.contains(pid) {
                    continue;
                }
            }
            let voice_participants = if ch.is_voice {
                gateway::get_voice_channel_participants(&ch.id)
            } else {
                Vec::new()
            };
            let vcount = voice_participants.len() as i32;
            ui_channels.push(ChannelItem {
                id: ch.id.clone().into(),
                name: crate::utils::styled_text::plain_display_name(&ch.name).as_ref().into(),
                is_voice: ch.is_voice,
                voice_count: vcount,
                is_voice_member: false,
                parent_channel_id: "".into(),
                avatar_text: "".into(),
                avatar_url: "".into(),
                avatar_slot: 0,
                is_category: false,
                has_parent: ch.parent_id.is_some(),
                is_collapsed: false,
                has_separator: false,
                unread_count: 0,
                ..crate::ui::helpers::channel_emoji_fields(&crate::utils::styled_text::plain_display_name(&ch.name))
            });

            for (user_id, name) in voice_participants {
                let avatar_url = gateway::get_user_avatar_url(user_id);
                let avatar_text: String = name
                    .split_whitespace()
                    .filter_map(|part| part.chars().next())
                    .take(2)
                    .flat_map(char::to_uppercase)
                    .collect();
                let my_uid = gateway::get_my_user_id();
                let is_self_member = user_id == 999999 || (my_uid != 0 && user_id == my_uid);
                let (is_muted_flag, is_deaf_flag) = gateway::get_user_voice_audio_flags(user_id);
                let (is_muted_saved, _, _) = gateway::get_user_audio_settings(&user_id.to_string());
                let final_muted = is_muted_flag || is_muted_saved;
                let (role_name, role_color) = gateway::get_user_display_role(user_id).unwrap_or_default();
                let (is_screen_streaming, is_camera_streaming) = if is_self_member {
                    let camera = crate::native_video::camera::is_camera_wanted();
                    (crate::screen_capture::is_self_streaming() && !camera, camera)
                } else {
                    crate::screen_capture::get_user_stream_flags(user_id)
                };
                ui_channels.push(ChannelItem {
                    id: format!("voice-member:{}:{}", ch.id, user_id).into(),
                    user_id: user_id.to_string().into(),
                    name: crate::utils::styled_text::plain_display_name(&name).as_ref().into(),
                    is_voice: false,
                    voice_count: 0,
                    is_voice_member: true,
                    is_speaking: gateway::is_voice_user_speaking(user_id),
                    is_streaming: if is_self_member {
                        crate::screen_capture::is_self_streaming()
                    } else {
                        crate::screen_capture::peer_is_streaming(user_id)
                    },
                    is_screen_streaming,
                    is_camera_streaming,
                    is_muted: final_muted,
                    is_deafened: is_deaf_flag,
                    is_self_member,
                    role_name: role_name.into(),
                    role_color: slint::Color::from_rgb_u8(
                        ((role_color >> 16) & 0xFF) as u8,
                        ((role_color >> 8) & 0xFF) as u8,
                        (role_color & 0xFF) as u8,
                    ),
                    parent_channel_id: ch.id.clone().into(),
                    avatar_text: avatar_text.into(),
                    avatar_slot: crate::utils::avatar_cache::get_avatar_cache().slot_for(&avatar_url),
                    avatar_url: avatar_url.into(),
                    is_category: false,
                    has_parent: ch.parent_id.is_some(),
                    is_collapsed: false,
                    has_separator: false,
                    unread_count: 0,
                    ..Default::default()
                });
            }
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

#[cfg(test)]
mod hidden_activity_tests {
    use super::*;

    #[test]
    fn a_dragged_channel_lands_on_the_row_under_its_centre() {
        let row = |cat: bool, member: bool| ChannelItem { is_category: cat, is_voice_member: member, ..Default::default() };
        // category 26 | text 34 | text 34 | voice 34 | member 26 | text 34   (4px apart)
        let rows = [row(true, false), row(false, false), row(false, false), row(false, false), row(false, true), row(false, false)];
        assert_eq!(channel_drop_row(&rows, 1, 0.0), 1);
        assert_eq!(channel_drop_row(&rows, 1, 25.0), 2);
        assert_eq!(channel_drop_row(&rows, 1, 60.0), 3);
        assert_eq!(channel_drop_row(&rows, 5, -70.0), 3);
        assert_eq!(channel_drop_row(&rows, 1, -500.0), 0);
        assert_eq!(channel_drop_row(&rows, 1, 5000.0), 5);
    }

    #[test]
    fn hidden_activity_keeps_one_entry_per_channel_in_last_activity_order() {
        let _ = take_hidden_channel_activity();
        record_hidden_channel_activity("a", 1);
        record_hidden_channel_activity("b", 1);
        record_hidden_channel_activity("a", 1);
        record_hidden_channel_activity("c", 0);
        assert_eq!(
            take_hidden_channel_activity(),
            vec![("b".to_string(), 1), ("a".to_string(), 2), ("c".to_string(), 0)]
        );
        assert!(take_hidden_channel_activity().is_empty());
    }
}

const CHANNEL_EMOJI_SLOTS: usize = 128;

/// Window and image pool of the channel list's emoji. `ChannelItem` rows are built off
/// the UI thread too, and an image cannot cross threads, so a row holds a slot number
/// and the images live in `channel_emoji_pool`, filled on the UI thread.
static CHANNEL_EMOJI_APP: std::sync::Mutex<Option<slint::Weak<AppWindow>>> = std::sync::Mutex::new(None);
static CHANNEL_EMOJI_SLOT_OF: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i32>>> =
    std::sync::OnceLock::new();

/// Must run on the UI thread: installs the image pool.
pub fn set_channel_emoji_app(app: slint::Weak<AppWindow>) {
    if let Some(ui) = app.upgrade() {
        let pool = std::rc::Rc::new(slint::VecModel::from(vec![slint::Image::default(); CHANNEL_EMOJI_SLOTS]));
        ui.set_channel_emoji_pool(pool.into());
    }
    if let Ok(mut slot) = CHANNEL_EMOJI_APP.lock() {
        *slot = Some(app);
    }
}

/// Channel names often start with an emoji. No loaded font draws emoji in colour,
/// so they showed as grey outlines; the chat already draws them as Twemoji images
/// from the emoji cache, and the channel list now does the same. Returns the
/// emoji fields of a `ChannelItem` (the rest default); a name without a leading
/// emoji keeps them empty and is drawn as text.
pub fn channel_emoji_fields(name: &str) -> ChannelItem {
    let trimmed = name.trim_start();
    let Some((0, len, _, emoji_id)) = gateway::formatting::find_next_unicode_emoji(trimmed) else {
        return ChannelItem::default();
    };
    let (slot, is_new) = {
        let mut slots = CHANNEL_EMOJI_SLOT_OF.get_or_init(Default::default).lock().unwrap();
        match slots.get(&emoji_id) {
            Some(slot) => (*slot, false),
            None if slots.len() < CHANNEL_EMOJI_SLOTS => {
                let slot = slots.len() as i32;
                slots.insert(emoji_id.clone(), slot);
                (slot, true)
            }
            // Pool full: this name keeps its emoji as text.
            None => return ChannelItem::default(),
        }
    };
    if is_new {
        let app = CHANNEL_EMOJI_APP.lock().ok().and_then(|a| a.clone());
        let id = emoji_id.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = app else { return };
            let emoji_mgr = emoji_cache::get_emoji_cache();
            match emoji_mgr.get(&id) {
                Some(img) => {
                    if let Some(ui) = app.upgrade() {
                        refresh_channel_emoji(&ui, &id, &img);
                    }
                }
                None => emoji_mgr.fetch_priority_async(&id, "", app),
            }
        });
    }
    ChannelItem {
        label: trimmed[len..].trim_start().into(),
        emoji_id: emoji_id.into(),
        emoji_slot: slot,
        ..Default::default()
    }
}

/// Puts `emoji_id`'s image in its pool slot once it is cached.
pub fn refresh_channel_emoji(ui: &AppWindow, emoji_id: &str, img: &slint::Image) {
    let slot = CHANNEL_EMOJI_SLOT_OF.get().and_then(|s| s.lock().ok()?.get(emoji_id).copied());
    if let Some(slot) = slot {
        let pool = ui.get_channel_emoji_pool();
        if (slot as usize) < pool.row_count() {
            pool.set_row_data(slot as usize, img.clone());
        }
    }
}

/// Emoji images that finished decoding and still have to reach the chat rows.
/// The pixels are kept as a raw buffer (which is `Send`) instead of a
/// `slint::Image` (which is not), so the queue can live in a static.
static PENDING_EMOJI_IMAGES: std::sync::LazyLock<Mutex<HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
/// Set while a refresh pass is already scheduled.
static EMOJI_REFRESH_SCHEDULED: AtomicBool = AtomicBool::new(false);

/// Queues an emoji image for the chat rows that use it.
///
/// A burst of decoded emojis used to trigger one full pass over the chat model
/// per emoji (each pass cloning every line and block of every row). The
/// pending images are now coalesced into a single pass.
pub fn queue_emoji_refresh(
    app_weak: &slint::Weak<AppWindow>,
    emoji_id: &str,
    buffer: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
) {
    if emoji_id.is_empty() {
        return;
    }
    if let Ok(mut pending) = PENDING_EMOJI_IMAGES.lock() {
        pending.insert(emoji_id.to_string(), buffer.clone());
    }
    if EMOJI_REFRESH_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = app_weak.clone();
    slint::Timer::single_shot(std::time::Duration::from_millis(60), move || {
        EMOJI_REFRESH_SCHEDULED.store(false, Ordering::Release);
        flush_emoji_refreshes(&weak);
    });
}

/// Applies every queued emoji image to the chat rows that use it.
fn flush_emoji_refreshes(app_weak: &slint::Weak<AppWindow>) {
    let pending = {
        let Ok(mut guard) = PENDING_EMOJI_IMAGES.lock() else { return; };
        std::mem::take(&mut *guard)
    };
    if pending.is_empty() {
        return;
    }
    let Some(ui) = app_weak.upgrade() else { return; };
    let images: HashMap<String, slint::Image> = pending
        .into_iter()
        .map(|(id, buffer)| (id, slint::Image::from_rgba8(buffer)))
        .collect();
    refresh_emojis_in_messages(&ui, &images);
}

/// Replaces emoji images in every row already painted that uses them.
///
/// Rows that do not use any of the given emojis are skipped without building
/// new vectors, so a pass over a 300 message channel stays cheap.
pub fn refresh_emojis_in_messages(ui: &AppWindow, images: &HashMap<String, slint::Image>) {
    if images.is_empty() {
        return;
    }
    let messages = ui.get_messages();
    let row_count = messages.row_count();

    for idx in 0..row_count {
        let Some(mut msg) = messages.row_data(idx) else { continue };

        // Fast path: most rows use none of these emojis. Checking without
        // allocating keeps the pass free for the common case.
        let uses_any = msg.content_lines.iter().any(|line| {
            line.blocks.iter().any(|b| b.is_emoji && images.contains_key(b.emoji_id.as_str()))
        }) || msg.embed_lines.iter().any(|line| {
            line.blocks.iter().any(|b| b.is_emoji && images.contains_key(b.emoji_id.as_str()))
        }) || msg.buttons.iter().any(|b| images.contains_key(b.emoji_id.as_str()));
        if !uses_any {
            continue;
        }

        let mut content_changed = false;
        let current_content_lines: Vec<MessageLine> = msg.content_lines.iter().collect();
        let mut updated_content_lines = Vec::with_capacity(current_content_lines.len());
        for line in current_content_lines {
            let blocks: Vec<MessageBlock> = line.blocks.iter().collect();
            let mut updated_blocks = Vec::with_capacity(blocks.len());
            let mut line_changed = false;
            for mut block in blocks {
                if block.is_emoji {
                    if let Some(img) = images.get(block.emoji_id.as_str()) {
                        block.emoji_img = img.clone();
                        line_changed = true;
                    }
                }
                updated_blocks.push(block);
            }
            if line_changed {
                content_changed = true;
                updated_content_lines.push(MessageLine {
                    blocks: slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_blocks))),
                });
            } else {
                // Unchanged line: keep the original model instead of rebuilding it.
                updated_content_lines.push(line);
            }
        }

        let mut embed_changed = false;
        let current_embed_lines: Vec<MessageLine> = msg.embed_lines.iter().collect();
        let mut updated_embed_lines = Vec::with_capacity(current_embed_lines.len());
        for line in current_embed_lines {
            let blocks: Vec<MessageBlock> = line.blocks.iter().collect();
            let mut updated_blocks = Vec::with_capacity(blocks.len());
            let mut line_changed = false;
            for mut block in blocks {
                if block.is_emoji {
                    if let Some(img) = images.get(block.emoji_id.as_str()) {
                        block.emoji_img = img.clone();
                        line_changed = true;
                    }
                }
                updated_blocks.push(block);
            }
            if line_changed {
                embed_changed = true;
                updated_embed_lines.push(MessageLine {
                    blocks: slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_blocks))),
                });
            } else {
                updated_embed_lines.push(line);
            }
        }

        let mut buttons_changed = false;
        let current_buttons: Vec<MessageButton> = msg.buttons.iter().collect();
        let mut updated_buttons = Vec::with_capacity(current_buttons.len());
        for mut button in current_buttons {
            if let Some(img) = images.get(button.emoji_id.as_str()) {
                button.emoji_img = img.clone();
                buttons_changed = true;
            }
            updated_buttons.push(button);
        }

        if content_changed {
            msg.content_lines = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_content_lines)));
        }
        if embed_changed {
            msg.embed_lines = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_embed_lines)));
        }
        if buttons_changed {
            msg.buttons = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_buttons)));
        }
        if content_changed || embed_changed || buttons_changed {
            messages.set_row_data(idx, msg);
        }
    }
}

/// Replaces one emoji image in every row already painted that uses it.
pub fn refresh_emoji_in_messages(ui: &AppWindow, emoji_id: &str, img: &slint::Image) {
    let mut images = HashMap::new();
    images.insert(emoji_id.to_string(), img.clone());
    refresh_emojis_in_messages(ui, &images);
}

#[cfg(test)]
mod command_index_tests {
    use super::*;

    #[test]
    fn command_index_keeps_chat_input_commands_and_typed_options() {
        let index = serde_json::json!({
            "applications": [
                {"id": "app-1", "name": "Example"}
            ],
            "application_commands": [
                {
                    "id": "cmd-1",
                    "application_id": "app-1",
                    "version": "v1",
                    "type": 1,
                    "name": "volume",
                    "description": "Set volume",
                    "options": [
                        {"type": 4, "name": "percent", "description": "0-200", "required": true}
                    ]
                },
                {
                    "id": "cmd-2",
                    "application_id": "app-1",
                    "type": 2,
                    "name": "User context",
                    "description": ""
                }
            ]
        });

        let items = command_items_from_index(&index);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "/volume");
        assert_eq!(items[0].option_type, 4);
        assert!(items[0].is_required);
        assert_eq!(items[0].usage, "/volume <percent>");
    }

    #[test]
    fn command_index_deduplicates_the_same_application_command() {
        let command = serde_json::json!({
            "id": "cmd-1",
            "application_id": "app-1",
            "type": 1,
            "name": "ping",
            "description": "Ping"
        });
        let index = serde_json::json!({
            "applications": [{"id": "app-1", "name": "Example"}],
            "application_commands": [command, command]
        });

        assert_eq!(command_items_from_index(&index).len(), 1);
    }
}

#[cfg(test)]
mod member_drag_tests {
    use super::*;

    fn row(id: &str, voice: bool, member: bool, parent: &str) -> ChannelItem {
        ChannelItem {
            id: id.into(),
            name: id.into(),
            is_voice: voice,
            is_voice_member: member,
            parent_channel_id: parent.into(),
            ..Default::default()
        }
    }

    #[test]
    fn dropping_a_member_on_a_voice_cluster_targets_its_channel() {
        let rows = vec![
            row("voice-1", true, false, ""),
            row("voice-member:voice-1:1", false, true, "voice-1"),
            row("voice-2", true, false, ""),
            row("voice-member:voice-2:2", false, true, "voice-2"),
        ];
        // From the first member row, moving down past voice-2 lands on its cluster.
        assert_eq!(voice_member_drop_row(&rows, 1, 20.0), Some(2));
    }

    #[test]
    fn dropping_a_member_on_a_text_channel_is_rejected() {
        let rows = vec![
            row("voice-member:voice-1:1", false, true, "voice-1"),
            row("text-1", false, false, ""),
        ];
        assert_eq!(voice_member_drop_row(&rows, 0, 50.0), None);
    }
}
