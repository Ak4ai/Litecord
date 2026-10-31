//! Low-level memory management and heap purging utilities.

extern "C" {
    fn mi_collect(force: bool);
}

/// Explicitly request mimalloc to return free memory pages back to the operating system.
///
/// Should be called after heavy burst allocations (such as the initial Gateway `READY` event
/// and initial server list hydration) without enabling aggressive global purge loops that
/// could cause OS syscall thrashing (madvise/mmap churn).
pub fn purge_unused_memory() {
    log::info!("🧹 [MEM] Solicitando purga de memória livre do heap para o sistema operacional...");
    unsafe {
        mi_collect(true);
    }
}
