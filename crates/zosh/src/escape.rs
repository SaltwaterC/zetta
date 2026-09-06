//! Mosh's `Ctrl-^` prefix state machine.

/// What the endpoint should do with input bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscapeAction {
    /// Forward these bytes to the remote shell.
    Send(Vec<u8>),
    /// Finish the session normally.
    Quit,
    /// Suspend the endpoint where the platform supports job control.
    Suspend,
    /// The prefix is waiting for its command byte.
    Pending,
}

/// The configurable Mosh escape prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EscapeKey {
    key: Option<u8>,
    pass: u8,
    pass_alt: u8,
}

const DEFAULT_KEY: u8 = 0x1e;
const DEFAULT_PASS: u8 = b'^';

impl Default for EscapeKey {
    fn default() -> Self {
        Self {
            key: Some(DEFAULT_KEY),
            pass: DEFAULT_PASS,
            pass_alt: DEFAULT_PASS,
        }
    }
}

impl EscapeKey {
    /// Read the same one-byte `MOSH_ESCAPE_KEY` setting as stock Mosh.
    pub fn from_env(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        if value.is_empty() {
            return Self {
                key: None,
                ..Self::default()
            };
        }
        let bytes = value.as_bytes();
        if bytes.len() != 1 {
            return Self::default();
        }
        Self::from_byte(bytes[0])
    }

    fn from_byte(key: u8) -> Self {
        if key == 0 || key >= 128 || matches!(key, 0x03 | 0x04 | 0x0a | 0x0c | 0x0d) {
            return Self::default();
        }
        let pass = if key < 32 { key + b'@' } else { key };
        let pass_alt = if pass.is_ascii_uppercase() {
            pass.to_ascii_lowercase()
        } else {
            pass
        };
        Self {
            key: Some(key),
            pass,
            pass_alt,
        }
    }

    /// A human-readable description used by the status hint.
    pub fn name(&self) -> Option<String> {
        let key = self.key?;
        Some(if key < 32 {
            format!("Ctrl-{}", self.pass as char)
        } else {
            format!("\"{}\"", key as char)
        })
    }
}

/// Stateful processing for one or more input bytes.
#[derive(Clone, Debug)]
pub struct EscapeState {
    escape: EscapeKey,
    armed: bool,
}

impl EscapeState {
    pub fn new(escape: EscapeKey) -> Self {
        Self {
            escape,
            armed: false,
        }
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn feed(&mut self, byte: u8) -> EscapeAction {
        let Some(key) = self.escape.key else {
            return EscapeAction::Send(vec![byte]);
        };
        if !self.armed {
            if byte == key {
                self.armed = true;
                EscapeAction::Pending
            } else {
                EscapeAction::Send(vec![byte])
            }
        } else {
            self.armed = false;
            match byte {
                b'.' => EscapeAction::Quit,
                0x1a => EscapeAction::Suspend,
                byte if byte == self.escape.pass || byte == self.escape.pass_alt => {
                    EscapeAction::Send(vec![key])
                }
                other => EscapeAction::Send(vec![key, other]),
            }
        }
    }

    /// Process a read as one batch, stopping at the first terminal action.
    pub fn feed_all(&mut self, bytes: &[u8]) -> (Vec<u8>, Option<EscapeAction>) {
        let mut sent = Vec::with_capacity(bytes.len());
        for byte in bytes {
            match self.feed(*byte) {
                EscapeAction::Send(mut bytes) => sent.append(&mut bytes),
                EscapeAction::Pending => {}
                action @ (EscapeAction::Quit | EscapeAction::Suspend) => {
                    return (sent, Some(action));
                }
            }
        }
        (sent, None)
    }
}

#[cfg(test)]
#[path = "tests/escape.rs"]
mod tests;
