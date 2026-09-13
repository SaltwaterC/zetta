//! What the server says about the history above the screen, carried inside its
//! authenticated display diff as OSC 777 sequences.
//!
//! Two things travel this way, for the same reason: a Mosh state describes one
//! screen, and neither of these is on it.
//!
//! - `777;zosh-clear-scrollback;N` is an absolute generation. A remote
//!   `CSI 3 J` bumps it, and the client clears its real terminal once when the
//!   state carrying it is shown.
//! - `777;zosh-scrollback;FIRST;BASE64` carries the rows that have scrolled off
//!   the top since the peer's acknowledged state, so the client can put them
//!   into its own terminal's history rather than losing them the way stock Mosh
//!   does. `FIRST` is the absolute index of the first row carried, counted from
//!   the moment the client asked for them.
//!
//! Keeping both in each protocol screen — rather than acting on packets as they
//! arrive — is what makes repeated and out-of-order state updates harmless: the
//! screen the client is about to show carries the numbers that describe it, and
//! the difference against the screen it is replacing is what reaches the
//! terminal.
//!
//! The payload is read on its way past rather than removed from the stream, so
//! the emulator behind this sees it too and discards it as an unknown OSC.
//! That costs a second pass over a burst's rows, and buys the whole extension
//! its correctness: the numbers end up attached to the protocol screen, which
//! is what makes a retransmission, an out-of-order arrival and a diff
//! recomputed from an older acknowledged state all already handled.
//!
//! `PROTOCOL.md` is the specification, and `zosh-server`'s
//! `terminal_state.rs` is the writing half.

use std::sync::Arc;

const PREFIX: &[u8] = b"777;zosh-clear-scrollback;";
const ROWS_PREFIX: &[u8] = b"777;zosh-scrollback;";
const MAX_MARKER: usize = PREFIX.len() + 20;
/// A row's frame: a flags byte, then a big-endian `u32` length.
const ROW_HEADER: usize = 5;
const FLAG_WRAPPED: u8 = 0b0000_0001;
/// The most Base64 one marker may carry. Mosh refuses an instruction over
/// 4 MiB, so a longer one than this cannot have arrived intact; the bound is
/// here so that a malformed sequence costs a bounded amount of memory rather
/// than an unbounded one.
const MAX_PAYLOAD: usize = 6 * 1024 * 1024;

/// One row that scrolled off the top of the server's screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScrollbackRow {
    /// The row's contents, formatting inline, written assuming only that the
    /// cursor is at the start of a blank row with default attributes.
    pub(crate) contents: Vec<u8>,
    /// Whether the logical line continued onto the row below, so replaying it
    /// can let the terminal wrap rather than breaking the line itself.
    pub(crate) wrapped: bool,
}

#[derive(Clone)]
pub(crate) struct ScrollbackState {
    pub(crate) generation: u64,
    /// Whether the server has ever carried rows. Until it has, this is a
    /// session with a peer that does not speak the extension — a stock
    /// `mosh-server`, or one whose client never asked — and the display falls
    /// back to inferring scrolls from the screen itself.
    pub(crate) active: bool,
    /// How many rows had scrolled off the server's screen when this state was
    /// produced. It only grows, so the difference between two screens is how
    /// many rows belong between them.
    pub(crate) evicted: u64,
    /// The rows the diff that produced this state carried, and the absolute
    /// index of the first. A cumulative diff carries everything since the
    /// peer's acknowledged state, so each one replaces rather than extends.
    ///
    /// Shared rather than copied: `ClientTerminal` keeps a screen per state
    /// and clones one for every diff it applies, and a burst's worth of rows
    /// must not be duplicated once per held state.
    pub(crate) rows: Arc<Vec<ScrollbackRow>>,
    pub(crate) rows_first: u64,
    parser: Parser,
    bytes: [u8; MAX_MARKER],
    len: usize,
    payload: Vec<u8>,
}

impl Default for ScrollbackState {
    fn default() -> Self {
        Self {
            generation: 0,
            active: false,
            evicted: 0,
            rows: Arc::new(Vec::new()),
            rows_first: 0,
            parser: Parser::Ground,
            bytes: [0; MAX_MARKER],
            len: 0,
            payload: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Default)]
enum Parser {
    #[default]
    Ground,
    Escape,
    Osc,
    /// An OSC whose prefix is `zosh-scrollback`, past the fixed marker buffer
    /// and into the Base64 rows.
    OscPayload,
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
                        self.payload.clear();
                        Parser::Osc
                    }
                    b'P' | b'X' | b'^' | b'_' => Parser::IgnoreString,
                    0x1b => Parser::Escape,
                    _ => Parser::Ground,
                },
                Parser::Osc | Parser::OscPayload if byte == 7 => {
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
                // The fixed buffer is full. Only the rows marker is allowed to
                // be longer than that, and by now its whole `FIRST;` header has
                // been read, so what follows is Base64 and nothing else.
                Parser::Osc if self.bytes.starts_with(ROWS_PREFIX) => {
                    self.payload.push(byte);
                    Parser::OscPayload
                }
                Parser::Osc => Parser::IgnoreOsc,
                Parser::OscPayload if byte == 0x1b => Parser::OscEnd,
                Parser::OscPayload if byte == 0x18 || byte == 0x1a => Parser::Ground,
                Parser::OscPayload if self.payload.len() < MAX_PAYLOAD => {
                    self.payload.push(byte);
                    Parser::OscPayload
                }
                Parser::OscPayload => {
                    self.payload.clear();
                    Parser::IgnoreOsc
                }
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
        if !self.bytes[..self.len].starts_with(ROWS_PREFIX) {
            self.payload.clear();
        }
        if let Some(digits) = self.bytes[..self.len].strip_prefix(PREFIX) {
            if let Some(generation) = parse_index(digits) {
                self.generation = generation;
            }
            return;
        }
        if self.bytes[..self.len].starts_with(ROWS_PREFIX) {
            self.finish_rows();
        }
    }

    /// `FIRST;BASE64`, where the header is always inside the fixed buffer and
    /// the Base64 may run past it into `payload`.
    fn finish_rows(&mut self) {
        let body = &self.bytes[ROWS_PREFIX.len()..self.len];
        let Some(semicolon) = body.iter().position(|&byte| byte == b';') else {
            return;
        };
        let Some(first) = parse_index(&body[..semicolon]) else {
            return;
        };
        let head = &body[semicolon + 1..];
        let mut encoded = Vec::with_capacity(head.len() + self.payload.len());
        encoded.extend_from_slice(head);
        encoded.extend_from_slice(&self.payload);
        // Held only while the sequence is being read. A screen is cloned for
        // every diff applied to it, and a burst's payload left sitting here
        // would be copied once per clone.
        self.payload.clear();
        let Some(rows) = decode_rows(&encoded) else {
            return;
        };
        // A state is only ever built by applying one diff to one base, so the
        // rows this diff carried replace whatever the base was carrying: both
        // describe the same range, and this one is the longer of the two.
        self.evicted = first.saturating_add(u64::try_from(rows.len()).unwrap_or(u64::MAX));
        self.rows_first = first;
        self.rows = Arc::new(rows);
        self.active = true;
    }
}

fn parse_index(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    digits.iter().try_fold(0_u64, |value, digit| {
        value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
    })
}

fn decode_rows(encoded: &[u8]) -> Option<Vec<ScrollbackRow>> {
    use base64::Engine as _;

    let payload = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(encoded)
        .ok()?;
    let mut rows = Vec::new();
    let mut rest = payload.as_slice();
    while !rest.is_empty() {
        let (header, tail) = rest.split_at_checked(ROW_HEADER)?;
        let length = usize::try_from(u32::from_be_bytes(header[1..].try_into().ok()?)).ok()?;
        let (contents, tail) = tail.split_at_checked(length)?;
        rows.push(ScrollbackRow {
            contents: contents.to_vec(),
            wrapped: header[0] & FLAG_WRAPPED != 0,
        });
        rest = tail;
    }
    Some(rows)
}

#[cfg(test)]
#[path = "tests/scrollback.rs"]
mod tests;
