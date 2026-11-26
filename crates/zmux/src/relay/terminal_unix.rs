//! The POSIX terminal `zosh-server` gives a relay: its size, raw mode, and
//! `SIGWINCH` as the resize signal.

use std::{
    io,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context as _, Result};

/// The terminal's size in columns and lines, if it has one.
pub(super) fn size() -> Option<(u16, u16)> {
    let size = rustix::termios::tcgetwinsize(io::stdout()).ok()?;
    (size.ws_col != 0 && size.ws_row != 0).then_some((size.ws_col, size.ws_row))
}

/// The terminal settings this process found, restored when it ends.
///
/// A relay whose stdin is not a terminal — a test, or a pipe — has nothing to
/// put in raw mode and nothing to restore.
pub(super) struct RawMode {
    previous: Option<rustix::termios::Termios>,
}

impl RawMode {
    pub(super) fn enter() -> io::Result<Self> {
        use rustix::termios::{OptionalActions, isatty, tcgetattr, tcsetattr};

        let stdin = io::stdin();
        if !isatty(&stdin) {
            return Ok(Self { previous: None });
        }
        let previous = tcgetattr(&stdin)?;
        let mut raw = previous.clone();
        raw.make_raw();
        tcsetattr(&stdin, OptionalActions::Now, &raw)?;
        Ok(Self {
            previous: Some(previous),
        })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let Some(previous) = &self.previous else {
            return;
        };
        let _ = rustix::termios::tcsetattr(
            io::stdin(),
            rustix::termios::OptionalActions::Now,
            previous,
        );
    }
}

/// Sets `resized` whenever the terminal is resized.
pub(super) fn watch_resizes(resized: Arc<AtomicBool>, _ending: Arc<AtomicBool>) -> Result<()> {
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, resized)
        .context("watching for terminal size changes")?;
    Ok(())
}

/// Where the pane's output is written: the terminal itself. Never fails; the
/// `Result` matches Windows, where the handle is duplicated.
pub(super) fn output() -> io::Result<io::Stdout> {
    Ok(io::stdout())
}
