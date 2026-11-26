//! Reading a secret from a Unix terminal: echo off for the duration, and
//! each typed character masked as it arrives.

use super::*;

pub(super) fn read_secret(prompt: &str) -> Result<Zeroizing<String>> {
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
///
/// It holds its own duplicate of the terminal, so the caller keeps the one it
/// passed free to read and write through while the guard is alive.
pub(super) struct EchoOff {
    pub(super) terminal: std::os::fd::OwnedFd,
    pub(super) original: rustix::termios::Termios,
}

impl EchoOff {
    pub(super) fn disable(terminal: &impl std::os::fd::AsFd) -> Result<Self> {
        use rustix::termios::{
            LocalModes, OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr,
        };

        let terminal = terminal
            .as_fd()
            .try_clone_to_owned()
            .context("duplicating the terminal to hide the session secret")?;
        let original = tcgetattr(&terminal)
            .context("could not read the terminal's settings to hide the session secret")?;
        let mut hidden = original.clone();
        hidden.local_modes &=
            !(LocalModes::ECHO | LocalModes::ICANON | LocalModes::ISIG | LocalModes::IEXTEN);
        hidden.special_codes[SpecialCodeIndex::VMIN] = 1;
        hidden.special_codes[SpecialCodeIndex::VTIME] = 0;
        tcsetattr(&terminal, OptionalActions::Now, &hidden)
            .context("could not hide the session secret as it is typed")?;
        Ok(Self { terminal, original })
    }
}

pub(super) fn read_masked_secret(
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

impl Drop for EchoOff {
    fn drop(&mut self) {
        let _ = rustix::termios::tcsetattr(
            &self.terminal,
            rustix::termios::OptionalActions::Now,
            &self.original,
        );
    }
}

impl SecretLine {
    pub(super) fn handle_char(&mut self, character: char) -> SecretInputAction {
        match character {
            '\r' | '\n' => SecretInputAction::Complete,
            '\u{3}' => SecretInputAction::Cancel,
            '\u{8}' | '\u{7f}' => self.erase(1),
            character if character.is_control() => SecretInputAction::Ignore,
            character => self.append_char(character),
        }
    }
}
