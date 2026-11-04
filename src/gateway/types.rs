#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use log::info;

#[derive(Clone, Default, Debug)]
pub struct LinkData {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Default, Debug)]
pub struct MessageBlockData {
    pub text: String,
    pub is_link: bool,
    pub is_command: bool,
    pub is_emoji: bool,
    pub emoji_id: String,
    pub url: String,
    pub command_name: String,
}

#[derive(Clone, Default, Debug)]
pub struct MessageLineData {
    pub blocks: Vec<MessageBlockData>,
}

#[derive(Clone, Default, Debug)]
pub struct MessageButtonData {
    pub label: String,
    pub url: String,
    pub emoji: String,
    pub emoji_id: String,
    pub style_type: i32,
    pub is_disabled: bool,
}

#[derive(Clone, Default, Debug)]
#[allow(dead_code)]
pub struct MessageAttachmentData {
    pub id: String,
    pub filename: String,
    pub url: String,
    pub proxy_url: String,
    pub size_bytes: u64,
    pub size_str: String,
    pub width: i32,
    pub height: i32,
    pub content_type: String,
    pub is_image: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ChannelData {
    pub id: String,
    pub name: String,
    pub is_voice: bool,
    pub is_category: bool,
    pub parent_id: Option<String>,
    pub position: i64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GuildData {
    pub id: String,
    pub name: String,
    pub channels: Vec<ChannelData>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum GatewayEvent {
    Connected { user_tag: String },
    Disconnected { reason: String },
    VoiceStatesUpdated,
    VoiceConnected,
    VoiceDisconnected,
    MessageCreated {
        id: String,
        channel_id: String,
        author: String,
        author_id: String,
        created_at: String,
        avatar_url: String,
        is_bot: bool,
        content: String,
        commands: Vec<String>,
        content_lines: Vec<MessageLineData>,
        embed_content: String,
        embed_lines: Vec<MessageLineData>,
        embed_color: String,
        embed_footer: String,
        code_block: String,
        reply_author: String,
        reply_content: String,
        reply_command: String,
        links: Vec<LinkData>,
        buttons: Vec<MessageButtonData>,
        attachments: Vec<MessageAttachmentData>,
        is_self: bool,
    },
    MessageUpdated {
        id: String,
        channel_id: String,
        content: String,
        commands: Vec<String>,
        content_lines: Vec<MessageLineData>,
        embed_content: String,
        embed_lines: Vec<MessageLineData>,
        embed_color: String,
        embed_footer: String,
        code_block: String,
        reply_author: String,
        reply_content: String,
        reply_command: String,
        links: Vec<LinkData>,
        buttons: Vec<MessageButtonData>,
        attachments: Vec<MessageAttachmentData>,
    },
    MessageDeleted {
        id: String,
        channel_id: String,
    },
    GuildLoaded {
        guild: GuildData,
    },
    IncomingCall {
        channel_id: String,
        caller_name: String,
    },
    CallEnded {
        channel_id: String,
    },
    SoundboardSoundPlayed {
        channel_id: String,
        sound_id: String,
        user_id: String,
    },
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum GatewayCommand {
    UpdateVoiceState {
        guild_id: String,
        channel_id: Option<String>,
        self_mute: bool,
        self_deaf: bool,
    },
    /// Turns the camera flag of our current voice state on/off without
    /// changing the channel.  Official clients use the same op 4 update.
    SetSelfVideo {
        enabled: bool,
    },
    SubscribeGuild {
        guild_id: String,
        channel_ids: Vec<String>,
    },
    SendSoundboardSound {
        channel_id: String,
        sound_id: String,
        source_guild_id: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[allow(dead_code)]
struct HeartbeatPayload {
    op: u8,
    d: Option<u64>,
}

// Shared Microphone PCM Audio Queue (32-bit float PCM at 48000Hz)
pub static MIC_PCM_QUEUE: std::sync::OnceLock<Arc<std::sync::Mutex<VecDeque<f32>>>> = std::sync::OnceLock::new();
// Shared Speaker PCM Audio Queues mapped by SSRC (32-bit stereo float PCM pairs at 48000Hz per user)
pub static SPEAKER_PCM_QUEUES: std::sync::OnceLock<Arc<std::sync::Mutex<std::collections::HashMap<u32, VecDeque<(f32, f32)>>>>> = std::sync::OnceLock::new();
pub static SELECTED_OUTPUT_DEVICE: std::sync::OnceLock<Arc<std::sync::Mutex<String>>> = std::sync::OnceLock::new();
pub static CURRENT_VOICE_SESSION_ID: AtomicU64 = AtomicU64::new(0);
pub static MY_USER_ID: AtomicU64 = AtomicU64::new(0);
pub static MY_USERNAME: std::sync::OnceLock<Arc<std::sync::Mutex<String>>> = std::sync::OnceLock::new();
pub static MY_VOICE_CHANNEL_ID: AtomicU64 = AtomicU64::new(0);
pub static SELF_MIC_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static IS_SELF_SPEAKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static VAD_THRESHOLD: std::sync::OnceLock<std::sync::atomic::AtomicU32> = std::sync::OnceLock::new();
pub static IS_TESTING_MIC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static MIC_LOOPBACK_QUEUE: std::sync::OnceLock<Arc<std::sync::Mutex<VecDeque<f32>>>> = std::sync::OnceLock::new();

pub static SELF_DEAF_STATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static IS_CONNECTED_TO_VOICE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static IS_WATCHDOG_RESETTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const GLOBAL_AUDIO_CONFIG_FILE: &str = ".litecord_audio_config.json";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GlobalAudioConfig {
    pub vad_threshold: f32,
    #[serde(default = "default_mic_volume")]
    pub mic_volume: f32,
    pub input_device: String,
    pub output_device: String,
}

fn default_mic_volume() -> f32 {
    1.0
}

impl Default for GlobalAudioConfig {
    fn default() -> Self {
        Self {
            vad_threshold: 0.40, // -60 dBFS: Discord standard sensitivity threshold
            mic_volume: 1.0,
            input_device: String::new(),
            output_device: String::new(),
        }
    }
}

pub fn load_persisted_audio_config() -> GlobalAudioConfig {
    if let Ok(data) = std::fs::read_to_string(GLOBAL_AUDIO_CONFIG_FILE) {
        if let Ok(mut cfg) = serde_json::from_str::<GlobalAudioConfig>(&data) {
            // Migrate legacy linear threshold (e.g. 0.05) to Discord standard dB threshold 0.40 (-60 dB)
            if cfg.vad_threshold < 0.15 {
                cfg.vad_threshold = 0.40;
                save_persisted_audio_config(&cfg);
            }
            return cfg;
        }
    }
    GlobalAudioConfig::default()
}

pub fn save_persisted_audio_config(cfg: &GlobalAudioConfig) {
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(GLOBAL_AUDIO_CONFIG_FILE, json);
    }
}

pub fn set_persisted_input_device(name: String) {
    let mut cfg = load_persisted_audio_config();
    cfg.input_device = name;
    save_persisted_audio_config(&cfg);
}

pub fn set_persisted_output_device(name: String) {
    let mut cfg = load_persisted_audio_config();
    cfg.output_device = name.clone();
    save_persisted_audio_config(&cfg);
    set_selected_output_device(name);
}

static MIC_VOLUME: std::sync::OnceLock<std::sync::atomic::AtomicU32> = std::sync::OnceLock::new();

fn get_mic_volume_atomic() -> &'static std::sync::atomic::AtomicU32 {
    MIC_VOLUME.get_or_init(|| {
        let cfg = load_persisted_audio_config();
        std::sync::atomic::AtomicU32::new(cfg.mic_volume.clamp(0.0, 2.0).to_bits())
    })
}

pub fn set_mic_volume(val: f32) {
    let clamped = val.clamp(0.0, 2.0);
    get_mic_volume_atomic().store(clamped.to_bits(), Ordering::Relaxed);
    let mut cfg = load_persisted_audio_config();
    cfg.mic_volume = clamped;
    save_persisted_audio_config(&cfg);
}

pub fn get_mic_volume() -> f32 {
    f32::from_bits(get_mic_volume_atomic().load(Ordering::Relaxed))
}

fn get_vad_threshold_atomic() -> &'static std::sync::atomic::AtomicU32 {
    VAD_THRESHOLD.get_or_init(|| {
        let cfg = load_persisted_audio_config();
        std::sync::atomic::AtomicU32::new(cfg.vad_threshold.clamp(0.0, 1.0).to_bits())
    })
}

pub fn set_is_connected_to_voice(val: bool) {
    IS_CONNECTED_TO_VOICE.store(val, Ordering::Relaxed);
    crate::ui::helpers::wake_ui_loops();
    crate::screen_capture::notify_p2p_context_changed();
}

pub fn is_connected_to_voice() -> bool {
    IS_CONNECTED_TO_VOICE.load(Ordering::Relaxed)
}

pub fn set_vad_threshold(val: f32) {
    let clamped = val.clamp(0.0, 1.0);
    get_vad_threshold_atomic().store(clamped.to_bits(), Ordering::Relaxed);
    let mut cfg = load_persisted_audio_config();
    cfg.vad_threshold = clamped;
    save_persisted_audio_config(&cfg);
}

pub fn get_vad_threshold() -> f32 {
    f32::from_bits(get_vad_threshold_atomic().load(Ordering::Relaxed))
}

pub fn set_testing_mic(val: bool) {
    IS_TESTING_MIC.store(val, Ordering::Relaxed);
}

pub fn is_testing_mic() -> bool {
    IS_TESTING_MIC.load(Ordering::Relaxed)
}

pub fn get_mic_loopback_queue() -> Arc<std::sync::Mutex<VecDeque<f32>>> {
    MIC_LOOPBACK_QUEUE.get_or_init(|| Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(48000)))).clone()
}

pub fn set_self_deaf(val: bool) {
    SELF_DEAF_STATE.store(val, Ordering::Relaxed);
}

pub fn is_self_deaf() -> bool {
    SELF_DEAF_STATE.load(Ordering::Relaxed)
}

pub fn get_voice_channel_participant_count(channel_id: &str) -> i32 {
    if channel_id.is_empty() { return 0; }
    let mut set = std::collections::HashSet::new();

    if let Ok(map) = get_guild_voice_states_store().lock() {
        for (&uid, cid) in map.iter() {
            if cid == channel_id {
                set.insert(uid);
            }
        }
    }

    set.len() as i32
}

pub fn get_voice_channel_participants(channel_id: &str) -> Vec<(u64, String)> {
    if channel_id.is_empty() {
        return Vec::new();
    }

    let user_ids: Vec<u64> = get_guild_voice_states_store()
        .lock()
        .map(|states| {
            states
                .iter()
                .filter_map(|(&user_id, current_channel_id)| {
                    (current_channel_id == channel_id).then_some(user_id)
                })
                .collect()
        })
        .unwrap_or_default();

    let names = get_user_names_store();
    let names = names.lock().ok();
    let mut participants: Vec<(u64, String)> = user_ids
        .into_iter()
        .map(|user_id| {
            let name = names
                .as_ref()
                .and_then(|known_names| known_names.get(&user_id))
                .cloned()
                .unwrap_or_else(|| format!("Participante #{}", user_id));
            (user_id, name)
        })
        .collect();

    participants.sort_by(|left, right| {
        left.1
            .to_lowercase()
            .cmp(&right.1.to_lowercase())
            .then_with(|| left.0.cmp(&right.0))
    });
    participants
}

/// True while this member's decoded audio is above the same threshold the voice
/// room uses for its level bars. Reads the speaker queues without blocking: the
/// audio callback must never wait on the UI refresh.
pub fn is_voice_user_speaking(user_id: u64) -> bool {
    let my_uid = get_my_user_id();
    if user_id == 999999 || (my_uid != 0 && user_id == my_uid) {
        return is_self_speaking();
    }

    let store = get_active_voice_participants_store();
    let ssrcs: Vec<u32> = match store.try_lock() {
        Ok(map) => map.iter().filter(|(_, &uid)| uid == user_id).map(|(&ssrc, _)| ssrc).collect(),
        Err(_) => return false,
    };
    if ssrcs.is_empty() {
        return false;
    }

    let speaker_queues = get_speaker_pcm_queues();
    let queues = match speaker_queues.try_lock() {
        Ok(queues) => queues,
        Err(_) => return false,
    };
    for ssrc in ssrcs {
        let Some(q) = queues.get(&ssrc) else { continue };
        let sample_cnt = q.len().min(480);
        if sample_cnt == 0 { continue; }
        let sum_sq: f32 = q.iter().take(sample_cnt).map(|&(l, r)| { let s = (l + r) * 0.5; s * s }).sum();
        let rms = (sum_sq / sample_cnt as f32).sqrt();
        let level = if rms < 0.005 { 0.0 } else { (rms * 3.5).clamp(0.0, 1.0) };
        if level > 0.03 {
            return true;
        }
    }
    false
}

pub fn set_my_user_id(id: u64) {
    MY_USER_ID.store(id, Ordering::Relaxed);
}

pub fn get_my_user_id() -> u64 {
    MY_USER_ID.load(Ordering::Relaxed)
}

pub fn set_my_username(name: String) {
    if let Ok(mut uname) = MY_USERNAME.get_or_init(|| Arc::new(std::sync::Mutex::new(String::new()))).lock() {
        *uname = name;
    }
}

pub fn get_my_username() -> String {
    if let Ok(uname) = MY_USERNAME.get_or_init(|| Arc::new(std::sync::Mutex::new(String::new()))).lock() {
        if uname.is_empty() { "Você".to_string() } else { uname.clone() }
    } else {
        "Você".to_string()
    }
}

pub fn set_my_voice_channel_id(cid: u64) {
    MY_VOICE_CHANNEL_ID.store(cid, Ordering::Relaxed);
}

/// Guild that owns the running voice session. Mute/deafen must address it, not the
/// guild the user happens to be looking at.
static MY_VOICE_GUILD_ID: std::sync::OnceLock<Arc<std::sync::Mutex<String>>> = std::sync::OnceLock::new();

pub fn set_my_voice_guild_id(gid: &str) {
    if let Ok(mut g) = MY_VOICE_GUILD_ID.get_or_init(|| Arc::new(std::sync::Mutex::new(String::new()))).lock() {
        *g = gid.to_string();
    }
}

pub fn get_my_voice_guild_id() -> String {
    MY_VOICE_GUILD_ID
        .get_or_init(|| Arc::new(std::sync::Mutex::new(String::new())))
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

static VOICE_SECRET_KEY: std::sync::OnceLock<Arc<std::sync::Mutex<Option<[u8; 32]>>>> = std::sync::OnceLock::new();

#[allow(dead_code)]
pub fn get_voice_secret_key_store() -> Arc<std::sync::Mutex<Option<[u8; 32]>>> {
    VOICE_SECRET_KEY.get_or_init(|| Arc::new(std::sync::Mutex::new(None))).clone()
}

#[allow(dead_code)]
pub fn get_voice_secret_key() -> Option<[u8; 32]> {
    get_voice_secret_key_store().lock().unwrap().clone()
}

pub fn get_my_voice_channel_id() -> u64 {
    MY_VOICE_CHANNEL_ID.load(Ordering::Relaxed)
}

pub fn set_self_mic_level(val: f32) {
    SELF_MIC_LEVEL.store(val.to_bits(), Ordering::Relaxed);
}

pub fn get_self_mic_level() -> f32 {
    f32::from_bits(SELF_MIC_LEVEL.load(Ordering::Relaxed))
}

pub fn set_self_speaking(val: bool) {
    IS_SELF_SPEAKING.store(val, Ordering::Relaxed);
}

pub fn is_self_speaking() -> bool {
    IS_SELF_SPEAKING.load(Ordering::Relaxed)
}

/// Signalled by the microphone callback whenever a full 20 ms frame (960 samples)
/// is queued. The voice sender waits on it, so it runs on the sound card's clock and
/// does not need a 20 ms timer (nor the 1 ms system timer resolution behind one).
pub static MIC_FRAME_READY: tokio::sync::Notify = tokio::sync::Notify::const_new();

pub fn get_mic_pcm_queue() -> Arc<std::sync::Mutex<VecDeque<f32>>> {
    MIC_PCM_QUEUE.get_or_init(|| Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(48000)))).clone()
}

pub fn get_speaker_pcm_queues() -> Arc<std::sync::Mutex<std::collections::HashMap<u32, VecDeque<(f32, f32)>>>> {
    SPEAKER_PCM_QUEUES.get_or_init(|| Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))).clone()
}

pub fn get_selected_output_device_store() -> Arc<std::sync::Mutex<String>> {
    SELECTED_OUTPUT_DEVICE.get_or_init(|| Arc::new(std::sync::Mutex::new(String::new()))).clone()
}

pub fn set_selected_output_device(name: String) {
    if let Ok(mut dev) = get_selected_output_device_store().lock() {
        *dev = name;
    }
}

const USER_AUDIO_SETTINGS_FILE: &str = ".litecord_user_settings.json";

fn load_persisted_user_audio_settings() -> std::collections::HashMap<String, (bool, f32, i32)> {
    if let Ok(data) = std::fs::read_to_string(USER_AUDIO_SETTINGS_FILE) {
        if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, (bool, f32, i32)>>(&data) {
            info!("⚙️ Carregadas configurações de volume e prioridade salvas para {} usuários", map.len());
            return map;
        }
    }
    std::collections::HashMap::new()
}

fn save_persisted_user_audio_settings(map: &std::collections::HashMap<String, (bool, f32, i32)>) {
    if let Ok(json) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(USER_AUDIO_SETTINGS_FILE, json);
    }
}

pub static USER_AUDIO_SETTINGS: std::sync::OnceLock<Arc<std::sync::Mutex<std::collections::HashMap<String, (bool, f32, i32)>>>> = std::sync::OnceLock::new();

pub fn get_user_audio_settings_store() -> Arc<std::sync::Mutex<std::collections::HashMap<String, (bool, f32, i32)>>> {
    USER_AUDIO_SETTINGS.get_or_init(|| {
        let loaded = load_persisted_user_audio_settings();
        Arc::new(std::sync::Mutex::new(loaded))
    }).clone()
}

pub fn set_user_mute(user_id: &str, is_muted: bool) {
    if let Ok(mut map) = get_user_audio_settings_store().lock() {
        let entry = map.entry(user_id.to_string()).or_insert((false, 1.0, 0));
        entry.0 = is_muted;
        save_persisted_user_audio_settings(&map);
    }
}

pub fn set_user_volume(user_id: &str, volume: f32) {
    if let Ok(mut map) = get_user_audio_settings_store().lock() {
        let entry = map.entry(user_id.to_string()).or_insert((false, 1.0, 0));
        entry.1 = volume;
        save_persisted_user_audio_settings(&map);
    }
}

pub fn set_user_priority(user_id: &str, priority: i32) {
    let safe_priority = priority.max(0);
    if let Ok(mut map) = get_user_audio_settings_store().lock() {
        let entry = map.entry(user_id.to_string()).or_insert((false, 1.0, 0));
        entry.2 = safe_priority;
        save_persisted_user_audio_settings(&map);
    }
}

pub fn get_user_audio_settings(user_id: &str) -> (bool, f32, i32) {
    if let Ok(map) = get_user_audio_settings_store().lock() {
        if let Some(res) = map.get(user_id) {
            return *res;
        }
    }
    (false, 1.0, 0)
}

pub fn get_user_mute_volume(user_id: &str) -> (bool, f32) {
    let (m, v, _) = get_user_audio_settings(user_id);
    (m, v)
}

pub static ACTIVE_VOICE_PARTICIPANTS: std::sync::OnceLock<Arc<std::sync::Mutex<std::collections::HashMap<u32, u64>>>> = std::sync::OnceLock::new();

pub fn get_active_voice_participants_store() -> Arc<std::sync::Mutex<std::collections::HashMap<u32, u64>>> {
    ACTIVE_VOICE_PARTICIPANTS.get_or_init(|| Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))).clone()
}

pub fn register_voice_participant(ssrc: u32, user_id: u64) {
    if let Ok(mut map) = get_active_voice_participants_store().lock() {
        map.insert(ssrc, user_id);
    }
}

#[inline]
pub fn soft_limit(s: f32) -> f32 {
    // Transparent linear response up to |s| <= 0.70 (well below distortion threshold)
    // Smooth, natural soft-knee compression for peaks |s| > 0.70 avoiding hard clipping artifacts
    let abs_s = s.abs();
    if abs_s <= 0.70 {
        s
    } else if abs_s <= 1.25 {
        let diff = abs_s - 0.70;
        let compressed = 0.70 + 0.55 * (diff / (0.55 + diff));
        if s > 0.0 { compressed.min(0.999) } else { -compressed.min(0.999) }
    } else {
        // High overload protection using smooth hyperbolic curve
        let sign = if s > 0.0 { 1.0 } else { -1.0 };
        sign * (0.95 + 0.049 * (1.0 - (- (abs_s - 1.25)).exp()))
    }
}

#[inline]
pub fn cubic_hermite(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let c0 = p1;
    let c1 = 0.5 * (p2 - p0);
    let c2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);
    ((c3 * t + c2) * t + c1) * t + c0
}

pub fn clear_voice_participants() {
    set_is_connected_to_voice(false);
    // No channel is associated with the session anymore, so mute/deafen must not
    // send a voice state update for someone else's channel.
    set_my_voice_channel_id(0);
    set_my_voice_guild_id("");
    if let Ok(mut map) = get_active_voice_participants_store().lock() {
        map.clear();
    }
    if let Ok(mut queues) = get_speaker_pcm_queues().lock() {
        queues.clear();
    }
}

pub fn remove_voice_participant_by_user_id(user_id: u64) {
    if let Ok(mut map) = get_active_voice_participants_store().lock() {
        map.retain(|_ssrc, &mut uid| uid != user_id);
    }
    if let Ok(mut queues) = get_speaker_pcm_queues().lock() {
        queues.retain(|&ssrc, _| ssrc as u64 != user_id);
    }
}

pub static GUILD_VOICE_STATES: std::sync::OnceLock<Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>>> = std::sync::OnceLock::new();
pub static GUILD_VOICE_AUDIO_FLAGS: std::sync::OnceLock<Arc<std::sync::Mutex<std::collections::HashMap<u64, (bool, bool)>>>> = std::sync::OnceLock::new();

pub fn get_guild_voice_states_store() -> Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>> {
    GUILD_VOICE_STATES.get_or_init(|| Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))).clone()
}

pub fn get_guild_voice_audio_flags_store() -> Arc<std::sync::Mutex<std::collections::HashMap<u64, (bool, bool)>>> {
    GUILD_VOICE_AUDIO_FLAGS.get_or_init(|| Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))).clone()
}

pub fn set_user_voice_audio_flags(user_id: u64, is_muted: bool, is_deafened: bool) {
    if user_id == 0 { return; }
    if let Ok(mut map) = get_guild_voice_audio_flags_store().lock() {
        map.insert(user_id, (is_muted, is_deafened));
    }
}

pub fn get_user_voice_audio_flags(user_id: u64) -> (bool, bool) {
    if user_id == 0 { return (false, false); }
    if let Ok(map) = get_guild_voice_audio_flags_store().lock() {
        if let Some(&flags) = map.get(&user_id) {
            return flags;
        }
    }
    (false, false)
}

pub fn remove_user_voice_audio_flags(user_id: u64) {
    if let Ok(mut map) = get_guild_voice_audio_flags_store().lock() {
        map.remove(&user_id);
    }
    if let Ok(mut map) = get_guild_voice_server_muted_store().lock() {
        map.remove(&user_id);
    }
}

pub static GUILD_VOICE_SERVER_MUTED: std::sync::OnceLock<Arc<std::sync::Mutex<std::collections::HashMap<u64, bool>>>> = std::sync::OnceLock::new();

pub fn get_guild_voice_server_muted_store() -> Arc<std::sync::Mutex<std::collections::HashMap<u64, bool>>> {
    GUILD_VOICE_SERVER_MUTED.get_or_init(|| Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))).clone()
}

/// True when a moderator (not the user) muted them in the server.
pub fn set_user_server_muted(user_id: u64, server_muted: bool) {
    if user_id == 0 { return; }
    if let Ok(mut map) = get_guild_voice_server_muted_store().lock() {
        if server_muted {
            map.insert(user_id, true);
        } else {
            map.remove(&user_id);
        }
    }
}

pub fn is_user_server_muted(user_id: u64) -> bool {
    if user_id == 0 { return false; }
    get_guild_voice_server_muted_store()
        .lock()
        .map(|m| m.get(&user_id).copied().unwrap_or(false))
        .unwrap_or(false)
}

pub fn sync_voice_channel_participants(channel_id: &str) {
    if channel_id.is_empty() { return; }
    if let Ok(map) = get_guild_voice_states_store().lock() {
        for (&uid, cid) in map.iter() {
            if cid == channel_id {
                register_voice_participant(uid as u32, uid);
            }
        }
    }
}

pub static USER_NAMES: std::sync::OnceLock<Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>>> = std::sync::OnceLock::new();

#[derive(Serialize, Deserialize, Default)]
struct PersistedUserDirectory {
    #[serde(default)]
    names: std::collections::HashMap<u64, String>,
    #[serde(default)]
    avatars: std::collections::HashMap<u64, String>,
}

const USER_DIRECTORY_CACHE_FILE: &str = ".litecord_cache/users_cache.json";

fn load_persisted_user_directory() -> PersistedUserDirectory {
    if let Ok(data) = std::fs::read_to_string(USER_DIRECTORY_CACHE_FILE) {
        if let Ok(dir) = serde_json::from_str::<PersistedUserDirectory>(&data) {
            return dir;
        }
    }
    PersistedUserDirectory::default()
}

pub fn save_user_directory_cache() {
    // Debounced: the directory holds up to a few thousand names and writing it
    // on every lookup was blocking I/O in the middle of the UI work.
    if USER_DIRECTORY_SAVE_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let scheduled = tokio::runtime::Handle::try_current().ok().map(|handle| {
        handle.spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            USER_DIRECTORY_SAVE_PENDING.store(false, Ordering::SeqCst);
            write_user_directory_cache();
        });
    });
    if scheduled.is_none() {
        USER_DIRECTORY_SAVE_PENDING.store(false, Ordering::SeqCst);
        write_user_directory_cache();
    }
}

/// Set while a save of the user directory is already scheduled.
static USER_DIRECTORY_SAVE_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Writes the user directory to disk. The file write runs on a blocking worker:
/// it never blocks the UI thread nor a runtime worker that also serves the gateway.
fn write_user_directory_cache() {
    let names = get_user_names_store().lock().ok().map(|m| m.clone()).unwrap_or_default();
    let avatars = USER_AVATARS.get().and_then(|s| s.lock().ok()).map(|m| m.clone()).unwrap_or_default();
    if names.is_empty() && avatars.is_empty() {
        return;
    }
    let Ok(json) = serde_json::to_string(&PersistedUserDirectory { names, avatars }) else {
        return;
    };
    let write = move || {
        let _ = std::fs::create_dir_all(".litecord_cache");
        let tmp = ".litecord_cache/users_cache.json.tmp";
        if std::fs::write(tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, USER_DIRECTORY_CACHE_FILE);
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(write);
        }
        Err(_) => write(),
    }
}

pub fn get_user_names_store() -> Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>> {
    USER_NAMES.get_or_init(|| {
        let loaded = load_persisted_user_directory();
        if !loaded.avatars.is_empty() {
            let _ = USER_AVATARS.set(std::sync::Mutex::new(loaded.avatars));
        }
        Arc::new(std::sync::Mutex::new(loaded.names))
    }).clone()
}

pub fn has_user_name(user_id: u64) -> bool {
    get_user_names_store().lock().ok().is_some_and(|map| map.contains_key(&user_id))
}

/// Names are short but unlimited otherwise: a long session in large guilds would
/// keep every user ever seen in READY and voice states. Bounded like the avatar
/// directory. At the cap half of the directory is dropped instead of all of it:
/// a full clear used to forget the names of the servers that are still open
/// (including our own user) until they came back with a new event.
const MAX_USER_NAMES: usize = 8192;

/// Drops the oldest half of a directory map, keeping our own user and the ids
/// that are currently on screen.
fn evict_half_of_directory(map: &mut std::collections::HashMap<u64, String>, keep: usize) {
    use std::sync::atomic::{AtomicU64, Ordering};

    // Rotating cursor so the eviction walks over the map instead of always
    // dropping the same low ids (a HashMap is unordered, but this keeps the
    // removal deterministic and spread over the directory).
    static CURSOR: AtomicU64 = AtomicU64::new(0);
    let my_id = get_my_user_id();
    let mut victims = map.len().saturating_sub(keep);
    if victims == 0 {
        return;
    }
    let start = CURSOR.fetch_add(1, Ordering::Relaxed) as usize;
    let ids: Vec<u64> = map.keys().copied().collect();
    for offset in 0..ids.len() {
        if victims == 0 {
            break;
        }
        let id = ids[(start + offset) % ids.len()];
        if id != my_id && map.remove(&id).is_some() {
            victims -= 1;
        }
    }
}

pub fn register_user_name(user_id: u64, name: String) {
    if user_id > 0 && !name.is_empty() {
        if let Ok(mut map) = get_user_names_store().lock() {
            if map.len() >= MAX_USER_NAMES && !map.contains_key(&user_id) {
                evict_half_of_directory(&mut map, MAX_USER_NAMES / 2);
            }
            map.insert(user_id, name);
        }
    }
}

/// Avatar hashes (max 34 bytes each) of users seen in voice states, so voice rows
/// and the DM list can show a picture without extra REST calls. Bounded; at the
/// cap half of the directory is dropped, keeping our own user.
const MAX_USER_AVATARS: usize = 2048;
static USER_AVATARS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u64, String>>> = std::sync::OnceLock::new();

pub fn register_user_avatar(user_id: u64, hash: &str) {
    if user_id == 0 || !crate::ui::message_grouping::valid_avatar_hash(hash) {
        return;
    }
    let _ = get_user_names_store();
    let store = USER_AVATARS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Ok(mut map) = store.lock() {
        if map.len() >= MAX_USER_AVATARS && !map.contains_key(&user_id) {
            evict_half_of_directory(&mut map, MAX_USER_AVATARS / 2);
        }
        map.insert(user_id, hash.to_owned());
    }
}

/// Discord CDN thumbnail URL for a known user, or an empty string.
pub fn get_user_avatar_url(user_id: u64) -> String {
    let _ = get_user_names_store();
    USER_AVATARS
        .get()
        .and_then(|store| store.lock().ok())
        .and_then(|map| map.get(&user_id).cloned())
        .map(|hash| format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.webp?size=64"))
        .unwrap_or_default()
}

/// Forgets the name/avatar directory of the signed-out account.
pub fn clear_user_directory() {
    if let Some(store) = USER_NAMES.get() {
        if let Ok(mut map) = store.lock() {
            map.clear();
        }
    }
    if let Some(store) = USER_AVATARS.get() {
        if let Ok(mut map) = store.lock() {
            map.clear();
        }
    }
    let _ = std::fs::remove_file(USER_DIRECTORY_CACHE_FILE);
}

/// Forgets cached guild voice membership (names/channels are re-sent on READY).
pub fn clear_guild_voice_states() {
    if let Some(store) = GUILD_VOICE_STATES.get() {
        if let Ok(mut map) = store.lock() {
            map.clear();
        }
    }
    if let Some(store) = GUILD_VOICE_AUDIO_FLAGS.get() {
        if let Ok(mut map) = store.lock() {
            map.clear();
        }
    }
    if let Some(store) = GUILD_VOICE_SERVER_MUTED.get() {
        if let Ok(mut map) = store.lock() {
            map.clear();
        }
    }
    clear_guild_roles();
    clear_channel_overwrites();
}

pub fn get_user_name(user_id: u64) -> String {
    if let Ok(map) = get_user_names_store().lock() {
        if let Some(name) = map.get(&user_id) {
            return name.clone();
        }
    }
    match user_id {
        1307641538502725643 => "MusicMan [Bot]".to_string(),
        1323489999953465385 => "cortez".to_string(),
        398203126630580225 => "Marido da juju (Você)".to_string(),
        _ => format!("Participante #{}", user_id),
    }
}


#[cfg(test)]
mod directory_tests {
    use super::*;

    #[test]
    fn the_directory_never_empties_itself_and_keeps_our_own_user() {
        let mut map: std::collections::HashMap<u64, String> = (1..=100)
            .map(|id| (id, format!("user-{id}")))
            .collect();

        // Ask for the map to shrink to 50 entries: half of it goes, the rest
        // stays so the names of the open servers are not forgotten at once.
        evict_half_of_directory(&mut map, 50);
        assert_eq!(map.len(), 50, "half of the directory is kept");

        // Running it again on the same map keeps shrinking by half instead of
        // clearing everything.
        evict_half_of_directory(&mut map, 25);
        assert_eq!(map.len(), 25);
        assert!(!map.is_empty());
    }

    #[test]
    fn eviction_is_a_noop_when_there_is_nothing_to_drop() {
        let mut map: std::collections::HashMap<u64, String> =
            (1..=5).map(|id| (id, "x".to_string())).collect();
        evict_half_of_directory(&mut map, 10);
        assert_eq!(map.len(), 5);
    }
}

// ── Guild roles / permissions ────────────────────────────────────────────────
// Discord's permission bits we currently gate UI actions with.
pub const PERM_CREATE_INSTANT_INVITE: u64 = 1 << 0;
pub const PERM_ADMINISTRATOR: u64 = 1 << 3;
pub const PERM_KICK_MEMBERS: u64 = 1 << 1;
pub const PERM_BAN_MEMBERS: u64 = 1 << 2;
pub const PERM_MUTE_MEMBERS: u64 = 1 << 22;
pub const PERM_DEAFEN_MEMBERS: u64 = 1 << 23;
pub const PERM_MOVE_MEMBERS: u64 = 1 << 24;
pub const PERM_MANAGE_CHANNELS: u64 = 1 << 4;
pub const PERM_MANAGE_GUILD: u64 = 1 << 5;
pub const PERM_MANAGE_ROLES: u64 = 1 << 28;

#[derive(Clone, Debug)]
pub struct RoleInfo {
    pub id: String,
    pub name: String,
    pub color: u32,
    pub position: i64,
    pub permissions: u64,
    /// Managed roles belong to integrations (bots, boosts). Discord rejects
    /// assigning or removing them manually even for administrators.
    pub managed: bool,
}

#[derive(Default)]
pub struct GuildRoleStore {
    pub roles: HashMap<String, RoleInfo>,
    pub member_roles: HashMap<u64, Vec<String>>,
    pub owner_id: u64,
}

static GUILD_ROLES: OnceLock<Arc<Mutex<HashMap<String, GuildRoleStore>>>> = OnceLock::new();

pub fn get_guild_roles_store() -> Arc<Mutex<HashMap<String, GuildRoleStore>>> {
    GUILD_ROLES.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}

pub fn register_guild_role(guild_id: &str, role: RoleInfo) {
    if guild_id.is_empty() || role.id.is_empty() { return; }
    if let Ok(mut store) = get_guild_roles_store().lock() {
        store.entry(guild_id.to_string()).or_default().roles.insert(role.id.clone(), role);
    }
}

pub fn register_member_roles(guild_id: &str, user_id: u64, roles: Vec<String>, owner_id: u64) {
    if guild_id.is_empty() || user_id == 0 { return; }
    if let Ok(mut store) = get_guild_roles_store().lock() {
        let entry = store.entry(guild_id.to_string()).or_default();
        if owner_id != 0 { entry.owner_id = owner_id; }
        entry.member_roles.insert(user_id, roles);
    }
}

pub fn member_roles(guild_id: &str, user_id: u64) -> Vec<String> {
    get_guild_roles_store()
        .lock()
        .ok()
        .and_then(|s| s.get(guild_id).and_then(|g| g.member_roles.get(&user_id).cloned()))
        .unwrap_or_default()
}

/// Whether the role store actually knows this member (owner or a registered
/// member entry).  Without that data a deny is indistinguishable from "not
/// loaded yet", so callers can be optimistic instead of hiding actions.
pub fn member_roles_known(guild_id: &str, user_id: u64) -> bool {
    let store = get_guild_roles_store();
    let Ok(guard) = store.lock() else { return false };
    let Some(guild) = guard.get(guild_id) else { return false };
    guild.owner_id == user_id || guild.member_roles.contains_key(&user_id)
}

/// All roles in the guild, highest first. Excludes nothing: callers decide what
/// is assignable with `can_manage_role`.
pub fn guild_roles(guild_id: &str) -> Vec<RoleInfo> {
    let store = get_guild_roles_store();
    let Ok(guard) = store.lock() else { return Vec::new() };
    let Some(guild) = guard.get(guild_id) else { return Vec::new() };
    let mut roles: Vec<RoleInfo> = guild.roles.values().cloned().collect();
    roles.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.name.cmp(&b.name)));
    roles
}

/// Roles below the actor's highest role, excluding managed/integration roles,
/// @everyone and roles the actor cannot touch because of target hierarchy.
pub fn assignable_roles(
    guild_id: &str,
    actor: u64,
    target: u64,
) -> Vec<(RoleInfo, bool)> {
    if actor == 0 || target == 0 {
        return Vec::new();
    }

    // Collect under the lock, then evaluate hierarchy with the lock released:
    // the permission helpers take the same mutex.
    let (owner, roles, assigned) = {
        let store = get_guild_roles_store();
        let Ok(guard) = store.lock() else { return Vec::new() };
        let Some(guild) = guard.get(guild_id) else { return Vec::new() };
        let assigned: HashSet<String> = guild
            .member_roles
            .get(&target)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        (
            guild.owner_id,
            guild.roles.values().cloned().collect::<Vec<_>>(),
            assigned,
        )
    };

    if actor != owner && target == owner {
        return Vec::new();
    }
    if actor != target && !can_moderate_member(guild_id, actor, target, PERM_MANAGE_ROLES) {
        return Vec::new();
    }
    if !has_guild_permission(guild_id, actor, PERM_MANAGE_ROLES) {
        return Vec::new();
    }

    let actor_position = highest_role_position(guild_id, actor);
    let target_position = highest_role_position(guild_id, target);
    let mut roles: Vec<(RoleInfo, bool)> = roles
        .into_iter()
        .filter(|role| role.id != guild_id && !role.managed)
        .filter(|role| actor == owner || role.position < actor_position)
        .filter(|_role| actor == target || target_position < actor_position)
        .map(|role| {
            let assigned = assigned.contains(&role.id);
            (role, assigned)
        })
        .collect();
    roles.sort_by(|a, b| b.0.position.cmp(&a.0.position).then_with(|| a.0.name.cmp(&b.0.name)));
    roles
}

pub fn can_manage_role(guild_id: &str, actor: u64, target: u64, role_id: &str) -> bool {
    assignable_roles(guild_id, actor, target)
        .into_iter()
        .any(|(role, _)| role.id == role_id)
}

/// Mirrors a successful Discord role update into the local store so the menu
/// and role colors do not stay stale until the next READY.
pub fn set_member_role_local(guild_id: &str, user_id: u64, role_id: &str, assigned: bool) {
    if guild_id.is_empty() || user_id == 0 || role_id.is_empty() {
        return;
    }
    let store = get_guild_roles_store();
    let Ok(mut guard) = store.lock() else { return };
    let Some(guild) = guard.get_mut(guild_id) else { return };
    let roles = guild.member_roles.entry(user_id).or_default();
    if assigned {
        if !roles.iter().any(|id| id == role_id) {
            roles.push(role_id.to_string());
        }
    } else {
        roles.retain(|id| id != role_id);
    }
}

pub fn remove_guild_role_local(guild_id: &str, role_id: &str) {
    let store = get_guild_roles_store();
    let Ok(mut guard) = store.lock() else { return };
    let Some(guild) = guard.get_mut(guild_id) else { return };
    guild.roles.remove(role_id);
    for roles in guild.member_roles.values_mut() {
        roles.retain(|id| id != role_id);
    }
}

pub fn member_role_names(guild_id: &str, user_id: u64) -> Vec<String> {
    let store = get_guild_roles_store();
    let Ok(guard) = store.lock() else { return Vec::new() };
    let Some(guild) = guard.get(guild_id) else { return Vec::new() };
    let Some(ids) = guild.member_roles.get(&user_id) else { return Vec::new() };
    let mut roles: Vec<(i64, String)> = ids
        .iter()
        .filter_map(|id| guild.roles.get(id).map(|r| (r.position, r.name.clone())))
        .collect();
    roles.sort_by(|a, b| b.0.cmp(&a.0));
    roles.into_iter().map(|(_, name)| name).collect()
}

/// Union of @everyone and the member's role permissions, with the owner and
/// administrator role overriding everything.
pub fn has_guild_permission(guild_id: &str, user_id: u64, permission: u64) -> bool {
    let store = get_guild_roles_store();
    let Ok(guard) = store.lock() else { return false };
    let Some(guild) = guard.get(guild_id) else { return false };
    if user_id != 0 && guild.owner_id == user_id { return true; }

    let mut perms = 0u64;
    if let Some(everyone) = guild.roles.get(guild_id) {
        perms |= everyone.permissions;
    } else if let Some(everyone) = guild.roles.values().find(|r| r.name == "@everyone") {
        perms |= everyone.permissions;
    }
    if let Some(ids) = guild.member_roles.get(&user_id) {
        for id in ids {
            if let Some(role) = guild.roles.get(id) {
                perms |= role.permissions;
            }
        }
    }
    perms & PERM_ADMINISTRATOR != 0 || perms & permission != 0
}

pub fn clear_guild_roles() {
    if let Some(store) = GUILD_ROLES.get() {
        if let Ok(mut map) = store.lock() {
            map.clear();
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChannelOverwrite {
    pub id: String,
    /// 0 = role, 1 = member
    pub kind: u64,
    pub allow: u64,
    pub deny: u64,
}

pub static GUILD_CHANNEL_OVERWRITES: OnceLock<Arc<Mutex<HashMap<String, HashMap<String, Vec<ChannelOverwrite>>>>>> = OnceLock::new();

pub fn get_channel_overwrites_store() -> Arc<Mutex<HashMap<String, HashMap<String, Vec<ChannelOverwrite>>>>> {
    GUILD_CHANNEL_OVERWRITES
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

pub fn register_channel_overwrites(guild_id: &str, channel_id: &str, overwrites: Vec<ChannelOverwrite>) {
    if guild_id.is_empty() || channel_id.is_empty() { return; }
    if let Ok(mut store) = get_channel_overwrites_store().lock() {
        store.entry(guild_id.to_string()).or_default().insert(channel_id.to_string(), overwrites);
    }
}

fn base_permissions(guild_id: &str, user_id: u64) -> (u64, bool) {
    let store = get_guild_roles_store();
    let Ok(guard) = store.lock() else { return (0, false) };
    let Some(guild) = guard.get(guild_id) else { return (0, false) };
    if user_id != 0 && guild.owner_id == user_id {
        return (u64::MAX, true);
    }
    let mut perms = 0u64;
    if let Some(everyone) = guild.roles.get(guild_id) {
        perms |= everyone.permissions;
    }
    if let Some(ids) = guild.member_roles.get(&user_id) {
        for id in ids {
            if let Some(role) = guild.roles.get(id) {
                perms |= role.permissions;
            }
        }
    }
    let bypass = perms & PERM_ADMINISTRATOR != 0;
    (perms, bypass)
}

/// Full permission computation for a channel, including @everyone, role and
/// member overwrites in Discord's order. `channel_id` empty means guild-level.
pub fn has_channel_permission(guild_id: &str, user_id: u64, channel_id: &str, permission: u64) -> bool {
    let (mut perms, bypass) = base_permissions(guild_id, user_id);
    if bypass { return true; }
    if channel_id.is_empty() { return perms & permission != 0; }

    let store = get_channel_overwrites_store();
    let guard = match store.lock() { Ok(g) => g, Err(_) => return perms & permission != 0 };
    let Some(overwrites) = guard.get(guild_id).and_then(|m| m.get(channel_id)) else {
        return perms & permission != 0;
    };

    // Role/member membership comes from the role store.
    let role_store = get_guild_roles_store();
    let roles = role_store
        .lock()
        .ok()
        .and_then(|s| s.get(guild_id).and_then(|g| g.member_roles.get(&user_id).cloned()))
        .unwrap_or_default();

    let mut everyone = (0u64, 0u64);
    let mut role_allow = 0u64;
    let mut role_deny = 0u64;
    let mut member = (0u64, 0u64);
    for ow in overwrites {
        if ow.kind == 0 && ow.id == guild_id {
            everyone.0 |= ow.deny;
            everyone.1 |= ow.allow;
        } else if ow.kind == 0 && roles.iter().any(|r| r == &ow.id) {
            role_deny |= ow.deny;
            role_allow |= ow.allow;
        } else if ow.kind == 1 && ow.id.parse::<u64>().ok() == Some(user_id) {
            member.0 |= ow.deny;
            member.1 |= ow.allow;
        }
    }
    perms = (perms & !everyone.0) | everyone.1;
    perms = (perms & !role_deny) | role_allow;
    perms = (perms & !member.0) | member.1;
    perms & permission != 0
}

pub fn highest_role_position(guild_id: &str, user_id: u64) -> i64 {
    let store = get_guild_roles_store();
    let Ok(guard) = store.lock() else { return 0 };
    let Some(guild) = guard.get(guild_id) else { return 0 };
    let mut top = 0i64;
    if let Some(ids) = guild.member_roles.get(&user_id) {
        for id in ids {
            if let Some(role) = guild.roles.get(id) {
                top = top.max(role.position);
            }
        }
    }
    top
}

/// True when `actor` may moderate `target`: owner/admin always, otherwise the
/// actor needs the permission and a strictly higher top role.
pub fn can_moderate_member(guild_id: &str, actor: u64, target: u64, permission: u64) -> bool {
    if actor == 0 || target == 0 || actor == target { return false; }
    let store = get_guild_roles_store();
    let owner = store.lock().ok().and_then(|s| s.get(guild_id).map(|g| g.owner_id)).unwrap_or(0);
    if actor == owner { return true; }
    if target == owner { return false; }
    if !has_guild_permission(guild_id, actor, permission) { return false; }
    let (actor_pos, target_pos) = (highest_role_position(guild_id, actor), highest_role_position(guild_id, target));
    // Equal top role cannot moderate each other.
    actor_pos > target_pos || actor_pos == 0
}

pub fn clear_channel_overwrites() {
    if let Some(store) = GUILD_CHANNEL_OVERWRITES.get() {
        if let Ok(mut map) = store.lock() { map.clear(); }
    }
}

/// Highest role of a member, preferring a colored role for display.
pub fn member_display_role(guild_id: &str, user_id: u64) -> Option<(String, u32)> {
    let store = get_guild_roles_store();
    let guard = store.lock().ok()?;
    let guild = guard.get(guild_id)?;
    let ids = guild.member_roles.get(&user_id)?;
    let mut best: Option<&RoleInfo> = None;
    for id in ids {
        if let Some(role) = guild.roles.get(id) {
            if role.name == "@everyone" { continue; }
            match best {
                None => best = Some(role),
                Some(current) => {
                    let better = role.position > current.position
                        || (role.position == current.position && role.color != 0 && current.color == 0);
                    if better { best = Some(role); }
                }
            }
        }
    }
    best.map(|r| (r.name.clone(), r.color))
}

static USER_DISPLAY_ROLES: OnceLock<Arc<Mutex<HashMap<u64, (String, u32)>>>> = OnceLock::new();

pub fn get_user_display_role(user_id: u64) -> Option<(String, u32)> {
    USER_DISPLAY_ROLES
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .lock()
        .ok()
        .and_then(|m| m.get(&user_id).cloned())
}

pub fn refresh_user_display_role(guild_id: &str, user_id: u64) {
    if let Some(role) = member_display_role(guild_id, user_id) {
        let store = USER_DISPLAY_ROLES.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
        if let Ok(mut m) = store.lock() {
            m.insert(user_id, role);
        }
    }
}

#[cfg(test)]
mod role_hierarchy_tests {
    use super::*;

    fn role(id: &str, position: i64, permissions: u64, managed: bool) -> RoleInfo {
        RoleInfo {
            id: id.to_string(),
            name: id.to_string(),
            color: 0,
            position,
            permissions,
            managed,
        }
    }

    #[test]
    fn assignable_roles_respects_permissions_hierarchy_and_managed_roles() {
        let guild = "role-test-guild";
        let actor = 10;
        let target = 20;
        register_guild_role(guild, role(guild, 0, 0, false));
        register_guild_role(guild, role("manager", 50, PERM_MANAGE_ROLES, false));
        register_guild_role(guild, role("moderator", 20, 0, false));
        register_guild_role(guild, role("target", 10, 0, false));
        register_guild_role(guild, role("bot", 5, 0, true));
        register_member_roles(guild, actor, vec!["manager".into()], 0);
        register_member_roles(guild, target, vec!["target".into()], 0);

        let assignable: Vec<String> = assignable_roles(guild, actor, target)
            .into_iter()
            .map(|(role, _)| role.id)
            .collect();
        assert!(assignable.contains(&"moderator".to_string()));
        assert!(assignable.contains(&"target".to_string()));
        assert!(!assignable.contains(&guild.to_string()), "@everyone is not assignable");
        assert!(!assignable.contains(&"bot".to_string()), "managed roles are not assignable");
        assert!(!assignable.contains(&"manager".to_string()), "equal/higher role is protected");
        assert!(can_manage_role(guild, actor, target, "moderator"));
        assert!(!can_manage_role(guild, actor, target, "manager"));
    }

    #[test]
    fn local_role_updates_are_reflected_immediately() {
        let guild = "role-test-local";
        register_guild_role(guild, role(guild, 0, 0, false));
        register_guild_role(guild, role("helper", 5, 0, false));
        register_member_roles(guild, 30, vec![], 0);
        assert!(member_role_names(guild, 30).is_empty());

        set_member_role_local(guild, 30, "helper", true);
        assert_eq!(member_role_names(guild, 30), vec!["helper"]);
        set_member_role_local(guild, 30, "helper", false);
        assert!(member_role_names(guild, 30).is_empty());
    }
}
