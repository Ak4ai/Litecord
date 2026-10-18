//! Test-only remote control for the UI.
//!
//! Compiled only with `--features automation`, and even then it stays off unless
//! `LITECORD_AUTOMATION_PORT` is set. It listens on 127.0.0.1 only: this process
//! holds a Discord token, so the control port must never be reachable from the
//! network (use an SSH tunnel to drive a remote machine).
//!
//! Protocol: one JSON object per line in, one JSON object per line out.
//!
//! ```text
//! {"cmd":"state"}
//! {"cmd":"screenshot","path":"C:/tmp/shot.png"}
//! {"cmd":"click","x":120,"y":300}            logical pixels, see "size"
//! {"cmd":"move","x":120,"y":300}
//! {"cmd":"scroll","x":400,"y":300,"dy":-120}
//! {"cmd":"type","text":"hello"}
//! {"cmd":"key","key":"Return"}               Return, Escape, Tab, Backspace, arrows...
//! {"cmd":"set","prop":"show_settings_modal","value":true}
//! {"cmd":"set_language","code":"de"}
//! {"cmd":"size"}
//! {"cmd":"minimize_to_tray"}                 same as the title bar button
//! {"cmd":"restore"}                          same as clicking the tray icon
//! {"cmd":"open_guild","name":"Server"}       select a server by exact name
//! {"cmd":"open_channel","name":"general"}    select a channel of the open server by exact
//!                                            name (a voice channel joins it); errors if absent
//! {"cmd":"leave_voice"}                      same as the leave button
//! {"cmd":"start_screen_share","res":"1080p","fps":60,"screen":0,"audio":false}
//! {"cmd":"stop_screen_share"}
//! {"cmd":"watch_stream","user_id":"123"}     open a peer's stream in the main view
//! {"cmd":"stream_fullscreen"}                toggle the stream full screen button
//! {"cmd":"resize","width":1920,"height":1080} set the window size in logical pixels
//! {"cmd":"web_share_start"}                  start sharing to browsers; replies with the link
//! {"cmd":"web_share_stop"}
//! ```

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use log::{info, warn};
use serde_json::{json, Value};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, SharedString};

use crate::AppWindow;

/// Starts the control server if `LITECORD_AUTOMATION_PORT` holds a port number.
pub fn start(ui: &AppWindow) {
    let port: u16 = match std::env::var("LITECORD_AUTOMATION_PORT").ok().and_then(|p| p.trim().parse().ok()) {
        Some(p) => p,
        None => return,
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            warn!("🤖 [AUTOMATION] Não foi possível escutar em 127.0.0.1:{}: {:?}", port, e);
            return;
        }
    };
    warn!("🤖 [AUTOMATION] Controle remoto de TESTE ativo em 127.0.0.1:{} (build com --features automation)", port);

    let weak = ui.as_weak();
    let _ = std::thread::Builder::new().name("automation".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let weak = weak.clone();
            let _ = std::thread::Builder::new().name("automation-conn".into()).spawn(move || serve(stream, weak));
        }
    });
}

fn serve(stream: TcpStream, weak: slint::Weak<AppWindow>) {
    let mut out = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    for line in BufReader::new(stream).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => return,
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(cmd) if cmd["cmd"] == "web_share_answer" => {
                match serde_json::from_value(cmd["offer"].clone()) {
                    Ok(offer) => match crate::screen_capture::web_share::answer_manual(offer) {
                        Ok(answer) => json!({"ok":true,"answer":answer}),
                        Err(error) => json!({"ok":false,"error":error}),
                    },
                    Err(e) => json!({"ok":false,"error":e.to_string()}),
                }
            },
            Ok(cmd) => run_on_ui_thread(&weak, cmd),
            Err(e) => json!({ "ok": false, "error": format!("invalid json: {e}") }),
        };
        if writeln!(out, "{reply}").is_err() {
            return;
        }
    }
}

fn run_on_ui_thread(weak: &slint::Weak<AppWindow>, cmd: Value) -> Value {
    let (tx, rx) = mpsc::channel();
    let weak = weak.clone();
    let posted = slint::invoke_from_event_loop(move || {
        let reply = match weak.upgrade() {
            Some(ui) => execute(&ui, &cmd).unwrap_or_else(|e| json!({ "ok": false, "error": e })),
            None => json!({ "ok": false, "error": "window is gone" }),
        };
        let _ = tx.send(reply);
    });
    if posted.is_err() {
        return json!({ "ok": false, "error": "event loop is not running" });
    }
    rx.recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|_| json!({ "ok": false, "error": "timed out waiting for the UI thread" }))
}

fn execute(ui: &AppWindow, cmd: &Value) -> Result<Value, String> {
    let name = cmd["cmd"].as_str().ok_or("missing \"cmd\"")?;
    let window = ui.window();
    match name {
        "size" => {
            let size = window.size();
            let scale = window.scale_factor();
            Ok(json!({
                "ok": true,
                "physical": [size.width, size.height],
                "logical": [size.width as f32 / scale, size.height as f32 / scale],
                "scale": scale,
            }))
        }
        "screenshot" => {
            let path = cmd["path"].as_str().ok_or("missing \"path\"")?;
            let shot = window.take_snapshot().map_err(|e| format!("snapshot failed: {e}"))?;
            image::save_buffer(path, shot.as_bytes(), shot.width(), shot.height(), image::ExtendedColorType::Rgba8)
                .map_err(|e| format!("could not write {path}: {e}"))?;
            Ok(json!({ "ok": true, "path": path, "width": shot.width(), "height": shot.height() }))
        }
        "move" | "click" => {
            let position = position_of(cmd)?;
            window.dispatch_event(WindowEvent::PointerMoved { position });
            if name == "click" {
                let button = PointerEventButton::Left;
                window.dispatch_event(WindowEvent::PointerPressed { position, button });
                window.dispatch_event(WindowEvent::PointerReleased { position, button });
            }
            Ok(json!({ "ok": true }))
        }
        "scroll" => {
            let position = position_of(cmd)?;
            window.dispatch_event(WindowEvent::PointerMoved { position });
            window.dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: cmd["dx"].as_f64().unwrap_or(0.0) as f32,
                delta_y: cmd["dy"].as_f64().unwrap_or(0.0) as f32,
            });
            Ok(json!({ "ok": true }))
        }
        "type" => {
            let text = cmd["text"].as_str().ok_or("missing \"text\"")?;
            for c in text.chars() {
                press(ui, SharedString::from(c.to_string()));
            }
            Ok(json!({ "ok": true }))
        }
        "key" => {
            let key = cmd["key"].as_str().ok_or("missing \"key\"")?;
            let key = match key {
                "Return" | "Enter" => Key::Return,
                "Escape" => Key::Escape,
                "Tab" => Key::Tab,
                "Backspace" => Key::Backspace,
                "Delete" => Key::Delete,
                "Up" => Key::UpArrow,
                "Down" => Key::DownArrow,
                "Left" => Key::LeftArrow,
                "Right" => Key::RightArrow,
                "PageUp" => Key::PageUp,
                "PageDown" => Key::PageDown,
                "Home" => Key::Home,
                "End" => Key::End,
                other => return Err(format!("unknown key {other}")),
            };
            press(ui, SharedString::from(char::from(key).to_string()));
            Ok(json!({ "ok": true }))
        }
        "set" => {
            let prop = cmd["prop"].as_str().ok_or("missing \"prop\"")?;
            let value = &cmd["value"];
            let as_bool = || value.as_bool().ok_or(format!("{prop} needs a boolean"));
            let as_int = || value.as_i64().map(|v| v as i32).ok_or(format!("{prop} needs an integer"));
            match prop {
                "show_settings_modal" => ui.set_show_settings_modal(as_bool()?),
                "settings_active_tab" => ui.set_settings_active_tab(as_int()?),
                "show_stream_modal" => ui.set_show_stream_modal(as_bool()?),
                "show_user_menu" => ui.set_show_user_menu(as_bool()?),
                other => return Err(format!("property {other} is not settable from automation")),
            }
            Ok(json!({ "ok": true }))
        }
        "set_language" => {
            let code = cmd["code"].as_str().ok_or("missing \"code\"")?;
            let lang = crate::utils::i18n::Language::from_code(code);
            crate::ui::helpers::apply_i18n_translations(ui, lang);
            Ok(json!({ "ok": true, "language": lang.code() }))
        }
        "minimize_to_tray" => {
            ui.invoke_minimize_to_tray();
            Ok(json!({ "ok": true }))
        }
        "restore" => {
            crate::ui::helpers::APP_IS_VISIBLE.store(true, std::sync::atomic::Ordering::Relaxed);
            crate::ui::helpers::NEED_UI_REFRESH.store(true, std::sync::atomic::Ordering::Relaxed);
            crate::utils::wake_hardware_monitor();
            crate::ui::helpers::wake_ui_loops();
            use i_slint_backend_winit::WinitWindowAccessor;
            let _ = ui.window().with_winit_window(|w| {
                w.set_visible(true);
                w.set_minimized(false);
            });
            i_slint_backend_winit::redraw_whole_window(ui.window());
            Ok(json!({ "ok": true }))
        }
        "set_full_conversion" => {
            let on = cmd["on"].as_bool().ok_or("missing \"on\"")?;
            crate::utils::test_hooks::FULL_SIZE_CONVERSION.store(on, std::sync::atomic::Ordering::Relaxed);
            Ok(json!({ "ok": true }))
        }
        "set_cpu_decode" => {
            let on = cmd["on"].as_bool().ok_or("missing \"on\"")?;
            crate::utils::test_hooks::CPU_DECODE.store(on, std::sync::atomic::Ordering::Relaxed);
            Ok(json!({ "ok": true }))
        }
        "drop_gateway" => {
            crate::utils::test_hooks::DROP_GATEWAY.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(json!({ "ok": true }))
        }
        "minimize" => {
            // Like the taskbar button: the window stays "visible" but has no area.
            use i_slint_backend_winit::WinitWindowAccessor;
            let _ = ui.window().with_winit_window(|w| w.set_minimized(true));
            Ok(json!({ "ok": true }))
        }
        "popout_stream" => {
            let uid = cmd["user_id"].as_str().ok_or("missing \"user_id\"")?;
            ui.invoke_popout_stream_window(uid.into(), "".into());
            Ok(json!({ "ok": true }))
        }
        "open_guild" => {
            let name = cmd["name"].as_str().ok_or("missing \"name\"")?;
            let guild = ui.get_guilds().iter().find(|g| g.name == name).ok_or(format!("no server named {name}"))?;
            ui.invoke_select_guild(guild.id.clone());
            Ok(json!({ "ok": true, "id": guild.id.to_string() }))
        }
        "open_channel" => {
            let name = cmd["name"].as_str().ok_or("missing \"name\"")?;
            let channel = ui
                .get_channels()
                .iter()
                .find(|c| c.name == name && !c.is_category)
                .ok_or(format!("no channel named {name} in the open server"))?;
            ui.invoke_select_channel(channel.id.clone());
            Ok(json!({ "ok": true, "id": channel.id.to_string(), "is_voice": channel.is_voice }))
        }
        "leave_voice" => {
            ui.invoke_leave_voice();
            Ok(json!({ "ok": true }))
        }
        "start_screen_share" => {
            if ui.get_is_screen_sharing() {
                return Err("already sharing".into());
            }
            ui.set_stream_source_tab("screens".into());
            ui.set_stream_selected_res(cmd["res"].as_str().unwrap_or("1080p").into());
            ui.set_stream_selected_fps(cmd["fps"].as_i64().unwrap_or(60) as i32);
            ui.set_stream_selected_source_index(cmd["screen"].as_i64().unwrap_or(0) as i32);
            ui.set_stream_include_audio(cmd["audio"].as_bool().unwrap_or(false));
            ui.invoke_toggle_screen_share();
            Ok(json!({ "ok": true }))
        }
        "stop_screen_share" => {
            if ui.get_is_screen_sharing() {
                ui.invoke_toggle_screen_share();
            }
            Ok(json!({ "ok": true }))
        }
        "watch_stream" => {
            let uid = cmd["user_id"].as_str().ok_or("missing \"user_id\"")?;
            ui.invoke_focus_stream(uid.into());
            Ok(json!({ "ok": true }))
        }
        "stream_fullscreen" => {
            ui.invoke_toggle_stream_fullscreen();
            Ok(json!({ "ok": true }))
        }
        "resize" => {
            let w = cmd["width"].as_f64().ok_or("missing \"width\"")? as f32;
            let h = cmd["height"].as_f64().ok_or("missing \"height\"")? as f32;
            ui.window().set_size(slint::LogicalSize::new(w, h));
            Ok(json!({ "ok": true }))
        }
        "web_share_start" => {
            if !ui.get_is_screen_sharing() { return Err("start a screen share first".into()); }
            if !ui.get_web_share_active() && !ui.get_web_share_pending() { ui.invoke_toggle_web_share(); }
            Ok(json!({ "ok": true, "pending": ui.get_web_share_pending(), "url": ui.get_web_share_url().to_string() }))
        }
        "web_share_stop" => {
            crate::screen_capture::web_share::stop();
            ui.set_web_share_active(false);
            ui.set_web_share_url("".into());
            Ok(json!({ "ok": true }))
        }
        "state" => Ok(state(ui)),
        other => Err(format!("unknown cmd {other}")),
    }
}

fn position_of(cmd: &Value) -> Result<LogicalPosition, String> {
    let x = cmd["x"].as_f64().ok_or("missing \"x\"")?;
    let y = cmd["y"].as_f64().ok_or("missing \"y\"")?;
    Ok(LogicalPosition::new(x as f32, y as f32))
}

fn press(ui: &AppWindow, text: SharedString) {
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window().dispatch_event(WindowEvent::KeyReleased { text });
}

fn state(ui: &AppWindow) -> Value {
    let peer_streams: Vec<Value> = crate::screen_capture::PEER_STREAMING_STATES
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|m| m.iter().map(|(uid, on)| json!({ "uid": uid.to_string(), "streaming": on })).collect()))
        .unwrap_or_default();
    let peer_addrs: Vec<Value> = crate::screen_capture::LAST_SEEN_PEER_ADDR
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|m| m.iter().map(|(uid, addr)| json!({ "uid": uid.to_string(), "addr": addr.to_string() })).collect()))
        .unwrap_or_default();

    json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "language": ui.get_current_language_code().to_string(),
        "is_logged_in": ui.get_is_logged_in(),
        "connection_status": ui.get_connection_status().to_string(),
        "login_alert_message": ui.get_login_alert_message().to_string(),
        "has_qr_code": ui.get_has_qr_code(),
        "user_tag": ui.get_user_tag().to_string(),
        "user_id": crate::gateway::get_my_user_id().to_string(),
        "active_guild": ui.get_active_guild_name().to_string(),
        "active_channel": ui.get_active_channel_name().to_string(),
        "active_channel_id": ui.get_active_channel_id().to_string(),
        "show_settings_modal": ui.get_show_settings_modal(),
        "settings_active_tab": ui.get_settings_active_tab(),
        "show_stream_modal": ui.get_show_stream_modal(),
        "voice": {
            "is_in_voice": ui.get_is_in_voice(),
            "is_connecting": ui.get_is_voice_connecting(),
            "channel": ui.get_current_voice_channel().to_string(),
            "channel_id": crate::gateway::get_my_voice_channel_id().to_string(),
            "is_muted": ui.get_is_muted(),
            "is_deafened": ui.get_is_deafened(),
        },
        "stream": {
            "web_active": ui.get_web_share_active(),
            "web_pending": ui.get_web_share_pending(),
            "web_url": ui.get_web_share_url().to_string(),
            "web_error": ui.get_web_share_error().to_string(),
            "web_viewers": crate::screen_capture::web_share::viewer_count(),
            "is_screen_sharing": ui.get_is_screen_sharing(),
            "has_active_stream": ui.get_has_active_stream(),
            "active_stream_user": ui.get_active_stream_user().to_string(),
            "remote_fps": ui.get_remote_stream_fps().to_string(),
            "local_fps": ui.get_local_preview_fps().to_string(),
            "frames_shown": crate::screen_capture::RX_FRAMES_SHOWN.load(std::sync::atomic::Ordering::Relaxed),
            "frames_unwatched": crate::screen_capture::RX_FRAMES_UNWATCHED.load(std::sync::atomic::Ordering::Relaxed),
            "frames_gpu": crate::screen_capture::RX_GPU_FRAMES.load(std::sync::atomic::Ordering::Relaxed),
            "peers_streaming": peer_streams,
            "peers_seen": peer_addrs,
        },
    })
}

