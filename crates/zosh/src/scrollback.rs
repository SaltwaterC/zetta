//! The server's absolute clear-scrollback generation, carried inside its
//! authenticated display diff as OSC 777;zosh-clear-scrollback;N ST.
//! Keeping it in each protocol screen (rather than acting on received packets)
//! makes repeated and out-of-order state updates harmless.

const PREFIX: &[u8] = b"777;zosh-clear-scrollback;";
const MAX_MARKER: usize = PREFIX.len() + 20;

#[derive(Clone)]
pub(crate) struct ScrollbackState {
    pub(crate) generation: u64,
    parser: Parser,
    bytes: [u8; MAX_MARKER],
    len: usize,
}

impl Default for ScrollbackState {
    fn default() -> Self {
        Self {
            generation: 0,
            parser: Parser::Ground,
            bytes: [0; MAX_MARKER],
            len: 0,
        }
    }
}

#[derive(Clone, Copy, Default)]
enum Parser {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEnd,
    IgnoreOsc,
    IgnoreOscEnd,
    IgnoreString,
    IgnoreEnd,
}

impl ScrollbackState {
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.parser = match self.parser {
                Parser::Ground if byte == 0x1b => Parser::Escape,
                Parser::Escape => match byte {
                    b']' => {
                        self.len = 0;
                        Parser::Osc
                    }
                    b'P' | b'X' | b'^' | b'_' => Parser::IgnoreString,
                    0x1b => Parser::Escape,
                    _ => Parser::Ground,
                },
                Parser::Osc if byte == 7 => {
                    self.finish();
                    Parser::Ground
                }
                Parser::Osc if byte == 0x1b => Parser::OscEnd,
                Parser::Osc if byte == 0x18 || byte == 0x1a => Parser::Ground,
                Parser::Osc if self.len < self.bytes.len() => {
                    self.bytes[self.len] = byte;
                    self.len += 1;
                    Parser::Osc
                }
                Parser::Osc => Parser::IgnoreOsc,
                Parser::OscEnd if byte == b'\\' => {
                    self.finish();
                    Parser::Ground
                }
                Parser::IgnoreOsc if byte == 0x1b => Parser::IgnoreOscEnd,
                Parser::IgnoreOsc if matches!(byte, 7 | 0x18 | 0x1a) => Parser::Ground,
                Parser::IgnoreOsc => Parser::IgnoreOsc,
                Parser::IgnoreOscEnd if byte == b'\\' => Parser::Ground,
                Parser::IgnoreOscEnd => Parser::IgnoreOsc,
                Parser::IgnoreString if byte == 0x1b => Parser::IgnoreEnd,
                Parser::IgnoreString if matches!(byte, 0x18 | 0x1a) => Parser::Ground,
                Parser::IgnoreString => Parser::IgnoreString,
                Parser::IgnoreEnd if byte == b'\\' => Parser::Ground,
                Parser::IgnoreEnd => Parser::IgnoreString,
                _ => Parser::Ground,
            };
        }
    }

    fn finish(&mut self) {
        let Some(digits) = self.bytes[..self.len].strip_prefix(PREFIX) else {
            return;
        };
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return;
        }
        let generation = digits.iter().try_fold(0_u64, |value, digit| {
            value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
        });
        if let Some(generation) = generation {
            self.generation = generation;
        }
    }
}

#[cfg(test)]
#[path = "tests/scrollback.rs"]
mod tests;
