use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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
    #[serde(default = "default_noise_suppression")]
    pub noise_suppression: String,        // "advanced", "standard", "none" (default "none")
    #[serde(default)]
    pub echo_cancellation: bool,         // default false
    #[serde(default)]
    pub automatic_gain_control: bool,    // default false
    /// Share to official Discord clients through native Go Live too.
    #[serde(default)]
    pub go_live: bool,                   // default false
    /// Go Live above the free tier (1080p/60) without Nitro, at the user's risk.
    #[serde(default)]
    pub go_live_unlock: bool,            // default false
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
            use_gpu_renderer: false,
            channel_sidebar_width: default_channel_sidebar_width(),
            noise_suppression: "none".to_string(),
            echo_cancellation: false,
            automatic_gain_control: false,
            go_live: false,
            go_live_unlock: false,
            window: None,
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

static SAVE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WRITTEN_GENERATION: Mutex<u64> = Mutex::new(0);

/// Saves the settings without doing disk I/O under the store's lock: the JSON is built
/// while the guard is held, the guard is released, and only then is the file written.
/// The voice thread and the UI read this store, and a slow write (antivirus scanning the
/// file, a busy disk) must not stall them. Generations keep concurrent saves in order:
/// an older snapshot never overwrites a newer one.
fn persist(store: std::sync::MutexGuard<'_, VideoSettings>) {
    mirror_audio(&store);
    let generation = SAVE_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let json = serde_json::to_string_pretty(&*store).ok();
    drop(store);
    let Some(json) = json else { return };
    let mut written = WRITTEN_GENERATION.lock().unwrap_or_else(|e| e.into_inner());
    if *written < generation {
        let _ = std::fs::write(VIDEO_SETTINGS_FILE, json);
        *written = generation;
    }
}

// Audio processing switches mirrored in atomics. The voice thread reads them for every
// 20 ms frame; reading the store meant taking its lock and cloning a String each time.
const AUDIO_NOT_LOADED: u8 = u8::MAX;
static AUDIO_MODE: AtomicU8 = AtomicU8::new(AUDIO_NOT_LOADED);
static AUDIO_AEC: AtomicBool = AtomicBool::new(false);
static AUDIO_AGC: AtomicBool = AtomicBool::new(false);

fn mirror_audio(settings: &VideoSettings) {
    let mode = match settings.noise_suppression.as_str() {
        "advanced" | "krisp" => 2,
        "standard" => 1,
        _ => 0,
    };
    AUDIO_AEC.store(settings.echo_cancellation, Ordering::Relaxed);
    AUDIO_AGC.store(settings.automatic_gain_control, Ordering::Relaxed);
    AUDIO_MODE.store(mode, Ordering::Release);
}

fn ensure_audio_loaded() {
    if AUDIO_MODE.load(Ordering::Acquire) == AUDIO_NOT_LOADED {
        mirror_audio(&get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()));
    }
}

pub fn get_video_encoder() -> String {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).video_encoder.clone()
}

pub fn set_video_encoder(val: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.video_encoder = val;
    persist(store);
}

pub fn get_video_capture_backend() -> String {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).video_capture_backend.clone()
}

pub fn set_video_capture_backend(val: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.video_capture_backend = val;
    persist(store);
}

pub fn get_audio_loopback_backend() -> String {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).audio_loopback_backend.clone()
}

pub fn set_audio_loopback_backend(val: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.audio_loopback_backend = val;
    persist(store);
}

pub fn get_enable_self_preview_notice() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).enable_self_preview_notice
}

pub fn set_enable_self_preview_notice(val: bool) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.enable_self_preview_notice = val;
    persist(store);
}

pub fn toggle_enable_self_preview_notice() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.enable_self_preview_notice = !store.enable_self_preview_notice;
    let new_val = store.enable_self_preview_notice;
    persist(store);
    new_val
}


pub fn get_go_live() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).go_live
}

pub fn set_go_live(val: bool) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    if store.go_live != val {
        store.go_live = val;
        persist(store);
    }
}

pub fn get_go_live_unlock() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).go_live_unlock
}

pub fn toggle_go_live_unlock() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.go_live_unlock = !store.go_live_unlock;
    let new_val = store.go_live_unlock;
    persist(store);
    new_val
}

pub fn get_show_hardware_monitor() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).show_hardware_monitor
}

pub fn set_show_hardware_monitor(val: bool) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.show_hardware_monitor = val;
    persist(store);
}

pub fn toggle_show_hardware_monitor() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.show_hardware_monitor = !store.show_hardware_monitor;
    let new_val = store.show_hardware_monitor;
    persist(store);
    new_val
}

pub fn get_channel_sidebar_width() -> f32 {
    let width = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).channel_sidebar_width;
    width.clamp(CHANNEL_SIDEBAR_MIN_WIDTH, CHANNEL_SIDEBAR_MAX_WIDTH)
}

pub fn set_channel_sidebar_width(width: f32) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.channel_sidebar_width = width.clamp(CHANNEL_SIDEBAR_MIN_WIDTH, CHANNEL_SIDEBAR_MAX_WIDTH);
    persist(store);
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
        persist(store);
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
    persist(store);
    new_val
}

pub fn get_use_light_theme() -> bool {
    get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner()).use_light_theme
}

pub fn toggle_use_light_theme() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.use_light_theme = !store.use_light_theme;
    let new_val = store.use_light_theme;
    persist(store);
    new_val
}

/// "advanced", "standard" or "none" ("krisp" from older settings reads as "advanced").
pub fn get_noise_suppression() -> &'static str {
    ensure_audio_loaded();
    match AUDIO_MODE.load(Ordering::Relaxed) {
        2 => "advanced",
        1 => "standard",
        _ => "none",
    }
}

pub fn set_noise_suppression(val: String) {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.noise_suppression = val;
    persist(store);
}

pub fn get_echo_cancellation() -> bool {
    ensure_audio_loaded();
    AUDIO_AEC.load(Ordering::Relaxed)
}

pub fn toggle_echo_cancellation() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.echo_cancellation = !store.echo_cancellation;
    let new_val = store.echo_cancellation;
    persist(store);
    new_val
}

pub fn get_automatic_gain_control() -> bool {
    ensure_audio_loaded();
    AUDIO_AGC.load(Ordering::Relaxed)
}

pub fn toggle_automatic_gain_control() -> bool {
    let mut store = get_video_settings_store().lock().unwrap_or_else(|e| e.into_inner());
    store.automatic_gain_control = !store.automatic_gain_control;
    let new_val = store.automatic_gain_control;
    persist(store);
    new_val
}

