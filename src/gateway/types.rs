#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
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

#[derive(Debug, Clone)]
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
    VoiceDisconnected,
    MessageCreated {
        id: String,
        channel_id: String,
        author: String,
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
        timestamp: String,
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
    SubscribeGuild {
        guild_id: String,
        channel_ids: Vec<String>,
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
    pub input_device: String,
    pub output_device: String,
}

impl Default for GlobalAudioConfig {
    fn default() -> Self {
        Self {
            vad_threshold: 0.05,
            input_device: String::new(),
            output_device: String::new(),
        }
    }
}

pub fn load_persisted_audio_config() -> GlobalAudioConfig {
    if let Ok(data) = std::fs::read_to_string(GLOBAL_AUDIO_CONFIG_FILE) {
        if let Ok(cfg) = serde_json::from_str::<GlobalAudioConfig>(&data) {
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

fn get_vad_threshold_atomic() -> &'static std::sync::atomic::AtomicU32 {
    VAD_THRESHOLD.get_or_init(|| {
        let cfg = load_persisted_audio_config();
        std::sync::atomic::AtomicU32::new(cfg.vad_threshold.clamp(0.0, 1.0).to_bits())
    })
}

pub fn set_is_connected_to_voice(val: bool) {
    IS_CONNECTED_TO_VOICE.store(val, Ordering::Relaxed);
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

pub fn get_guild_voice_states_store() -> Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>> {
    GUILD_VOICE_STATES.get_or_init(|| Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))).clone()
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

pub fn get_user_names_store() -> Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>> {
    USER_NAMES.get_or_init(|| Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))).clone()
}

pub fn register_user_name(user_id: u64, name: String) {
    if user_id > 0 && !name.is_empty() {
        if let Ok(mut map) = get_user_names_store().lock() {
            map.insert(user_id, name);
        }
    }
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

