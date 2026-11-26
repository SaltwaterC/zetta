//! Reading a secret from the Windows console: raw key events rather than a
//! line, because the console's own line editing would echo what is typed.

use super::*;

pub(super) fn read_secret(prompt: &str) -> Result<Zeroizing<String>> {
    use std::io::{IsTerminal as _, Write as _};

    anyhow::ensure!(
        std::io::stdin().is_terminal(),
        "protected session reconnect requires an interactive terminal"
    );
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "{prompt}")?;
    stdout.flush()?;
    let echo = EchoOff::disable()?;
    let read = read_console_secret(echo.handle, &mut stdout);
    drop(echo);
    writeln!(stdout).ok();
    read.context("reading the session secret")
}

pub(super) const VK_BACK: u16 = 0x08;
pub(super) const VK_RETURN: u16 = 0x0d;
pub(super) const VK_C: u16 = b'C' as u16;
pub(super) const CTRL_C: u16 = 0x03;
pub(super) const CTRL_KEY_STATE_MASK: u32 = 0x0004 | 0x0008;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ConsoleKeyEvent {
    pub(super) key_down: bool,
    pub(super) repeat_count: u16,
    pub(super) virtual_key_code: u16,
    pub(super) unicode_char: u16,
    pub(super) control_key_state: u32,
}

impl From<windows::Win32::System::Console::KEY_EVENT_RECORD> for ConsoleKeyEvent {
    fn from(event: windows::Win32::System::Console::KEY_EVENT_RECORD) -> Self {
        // SAFETY: `ReadConsoleInputW` fills the UnicodeChar arm of this union
        // for a key event read through the wide API, and both arms are plain
        // integers over the same initialized bytes, so the read is defined.
        let unicode_char = unsafe { event.uChar.UnicodeChar };
        Self {
            key_down: event.bKeyDown.as_bool(),
            repeat_count: event.wRepeatCount,
            virtual_key_code: event.wVirtualKeyCode,
            unicode_char,
            control_key_state: event.dwControlKeyState,
        }
    }
}

impl SecretLine {
    pub(super) fn handle(&mut self, event: ConsoleKeyEvent) -> SecretInputAction {
        if !event.key_down {
            return SecretInputAction::Ignore;
        }
        if self.is_cancel(event) {
            return SecretInputAction::Cancel;
        }
        if event.virtual_key_code == VK_RETURN {
            return SecretInputAction::Complete;
        }

        let repeat_count = usize::from(event.repeat_count);
        if event.virtual_key_code == VK_BACK {
            return self.erase(repeat_count);
        }
        // Control and modifier keys do not contribute a character of their
        // own. Ctrl-C was handled above because processed input is disabled.
        if repeat_count == 0 || event.unicode_char < 0x20 || event.unicode_char == 0x7f {
            return SecretInputAction::Ignore;
        }

        let mut echoed = 0;
        for _ in 0..repeat_count {
            echoed += self.append_utf16(event.unicode_char);
        }
        if echoed == 0 {
            SecretInputAction::Ignore
        } else {
            SecretInputAction::Echo(echoed)
        }
    }

    pub(super) fn is_cancel(&self, event: ConsoleKeyEvent) -> bool {
        event.control_key_state & CTRL_KEY_STATE_MASK != 0
            && (event.virtual_key_code == VK_C || event.unicode_char == CTRL_C)
    }
}

pub(super) fn read_console_secret(
    handle: windows::Win32::Foundation::HANDLE,
    stdout: &mut impl std::io::Write,
) -> Result<Zeroizing<String>> {
    use windows::Win32::System::Console::{INPUT_RECORD, KEY_EVENT, ReadConsoleInputW};

    let mut line = SecretLine::new();
    loop {
        let mut record = INPUT_RECORD::default();
        let mut events_read = 0;
        // SAFETY: `record` is a writable INPUT_RECORD and `handle` is the
        // console input handle captured by EchoOff.
        unsafe { ReadConsoleInputW(handle, std::slice::from_mut(&mut record), &mut events_read) }?;
        if events_read == 0 || record.EventType != KEY_EVENT as u16 {
            continue;
        }

        // SAFETY: EventType was checked immediately above, so the KeyEvent
        // union arm is the one populated by the console.
        let event = unsafe { record.Event.KeyEvent };
        if render_secret_input_action(stdout, line.handle(event.into()))? {
            return Ok(line.finish());
        }
    }
}

pub(super) fn secret_console_mode(
    original: windows::Win32::System::Console::CONSOLE_MODE,
) -> windows::Win32::System::Console::CONSOLE_MODE {
    use windows::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
    };
    original & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT)
}

pub(super) struct EchoOff {
    pub(super) handle: windows::Win32::Foundation::HANDLE,
    pub(super) original: windows::Win32::System::Console::CONSOLE_MODE,
}

impl EchoOff {
    pub(super) fn disable() -> Result<Self> {
        use windows::Win32::System::Console::{
            CONSOLE_MODE, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };
        // SAFETY: the API obtains the current process's standard input handle.
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) }?;
        let mut original = CONSOLE_MODE(0);
        // SAFETY: handle comes from GetStdHandle and original is writable.
        unsafe { GetConsoleMode(handle, &mut original) }?;
        let quiet = secret_console_mode(original);
        // SAFETY: handle comes from GetStdHandle and quiet is a valid mode bitset.
        unsafe { SetConsoleMode(handle, quiet) }?;
        Ok(Self { handle, original })
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        use windows::Win32::System::Console::SetConsoleMode;
        // SAFETY: handle and original were captured from the active console.
        let _ = unsafe { SetConsoleMode(self.handle, self.original) };
    }
}
