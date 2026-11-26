//! Raw mode for the terminal `zetta serial` runs in, so every key reaches the
//! device, restored when the session ends; one implementation per platform.

#[cfg(any(unix, windows))]
use anyhow::Context as _;
use anyhow::Result;

#[cfg(unix)]
pub(crate) struct RawTerminal {
    original: rustix::termios::Termios,
}

#[cfg(unix)]
impl RawTerminal {
    pub(crate) fn enable() -> Result<Option<Self>> {
        use rustix::termios::{OptionalActions, tcgetattr, tcsetattr};

        let stdin = std::io::stdin();
        let Ok(original) = tcgetattr(&stdin) else {
            return Ok(None);
        };
        let mut raw = original.clone();
        raw.make_raw();
        tcsetattr(&stdin, OptionalActions::Now, &raw).context("enabling raw terminal input")?;
        Ok(Some(Self { original }))
    }
}

#[cfg(unix)]
impl Drop for RawTerminal {
    fn drop(&mut self) {
        use rustix::termios::{OptionalActions, tcsetattr};

        let _ = tcsetattr(std::io::stdin(), OptionalActions::Now, &self.original);
    }
}

#[cfg(windows)]
pub(crate) struct RawTerminal {
    handle: windows::Win32::Foundation::HANDLE,
    original: windows::Win32::System::Console::CONSOLE_MODE,
}

#[cfg(windows)]
impl RawTerminal {
    pub(crate) fn enable() -> Result<Option<Self>> {
        use windows::Win32::System::Console::{
            CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };

        // SAFETY: the API obtains the current process's standard input handle.
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) }?;
        let mut original = CONSOLE_MODE(0);
        // SAFETY: handle comes from GetStdHandle and original points to writable storage.
        if unsafe { GetConsoleMode(handle, &mut original) }.is_err() {
            return Ok(None);
        }
        let raw = original & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT);
        // SAFETY: handle comes from GetStdHandle and raw is a valid console mode bitset.
        unsafe { SetConsoleMode(handle, raw) }.context("enabling raw terminal input")?;
        Ok(Some(Self { handle, original }))
    }
}

#[cfg(windows)]
impl Drop for RawTerminal {
    fn drop(&mut self) {
        use windows::Win32::System::Console::SetConsoleMode;

        // SAFETY: handle and original were captured from the active console in enable.
        let _ = unsafe { SetConsoleMode(self.handle, self.original) };
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct RawTerminal;

#[cfg(not(any(unix, windows)))]
impl RawTerminal {
    pub(crate) fn enable() -> Result<Option<Self>> {
        Ok(None)
    }
}
