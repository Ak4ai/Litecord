//! Scheduling priority for the threads a screen share depends on.
//!
//! With every core busy (a CPU-heavy game, a compile), threads at normal priority wait
//! their turn like any other and the stream drops frames: in a test with the CPU at
//! 100% the viewer fell to ~46 FPS with freezes. One step above normal lets the
//! capture, the encoder and the senders run first; they use little CPU, so the game
//! loses almost nothing.

/// Raises the calling thread one step above normal.
pub fn raise_current_thread() {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
        if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) == 0 {
            log::debug!("SetThreadPriority failed for {:?}", std::thread::current().name());
        }
    }
}
