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
const DEFAULT_POSITION: (f64, f64) = (100.0, 100.0);
const SAVE_AFTER: Duration = Duration::from_secs(1);
/// How much of the title bar must lie on a monitor for a saved position to be used.
const MIN_VISIBLE: (i32, i32) = (120, 24);

thread_local! {
    // Only the UI thread sees window events, so the state lives there.
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
        None => window.set_outer_position(winit::dpi::LogicalPosition::new(DEFAULT_POSITION.0, DEFAULT_POSITION.1)),
    }
    let maximized = saved.is_some_and(|g| g.maximized);
    if maximized {
        window.set_maximized(true);
    }
    maximized
}

/// A move or resize happened: the geometry is read when it is saved, a second later.
/// Read during the events it can be wrong: maximizing reports the maximized size first
/// and the maximized flag only after.
pub fn observe(app: &slint::Weak<AppWindow>) {
    DIRTY.with(|d| d.set(true));
    WINDOW.with(|w| *w.borrow_mut() = Some(app.clone()));
    SAVE_TIMER.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(slint::Timer::default)
            .start(slint::TimerMode::SingleShot, SAVE_AFTER, save_now);
    });
}

/// Writes a pending change at once (closing, going to the tray).
pub fn save_now() {
    if !DIRTY.with(|d| d.replace(false)) {
        return;
    }
    let Some(app) = WINDOW.with(|w| w.borrow().as_ref().and_then(|w| w.upgrade())) else { return };
    let previous = LAST.with(|l| l.get());
    // The UI property is the source of truth for maximize state: on macOS
    // `Window::is_maximized` is a mutating getter for borderless windows, and
    // elsewhere the window event handler keeps it in sync with the OS.
    let maximized = app.get_is_maximized();
    let Some(geometry) = app.window().with_winit_window(|w| current(w, previous, maximized)).flatten() else { return };
    if Some(geometry) != previous {
        LAST.with(|l| l.set(Some(geometry)));
        video_settings::set_window_geometry(geometry);
    }
}

fn current(window: &winit::window::Window, previous: Option<WindowGeometry>, maximized: bool) -> Option<WindowGeometry> {
    if window.is_minimized().unwrap_or(false) {
        return None;
    }
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
