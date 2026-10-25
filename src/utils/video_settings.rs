use std::sync::{Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use log::info;

pub const VIDEO_SETTINGS_FILE: &str = ".litecord_video_settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSettings {
    pub video_encoder: String,             // "auto", "gpu:VVVV:DDDD" (Windows), "openh264", or legacy encoder names
    pub video_capture_backend: String,     // "auto", "printwindow", "bitblt" (Win) / "auto", "portal", "x11" (Linux)
    pub audio_loopback_backend: String,    // "auto", "wasapi_isolated", "cpal" (Win) / "auto", "pulsesrc", "cpal" (Linux)
    pub enable_self_preview_notice: bool, // default true
    #[serde(default = "default_true")]
    pub show_hardware_monitor: bool,      // default true
    #[serde(default)]
    pub use_light_theme: bool,            // default false
    /// Draw the UI with the GPU (OpenGL) instead of the CPU software renderer. Off by
    /// default: idle in the tray the software renderer costs almost nothing and does not
    /// keep a GPU context (and its driver threads) next to the game.
    #[serde(default)]
    pub use_gpu_renderer: bool,           // default false
    /// Width of the channel list, in logical pixels, as the user dragged it.
    #[serde(default = "default_channel_sidebar_width")]
    pub channel_sidebar_width: f32,
}

pub const CHANNEL_SIDEBAR_MIN_WIDTH: f32 = 180.0;
pub const CHANNEL_SIDEBAR_MAX_WIDTH: f32 = 420.0;

fn default_channel_sidebar_width() -> f32 {
    230.0
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
            use_light_theme: false,
            use_gpu_renderer: false,
            channel_sidebar_width: default_channel_sidebar_width(),
        }
    }
}

static VIDEO_SETTINGS: OnceLock<Mutex<VideoSettings>> = OnceLock::new();

pub fn get_video_settings_store() -> &'static Mutex<VideoSettings> {
    VIDEO_SETTINGS.get_or_init(|| {
        let settings = if let Ok(data) = std::fs::read_to_string(VIDEO_SETTINGS_FILE) {
            // Tolerate a BOM (editors on Windows add one) instead of silently resetting.
            serde_json::from_str::<VideoSettings>(data.trim_start_matches('\u{feff}')).unwrap_or_default()
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

pub fn get_channel_sidebar_width() -> f32 {
    let width = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).channel_sidebar_width;
    width.clamp(CHANNEL_SIDEBAR_MIN_WIDTH, CHANNEL_SIDEBAR_MAX_WIDTH)
}

pub fn set_channel_sidebar_width(width: f32) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.channel_sidebar_width = width.clamp(CHANNEL_SIDEBAR_MIN_WIDTH, CHANNEL_SIDEBAR_MAX_WIDTH);
    save_video_settings(&store);
}

pub fn get_use_gpu_renderer() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).use_gpu_renderer
}

/// Takes effect on the next start: the renderer is chosen before the window exists.
pub fn toggle_use_gpu_renderer() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.use_gpu_renderer = !store.use_gpu_renderer;
    let new_val = store.use_gpu_renderer;
    save_video_settings(&store);
    new_val
}

pub fn get_use_light_theme() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).use_light_theme
}

pub fn toggle_use_light_theme() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.use_light_theme = !store.use_light_theme;
    let new_val = store.use_light_theme;
    save_video_settings(&store);
    new_val
}

