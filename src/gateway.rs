use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{SinkExt, StreamExt};
use log::{info, error, warn};
use tokio::net::UdpSocket;
use std::collections::VecDeque;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce
};
use opus_rs::{OpusEncoder, OpusDecoder, Application};
use davey::{DaveSession, ProposalsOperationType, MediaType};
use std::num::NonZeroU16;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ChannelData {
    pub id: String,
    pub name: String,
    pub is_voice: bool,
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

use std::sync::atomic::{AtomicU64, Ordering};

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

    if let Ok(parts_map) = get_active_voice_participants_store().lock() {
        for (&ssrc, &uid) in parts_map.iter() {
            if ssrc != 999999 && uid != 999999 && uid > 0 {
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

pub fn clean_discord_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            // Em space, En space, Non-breaking space, tabs, ideographic space -> single regular space
            '\u{00A0}' | '\u{2000}' | '\u{2001}' | '\u{2002}' | '\u{2003}' |
            '\u{2004}' | '\u{2005}' | '\u{2006}' | '\u{2007}' | '\u{2008}' |
            '\u{2009}' | '\u{200A}' | '\u{3000}' | '\t' => {
                out.push(' ');
            }
            // Zero-width spaces, soft hyphens -> strip
            '\u{200B}' | '\u{FEFF}' | '\u{200C}' | '\u{200D}' | '\u{200E}' | '\u{200F}' | '\u{00AD}' => {
                // skip
            }
            _ => {
                out.push(c);
            }
        }
    }
    out
}

/// Converts raw Discord Markdown syntax into clean readable plain text.
/// Applied in the correct order so that code blocks are parsed first (immune
/// to inner formatting) and inline formatting runs last.
pub fn parse_discord_markdown(input: &str) -> String {
    let cleaned_input = clean_discord_whitespace(input);
    let mut output_lines: Vec<String> = Vec::new();

    // ── Step 1: Protect code blocks (```) from inner parsing ──────────────
    // Split by triple-backtick blocks, mark odd-indexed segments as code.
    let triple_parts: Vec<&str> = cleaned_input.split("```").collect();
    let mut lines_to_process: Vec<(String, bool)> = Vec::new(); // (text, is_code_block)

    for (idx, part) in triple_parts.iter().enumerate() {
        if idx % 2 == 1 {
            // Inside a code block
            // First "word" of the block may be a language hint (e.g. "python\ncode")
            let trimmed = part.trim_start_matches('\n');
            let (lang, code_body) = if let Some(nl) = trimmed.find('\n') {
                let potential_lang = &trimmed[..nl];
                // Language hints are short alphanumeric words
                if potential_lang.len() <= 20 && potential_lang.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '-' || c == '_') {
                    (potential_lang, &trimmed[nl+1..])
                } else {
                    ("", trimmed)
                }
            } else {
                ("", trimmed)
            };
            if lang.is_empty() {
                lines_to_process.push((format!("[Bloco de Código]\n{}", code_body.trim_end()), true));
            } else {
                lines_to_process.push((format!("[Bloco de Código ({})]\n{}", lang, code_body.trim_end()), true));
            }
        } else {
            for line in part.split('\n') {
                lines_to_process.push((line.to_string(), false));
            }
        }
    }

    // ── Step 2: Process each line ──────────────────────────────────────────
    for (text, is_code) in lines_to_process {
        if is_code {
            output_lines.push(text);
            continue;
        }

        let raw_line = text.as_str();
        let line = raw_line.trim_start();
        if line.is_empty() {
            output_lines.push(String::new());
            continue;
        }

        // Block-level: Headers (must be at start of line)
        if let Some(rest) = line.strip_prefix("### ") {
            output_lines.push(format!("▪ {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            output_lines.push(format!("▌ {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            output_lines.push(format!("▌ {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        // Subtext
        if let Some(rest) = line.strip_prefix("-# ") {
            output_lines.push(apply_inline_markdown(rest.trim_start()));
            continue;
        }
        // Blockquote multi-line >>>
        if let Some(rest) = line.strip_prefix(">>> ") {
            for bq_line in rest.lines() {
                output_lines.push(format!("│ {}", apply_inline_markdown(bq_line.trim_start())));
            }
            continue;
        }
        // Blockquote single line >
        if let Some(rest) = line.strip_prefix("> ") {
            output_lines.push(format!("│ {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        // Unordered list (- or * at start)
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            output_lines.push(format!("• {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        // Ordered list (number followed by ". ")
        {
            let mut chars = line.chars().peekable();
            let mut digits = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() { digits.push(c); chars.next(); } else { break; }
            }
            if !digits.is_empty() && line[digits.len()..].starts_with(". ") {
                let rest = &line[digits.len() + 2..];
                output_lines.push(format!("{}. {}", digits, apply_inline_markdown(rest.trim_start())));
                continue;
            }
        }

        // Normal line — apply inline markdown
        output_lines.push(apply_inline_markdown(line));
    }

    output_lines.join("\n")
}

/// Applies all inline Discord Markdown transforms to a single line of text.
/// Code blocks have already been extracted, so this only handles inline syntax.
fn apply_inline_markdown(input: &str) -> String {
    let s = input.to_string();

    // ── Inline code (backtick) — protect from further parsing ─────────────
    // We replace inline code with a placeholder, process the rest, then restore.
    let backtick_re_parts: Vec<&str> = s.split('`').collect();
    let mut reconstructed = String::new();
    let mut code_segments: Vec<String> = Vec::new();

    for (idx, part) in backtick_re_parts.iter().enumerate() {
        if idx % 2 == 1 {
            // Inside inline code — protect it
            let placeholder = format!("\x00CODE{}\x00", code_segments.len());
            code_segments.push(format!("`{}`", part));
            reconstructed.push_str(&placeholder);
        } else {
            reconstructed.push_str(part);
        }
    }

    let mut s = reconstructed;

    // ── Markdown links [text](url) ─────────────────────────────────────────
    // Parse [label](url) → "label (url)"
    {
        let mut out = String::with_capacity(s.len());
        let mut rem = s.as_str();
        while let Some(bracket_start) = rem.find('[') {
            // Check it's not an escaped bracket
            out.push_str(&rem[..bracket_start]);
            let after_bracket = &rem[bracket_start + 1..];
            if let Some(bracket_end) = after_bracket.find(']') {
                let label = &after_bracket[..bracket_end];
                let after_label = &after_bracket[bracket_end + 1..];
                if after_label.starts_with('(') {
                    if let Some(paren_end) = after_label.find(')') {
                        let url = &after_label[1..paren_end];
                        if url.starts_with("http") {
                            if label.is_empty() {
                                out.push_str(url);
                            } else {
                                out.push_str(label);
                            }
                            rem = &after_label[paren_end + 1..];
                            continue;
                        }
                    }
                }
                // Not a valid link — emit literally
                out.push('[');
            } else {
                out.push('[');
            }
            rem = &rem[bracket_start + 1..];
        }
        out.push_str(rem);
        s = out;
    }

    // ── Spoilers ||text|| ──────────────────────────────────────────────────
    s = replace_between(&s, "||", "||", |_| "[SPOILER]".to_string());

    // ── Mentions and Discord entities ──────────────────────────────────────
    // <t:timestamp:format> — Discord timestamps
    s = regex_replace_simple(&s, "<t:", ">", |inner| {
        let parts: Vec<&str> = inner.splitn(2, ':').collect();
        let ts: i64 = parts[0].parse().unwrap_or(0);
        format!("[{}]", format_unix_timestamp(ts))
    });

    // <@!USER_ID> and <@USER_ID> — user mentions
    s = regex_replace_simple(&s, "<@!", ">", |_inner| "@usuário".to_string());
    s = regex_replace_simple(&s, "<@&", ">", |_inner| "@cargo".to_string());
    s = regex_replace_simple(&s, "<@", ">", |inner| {
        // Only if purely numeric (user ID)
        if inner.chars().all(|c| c.is_ascii_digit()) {
            "@usuário".to_string()
        } else {
            format!("<@{}>", inner)
        }
    });
    // <#CHANNEL_ID> — channel mentions
    s = regex_replace_simple(&s, "<#", ">", |_inner| "#canal".to_string());

    // <:name:ID> — custom emoji
    s = regex_replace_simple(&s, "<a:", ">", |inner| {
        let name = inner.split(':').next().unwrap_or("emoji");
        format!(":{}:", name)
    });
    s = regex_replace_simple(&s, "<:", ">", |inner| {
        let name = inner.split(':').next().unwrap_or("emoji");
        format!(":{}:", name)
    });

    // </name:ID> — command mentions
    s = regex_replace_simple(&s, "</", ">", |inner| {
        let name = inner.split(':').next().unwrap_or("comando");
        format!("/{}", name)
    });

    // ── Text formatting (order matters: most-specific first) ───────────────
    // Bold + Italic ***
    s = replace_between(&s, "***", "***", |inner| inner.to_string());
    // Bold **
    s = replace_between(&s, "**", "**", |inner| inner.to_string());
    // Italic * (single, but not at word boundaries where it would be a list)
    s = replace_between(&s, "*", "*", |inner| inner.to_string());
    // Underline __
    s = replace_between(&s, "__", "__", |inner| inner.to_string());
    // Italic _
    s = replace_between(&s, "_", "_", |inner| inner.to_string());
    // Strikethrough ~~
    s = replace_between(&s, "~~", "~~", |inner| inner.to_string());

    // ── Backslash escape removal ───────────────────────────────────────────
    let mut unescaped = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if matches!(next, '*' | '_' | '~' | '`' | '|' | '>' | '#' | '-' | '\\') {
                    unescaped.push(next);
                    chars.next();
                    continue;
                }
            }
        }
        unescaped.push(c);
    }
    s = unescaped;

    // ── HTML entities ──────────────────────────────────────────────────────
    s = s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"");

    // ── Restore inline code segments ──────────────────────────────────────
    for (idx, code) in code_segments.iter().enumerate() {
        let placeholder = format!("\x00CODE{}\x00", idx);
        s = s.replace(&placeholder, code);
    }

    s
}

/// Replaces all occurrences of text between `open` and `close` delimiters,
/// passing the inner content to `replacer`. Non-greedy (finds first closing).
fn replace_between<F>(input: &str, open: &str, close: &str, replacer: F) -> String
where
    F: Fn(&str) -> String,
{
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(start) = remaining.find(open) {
        let after_open = &remaining[start + open.len()..];
        if let Some(end) = after_open.find(close) {
            let inner = &after_open[..end];
            // Only replace if inner is non-empty
            if !inner.is_empty() {
                result.push_str(&remaining[..start]);
                result.push_str(&replacer(inner));
                remaining = &after_open[end + close.len()..];
                continue;
            }
        }
        // No matching close found — emit the open delimiter literally
        result.push_str(&remaining[..start + open.len()]);
        remaining = &remaining[start + open.len()..];
    }
    result.push_str(remaining);
    result
}

/// Replaces `prefix...suffix` patterns (single-pass, non-overlapping).
fn regex_replace_simple<F>(input: &str, prefix: &str, suffix: &str, replacer: F) -> String
where
    F: Fn(&str) -> String,
{
    replace_between(input, prefix, suffix, replacer)
}

/// Formats a Unix timestamp (seconds since epoch) into a human-readable string.
fn format_unix_timestamp(unix_secs: i64) -> String {
    // Simple calculation: days since 1970-01-01
    if unix_secs <= 0 {
        return "data desconhecida".to_string();
    }
    let secs_per_day = 86400i64;
    let total_days = unix_secs / secs_per_day;
    let time_secs = unix_secs % secs_per_day;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;

    // Compute year/month/day from days since epoch (Gregorian)
    let days_remaining = total_days + 719468; // offset to March 1, year 0
    let era = if days_remaining >= 0 { days_remaining } else { days_remaining - 146096 } / 146097;
    let doe = days_remaining - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:02}/{:02}/{} {:02}:{:02}", d, m, y, hours, minutes)
}

pub fn format_discord_author(m: &Value) -> String {
    let author_obj = &m["author"];
    let name = author_obj["global_name"].as_str()
        .unwrap_or_else(|| author_obj["username"].as_str().unwrap_or("Unknown"));
    let is_bot = author_obj["bot"].as_bool().unwrap_or(false);

    if is_bot {
        format!("{} [BOT]", name)
    } else {
        name.to_string()
    }
}

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

fn push_text_before_cmd(before: &str, line_blocks: &mut Vec<MessageBlockData>, lines: &mut Vec<MessageLineData>) {
    let trimmed = before.trim();
    if trimmed.is_empty() {
        return;
    }
    
    // If before is long (> 40 chars), find the best natural breaking point (punctuation or space)
    // so the text immediately preceding the command chip is short (< 35 chars) and stays on the same line with the chip!
    if trimmed.chars().count() > 40 {
        let chars: Vec<char> = trimmed.chars().collect();
        let total = chars.len();
        
        let mut split_idx = None;
        for i in (15..total.saturating_sub(12)).rev() {
            let c = chars[i];
            if c == '.' || c == '!' || c == '?' || c == ':' || c == ';' || c == ',' {
                split_idx = Some(i + 1);
                break;
            }
        }
        if split_idx.is_none() {
            for i in (15..total.saturating_sub(12)).rev() {
                if chars[i] == ' ' {
                    split_idx = Some(i);
                    break;
                }
            }
        }

        if let Some(idx) = split_idx {
            let p1: String = chars[..idx].iter().collect();
            let p2: String = chars[idx..].iter().collect();
            if !p1.trim().is_empty() {
                line_blocks.push(MessageBlockData {
                    text: parse_discord_markdown(p1.trim()),
                    is_link: false,
                    is_command: false,
                    is_emoji: false,
                    emoji_id: String::new(),
                    url: String::new(),
                    command_name: String::new(),
                });
                lines.push(MessageLineData { blocks: std::mem::take(line_blocks) });
            }
            let p2_clean = p2.trim_start();
            if !p2_clean.is_empty() {
                line_blocks.push(MessageBlockData {
                    text: parse_discord_markdown(p2_clean),
                    is_link: false,
                    is_command: false,
                    is_emoji: false,
                    emoji_id: String::new(),
                    url: String::new(),
                    command_name: String::new(),
                });
            }
            return;
        }
    }

    line_blocks.push(MessageBlockData {
        text: parse_discord_markdown(before),
        is_link: false,
        is_command: false,
        is_emoji: false,
        emoji_id: String::new(),
        url: String::new(),
        command_name: String::new(),
    });
}

pub fn parse_text_into_lines(input: &str, links: &mut Vec<LinkData>) -> Vec<MessageLineData> {
    let mut lines = Vec::new();
    if input.is_empty() {
        return lines;
    }

    let cleaned = clean_discord_whitespace(input);

    for raw_line in cleaned.split('\n') {
        let trimmed_line = raw_line.trim_start();
        if trimmed_line.is_empty() {
            continue;
        }
        let mut line_blocks = Vec::new();
        let mut rem = trimmed_line;

        while !rem.is_empty() {
            let next_bracket = rem.find('[');
            let next_cmd = rem.find("</");
            let next_emoji = match (rem.find("<:"), rem.find("<a:")) {
                (Some(s), Some(a)) => Some((s.min(a), if s < a { "<:" } else { "<a:" })),
                (Some(s), None) => Some((s, "<:")),
                (None, Some(a)) => Some((a, "<a:")),
                (None, None) => None,
            };
            let next_http = rem.find("http://");
            let next_https = rem.find("https://");

            let next_raw_url = match (next_http, next_https) {
                (Some(h), Some(hs)) => Some(h.min(hs)),
                (Some(h), None) => Some(h),
                (None, Some(hs)) => Some(hs),
                (None, None) => None,
            };

            let mut candidates: Vec<(usize, &str)> = Vec::new();
            if let Some(idx) = next_bracket { candidates.push((idx, "bracket")); }
            if let Some(idx) = next_cmd { candidates.push((idx, "cmd")); }
            if let Some((idx, _)) = next_emoji { candidates.push((idx, "emoji")); }
            if let Some(idx) = next_raw_url { candidates.push((idx, "raw_url")); }

            if candidates.is_empty() {
                let parsed = parse_discord_markdown(rem);
                if !parsed.is_empty() {
                    line_blocks.push(MessageBlockData {
                        text: parsed,
                        is_link: false,
                        is_command: false,
                        is_emoji: false,
                        emoji_id: String::new(),
                        url: String::new(),
                        command_name: String::new(),
                    });
                }
                break;
            }

            candidates.sort_by_key(|c| c.0);
            let (first_idx, first_type) = candidates[0];

            match first_type {
                "bracket" => {
                    let before = &rem[..first_idx];
                    let after_bracket = &rem[first_idx + 1..];
                    if let Some(bracket_end) = after_bracket.find(']') {
                        let label = &after_bracket[..bracket_end];
                        let after_label = &after_bracket[bracket_end + 1..];
                        if after_label.starts_with('(') {
                            if let Some(paren_end) = after_label.find(')') {
                                let url = &after_label[1..paren_end];
                                if url.starts_with("http") {
                                    if !before.is_empty() {
                                        line_blocks.push(MessageBlockData {
                                            text: parse_discord_markdown(before),
                                            is_link: false,
                                            is_command: false,
                                            is_emoji: false,
                                            emoji_id: String::new(),
                                            url: String::new(),
                                            command_name: String::new(),
                                        });
                                    }
                                    let display_label = if label.is_empty() { url.to_string() } else { parse_discord_markdown(label) };
                                    line_blocks.push(MessageBlockData {
                                        text: display_label.clone(),
                                        is_link: true,
                                        is_command: false,
                                        is_emoji: false,
                                        emoji_id: String::new(),
                                        url: url.to_string(),
                                        command_name: String::new(),
                                    });
                                    if !links.iter().any(|l| l.url == url) {
                                        links.push(LinkData {
                                            label: display_label,
                                            url: url.to_string(),
                                        });
                                    }
                                    rem = &after_label[paren_end + 1..];
                                    continue;
                                }
                            }
                        }
                    }
                    let slice_len = first_idx + 1;
                    line_blocks.push(MessageBlockData {
                        text: parse_discord_markdown(&rem[..slice_len]),
                        is_link: false,
                        is_command: false,
                        is_emoji: false,
                        emoji_id: String::new(),
                        url: String::new(),
                        command_name: String::new(),
                    });
                    rem = &rem[slice_len..];
                }
                "cmd" => {
                    let before = &rem[..first_idx];
                    let after_tag = &rem[first_idx + 2..];
                    if let Some(gt_idx) = after_tag.find('>') {
                        let inside = &after_tag[..gt_idx];
                        let (cmd_name, _cmd_id) = if let Some(colon) = inside.find(':') {
                            (&inside[..colon], &inside[colon + 1..])
                        } else {
                            (inside, "")
                        };
                        let clean_cmd = cmd_name.trim().trim_start_matches('/');
                        if !clean_cmd.is_empty() {
                            if !before.is_empty() {
                                push_text_before_cmd(before, &mut line_blocks, &mut lines);
                            }
                            line_blocks.push(MessageBlockData {
                                text: format!("/{}", clean_cmd),
                                is_link: false,
                                is_command: true,
                                is_emoji: false,
                                emoji_id: String::new(),
                                url: String::new(),
                                command_name: format!("/{}", clean_cmd),
                            });
                            rem = &after_tag[gt_idx + 1..];
                            continue;
                        }
                    }
                    let slice_len = first_idx + 2;
                    line_blocks.push(MessageBlockData {
                        text: parse_discord_markdown(&rem[..slice_len]),
                        is_link: false,
                        is_command: false,
                        is_emoji: false,
                        emoji_id: String::new(),
                        url: String::new(),
                        command_name: String::new(),
                    });
                    rem = &rem[slice_len..];
                }
                "emoji" => {
                    let before = &rem[..first_idx];
                    let tag_len = if rem[first_idx..].starts_with("<a:") { 3 } else { 2 };
                    let after_tag = &rem[first_idx + tag_len..];
                    if let Some(gt_idx) = after_tag.find('>') {
                        let inside = &after_tag[..gt_idx];
                        if let Some(colon) = inside.find(':') {
                            let emoji_name = &inside[..colon];
                            let emoji_id = &inside[colon + 1..];
                            if !emoji_id.is_empty() && emoji_id.chars().all(|c| c.is_ascii_digit()) {
                                if !before.is_empty() {
                                    line_blocks.push(MessageBlockData {
                                        text: parse_discord_markdown(before),
                                        is_link: false,
                                        is_command: false,
                                        is_emoji: false,
                                        emoji_id: String::new(),
                                        url: String::new(),
                                        command_name: String::new(),
                                    });
                                }
                                line_blocks.push(MessageBlockData {
                                    text: format!(":{}:", emoji_name),
                                    is_link: false,
                                    is_command: false,
                                    is_emoji: true,
                                    emoji_id: emoji_id.to_string(),
                                    url: String::new(),
                                    command_name: String::new(),
                                });
                                rem = &after_tag[gt_idx + 1..];
                                continue;
                            }
                        }
                    }
                    let slice_len = first_idx + tag_len;
                    line_blocks.push(MessageBlockData {
                        text: parse_discord_markdown(&rem[..slice_len]),
                        is_link: false,
                        is_command: false,
                        is_emoji: false,
                        emoji_id: String::new(),
                        url: String::new(),
                        command_name: String::new(),
                    });
                    rem = &rem[slice_len..];
                }
                "raw_url" => {
                    let before = &rem[..first_idx];
                    let url_candidate = &rem[first_idx..];
                    
                    let end_pos = url_candidate.find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == ')' || c == '"' || c == '\'')
                        .unwrap_or(url_candidate.len());
                    
                    let mut raw_url = &url_candidate[..end_pos];
                    while raw_url.ends_with('.') || raw_url.ends_with(',') || raw_url.ends_with(';') {
                        raw_url = &raw_url[..raw_url.len() - 1];
                    }

                    if raw_url.len() > 8 {
                        if !before.is_empty() {
                            line_blocks.push(MessageBlockData {
                                text: parse_discord_markdown(before),
                                is_link: false,
                                is_command: false,
                                is_emoji: false,
                                emoji_id: String::new(),
                                url: String::new(),
                                command_name: String::new(),
                            });
                        }
                        line_blocks.push(MessageBlockData {
                            text: raw_url.to_string(),
                            is_link: true,
                            is_command: false,
                            is_emoji: false,
                            emoji_id: String::new(),
                            url: raw_url.to_string(),
                            command_name: String::new(),
                        });
                        if !links.iter().any(|l| l.url == raw_url) {
                            links.push(LinkData {
                                label: raw_url.to_string(),
                                url: raw_url.to_string(),
                            });
                        }
                        rem = &rem[first_idx + raw_url.len()..];
                    } else {
                        let slice_len = first_idx + 4;
                        line_blocks.push(MessageBlockData {
                            text: parse_discord_markdown(&rem[..slice_len]),
                            is_link: false,
                            is_command: false,
                            is_emoji: false,
                            emoji_id: String::new(),
                            url: String::new(),
                            command_name: String::new(),
                        });
                        rem = &rem[slice_len..];
                    }
                }
                _ => break,
            }
        }

        if !line_blocks.is_empty() {
            lines.push(MessageLineData { blocks: line_blocks });
        }
    }

    lines
}

pub fn format_decimal_color(color_val: &Value) -> String {
    if let Some(dec) = color_val.as_u64() {
        let r = (dec >> 16) & 0xFF;
        let g = (dec >> 8) & 0xFF;
        let b = dec & 0xFF;
        format!("#{:02X}{:02X}{:02X}", r, g, b)
    } else {
        "#5865f2".to_string() // Default blurple
    }
}

pub fn extract_and_clean_links(input: &str, links: &mut Vec<LinkData>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rem = input;
    while let Some(bracket_start) = rem.find('[') {
        out.push_str(&rem[..bracket_start]);
        let after_bracket = &rem[bracket_start + 1..];
        if let Some(bracket_end) = after_bracket.find(']') {
            let label = &after_bracket[..bracket_end];
            let after_label = &after_bracket[bracket_end + 1..];
            if after_label.starts_with('(') {
                if let Some(paren_end) = after_label.find(')') {
                    let url = &after_label[1..paren_end];
                    if url.starts_with("http") {
                        let link_label = if label.is_empty() { url.to_string() } else { label.to_string() };
                        if !links.iter().any(|l| l.url == url) {
                            links.push(LinkData {
                                label: link_label.clone(),
                                url: url.to_string(),
                            });
                        }
                        if label.is_empty() {
                            out.push_str(url);
                        } else {
                            out.push_str(&format!("🔗 {}", label));
                        }
                        rem = &after_label[paren_end + 1..];
                        continue;
                    }
                }
            }
            out.push('[');
        } else {
            out.push('[');
        }
        rem = &rem[bracket_start + 1..];
    }
    out.push_str(rem);

    // Also extract standalone raw URLs (http:// or https://) into links if not already present
    for word in out.split_whitespace() {
        let clean_word = word.trim_matches(|c| c == '<' || c == '>' || c == '(' || c == ')' || c == '"' || c == '\'' || c == '[' || c == ']');
        if (clean_word.starts_with("http://") || clean_word.starts_with("https://")) && clean_word.len() > 8 {
            if !links.iter().any(|l| l.url == clean_word) {
                links.push(LinkData {
                    label: clean_word.to_string(),
                    url: clean_word.to_string(),
                });
            }
        }
    }

    // Clean </name:id> to </name> for clean text representation
    while let Some(tag_start) = out.find("</") {
        let after = &out[tag_start + 2..];
        if let Some(tag_end) = after.find('>') {
            let inside = &after[..tag_end];
            if inside.contains(':') {
                let name = if let Some(colon) = inside.find(':') {
                    &inside[..colon]
                } else {
                    inside
                };
                let mut new_out = String::with_capacity(out.len());
                new_out.push_str(&out[..tag_start]);
                new_out.push_str(&format!("</{}>", name.trim()));
                new_out.push_str(&after[tag_end + 1..]);
                out = new_out;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    out
}

fn extract_triple_backtick_code_blocks(input: &str) -> (String, String) {
    let parts: Vec<&str> = input.split("```").collect();
    if parts.len() < 3 {
        return (input.to_string(), String::new());
    }

    let mut text_parts = Vec::new();
    let mut code_parts = Vec::new();

    for (idx, part) in parts.iter().enumerate() {
        if idx % 2 == 1 {
            // Inside code block
            let trimmed = part.trim_start_matches('\n');
            let (_, code_body) = if let Some(nl) = trimmed.find('\n') {
                let potential_lang = &trimmed[..nl];
                if potential_lang.len() <= 20 && potential_lang.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '-' || c == '_') {
                    (potential_lang, &trimmed[nl+1..])
                } else {
                    ("", trimmed)
                }
            } else {
                ("", trimmed)
            };
            code_parts.push(code_body.trim_end().to_string());
        } else {
            text_parts.push(*part);
        }
    }

    (text_parts.join(""), code_parts.join("\n\n"))
}

/// Returns (content, embed_content, embed_color, embed_footer, code_block, reply_author, reply_content, links) as separate components.
/// `content` has regular text, replies, interactions, stickers, components.
/// `embed_content` has all embed text (title, desc, fields etc.).
/// `embed_footer` has footer text.
/// `code_block` has extracted monospaced code blocks.
pub fn extract_commands_from_text(text: &str, commands: &mut Vec<String>) {
    let t = text.trim();
    if t.is_empty() { return; }

    // ONLY extract real Discord command mentions e.g. </name:id> or </name>
    let mut rem = t;
    while let Some(pos) = rem.find("</") {
        let after = &rem[pos + 2..];
        if let Some(end) = after.find('>') {
            let inside = &after[..end];
            let name = if let Some(colon) = inside.find(':') {
                &inside[..colon]
            } else {
                inside
            };
            let clean = name.trim().trim_start_matches('/');
            if !clean.is_empty() && clean.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                let cmd_str = format!("/{}", clean);
                if !commands.contains(&cmd_str) {
                    commands.push(cmd_str);
                }
            }
            rem = &after[end + 1..];
        } else {
            break;
        }
    }
}

pub fn format_discord_message_parts(m: &Value) -> (String, Vec<String>, Vec<MessageLineData>, String, Vec<MessageLineData>, String, String, String, String, String, String, Vec<LinkData>, Vec<MessageButtonData>, Vec<MessageAttachmentData>) {
    let mut content_parts: Vec<String> = Vec::new();
    let mut commands: Vec<String> = Vec::new();
    let mut content_lines: Vec<MessageLineData> = Vec::new();
    let mut embed_parts_all: Vec<String> = Vec::new();
    let mut embed_lines: Vec<MessageLineData> = Vec::new();
    let mut links: Vec<LinkData> = Vec::new();
    let mut buttons: Vec<MessageButtonData> = Vec::new();
    let mut attachments_list: Vec<MessageAttachmentData> = Vec::new();
    let mut embed_footer = String::new();
    let mut reply_author = String::new();
    let mut reply_content = String::new();
    let mut reply_command = String::new();
    let msg_type = m["type"].as_u64().unwrap_or(0);

    // Extract embed color (first embed's color if present)
    let embed_color = m["embeds"].as_array()
        .and_then(|arr| arr.first())
        .and_then(|e| e.get("color"))
        .map(|c| format_decimal_color(c))
        .unwrap_or_else(|| "#5865f2".to_string());

    // Handle reply context (type 19)
    if msg_type == 19 {
        if let Some(ref_msg) = m["referenced_message"].as_object() {
            let ref_author = ref_msg.get("author")
                .and_then(|a| a["global_name"].as_str()
                    .or_else(|| a["username"].as_str()))
                .unwrap_or("alguem");
            let ref_content = ref_msg.get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let preview = if ref_content.chars().count() > 60 {
                format!("{}...", ref_content.chars().take(57).collect::<String>())
            } else if ref_content.is_empty() {
                "[midia/embed]".to_string()
            } else {
                ref_content.to_string()
            };
            reply_author = ref_author.to_string();
            reply_content = parse_discord_markdown(&preview);
        }
    }

    // Handle interaction context (type 20 = slash command, 23 = context menu, or message with interaction)
    if let Some(interaction) = m["interaction"].as_object()
        .or_else(|| m["interaction_metadata"].as_object()) {
        let cmd_name = interaction.get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("comando");
        let user_name = interaction.get("user")
            .and_then(|u| u["global_name"].as_str()
                .or_else(|| u["username"].as_str()))
            .unwrap_or("");
        let cmd_str = format!("/{}", cmd_name);
        if !commands.contains(&cmd_str) {
            commands.push(cmd_str.clone());
        }
        if !user_name.is_empty() {
            reply_author = user_name.to_string();
        }
        reply_command = cmd_str;
    }

    // 1. Raw Text Content — parsed into lines and regular text
    let mut code_block = String::new();
    if let Some(content) = m["content"].as_str() {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            extract_commands_from_text(trimmed, &mut commands);

            let (cleaned_text, extracted_code) = extract_triple_backtick_code_blocks(trimmed);
            if !extracted_code.is_empty() {
                code_block = extracted_code;
            }
            if !cleaned_text.trim().is_empty() {
                let parsed_lines = parse_text_into_lines(&cleaned_text, &mut links);
                if parsed_lines.iter().any(|l| l.blocks.iter().any(|b| b.is_link || b.is_command || b.is_emoji)) {
                    content_lines = parsed_lines;
                }
                let cleaned = extract_and_clean_links(&cleaned_text, &mut links);
                content_parts.push(parse_discord_markdown(&cleaned));
            }
        }
    }

    // 2. Embeds → go into embed_content & embed_lines
    if let Some(embeds) = m["embeds"].as_array() {
        for embed in embeds {
            let mut ep: Vec<String> = Vec::new();

            // Embed author
            if let Some(author_name) = embed["author"]["name"].as_str() {
                let a = author_name.trim();
                if !a.is_empty() {
                    extract_commands_from_text(a, &mut commands);
                    let cleaned = extract_and_clean_links(a, &mut links);
                    ep.push(format!("**{}**", parse_discord_markdown(&cleaned)));
                }
            }

            // Embed title (with optional URL)
            if let Some(title) = embed["title"].as_str() {
                let t = title.trim();
                if !t.is_empty() {
                    extract_commands_from_text(t, &mut commands);
                    let cleaned_title = extract_and_clean_links(t, &mut links);
                    let parsed_title = parse_discord_markdown(&cleaned_title);
                    if let Some(url) = embed["url"].as_str() {
                        if !url.is_empty() {
                            embed_lines.push(MessageLineData {
                                blocks: vec![MessageBlockData {
                                    text: parsed_title.clone(),
                                    is_link: true,
                                    is_command: false,
                                    is_emoji: false,
                                    emoji_id: String::new(),
                                    url: url.to_string(),
                                    command_name: String::new(),
                                }],
                            });
                            if !links.iter().any(|l| l.url == url) {
                                links.push(LinkData {
                                    label: format!("Titulo: {}", parsed_title),
                                    url: url.to_string(),
                                });
                            }
                        } else {
                            embed_lines.extend(parse_text_into_lines(t, &mut links));
                        }
                    } else {
                        embed_lines.extend(parse_text_into_lines(t, &mut links));
                    }
                    ep.push(parsed_title);
                }
            }

            // Embed description
            if let Some(desc) = embed["description"].as_str() {
                let d = desc.trim();
                if !d.is_empty() {
                    extract_commands_from_text(d, &mut commands);
                    embed_lines.extend(parse_text_into_lines(d, &mut links));
                    let cleaned = extract_and_clean_links(d, &mut links);
                    ep.push(parse_discord_markdown(&cleaned));
                }
            }

            // Embed fields (e.g. Duration, Artist, Track Number)
            if let Some(fields) = embed["fields"].as_array() {
                for field in fields {
                    let name = field["name"].as_str().unwrap_or("").trim();
                    let val  = field["value"].as_str().unwrap_or("").trim();
                    if !name.is_empty() && !val.is_empty() {
                        extract_commands_from_text(name, &mut commands);
                        extract_commands_from_text(val, &mut commands);

                        let field_line = format!("{}: {}", name, val);
                        embed_lines.extend(parse_text_into_lines(&field_line, &mut links));

                        let cleaned_name = extract_and_clean_links(name, &mut links);
                        let cleaned_val = extract_and_clean_links(val, &mut links);
                        ep.push(format!("{}: {}",
                            parse_discord_markdown(&cleaned_name),
                            parse_discord_markdown(&cleaned_val)));
                    } else if !val.is_empty() {
                        extract_commands_from_text(val, &mut commands);
                        embed_lines.extend(parse_text_into_lines(val, &mut links));
                        let cleaned_val = extract_and_clean_links(val, &mut links);
                        ep.push(parse_discord_markdown(&cleaned_val));
                    } else if !name.is_empty() {
                        extract_commands_from_text(name, &mut commands);
                        embed_lines.extend(parse_text_into_lines(name, &mut links));
                        let cleaned_name = extract_and_clean_links(name, &mut links);
                        ep.push(parse_discord_markdown(&cleaned_name));
                    }
                }
            }

            // Embed image
            if let Some(img_url) = embed["image"]["url"].as_str() {
                if !img_url.is_empty() {
                    links.push(LinkData {
                        label: "Ver Imagem do Embed".to_string(),
                        url: img_url.to_string(),
                    });
                }
            }

            // Embed thumbnail
            if let Some(thumb_url) = embed["thumbnail"]["url"].as_str() {
                if !thumb_url.is_empty() {
                    links.push(LinkData {
                        label: "Ver Miniatura do Embed".to_string(),
                        url: thumb_url.to_string(),
                    });
                }
            }

            // Embed timestamp
            if let Some(ts) = embed["timestamp"].as_str() {
                if !ts.is_empty() {
                    let display = if ts.chars().count() >= 10 { ts.chars().take(10).collect::<String>() } else { ts.to_string() };
                    ep.push(format!("[{}]", display));
                }
            }

            // Embed footer (accumulate footers)
            if let Some(footer) = embed["footer"]["text"].as_str() {
                let f = footer.trim();
                if !f.is_empty() {
                    extract_commands_from_text(f, &mut commands);
                    let cleaned = extract_and_clean_links(f, &mut links);
                    let footer_parsed = parse_discord_markdown(&cleaned);
                    if embed_footer.is_empty() {
                        embed_footer = footer_parsed;
                    } else {
                        embed_footer = format!("{}\n{}", embed_footer, footer_parsed);
                    }
                }
            }

            if !ep.is_empty() {
                embed_parts_all.push(ep.join("\n"));
            }
        }
    }

    // 3. Attachments → content & clickable links
    if let Some(attachments) = m["attachments"].as_array() {
        for att in attachments {
            let id = att["id"].as_str().unwrap_or("").to_string();
            let filename = att["filename"].as_str().unwrap_or("arquivo").to_string();
            let url = att["url"].as_str().unwrap_or("").to_string();
            let proxy_url = att["proxy_url"].as_str().unwrap_or(&url).to_string();
            let content_type = att["content_type"].as_str().unwrap_or("").to_string();
            let size_bytes = att["size"].as_u64().unwrap_or(0);
            let width = att["width"].as_i64().unwrap_or(0) as i32;
            let height = att["height"].as_i64().unwrap_or(0) as i32;

            let size_str = if size_bytes < 1024 {
                format!("{} B", size_bytes)
            } else if size_bytes < 1024 * 1024 {
                format!("{:.1} KB", size_bytes as f64 / 1024.0)
            } else {
                format!("{:.1} MB", size_bytes as f64 / (1024.0 * 1024.0))
            };

            let is_image = content_type.starts_with("image/") 
                || filename.ends_with(".png") 
                || filename.ends_with(".jpg") 
                || filename.ends_with(".jpeg") 
                || filename.ends_with(".webp") 
                || filename.ends_with(".gif");

            if !is_image {
                let label = if content_type.starts_with("video/") {
                    "Video"
                } else if content_type.starts_with("audio/") {
                    "Audio"
                } else {
                    "Anexo"
                };
                content_parts.push(format!("[{}: {}]", label, filename));
                if !url.is_empty() {
                    links.push(LinkData {
                        label: format!("Abrir {}: {}", label, filename),
                        url: url.clone(),
                    });
                }
            }

            attachments_list.push(MessageAttachmentData {
                id,
                filename,
                url,
                proxy_url,
                size_bytes,
                size_str,
                width,
                height,
                content_type,
                is_image,
            });
        }
    }

    // 4. Stickers → content
    if let Some(stickers) = m["sticker_items"].as_array() {
        for st in stickers {
            let st_name = st["name"].as_str().unwrap_or("Sticker");
            content_parts.push(format!("[Sticker: {}]", st_name));
        }
    }

    // 5. Components V2 → Rich interactive buttons (ActionRows)
    if let Some(components) = m["components"].as_array() {
        for row in components {
            if let Some(row_components) = row["components"].as_array() {
                for comp in row_components {
                    let comp_type = comp["type"].as_u64().unwrap_or(0);
                    match comp_type {
                        2 => {
                            let label = comp["label"].as_str().unwrap_or("").trim();
                            let url = comp["url"].as_str().unwrap_or("").trim();
                            let style = comp["style"].as_i64().unwrap_or(if !url.is_empty() { 5 } else { 2 }) as i32;
                            let is_disabled = comp["disabled"].as_bool().unwrap_or(false);
                            
                            let mut emoji_str = String::new();
                            if let Some(emoji_obj) = comp.get("emoji") {
                                if let Some(emoji_name) = emoji_obj["name"].as_str() {
                                    emoji_str = emoji_name.to_string();
                                }
                            }

                            if !label.is_empty() || !emoji_str.is_empty() {
                                buttons.push(MessageButtonData {
                                    label: label.to_string(),
                                    url: url.to_string(),
                                    emoji: emoji_str,
                                    style_type: style,
                                    is_disabled,
                                });
                            }
                        }
                        3 | 5 | 6 | 7 | 8 => {
                            if let Some(placeholder) = comp["placeholder"].as_str() {
                                if !placeholder.is_empty() {
                                    buttons.push(MessageButtonData {
                                        label: placeholder.to_string(),
                                        url: String::new(),
                                        emoji: "📋".to_string(),
                                        style_type: 2,
                                        is_disabled: true,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Build content string
    let content = if content_parts.is_empty() && embed_parts_all.is_empty() && buttons.is_empty() && attachments_list.is_empty() {
        match msg_type {
            1 => "[Membro adicionado ao grupo]".to_string(),
            2 => "[Membro removido do grupo]".to_string(),
            3 => "[Chamada]".to_string(),
            4 => "[Nome do canal alterado]".to_string(),
            5 => "[Icone do canal alterado]".to_string(),
            6 => "[Mensagem Fixada]".to_string(),
            7 => "[Novo membro entrou no servidor!]".to_string(),
            8 => "[Servidor Impulsionado!]".to_string(),
            9 => "[Servidor alcancou nivel 1 de impulso!]".to_string(),
            10 => "[Servidor alcancou nivel 2 de impulso!]".to_string(),
            11 => "[Servidor alcancou nivel 3 de impulso!]".to_string(),
            12 => "[Canal seguido adicionado]".to_string(),
            14 => "[Servidor desqualificado da descoberta]".to_string(),
            15 => "[Servidor requalificado na descoberta]".to_string(),
            19 => String::new(),
            20 => String::new(),
            21 => "[Mensagem inicial de thread]".to_string(),
            22 => "[Lembrete de convite]".to_string(),
            23 => String::new(),
            24 => "[Acao do AutoMod]".to_string(),
            25 => "[Compra de assinatura de cargo]".to_string(),
            _ => "[Conteudo especial]".to_string(),
        }
    } else {
        content_parts.join("\n")
    };

    let embed_content = embed_parts_all.join("\n\n");

    (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments_list)
}

/// Legacy wrapper — returns combined content + embed as a single string.
#[allow(dead_code)]
pub fn format_discord_message(m: &Value) -> String {
    let (content, _, _, embed, _, _, _, _, _, _, _, _, _, _) = format_discord_message_parts(m);
    if embed.is_empty() {
        content
    } else if content.is_empty() {
        embed
    } else {
        format!("{}\n{}", content, embed)
    }
}

pub struct GatewayClient {
    token: String,
    event_tx: mpsc::Sender<GatewayEvent>,
    user_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_session_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_token: Arc<std::sync::Mutex<Option<String>>>,
    voice_endpoint: Arc<std::sync::Mutex<Option<String>>>,
    voice_guild_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_channel_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_self_mute: Arc<std::sync::Mutex<bool>>,
}

impl GatewayClient {
    pub fn new(raw_token: String, event_tx: mpsc::Sender<GatewayEvent>) -> Self {
        let mut token = raw_token.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();

        if token.to_lowercase().starts_with("authorization:") {
            token = token[14..].to_string();
        }
        if token.to_lowercase().starts_with("bearer ") {
            token = token[7..].to_string();
        }

        let prefix_len = token.len().min(12);
        info!("Token sanitizado (inÃ­cio): {}...", &token[..prefix_len]);

        Self {
            token,
            event_tx,
            user_id: Arc::new(std::sync::Mutex::new(None)),
            voice_session_id: Arc::new(std::sync::Mutex::new(None)),
            voice_token: Arc::new(std::sync::Mutex::new(None)),
            voice_endpoint: Arc::new(std::sync::Mutex::new(None)),
            voice_guild_id: Arc::new(std::sync::Mutex::new(None)),
            voice_channel_id: Arc::new(std::sync::Mutex::new(None)),
            voice_self_mute: Arc::new(std::sync::Mutex::new(false)),
        }
    }

    fn try_trigger_voice_connect(&self) {
        let user_id = self.user_id.lock().unwrap().clone();
        let voice_sid = self.voice_session_id.lock().unwrap().clone();
        let voice_tok = self.voice_token.lock().unwrap().clone();
        let voice_ep = self.voice_endpoint.lock().unwrap().clone();
        let voice_gid = self.voice_guild_id.lock().unwrap().clone().unwrap_or_default();
        let voice_cid = self.voice_channel_id.lock().unwrap().clone().unwrap_or_default();

        info!("Status de disparo de voz: user_id={:?}, sid={:?}, token={:?}, ep={:?}, gid='{}', cid='{}'",
            user_id.is_some(), voice_sid.is_some(), voice_tok.is_some(), voice_ep.is_some(), voice_gid, voice_cid);

        if let (Some(uid), Some(sid), Some(tok), Some(ep)) = (user_id, voice_sid, voice_tok, voice_ep) {
            let effective_gid = if voice_gid.is_empty() { voice_cid.clone() } else { voice_gid };
            if !voice_cid.is_empty() || !effective_gid.is_empty() {
                info!("⚡ TODAS AS CREDENCIAIS DE VOZ PRONTAS! Conectando à Voice Gateway no endpoint {}...", ep);
                *self.voice_token.lock().unwrap() = None;
                *self.voice_session_id.lock().unwrap() = None;

                let self_mute_state = Arc::clone(&self.voice_self_mute);
                let event_tx_conn = self.event_tx.clone();
                tokio::spawn(async move {
                    connect_voice_gateway(&ep, &effective_gid, &uid, &sid, &tok, &voice_cid, self_mute_state, event_tx_conn).await;
                });
            }
        }
    }

    pub async fn start(self: Arc<Self>, mut cmd_rx: mpsc::Receiver<GatewayCommand>) {
        // Gateway v9 is required for User Account Tokens
        let url = "wss://gateway.discord.gg/?v=9&encoding=json";
        info!("Conectando à Gateway v9 do Discord...");

        match connect_async(url).await {
            Ok((ws_stream, _)) => {
                info!("Conexão WebSocket estabelecida com sucesso!");
                let (write, mut read) = ws_stream.split();
                let write_arc = Arc::new(Mutex::new(write));

                // Spawn GatewayCommand listener loop (OP 4 Voice State Update, etc)
                let write_cmd = Arc::clone(&write_arc);
                let client_cmd = Arc::clone(&self);
                tokio::spawn(async move {
                    while let Some(cmd) = cmd_rx.recv().await {
                        match cmd {
                            GatewayCommand::UpdateVoiceState { guild_id, channel_id, self_mute, self_deaf } => {
                                let channel_changed = {
                                    let cur_channel = client_cmd.voice_channel_id.lock().unwrap();
                                    *cur_channel != channel_id
                                };

                                *client_cmd.voice_self_mute.lock().unwrap() = self_mute;

                                if channel_changed {
                                    // Terminate any existing background UDP audio tasks
                                    CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst);

                                    // Reset voice token, session_id and endpoint buffers for clean new connection
                                    *client_cmd.voice_token.lock().unwrap() = None;
                                    *client_cmd.voice_session_id.lock().unwrap() = None;
                                    *client_cmd.voice_endpoint.lock().unwrap() = None;
                                    *client_cmd.voice_guild_id.lock().unwrap() = Some(guild_id.clone());
                                    *client_cmd.voice_channel_id.lock().unwrap() = channel_id.clone();
                                }

                                let effective_gid_val = if guild_id.trim().is_empty() {
                                    serde_json::Value::Null
                                } else {
                                    serde_json::json!(guild_id)
                                };

                                let payload = serde_json::json!({
                                    "op": 4,
                                    "d": {
                                        "guild_id": effective_gid_val,
                                        "channel_id": channel_id,
                                        "self_mute": self_mute,
                                        "self_deaf": self_deaf
                                    }
                                });

                                 info!("Enviando OP 4 VoiceStateUpdate à Gateway: {}", payload);
                                let mut w = write_cmd.lock().await;
                                if let Err(e) = w.send(Message::Text(payload.to_string().into())).await {
                                    warn!("Falha ao enviar Opcode 4 (VoiceStateUpdate): {:?}", e);
                                }
                            }
                            GatewayCommand::SubscribeGuild { .. } => {
                                // Opcode 14 is not needed for user accounts; voice states and members are pre-loaded via GUILD_CREATE and READY_SUPPLEMENTAL
                            }
                        }
                    }
                });

                // Read incoming Gateway messages loop
                while let Some(msg_result) = read.next().await {
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                let op = value["op"].as_u64().unwrap_or(99);

                                match op {
                                    10 => {
                                        // Opcode 10: HELLO
                                        let heartbeat_interval = value["d"]["heartbeat_interval"]
                                            .as_u64()
                                            .unwrap_or(41250);

                                        info!("Heartbeat interval recebido: {} ms", heartbeat_interval);

                                        // Send initial Heartbeat (Opcode 1)
                                        let hb_initial = serde_json::json!({ "op": 1, "d": null });
                                        {
                                            let mut w = write_arc.lock().await;
                                            let _ = w.send(Message::Text(hb_initial.to_string().into())).await;
                                        }

                                        // Send Identify Payload (Opcode 2) for Discord User Tokens
                                        let identify = serde_json::json!({
                                            "op": 2,
                                            "d": {
                                                "token": self.token,
                                                "capabilities": 16381,
                                                "properties": {
                                                    "os": "Windows",
                                                    "browser": "Chrome",
                                                    "device": "",
                                                    "system_locale": "pt-BR",
                                                    "browser_user_agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/127.0.0.0 Safari/537.36",
                                                    "browser_version": "127.0.0.0",
                                                    "os_version": "10.0.19045",
                                                    "referrer": "",
                                                    "referring_domain": "",
                                                    "referrer_current": "",
                                                    "referring_domain_current": "",
                                                    "release_channel": "stable",
                                                    "client_build_number": 320000,
                                                    "client_event_source": null
                                                },
                                                "presence": {
                                                    "status": "online",
                                                    "since": 0,
                                                    "activities": [],
                                                    "afk": false
                                                },
                                                "compress": false,
                                                "client_state": {
                                                    "guild_versions": {}
                                                }
                                            }
                                        });

                                        info!("Enviando payload IDENTIFY v9 para a Gateway...");
                                        {
                                            let mut w = write_arc.lock().await;
                                            if let Err(e) = w.send(Message::Text(identify.to_string().into())).await {
                                                error!("Erro ao enviar payload IDENTIFY: {:?}", e);
                                                return;
                                            }
                                        }

                                        // Spawn Heartbeat Loop
                                        let write_hb = Arc::clone(&write_arc);
                                        tokio::spawn(async move {
                                            loop {
                                                sleep(Duration::from_millis(heartbeat_interval)).await;
                                                let hb = serde_json::json!({ "op": 1, "d": null });
                                                let mut w = write_hb.lock().await;
                                                if let Err(e) = w.send(Message::Text(hb.to_string().into())).await {
                                                    warn!("Falha ao enviar Heartbeat: {:?}", e);
                                                    break;
                                                }
                                            }
                                        });
                                    }
                                    _ => {
                                        self.handle_event(&value).await;
                                    }
                                }
                            }
                        }
                        Ok(Message::Close(close_frame)) => {
                            warn!("Gateway fechou a conexão (Close Frame): {:?}", close_frame);
                            let reason = match close_frame {
                                Some(frame) => format!("Código {}: {}", frame.code, frame.reason),
                                None => "Conexão encerrada pelo servidor".to_string(),
                            };
                            let _ = self.event_tx.send(GatewayEvent::Disconnected { reason }).await;
                            break;
                        }
                        Err(e) => {
                            error!("Erro de leitura no WebSocket: {:?}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("Falha ao conectar na Gateway do Discord: {:?}", e);
                let _ = self.event_tx.send(GatewayEvent::Disconnected {
                    reason: format!("Erro de conexão: {}", e)
                }).await;
            }
        }
    }

    async fn handle_event(&self, v: &Value) {
        if let Some(t) = v["t"].as_str() {
            match t {
                "READY" => {
                    let uid = v["d"]["user"]["id"].as_str().unwrap_or("").to_string();
                    *self.user_id.lock().unwrap() = Some(uid.clone());
                    let uid_num = uid.parse::<u64>().unwrap_or(0);

                    let username = v["d"]["user"]["username"].as_str().unwrap_or("User");
                    let global_name = v["d"]["user"]["global_name"].as_str().unwrap_or(username);
                    if uid_num > 0 {
                        set_my_user_id(uid_num);
                        set_my_username(global_name.to_string());
                        register_user_name(uid_num, global_name.to_string());
                    }

                    // Parse all user/bot objects in READY payload
                    if let Some(users_arr) = v["d"]["users"].as_array() {
                        for u in users_arr {
                            let u_id: u64 = if let Some(s) = u["id"].as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                u["id"].as_u64().unwrap_or(0)
                            };
                            if u_id > 0 {
                                let mut dname = String::new();
                                if let Some(gname) = u["global_name"].as_str() {
                                    if !gname.is_empty() { dname = gname.to_string(); }
                                }
                                if dname.is_empty() {
                                    if let Some(uname) = u["username"].as_str() {
                                        if !uname.is_empty() { dname = uname.to_string(); }
                                    }
                                }
                                if !dname.is_empty() {
                                    register_user_name(u_id, dname);
                                }
                            }
                        }
                    }

                    // Parse initial voice states in READY payload
                    if let Some(guilds_arr) = v["d"]["guilds"].as_array() {
                        for g in guilds_arr {
                            if let Some(vs_arr) = g["voice_states"].as_array() {
                                for vs in vs_arr {
                                    let u_id: u64 = if let Some(s) = vs["user_id"].as_str() {
                                        s.parse().unwrap_or(0)
                                    } else {
                                        vs["user_id"].as_u64().unwrap_or(0)
                                    };
                                    let c_str = if let Some(s) = vs["channel_id"].as_str() {
                                        s.to_string()
                                    } else if let Some(n) = vs["channel_id"].as_u64() {
                                        n.to_string()
                                    } else {
                                        String::new()
                                    };
                                    if u_id > 0 && !c_str.is_empty() {
                                        if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                                            gvs.insert(u_id, c_str.clone());
                                            info!("📌 [READY] Pré-carregado estado de voz: User {} -> Canal {}", u_id, c_str);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let discriminator = v["d"]["user"]["discriminator"].as_str().unwrap_or("0");
                    let user_tag = if discriminator == "0" {
                        global_name.to_string()
                    } else {
                        format!("{}#{}", global_name, discriminator)
                    };
                    info!("Login BEM-SUCEDIDO na Gateway! Usuário: {} (@{})", global_name, username);
                    let _ = self.event_tx.send(GatewayEvent::Connected { user_tag }).await;
                }
                "READY_SUPPLEMENTAL" => {
                    info!("📌 Evento READY_SUPPLEMENTAL recebido da Gateway! Keys: {:?}", v["d"].as_object().map(|m| m.keys().collect::<Vec<_>>()));
                    if let Some(vs_arr) = v["d"]["voice_states"].as_array() {
                        info!("📌 [READY_SUPPLEMENTAL] Encontrados {} voice_states na raiz!", vs_arr.len());
                        for vs in vs_arr {
                            let u_id: u64 = if let Some(s) = vs["user_id"].as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                vs["user_id"].as_u64().unwrap_or(0)
                            };
                            let c_str = if let Some(s) = vs["channel_id"].as_str() {
                                s.to_string()
                            } else if let Some(n) = vs["channel_id"].as_u64() {
                                n.to_string()
                            } else {
                                String::new()
                            };
                            if u_id > 0 && !c_str.is_empty() {
                                if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                                    gvs.insert(u_id, c_str.clone());
                                    info!("📌 [READY_SUPPLEMENTAL] Estado de voz pré-carregado: User {} -> Canal {}", u_id, c_str);
                                }
                            }
                        }
                    }
                    if let Some(guilds_arr) = v["d"]["guilds"].as_array() {
                        info!("📌 [READY_SUPPLEMENTAL] Encontrados {} guilds no READY_SUPPLEMENTAL!", guilds_arr.len());
                        for g in guilds_arr {
                            if let Some(vs_arr) = g["voice_states"].as_array() {
                                info!("📌 [READY_SUPPLEMENTAL] Guild {} tem {} voice_states!", g["id"], vs_arr.len());
                                for vs in vs_arr {
                                    let u_id: u64 = if let Some(s) = vs["user_id"].as_str() {
                                        s.parse().unwrap_or(0)
                                    } else {
                                        vs["user_id"].as_u64().unwrap_or(0)
                                    };
                                    let c_str = if let Some(s) = vs["channel_id"].as_str() {
                                        s.to_string()
                                    } else if let Some(n) = vs["channel_id"].as_u64() {
                                        n.to_string()
                                    } else {
                                        String::new()
                                    };
                                    if u_id > 0 && !c_str.is_empty() {
                                        if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                                            gvs.insert(u_id, c_str.clone());
                                            info!("📌 [READY_SUPPLEMENTAL] Estado de voz pré-carregado: User {} -> Canal {}", u_id, c_str);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let _ = self.event_tx.send(GatewayEvent::VoiceStatesUpdated).await;
                }
                "GUILD_MEMBERS_CHUNK" => {
                    if let Some(members_arr) = v["d"]["members"].as_array() {
                        for m in members_arr {
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
                                    register_user_name(uid, display_name);
                                }
                            }
                        }
                    }
                }
                "VOICE_STATE_UPDATE" => {
                    let event_uid: u64 = if let Some(s) = v["d"]["user_id"].as_str() {
                        s.parse().unwrap_or(0)
                    } else {
                        v["d"]["user_id"].as_u64().unwrap_or(0)
                    };

                    if event_uid > 0 {
                        let mut display_name = String::new();
                        if let Some(nick) = v["d"]["member"]["nick"].as_str() {
                            if !nick.is_empty() { display_name = nick.to_string(); }
                        }
                        if display_name.is_empty() {
                            if let Some(gname) = v["d"]["member"]["user"]["global_name"].as_str() {
                                if !gname.is_empty() { display_name = gname.to_string(); }
                            }
                        }
                        if display_name.is_empty() {
                            if let Some(uname) = v["d"]["member"]["user"]["username"].as_str() {
                                if !uname.is_empty() { display_name = uname.to_string(); }
                            }
                        }

                        if !display_name.is_empty() {
                            register_user_name(event_uid, display_name.clone());
                            info!("VOICE_STATE_UPDATE: Registrado nome de usuário {} -> {}", event_uid, display_name);
                        }

                        let event_chan = if let Some(s) = v["d"]["channel_id"].as_str() {
                            Some(s.to_string())
                        } else if let Some(n) = v["d"]["channel_id"].as_u64() {
                            Some(n.to_string())
                        } else {
                            None
                        };

                        if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                            if let Some(ref cid) = event_chan {
                                gvs.insert(event_uid, cid.clone());
                                info!("📌 [VOICE_STATE_UPDATE] User {} -> Canal {}", event_uid, cid);
                            } else {
                                gvs.remove(&event_uid);
                            }
                        }
                        let my_voice_chan = self.voice_channel_id.lock().unwrap().clone();

                        if let Some(ref my_cid) = my_voice_chan {
                            if event_chan.as_deref() == Some(my_cid.as_str()) {
                                info!("👥 [VOICE CHANNEL JOIN] Usuário {} ({}) entrou no nosso canal de voz!", event_uid, display_name);
                                register_voice_participant(event_uid as u32, event_uid);
                            } else {
                                info!("👥 [VOICE CHANNEL LEAVE] Usuário {} saiu do nosso canal de voz!", event_uid);
                                remove_voice_participant_by_user_id(event_uid);
                            }
                        }

                        // Check if this is MY OWN voice state update
                        let my_uid = self.user_id.lock().unwrap().clone().unwrap_or_default();
                        let event_uid_str = event_uid.to_string();
                        if !my_uid.is_empty() && event_uid_str == my_uid {
                            if let Some(sid) = v["d"]["session_id"].as_str() {
                                info!("VOICE_STATE_UPDATE do meu usuário recebido! Session ID: {}, Channel: {:?}, Guild: {:?}", sid, event_chan, v["d"]["guild_id"].as_str());
                                if let Some(cid) = event_chan {
                                    *self.voice_session_id.lock().unwrap() = Some(sid.to_string());
                                    *self.voice_channel_id.lock().unwrap() = Some(cid.to_string());
                                    if let Some(gid) = v["d"]["guild_id"].as_str() {
                                        *self.voice_guild_id.lock().unwrap() = Some(gid.to_string());
                                    }
                                    self.try_trigger_voice_connect();
                                } else {
                                    // We left the voice channel or were displaced by another client
                                    info!("🚪 [VOICE_STATE_UPDATE] Desconectado da sala de voz (deslocado ou saiu)");
                                    *self.voice_session_id.lock().unwrap() = None;
                                    *self.voice_token.lock().unwrap() = None;
                                    *self.voice_endpoint.lock().unwrap() = None;
                                    *self.voice_channel_id.lock().unwrap() = None;
                                    clear_voice_participants();
                                    let _ = self.event_tx.send(GatewayEvent::VoiceDisconnected).await;
                                }
                            }
                        }
                        let _ = self.event_tx.send(GatewayEvent::VoiceStatesUpdated).await;
                    }
                }
                "VOICE_SERVER_UPDATE" => {
                    info!("VOICE_SERVER_UPDATE bruto recebido: {:?}", v["d"]);
                    let token = v["d"]["token"].as_str().unwrap_or("").to_string();
                    let guild_id = v["d"]["guild_id"].as_str()
                        .or_else(|| v["d"]["channel_id"].as_str())
                        .unwrap_or("")
                        .to_string();
                    let endpoint = v["d"]["endpoint"].as_str().unwrap_or("").to_string();

                    if !token.is_empty() && !endpoint.is_empty() {
                        info!("VOICE_SERVER_UPDATE processado com Sucesso! Endpoint: {}, Guild/Channel ID: {}", endpoint, guild_id);
                        *self.voice_token.lock().unwrap() = Some(token);
                        *self.voice_endpoint.lock().unwrap() = Some(endpoint);
                        if !guild_id.is_empty() {
                            *self.voice_guild_id.lock().unwrap() = Some(guild_id);
                        }
                        self.try_trigger_voice_connect();
                    } else {
                        warn!("VOICE_SERVER_UPDATE recebido com campos ausentes ou nulos: token_empty={}, endpoint_empty={}", token.is_empty(), endpoint.is_empty());
                    }
                }
                "MESSAGE_CREATE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let author = format_discord_author(&v["d"]);
                    let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(&v["d"]);
                    let timestamp = "Agora".to_string();

                    let _ = self.event_tx.send(GatewayEvent::MessageCreated {
                        id,
                        channel_id,
                        author,
                        content,
                        commands,
                        content_lines,
                        embed_content,
                        embed_lines,
                        embed_color,
                        embed_footer,
                        code_block,
                        reply_author,
                        reply_content,
                        reply_command,
                        links,
                        buttons,
                        attachments,
                        timestamp,
                    }).await;
                }
                "MESSAGE_UPDATE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(&v["d"]);

                    let _ = self.event_tx.send(GatewayEvent::MessageUpdated {
                        id,
                        channel_id,
                        content,
                        commands,
                        content_lines,
                        embed_content,
                        embed_lines,
                        embed_color,
                        embed_footer,
                        code_block,
                        reply_author,
                        reply_content,
                        reply_command,
                        links,
                        buttons,
                        attachments,
                    }).await;
                }
                "MESSAGE_DELETE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let _ = self.event_tx.send(GatewayEvent::MessageDeleted { id, channel_id }).await;
                }
                "GUILD_CREATE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let name = v["d"]["name"].as_str().unwrap_or("Servidor").to_string();
                    let mut channels = Vec::new();

                    if let Some(chans_arr) = v["d"]["channels"].as_array() {
                        for ch in chans_arr {
                            let ch_id = ch["id"].as_str().unwrap_or("").to_string();
                            let ch_name = ch["name"].as_str().unwrap_or("canal").to_string();
                            let ch_type = ch["type"].as_u64().unwrap_or(0);

                            // type 0 = text, type 2 = voice
                            if ch_type == 0 || ch_type == 2 {
                                channels.push(ChannelData {
                                    id: ch_id,
                                    name: ch_name,
                                    is_voice: ch_type == 2,
                                });
                            }
                        }
                    }

                    // Parse members to cache user display names
                    if let Some(members_arr) = v["d"]["members"].as_array() {
                        for m in members_arr {
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
                                    register_user_name(uid, display_name);
                                }
                            }
                        }
                    }

                    // Parse voice_states to populate existing voice channel participants
                    if let Some(vs_arr) = v["d"]["voice_states"].as_array() {
                        for vs in vs_arr {
                            let uid: u64 = if let Some(s) = vs["user_id"].as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                vs["user_id"].as_u64().unwrap_or(0)
                            };
                            let cid_str = if let Some(s) = vs["channel_id"].as_str() {
                                s.to_string()
                            } else if let Some(n) = vs["channel_id"].as_u64() {
                                n.to_string()
                            } else {
                                String::new()
                            };

                            if uid > 0 && !cid_str.is_empty() {
                                if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                                    gvs.insert(uid, cid_str.clone());
                                    info!("📌 [GUILD_CREATE] Pré-carregado estado de voz: User {} -> Canal {}", uid, cid_str);
                                }
                                let mut display_name = String::new();
                                if let Some(nick) = vs["member"]["nick"].as_str() {
                                    if !nick.is_empty() { display_name = nick.to_string(); }
                                }
                                if display_name.is_empty() {
                                    if let Some(gname) = vs["member"]["user"]["global_name"].as_str() {
                                        if !gname.is_empty() { display_name = gname.to_string(); }
                                    }
                                }
                                if display_name.is_empty() {
                                    if let Some(uname) = vs["member"]["user"]["username"].as_str() {
                                        if !uname.is_empty() { display_name = uname.to_string(); }
                                    }
                                }
                                if !display_name.is_empty() {
                                    register_user_name(uid, display_name);
                                }
                            }
                        }
                    }

                    info!("Servidor carregado: '{}' ({} canais)", name, channels.len());

                    let guild = GuildData { id, name, channels };
                    let _ = self.event_tx.send(GatewayEvent::GuildLoaded { guild }).await;
                }
                _ => {}
            }
        }
    }
}

pub async fn connect_voice_gateway(
    raw_endpoint: &str,
    guild_id: &str,
    user_id: &str,
    session_id: &str,
    token: &str,
    channel_id: &str,
    self_mute_state: Arc<std::sync::Mutex<bool>>,
    event_tx: mpsc::Sender<GatewayEvent>,
) {
    let clean_endpoint = raw_endpoint.trim();
    let voice_url = if clean_endpoint.starts_with("wss://") || clean_endpoint.starts_with("ws://") {
        clean_endpoint.to_string()
    } else {
        format!("wss://{}/?v=4", clean_endpoint)
    };
    info!("Conectando Ã  Discord Voice Gateway: {}...", voice_url);

    let my_session_id = CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst) + 1;
    info!("Iniciando nova sessÃ£o de voz ID={}", my_session_id);
    let cid_num: u64 = channel_id.parse().unwrap_or(0);
    set_my_voice_channel_id(cid_num);

    match connect_async(&voice_url).await {
        Ok((ws_stream, _)) => {
            info!("ConexÃ£o WebSocket com Voice Gateway estabelecida!");
            let (write, mut read) = ws_stream.split();
            let write_arc = Arc::new(Mutex::new(write));

            let guild_id = guild_id.to_string();
            let user_id = user_id.to_string();
            let session_id = session_id.to_string();
            let token = token.to_string();
            let channel_id_str = channel_id.to_string();
            let event_tx_vclose = event_tx.clone();
            sync_voice_channel_participants(&channel_id_str);
            let active_ssrc: Arc<std::sync::Mutex<u32>> = Arc::new(std::sync::Mutex::new(12345));
            let ssrc_to_userid: Arc<std::sync::Mutex<HashMap<u32, u64>>> = Arc::new(std::sync::Mutex::new(HashMap::new()));
            let secret_key_arc: Arc<std::sync::Mutex<Option<Vec<u8>>>> = Arc::new(std::sync::Mutex::new(None));

            // Create DAVE (Discord Audio/Video E2EE) session using the davey crate
            let uid_num: u64 = user_id.parse().unwrap_or(0);
            let cid_num: u64 = channel_id_str.parse().unwrap_or(0);
            let dave_session: Arc<std::sync::Mutex<Option<DaveSession>>> = Arc::new(std::sync::Mutex::new(
                DaveSession::new(NonZeroU16::new(1).unwrap(), uid_num, cid_num, None)
                    .map_err(|e| { warn!("Falha ao criar DaveSession: {:?}", e); })
                    .ok()
            ));
            let saved_external_sender: Arc<std::sync::Mutex<Option<Vec<u8>>>> = Arc::new(std::sync::Mutex::new(None));

            // Read loop: wait for Opcode 8 HELLO from Voice Gateway before sending Opcode 0 Identify!
            while let Some(msg_result) = read.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        if let Ok(val) = serde_json::from_str::<Value>(&text) {
                            let op = val["op"].as_u64().unwrap_or(99);
                            info!("Payload recebido da Voice Gateway: op={}", op);

                            match op {
                                8 => {
                                    // Voice Opcode 8: HELLO
                                    let heartbeat_interval = val["d"]["heartbeat_interval"]
                                        .as_u64()
                                        .unwrap_or(20000);
                                    info!("Voice Gateway HELLO recebido! Intervalo de Heartbeat: {} ms", heartbeat_interval);

                                    // 1. Send Opcode 0 Voice Identify (with max_dave_protocol_version: 1 for E2EE DAVE support)
                                    let voice_identify = serde_json::json!({
                                        "op": 0,
                                        "d": {
                                            "server_id": guild_id,
                                            "user_id": user_id,
                                            "session_id": session_id,
                                            "token": token,
                                            "video": false,
                                            "streams": [],
                                            "max_dave_protocol_version": 1
                                        }
                                    });

                                    info!("Enviando OP 0 Voice Identify para a Voice Gateway: {}", voice_identify);
                                    {
                                        let mut w = write_arc.lock().await;
                                        if let Err(e) = w.send(Message::Text(voice_identify.to_string().into())).await {
                                            error!("Erro ao enviar Opcode 0 Voice Identify: {:?}", e);
                                            break;
                                        }
                                    }

                                    // 2. Spawn Voice Heartbeat loop (Opcode 3 with incremental nonces)
                                    let write_hb = Arc::clone(&write_arc);
                                    tokio::spawn(async move {
                                        let mut nonce: u64 = 1000;
                                        loop {
                                            if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id { break; }
                                            sleep(Duration::from_millis(heartbeat_interval)).await;
                                            nonce += 1;
                                            let hb = serde_json::json!({ "op": 3, "d": nonce });
                                            let mut w = write_hb.lock().await;
                                            if let Err(_) = w.send(Message::Text(hb.to_string().into())).await {
                                                break;
                                            }
                                        }
                                    });
                                }
                                2 => {
                                    // Voice Opcode 2: READY!
                                    set_is_connected_to_voice(true);
                                    let ssrc = val["d"]["ssrc"].as_u64().unwrap_or(12345) as u32;
                                    *active_ssrc.lock().unwrap() = ssrc;
                                    ssrc_to_userid.lock().unwrap().insert(ssrc, uid_num);
                                    register_voice_participant(ssrc, uid_num);
                                    register_voice_participant(999999, 999999);

                                    let voice_ip = val["d"]["ip"].as_str().unwrap_or("").to_string();
                                    let voice_port = val["d"]["port"].as_u64().unwrap_or(0) as u16;

                                    let selected_mode = if let Some(modes) = val["d"]["modes"].as_array() {
                                        modes.iter()
                                            .find_map(|m| m.as_str())
                                            .unwrap_or("aead_aes256_gcm_rtpsize")
                                            .to_string()
                                    } else {
                                        "aead_aes256_gcm_rtpsize".to_string()
                                    };

                                    info!("🎉 VOICE GATEWAY PRONTA (Opcode 2 READY)! SSRC={}, IP={}:{}, Encryption Mode={}", ssrc, voice_ip, voice_port, selected_mode);

                                    // Synthesize Join Voice Chime (523.25Hz C5 -> 659.25Hz E5) to test local audio playback
                                    let mut chime_samples = Vec::new();
                                    let sr = 48000.0f32;
                                    for i in 0..(0.12 * sr) as usize {
                                        let t = i as f32 / sr;
                                        let s = (t * 523.25 * 2.0 * std::f32::consts::PI).sin() * 0.4;
                                        chime_samples.push(s);
                                    }
                                    for i in 0..(0.18 * sr) as usize {
                                        let t = i as f32 / sr;
                                        let s = (t * 659.25 * 2.0 * std::f32::consts::PI).sin() * 0.4;
                                        chime_samples.push(s);
                                    }

                                    if let Ok(mut queues) = get_speaker_pcm_queues().lock() {
                                        let q = queues.entry(999999).or_insert_with(|| VecDeque::with_capacity(48000));
                                        q.clear();
                                        for &s in &chime_samples {
                                            q.push_back((s, s));
                                        }
                                    }
                                    info!("🎵 Efeito sonoro de entrada no canal de voz injetado na fila dos alto-falantes!");

                                    // UDP IP Discovery & Opcode 1 Select Protocol Handshake
                                    if !voice_ip.is_empty() && voice_port > 0 {
                                        let write_arc_proto = Arc::clone(&write_arc);
                                        let secret_key_udp = Arc::clone(&secret_key_arc);
                                        let dave_session_audio = Arc::clone(&dave_session);
                                        let ssrc_to_userid_audio = Arc::clone(&ssrc_to_userid);
                                        let self_mute_rx = Arc::clone(&self_mute_state);

                                        tokio::spawn(async move {
                                            let socket = match UdpSocket::bind("0.0.0.0:0").await {
                                                Ok(s) => Arc::new(s),
                                                Err(e) => { warn!("Falha ao criar socket UDP de voz: {:?}", e); return; }
                                            };
                                                let target_addr = format!("{}:{}", voice_ip, voice_port);
                                                info!("Socket UDP de Voz conectado a {}", target_addr);

                                                // 1. Send 74-byte UDP IP Discovery Packet (RFC / Discord spec: 2 bytes type, 2 bytes len, 4 bytes ssrc, 64 bytes addr, 2 bytes port)
                                                let mut discovery = [0u8; 74];
                                                discovery[0..2].copy_from_slice(&1u16.to_be_bytes());
                                                discovery[2..4].copy_from_slice(&70u16.to_be_bytes());
                                                discovery[4..8].copy_from_slice(&ssrc.to_be_bytes());

                                                let mut my_pub_ip = String::new();
                                                let mut my_pub_port = 0u16;

                                                for attempt in 1..=5 {
                                                    if socket.send_to(&discovery, &target_addr).await.is_ok() {
                                                        let mut buf = [0u8; 128];
                                                        if let Ok(Ok((len, _))) = tokio::time::timeout(Duration::from_millis(600), socket.recv_from(&mut buf)).await {
                                                            if len >= 70 {
                                                                let ip_slice = &buf[8..len.saturating_sub(2)];
                                                                let ip_end = ip_slice.iter().position(|&b| b == 0).unwrap_or(ip_slice.len());
                                                                let parsed_ip = String::from_utf8_lossy(&ip_slice[..ip_end]).trim().to_string();
                                                                let parsed_port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);

                                                                if !parsed_ip.is_empty() && parsed_port > 0 {
                                                                    my_pub_ip = parsed_ip;
                                                                    my_pub_port = parsed_port;
                                                                    info!("UDP IP Discovery resolvido com sucesso na tentativa {}: {}:{}", attempt, my_pub_ip, my_pub_port);
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    warn!("Tentativa {} de UDP IP Discovery falhou/timeout, tentando novamente...", attempt);
                                                }

                                                if my_pub_ip.is_empty() {
                                                    // Fallback to fetching public IP via HTTP if UDP discovery failed completely
                                                    if let Ok(res) = reqwest::get("https://api.ipify.org").await {
                                                        if let Ok(ip_text) = res.text().await {
                                                            let ip_trimmed = ip_text.trim().to_string();
                                                            if !ip_trimmed.is_empty() {
                                                                my_pub_ip = ip_trimmed;
                                                                my_pub_port = socket.local_addr().map(|a| a.port()).unwrap_or(50000);
                                                                warn!("Usando fallback HTTP para IP público: {}:{}", my_pub_ip, my_pub_port);
                                                            }
                                                        }
                                                    }
                                                }

                                                if my_pub_ip.is_empty() {
                                                    my_pub_ip = voice_ip.clone();
                                                    my_pub_port = socket.local_addr().map(|a| a.port()).unwrap_or(50000);
                                                    error!("CRÍTICO: Não foi possível determinar o IP público local!");
                                                }

                                                info!("UDP IP Discovery Concluído! IP: {}:{}", my_pub_ip, my_pub_port);

                                                // 2. Send Opcode 1 Select Protocol to Voice Gateway WebSocket
                                                let select_proto = serde_json::json!({
                                                    "op": 1,
                                                    "d": {
                                                        "protocol": "udp",
                                                        "data": {
                                                            "address": my_pub_ip,
                                                            "port": my_pub_port,
                                                            "mode": selected_mode
                                                        }
                                                    }
                                                });

                                                info!("Enviando Opcode 1 Select Protocol para a Voice Gateway...");
                                                {
                                                    let mut w = write_arc_proto.lock().await;
                                                    let _ = w.send(Message::Text(select_proto.to_string().into())).await;
                                                }

                                                // 3. Initialize Pure Rust OpusEncoder (48000Hz, Stereo 2 channels, Application::Voip)
                                                let mut opus_encoder = OpusEncoder::new(48000, 2, Application::Voip)
                                                    .expect("Falha ao inicializar o OpusEncoder nativo em Rust");

                                                // 4. Spawn incoming UDP voice receive loop
                                                let socket_rx = Arc::clone(&socket);
                                                let secret_key_rx = Arc::clone(&secret_key_udp);
                                                let dave_session_rx = Arc::clone(&dave_session_audio);
                                                let ssrc_to_userid_rx = Arc::clone(&ssrc_to_userid_audio);
                                                let speaker_queues_rx = get_speaker_pcm_queues();
                                                let my_ssrc = ssrc;
                                                let rx_session_id = my_session_id;
                                                tokio::spawn(async move {
                                                    let mut opus_decoders: HashMap<(u32, usize), OpusDecoder> = HashMap::new();
                                                    let mut ssrc_last_pkt_time: HashMap<u32, std::time::Instant> = HashMap::new();
                                                    let mut ssrc_expected_seq: HashMap<u32, u16> = HashMap::new();
                                                    let mut recv_buf = vec![0u8; 4096];
                                                    let mut pcm_out_buf = vec![0.0f32; 11520];
                                                    let mut detected_ssrcs = std::collections::HashSet::new();
                                                    let mut total_pkts_recv = 0u64;
                                                    let mut decrypt_err_cnt = 0u64;
                                                    let mut opus_err_cnt = 0u64;
                                                    let mut dave_decrypt_fail_cnt = 0u64;
                                                    let mut _dave_not_ready_cnt = 0u64;

                                                    info!("🎧 Loop UDP de recepção de voz INICIADO (Session ID={})!", rx_session_id);

                                                    loop {
                                                        if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != rx_session_id { break; }
                                                        match tokio::time::timeout(Duration::from_millis(100), socket_rx.recv_from(&mut recv_buf)).await {
                                                            Ok(Ok((len, addr))) => {
                                                                if len < 12 { continue; }
                                                                let pkt = &recv_buf[..len];
                                                                
                                                                // Parse RTP header
                                                                let version = (pkt[0] >> 6) & 0x3;
                                                                if version != 2 { continue; }
                                                                let pt = pkt[1] & 0x7F;
                                                                if pt != 120 { continue; } // Strictly allow ONLY Opus Audio packets (Payload Type 120)

                                                                let ssrc_recv = u32::from_be_bytes([pkt[8], pkt[9], pkt[10], pkt[11]]);
                                                                if ssrc_recv == my_ssrc { continue; }

                                                                total_pkts_recv += 1;

                                                                if detected_ssrcs.insert(ssrc_recv) {
                                                                    info!("🎙️ NOVO SSRC DE VOZ REMOTO RECEBIDO! SSRC={} de {}", ssrc_recv, addr);
                                                                }

                                                                 let cc = (pkt[0] & 0x0F) as usize;
                                                                let has_ext = (pkt[0] & 0x10) != 0;
                                                                let base_header_len = 12 + 4 * cc;

                                                                let ext_len_words = if has_ext && len >= base_header_len + 4 {
                                                                    u16::from_be_bytes([pkt[base_header_len + 2], pkt[base_header_len + 3]]) as usize
                                                                } else {
                                                                    0
                                                                };
                                                                let ext_bytes_len = ext_len_words * 4;

                                                                // rtpsize nonce: last 4 bytes of packet, padding byte before nonce if P bit set
                                                                let has_padding = (pkt[0] & 0x20) != 0;
                                                                let padding_len = if has_padding && len > 5 {
                                                                    pkt[len - 5] as usize
                                                                } else {
                                                                    0
                                                                };

                                                                let ciphertext_end = if len >= 4 + padding_len && len - 4 - padding_len > base_header_len {
                                                                    len - 4 - padding_len
                                                                } else {
                                                                    len - 4
                                                                };

                                                                if ciphertext_end <= base_header_len { continue; }

                                                                let nonce_bytes_rx: [u8; 4] = [pkt[len-4], pkt[len-3], pkt[len-2], pkt[len-1]];
                                                                let nonce_u32_le = u32::from_le_bytes(nonce_bytes_rx);
                                                                let nonce_u32_be = u32::from_be_bytes(nonce_bytes_rx);

                                                                let mut n1 = [0u8; 12]; n1[0..4].copy_from_slice(&nonce_bytes_rx);
                                                                let mut n2 = [0u8; 12]; n2[8..12].copy_from_slice(&nonce_bytes_rx);
                                                                let mut n3 = [0u8; 12]; n3[0..4].copy_from_slice(&nonce_u32_le.to_be_bytes());
                                                                let mut n4 = [0u8; 12]; n4[8..12].copy_from_slice(&nonce_u32_le.to_be_bytes());
                                                                let mut n5 = [0u8; 12]; n5[0..4].copy_from_slice(&nonce_u32_be.to_le_bytes());
                                                                let mut n6 = [0u8; 12]; n6[8..12].copy_from_slice(&nonce_u32_be.to_le_bytes());

                                                                let nonce_candidates = [&n1, &n2, &n3, &n4, &n5, &n6];

                                                                let key_opt = secret_key_rx.lock().unwrap().clone();
                                                                if let Some(key_bytes) = key_opt {
                                                                    if let Ok(cipher) = Aes256Gcm::new_from_slice(&key_bytes) {
                                                                        let candidates: &[(usize, usize)] = if has_ext {
                                                                            &[
                                                                                (base_header_len + 4, ext_bytes_len),
                                                                                (base_header_len + 4 + ext_bytes_len, 0),
                                                                                (base_header_len, ext_bytes_len + 4),
                                                                                (base_header_len, 0),
                                                                            ]
                                                                        } else {
                                                                            &[(base_header_len, 0)]
                                                                        };

                                                                        let mut decrypted_opt: Option<(Vec<u8>, usize)> = None;

                                                                        'try_decrypt: for &(test_aad_len, test_ext_len) in candidates {
                                                                            if test_aad_len >= ciphertext_end { continue; }
                                                                            let header = &pkt[..test_aad_len];
                                                                            let ciphertext = &pkt[test_aad_len..ciphertext_end];

                                                                            for &nonce_cand in &nonce_candidates {
                                                                                let payload = aes_gcm::aead::Payload { msg: ciphertext, aad: header };
                                                                                if let Ok(dec) = cipher.decrypt(Nonce::from_slice(nonce_cand), payload) {
                                                                                    if dec.len() >= test_ext_len {
                                                                                        decrypted_opt = Some((dec, test_ext_len));
                                                                                        break 'try_decrypt;
                                                                                    }
                                                                                }
                                                                            }
                                                                        }

                                                                        let (decrypted_raw, ext_skip) = match decrypted_opt {
                                                                            Some(res) => {
                                                                                static LOGGED_SUCCESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                                                                if !LOGGED_SUCCESS.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                                                                    info!("🎉 Descriptografia AES-GCM BEM-SUCEDIDA para SSRC {}! ext_skip={} bytes", ssrc_recv, res.1);
                                                                                }
                                                                                res
                                                                            },
                                                                            None => {
                                                                                decrypt_err_cnt += 1;
                                                                                if decrypt_err_cnt % 50 == 1 {
                                                                                    warn!("Descriptografia AES-GCM falhou para SSRC {}: len={} has_ext={} cc={} base_len={} ext_words={} header={:02X?} nonce={:02X?}",
                                                                                        ssrc_recv, len, has_ext, cc, base_header_len, ext_len_words, &pkt[..16.min(len)], nonce_bytes_rx);
                                                                                }
                                                                                continue;
                                                                            }
                                                                        };

                                                                        if decrypted_raw.len() < ext_skip { continue; }
                                                                        let transport_payload = &decrypted_raw[ext_skip..];

                                                                        let user_id_opt = ssrc_to_userid_rx.lock().unwrap().get(&ssrc_recv).copied();
                                                                        let sender_user_id = user_id_opt.unwrap_or(ssrc_recv as u64);

                                                                        let (opus_data, can_decode) = {
                                                                            let mut sess = dave_session_rx.lock().unwrap();
                                                                            if let Some(ref mut s) = *sess {
                                                                                if s.is_ready() {
                                                                                    let res = s.decrypt(sender_user_id, MediaType::AUDIO, transport_payload);
                                                                                    let res = match res {
                                                                                        Ok(d) => Ok(d),
                                                                                        Err(_) => {
                                                                                            let swapped_uid = u64::from_be_bytes(sender_user_id.to_le_bytes());
                                                                                            s.decrypt(swapped_uid, MediaType::AUDIO, transport_payload)
                                                                                        }
                                                                                    };
                                                                                    let res = match res {
                                                                                        Ok(d) => Ok(d),
                                                                                        Err(_) => {
                                                                                            if let Some(uids) = s.get_user_ids() {
                                                                                                let mut found = None;
                                                                                                for &alt_uid in &uids {
                                                                                                    if alt_uid != s.user_id() {
                                                                                                        if let Ok(d) = s.decrypt(alt_uid, MediaType::AUDIO, transport_payload) {
                                                                                                            found = Some((d, alt_uid));
                                                                                                            break;
                                                                                                        }
                                                                                                    }
                                                                                                }
                                                                                                if let Some((d, matched_uid)) = found {
                                                                                                    if let Ok(mut m) = ssrc_to_userid_rx.lock() {
                                                                                                        m.insert(ssrc_recv, matched_uid);
                                                                                                    }
                                                                                                    register_voice_participant(ssrc_recv, matched_uid);
                                                                                                    Ok(d)
                                                                                                } else {
                                                                                                    Err(davey::errors::DecryptError::NoDecryptorForUser)
                                                                                                }
                                                                                            } else {
                                                                                                Err(davey::errors::DecryptError::NoDecryptorForUser)
                                                                                            }
                                                                                        }
                                                                                    };

                                                                                    match res {
                                                                                        Ok(d) => (d, true),
                                                                                        Err(e) => {
                                                                                            dave_decrypt_fail_cnt += 1;
                                                                                            if dave_decrypt_fail_cnt % 50 == 1 {
                                                                                                warn!("🔑 [DAVE DIAG] decrypt() FALHOU para SSRC={} UserID={} (payload_len={}, group_users={:?}): {:?}",
                                                                                                    ssrc_recv, sender_user_id, transport_payload.len(), s.get_user_ids(), e);
                                                                                            }
                                                                                            (Vec::new(), false)
                                                                                        }
                                                                                    }
                                                                                } else {
                                                                                    _dave_not_ready_cnt += 1;
                                                                                    let has_dave_magic = transport_payload.len() >= 4 && transport_payload.ends_with(&[0xFA, 0xFA]);
                                                                                    if has_dave_magic {
                                                                                        (Vec::new(), false)
                                                                                    } else {
                                                                                        (transport_payload.to_vec(), true)
                                                                                    }
                                                                                }
                                                                            } else {
                                                                                let has_dave_magic = transport_payload.len() >= 4 && transport_payload.ends_with(&[0xFA, 0xFA]);
                                                                                if has_dave_magic {
                                                                                    (Vec::new(), false)
                                                                                } else {
                                                                                    (transport_payload.to_vec(), true)
                                                                                }
                                                                            }
                                                                        };

                                                                        let mut decode_success = false;
                                                                        let mut decoded_count = 0;
                                                                        let mut plc_pairs: Vec<(f32, f32)> = Vec::new();
                                                                        let mut pkt_channels = 2usize;
                                                                        if can_decode && !opus_data.is_empty() {
                                                                            let mut raw_opus = opus_data.as_slice();

                                                                            if raw_opus.len() >= 4 && (raw_opus.starts_with(&[0xBE, 0xDE]) || raw_opus.starts_with(&[0x10, 0x00])) {
                                                                                let ext_words = u16::from_be_bytes([raw_opus[2], raw_opus[3]]) as usize;
                                                                                let ext_total_bytes = 4 + ext_words * 4;
                                                                                if raw_opus.len() > ext_total_bytes {
                                                                                    raw_opus = &raw_opus[ext_total_bytes..];
                                                                                }
                                                                            }

                                                                            if raw_opus.first() == Some(&0x00) && raw_opus.len() > 1 {
                                                                                raw_opus = &raw_opus[1..];
                                                                            }

                                                                            if (pkt[0] & 0x20) != 0 && !raw_opus.is_empty() {
                                                                                let pad_len = raw_opus[raw_opus.len() - 1] as usize;
                                                                                if pad_len > 0 && pad_len <= raw_opus.len() {
                                                                                    raw_opus = &raw_opus[..raw_opus.len() - pad_len];
                                                                                }
                                                                            }

                                                                            if raw_opus.is_empty() { continue; }

                                                                            pkt_channels = if (raw_opus[0] & 0x04) != 0 { 2 } else { 1 };
                                                                            let dec = opus_decoders.entry((ssrc_recv, pkt_channels)).or_insert_with(|| {
                                                                                OpusDecoder::new(48000, pkt_channels).expect("Falha ao inicializar OpusDecoder 48kHz")
                                                                            });

                                                                            let now = std::time::Instant::now();
                                                                            let rtp_seq = u16::from_be_bytes([pkt[2], pkt[3]]);
                                                                            let last_time_opt = ssrc_last_pkt_time.get(&ssrc_recv).copied();
                                                                            let is_new_talkspurt = match last_time_opt {
                                                                                Some(t) => now.duration_since(t) > Duration::from_millis(100),
                                                                                None => true,
                                                                            };

                                                                            if is_new_talkspurt {
                                                                                // New burst of speech after silence/VAD gap: sync sequence and skip false PLC to avoid pops
                                                                                ssrc_expected_seq.insert(ssrc_recv, rtp_seq.wrapping_add(1));
                                                                            } else {
                                                                                if let Some(last_seq) = ssrc_expected_seq.get(&ssrc_recv).copied() {
                                                                                    let missed = rtp_seq.wrapping_sub(last_seq);
                                                                                    if missed > 0 && missed <= 4 {
                                                                                        let mut plc_buf = [0.0f32; 1920];
                                                                                        for _ in 0..missed {
                                                                                            if let Ok(samples) = dec.decode(&[], 960, &mut plc_buf[..]) {
                                                                                                for i in 0..samples {
                                                                                                    if pkt_channels == 2 {
                                                                                                        plc_pairs.push((plc_buf[i * 2].clamp(-1.0, 1.0), plc_buf[i * 2 + 1].clamp(-1.0, 1.0)));
                                                                                                    } else {
                                                                                                        let m = plc_buf[i].clamp(-1.0, 1.0);
                                                                                                        plc_pairs.push((m, m));
                                                                                                    }
                                                                                                }
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                }
                                                                                ssrc_expected_seq.insert(ssrc_recv, rtp_seq.wrapping_add(1));
                                                                            }
                                                                            ssrc_last_pkt_time.insert(ssrc_recv, now);

                                                                            let is_dtx_silence = raw_opus.len() <= 3 && raw_opus == [0xF8, 0xFF, 0xFE];
                                                                            if is_dtx_silence {
                                                                                decode_success = true;
                                                                                decoded_count = 960;
                                                                                for s in pcm_out_buf[..1920].iter_mut() { *s = 0.0; }
                                                                            } else {
                                                                                match dec.decode(raw_opus, 5760, &mut pcm_out_buf[..]) {
                                                                                    Ok(samples) => {
                                                                                        decode_success = true;
                                                                                        decoded_count = samples;
                                                                                    }
                                                                                    Err(e) => {
                                                                                        warn!("📊 [OPUS DIAG] Decode FALHOU: {:?}", e);
                                                                                    }
                                                                                }
                                                                            }
                                                                        }

                                                                        if decode_success && decoded_count > 0 {
                                                                            register_voice_participant(ssrc_recv, sender_user_id);
                                                                            let total_samples = if pkt_channels == 2 { decoded_count * 2 } else { decoded_count };
                                                                            let mut sum_sq = 0.0f32;
                                                                            let mut max_peak = 0.0f32;
                                                                            let mut zc_count = 0usize;
                                                                            let mut prev_s = 0.0f32;
                                                                            let mut diff_energy = 0.0f32;

                                                                            for (idx, &s) in pcm_out_buf[..total_samples].iter().enumerate() {
                                                                                let abs_s = s.abs();
                                                                                if abs_s > max_peak { max_peak = abs_s; }
                                                                                sum_sq += s * s;
                                                                                if idx > 0 {
                                                                                    if (s >= 0.0 && prev_s < 0.0) || (s < 0.0 && prev_s >= 0.0) {
                                                                                        zc_count += 1;
                                                                                    }
                                                                                    let diff = s - prev_s;
                                                                                    diff_energy += diff * diff;
                                                                                }
                                                                                prev_s = s;
                                                                            }
                                                                            let frame_rms = (sum_sq / total_samples.max(1) as f32).sqrt();
                                                                            let hf_noise_ratio = (diff_energy / (sum_sq + 1e-6)).sqrt();
                                                                            let is_silence = frame_rms < 0.003 || opus_data.len() <= 3;

                                                                            let mut dump_vec = Vec::with_capacity(decoded_count);
                                                                            if is_silence {
                                                                                for _ in 0..decoded_count { dump_vec.push((0.0, 0.0)); }
                                                                            } else if pkt_channels == 2 {
                                                                                for i in 0..decoded_count {
                                                                                    dump_vec.push((pcm_out_buf[i * 2].clamp(-1.0, 1.0), pcm_out_buf[i * 2 + 1].clamp(-1.0, 1.0)));
                                                                                }
                                                                            } else {
                                                                                for &s in &pcm_out_buf[..decoded_count] {
                                                                                    let mono = s.clamp(-1.0, 1.0);
                                                                                    dump_vec.push((mono, mono));
                                                                                }
                                                                            }
                                                                                    ssrc_last_pkt_time.insert(ssrc_recv, std::time::Instant::now());

                                                                                    if let Ok(mut queues) = speaker_queues_rx.lock() {
                                                                                        let q = queues.entry(ssrc_recv).or_insert_with(|| VecDeque::with_capacity(48000));
                                                                                        // Maintain real-time jitter buffer: cap at 1s (48000 samples)
                                                                                        while q.len() > 48000 {
                                                                                            q.pop_front();
                                                                                        }
                                                                                        // Push synthesized PLC concealed frames first to maintain unbroken audio stream
                                                                                        for &pair in &plc_pairs {
                                                                                            q.push_back(pair);
                                                                                        }
                                                                                        for &pair in &dump_vec {
                                                                                            q.push_back(pair);
                                                                                        }

                                                                                        if total_pkts_recv % 100 == 0 {
                                                                                            info!("📊 [ESPECTRO DE VOZ & NOISE DUMP] Pkts={:4} | SSRC={} | RMS={:.5} | Peak={:.4} | ZC={:3} | HF_Ratio={:.3} | QueueLen={:5}",
                                                                                                total_pkts_recv, ssrc_recv, frame_rms, max_peak, zc_count, hf_noise_ratio, q.len());
                                                                                        }
                                                                                    }
                                                                                } else if !opus_data.is_empty() {
                                                                                    opus_err_cnt += 1;
                                                                                    if opus_err_cnt % 50 == 1 {
                                                                                        warn!("📊 [MÉTRICA DE ERRO] Opus decode falhou para SSRC {} (Erros={})", ssrc_recv, opus_err_cnt);
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                            }
                                                            Ok(Err(e)) => {
                                                                warn!("Erro socket.recv_from: {:?}", e);
                                                            }
                                                            Err(_) => { /* timeout 100ms, keep looping */ }
                                                        }
                                                    }
                                                    info!("Loop de recepção de voz (ID={}) encerrado.", rx_session_id);
                                                });

                                                let speaker_queues_out = get_speaker_pcm_queues();
                                                let ssrc_to_userid_speaker = Arc::clone(&ssrc_to_userid_audio);
                                                let out_session_id = my_session_id;
                                                std::thread::spawn(move || {
                                                    use cpal::traits::{HostTrait, DeviceTrait};
                                                    let host = cpal::default_host();

                                                    let target_dev_name = get_selected_output_device_store().lock().unwrap().clone();
                                                    let device = if let Ok(devices) = host.output_devices() {
                                                        if !target_dev_name.is_empty() && !target_dev_name.contains("Padrão") {
                                                            devices.into_iter().find(|d| d.name().map(|n| n == target_dev_name).unwrap_or(false))
                                                                .or_else(|| host.default_output_device())
                                                        } else {
                                                            host.default_output_device()
                                                        }
                                                    } else {
                                                        host.default_output_device()
                                                    };

                                                    let device = match device {
                                                        Some(d) => d,
                                                        None => { warn!("Nenhum dispositivo de saída de áudio encontrado!"); return; }
                                                    };

                                                    let config = match device.default_output_config() {
                                                        Ok(c) => c,
                                                        Err(e) => { warn!("Falha ao obter config de saída: {:?}", e); return; }
                                                    };
                                                    let out_sample_rate = config.sample_rate().0;
                                                    let out_channels = config.channels() as usize;
                                                    info!("Saída de Áudio (Speaker): {}Hz, {} canal(is), formato={:?}",
                                                        out_sample_rate, out_channels, config.sample_format());

                                                    let sq = speaker_queues_out;
                                                    let ssrc_to_userid_spk = Arc::clone(&ssrc_to_userid_speaker);
                                                    let mut started_ssrcs = std::collections::HashSet::new();
                                                    let mut inactive_ticks = std::collections::HashMap::new();
                                                    // Fractional phase counters per SSRC for smooth Hermite cubic sample rate conversion (48kHz -> out_sample_rate)
                                                    let mut ssrc_phases: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
                                                    let mut ssrc_histories: std::collections::HashMap<u32, [(f32, f32); 4]> = std::collections::HashMap::new();
                                                    let _step = 48000.0f64 / out_sample_rate.max(1) as f64;

                                                    let stream_res = match config.sample_format() {
                                                        cpal::SampleFormat::F32 => {
                                                            let sq_f32 = Arc::clone(&sq);
                                                            let ssrc_to_userid_f32 = Arc::clone(&ssrc_to_userid_spk);
                                                            device.build_output_stream(
                                                                &config.into(),
                                                                move |output: &mut [f32], _| {
                                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != out_session_id || is_self_deaf() {
                                                                        for s in output.iter_mut() { *s = 0.0; }
                                                                        return;
                                                                    }
                                                                    for s in output.iter_mut() { *s = 0.0; }

                                                                    if let Ok(mut queues) = sq_f32.lock() {
                                                                        let frames = output.len() / out_channels.max(1);
                                                                        let mut ready_ssrcs = Vec::new();
                                                                        let mut to_remove = Vec::new();

                                                                        for (&ssrc, q) in queues.iter() {
                                                                            if q.is_empty() {
                                                                                let ticks = inactive_ticks.entry(ssrc).or_insert(0);
                                                                                *ticks += 1;
                                                                                // Buffer underflow: reset state so it re-buffers 10ms
                                                                                if *ticks > 3 {
                                                                                    started_ssrcs.remove(&ssrc);
                                                                                    ssrc_histories.remove(&ssrc);
                                                                                    ssrc_phases.remove(&ssrc);
                                                                                }
                                                                                if *ticks > 150 {
                                                                                    to_remove.push(ssrc);
                                                                                }
                                                                            } else {
                                                                                inactive_ticks.insert(ssrc, 0);
                                                                            }

                                                                            if started_ssrcs.contains(&ssrc) {
                                                                                if !q.is_empty() {
                                                                                    ready_ssrcs.push(ssrc);
                                                                                }
                                                                            } else if !q.is_empty() {
                                                                                // 10ms pre-buffer (480 samples) to start playback immediately with zero latency
                                                                                if q.len() >= 480 {
                                                                                    started_ssrcs.insert(ssrc);
                                                                                    ready_ssrcs.push(ssrc);
                                                                                }
                                                                            }
                                                                        }

                                                                        for ssrc in to_remove {
                                                                            queues.remove(&ssrc);
                                                                            started_ssrcs.remove(&ssrc);
                                                                            inactive_ticks.remove(&ssrc);
                                                                            ssrc_phases.remove(&ssrc);
                                                                            ssrc_histories.remove(&ssrc);
                                                                        }

                                                                        if ready_ssrcs.is_empty() { return; }

                                                                        let mut ssrc_vol_map = std::collections::HashMap::new();
                                                                        let mut max_active_priority = 0i32;
                                                                        let active_spk_store = get_active_voice_participants_store();
                                                                        let active_spk_map = active_spk_store.lock().ok();
                                                                        if let Ok(spk_map) = ssrc_to_userid_f32.lock() {
                                                                            for &ssrc in &ready_ssrcs {
                                                                                let uid_num = spk_map.get(&ssrc)
                                                                                    .or_else(|| active_spk_map.as_ref().and_then(|m| m.get(&ssrc)))
                                                                                    .copied()
                                                                                    .unwrap_or(ssrc as u64);

                                                                                let (is_muted_uid, vol_uid, prio_uid) = get_user_audio_settings(&uid_num.to_string());
                                                                                let (is_muted_ssrc, vol_ssrc, prio_ssrc) = get_user_audio_settings(&ssrc.to_string());
                                                                                let is_muted = is_muted_uid || is_muted_ssrc;
                                                                                let user_vol = if vol_uid != 1.0 { vol_uid } else { vol_ssrc };
                                                                                let user_prio = prio_uid.max(prio_ssrc);
                                                                                if !is_muted {
                                                                                    if user_prio > max_active_priority {
                                                                                        max_active_priority = user_prio;
                                                                                    }
                                                                                }
                                                                                ssrc_vol_map.insert(ssrc, (is_muted, user_vol, user_prio));
                                                                            }
                                                                        }

                                                                        for f in 0..frames {
                                                                            let mut mixed_l = 0.0f32;
                                                                            let mut mixed_r = 0.0f32;
                                                                            for &ssrc in &ready_ssrcs {
                                                                                let (is_muted, user_vol, user_prio) = ssrc_vol_map.get(&ssrc).copied().unwrap_or((false, 1.0, 0));
                                                                                if is_muted { continue; }

                                                                                let priority_multiplier = if max_active_priority > 0 && user_prio < max_active_priority {
                                                                                    let diff = max_active_priority - user_prio;
                                                                                    let mult = 0.5f32 - (diff as f32 - 1.0f32) * 0.1f32;
                                                                                    mult.max(0.05f32)
                                                                                } else {
                                                                                    1.0f32
                                                                                };
                                                                                let effective_vol = user_vol * priority_multiplier;

                                                                                if let Some(q) = queues.get_mut(&ssrc) {
                                                                                    let phase = ssrc_phases.entry(ssrc).or_insert(0.0);
                                                                                    let hist = ssrc_histories.entry(ssrc).or_insert([((0.0, 0.0)), ((0.0, 0.0)), ((0.0, 0.0)), ((0.0, 0.0))]);

                                                                                    let step = 48000.0f64 / out_sample_rate.max(1) as f64;

                                                                                    *phase += step;
                                                                                    let pops = *phase as usize;
                                                                                    if pops > 0 {
                                                                                        *phase -= pops as f64;
                                                                                        for _ in 0..pops {
                                                                                            hist[0] = hist[1];
                                                                                            hist[1] = hist[2];
                                                                                            hist[2] = hist[3];
                                                                                            if let Some(next_p) = q.pop_front() {
                                                                                                if hist[0] == (0.0, 0.0) && hist[1] == (0.0, 0.0) && hist[2] == (0.0, 0.0) {
                                                                                                    hist[0] = next_p;
                                                                                                    hist[1] = next_p;
                                                                                                    hist[2] = next_p;
                                                                                                    hist[3] = next_p;
                                                                                                } else {
                                                                                                    hist[3] = next_p;
                                                                                                }
                                                                                            } else {
                                                                                                let decay_l = if hist[2].0.abs() < 0.0001 { 0.0 } else { hist[2].0 * 0.999 };
                                                                                                let decay_r = if hist[2].1.abs() < 0.0001 { 0.0 } else { hist[2].1 * 0.999 };
                                                                                                hist[3] = (decay_l, decay_r);
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                    let t = (*phase as f32).clamp(0.0, 1.0);
                                                                                    let src_l = cubic_hermite(hist[0].0, hist[1].0, hist[2].0, hist[3].0, t);
                                                                                    let src_r = cubic_hermite(hist[0].1, hist[1].1, hist[2].1, hist[3].1, t);

                                                                                    mixed_l += src_l * effective_vol;
                                                                                    mixed_r += src_r * effective_vol;
                                                                                }
                                                                            }

                                                                            let limited_l = soft_limit(mixed_l);
                                                                            let limited_r = soft_limit(mixed_r);
                                                                            if out_channels >= 2 {
                                                                                output[f * out_channels + 0] = limited_l;
                                                                                        output[f * out_channels + 1] = limited_r;
                                                                                for ch in 2..out_channels {
                                                                                    output[f * out_channels + ch] = 0.0;
                                                                                }
                                                                            } else {
                                                                                output[f] = soft_limit((mixed_l + mixed_r) * 0.5);
                                                                            }
                                                                        }
                                                                    }
                                                                },
                                                                |err| { warn!("Erro no stream de saída F32: {:?}", err); },
                                                                None,
                                                            )
                                                        }
                                                        cpal::SampleFormat::I16 => {
                                                            let sq_i16 = Arc::clone(&sq);
                                                            let ssrc_to_userid_i16 = Arc::clone(&ssrc_to_userid_spk);
                                                            device.build_output_stream(
                                                                &config.into(),
                                                                move |output: &mut [i16], _| {
                                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != out_session_id || is_self_deaf() {
                                                                        for s in output.iter_mut() { *s = 0; }
                                                                        return;
                                                                    }
                                                                    for s in output.iter_mut() { *s = 0; }

                                                                    if let Ok(mut queues) = sq_i16.lock() {
                                                                        let frames = output.len() / out_channels.max(1);
                                                                        let mut ready_ssrcs = Vec::new();
                                                                        let mut to_remove = Vec::new();

                                                                        for (&ssrc, q) in queues.iter() {
                                                                            if q.is_empty() {
                                                                                let ticks = inactive_ticks.entry(ssrc).or_insert(0);
                                                                                *ticks += 1;
                                                                                if *ticks > 3 {
                                                                                    started_ssrcs.remove(&ssrc);
                                                                                    ssrc_histories.remove(&ssrc);
                                                                                    ssrc_phases.remove(&ssrc);
                                                                                }
                                                                                if *ticks > 150 {
                                                                                    to_remove.push(ssrc);
                                                                                }
                                                                            } else {
                                                                                inactive_ticks.insert(ssrc, 0);
                                                                            }

                                                                            if started_ssrcs.contains(&ssrc) {
                                                                                if !q.is_empty() {
                                                                                    ready_ssrcs.push(ssrc);
                                                                                }
                                                                            } else if q.len() >= 1920 { // 40ms Jitter Pre-buffer
                                                                                started_ssrcs.insert(ssrc);
                                                                                ready_ssrcs.push(ssrc);
                                                                            }
                                                                        }

                                                                        for ssrc in to_remove {
                                                                            queues.remove(&ssrc);
                                                                            started_ssrcs.remove(&ssrc);
                                                                            inactive_ticks.remove(&ssrc);
                                                                            ssrc_phases.remove(&ssrc);
                                                                            ssrc_histories.remove(&ssrc);
                                                                        }

                                                                        if ready_ssrcs.is_empty() { return; }

                                                                        let mut ssrc_vol_map_i16 = std::collections::HashMap::new();
                                                                        let mut max_active_priority_i16 = 0i32;
                                                                        let active_spk_store_i16 = get_active_voice_participants_store();
                                                                        let active_spk_map_i16 = active_spk_store_i16.lock().ok();
                                                                        if let Ok(spk_map) = ssrc_to_userid_i16.lock() {
                                                                            for &ssrc in &ready_ssrcs {
                                                                                let uid_num = spk_map.get(&ssrc)
                                                                                    .or_else(|| active_spk_map_i16.as_ref().and_then(|m| m.get(&ssrc)))
                                                                                    .copied()
                                                                                    .unwrap_or(ssrc as u64);

                                                                                let (is_muted_uid, vol_uid, prio_uid) = get_user_audio_settings(&uid_num.to_string());
                                                                                let (is_muted_ssrc, vol_ssrc, prio_ssrc) = get_user_audio_settings(&ssrc.to_string());
                                                                                let is_muted = is_muted_uid || is_muted_ssrc;
                                                                                let user_vol = if vol_uid != 1.0 { vol_uid } else { vol_ssrc };
                                                                                let user_prio = prio_uid.max(prio_ssrc);
                                                                                if !is_muted {
                                                                                    if user_prio > max_active_priority_i16 {
                                                                                        max_active_priority_i16 = user_prio;
                                                                                    }
                                                                                }
                                                                                ssrc_vol_map_i16.insert(ssrc, (is_muted, user_vol, user_prio));
                                                                            }
                                                                        }

                                                                        for f in 0..frames {
                                                                            let mut mixed_l = 0.0f32;
                                                                            let mut mixed_r = 0.0f32;
                                                                            for &ssrc in &ready_ssrcs {
                                                                                let (is_muted, user_vol, user_prio) = ssrc_vol_map_i16.get(&ssrc).copied().unwrap_or((false, 1.0, 0));
                                                                                if is_muted { continue; }

                                                                                let priority_multiplier = if max_active_priority_i16 > 0 && user_prio < max_active_priority_i16 {
                                                                                    let diff = max_active_priority_i16 - user_prio;
                                                                                    let mult = 0.5f32 - (diff as f32 - 1.0f32) * 0.1f32;
                                                                                    mult.max(0.05f32)
                                                                                } else {
                                                                                    1.0f32
                                                                                };
                                                                                let effective_vol = user_vol * priority_multiplier;

                                                                                if let Some(q) = queues.get_mut(&ssrc) {
                                                                                    let phase = ssrc_phases.entry(ssrc).or_insert(0.0);
                                                                                    let hist = ssrc_histories.entry(ssrc).or_insert([((0.0, 0.0)), ((0.0, 0.0)), ((0.0, 0.0)), ((0.0, 0.0))]);

                                                                                    let step = 48000.0f64 / out_sample_rate.max(1) as f64;

                                                                                    *phase += step;
                                                                                    let pops = *phase as usize;
                                                                                    if pops > 0 {
                                                                                        *phase -= pops as f64;
                                                                                        for _ in 0..pops {
                                                                                            hist[0] = hist[1];
                                                                                            hist[1] = hist[2];
                                                                                            hist[2] = hist[3];
                                                                                            if let Some(next_p) = q.pop_front() {
                                                                                                hist[3] = next_p;
                                                                                            } else {
                                                                                                hist[3] = (hist[2].0 * 0.995, hist[2].1 * 0.995);
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                    let t = (*phase as f32).clamp(0.0, 1.0);
                                                                                    let src_l = cubic_hermite(hist[0].0, hist[1].0, hist[2].0, hist[3].0, t);
                                                                                    let src_r = cubic_hermite(hist[0].1, hist[1].1, hist[2].1, hist[3].1, t);

                                                                                    mixed_l += src_l * effective_vol;
                                                                                    mixed_r += src_r * effective_vol;
                                                                                }
                                                                            }

                                                                            let clamped_l = (soft_limit(mixed_l) * 32767.0) as i16;
                                                                            let clamped_r = (soft_limit(mixed_r) * 32767.0) as i16;
                                                                            if out_channels >= 2 {
                                                                                output[f * out_channels + 0] = clamped_l;
                                                                                output[f * out_channels + 1] = clamped_r;
                                                                                for ch in 2..out_channels {
                                                                                    output[f * out_channels + ch] = 0;
                                                                                }
                                                                                    output[f] = (soft_limit((mixed_l + mixed_r) * 0.5) * 32767.0) as i16;
                                                                            }
                                                                        }
                                                                    }
                                                                },
                                                                |err| { warn!("Erro no stream de saída I16: {:?}", err); },
                                                                None,
                                                            )
                                                        }
                                                        _ => {
                                                            warn!("Formato de saída não suportado: {:?}", config.sample_format());
                                                            return;
                                                        }
                                                    };

                                                    match stream_res {
                                                        Ok(stream) => {
                                                            use cpal::traits::StreamTrait;
                                                            if let Err(e) = stream.play() {
                                                                warn!("Falha ao iniciar stream de saída: {:?}", e);
                                                                return;
                                                            }
                                                            info!("🔊 Stream de Saída de Áudio (Speaker) ATIVO! Reproduzindo vozes dos outros usuários...");
                                                            // Keep stream alive until voice session ends (dropping stream stops playback)
                                                            loop {
                                                                std::thread::sleep(std::time::Duration::from_millis(100));
                                                                if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != out_session_id {
                                                                    info!("Stream de saída encerrado (sessão expirada).");
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                        Err(e) => { warn!("Falha ao criar stream de saída: {:?}", e); }
                                                    }
                                                });

                                                tokio::spawn(async move {
                                                let pcm_queue = get_mic_pcm_queue();
                                                let mut seq: u16 = 0;
                                                let mut timestamp: u32 = 0;
                                                let mut nonce_cnt: u32 = 0;
                                                let mut opus_out = vec![0u8; 1000];
                                                let mut speaking_loop_counter: u32 = 0;

                                                // Wait for the secret key (received in op=4) before starting audio
                                                info!("Aguardando chave secreta (op=4) antes de iniciar o áudio...");
                                                loop {
                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id { break; }
                                                    let has_key = secret_key_udp.lock().unwrap().is_some();
                                                    if has_key { break; }
                                                    sleep(Duration::from_millis(50)).await;
                                                }
                                                info!("Chave secreta recebida! Iniciando transmissão de áudio...");

                                                // Flush any stale mic buffer accumulated before voice handshake completed
                                                if let Ok(mut q) = pcm_queue.lock() {
                                                    q.clear();
                                                }

                                                // Stream AES-256-GCM Encrypted Opus microphone audio RTP frames every 20ms over UDP
                                                let mut timer = tokio::time::interval(Duration::from_millis(20));
                                                timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                                                // Send initial 10 silence frames (200ms) to immediately punch UDP NAT hole and activate SFU downstream routing
                                                let mut remaining_silence_frames: usize = 10;
                                                let mut speech_hangover_frames: usize = 0;
                                                let mut silence_keepalive_counter: u32 = 0;

                                                loop {
                                                    timer.tick().await;
                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id {
                                                        info!("Sessão de voz antiga (ID={}) encerrada, saindo do loop UDP!", my_session_id);
                                                        break;
                                                    }

                                                    // Monotonically advance media timestamp on every 20ms tick (RFC 3550 & WebRTC standard)
                                                    timestamp = timestamp.wrapping_add(960);

                                                    let is_muted = *self_mute_rx.lock().unwrap();

                                                    // Extract full 960 f32 PCM samples (20ms of audio at 48000Hz) from microphone buffer
                                                    let mut pcm_frame = [0.0f32; 960];
                                                    let mut has_audio = false;
                                                    {
                                                        if let Ok(mut q) = pcm_queue.lock() {
                                                            if is_muted {
                                                                q.clear();
                                                            } else {
                                                                // Keep queue latency ultra-low (cap at <= 200ms max backlog)
                                                                if q.len() > 960 * 10 {
                                                                    let drop_count = q.len() - 960 * 2;
                                                                    q.drain(0..drop_count);
                                                                }
                                                                // Only extract when full 20ms (960 samples) frame is available
                                                                if q.len() >= 960 {
                                                                    for i in 0..960 {
                                                                        pcm_frame[i] = q.pop_front().unwrap_or(0.0);
                                                                    }
                                                                    // Enforce VAD threshold gating
                                                                    let mut sum_sq = 0.0f32;
                                                                    for &s in pcm_frame.iter() {
                                                                        sum_sq += s * s;
                                                                    }
                                                                    let rms = (sum_sq / 960.0).sqrt();
                                                                    let level = (rms * 6.0).min(1.0);
                                                                    if level >= get_vad_threshold() {
                                                                        has_audio = true;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }

                                                    let is_silence_packet = if has_audio {
                                                        speech_hangover_frames = 12; // 240ms hangover
                                                        remaining_silence_frames = 5;
                                                        silence_keepalive_counter = 0;
                                                        false
                                                    } else if speech_hangover_frames > 0 {
                                                        speech_hangover_frames -= 1;
                                                        false
                                                    } else if remaining_silence_frames > 0 {
                                                        remaining_silence_frames -= 1;
                                                        true
                                                    } else {
                                                        silence_keepalive_counter = silence_keepalive_counter.wrapping_add(1);
                                                        // Send a keepalive silence packet every 2.5 seconds (125 frames) to keep NAT route open
                                                        if silence_keepalive_counter >= 125 {
                                                            silence_keepalive_counter = 0;
                                                            true
                                                        } else {
                                                            continue;
                                                        }
                                                    };

                                                    seq = seq.wrapping_add(1);
                                                    nonce_cnt = nonce_cnt.wrapping_add(1);

                                                    let mut audio_header = [0u8; 12];
                                                    audio_header[0] = 0x80; // RTP Version 2
                                                    audio_header[1] = 0x78; // Opus Payload 120
                                                    audio_header[2..4].copy_from_slice(&seq.to_be_bytes());
                                                    audio_header[4..8].copy_from_slice(&timestamp.to_be_bytes());
                                                    audio_header[8..12].copy_from_slice(&ssrc.to_be_bytes());

                                                    // Prepare 1920 interleaved stereo samples for 48kHz Stereo Opus encoding
                                                    let mut pcm_stereo = [0.0f32; 1920];
                                                    for (i, &s) in pcm_frame.iter().enumerate() {
                                                        pcm_stereo[i * 2] = s;
                                                        pcm_stereo[i * 2 + 1] = s;
                                                    }

                                                    let opus_bytes: &[u8] = if !is_silence_packet {
                                                        if let Ok(encoded_len) = opus_encoder.encode(&pcm_stereo, 960, &mut opus_out) {
                                                            &opus_out[..encoded_len]
                                                        } else {
                                                            &[0xF8, 0xFF, 0xFE] // Silence frame fallback
                                                        }
                                                    } else {
                                                        &[0xF8, 0xFF, 0xFE] // 5x Silence frame trailing pulse
                                                    };

                                                    let key_opt = secret_key_udp.lock().unwrap().clone();

                                                    // Apply DAVE frame encryption if session is ready
                                                    let mut dave_enc_ok = false;
                                                    let dave_encrypted: Option<Vec<u8>> = {
                                                        let mut sess = dave_session_audio.lock().unwrap();
                                                        if let Some(ref mut s) = *sess {
                                                            if s.is_ready() {
                                                                match s.encrypt_opus(opus_bytes) {
                                                                    Ok(cow) => {
                                                                        dave_enc_ok = true;
                                                                        Some(cow.into_owned())
                                                                    }
                                                                    Err(e) => {
                                                                        if speaking_loop_counter % 250 == 0 {
                                                                            warn!("DAVE: Falha em encrypt_opus: {:?}", e);
                                                                        }
                                                                        None
                                                                    }
                                                                }
                                                            } else { None }
                                                        } else { None }
                                                    };
                                                    let opus_payload: &[u8] = if let Some(ref v) = dave_encrypted { v } else { opus_bytes };

                                                    if let Some(key_bytes) = key_opt {
                                                        if let Ok(cipher) = Aes256Gcm::new_from_slice(&key_bytes) {
                                                            // 12-byte AES-GCM Nonce for aead_aes256_gcm_rtpsize:
                                                            let mut nonce_bytes = [0u8; 12];
                                                            nonce_bytes[0..4].copy_from_slice(&nonce_cnt.to_be_bytes());

                                                            let nonce = Nonce::from_slice(&nonce_bytes);

                                                            // Encrypt the (DAVE-encrypted) Opus payload with transport AES-256-GCM
                                                            let payload = Payload {
                                                                msg: opus_payload,
                                                                aad: &audio_header,
                                                            };

                                                            if let Ok(ciphertext) = cipher.encrypt(nonce, payload) {
                                                                let mut rtp_pkt = Vec::with_capacity(12 + ciphertext.len() + 4);
                                                                rtp_pkt.extend_from_slice(&audio_header);
                                                                rtp_pkt.extend_from_slice(&ciphertext);
                                                                // Append 4-byte nonce counter as suffix (rtpsize format)
                                                                rtp_pkt.extend_from_slice(&nonce_cnt.to_be_bytes());

                                                                if let Err(_) = socket.send_to(&rtp_pkt, &target_addr).await {
                                                                    break;
                                                                }

                                                                // Resend OP 5 Speaking and log audio stats every ~5 seconds (every 250 frames)
                                                                speaking_loop_counter = speaking_loop_counter.wrapping_add(1);
                                                                if speaking_loop_counter % 250 == 0 {
                                                                    let q_len = pcm_queue.lock().map(|q| q.len()).unwrap_or(0);
                                                                    info!("TransmissÃ£o de Ãudio: frames_enviados={}, pcm_queue_buffer={}, has_audio={}, dave_encrypted={}",
                                                                        speaking_loop_counter, q_len, has_audio, dave_enc_ok);

                                                                    let speaking_pkt = serde_json::json!({
                                                                        "op": 5,
                                                                        "d": { "speaking": 1, "delay": 0, "ssrc": ssrc }
                                                                    });
                                                                    let mut w = write_arc_proto.lock().await;
                                                                    let _ = w.send(Message::Text(speaking_pkt.to_string().into())).await;
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            });
                                        });
                                    }
                                }
                                4 => {
                                    // Voice Opcode 4: Session Description (Handshake Complete & Secret Key received!)
                                    let ssrc = *active_ssrc.lock().unwrap();
                                    info!("ðŸŽ‰ VOICE GATEWAY OPCODE 4 SESSION DESCRIPTION RECEBIDO! SessÃ£o de Voz Ativada com Sucesso!");

                                    if let Some(key_arr) = val["d"]["secret_key"].as_array() {
                                        let key_bytes: Vec<u8> = key_arr.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect();
                                        if key_bytes.len() == 32 {
                                            info!("Chave de criptografia AES-256 de 32 bytes configurada com SUCESSO para os pacotes de Ã¡udio!");
                                            *secret_key_arc.lock().unwrap() = Some(key_bytes);
                                        }
                                    }

                                    // Send Voice Opcode 5: Speaking
                                    let speaking_pkt = serde_json::json!({
                                        "op": 5,
                                        "d": {
                                            "speaking": 1,
                                            "delay": 0,
                                            "ssrc": ssrc
                                        }
                                    });

                                    info!("Enviando OP 5 Speaking para a Voice Gateway...");
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(speaking_pkt.to_string().into())).await;
                                }
                                 5 => {
                                     // Voice Opcode 5: Speaking notification from another user
                                     let s_ssrc = val["d"]["ssrc"].as_u64().map(|n| n as u32).unwrap_or(0);
                                     let u_id: u64 = if let Some(s) = val["d"]["user_id"].as_str() {
                                         s.parse().unwrap_or(0)
                                     } else {
                                         val["d"]["user_id"].as_u64().unwrap_or(0)
                                     };
                                     if s_ssrc > 0 && u_id > 0 {
                                         ssrc_to_userid.lock().unwrap().insert(s_ssrc, u_id);
                                         register_voice_participant(s_ssrc, u_id);
                                         info!("Voice Gateway OP 5: Mapeado SSRC {} -> User ID {}", s_ssrc, u_id);
                                     }
                                 }
                                 6 => {
                                     // Voice Opcode 6: Heartbeat ACK!
                                     info!("Voice Gateway Heartbeat ACK (Opcode 6) recebido!");
                                 }
                                 11 | 12 => {
                                     // Voice Opcode 11/12: User Joined / Client Connect
                                     if let Some(arr) = val["d"]["user_ids"].as_array() {
                                         for v in arr {
                                             if let Some(uid_str) = v.as_str() {
                                                 if let Ok(uid) = uid_str.parse::<u64>() {
                                                     let ssrc = (uid & 0xFFFFFFFF) as u32;
                                                     register_voice_participant(ssrc, uid);
                                                     info!("Voice Gateway OP 11/12 (user_ids): Registrado participante User ID {}", uid);
                                                 }
                                             }
                                         }
                                     }
                                     let s_ssrc = val["d"]["audio_ssrc"].as_u64().or_else(|| val["d"]["ssrc"].as_u64()).unwrap_or(0) as u32;
                                     let u_id: u64 = if let Some(s) = val["d"]["user_id"].as_str() {
                                         s.parse().unwrap_or(0)
                                     } else {
                                         val["d"]["user_id"].as_u64().unwrap_or(0)
                                     };
                                     if s_ssrc > 0 && u_id > 0 {
                                         ssrc_to_userid.lock().unwrap().insert(s_ssrc, u_id);
                                         register_voice_participant(s_ssrc, u_id);
                                         info!("Voice Gateway OP 11/12: Mapeado SSRC {} -> User ID {}", s_ssrc, u_id);
                                     }
                                 }
                                18 => {
                                    // DAVE Opcode 18: DAVE_PREPARE_TRANSITION (Sent for all active voice channel participants)
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    let protocol_version = val["d"]["protocol_version"].as_u64().unwrap_or(99);
                                    let u_id: u64 = if let Some(s) = val["d"]["user_id"].as_str() {
                                        s.parse().unwrap_or(0)
                                    } else {
                                        val["d"]["user_id"].as_u64().unwrap_or(0)
                                    };
                                    if u_id > 0 {
                                        let ssrc = (u_id & 0xFFFFFFFF) as u32;
                                        register_voice_participant(ssrc, u_id);
                                        info!("👥 Voice Gateway OP 18 DAVE Transition: Registrado participante no canal de voz! User ID {}", u_id);
                                    }
                                    info!("DAVE Prepare Transition (op=18): transition_id={}, protocol_version={}, payload={}",
                                        transition_id, protocol_version, val["d"]);

                                    let ready = serde_json::json!({
                                        "op": 23,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Ready For Transition (op=23) enviado para transition_id={}", transition_id);
                                }
                                19 => {
                                    // DAVE Opcode 19: DAVE_EXECUTE_TRANSITION (Server -> Client)
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    info!("DAVE Execute Transition (op=19): transition_id={}, payload={}", transition_id, val["d"]);
                                    let ssrc = *active_ssrc.lock().unwrap();
                                    let speaking_pkt = serde_json::json!({
                                        "op": 5,
                                        "d": { "speaking": 1, "delay": 0, "ssrc": ssrc }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(speaking_pkt.to_string().into())).await;
                                    info!("DAVE OP 19: Enviado OP 5 Speaking para sincronizar SSRC {} na transição de época!", ssrc);
                                }
                                20 => {
                                    // DAVE Opcode 20: DAVE_TRANSITION_READY
                                }
                                21 => {
                                    // DAVE Opcode 21: DAVE_PREPARE_TRANSITION (Server -> Client)
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    let protocol_version = val["d"]["protocol_version"].as_u64().unwrap_or(0);
                                    info!("DAVE Prepare Transition (op=21): transition_id={}, protocol_version={}", transition_id, protocol_version);
                                    let ready = serde_json::json!({
                                        "op": 23,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Ready For Transition (op=23) enviado para op=21 transition_id={}", transition_id);
                                }
                                22 => {
                                    // DAVE Opcode 22: DAVE_EXECUTE_TRANSITION (Server -> Client)
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    info!("DAVE Execute Transition (op=22): transition_id={}, payload={}", transition_id, val["d"]);
                                    let ssrc = *active_ssrc.lock().unwrap();
                                    let speaking_pkt = serde_json::json!({
                                        "op": 5,
                                        "d": { "speaking": 1, "delay": 0, "ssrc": ssrc }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(speaking_pkt.to_string().into())).await;
                                    info!("DAVE OP 22: Enviado OP 5 Speaking para sincronizar SSRC {} na transição de época!", ssrc);
                                }
                                24 => {
                                    // DAVE Opcode 24: DAVE_PREPARE_EPOCH (Server -> Client)
                                    let epoch = val["d"]["epoch"].as_u64().unwrap_or(0);
                                    let protocol_version = val["d"]["protocol_version"].as_u64().unwrap_or(1);
                                    info!("DAVE Prepare Epoch (op=24): epoch={}, protocol_version={}", epoch, protocol_version);
                                    if epoch == 1 {
                                        let key_pkg_bytes = {
                                            let mut sess = dave_session.lock().unwrap();
                                            if let Some(ref mut s) = *sess {
                                                let _ = s.reset();
                                                if let Ok(mut new_s) = DaveSession::new(
                                                    NonZeroU16::new(protocol_version as u16).unwrap_or(NonZeroU16::new(1).unwrap()),
                                                    uid_num, cid_num, None
                                                ) {
                                                    if let Some(ref ext_bytes) = saved_external_sender.lock().unwrap().as_ref() {
                                                        let _ = new_s.set_external_sender(ext_bytes);
                                                    }
                                                    let kp = new_s.create_key_package().ok();
                                                    *s = new_s;
                                                    kp
                                                } else { None }
                                            } else { None }
                                        };
                                        if let Some(kp) = key_pkg_bytes {
                                            let mut pkt = vec![26u8];
                                            pkt.extend_from_slice(&kp);
                                            let mut w = write_arc.lock().await;
                                            let _ = w.send(Message::Binary(pkt.into())).await;
                                            info!("DAVE: Novo KeyPackage (op=26) gerado e enviado para Epoch Reset (op=24)!");
                                        }
                                    }
                                }
                                _ => {
                                    info!("Voice Gateway opcode JSON ignorado: op={}, data={}", op, val["d"]);
                                }
                            }
                        }
                    }
                    Ok(Message::Binary(data)) => {
                        if data.is_empty() { continue; }
                        let dave_op = data[0];
                        let payload = if data.len() > 1 { &data[1..] } else { &[][..] };

                        let preview: String = data[..data.len().min(16)].iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                        info!("Voice Gateway BINARY: dave_op={}, {} bytes total, preview=[{}]", dave_op, data.len(), preview);

                        match dave_op {
                            25 => {
                                // dave_mls_external_sender_package (25): gateway's MLS credential
                                // Process it, then send our KeyPackage (opcode 26)
                                *saved_external_sender.lock().unwrap() = Some(payload.to_vec());
                                let key_pkg_bytes = {
                                    let mut sess = dave_session.lock().unwrap();
                                    if let Some(ref mut s) = *sess {
                                        match s.set_external_sender(payload) {
                                            Ok(()) => {
                                                info!("DAVE: External sender configurado com sucesso!");
                                                match s.create_key_package() {
                                                    Ok(kp) => { info!("DAVE: KeyPackage gerado ({} bytes)", kp.len()); Some(kp) }
                                                    Err(e) => { warn!("DAVE: Falha ao gerar KeyPackage: {:?}", e); None }
                                                }
                                            }
                                            Err(e) => { warn!("DAVE: Falha ao configurar external sender: {:?}", e); None }
                                        }
                                    } else { None }
                                };
                                if let Some(kp) = key_pkg_bytes {
                                    // Send key package as binary (opcode 26 = dave_mls_key_package)
                                    let mut pkt = vec![26u8];
                                    pkt.extend_from_slice(&kp);
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Binary(pkt.into())).await;
                                    info!("DAVE: KeyPackage (op=26) enviado Ã  Voice Gateway!");
                                }
                            }
                            27 => {
                                // dave_mls_proposals (27): MLS proposals [op_type u8][VLBytes proposals]
                                let op_type = match data.get(1).copied().unwrap_or(0) {
                                    0 => ProposalsOperationType::APPEND,
                                    _ => ProposalsOperationType::REVOKE,
                                };
                                let proposals_data = if data.len() > 2 { &data[2..] } else { &[][..] };
                                info!("DAVE: Proposals (op=27) recebido, type={:?}", op_type);
                                let commit_bytes = {
                                    let mut sess = dave_session.lock().unwrap();
                                    if let Some(ref mut s) = *sess {
                                        match s.process_proposals(op_type, proposals_data, None) {
                                            Ok(Some(cw)) => {
                                                info!("DAVE: CommitWelcome gerado ({} commit bytes, welcome={})",
                                                    cw.commit.len(), cw.welcome.as_ref().map(|w| w.len()).unwrap_or(0));
                                                let mut out = cw.commit.clone();
                                                if let Some(w) = cw.welcome {
                                                    out.push(1u8); // RFC 9420 optional<Welcome> presence flag = 1
                                                    out.extend_from_slice(&w);
                                                } else {
                                                    out.push(0u8); // RFC 9420 optional<Welcome> presence flag = 0
                                                }
                                                Some(out)
                                            }
                                            Ok(None) => { info!("DAVE: process_proposals OK sem commit"); None }
                                            Err(e) => { warn!("DAVE: Falha ao processar proposals: {:?}", e); None }
                                        }
                                    } else { None }
                                };
                                if let Some(commit_data) = commit_bytes {
                                    // Send commit+welcome as binary (opcode 28 = dave_mls_commit_welcome)
                                    let mut pkt = vec![28u8];
                                    pkt.extend_from_slice(&commit_data);
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Binary(pkt.into())).await;
                                    info!("DAVE: CommitWelcome (op=28) enviado Ã  Voice Gateway!");
                                }
                            }
                            29 => {
                                // dave_mls_announce_commit_transition (29): [op: u8 (29)][transition_id: u16 (2 bytes)][commit_bytes]
                                let transition_id = if data.len() >= 3 {
                                    u16::from_be_bytes([data[1], data[2]]) as u64
                                } else { 0 };
                                let commit_payload = if data.len() > 3 { &data[3..] } else { &[][..] };
                                info!("DAVE: AnnounceCommitTransition (op=29) recebido: transition_id={}", transition_id);
                                let commit_ok = {
                                    let mut sess = dave_session.lock().unwrap();
                                    if let Some(ref mut s) = *sess {
                                        match s.process_commit(commit_payload) {
                                            Ok(()) => {
                                                info!("DAVE: Commit processado com sucesso! is_ready={}", s.is_ready());
                                                true
                                            }
                                            Err(e) => {
                                                warn!("DAVE: Falha ao processar commit: {:?}", e);
                                                false
                                            }
                                        }
                                    } else { false }
                                };
                                if commit_ok {
                                    let ready = serde_json::json!({
                                        "op": 23,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Ready For Transition (op=23) enviado para AnnounceCommit transition_id={}", transition_id);
                                }
                            }
                            30 => {
                                // dave_mls_welcome (30): [op: u8 (30)][transition_id: u16 (2 bytes)][welcome_message]
                                let transition_id = if data.len() >= 3 {
                                    u16::from_be_bytes([data[1], data[2]]) as u64
                                } else { 0 };
                                let welcome_payload = if data.len() > 3 { &data[3..] } else { &[][..] };
                                info!("DAVE: Welcome (op=30) recebido! transition_id={}, payload len={}", transition_id, welcome_payload.len());
                                let is_active = {
                                    let mut sess = dave_session.lock().unwrap();
                                    if let Some(ref mut s) = *sess {
                                        match s.process_welcome(welcome_payload) {
                                            Ok(()) => {
                                                info!("ðŸŽ‰ DAVE: Welcome processado com SUCESSO! SessÃ£o ATIVA! is_ready={}", s.is_ready());
                                                true
                                            }
                                            Err(e) => {
                                                warn!("DAVE: Falha ao processar welcome: {:?}", e);
                                                false
                                            }
                                        }
                                    } else { false }
                                };
                                if is_active {
                                    let ready = serde_json::json!({
                                        "op": 23,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Ready For Transition (op=23) enviado para Welcome transition_id={}", transition_id);
                                }
                            }
                            _ => {
                                info!("DAVE: Opcode binÃ¡rio desconhecido: {}", dave_op);
                            }
                        }
                     }
                    Ok(Message::Close(frame)) => {
                        info!("Voice Gateway encerrada normalmente ou deslocada: {:?}", frame);
                        clear_voice_participants();
                        let _ = event_tx_vclose.send(GatewayEvent::VoiceDisconnected).await;
                        break;
                    }
                    Err(e) => {
                        warn!("Encerrando leitura da Voice Gateway: {:?}", e);
                        clear_voice_participants();
                        let _ = event_tx_vclose.send(GatewayEvent::VoiceDisconnected).await;
                        break;
                    }
                    _ => {}
                }
            }
        }
        Err(e) => {
            error!("Falha ao conectar na Voice Gateway: {:?}", e);
        }
    }
}
