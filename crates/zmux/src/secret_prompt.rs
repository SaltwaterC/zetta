//! Asking for a session secret at a terminal.
//!
//! Sharing a session from the command line has to set a secret, because sharing
//! is the moment the session becomes joinable from another process — see
//! [`crate::server`]'s refusal to offer one without a verifier. The prompt lives
//! here rather than in [`crate::auth`], which is deliberately free of platform
//! and terminal dependencies so the daemon and every client can share it.
//!
//! Read from the controlling terminal rather than standard input: the point of
//! asking is that a person is present, and standard input is exactly what a
//! script redirects.

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

#[cfg(not(unix))]
fn read_secret(_prompt: &str) -> Result<Zeroizing<String>> {
    anyhow::bail!("asking for a session secret is not supported on this platform")
}

#[cfg(test)]
#[path = "tests/secret_prompt.rs"]
mod tests;
