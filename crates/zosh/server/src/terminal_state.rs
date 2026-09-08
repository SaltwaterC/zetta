use std::collections::BTreeMap;

const CLEAR_SCROLLBACK_MARKER_PREFIX: &[u8] = b"\x1b]777;zosh-clear-scrollback;";

#[derive(Clone)]
struct TerminalSnapshot {
    screen: vt100::Screen,
    scrollback_clear_count: u64,
}

#[derive(Clone, Copy, Default)]
enum ScrollbackDetectorState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEnd,
    String,
    StringEnd,
}

#[derive(Clone)]
struct ScrollbackClearDetector {
    state: ScrollbackDetectorState,
    parameters: [u8; 100],
    parameter_len: usize,
    overflowed: bool,

    // Mosh parses PTY output as UTF-8 before feeding codepoints into its
    // terminal parser. C1 controls U+0080..U+009F therefore arrive as
    // UTF-8 C2 80..9F, never as standalone raw 80..9F bytes.
    c1_utf8_prefix: bool,
}

impl Default for ScrollbackClearDetector {
    fn default() -> Self {
        Self {
            state: ScrollbackDetectorState::Ground,
            parameters: [0; 100],
            parameter_len: 0,
            overflowed: false,
            c1_utf8_prefix: false,
        }
    }
}

impl ScrollbackClearDetector {
    fn feed(&mut self, bytes: &[u8]) -> u64 {
        let mut count = 0_u64;

        for &byte in bytes {
            // Decode only the UTF-8 representation of C1 controls. This is
            // enough for the terminal state machine while ensuring arbitrary
            // UTF-8 continuation bytes cannot masquerade as CSI/OSC/etc.
            //
            // For example U+276F `❯` is E2 9D AF. Treating its 9D byte as a
            // standalone C1 OSC poisons the detector and makes a later
            // ESC [ 3 J disappear.
            if self.c1_utf8_prefix {
                self.c1_utf8_prefix = false;

                if (0x80..=0x9f).contains(&byte) {
                    if self.step(byte) {
                        count = count.wrapping_add(1);
                    }
                    continue;
                }
            }

            if byte == 0xc2 {
                self.c1_utf8_prefix = true;
                continue;
            }

            // All other non-ASCII bytes are UTF-8 payload from printable
            // Unicode characters and are irrelevant to CSI detection.
            if byte >= 0x80 {
                continue;
            }

            if self.step(byte) {
                count = count.wrapping_add(1);
            }
        }

        count
    }

    fn step(&mut self, byte: u8) -> bool {
        match self.state {
            ScrollbackDetectorState::Ground => match byte {
                0x1b => self.state = ScrollbackDetectorState::Escape,
                0x90 | 0x98 | 0x9e | 0x9f => self.state = ScrollbackDetectorState::String,
                0x9b => self.start_csi(),
                0x9d => self.state = ScrollbackDetectorState::Osc,
                _ => {}
            },
            ScrollbackDetectorState::Escape => match byte {
                b'[' => self.start_csi(),
                b']' => self.state = ScrollbackDetectorState::Osc,
                b'P' | b'X' | b'^' | b'_' => self.state = ScrollbackDetectorState::String,
                0x9b => self.start_csi(),
                0x9d => self.state = ScrollbackDetectorState::Osc,
                0x1b => {}
                _ => self.state = ScrollbackDetectorState::Ground,
            },
            ScrollbackDetectorState::Csi => {
                if byte == 0x1b {
                    self.state = ScrollbackDetectorState::Escape;
                } else if matches!(byte, 0x18 | 0x1a) {
                    self.state = ScrollbackDetectorState::Ground;
                } else if (0x20..=0x3f).contains(&byte) {
                    if self.parameter_len < self.parameters.len() {
                        self.parameters[self.parameter_len] = byte;
                        self.parameter_len += 1;
                    } else {
                        self.overflowed = true;
                    }
                } else if (0x40..=0x7e).contains(&byte) {
                    let is_clear = !self.overflowed
                        && byte == b'J'
                        && first_parameter_is_three(&self.parameters[..self.parameter_len]);
                    self.state = ScrollbackDetectorState::Ground;
                    self.parameter_len = 0;
                    self.overflowed = false;
                    return is_clear;
                }
            }
            ScrollbackDetectorState::Osc => match byte {
                0x07 => self.state = ScrollbackDetectorState::Ground,
                0x18 | 0x1a => self.state = ScrollbackDetectorState::Ground,
                0x1b => self.state = ScrollbackDetectorState::OscEnd,
                _ => {}
            },
            ScrollbackDetectorState::OscEnd => {
                if byte == b'\\' {
                    self.state = ScrollbackDetectorState::Ground;
                } else if byte != 0x1b {
                    self.state = ScrollbackDetectorState::Osc;
                }
            }
            ScrollbackDetectorState::String => match byte {
                0x18 | 0x1a => self.state = ScrollbackDetectorState::Ground,
                0x1b => self.state = ScrollbackDetectorState::StringEnd,
                _ => {}
            },
            ScrollbackDetectorState::StringEnd => {
                if byte == b'\\' {
                    self.state = ScrollbackDetectorState::Ground;
                } else if byte != 0x1b {
                    self.state = ScrollbackDetectorState::String;
                }
            }
        }
        false
    }

    fn start_csi(&mut self) {
        self.state = ScrollbackDetectorState::Csi;
        self.parameter_len = 0;
        self.overflowed = false;
    }
}

/// Mosh's terminal dispatcher uses the first CSI parameter and accepts
/// equivalent spellings such as `03J` and `3;0J`. Match that behavior instead
/// of comparing the raw parameter bytes with only the shortest spelling.
fn first_parameter_is_three(parameters: &[u8]) -> bool {
    let first = parameters
        .split(|&byte| byte == b';')
        .next()
        .unwrap_or_default();
    if first.is_empty() {
        return false;
    }

    let mut value = 0_u64;
    for &byte in first {
        if !byte.is_ascii_digit() {
            return false;
        }
        let Some(next) = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
        else {
            return false;
        };
        value = next;
    }
    value == 3
}

/// Authoritative server-side terminal state. Mosh synchronizes terminal state,
/// not a byte stream; `vt100::Screen::state_diff` gives us idempotent terminal
/// mutations suitable for HostBytes SSP states.
pub struct TerminalState {
    parser: vt100::Parser,
    base_screen: vt100::Screen,
    base_num: u64,
    scrollback_detector: ScrollbackClearDetector,
    scrollback_clear_count: u64,
    base_scrollback_clear_count: u64,
    snapshots: BTreeMap<u64, TerminalSnapshot>,
    max_snapshots: usize,
}

impl TerminalState {
    pub fn new(rows: u16, cols: u16) -> Self {
        let parser = vt100::Parser::new(rows, cols, 0);
        let base_screen = parser.screen().clone();
        Self {
            parser,
            base_screen,
            base_num: 0,
            scrollback_detector: ScrollbackClearDetector::default(),
            scrollback_clear_count: 0,
            base_scrollback_clear_count: 0,
            snapshots: BTreeMap::new(),
            max_snapshots: 64,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.scrollback_clear_count = self
            .scrollback_clear_count
            .wrapping_add(self.scrollback_detector.feed(bytes));
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }

    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// Produce a cumulative terminal-state transform from the screen associated
    /// with the peer's latest acknowledged SSP state to the current screen.
    pub fn diff_from_ack(&self) -> Vec<u8> {
        let current = self.parser.screen();
        let mut diff = if current.size() == self.base_screen.size() {
            current.state_diff(&self.base_screen)
        } else {
            // CompleteTerminal sends an explicit Resize instruction before the
            // visual repaint. A full repaint is safer than a cross-size delta.
            current.state_formatted()
        };

        if self.scrollback_clear_count != self.base_scrollback_clear_count {
            let marker = scrollback_clear_marker(self.scrollback_clear_count);
            let mut marked = Vec::with_capacity(marker.len() + diff.len());
            marked.extend_from_slice(&marker);
            marked.append(&mut diff);
            diff = marked;
        }
        diff
    }

    pub fn resize_from_ack(&self) -> Option<(u16, u16)> {
        let current = self.parser.screen();
        (current.size() != self.base_screen.size()).then(|| current.size())
    }

    pub fn snapshot_for_state(&mut self, state_num: u64) {
        self.snapshots.insert(
            state_num,
            TerminalSnapshot {
                screen: self.parser.screen().clone(),
                scrollback_clear_count: self.scrollback_clear_count,
            },
        );
        while self.snapshots.len() > self.max_snapshots {
            let Some(first) = self.snapshots.keys().next().copied() else {
                break;
            };
            self.snapshots.remove(&first);
        }
    }

    /// Advance the visual base whenever the peer ACKs an SSP state. Transport
    /// heartbeats can have state numbers with no explicit screen snapshot; in
    /// that case the most recent screen snapshot below the ACK is equivalent.
    pub fn acknowledge(&mut self, ack_num: u64) {
        if ack_num <= self.base_num {
            return;
        }

        if let Some((_, snapshot)) = self.snapshots.range(..=ack_num).next_back() {
            self.base_screen = snapshot.screen.clone();
            self.base_scrollback_clear_count = snapshot.scrollback_clear_count;
        }
        self.base_num = ack_num;
        self.snapshots.retain(|num, _| *num > ack_num);
    }

    #[cfg(test)]
    pub fn scrollback_clear_count(&self) -> u64 {
        self.scrollback_clear_count
    }
}

fn scrollback_clear_marker(generation: u64) -> Vec<u8> {
    let digits = generation.to_string();
    let mut marker = Vec::with_capacity(CLEAR_SCROLLBACK_MARKER_PREFIX.len() + digits.len() + 1);
    marker.extend_from_slice(CLEAR_SCROLLBACK_MARKER_PREFIX);
    marker.extend_from_slice(digits.as_bytes());
    marker.push(0x07);
    marker
}

/// Minimal terminal-query responder for applications that expect a real TTY to
/// answer status/cursor/device queries. vt100 intentionally models display
/// state rather than writing replies back to the child, so Mosh must provide the
/// host-side answers.
pub struct QueryResponder {
    tail: Vec<u8>,
}

impl QueryResponder {
    pub fn new() -> Self {
        Self { tail: Vec::new() }
    }

    /// Return reply byte strings for query sequences that became complete in
    /// this chunk. Keeping a short tail handles escape sequences split across
    /// PTY reads without replying twice to a sequence wholly in the old tail.
    pub fn feed(&mut self, data: &[u8], cursor: (u16, u16), size: (u16, u16)) -> Vec<Vec<u8>> {
        let old_len = self.tail.len();
        let mut combined = Vec::with_capacity(old_len + data.len());
        combined.extend_from_slice(&self.tail);
        combined.extend_from_slice(data);

        let mut replies = Vec::new();
        let patterns: &[(&[u8], QueryKind)] = &[
            (b"\x1b[5n", QueryKind::Status),
            (b"\x1b[6n", QueryKind::Cursor),
            (b"\x1b[?6n", QueryKind::DecCursor),
            (b"\x1b[c", QueryKind::PrimaryDa),
            (b"\x1b[0c", QueryKind::PrimaryDa),
            (b"\x1b[>c", QueryKind::SecondaryDa),
            (b"\x1b[>0c", QueryKind::SecondaryDa),
            (b"\x1b[18t", QueryKind::TextArea),
        ];

        for &(pattern, kind) in patterns {
            let mut start = 0usize;
            while start + pattern.len() <= combined.len() {
                let Some(relative) = find_bytes(&combined[start..], pattern) else {
                    break;
                };
                let at = start + relative;
                let end = at + pattern.len();
                // Only a query that crosses into newly received data is new.
                if end > old_len {
                    replies.push(kind.reply(cursor, size));
                }
                start = at + 1;
            }
        }

        const KEEP: usize = 32;
        let keep_from = combined.len().saturating_sub(KEEP);
        self.tail.clear();
        self.tail.extend_from_slice(&combined[keep_from..]);
        replies
    }
}

#[derive(Clone, Copy)]
enum QueryKind {
    Status,
    Cursor,
    DecCursor,
    PrimaryDa,
    SecondaryDa,
    TextArea,
}

impl QueryKind {
    fn reply(self, cursor: (u16, u16), size: (u16, u16)) -> Vec<u8> {
        match self {
            QueryKind::Status => b"\x1b[0n".to_vec(),
            QueryKind::Cursor => format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1).into_bytes(),
            QueryKind::DecCursor => {
                format!("\x1b[?{};{}R", cursor.0 + 1, cursor.1 + 1).into_bytes()
            }
            // A conservative VT100-with-advanced-video identity. Applications
            // use this to choose capability paths; TERM remains authoritative.
            QueryKind::PrimaryDa => b"\x1b[?1;2c".to_vec(),
            // xterm-compatible secondary DA shape; exact patch level is not
            // semantically important to applications that merely probe support.
            QueryKind::SecondaryDa => b"\x1b[>0;276;0c".to_vec(),
            QueryKind::TextArea => format!("\x1b[8;{};{}t", size.0, size.1).into_bytes(),
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn htop_ascii_padding_survives_server_state_updates() {
        let mut terminal = TerminalState::new(3, 80);
        // ncurses emits REP for runs of spaces in htop's ASCII mode,
        // triggered when macOS forwards LC_CTYPE=UTF-8 to a Linux host.
        terminal.process(b"\x1b[2;5H275 \x1b[90;1mroot \x1b[");
        terminal.process(b"6b\x1b[0m 20   0 43360");
        let mut client = vt100::Parser::new(3, 80, 0);
        client.process(&terminal.diff_from_ack());
        assert_eq!(client.screen().cell(1, 20).unwrap().contents(), "2");
        assert_eq!(client.screen().cell(1, 30).unwrap().contents(), "6");
        terminal.snapshot_for_state(1);
        terminal.acknowledge(1);
        terminal.process(b"\x1b[2;31H9");
        client.process(&terminal.diff_from_ack());
        assert_eq!(client.screen().cell(1, 20).unwrap().contents(), "2");
        assert_eq!(client.screen().cell(1, 30).unwrap().contents(), "9");
    }

    #[test]
    fn query_split_across_reads() {
        let mut responder = QueryResponder::new();
        assert!(responder.feed(b"hello\x1b[", (2, 3), (24, 80)).is_empty());
        assert_eq!(
            responder.feed(b"6n", (2, 3), (24, 80)),
            vec![b"\x1b[3;4R".to_vec()]
        );
    }

    #[test]
    fn terminal_diff_reconstructs_visible_state() {
        let mut state = TerminalState::new(24, 80);
        state.process(b"hello");
        let diff = state.diff_from_ack();

        let mut peer = vt100::Parser::new(24, 80, 0);
        peer.process(&diff);
        assert_eq!(peer.screen().contents(), state.parser.screen().contents());
    }

    #[test]
    fn clear_scrollback_marker_survives_chunk_boundaries() {
        let mut state = TerminalState::new(24, 80);
        state.process(b"before\x1b[3");
        state.process(b"Jafter");

        assert_eq!(state.scrollback_clear_count(), 1);
        assert_eq!(
            state.diff_from_ack(),
            b"\x1b]777;zosh-clear-scrollback;1\x07beforeafter"
        );
    }

    #[test]
    fn clear_scrollback_marker_is_sent_when_the_screen_is_unchanged() {
        let mut state = TerminalState::new(24, 80);
        state.process(b"\x1b[3J");

        assert_eq!(
            state.diff_from_ack(),
            b"\x1b]777;zosh-clear-scrollback;1\x07"
        );
    }

    #[test]
    fn clear_scrollback_detector_matches_moshs_first_parameter_rules() {
        for sequence in [b"\x1b[03J".as_slice(), b"\x1b[3;0J", b"\xc2\x9b3J"] {
            let mut state = TerminalState::new(24, 80);
            state.process(sequence);
            assert_eq!(state.scrollback_clear_count(), 1, "{sequence:?}");
        }

        for sequence in [b"\x1b[30J".as_slice(), b"\x1b[?3J", b"\x1b[3 J"] {
            let mut state = TerminalState::new(24, 80);
            state.process(sequence);
            assert_eq!(state.scrollback_clear_count(), 0, "{sequence:?}");
        }
    }

    #[test]
    fn unicode_prompt_bytes_do_not_poison_clear_detection() {
        let mut state = TerminalState::new(24, 80);

        // This is present in the captured real zsh prompt. U+276F is encoded
        // as E2 9D AF; the old raw-byte detector interpreted 9D as C1 OSC.
        state.process("❯".as_bytes());
        state.process(b"\x1b[3J");

        assert_eq!(state.scrollback_clear_count(), 1);
    }

    #[test]
    fn unicode_prompt_and_clear_in_same_chunk_are_detected() {
        let mut state = TerminalState::new(24, 80);

        state.process("prompt ❯ \x1b[3J".as_bytes());

        assert_eq!(state.scrollback_clear_count(), 1);
    }

    #[test]
    fn utf8_c1_csi_is_still_supported() {
        let mut state = TerminalState::new(24, 80);

        // U+009B CSI encoded as UTF-8.
        state.process(b"\xc2\x9b3J");

        assert_eq!(state.scrollback_clear_count(), 1);
    }

    #[test]
    fn utf8_c1_csi_survives_pty_chunk_boundary() {
        let mut state = TerminalState::new(24, 80);

        state.process(b"\xc2");
        state.process(b"\x9b3J");

        assert_eq!(state.scrollback_clear_count(), 1);
    }

    #[test]
    fn raw_utf8_continuation_byte_is_not_a_c1_control() {
        let mut state = TerminalState::new(24, 80);

        // Standalone 9D is not a valid UTF-8 encoding of U+009D.
        state.process(b"\x9d");
        state.process(b"\x1b[3J");

        assert_eq!(state.scrollback_clear_count(), 1);
    }

    #[test]
    fn clear_scrollback_marker_is_not_found_inside_a_string() {
        let mut state = TerminalState::new(24, 80);
        state.process(b"\x1b]title \x1b[3J still title\x07");
        state.process(b"\x1bPdata \x1b[3J still data\x1b\\");

        assert_eq!(state.scrollback_clear_count(), 0);
    }

    #[test]
    fn acknowledged_snapshot_stops_repeating_the_clear_marker() {
        let mut state = TerminalState::new(24, 80);
        state.process(b"\x1b[3J");
        assert!(!state.diff_from_ack().is_empty());
        state.snapshot_for_state(1);
        state.acknowledge(1);

        assert!(state.diff_from_ack().is_empty());
    }
}
