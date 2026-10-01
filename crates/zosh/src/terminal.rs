//! Crossterm terminal setup and restoration.

use std::io::{self, IsTerminal as _, Write as _};

#[cfg(unix)]
use std::{mem::MaybeUninit, os::fd::AsRawFd as _, sync::Arc};

use anyhow::{Context as _, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

/// A conservative terminal reset that also undoes modes a remote program can
/// leave enabled. The escape sequence is the portable part of handing the
/// display back; raw-mode ownership is explicit below so Unix cleanup can
/// restore the exact termios snapshot captured for this session.
///
/// The alternate-screen transition is deliberately not part of this sequence.
/// `--no-init` runs on the user's normal screen, so emitting `?1049l` during
/// cleanup would ask the terminal to restore an alternate-screen snapshot that
/// Zosh never created. That can discard the normal screen's scrollback.
const CLOSE_SEQUENCE: &[u8] = concat!(
    "\x1b[?1l",
    "\x1b[0m",
    "\x1b[?25h",
    "\x1b[?1003l",
    "\x1b[?1002l",
    "\x1b[?1001l",
    "\x1b[?1000l",
    "\x1b[?1004l",
    "\x1b[?2004l",
    "\x1b[?1015l",
    "\x1b[?1006l",
    "\x1b[?1005l",
)
.as_bytes();

/// A terminal guard that restores raw mode, cursor state, and the screen once.
pub struct TerminalGuard {
    initialized: bool,
    alternate_screen: bool,
    restored: bool,
    #[cfg(unix)]
    saved_mode: Option<Arc<libc::termios>>,
    /// Whether this guard turned on the console's window input, and so has
    /// to turn it off again.
    #[cfg(windows)]
    added_window_input: bool,
}

#[derive(Clone)]
pub(crate) struct TerminalState {
    initialized: bool,
    alternate_screen: bool,
    #[cfg(unix)]
    saved_mode: Option<Arc<libc::termios>>,
    #[cfg(windows)]
    added_window_input: bool,
}

impl TerminalGuard {
    /// Initialize the terminal according to the Mosh launcher's explicit
    /// `--init` setting.
    pub(crate) fn enter_with_initialization(initialize: bool) -> Result<Self> {
        let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
        if !interactive {
            return Ok(Self {
                initialized: false,
                alternate_screen: false,
                restored: false,
                #[cfg(unix)]
                saved_mode: None,
                #[cfg(windows)]
                added_window_input: false,
            });
        }

        #[cfg(unix)]
        let saved_mode = enter_raw_mode().context("enabling terminal raw mode")?;
        #[cfg(not(unix))]
        terminal::enable_raw_mode().context("enabling terminal raw mode")?;
        #[cfg(windows)]
        let added_window_input = window_input::enable();
        if initialize {
            let mut stdout = io::stdout();
            let initialization = execute!(
                stdout,
                EnterAlternateScreen,
                Clear(ClearType::All),
                MoveTo(0, 0),
                Hide
            )
            .context("initializing terminal display")
            .and_then(|()| stdout.flush().context("flushing terminal initialization"));
            if let Err(error) = initialization {
                let _ = write_close_sequence(&mut stdout, true);
                let _ = execute!(stdout, Show);
                #[cfg(unix)]
                let _ = restore_raw_mode(&saved_mode);
                #[cfg(not(unix))]
                let _ = terminal::disable_raw_mode();
                #[cfg(windows)]
                window_input::restore(added_window_input);
                return Err(error);
            }
        }
        Ok(Self {
            initialized: true,
            alternate_screen: initialize,
            restored: false,
            #[cfg(unix)]
            saved_mode: Some(saved_mode),
            #[cfg(windows)]
            added_window_input,
        })
    }

    /// Restore terminal state. It is deliberately best-effort and idempotent
    /// so it is safe from both normal and panic cleanup paths.
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        if !self.initialized {
            return;
        }
        restore_state(self.state());
    }

    pub(crate) fn state(&self) -> TerminalState {
        TerminalState {
            initialized: self.initialized,
            alternate_screen: self.alternate_screen,
            #[cfg(unix)]
            saved_mode: self.saved_mode.clone(),
            #[cfg(windows)]
            added_window_input: self.added_window_input,
        }
    }

    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized
    }
}

pub(crate) fn restore_state(state: TerminalState) {
    if !state.initialized {
        return;
    }
    let mut stdout = io::stdout();
    let _ = write_close_sequence(&mut stdout, state.alternate_screen);
    let _ = execute!(stdout, Show);
    let _ = stdout.flush();
    #[cfg(unix)]
    if let Some(saved_mode) = state.saved_mode.as_deref() {
        let _ = restore_raw_mode(saved_mode);
    }
    #[cfg(not(unix))]
    let _ = terminal::disable_raw_mode();
    #[cfg(windows)]
    window_input::restore(state.added_window_input);
}

/// The console's `ENABLE_WINDOW_INPUT`, which is what makes it report a
/// resize as an input event at all. crossterm turns that event into
/// `Event::Resize` but does not set the mode, so without this the client
/// could only learn of a resize by asking for the size on a timer.
///
/// The event describes the screen *buffer*, which in a classic console with
/// scrollback is taller than the window, so the client treats it as a cue to
/// read the window size rather than as the size itself.
#[cfg(windows)]
mod window_input {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_WINDOW_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        SetConsoleMode,
    };

    /// Turns window input on, reporting whether it was this call that did.
    pub(super) fn enable() -> bool {
        // SAFETY: plain console calls on this process's own input handle,
        // with `mode` written only on success.
        unsafe {
            let Ok(input) = GetStdHandle(STD_INPUT_HANDLE) else {
                return false;
            };
            let mut mode = CONSOLE_MODE::default();
            if GetConsoleMode(input, &mut mode).is_err() || mode.contains(ENABLE_WINDOW_INPUT) {
                return false;
            }
            SetConsoleMode(input, mode | ENABLE_WINDOW_INPUT).is_ok()
        }
    }

    /// Turns window input back off if [`enable`] turned it on.
    pub(super) fn restore(added: bool) {
        if !added {
            return;
        }
        // SAFETY: as in `enable`.
        unsafe {
            let Ok(input) = GetStdHandle(STD_INPUT_HANDLE) else {
                return;
            };
            let mut mode = CONSOLE_MODE::default();
            if GetConsoleMode(input, &mut mode).is_ok() {
                let _ = SetConsoleMode(input, CONSOLE_MODE(mode.0 & !ENABLE_WINDOW_INPUT.0));
            }
        }
    }
}

/// Put Unix terminal handling under this guard's ownership instead of relying
/// on crossterm's process-global raw-mode snapshot. The snapshot can be
/// replaced by another terminal operation before cleanup; keeping the exact
/// termios value from this session is what guarantees that output processing
/// such as `OPOST` is restored for the shell that launched Zosh.
#[cfg(unix)]
fn enter_raw_mode() -> io::Result<Arc<libc::termios>> {
    let fd = io::stdin().as_raw_fd();
    let mut saved = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: `saved` points to writable storage for libc's termios value and
    // remains alive until the call returns.
    if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr initialized the value on success.
    let saved = unsafe { saved.assume_init() };
    let mut raw = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: `saved` is initialized and `raw` has room for one termios
    // value. A bytewise copy keeps both values independently owned; using
    // `ptr::read` here would move the value and leave the saved snapshot
    // unavailable for cleanup.
    unsafe { std::ptr::copy_nonoverlapping(&saved, raw.as_mut_ptr(), 1) };
    let mut raw = unsafe { raw.assume_init() };
    // SAFETY: `raw` is a valid termios value owned by this function.
    unsafe { libc::cfmakeraw(&mut raw) };
    // SAFETY: `raw` points to a valid termios value and fd is the terminal
    // from which input is read throughout the session.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(Arc::new(saved))
}

#[cfg(unix)]
fn restore_raw_mode(saved: &libc::termios) -> io::Result<()> {
    let fd = io::stdin().as_raw_fd();
    // SAFETY: `saved` is the termios snapshot captured from this terminal and
    // fd is still the session's controlling input terminal during cleanup.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, saved) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn write_close_sequence<W: io::Write>(stdout: &mut W, alternate_screen: bool) -> io::Result<()> {
    stdout.write_all(CLOSE_SEQUENCE)?;
    if alternate_screen {
        execute!(stdout, LeaveAlternateScreen)?;
    }
    stdout.flush()
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Return `(columns, rows)`, using Mosh's automation-friendly fallback when a
/// pipe or a platform terminal cannot report its size.
pub fn size() -> (u16, u16) {
    usable_size(terminal::size().unwrap_or((0, 0)))
}

fn usable_size((columns, rows): (u16, u16)) -> (u16, u16) {
    if columns > 0 && rows > 0 {
        (columns, rows)
    } else {
        (80, 24)
    }
}

/// Return the terminal color count used by the Mosh bootstrap.
pub fn color_count() -> u16 {
    let term = std::env::var("TERM").ok();
    let colorterm = std::env::var("COLORTERM").ok();
    color_count_from_environment(term.as_deref(), colorterm.as_deref())
}

fn color_count_from_environment(term: Option<&str>, colorterm: Option<&str>) -> u16 {
    if colorterm.is_some_and(|value| {
        value.eq_ignore_ascii_case("truecolor") || value.eq_ignore_ascii_case("24bit")
    }) {
        return 1 << 15;
    }

    match term {
        Some(value) if value.contains("256") => 256,
        Some(value) if value.contains("color") => 8,
        _ => 0,
    }
}

#[cfg(test)]
#[path = "tests/terminal.rs"]
mod tests;
