use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use log::info;

pub const VIDEO_SETTINGS_FILE: &str = ".litecord_video_settings.json";

pub fn get_video_settings_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(app_data) = std::env::var("APPDATA") {
            let path = PathBuf::from(app_data).join("Litecord");
            let _ = std::fs::create_dir_all(&path);
            return path.join("video_settings.json");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let path = PathBuf::from(home).join("Library").join("Application Support").join("Litecord");
            let _ = std::fs::create_dir_all(&path);
            return path.join("video_settings.json");
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            let path = PathBuf::from(xdg).join("litecord");
            let _ = std::fs::create_dir_all(&path);
            return path.join("video_settings.json");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let path = PathBuf::from(home).join(".config").join("litecord");
        let _ = std::fs::create_dir_all(&path);
        return path.join("video_settings.json");
    }
    PathBuf::from(VIDEO_SETTINGS_FILE)
}

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
    #[serde(default = "default_theme_mode")]
    pub theme_mode: String,               // "dark", "light", "amoled"
    /// Draw the UI with the GPU (OpenGL) instead of the CPU software renderer. Off by
    /// default: idle in the tray the software renderer costs almost nothing and does not
    /// keep a GPU context (and its driver threads) next to the game.
    #[serde(default)]
    pub use_gpu_renderer: bool,           // default false
    /// Width of the channel list, in logical pixels, as the user dragged it.
    #[serde(default = "default_channel_sidebar_width")]
    pub channel_sidebar_width: f32,
    #[serde(default = "default_noise_suppression")]
    pub noise_suppression: String,        // "advanced", "standard", "none" (default "none")
    #[serde(default)]
    pub echo_cancellation: bool,         // default false
    #[serde(default)]
    pub automatic_gain_control: bool,    // default false
    /// Where the main window was when it last moved or resized. None until it does.
    #[serde(default)]
    pub window: Option<WindowGeometry>,
}

/// Position in physical pixels (a logical position is ambiguous across monitors with
/// different scaling), size in logical pixels. While maximized the size kept is the one
/// the window returns to. Wayland gives no position, so it can be missing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
}

/// Smaller saved sizes are treated as damaged and ignored.
pub const WINDOW_MIN_WIDTH: u32 = 400;
pub const WINDOW_MIN_HEIGHT: u32 = 300;

pub const CHANNEL_SIDEBAR_MIN_WIDTH: f32 = 180.0;
pub const CHANNEL_SIDEBAR_MAX_WIDTH: f32 = 420.0;

fn default_channel_sidebar_width() -> f32 {
    230.0
}

fn default_true() -> bool {
    true
}

fn default_theme_mode() -> String {
    "dark".to_string()
}

fn default_noise_suppression() -> String {
    "none".to_string()
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
            theme_mode: "dark".to_string(),
            use_gpu_renderer: false,
            channel_sidebar_width: default_channel_sidebar_width(),
            noise_suppression: "none".to_string(),
            echo_cancellation: false,
            automatic_gain_control: false,
            window: None,
        }
    }
}

static VIDEO_SETTINGS: OnceLock<Mutex<VideoSettings>> = OnceLock::new();

pub fn get_video_settings_store() -> &'static Mutex<VideoSettings> {
    VIDEO_SETTINGS.get_or_init(|| {
        let path = get_video_settings_path();
        let settings = if let Ok(data) = std::fs::read_to_string(&path)
            .or_else(|_| std::fs::read_to_string(VIDEO_SETTINGS_FILE))
        {
            // Tolerate a BOM (editors on Windows add one) instead of silently resetting.
            serde_json::from_str::<VideoSettings>(data.trim_start_matches('\u{feff}')).unwrap_or_default()
        } else {
            VideoSettings::default()
        };
        info!("⚙️ Configurações de Vídeo e Interface carregadas de {:?}: {:?}", path, settings);
        Mutex::new(settings)
    })
}

pub fn save_video_settings(settings: &VideoSettings) {
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let path = get_video_settings_path();
        let _ = std::fs::write(&path, &json);
        let _ = std::fs::write(VIDEO_SETTINGS_FILE, &json);
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

pub fn get_window_geometry() -> Option<WindowGeometry> {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).window
        .filter(|g| g.width >= WINDOW_MIN_WIDTH && g.height >= WINDOW_MIN_HEIGHT)
}

/// Writes the file only when the geometry actually changed.
pub fn set_window_geometry(geometry: WindowGeometry) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    if store.window != Some(geometry) {
        store.window = Some(geometry);
        save_video_settings(&store);
    }
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
    let mode = get_theme_mode();
    mode == "light"
}

pub fn toggle_use_light_theme() -> bool {
    let current = get_theme_mode();
    let new_mode = if current == "light" { "dark" } else { "light" };
    set_theme_mode(new_mode.to_string());
    new_mode == "light"
}

pub fn get_theme_mode() -> String {
    let store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    if !store.theme_mode.is_empty() {
        store.theme_mode.clone()
    } else if store.use_light_theme {
        "light".to_string()
    } else {
        "dark".to_string()
    }
}

pub fn set_theme_mode(mode: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.use_light_theme = mode == "light";
    store.theme_mode = mode;
    save_video_settings(&store);
}

pub fn get_noise_suppression() -> String {
    let mode = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).noise_suppression.clone();
    if mode == "krisp" {
        "advanced".to_string()
    } else {
        mode
    }
}

pub fn set_noise_suppression(val: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.noise_suppression = val;
    save_video_settings(&store);
}

pub fn get_echo_cancellation() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).echo_cancellation
}

pub fn toggle_echo_cancellation() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.echo_cancellation = !store.echo_cancellation;
    let new_val = store.echo_cancellation;
    save_video_settings(&store);
    new_val
}

pub fn get_automatic_gain_control() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).automatic_gain_control
}

pub fn toggle_automatic_gain_control() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.automatic_gain_control = !store.automatic_gain_control;
    let new_val = store.automatic_gain_control;
    save_video_settings(&store);
    new_val
}

