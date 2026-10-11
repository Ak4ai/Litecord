//! Keeps the 1 ms Windows timer resolution only while something needs it.
//!
//! A permanent `timeBeginPeriod(1)` makes the whole machine take ~1000 timer
//! interrupts per second even when Litecord is idle in the tray. Voice (20 ms Opus
//! pacing), stream playback and P2P stream reception need the fine resolution;
//! everything else works fine on the default tick.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the P2P receive thread while at least one remote stream is arriving.
pub static STREAM_RX_ACTIVE: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
pub fn start_timer_resolution_governor() {
    std::thread::Builder::new()
        .name("timer-resolution".to_string())
        .spawn(|| {
            let mut held = false;
            loop {
                let wanted = crate::gateway::CURRENT_VOICE_SESSION_ID.load(Ordering::Relaxed) != 0
                    || STREAM_RX_ACTIVE.load(Ordering::Relaxed)
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
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        })
        .ok();
}

#[cfg(not(target_os = "windows"))]
pub fn start_timer_resolution_governor() {}
