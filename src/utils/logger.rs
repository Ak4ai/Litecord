use std::sync::Mutex;

pub struct AppLogger {
    pub file: Mutex<Option<std::fs::File>>,
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            use std::io::Write;
            let time_str = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => {
                    let total_secs = d.as_secs();
                    let millis = d.subsec_millis();
                    let secs = total_secs % 60;
                    let mins = (total_secs / 60) % 60;
                    let hours = (total_secs / 3600) % 24;
                    format!("{:02}:{:02}:{:02}.{:03}", hours, mins, secs, millis)
                }
                Err(_) => "00:00:00.000".to_string(),
            };

            let level_str = match record.level() {
                log::Level::Error => "\x1b[31mERROR\x1b[0m",
                log::Level::Warn => "\x1b[33mWARN \x1b[0m",
                log::Level::Info => "\x1b[32mINFO \x1b[0m",
                log::Level::Debug => "\x1b[36mDEBUG\x1b[0m",
                log::Level::Trace => "\x1b[35mTRACE\x1b[0m",
            };

            let plain_level = match record.level() {
                log::Level::Error => "ERROR",
                log::Level::Warn => "WARN ",
                log::Level::Info => "INFO ",
                log::Level::Debug => "DEBUG",
                log::Level::Trace => "TRACE",
            };

            let file_msg = format!("[{} {} {}] {}\n", time_str, plain_level, record.target(), record.args());

            // 1. Output to the attached console, never on the caller's thread: a console
            // paused by a selection, or a pipe nobody reads, would block every thread
            // that logs (UI, voice, stream) until it drains.
            if let Some(console) = console() {
                let console_msg = format!("[{} {} {}] {}\n", time_str, level_str, record.target(), record.args());
                let _ = console.try_send(console_msg);
            }

            // 2. Persist to log file without file-open overhead on hot path
            if let Ok(mut guard) = self.file.lock() {
                if let Some(ref mut f) = *guard {
                    let _ = f.write_all(file_msg.as_bytes());
                }
            }
        }
    }

    fn flush(&self) {
        use std::io::Write;
        if let Ok(mut guard) = self.file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = f.flush();
            }
        }
    }
}

/// Lines waiting for the console; when it stops taking them, new ones are dropped.
const CONSOLE_BACKLOG: usize = 256;

/// The console writer, or None when the process has no standard output (the usual
/// Windows launch, without a terminal).
fn console() -> Option<&'static std::sync::mpsc::SyncSender<String>> {
    static CONSOLE: std::sync::OnceLock<Option<std::sync::mpsc::SyncSender<String>>> = std::sync::OnceLock::new();
    CONSOLE.get_or_init(|| {
        if !has_stdout() {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(CONSOLE_BACKLOG);
        std::thread::Builder::new()
            .name("console-log".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                use std::io::Write;
                let mut out = std::io::stdout();
                for line in rx {
                    let _ = out.write_all(line.as_bytes());
                    let _ = out.flush();
                }
            })
            .ok()?;
        Some(tx)
    }).as_ref()
}

#[cfg(windows)]
fn has_stdout() -> bool {
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    !handle.is_null() && handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
}

#[cfg(not(windows))]
fn has_stdout() -> bool {
    true
}

pub fn init_logger() {
    let log_file = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open("litecord_app.log").ok();
    let logger = AppLogger { file: Mutex::new(log_file) };
    let _ = log::set_boxed_logger(Box::new(logger));
    log::set_max_level(log::LevelFilter::Info);
}

/// Collapses a warning that can fire once per video frame into one line per
/// interval: `hit` counts every occurrence and returns the count to report only
/// when the interval since the last report has passed.
pub struct LogThrottle {
    interval_ms: u64,
    last_ms: std::sync::atomic::AtomicU64,
    pending: std::sync::atomic::AtomicU32,
}

impl LogThrottle {
    pub const fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            last_ms: std::sync::atomic::AtomicU64::new(0),
            pending: std::sync::atomic::AtomicU32::new(0),
        }
    }

    pub fn hit(&self) -> Option<u32> {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        // Offset by the interval so the very first hit is always reported.
        let now = START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + self.interval_ms;
        self.hit_at(now)
    }

    fn hit_at(&self, now_ms: u64) -> Option<u32> {
        use std::sync::atomic::Ordering;
        let count = self.pending.fetch_add(1, Ordering::Relaxed) + 1;
        let last = self.last_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= self.interval_ms
            && self.last_ms.compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            self.pending.fetch_sub(count, Ordering::Relaxed);
            return Some(count);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::LogThrottle;

    #[test]
    fn throttle_reports_first_hit_then_counts_until_interval() {
        let t = LogThrottle::new(1000);
        assert_eq!(t.hit_at(5_000), Some(1));
        assert_eq!(t.hit_at(5_100), None);
        assert_eq!(t.hit_at(5_900), None);
        assert_eq!(t.hit_at(6_000), Some(3));
        assert_eq!(t.hit_at(6_001), None);
    }
}
