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

            let console_msg = format!("[{} {} {}] {}\n", time_str, level_str, record.target(), record.args());
            let file_msg = format!("[{} {} {}] {}\n", time_str, plain_level, record.target(), record.args());

            // 1. Output to attached console
            print!("{}", console_msg);
            let _ = std::io::stdout().flush();

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
        let _ = std::io::stdout().flush();
        if let Ok(mut guard) = self.file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = f.flush();
            }
        }
    }
}

pub fn init_logger() {
    let log_file = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open("litecord_app.log").ok();
    let logger = AppLogger { file: Mutex::new(log_file) };
    let _ = log::set_boxed_logger(Box::new(logger));
    log::set_max_level(log::LevelFilter::Info);
}
