use std::sync::{Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use log::info;

pub const VIDEO_SETTINGS_FILE: &str = ".litecord_video_settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSettings {
    pub video_encoder: String,             // "auto", "nvenc", "amf", "ffmpeg", "wmf", "openh264"
    pub video_capture_backend: String,     // "auto", "printwindow", "bitblt" (Win) / "auto", "portal", "x11" (Linux)
    pub audio_loopback_backend: String,    // "auto", "wasapi_isolated", "cpal" (Win) / "auto", "pulsesrc", "cpal" (Linux)
    pub enable_self_preview_notice: bool, // default true
    #[serde(default = "default_true")]
    pub show_hardware_monitor: bool,      // default true
}

fn default_true() -> bool {
    true
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            video_encoder: "auto".to_string(),
            video_capture_backend: "auto".to_string(),
            audio_loopback_backend: "auto".to_string(),
            enable_self_preview_notice: true,
            show_hardware_monitor: true,
        }
    }
}

static VIDEO_SETTINGS: OnceLock<Mutex<VideoSettings>> = OnceLock::new();

pub fn get_video_settings_store() -> &'static Mutex<VideoSettings> {
    VIDEO_SETTINGS.get_or_init(|| {
        let settings = if let Ok(data) = std::fs::read_to_string(VIDEO_SETTINGS_FILE) {
            serde_json::from_str::<VideoSettings>(&data).unwrap_or_default()
        } else {
            VideoSettings::default()
        };
        info!("⚙️ Configurações de Vídeo e Interface carregadas: {:?}", settings);
        Mutex::new(settings)
    })
}

pub fn save_video_settings(settings: &VideoSettings) {
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(VIDEO_SETTINGS_FILE, json);
    }
}

pub fn get_video_encoder() -> String {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).video_encoder.clone()
}

pub fn set_video_encoder(val: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.video_encoder = val;
    save_video_settings(&store);
}

pub fn get_video_capture_backend() -> String {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).video_capture_backend.clone()
}

pub fn set_video_capture_backend(val: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.video_capture_backend = val;
    save_video_settings(&store);
}

pub fn get_audio_loopback_backend() -> String {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).audio_loopback_backend.clone()
}

pub fn set_audio_loopback_backend(val: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.audio_loopback_backend = val;
    save_video_settings(&store);
}

pub fn get_enable_self_preview_notice() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).enable_self_preview_notice
}

pub fn set_enable_self_preview_notice(val: bool) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.enable_self_preview_notice = val;
    save_video_settings(&store);
}

pub fn toggle_enable_self_preview_notice() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.enable_self_preview_notice = !store.enable_self_preview_notice;
    let new_val = store.enable_self_preview_notice;
    save_video_settings(&store);
    new_val
}


pub fn get_show_hardware_monitor() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).show_hardware_monitor
}

pub fn set_show_hardware_monitor(val: bool) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.show_hardware_monitor = val;
    save_video_settings(&store);
}

pub fn toggle_show_hardware_monitor() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.show_hardware_monitor = !store.show_hardware_monitor;
    let new_val = store.show_hardware_monitor;
    save_video_settings(&store);
    new_val
}

