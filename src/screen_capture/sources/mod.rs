#![allow(dead_code)]

pub mod overlay;

#[cfg(windows)]
pub mod windows;

pub mod linux_portal;

pub use overlay::{start_window_border_overlay, stop_window_border_overlay};

#[cfg(windows)]
pub use self::windows::{capture_screen_rgb, list_capturable_windows, list_screens};

#[cfg(not(windows))]
pub use self::linux_portal::{capture_screen_rgb, list_capturable_windows, list_screens};

#[cfg(not(windows))]
pub use self::linux_portal::{PORTAL_CANCELLED, PORTAL_FRAME, PORTAL_INITIALIZED};

#[cfg(target_os = "linux")]
pub use self::linux_portal::{
    draw_test_watermark, get_test_watermark_digit, init_wayland_portal_screencast,
    kill_portal_child, reset_wayland_portal_cancelled, PORTAL_CHILD, PORTAL_LOCAL_CB,
};
