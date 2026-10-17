//! Test-only counter of Discord gateway traffic and the UI work it causes.
//!
//! The recording functions are no-ops unless the build has `--features gateway-stats`.
//! With the feature, a thread appends one row per event type per minute to
//! `gateway_stats.csv` in the working directory:
//!
//! ```text
//! unix_time,visible,in_voice,kind,name,count,bytes,parse_us,work_us
//! ```
//!
//! `kind` is `gw` for gateway dispatches (`work_us` = time in `handle_event`) and
//! `ui` for closures run on the Slint thread because of an event (`work_us` = time
//! inside the closure).

use std::time::Duration;

#[cfg(feature = "gateway-stats")]
mod imp {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct Row {
        count: u64,
        bytes: u64,
        parse_us: u64,
        work_us: u64,
    }

    fn rows() -> &'static Mutex<HashMap<(&'static str, String), Row>> {
        static ROWS: OnceLock<Mutex<HashMap<(&'static str, String), Row>>> = OnceLock::new();
        ROWS.get_or_init(|| {
            let _ = std::thread::Builder::new().name("gateway-stats".into()).spawn(flush_loop);
            Mutex::new(HashMap::new())
        })
    }

    pub fn add(kind: &'static str, name: &str, bytes: usize, parse: Duration, work: Duration) {
        if let Ok(mut map) = rows().lock() {
            let row = map.entry((kind, name.to_string())).or_default();
            row.count += 1;
            row.bytes += bytes as u64;
            row.parse_us += parse.as_micros() as u64;
            row.work_us += work.as_micros() as u64;
        }
    }

    fn flush_loop() {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open("gateway_stats.csv").ok();
        if let Some(f) = file.as_mut() {
            let _ = writeln!(f, "unix_time,visible,in_voice,kind,name,count,bytes,parse_us,work_us");
        }
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let drained: Vec<_> = match rows().lock() {
                Ok(mut map) => map.drain().collect(),
                Err(_) => continue,
            };
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let visible = crate::ui::helpers::APP_IS_VISIBLE.load(std::sync::atomic::Ordering::Relaxed);
            let in_voice = crate::gateway::types::is_connected_to_voice();
            if let Some(f) = file.as_mut() {
                for ((kind, name), r) in drained {
                    let _ = writeln!(f, "{now},{visible},{in_voice},{kind},{name},{},{},{},{}", r.count, r.bytes, r.parse_us, r.work_us);
                }
                let _ = f.flush();
            }
        }
    }
}

/// Records one gateway dispatch: its payload size, JSON parse time and `handle_event` time.
#[inline]
#[allow(unused_variables)]
pub fn record_gateway(name: &str, bytes: usize, parse: Duration, work: Duration) {
    #[cfg(feature = "gateway-stats")]
    imp::add("gw", name, bytes, parse, work);
}

/// Runs `f` (a closure on the UI thread) and, with the feature, records how long it took.
#[inline]
#[allow(unused_variables)]
pub fn ui_timed<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    #[cfg(feature = "gateway-stats")]
    {
        let start = std::time::Instant::now();
        let out = f();
        imp::add("ui", name, 0, Duration::ZERO, start.elapsed());
        out
    }
    #[cfg(not(feature = "gateway-stats"))]
    f()
}
