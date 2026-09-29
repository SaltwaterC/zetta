//! Where the daemon's `log` output goes.
//!
//! The daemon is started detached, with its stderr on `/dev/null`, so nothing
//! it logs would survive without a file of its own. Until this existed nothing
//! in the process installed a logger at all, and every `log::warn!` in the
//! crate was a no-op — which is how a viewer being dropped, the one event that
//! explained a frozen pane, left no trace anywhere.
//!
//! The file is `daemon.log` in the session directory, which is private to the
//! user already. It is capped: past [`MAX_LOG_BYTES`] it is renamed to
//! `daemon.log.1`, replacing the previous one, so the two together stay under
//! twice that. `ZMUX_LOG` sets the level (`error`, `warn`, `info`, `debug`,
//! `trace` or `off`); the default is `warn`.

use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use log::{LevelFilter, Log, Metadata, Record};

/// Size at which the log is rotated.
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// The environment variable that sets the level.
pub const LOG_LEVEL_VARIABLE: &str = "ZMUX_LOG";

/// Installs the daemon's file logger for `directory`. Best effort: a daemon
/// that cannot open its log still serves sessions, and a second call in the
/// same process (a test, say) leaves the first logger in place.
pub fn init_daemon_log(directory: &Path) {
    let level = level_from(std::env::var(LOG_LEVEL_VARIABLE).ok().as_deref());
    if level == LevelFilter::Off {
        return;
    }
    // The daemon has not created its session directory yet when this runs;
    // on a first start there is nowhere to open the file otherwise. Created
    // exactly as the daemon itself creates it.
    if crate::catalog::create_private_dir(directory).is_err() {
        return;
    }
    let Ok(logger) = FileLogger::open(directory.join("daemon.log"), level) else {
        return;
    };
    // Leaked on purpose: a logger lives as long as the process, and this is
    // what `log::set_boxed_logger` does too, without needing `log`'s `std`
    // feature.
    if log::set_logger(Box::leak(Box::new(logger))).is_ok() {
        log::set_max_level(level);
    }
}

/// The level a `ZMUX_LOG` value names, or `warn` for anything else.
pub(crate) fn level_from(value: Option<&str>) -> LevelFilter {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("off") => LevelFilter::Off,
        Some("error") => LevelFilter::Error,
        Some("info") => LevelFilter::Info,
        Some("debug") => LevelFilter::Debug,
        Some("trace") => LevelFilter::Trace,
        _ => LevelFilter::Warn,
    }
}

pub(crate) struct FileLogger {
    path: PathBuf,
    level: LevelFilter,
    file: Mutex<OpenLog>,
}

struct OpenLog {
    file: File,
    written: u64,
}

impl FileLogger {
    pub(crate) fn open(path: PathBuf, level: LevelFilter) -> std::io::Result<Self> {
        let file = open_append(&path)?;
        let written = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        Ok(Self {
            path,
            level,
            file: Mutex::new(OpenLog { file, written }),
        })
    }

    /// Moves a full log aside and starts a fresh one.
    fn rotate(&self, log: &mut OpenLog) {
        let rotated = self.path.with_extension("log.1");
        if std::fs::rename(&self.path, rotated).is_err() {
            return;
        }
        if let Ok(file) = open_append(&self.path) {
            log.file = file;
            log.written = 0;
        }
    }
}

fn open_append(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_line(SystemTime::now(), std::process::id(), record);
        // A lock poisoned by a panic mid-write loses at most that line; the
        // file itself is still fine to append to.
        let mut log = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if log.written.saturating_add(line.len() as u64) > MAX_LOG_BYTES {
            self.rotate(&mut log);
        }
        if log.file.write_all(line.as_bytes()).is_ok() {
            log.written = log.written.saturating_add(line.len() as u64);
        }
    }

    fn flush(&self) {
        let _ = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .file
            .flush();
    }
}

/// One log line: UTC time, process, level, target, message.
pub(crate) fn format_line(now: SystemTime, process_id: u32, record: &Record<'_>) -> String {
    format!(
        "{} [{process_id}] {:<5} {}: {}\n",
        utc_timestamp(now),
        record.level(),
        record.target(),
        record.args()
    )
}

/// `2026-09-29T19:37:53.123Z`, without a date library.
pub(crate) fn utc_timestamp(now: SystemTime) -> String {
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let second_of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        second_of_day / 3600,
        second_of_day / 60 % 60,
        second_of_day % 60,
        since_epoch.subsec_millis()
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
#[path = "tests/logging.rs"]
mod tests;
