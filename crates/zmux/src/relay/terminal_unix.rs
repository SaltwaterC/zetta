//! The POSIX terminal `zosh-server` gives a relay: its size, raw mode, and
//! `SIGWINCH` as the resize signal.

use std::{
    io,
    os::fd::AsRawFd as _,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context as _, Result};

/// The terminal's size in columns and lines, if it has one.
pub(super) fn size() -> Option<(u16, u16)> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: the descriptor is this process's own standard output and the
    // argument is the `winsize` this request writes.
    let result = unsafe { libc::ioctl(io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &raw mut size) };
    (result == 0 && size.ws_col != 0 && size.ws_row != 0).then_some((size.ws_col, size.ws_row))
}

/// The terminal settings this process found, restored when it ends.
///
/// A relay whose stdin is not a terminal — a test, or a pipe — has nothing to
/// put in raw mode and nothing to restore.
pub(super) struct RawMode {
    previous: Option<libc::termios>,
}

impl RawMode {
    pub(super) fn enter() -> io::Result<Self> {
        let descriptor = io::stdin().as_raw_fd();
        // SAFETY: `isatty` only inspects the descriptor.
        if unsafe { libc::isatty(descriptor) } != 1 {
            return Ok(Self { previous: None });
        }
        let mut settings = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: the descriptor is this process's own standard input and
        // `tcgetattr` initializes the structure it is given.
        if unsafe { libc::tcgetattr(descriptor, settings.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `tcgetattr` returned success, so the value is initialized.
        let previous = unsafe { settings.assume_init() };
        let mut raw = previous;
        // SAFETY: `cfmakeraw` only rewrites the structure it is given.
        unsafe { libc::cfmakeraw(&raw mut raw) };
        // SAFETY: the descriptor is this process's own standard input and
        // `raw` is a complete, initialized settings structure.
        if unsafe { libc::tcsetattr(descriptor, libc::TCSANOW, &raw const raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            previous: Some(previous),
        })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let Some(previous) = self.previous else {
            return;
        };
        // SAFETY: the descriptor is this process's own standard input and
        // `previous` is the settings structure read from it.
        unsafe {
            libc::tcsetattr(io::stdin().as_raw_fd(), libc::TCSANOW, &raw const previous);
        }
    }
}

/// Sets `resized` whenever the terminal is resized.
pub(super) fn watch_resizes(resized: Arc<AtomicBool>, _ending: Arc<AtomicBool>) -> Result<()> {
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, resized)
        .context("watching for terminal size changes")?;
    Ok(())
}

/// Where the pane's output is written: the terminal itself.
pub(super) fn output() -> io::Stdout {
    io::stdout()
}
