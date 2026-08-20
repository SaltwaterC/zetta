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
fn strip_line_ending(line: &mut String) {
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
}

#[cfg(unix)]
fn read_secret(prompt: &str) -> Result<Zeroizing<String>> {
    use std::io::{BufRead as _, BufReader, Write as _};

    let mut terminal = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("opening the terminal to ask for the session secret")?;
    write!(terminal, "{prompt}").and_then(|()| terminal.flush())?;
    let echo = EchoOff::disable(&terminal)?;
    let mut reader = BufReader::new(
        terminal
            .try_clone()
            .context("duplicating the terminal to read the session secret")?,
    );
    let mut line = Zeroizing::new(String::new());
    let read = reader.read_line(&mut line);
    drop(echo);
    writeln!(terminal).ok();
    read.context("reading the session secret")?;
    strip_line_ending(&mut line);
    Ok(line)
}

/// Turns terminal echo off for as long as it is held, and back on however the
/// caller leaves — including by `?`, which is why this is a guard and not a pair
/// of calls.
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
        hidden.c_lflag &= !libc::ECHO;
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
    let mut stdin = std::io::stdin().lock();
    let mut line = Zeroizing::new(String::new());
    let read = std::io::BufRead::read_line(&mut stdin, &mut line);
    drop(echo);
    writeln!(stdout).ok();
    read.context("reading the session secret")?;
    strip_line_ending(&mut line);
    Ok(line)
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
            CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };
        // SAFETY: the API obtains the current process's standard input handle.
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) }?;
        let mut original = CONSOLE_MODE(0);
        // SAFETY: handle comes from GetStdHandle and original is writable.
        unsafe { GetConsoleMode(handle, &mut original) }?;
        let quiet = original & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT);
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
