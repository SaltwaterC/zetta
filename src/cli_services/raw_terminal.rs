#[cfg(any(unix, windows))]
use anyhow::Context as _;
use anyhow::Result;

#[cfg(unix)]
pub(crate) struct RawTerminal {
    original: libc::termios,
}

#[cfg(unix)]
impl RawTerminal {
    pub(crate) fn enable() -> Result<Option<Self>> {
        use std::mem::MaybeUninit;

        let mut original = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: stdin remains open for the lifetime of the process and the pointer is writable.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) } != 0 {
            return Ok(None);
        }
        // SAFETY: tcgetattr initialized original after returning zero above.
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        // SAFETY: raw is a valid termios structure owned by this function.
        unsafe { libc::cfmakeraw(&mut raw) };
        // SAFETY: stdin is a valid file descriptor and raw is a valid termios structure.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
            return Err(std::io::Error::last_os_error()).context("enabling raw terminal input");
        }
        Ok(Some(Self { original }))
    }
}

#[cfg(unix)]
impl Drop for RawTerminal {
    fn drop(&mut self) {
        // SAFETY: stdin is a valid file descriptor and original was read from it in enable.
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original) };
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
