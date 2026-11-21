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

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix::*;
#[cfg(windows)]
mod windows_console;
#[cfg(windows)]
use windows_console::*;

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

/// Reads one optional session secret from standard input for scripts.
///
/// Interactive callers should use [`prompt_for_optional_secret`] so the
/// secret is masked and confirmed. This path is intentionally one-line and
/// deterministic: an empty line leaves the session unprotected.
pub fn read_optional_secret_from_stdin() -> Result<Option<SessionSecret>> {
    let line = read_secret_line(&mut std::io::stdin().lock())
        .context("reading the session secret from standard input")?;
    if line.is_empty() {
        Ok(None)
    } else {
        Ok(Some(SessionSecret::from_zeroizing(line)))
    }
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

/// Reads one line, without its line ending, into memory that is erased.
///
/// Not `read_line`: it appends through a `String` that reallocates as it grows,
/// and a line that is not UTF-8 comes back as an error after the bytes have
/// been dropped unerased. This grows as [`crate::auth::SecretBytes`] does and
/// erases a refused line. The reader's own buffer still holds what it read
/// until it is overwritten; for standard input that buffer is the standard
/// library's, and out of reach.
fn read_secret_line(reader: &mut impl std::io::BufRead) -> Result<Zeroizing<String>> {
    let mut line = crate::auth::SecretBytes::default();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let (taken, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(end) => (end + 1, true),
            None => (available.len(), false),
        };
        line.extend_from_slice(&available[..taken]);
        reader.consume(taken);
        if complete {
            break;
        }
    }
    let mut line = crate::auth::secret_utf8(line.into_inner())?;
    strip_line_ending(&mut line);
    Ok(line)
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
        crate::auth::push_secret_char(&mut self.value, character);
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
                crate::auth::push_secret_char(
                    &mut self.value,
                    char::from_u32(code_point)
                        .expect("a UTF-16 surrogate pair must produce a Unicode scalar"),
                );
                return 1;
            }

            // A malformed pair should not panic or leave the reducer stuck.
            // The console normally supplies valid pairs; replacement keeps
            // the reducer deterministic if an input record is incomplete.
            crate::auth::push_secret_char(&mut self.value, '\u{fffd}');
            completed += 1;
        }

        if (0xd800..=0xdbff).contains(&code_unit) {
            self.pending_high_surrogate = Some(code_unit);
        } else if (0xdc00..=0xdfff).contains(&code_unit) {
            crate::auth::push_secret_char(&mut self.value, '\u{fffd}');
            completed += 1;
        } else {
            crate::auth::push_secret_char(
                &mut self.value,
                char::from_u32(u32::from(code_unit))
                    .expect("a non-surrogate UTF-16 code unit must be a Unicode scalar"),
            );
            completed += 1;
        }
        completed
    }

    fn finish(mut self) -> Zeroizing<String> {
        if self.pending_high_surrogate.take().is_some() {
            crate::auth::push_secret_char(&mut self.value, '\u{fffd}');
        }
        self.value
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

#[cfg(not(any(unix, windows)))]
fn read_secret(_prompt: &str) -> Result<Zeroizing<String>> {
    anyhow::bail!("asking for a session secret is not supported on this platform")
}

#[cfg(test)]
#[path = "tests/secret_prompt.rs"]
mod tests;
