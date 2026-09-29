//! Where the GUI process's `log` output goes.
//!
//! Nothing installed a logger before this, so every `log::warn!` in Zetta and
//! in the Zed crates it builds on was a no-op — including the ones that would
//! have said a shared pane's stream failed and was reattached. Only the GUI
//! installs it: a `zetta` subcommand prints what it has to say itself, and
//! stray warnings in a command's output would be read as its answer.
//!
//! Lines go to stderr, which the desktop session hands to the user journal
//! with its own timestamp, so none is added here. `ZETTA_LOG` sets the level
//! (`error`, `warn`, `info`, `debug`, `trace` or `off`); the default is `warn`.

use std::io::Write as _;

use log::{LevelFilter, Log, Metadata, Record};

/// The environment variable that sets the level.
pub(crate) const LOG_LEVEL_VARIABLE: &str = "ZETTA_LOG";

/// Installs the stderr logger. A second call leaves the first in place.
pub(crate) fn init_gui_log() {
    let level = level_from(std::env::var(LOG_LEVEL_VARIABLE).ok().as_deref());
    if level == LevelFilter::Off {
        return;
    }
    static LOGGER: StderrLogger = StderrLogger;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

/// The level a `ZETTA_LOG` value names, or `warn` for anything else.
fn level_from(value: Option<&str>) -> LevelFilter {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("off") => LevelFilter::Off,
        Some("error") => LevelFilter::Error,
        Some("info") => LevelFilter::Info,
        Some("debug") => LevelFilter::Debug,
        Some("trace") => LevelFilter::Trace,
        _ => LevelFilter::Warn,
    }
}

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        // `log::set_max_level` already filters by level before this is asked.
        true
    }

    fn log(&self, record: &Record<'_>) {
        // One write per line, so lines from different threads do not
        // interleave mid-line in the journal.
        let line = format_line(record);
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

fn format_line(record: &Record<'_>) -> String {
    format!(
        "{:<5} {}: {}\n",
        record.level(),
        record.target(),
        record.args()
    )
}

#[cfg(test)]
#[path = "tests/logging.rs"]
mod tests;
