use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU8, Ordering}};
use std::time::Duration;
use log::info;

pub const KEYBINDS_CONFIG_FILE: &str = ".litecord_keybinds.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct KeybindsConfig {
    pub mute_shortcut: String,
    pub deafen_shortcut: String,
}

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            mute_shortcut: "Ctrl+Shift+M".to_string(),
            deafen_shortcut: "Ctrl+Shift+D".to_string(),
        }
    }
}

pub fn load_persisted_keybinds_config() -> KeybindsConfig {
    if let Ok(data) = std::fs::read_to_string(KEYBINDS_CONFIG_FILE) {
        if let Ok(cfg) = serde_json::from_str::<KeybindsConfig>(&data) {
            return cfg;
        }
    }
    KeybindsConfig::default()
}

pub fn save_persisted_keybinds_config(cfg: &KeybindsConfig) {
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(KEYBINDS_CONFIG_FILE, json);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub vkey: i32,
    pub display: String,
}

impl KeyCombo {
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("None") || trimmed.eq_ignore_ascii_case("Desativado") {
            return None;
        }

        let parts: Vec<&str> = trimmed.split('+').map(|p| p.trim()).collect();
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut key_part = "";

        for p in parts {
            if p.eq_ignore_ascii_case("ctrl") || p.eq_ignore_ascii_case("control") {
                ctrl = true;
            } else if p.eq_ignore_ascii_case("shift") {
                shift = true;
            } else if p.eq_ignore_ascii_case("alt") {
                alt = true;
            } else {
                key_part = p;
            }
        }

        if key_part.is_empty() {
            return None;
        }

        let vkey = name_to_vkey(key_part)?;
        let display = format_combo_display(ctrl, shift, alt, key_part);

        Some(Self {
            ctrl,
            shift,
            alt,
            vkey,
            display,
        })
    }
}

pub fn format_combo_display(ctrl: bool, shift: bool, alt: bool, key_name: &str) -> String {
    let mut parts = Vec::new();
    if ctrl { parts.push("Ctrl"); }
    if shift { parts.push("Shift"); }
    if alt { parts.push("Alt"); }
    let upper = key_name.to_uppercase();
    parts.push(&upper);
    parts.join("+")
}

pub fn name_to_vkey(name: &str) -> Option<i32> {
    let upper = name.to_uppercase();
    match upper.as_str() {
        "A" => Some(0x41), "B" => Some(0x42), "C" => Some(0x43), "D" => Some(0x44),
        "E" => Some(0x45), "F" => Some(0x46), "G" => Some(0x47), "H" => Some(0x48),
        "I" => Some(0x49), "J" => Some(0x4A), "K" => Some(0x4B), "L" => Some(0x4C),
        "M" => Some(0x4D), "N" => Some(0x4E), "O" => Some(0x4F), "P" => Some(0x50),
        "Q" => Some(0x51), "R" => Some(0x52), "S" => Some(0x53), "T" => Some(0x54),
        "U" => Some(0x55), "V" => Some(0x56), "W" => Some(0x57), "X" => Some(0x58),
        "Y" => Some(0x59), "Z" => Some(0x5A),
        "0" => Some(0x30), "1" => Some(0x31), "2" => Some(0x32), "3" => Some(0x33),
        "4" => Some(0x34), "5" => Some(0x35), "6" => Some(0x36), "7" => Some(0x37),
        "8" => Some(0x38), "9" => Some(0x39),
        "F1" => Some(0x70), "F2" => Some(0x71), "F3" => Some(0x72), "F4" => Some(0x73),
        "F5" => Some(0x74), "F6" => Some(0x75), "F7" => Some(0x76), "F8" => Some(0x77),
        "F9" => Some(0x78), "F10" => Some(0x79), "F11" => Some(0x7A), "F12" => Some(0x7B),
        "SPACE" | "ESPAÇO" => Some(0x20),
        "TAB" => Some(0x09),
        "ESCAPE" | "ESC" => Some(0x1B),
        "INSERT" | "INS" => Some(0x2D),
        "DELETE" | "DEL" => Some(0x2E),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" | "PGUP" => Some(0x21),
        "PAGEDOWN" | "PGDN" => Some(0x22),
        "NUMPAD0" | "NUM0" => Some(0x60),
        "NUMPAD1" | "NUM1" => Some(0x61),
        "NUMPAD2" | "NUM2" => Some(0x62),
        "NUMPAD3" | "NUM3" => Some(0x63),
        "NUMPAD4" | "NUM4" => Some(0x64),
        "NUMPAD5" | "NUM5" => Some(0x65),
        "NUMPAD6" | "NUM6" => Some(0x66),
        "NUMPAD7" | "NUM7" => Some(0x67),
        "NUMPAD8" | "NUM8" => Some(0x68),
        "NUMPAD9" | "NUM9" => Some(0x69),
        "NUMPAD_MULTIPLY" | "NUM*" => Some(0x6A),
        "NUMPAD_ADD" | "NUM+" => Some(0x6B),
        "NUMPAD_SUBTRACT" | "NUM-" => Some(0x6D),
        "NUMPAD_DIVIDE" | "NUM/" => Some(0x6F),
        "UP" => Some(0x26),
        "DOWN" => Some(0x28),
        "LEFT" => Some(0x25),
        "RIGHT" => Some(0x27),
        "CAPSLOCK" | "CAPS" => Some(0x14),
        "PAUSE" => Some(0x13),
        "SCROLLLOCK" => Some(0x91),
        "TILDE" | "~" | "`" => Some(0xC0),
        "-" | "MINUS" => Some(0xBD),
        "=" | "PLUS" => Some(0xBB),
        "[" => Some(0xDB),
        "]" => Some(0xDD),
        ";" => Some(0xBA),
        "'" => Some(0xDE),
        "," => Some(0xBC),
        "." => Some(0xBE),
        "/" => Some(0xBF),
        "\\" => Some(0xDC),
        _ => None,
    }
}

pub fn vkey_to_name(vkey: i32) -> &'static str {
    match vkey {
        0x41 => "A", 0x42 => "B", 0x43 => "C", 0x44 => "D",
        0x45 => "E", 0x46 => "F", 0x47 => "G", 0x48 => "H",
        0x49 => "I", 0x4A => "J", 0x4B => "K", 0x4C => "L",
        0x4D => "M", 0x4E => "N", 0x4F => "O", 0x50 => "P",
        0x51 => "Q", 0x52 => "R", 0x53 => "S", 0x54 => "T",
        0x55 => "U", 0x56 => "V", 0x57 => "W", 0x58 => "X",
        0x59 => "Y", 0x5A => "Z",
        0x30 => "0", 0x31 => "1", 0x32 => "2", 0x33 => "3",
        0x34 => "4", 0x35 => "5", 0x36 => "6", 0x37 => "7",
        0x38 => "8", 0x39 => "9",
        0x70 => "F1", 0x71 => "F2", 0x72 => "F3", 0x73 => "F4",
        0x74 => "F5", 0x75 => "F6", 0x76 => "F7", 0x77 => "F8",
        0x78 => "F9", 0x79 => "F10", 0x7A => "F11", 0x7B => "F12",
        0x20 => "Space",
        0x09 => "Tab",
        0x1B => "Esc",
        0x2D => "Insert",
        0x2E => "Delete",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x60 => "NumPad0",
        0x61 => "NumPad1",
        0x62 => "NumPad2",
        0x63 => "NumPad3",
        0x64 => "NumPad4",
        0x65 => "NumPad5",
        0x66 => "NumPad6",
        0x67 => "NumPad7",
        0x68 => "NumPad8",
        0x69 => "NumPad9",
        0x6A => "NumPad*",
        0x6B => "NumPad+",
        0x6D => "NumPad-",
        0x6F => "NumPad/",
        0x26 => "Up",
        0x28 => "Down",
        0x25 => "Left",
        0x27 => "Right",
        0x14 => "CapsLock",
        0x13 => "Pause",
        0x91 => "ScrollLock",
        0xC0 => "~",
        0xBD => "-",
        0xBB => "=",
        0xDB => "[",
        0xDD => "]",
        0xBA => ";",
        0xDE => "'",
        0xBC => ",",
        0xBE => ".",
        0xBF => "/",
        0xDC => "\\",
        _ => "Key",
    }
}

/// 0 = None, 1 = Recording Mute, 2 = Recording Deafen
pub static RECORDING_TARGET: AtomicU8 = AtomicU8::new(0);

pub struct KeybindManager {
    config: Arc<Mutex<KeybindsConfig>>,
    is_running: Arc<AtomicBool>,
}

impl KeybindManager {
    pub fn new(initial_config: KeybindsConfig) -> Self {
        Self {
            config: Arc::new(Mutex::new(initial_config)),
            is_running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn get_config(&self) -> KeybindsConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn update_config(&self, new_cfg: KeybindsConfig) {
        save_persisted_keybinds_config(&new_cfg);
        *self.config.lock().unwrap() = new_cfg;
    }

    pub fn start_global_listener<FMute, FDeaf, FRecordDone>(
        &self,
        on_mute_trigger: FMute,
        on_deafen_trigger: FDeaf,
        on_recorded: FRecordDone,
    ) where
        FMute: Fn() + Send + Sync + 'static,
        FDeaf: Fn() + Send + Sync + 'static,
        FRecordDone: Fn(u8, String) + Send + Sync + 'static,
    {
        #[cfg(target_os = "windows")]
        {
            let config_arc = Arc::clone(&self.config);
            let is_running = Arc::clone(&self.is_running);

            std::thread::Builder::new()
                .name("global-keybinds-listener".to_string())
                .spawn(move || {
                    info!("⌨️ [KEYBINDS] Listener global de atalhos iniciado com sucesso");
                    let mut mute_was_pressed = false;
                    let mut deafen_was_pressed = false;

                    // Windows VK constants
                    const VK_CONTROL: i32 = 0x11;
                    const VK_SHIFT: i32 = 0x10;
                    const VK_MENU: i32 = 0x12; // Alt

                    let is_key_down = |vk: i32| -> bool {
                        unsafe {
                            (windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk) as u16 & 0x8000) != 0
                        }
                    };

                    while is_running.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(20));

                        let rec_target = RECORDING_TARGET.load(Ordering::Relaxed);
                        if rec_target != 0 {
                            // In recording mode: check if any non-modifier key is pressed
                            let ctrl = is_key_down(VK_CONTROL);
                            let shift = is_key_down(VK_SHIFT);
                            let alt = is_key_down(VK_MENU);

                            let candidate_vkeys: &[i32] = &[
                                // A-Z
                                0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C,
                                0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
                                0x59, 0x5A,
                                // 0-9
                                0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
                                // F1-F12
                                0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B,
                                // Numpad
                                0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
                                0x6A, 0x6B, 0x6D, 0x6F,
                                // Special keys
                                0x20, 0x09, 0x1B, 0x2D, 0x2E, 0x24, 0x23, 0x21, 0x22,
                                0x26, 0x28, 0x25, 0x27, 0x14, 0x13, 0x91,
                                0xC0, 0xBD, 0xBB, 0xDB, 0xDD, 0xBA, 0xDE, 0xBC, 0xBE, 0xBF, 0xDC,
                            ];

                            for &vk in candidate_vkeys {
                                if is_key_down(vk) {
                                    // Escape cancels recording
                                    if vk == 0x1B && !ctrl && !shift && !alt {
                                        RECORDING_TARGET.store(0, Ordering::Relaxed);
                                        info!("⌨️ [KEYBINDS] Gravação cancelada via ESC");
                                        on_recorded(rec_target, String::new());
                                        // Wait until release
                                        while is_key_down(0x1B) {
                                            std::thread::sleep(Duration::from_millis(30));
                                        }
                                        break;
                                    }

                                    let key_name = vkey_to_name(vk);
                                    let combo_str = format_combo_display(ctrl, shift, alt, key_name);
                                    info!("⌨️ [KEYBINDS] Gravado com sucesso para target {}: {}", rec_target, combo_str);

                                    // Save new combo
                                    {
                                        let mut cfg = config_arc.lock().unwrap();
                                        if rec_target == 1 {
                                            cfg.mute_shortcut = combo_str.clone();
                                        } else if rec_target == 2 {
                                            cfg.deafen_shortcut = combo_str.clone();
                                        }
                                        save_persisted_keybinds_config(&cfg);
                                    }

                                    RECORDING_TARGET.store(0, Ordering::Relaxed);
                                    on_recorded(rec_target, combo_str);

                                    // Wait until key is released to avoid instant triggering
                                    while is_key_down(vk) || is_key_down(VK_CONTROL) || is_key_down(VK_SHIFT) || is_key_down(VK_MENU) {
                                        std::thread::sleep(Duration::from_millis(30));
                                    }
                                    break;
                                }
                            }
                            continue;
                        }

                        // Normal hotkey detection mode
                        let (mute_combo, deaf_combo) = {
                            let cfg = config_arc.lock().unwrap();
                            (
                                KeyCombo::parse(&cfg.mute_shortcut),
                                KeyCombo::parse(&cfg.deafen_shortcut),
                            )
                        };

                        let ctrl_down = is_key_down(VK_CONTROL);
                        let shift_down = is_key_down(VK_SHIFT);
                        let alt_down = is_key_down(VK_MENU);

                        // Check Mute
                        if let Some(ref combo) = mute_combo {
                            let match_modifiers = (!combo.ctrl || ctrl_down)
                                && (!combo.shift || shift_down)
                                && (!combo.alt || alt_down)
                                && (combo.ctrl == ctrl_down)
                                && (combo.shift == shift_down)
                                && (combo.alt == alt_down);

                            let is_pressed = match_modifiers && is_key_down(combo.vkey);

                            if is_pressed && !mute_was_pressed {
                                info!("🎙️ [KEYBINDS] Atalho Global de Mutar disparado: {}", combo.display);
                                on_mute_trigger();
                            }
                            mute_was_pressed = is_pressed;
                        } else {
                            mute_was_pressed = false;
                        }

                        // Check Deafen
                        if let Some(ref combo) = deaf_combo {
                            let match_modifiers = (!combo.ctrl || ctrl_down)
                                && (!combo.shift || shift_down)
                                && (!combo.alt || alt_down)
                                && (combo.ctrl == ctrl_down)
                                && (combo.shift == shift_down)
                                && (combo.alt == alt_down);

                            let is_pressed = match_modifiers && is_key_down(combo.vkey);

                            if is_pressed && !deafen_was_pressed {
                                info!("🎧 [KEYBINDS] Atalho Global de Ensurdecer disparado: {}", combo.display);
                                on_deafen_trigger();
                            }
                            deafen_was_pressed = is_pressed;
                        } else {
                            deafen_was_pressed = false;
                        }
                    }
                })
                .expect("Falha ao iniciar thread de keybinds global");
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = on_mute_trigger;
            let _ = on_deafen_trigger;
            let _ = on_recorded;
            info!("⌨️ [KEYBINDS] Atalhos globais não suportados nativamente nesta plataforma.");
        }
    }
}
