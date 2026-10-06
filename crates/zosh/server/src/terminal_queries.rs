//! Answers local terminal queries and relays escapes that need the displaying terminal.
//! Kept separate from terminal state because these escapes are events, not screen contents.

/// Minimal terminal-query responder for applications that expect a real TTY to
/// answer status/cursor/device queries. vt100 intentionally models display
/// state rather than writing replies back to the child, so Mosh must provide the
/// host-side answers.
pub struct QueryResponder {
    scanner: QueryScanner,
    terminal_queries: Vec<Vec<u8>>,
}

impl QueryResponder {
    pub fn new() -> Self {
        Self {
            scanner: QueryScanner::default(),
            terminal_queries: Vec::new(),
        }
    }

    /// Return reply byte strings for query sequences that became complete in
    /// this chunk. The scanner itself retains an incomplete sequence across
    /// PTY reads without replying twice to a sequence.
    pub fn feed(&mut self, data: &[u8], cursor: (u16, u16), size: (u16, u16)) -> Vec<Vec<u8>> {
        let result = self.scanner.feed(data, cursor, size);
        self.terminal_queries.extend(result.terminal_queries);
        result.replies
    }

    /// Take color queries and clipboard relay escapes detected since the previous call.
    pub fn take_terminal_queries(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.terminal_queries)
    }
}

#[derive(Default)]
struct QueryScanner {
    state: QueryScannerState,
    sequence: Vec<u8>,
}

#[derive(Default)]
enum QueryScannerState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

struct QueryScanResult {
    replies: Vec<Vec<u8>>,
    terminal_queries: Vec<Vec<u8>>,
}

const MAX_QUERY_SEQUENCE: usize = zclip::osc52::MAX_SEQUENCE_BYTES;

impl QueryScanner {
    fn feed(&mut self, data: &[u8], cursor: (u16, u16), size: (u16, u16)) -> QueryScanResult {
        let mut result = QueryScanResult {
            replies: Vec::new(),
            terminal_queries: Vec::new(),
        };
        for &byte in data {
            self.step(byte, cursor, size, &mut result);
        }
        result
    }

    fn step(
        &mut self,
        byte: u8,
        cursor: (u16, u16),
        size: (u16, u16),
        result: &mut QueryScanResult,
    ) {
        match self.state {
            QueryScannerState::Ground => {
                if byte == 0x1b {
                    self.start(QueryScannerState::Escape, byte);
                }
            }
            QueryScannerState::Escape => match byte {
                b'[' => {
                    self.push(byte);
                    self.state = QueryScannerState::Csi;
                }
                b']' => {
                    self.push(byte);
                    self.state = QueryScannerState::Osc;
                }
                0x1b => self.start(QueryScannerState::Escape, byte),
                _ => self.reset(),
            },
            QueryScannerState::Csi => {
                if byte == 0x1b {
                    self.start(QueryScannerState::Escape, byte);
                } else if matches!(byte, 0x18 | 0x1a) {
                    self.reset();
                } else if self.push(byte) && (0x40..=0x7e).contains(&byte) {
                    self.finish_csi(cursor, size, result);
                }
            }
            QueryScannerState::Osc => match byte {
                0x07 => {
                    if self.push(byte) {
                        self.finish_osc(result);
                    }
                }
                0x18 | 0x1a => self.reset(),
                0x1b => {
                    if self.push(byte) {
                        self.state = QueryScannerState::OscEscape;
                    }
                }
                _ => {
                    self.push(byte);
                }
            },
            QueryScannerState::OscEscape => {
                if !self.push(byte) {
                    return;
                }
                if byte == b'\\' {
                    self.finish_osc(result);
                } else if byte == b']' {
                    self.start_osc();
                } else {
                    self.state = QueryScannerState::Osc;
                }
            }
        }
    }

    fn finish_csi(&mut self, cursor: (u16, u16), size: (u16, u16), result: &mut QueryScanResult) {
        if let Some(kind) = csi_query_kind(&self.sequence) {
            result.replies.push(kind.reply(cursor, size));
        }
        self.reset();
    }

    fn finish_osc(&mut self, result: &mut QueryScanResult) {
        if is_terminal_color_query(&self.sequence)
            || zclip::osc52::is_write(&self.sequence)
            || zclip::protocol::Frame::parse(&self.sequence).is_some()
        {
            result
                .terminal_queries
                .push(std::mem::take(&mut self.sequence));
        } else {
            self.sequence.clear();
        }
        self.state = QueryScannerState::Ground;
    }

    fn start(&mut self, state: QueryScannerState, byte: u8) {
        self.sequence.clear();
        self.sequence.push(byte);
        self.state = state;
    }

    fn start_osc(&mut self) {
        self.sequence.clear();
        self.sequence.extend_from_slice(b"\x1b]");
        self.state = QueryScannerState::Osc;
    }

    fn push(&mut self, byte: u8) -> bool {
        if self.sequence.len() >= MAX_QUERY_SEQUENCE {
            self.reset();
            return false;
        }
        self.sequence.push(byte);
        true
    }

    fn reset(&mut self) {
        self.sequence.clear();
        self.state = QueryScannerState::Ground;
    }
}

fn csi_query_kind(sequence: &[u8]) -> Option<QueryKind> {
    match sequence {
        b"\x1b[5n" => Some(QueryKind::Status),
        b"\x1b[6n" => Some(QueryKind::Cursor),
        b"\x1b[?6n" => Some(QueryKind::DecCursor),
        b"\x1b[c" | b"\x1b[0c" => Some(QueryKind::PrimaryDa),
        b"\x1b[>c" | b"\x1b[>0c" => Some(QueryKind::SecondaryDa),
        b"\x1b[18t" => Some(QueryKind::TextArea),
        _ => None,
    }
}

fn is_terminal_color_query(sequence: &[u8]) -> bool {
    let Some(payload) = osc_payload(sequence) else {
        return false;
    };
    payload == b"10;?" || payload == b"11;?"
}

fn osc_payload(sequence: &[u8]) -> Option<&[u8]> {
    let prefix_len = sequence.starts_with(b"\x1b]").then_some(2)?;
    let payload_end = if sequence.ends_with(b"\x07") {
        sequence.len() - 1
    } else if sequence.ends_with(b"\x1b\\") {
        sequence.len() - 2
    } else {
        return None;
    };
    (payload_end >= prefix_len).then_some(&sequence[prefix_len..payload_end])
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

#[cfg(test)]
#[path = "tests/terminal_queries.rs"]
mod tests;
