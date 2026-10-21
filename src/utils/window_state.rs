use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use log::info;

pub const WINDOW_STATE_FILE: &str = ".litecord_window_state.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub is_maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: Some(100),
            y: Some(100),
            width: 980,
            height: 620,
            is_maximized: false,
        }
    }
}

pub fn load_window_state() -> WindowState {
    if let Ok(data) = std::fs::read_to_string(WINDOW_STATE_FILE) {
        if let Ok(state) = serde_json::from_str::<WindowState>(&data) {
            // Guard against corrupted or zero/negative dimensions
            if state.width >= 400 && state.height >= 300 {
                info!("🪟 Estado e posição da janela carregados: {:?}", state);
                return state;
            }
        }
    }
    WindowState::default()
}

pub fn save_window_state(state: &WindowState) {
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(WINDOW_STATE_FILE, json);
    }
}

static WINDOW_STATE_STORE: OnceLock<Mutex<WindowState>> = OnceLock::new();

pub fn get_window_state_store() -> &'static Mutex<WindowState> {
    WINDOW_STATE_STORE.get_or_init(|| Mutex::new(load_window_state()))
}

pub fn update_window_geometry(
    x: Option<i32>,
    y: Option<i32>,
    width: Option<u32>,
    height: Option<u32>,
    is_maximized: bool,
) {
    let mut store = get_window_state_store().lock().unwrap_or_else(|e| e.into_inner());
    store.is_maximized = is_maximized;

    // Only update non-maximized position and size to avoid saving maximized screen geometry
    if !is_maximized {
        if let (Some(x_pos), Some(y_pos)) = (x, y) {
            // Filter out off-screen coordinates or invalid negative coordinates (-32000 on Windows when minimized)
            if x_pos >= -10000 && y_pos >= -10000 {
                store.x = Some(x_pos);
                store.y = Some(y_pos);
            }
        }
        if let (Some(w), Some(h)) = (width, height) {
            if w >= 400 && h >= 300 {
                store.width = w;
                store.height = h;
            }
        }
    }
    save_window_state(&store);
}
