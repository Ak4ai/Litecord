//! Remembers where the main window was and puts it back there on the next start.
//!
//! Moving or resizing sends an event per mouse step, so nothing is written while that
//! happens: the geometry is read and written once, a second after the last change, or
//! right away when the app closes or goes to the tray.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use slint::ComponentHandle;

use i_slint_backend_winit::WinitWindowAccessor;

use crate::utils::video_settings::{self, WindowGeometry, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH};
use crate::AppWindow;

const DEFAULT_SIZE: (u32, u32) = (980, 620);
/// A window covering this much of its monitor is treated as maximized. Wayland
/// compositors do not always report the maximized state, and the work area can be
/// shorter than the monitor (panels, docks), so compare proportions instead of pixels.
#[cfg(target_os = "linux")]
const MAXIMIZED_WIDTH_RATIO: f64 = 0.98;
#[cfg(target_os = "linux")]
const MAXIMIZED_HEIGHT_RATIO: f64 = 0.85;
#[cfg(not(target_os = "linux"))]
const DEFAULT_POSITION: (f64, f64) = (100.0, 100.0);
const SAVE_AFTER: Duration = Duration::from_secs(1);
/// How much of the title bar must lie on a monitor for a saved position to be used.
const MIN_VISIBLE: (i32, i32) = (120, 24);

thread_local! {
    // Only the UI thread sees window events, so the state lives there.
    static READY: Cell<bool> = const { Cell::new(false) };
    static DIRTY: Cell<bool> = const { Cell::new(false) };
    static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    static LAST: Cell<Option<WindowGeometry>> = const { Cell::new(None) };
    static SAVE_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Sizes and places the window as it was saved, or at the defaults. A saved position
/// that no longer lands on any monitor (one was unplugged) is dropped.
pub fn restore(window: &winit::window::Window) -> bool {
    let saved = video_settings::get_window_geometry();
    LAST.with(|l| l.set(saved));
    // Events from before this (the window appearing where the system put it) are stale.
    DIRTY.with(|d| d.set(false));
    READY.with(|r| r.set(false));
    // Ignore initial asynchronous configure/resize events emitted by the compositor during surface mapping
    slint::Timer::single_shot(Duration::from_millis(800), move || {
        READY.with(|r| r.set(true));
    });

    let (width, height) = saved.map_or(DEFAULT_SIZE, |g| (g.width, g.height));
    let _ = window.request_inner_size(winit::dpi::LogicalSize::new(width as f64, height as f64));
    let position = saved.and_then(|g| Some((g.x?, g.y?))).filter(|&(x, y)| {
        let scale = window.scale_factor();
        let title_width = (width as f64 * scale) as i32;
        window.available_monitors().any(|m| {
            let (mp, ms) = (m.position(), m.size());
            let visible_x = (x + title_width).min(mp.x + ms.width as i32) - x.max(mp.x);
            let visible_y = (y + MIN_VISIBLE.1).min(mp.y + ms.height as i32) - y.max(mp.y);
            visible_x >= MIN_VISIBLE.0 && visible_y >= MIN_VISIBLE.1
        })
    });
    match position {
        Some((x, y)) => window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y)),
        None => {
            #[cfg(not(target_os = "linux"))]
            window.set_outer_position(winit::dpi::LogicalPosition::new(DEFAULT_POSITION.0, DEFAULT_POSITION.1));
        }
    }
    let maximized = saved.is_some_and(|g| g.maximized);
    if maximized {
        window.set_maximized(true);
    }
    maximized
}

/// Registers the window so its geometry can be written on close, even when the
/// compositor never delivered a move or resize event.
pub fn register(app: &slint::Weak<AppWindow>) {
    WINDOW.with(|w| *w.borrow_mut() = Some(app.clone()));
}

/// A move or resize happened: the geometry is read when it is saved, a second later.
/// Read during the events it can be wrong: maximizing reports the maximized size first
/// and the maximized flag only after.
pub fn observe(app: &slint::Weak<AppWindow>) {
    WINDOW.with(|w| *w.borrow_mut() = Some(app.clone()));
    if !READY.with(|r| r.get()) {
        return;
    }
    DIRTY.with(|d| d.set(true));
    SAVE_TIMER.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(slint::Timer::default)
            .start(slint::TimerMode::SingleShot, SAVE_AFTER, save_if_dirty);
    });
}

/// Writes the geometry after a move/resize, once the events have settled.
fn save_if_dirty() {
    if DIRTY.with(|d| d.replace(false)) {
        save_geometry();
    }
}

/// Writes the geometry right away (closing, going to the tray). Some Wayland
/// compositors do not deliver move/resize events, so this cannot depend on `DIRTY`.
pub fn save_now() {
    DIRTY.with(|d| d.set(false));
    save_geometry();
}

fn save_geometry() {
    let Some(app) = WINDOW.with(|w| w.borrow().as_ref().and_then(|w| w.upgrade())) else { return };
    let previous = LAST.with(|l| l.get());
    let Some(geometry) = app.window().with_winit_window(|w| current(w, previous)).flatten() else { return };
    if app.get_is_maximized() != geometry.maximized {
        app.set_is_maximized(geometry.maximized);
    }
    if Some(geometry) != previous {
        LAST.with(|l| l.set(Some(geometry)));
        video_settings::set_window_geometry(geometry);
    }
}

/// Whether the window is maximized. On Wayland the compositor does not always report
/// it, so fall back to comparing the window size with its monitor.
pub fn is_maximized(window: &winit::window::Window) -> bool {
    if window.is_maximized() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(monitor) = window.current_monitor() {
            let monitor_size = monitor.size();
            let inner = window.inner_size();
            let width_ratio = inner.width as f64 / monitor_size.width as f64;
            let height_ratio = inner.height as f64 / monitor_size.height as f64;
            return width_ratio >= MAXIMIZED_WIDTH_RATIO && height_ratio >= MAXIMIZED_HEIGHT_RATIO;
        }
    }
    false
}

fn current(window: &winit::window::Window, previous: Option<WindowGeometry>) -> Option<WindowGeometry> {
    // A hidden window reports no usable area. `is_minimized` is unreliable on
    // Wayland, where it may stay true while the window is on screen.
    let inner = window.inner_size();
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let maximized = is_maximized(window);
    if maximized {
        // Keep the size and place the window goes back to when restored.
        let (width, height) = previous.map_or(DEFAULT_SIZE, |g| (g.width, g.height));
        return Some(WindowGeometry { x: previous.and_then(|g| g.x), y: previous.and_then(|g| g.y), width, height, maximized });
    }
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let (width, height) = (size.width.round() as u32, size.height.round() as u32);
    if width < WINDOW_MIN_WIDTH || height < WINDOW_MIN_HEIGHT {
        return None;
    }
    let position = window.outer_position().ok();
    Some(WindowGeometry {
        x: position.map(|p| p.x).or(previous.and_then(|g| g.x)),
        y: position.map(|p| p.y).or(previous.and_then(|g| g.y)),
        width,
        height,
        maximized,
    })
}
