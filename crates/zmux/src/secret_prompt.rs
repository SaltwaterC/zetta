//! Asking for a session secret at a terminal.
//!
//! Sharing a session from the command line has to set a secret, because sharing
//! is the moment the session becomes joinable from another process — see
//! [`crate::server`]'s refusal to offer one without a verifier. The prompt lives
//! here rather than in [`crate::auth`], which is deliberately free of platform
//! and terminal dependencies so the daemon and every client can share it.
//!
//! Read from the controlling terminal rather than standard input on Unix: the
//! point of asking is that a person is present, and standard input is exactly
//! what a script redirects.

use anyhow::{Context as _, Result};
use zeroize::Zeroizing;

use crate::auth::SessionSecret;

/// Asks for a secret, twice, so a mistyped one cannot lock a session away.
///
/// `None` when nothing is typed: leaving it empty is how the session is left
/// unprotected, which is the same choice the dialog offers in a window. The
/// confirmation is only asked for once there is something to confirm.
pub fn prompt_for_optional_secret() -> Result<Option<SessionSecret>> {
    let secret = read_secret("Session secret (empty for none): ")?;
    if secret.is_empty() {
        return Ok(None);
    }
    let confirmation = read_secret("Confirm session secret: ")?;
    confirmed(secret, &confirmation).map(Some)
}

/// Asks for the secret needed to open a protected session.
///
/// Reconnect is different from sharing: an empty answer is not a choice to
/// leave the session unprotected, so it is rejected rather than sent as an
/// authentication attempt.
pub fn prompt_for_reconnect_secret() -> Result<SessionSecret> {
    let secret = read_secret("Session secret: ")?;
    anyhow::ensure!(!secret.is_empty(), "session secret must not be empty");
    Ok(SessionSecret::from_zeroizing(secret))
}

/// Reads a masked passphrase for an encrypted age or SSH identity.
pub fn prompt_for_passphrase(prompt: &str) -> Result<Zeroizing<String>> {
    read_secret(prompt)
}

/// What a typed pair has to satisfy to become a session's secret.
///
/// Separate from the reading so the rule is testable without a terminal: a
/// mismatched pair is a typo that would otherwise lock the session away behind
/// something nobody knows.
fn confirmed(secret: Zeroizing<String>, confirmation: &str) -> Result<SessionSecret> {
    anyhow::ensure!(secret.as_str() == confirmation, "the secrets do not match");
    Ok(SessionSecret::from_zeroizing(secret))
}

/// Strips the line ending a terminal read leaves on a typed secret.
///
/// Both endings, and repeatedly: a `\r\n` terminal would otherwise make the
/// secret differ from the same characters typed anywhere else.
#[cfg(test)]
fn strip_line_ending(line: &mut String) {
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
}

#[cfg(unix)]
fn read_secret(prompt: &str) -> Result<Zeroizing<String>> {
    use std::io::Write as _;

    let mut terminal = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        // Deliberately not "session secret": this helper also asks for an
        // identity passphrase, and naming the wrong thing is how the last
        // failure in this area sent someone looking at the wrong key.
        .context("opening the terminal to ask for a secret")?;
    write!(terminal, "{prompt}").and_then(|()| terminal.flush())?;
    let echo = EchoOff::disable(&terminal)?;
    let mut reader = terminal
        .try_clone()
        .context("duplicating the terminal to read the session secret")?;
    let read = read_masked_secret(&mut reader, &mut terminal);
    drop(echo);
    writeln!(terminal).ok();
    read.context("reading the session secret")
}

/// Configures byte-at-a-time, no-echo input for as long as it is held, and
/// restores the terminal however the caller leaves — including by `?`, which
/// is why this is a guard and not a pair of calls.
#[cfg(unix)]
struct EchoOff {
    descriptor: std::os::unix::io::RawFd,
    original: libc::termios,
}

#[cfg(unix)]
impl EchoOff {
    fn disable<T: std::os::unix::io::AsRawFd>(terminal: &T) -> Result<Self> {
        let descriptor = terminal.as_raw_fd();
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        // SAFETY: `descriptor` is a live terminal and `original` is a valid
        // `termios` this call fills in.
        anyhow::ensure!(
            unsafe { libc::tcgetattr(descriptor, &mut original) } == 0,
            "could not read the terminal's settings to hide the session secret"
        );
        let mut hidden = original;
        hidden.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN);
        hidden.c_cc[libc::VMIN] = 1;
        hidden.c_cc[libc::VTIME] = 0;
        // SAFETY: as above, with settings derived from what was just read.
        anyhow::ensure!(
            unsafe { libc::tcsetattr(descriptor, libc::TCSANOW, &hidden) } == 0,
            "could not hide the session secret as it is typed"
        );
        Ok(Self {
            descriptor,
            original,
        })
    }
}

#[cfg(unix)]
fn read_masked_secret(
    reader: &mut impl std::io::Read,
    stdout: &mut impl std::io::Write,
) -> Result<Zeroizing<String>> {
    let mut line = SecretLine::new();
    let mut utf8 = Zeroizing::new(Vec::with_capacity(4));
    let mut escape_sequence = false;
    let mut escape_parameter = false;
    loop {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        let byte = byte[0];

        if escape_sequence {
            if byte == 0x1b {
                escape_parameter = false;
                continue;
            }
            if byte < 0x20 || byte == 0x7f {
                escape_sequence = false;
                escape_parameter = false;
            } else if !escape_parameter {
                if byte == b'[' || byte == b'O' {
                    escape_parameter = true;
                } else {
                    escape_sequence = false;
                }
                continue;
            } else {
                if (0x40..=0x7e).contains(&byte) {
                    escape_sequence = false;
                    escape_parameter = false;
                }
                continue;
            }
        }

        if byte == 0x1b {
            utf8.clear();
            escape_sequence = true;
            continue;
        }
        if !utf8.is_empty() && (byte < 0x20 || byte == 0x7f) {
            utf8.clear();
        }

        let action = if utf8.is_empty() && byte < 0x80 {
            line.handle_char(char::from(byte))
        } else {
            utf8.push(byte);
            match std::str::from_utf8(utf8.as_slice()) {
                Ok(text) => {
                    let character = text
                        .chars()
                        .next()
                        .expect("valid UTF-8 input must contain a character");
                    utf8.clear();
                    line.handle_char(character)
                }
                Err(error) if error.error_len().is_none() && utf8.len() < 4 => continue,
                Err(_) => {
                    utf8.clear();
                    continue;
                }
            }
        };

        if render_secret_input_action(stdout, action)? {
            return Ok(line.finish());
        }
    }
}

#[cfg(unix)]
impl Drop for EchoOff {
    fn drop(&mut self) {
        // SAFETY: restoring settings this guard read from the same descriptor.
        unsafe { libc::tcsetattr(self.descriptor, libc::TCSANOW, &self.original) };
    }
}

#[cfg(windows)]
fn read_secret(prompt: &str) -> Result<Zeroizing<String>> {
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

#[cfg(windows)]
const VK_BACK: u16 = 0x08;

#[cfg(windows)]
const VK_RETURN: u16 = 0x0d;

#[cfg(windows)]
const VK_C: u16 = b'C' as u16;

#[cfg(windows)]
const CTRL_C: u16 = 0x03;

#[cfg(windows)]
const CTRL_KEY_STATE_MASK: u32 = 0x0004 | 0x0008;

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ConsoleKeyEvent {
    key_down: bool,
    repeat_count: u16,
    virtual_key_code: u16,
    unicode_char: u16,
    control_key_state: u32,
}

#[cfg(windows)]
impl From<windows::Win32::System::Console::KEY_EVENT_RECORD> for ConsoleKeyEvent {
    fn from(event: windows::Win32::System::Console::KEY_EVENT_RECORD) -> Self {
        // `ReadConsoleInputW` fills the UnicodeChar arm of this union for a
        // key event read through the wide API.
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

#[cfg(any(unix, windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SecretInputAction {
    Ignore,
    Echo(usize),
    Erase(usize),
    Complete,
    Cancel,
}

#[cfg(any(unix, windows))]
struct SecretLine {
    value: Zeroizing<String>,
    pending_high_surrogate: Option<u16>,
}

#[cfg(any(unix, windows))]
impl SecretLine {
    fn new() -> Self {
        Self {
            value: Zeroizing::new(String::new()),
            pending_high_surrogate: None,
        }
    }

    fn erase(&mut self, repeat_count: usize) -> SecretInputAction {
        if repeat_count == 0 {
            return SecretInputAction::Ignore;
        }

        let mut remaining = repeat_count;
        // A high surrogate has not produced a visible mask yet, so deleting
        // it does not require an erase sequence on the console.
        if self.pending_high_surrogate.take().is_some() {
            remaining -= 1;
        }

        let mut erased = 0;
        for _ in 0..remaining {
            if self.value.pop().is_some() {
                erased += 1;
            } else {
                break;
            }
        }
        if erased == 0 {
            SecretInputAction::Ignore
        } else {
            SecretInputAction::Erase(erased)
        }
    }

    #[cfg(unix)]
    fn append_char(&mut self, character: char) -> SecretInputAction {
        self.value.push(character);
        SecretInputAction::Echo(1)
    }

    /// Appends one UTF-16 code unit and returns the number of Unicode scalar
    /// values completed, which is also the number of mask characters needed.
    #[cfg(windows)]
    fn append_utf16(&mut self, code_unit: u16) -> usize {
        let mut completed = 0;
        if let Some(high) = self.pending_high_surrogate.take() {
            if (0xdc00..=0xdfff).contains(&code_unit) {
                let code_point =
                    0x1_0000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(code_unit) - 0xdc00);
                self.value.push(
                    char::from_u32(code_point)
                        .expect("a UTF-16 surrogate pair must produce a Unicode scalar"),
                );
                return 1;
            }

            // A malformed pair should not panic or leave the reducer stuck.
            // The console normally supplies valid pairs; replacement keeps
            // the reducer deterministic if an input record is incomplete.
            self.value.push('\u{fffd}');
            completed += 1;
        }

        if (0xd800..=0xdbff).contains(&code_unit) {
            self.pending_high_surrogate = Some(code_unit);
        } else if (0xdc00..=0xdfff).contains(&code_unit) {
            self.value.push('\u{fffd}');
            completed += 1;
        } else {
            self.value.push(
                char::from_u32(u32::from(code_unit))
                    .expect("a non-surrogate UTF-16 code unit must be a Unicode scalar"),
            );
            completed += 1;
        }
        completed
    }

    fn finish(mut self) -> Zeroizing<String> {
        if self.pending_high_surrogate.take().is_some() {
            self.value.push('\u{fffd}');
        }
        self.value
    }
}

#[cfg(windows)]
impl SecretLine {
    fn handle(&mut self, event: ConsoleKeyEvent) -> SecretInputAction {
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

    fn is_cancel(&self, event: ConsoleKeyEvent) -> bool {
        event.control_key_state & CTRL_KEY_STATE_MASK != 0
            && (event.virtual_key_code == VK_C || event.unicode_char == CTRL_C)
    }
}

#[cfg(unix)]
impl SecretLine {
    fn handle_char(&mut self, character: char) -> SecretInputAction {
        match character {
            '\r' | '\n' => SecretInputAction::Complete,
            '\u{3}' => SecretInputAction::Cancel,
            '\u{8}' | '\u{7f}' => self.erase(1),
            character if character.is_control() => SecretInputAction::Ignore,
            character => self.append_char(character),
        }
    }
}

#[cfg(any(unix, windows))]
fn render_secret_input_action(
    stdout: &mut impl std::io::Write,
    action: SecretInputAction,
) -> Result<bool> {
    match action {
        SecretInputAction::Ignore => return Ok(false),
        SecretInputAction::Complete => return Ok(true),
        SecretInputAction::Cancel => anyhow::bail!("secret prompt cancelled"),
        SecretInputAction::Echo(count) => {
            for _ in 0..count {
                stdout.write_all(b"*")?;
            }
            stdout.flush()?;
        }
        SecretInputAction::Erase(count) => {
            for _ in 0..count {
                stdout.write_all(b"\x08 \x08")?;
            }
            stdout.flush()?;
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn read_console_secret(
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

#[cfg(windows)]
fn secret_console_mode(
    original: windows::Win32::System::Console::CONSOLE_MODE,
) -> windows::Win32::System::Console::CONSOLE_MODE {
    use windows::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
    };
    original & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT)
}

#[cfg(windows)]
struct EchoOff {
    handle: windows::Win32::Foundation::HANDLE,
    original: windows::Win32::System::Console::CONSOLE_MODE,
}

#[cfg(windows)]
impl EchoOff {
    fn disable() -> Result<Self> {
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

#[cfg(windows)]
impl Drop for EchoOff {
    fn drop(&mut self) {
        use windows::Win32::System::Console::SetConsoleMode;
        // SAFETY: handle and original were captured from the active console.
        let _ = unsafe { SetConsoleMode(self.handle, self.original) };
    }
}

#[cfg(not(any(unix, windows)))]
fn read_secret(_prompt: &str) -> Result<Zeroizing<String>> {
    anyhow::bail!("asking for a session secret is not supported on this platform")
}

#[cfg(test)]
#[path = "tests/secret_prompt.rs"]
mod tests;
