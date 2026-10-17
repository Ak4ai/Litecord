//! Keeps the 1 ms Windows timer resolution only while something needs it.
//!
//! A permanent `timeBeginPeriod(1)` makes the whole machine take ~1000 timer
//! interrupts per second even when Litecord is idle in the tray. P2P stream reception
//! and stream playback need the fine resolution; voice does not (its sender is paced
//! by the microphone, see `MIC_FRAME_READY`), and neither does anything else.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the P2P receive thread while at least one remote stream is arriving.
static STREAM_RX_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static GOVERNOR_THREAD: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();

/// Called by the P2P receive thread; a stream starting wakes the governor.
#[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
pub fn set_stream_rx_active(active: bool) {
    let was = STREAM_RX_ACTIVE.swap(active, Ordering::Relaxed);
    #[cfg(target_os = "windows")]
    if active && !was {
        if let Some(t) = GOVERNOR_THREAD.get() {
            t.unpark();
        }
    }
}

/// Holds the 1 ms timer resolution for as long as it lives. Threads that pace
/// themselves with short sleeps take one of these instead of calling
/// `timeBeginPeriod` directly, so every exit path (early `return` included)
/// gives the resolution back.
pub struct FineTimerGuard(());

impl FineTimerGuard {
    pub fn new() -> Self {
        #[cfg(target_os = "windows")]
        unsafe {
            windows_sys::Win32::Media::timeBeginPeriod(1);
        }
        FineTimerGuard(())
    }
}

impl Drop for FineTimerGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        unsafe {
            windows_sys::Win32::Media::timeEndPeriod(1);
        }
    }
}

#[cfg(target_os = "windows")]
pub fn start_timer_resolution_governor() {
    std::thread::Builder::new()
        .name("timer-resolution".to_string())
        .spawn(|| {
            let _ = GOVERNOR_THREAD.set(std::thread::current());
            let mut held = false;
            loop {
                let wanted = STREAM_RX_ACTIVE.load(Ordering::Relaxed)
                    || crate::screen_capture::stream_audio_recent();
                if wanted != held {
                    unsafe {
                        if wanted {
                            windows_sys::Win32::Media::timeBeginPeriod(1);
                        } else {
                            windows_sys::Win32::Media::timeEndPeriod(1);
                        }
                    }
                    held = wanted;
                }
                // While held, poll to notice the stream audio going quiet; otherwise
                // sleep until `set_stream_rx_active` reports a stream.
                if held {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                } else {
                    std::thread::park();
                }
            }
        })
        .ok();
}

#[cfg(not(target_os = "windows"))]
pub fn start_timer_resolution_governor() {}
