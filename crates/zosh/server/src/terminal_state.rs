use std::collections::{BTreeMap, VecDeque};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;

const CLEAR_SCROLLBACK_MARKER_PREFIX: &[u8] = b"\x1b]777;zosh-clear-scrollback;";
const SCROLLBACK_MARKER_PREFIX: &[u8] = b"\x1b]777;zosh-scrollback;";

/// The smallest and largest a client may ask this server to hold of the rows
/// that have scrolled off its screen.
///
/// The ceiling is what keeps one state inside Mosh's 4 MiB instruction limit:
/// the rows are Base64 in the state that carries them, and the screen diff
/// shares the same instruction.
pub const SCROLLBACK_BUDGET_MIN: usize = 16 * 1024;
pub const SCROLLBACK_BUDGET_MAX: usize = 2048 * 1024;
/// What is collected before a client has said whether it wants any.
///
/// The program starts writing the moment the server does, which is before the
/// first datagram can have arrived, so a session that only began collecting on
/// request would lose its opening screenfuls. This is held on spec and thrown
/// away the moment a client turns out not to want it.
const SCROLLBACK_BUDGET_PROVISIONAL: usize = SCROLLBACK_BUDGET_MAX;

/// One row that has scrolled off the top, waiting to be acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingRow {
    /// Counted from the moment a client asked for scrollback, so that the
    /// client's own count of what it has replayed is the same number.
    index: u64,
    contents: Vec<u8>,
    wrapped: bool,
}

impl PendingRow {
    fn cost(&self) -> usize {
        // The row plus the per-row header it is framed with, so a flood of
        // empty lines is charged for what it actually costs to carry.
        self.contents.len() + SCROLLBACK_ROW_HEADER
    }
}

/// `flags` then a big-endian `u32` length, ahead of each row's bytes.
const SCROLLBACK_ROW_HEADER: usize = 5;
const SCROLLBACK_FLAG_WRAPPED: u8 = 0b0000_0001;

#[derive(Clone)]
struct TerminalSnapshot {
    screen: vt100::Screen,
    scrollback_clear_count: u64,
    /// How many rows had scrolled off the top when this state was sent. Rows
    /// below it are ones the peer has, so acknowledging the state is what
    /// lets them be dropped.
    evicted_total: u64,
    /// The title that state showed, so an acknowledged one is not restated on
    /// every later diff.
    title: Option<String>,
    query_count: usize,
}

/// Picks the window title out of the program's byte stream.
///
/// `vt100` parses the sequence and hands it to a callback, keeping none of it
/// in the screen; this reads the same bytes on the way past so the title can
/// be carried in the state the client is sent. OSC 0 sets the icon name and
/// the title together and OSC 2 sets the title alone — both are titles; OSC 1
/// is an icon name and deliberately is not.
#[derive(Default)]
struct TitleScanner {
    state: TitleScannerState,
    command: u16,
    pending: String,
    title: Option<String>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum TitleScannerState {
    #[default]
    Ground,
    Escape,
    Command,
    Text,
    Skip,
    Terminator,
}

/// The longest title accepted. A program that never terminates its sequence
/// must not be able to make the server allocate without bound.
const MAX_TITLE_BYTES: usize = 4096;

impl TitleScanner {
    fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.step(byte);
        }
    }

    fn step(&mut self, byte: u8) {
        match self.state {
            TitleScannerState::Ground => {
                if byte == 0x1b {
                    self.state = TitleScannerState::Escape;
                }
            }
            TitleScannerState::Escape => {
                if byte == b']' {
                    self.state = TitleScannerState::Command;
                    self.command = 0;
                    self.pending.clear();
                } else {
                    // Any other escape sequence belongs to somebody else.
                    self.state = TitleScannerState::Ground;
                }
            }
            TitleScannerState::Command => match byte {
                b'0'..=b'9' => {
                    self.command = self.command.saturating_mul(10) + u16::from(byte - b'0');
                }
                b';' => {
                    self.state = if matches!(self.command, 0 | 2) {
                        TitleScannerState::Text
                    } else {
                        TitleScannerState::Skip
                    };
                }
                // A malformed OSC: give up rather than swallow the rest of the
                // stream looking for a terminator.
                _ => self.state = TitleScannerState::Ground,
            },
            TitleScannerState::Text | TitleScannerState::Skip => match byte {
                // BEL ends it, and so does ST; both are in use.
                0x07 => self.finish(),
                0x1b => self.state = TitleScannerState::Terminator,
                0x18 | 0x1a => self.state = TitleScannerState::Ground,
                _ if self.state == TitleScannerState::Text => {
                    if byte < 0x20 || self.pending.len() >= MAX_TITLE_BYTES {
                        // A title is text, and a bounded amount of it.
                        self.state = TitleScannerState::Ground;
                    } else {
                        self.pending.push(char::from(byte));
                    }
                }
                _ => {}
            },
            TitleScannerState::Terminator => {
                if byte == b'\\' {
                    self.finish();
                } else if byte == 0x1b {
                    // Another escape: still waiting for the terminator.
                } else {
                    self.state = TitleScannerState::Text;
                    self.step(byte);
                }
            }
        }
    }

    fn finish(&mut self) {
        if self.state == TitleScannerState::Text || self.state == TitleScannerState::Terminator {
            self.title = Some(std::mem::take(&mut self.pending));
        }
        self.pending.clear();
        self.state = TitleScannerState::Ground;
    }
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
    /// The window title the program has asked for, and the one the peer's
    /// acknowledged state already shows. `vt100` hands a title to a callback
    /// and models none of it in the screen, so `state_diff` cannot carry it —
    /// see [`TerminalState::diff_from_ack`] for why that matters.
    title_scanner: TitleScanner,
    base_title: Option<String>,
    /// How much scrolled-off history may be held unacknowledged, in bytes, or
    /// zero once it is known that nobody wants any.
    scrollback_budget: usize,
    /// Whether a client has actually asked. Until one has, rows are collected
    /// on the chance that one will — the program starts writing before the
    /// first datagram arrives, and the opening screenful of a session is worth
    /// as much as any other — but nothing is held back for them, because a
    /// stock Mosh client is never going to ask and must not be made to wait.
    scrollback_announced: bool,
    pending_scrollback: VecDeque<PendingRow>,
    pending_scrollback_bytes: usize,
    /// The eviction count of the peer's acknowledged state.
    base_evicted_total: u64,
    snapshots: BTreeMap<u64, TerminalSnapshot>,
    max_snapshots: usize,
    pending_queries: Vec<TerminalQuery>,
    next_query_id: u64,
}

/// One OSC 10/11 query that must be answered by the terminal outside the
/// server's PTY. Queries remain attached to cumulative terminal states until
/// the client acknowledges them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalQuery {
    pub id: u64,
    pub bytes: Vec<u8>,
}

impl TerminalState {
    pub fn new(rows: u16, cols: u16) -> Self {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        // On from the first byte: see SCROLLBACK_BUDGET_PROVISIONAL.
        parser.screen_mut().set_capture_evicted_rows(true);
        let base_screen = parser.screen().clone();
        Self {
            parser,
            base_screen,
            base_num: 0,
            scrollback_detector: ScrollbackClearDetector::default(),
            scrollback_clear_count: 0,
            base_scrollback_clear_count: 0,
            title_scanner: TitleScanner::default(),
            base_title: None,
            scrollback_budget: SCROLLBACK_BUDGET_PROVISIONAL,
            scrollback_announced: false,
            pending_scrollback: VecDeque::new(),
            pending_scrollback_bytes: 0,
            base_evicted_total: 0,
            snapshots: BTreeMap::new(),
            max_snapshots: 64,
            pending_queries: Vec::new(),
            next_query_id: 1,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        let cleared = self.scrollback_detector.feed(bytes);
        self.scrollback_clear_count = self.scrollback_clear_count.wrapping_add(cleared);
        self.title_scanner.feed(bytes);
        self.parser.process(bytes);
        if cleared > 0 {
            // The program asked for the history to be thrown away, so rows
            // still waiting to be sent are rows the client is about to be
            // told to forget. The index they would have occupied is skipped
            // rather than reused, and the clear travelling in the same state
            // is what tells the client the gap is deliberate.
            self.pending_scrollback.clear();
            self.pending_scrollback_bytes = 0;
        }
        self.collect_scrollback();
    }

    /// Starts carrying the rows that scroll off the top, holding at most
    /// `budget` bytes of unacknowledged ones.
    ///
    /// Called when a client announces that it can receive them. Announcing it
    /// twice is harmless: the budget is replaced and nothing already collected
    /// is disturbed.
    pub fn set_scrollback_budget(&mut self, budget: usize) {
        self.scrollback_budget = budget.clamp(SCROLLBACK_BUDGET_MIN, SCROLLBACK_BUDGET_MAX);
        self.scrollback_announced = true;
        self.parser.screen_mut().set_capture_evicted_rows(true);
    }

    /// Stops carrying scrolled-off rows and forgets the ones collected so far.
    ///
    /// Called once it is known that the client cannot use them, which is what
    /// every stock Mosh client silently says by never asking.
    pub fn forget_scrollback(&mut self) {
        self.scrollback_budget = 0;
        self.scrollback_announced = false;
        self.pending_scrollback.clear();
        self.pending_scrollback_bytes = 0;
        self.parser.screen_mut().set_capture_evicted_rows(false);
    }

    /// Whether the unacknowledged rows have reached the client's budget.
    ///
    /// The server stops reading the program while this holds, which is how a
    /// client that cannot keep up slows the program down instead of losing
    /// its output — the same backpressure an SSH session has.
    pub fn scrollback_over_budget(&self) -> bool {
        self.scrollback_announced && self.pending_scrollback_bytes >= self.scrollback_budget
    }

    /// Drops the oldest unacknowledged rows back to the budget.
    ///
    /// This is what a session whose client has gone does instead of applying
    /// backpressure: a Mosh session outliving its client is the whole point,
    /// and a program blocked on a viewer that may never return is not. The
    /// indices of the dropped rows are skipped, so the client that comes back
    /// can say how much it missed rather than silently showing less.
    pub fn drop_scrollback_over_budget(&mut self) {
        while self.pending_scrollback_bytes > self.scrollback_budget {
            let Some(row) = self.pending_scrollback.pop_front() else {
                break;
            };
            self.pending_scrollback_bytes -= row.cost();
        }
    }

    fn collect_scrollback(&mut self) {
        if self.scrollback_budget == 0 {
            return;
        }
        let (first, rows) = self.parser.screen_mut().take_evicted_rows();
        for (offset, row) in rows.into_iter().enumerate() {
            let row = PendingRow {
                index: first + offset as u64,
                contents: row.contents,
                wrapped: row.wrapped,
            };
            self.pending_scrollback_bytes += row.cost();
            self.pending_scrollback.push_back(row);
        }
        if !self.scrollback_announced {
            // Nobody has asked yet, so nobody may be slowed down for this.
            // What does not fit gives way, and the skipped indices are what
            // would tell a client that did turn up how much it had missed.
            self.drop_scrollback_over_budget();
        }
    }

    fn evicted_total(&self) -> u64 {
        self.parser.screen().evicted_rows_total()
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

    /// Record a query found in PTY output and assign it the next session-local
    /// ID. It is emitted in every cumulative state until that state is acked.
    pub fn add_query(&mut self, bytes: Vec<u8>) -> u64 {
        let id = self.next_query_id;
        self.next_query_id = id
            .checked_add(1)
            .expect("terminal query ID space exhausted");
        self.pending_queries.push(TerminalQuery { id, bytes });
        id
    }

    /// Queries not yet covered by the client's acknowledged terminal state.
    pub fn queries_from_ack(&self) -> &[TerminalQuery] {
        &self.pending_queries
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

        // Ahead of the screen, because it describes what was above it. The
        // marker is emitted whenever the count has moved at all, even with no
        // rows to show for it: the number is how the client knows where the
        // screen it is about to be shown sits in the session's output, and a
        // count that moved with nothing attached is exactly the case where it
        // must be told that something went missing.
        if self.scrollback_announced && self.evicted_total() != self.base_evicted_total {
            let mut marked = scrollback_marker(&self.pending_scrollback, self.evicted_total());
            marked.append(&mut diff);
            diff = marked;
        }

        if self.scrollback_clear_count != self.base_scrollback_clear_count {
            let marker = scrollback_clear_marker(self.scrollback_clear_count);
            let mut marked = Vec::with_capacity(marker.len() + diff.len());
            marked.extend_from_slice(&marker);
            marked.append(&mut diff);
            diff = marked;
        }
        // The title is part of a terminal's state and none of its contents, so
        // it has to be restated here or it never crosses at all: `vt100` gives
        // it to a callback and `state_diff` knows nothing about it. It is not
        // cosmetic — Zetta reports a pane's working directory as a window
        // title, so a session whose titles are dropped is one whose panes
        // never learn where they are.
        let title = self.title_scanner.title();
        if title.is_some() && title != self.base_title.as_deref() {
            let mut titled = title_escape(title.unwrap_or_default());
            titled.append(&mut diff);
            diff = titled;
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
                evicted_total: self.evicted_total(),
                title: self.title_scanner.title().map(str::to_owned),
                query_count: self.pending_queries.len(),
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

        let acknowledged_query_count = self
            .snapshots
            .range(..=ack_num)
            .next_back()
            .map_or(0, |(_, snapshot)| snapshot.query_count);
        if let Some((_, snapshot)) = self.snapshots.range(..=ack_num).next_back() {
            self.base_screen = snapshot.screen.clone();
            self.base_scrollback_clear_count = snapshot.scrollback_clear_count;
            self.base_title = snapshot.title.clone();
            // The rows that state carried are rows the peer now has, so they
            // stop being resent and stop counting against its budget.
            self.base_evicted_total = snapshot.evicted_total;
            self.retire_scrollback_below(snapshot.evicted_total);
        }
        if acknowledged_query_count > 0 {
            let query_count = acknowledged_query_count.min(self.pending_queries.len());
            self.pending_queries.drain(..query_count);
            for snapshot in self.snapshots.values_mut() {
                snapshot.query_count = snapshot
                    .query_count
                    .saturating_sub(acknowledged_query_count);
            }
        }
        self.base_num = ack_num;
        self.snapshots.retain(|num, _| *num > ack_num);
    }

    fn retire_scrollback_below(&mut self, index: u64) {
        while self
            .pending_scrollback
            .front()
            .is_some_and(|row| row.index < index)
        {
            let row = self
                .pending_scrollback
                .pop_front()
                .expect("just checked that there is a front row");
            self.pending_scrollback_bytes -= row.cost();
        }
    }

    #[cfg(test)]
    pub fn scrollback_clear_count(&self) -> u64 {
        self.scrollback_clear_count
    }

    #[cfg(test)]
    pub fn pending_scrollback_bytes(&self) -> usize {
        self.pending_scrollback_bytes
    }
}

/// `ESC ] 0 ; <title> BEL`, the spelling stock Mosh sends a title with: OSC 0
/// sets the icon name and the title together, and BEL is the terminator with
/// the widest support.
fn title_escape(title: &str) -> Vec<u8> {
    let mut escape = Vec::with_capacity(title.len() + 5);
    escape.extend_from_slice(b"\x1b]0;");
    escape.extend_from_slice(title.as_bytes());
    escape.push(0x07);
    escape
}

/// `ESC ] 777 ; zosh-scrollback ; <first> ; <Base64 rows> BEL`.
///
/// `first` is the absolute index of the first row carried, and `total` is
/// where the screen that follows begins, so the client learns both what it is
/// being given and where it now is. When the two disagree by more than the
/// rows carried, history was dropped, and saying so is the client's job.
///
/// Each row is framed as a flags byte, a big-endian `u32` length, and that
/// many bytes of contents. Base64 keeps it inside an OSC string, which is
/// what lets a Mosh implementation that has never heard of the extension
/// discard it as an unknown OSC rather than draw it.
fn scrollback_marker(rows: &VecDeque<PendingRow>, total: u64) -> Vec<u8> {
    let first = rows.front().map_or(total, |row| row.index);
    let mut payload = Vec::with_capacity(rows.iter().map(PendingRow::cost).sum::<usize>());
    for row in rows {
        payload.push(if row.wrapped {
            SCROLLBACK_FLAG_WRAPPED
        } else {
            0
        });
        payload.extend_from_slice(
            &u32::try_from(row.contents.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        payload.extend_from_slice(&row.contents);
    }
    let encoded = STANDARD_NO_PAD.encode(&payload);
    let mut marker = Vec::with_capacity(SCROLLBACK_MARKER_PREFIX.len() + 24 + encoded.len());
    marker.extend_from_slice(SCROLLBACK_MARKER_PREFIX);
    marker.extend_from_slice(first.to_string().as_bytes());
    marker.push(b';');
    marker.extend_from_slice(encoded.as_bytes());
    marker.push(0x07);
    marker
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

    /// Take OSC 10/11 queries detected since the previous call.
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

const MAX_QUERY_SEQUENCE: usize = 4096;

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
        if is_terminal_color_query(&self.sequence) {
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
    fn osc_color_queries_forward_with_bel_and_st_terminators() {
        let mut responder = QueryResponder::new();
        assert!(responder.feed(b"\x1b]10;?", (0, 0), (24, 80)).is_empty());
        assert!(responder.take_terminal_queries().is_empty());
        assert!(responder.feed(b"\x07", (0, 0), (24, 80)).is_empty());
        assert_eq!(
            responder.take_terminal_queries(),
            vec![b"\x1b]10;?\x07".to_vec()]
        );

        assert!(
            responder
                .feed(b"\x1b]11;?\x1b", (0, 0), (24, 80))
                .is_empty()
        );
        assert_eq!(
            responder.feed(b"\\", (0, 0), (24, 80)),
            Vec::<Vec<u8>>::new()
        );
        assert_eq!(
            responder.take_terminal_queries(),
            vec![b"\x1b]11;?\x1b\\".to_vec()]
        );
    }

    #[test]
    fn malformed_osc_sequences_do_not_become_color_queries() {
        let mut responder = QueryResponder::new();
        responder.feed(
            b"\x1b]10;not-a-query\x07\x1b]12;?\x07\x1b]10;?\x18\x1b]10;?\x07",
            (0, 0),
            (24, 80),
        );
        assert_eq!(
            responder.take_terminal_queries(),
            vec![b"\x1b]10;?\x07".to_vec()]
        );
    }

    #[test]
    fn ordinary_csi_queries_keep_their_local_replies() {
        let mut responder = QueryResponder::new();
        assert_eq!(
            responder.feed(b"\x1b[6n\x1b[5n", (2, 3), (24, 80)),
            vec![b"\x1b[3;4R".to_vec(), b"\x1b[0n".to_vec()]
        );
        assert!(responder.take_terminal_queries().is_empty());
    }

    #[test]
    fn terminal_queries_follow_snapshots_and_acknowledgements() {
        let mut terminal = TerminalState::new(24, 80);
        assert_eq!(terminal.add_query(b"q1".to_vec()), 1);
        terminal.snapshot_for_state(1);
        assert_eq!(terminal.add_query(b"q2".to_vec()), 2);
        terminal.snapshot_for_state(2);
        assert_eq!(
            terminal.queries_from_ack(),
            &[
                TerminalQuery {
                    id: 1,
                    bytes: b"q1".to_vec(),
                },
                TerminalQuery {
                    id: 2,
                    bytes: b"q2".to_vec(),
                },
            ]
        );

        // A retransmitted state still sees both IDs before the ACK. Once
        // state 1 is acknowledged, only the newer query remains to repeat.
        terminal.acknowledge(1);
        assert_eq!(
            terminal.queries_from_ack(),
            &[TerminalQuery {
                id: 2,
                bytes: b"q2".to_vec(),
            }]
        );
        terminal.acknowledge(2);
        assert!(terminal.queries_from_ack().is_empty());
    }

    /// A title is terminal state that `vt100::Screen::state_diff` knows
    /// nothing about, so the server has to restate it or it never crosses.
    /// Zetta reports a pane's working directory this way, which is what makes
    /// this more than cosmetic.
    #[test]
    fn a_window_title_is_carried_in_the_state_the_client_is_sent() {
        let mut state = TerminalState::new(24, 80);
        state.process(b"\x1b]2;zetta-cwd:/tmp/project\x1b\\");

        let diff = state.diff_from_ack();
        let text = String::from_utf8_lossy(&diff);
        assert!(
            text.contains("\u{1b}]0;zetta-cwd:/tmp/project\u{7}"),
            "the title has to be restated in the diff: {text:?}"
        );
        // And in front of the contents, so a client that is still painting the
        // screen it belongs to has it by the time the frame is shown.
        assert!(text.starts_with("\u{1b}]0;"), "{text:?}");
    }

    /// Cumulative diffs are sent until one is acknowledged, so an unchanged
    /// title must not be restated forever — and a changed one must be.
    #[test]
    fn an_acknowledged_title_is_not_restated_and_a_new_one_is() {
        let mut state = TerminalState::new(24, 80);
        state.process(b"\x1b]2;first\x07");
        state.snapshot_for_state(1);
        state.acknowledge(1);
        assert!(
            !String::from_utf8_lossy(&state.diff_from_ack()).contains("\u{1b}]0;"),
            "a title the peer already shows is not state it is missing"
        );

        state.process(b"\x1b]2;second\x07");
        assert!(
            String::from_utf8_lossy(&state.diff_from_ack()).contains("\u{1b}]0;second\u{7}"),
            "a title that changed since the acknowledged state has to be sent"
        );
    }

    /// OSC 1 is an icon name rather than a window title, and a sequence that
    /// never terminates must not be able to make the server hold an unbounded
    /// string.
    #[test]
    fn the_title_scanner_reads_titles_and_only_titles() {
        let mut scanner = TitleScanner::default();
        scanner.feed(b"\x1b]1;icon-name\x07");
        assert_eq!(scanner.title(), None);

        scanner.feed(b"\x1b]0;icon and title\x07");
        assert_eq!(scanner.title(), Some("icon and title"));

        scanner.feed(b"\x1b]2;title alone\x1b\\");
        assert_eq!(scanner.title(), Some("title alone"));

        let mut unbounded = TitleScanner::default();
        unbounded.feed(b"\x1b]2;");
        unbounded.feed(&vec![b'x'; MAX_TITLE_BYTES * 2]);
        unbounded.feed(b"\x07");
        assert_eq!(
            unbounded.title(),
            None,
            "an unbounded title is abandoned rather than buffered"
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

    // ------------------------------------------------------------------ //
    // Scrollback carriage. `PROTOCOL.md` is the specification; the reading
    // half is `crates/zosh/src/scrollback.rs`, which is tested against the
    // same framing.
    // ------------------------------------------------------------------ //

    /// One row as the reading half recovers it: its contents, and whether
    /// the logical line continued onto the row below.
    type ReadRow = (Vec<u8>, bool);

    /// The marker a diff carries, as `(first_index, rows)`. Deliberately a
    /// second implementation of the client's reader: the two agreeing is the
    /// claim being tested.
    fn read_scrollback(diff: &[u8]) -> Option<(u64, Vec<ReadRow>)> {
        let start = diff
            .windows(SCROLLBACK_MARKER_PREFIX.len())
            .position(|window| window == SCROLLBACK_MARKER_PREFIX)?
            + SCROLLBACK_MARKER_PREFIX.len();
        let end = start + diff[start..].iter().position(|&byte| byte == 0x07)?;
        let body = &diff[start..end];
        let semicolon = body.iter().position(|&byte| byte == b';')?;
        let first: u64 = std::str::from_utf8(&body[..semicolon]).ok()?.parse().ok()?;
        let payload = STANDARD_NO_PAD.decode(&body[semicolon + 1..]).ok()?;

        let mut rows = Vec::new();
        let mut rest = payload.as_slice();
        while !rest.is_empty() {
            let flags = rest[0];
            let length = u32::from_be_bytes(rest[1..5].try_into().ok()?) as usize;
            rows.push((
                rest[SCROLLBACK_ROW_HEADER..SCROLLBACK_ROW_HEADER + length].to_vec(),
                flags & SCROLLBACK_FLAG_WRAPPED != 0,
            ));
            rest = &rest[SCROLLBACK_ROW_HEADER + length..];
        }
        Some((first, rows))
    }

    fn row_text(rows: &[ReadRow]) -> Vec<String> {
        rows.iter()
            .map(|(bytes, _)| String::from_utf8_lossy(bytes).into_owned())
            .collect()
    }

    /// A stock Mosh client has no way to ask, so it must get exactly what it
    /// got before: a screen, and nothing wrapped around it. The rows are
    /// collected on spec until the first state settles the question, and a
    /// client that never asks never sees them.
    #[test]
    fn nothing_is_carried_until_a_client_asks_for_it() {
        let mut state = TerminalState::new(2, 20);
        state.process(b"one\r\ntwo\r\nthree\r\n");
        assert!(state.pending_scrollback_bytes() > 0, "held on spec");
        state.forget_scrollback();
        assert_eq!(state.pending_scrollback_bytes(), 0);
        state.process(b"four\r\n");

        let diff = state.diff_from_ack();
        assert!(read_scrollback(&diff).is_none(), "{diff:?}");
        assert!(
            !String::from_utf8_lossy(&diff).contains("zosh-scrollback"),
            "{diff:?}"
        );
    }

    #[test]
    fn rows_that_scrolled_off_are_carried_oldest_first_from_zero() {
        let mut state = TerminalState::new(2, 20);
        // The program starts writing before the first datagram can arrive, so
        // what it wrote in the meantime is collected on spec and is there for
        // a client that turns out to want it.
        state.process(b"before\r\nalso before\r\n");
        state.set_scrollback_budget(64 * 1024);
        state.process(b"one\r\ntwo\r\nthree\r\n");

        let (first, rows) = read_scrollback(&state.diff_from_ack()).expect("a marker");
        assert_eq!(first, 0);
        assert_eq!(row_text(&rows), vec!["before", "also before", "one", "two"]);
    }

    #[test]
    fn an_acknowledged_row_is_not_carried_again_and_the_index_continues() {
        let mut state = TerminalState::new(2, 20);
        state.set_scrollback_budget(64 * 1024);
        state.process(b"one\r\ntwo\r\nthree\r\n");
        state.snapshot_for_state(1);
        state.acknowledge(1);

        // Nothing has scrolled since, so there is nothing left to say.
        assert!(read_scrollback(&state.diff_from_ack()).is_none());
        assert_eq!(state.pending_scrollback_bytes(), 0);

        state.process(b"four\r\n");
        let (first, rows) = read_scrollback(&state.diff_from_ack()).expect("a marker");
        assert_eq!(first, 2, "the index is absolute, not per-diff");
        assert_eq!(row_text(&rows), vec!["three"]);
    }

    /// A cumulative diff is recomputed from the peer's acknowledged state on
    /// every pass, so an unacknowledged row has to keep travelling.
    #[test]
    fn an_unacknowledged_row_is_carried_by_every_later_diff() {
        let mut state = TerminalState::new(2, 20);
        state.set_scrollback_budget(64 * 1024);
        state.process(b"one\r\ntwo\r\nthree\r\n");
        let first_diff = read_scrollback(&state.diff_from_ack()).expect("a marker");
        state.process(b"four\r\n");
        let second_diff = read_scrollback(&state.diff_from_ack()).expect("a marker");

        assert_eq!(first_diff.0, 0);
        assert_eq!(second_diff.0, 0);
        assert_eq!(row_text(&second_diff.1), vec!["one", "two", "three"]);
    }

    #[test]
    fn a_wrapped_row_says_so_and_its_colour_travels_with_it() {
        let mut state = TerminalState::new(2, 4);
        state.set_scrollback_budget(64 * 1024);
        state.process(b"\x1b[31mabcdefgh\x1b[m\r\nx\r\ny\r\n");

        let (_, rows) = read_scrollback(&state.diff_from_ack()).expect("a marker");
        assert!(rows[0].1, "a full row that continued below");
        assert!(!rows[1].1, "the row it continued onto ended the line");
        assert!(
            String::from_utf8_lossy(&rows[0].0).contains("\x1b[31m"),
            "{:?}",
            rows[0].0
        );
    }

    #[test]
    fn the_budget_is_clamped_and_reports_pressure_once_it_is_reached() {
        let mut state = TerminalState::new(2, 20);
        // Below the floor: a client cannot ask for a budget so small that a
        // single screen cannot fit through it.
        state.set_scrollback_budget(1);
        assert!(!state.scrollback_over_budget());

        for line in 0..4000 {
            state.process(format!("line {line}\r\n").as_bytes());
        }
        assert!(
            state.scrollback_over_budget(),
            "{} bytes pending against a {SCROLLBACK_BUDGET_MIN} byte floor",
            state.pending_scrollback_bytes()
        );
    }

    /// What a session does once its client has gone: a Mosh session outliving
    /// its client is the point, so the history gives way rather than the
    /// program. The skipped indices are what let the client say so.
    #[test]
    fn dropping_over_budget_skips_indices_rather_than_renumbering() {
        let mut state = TerminalState::new(2, 20);
        state.set_scrollback_budget(SCROLLBACK_BUDGET_MIN);
        for line in 0..4000 {
            state.process(format!("line {line}\r\n").as_bytes());
        }
        state.drop_scrollback_over_budget();
        assert!(state.pending_scrollback_bytes() <= SCROLLBACK_BUDGET_MIN);

        let (first, rows) = read_scrollback(&state.diff_from_ack()).expect("a marker");
        assert!(first > 0, "the oldest rows were dropped");
        assert_eq!(
            row_text(&rows)[0],
            format!("line {first}"),
            "the index still names the row it is attached to"
        );
    }

    /// A program that clears the history is asking for the rows above the
    /// screen to be forgotten, including ones still in flight.
    #[test]
    fn a_clear_drops_the_rows_that_were_still_waiting() {
        let mut state = TerminalState::new(2, 20);
        state.set_scrollback_budget(64 * 1024);
        state.process(b"one\r\ntwo\r\nthree\r\n");
        assert!(state.pending_scrollback_bytes() > 0);

        state.process(b"\x1b[3J");
        assert_eq!(state.pending_scrollback_bytes(), 0);
        // The count still moved, so the marker still travels: the client has
        // to learn where the screen now sits even when it is given no rows.
        let diff = state.diff_from_ack();
        let (first, rows) = read_scrollback(&diff).expect("a marker");
        assert_eq!(first, 2);
        assert!(rows.is_empty());
        assert!(String::from_utf8_lossy(&diff).contains("zosh-clear-scrollback"));
    }

    /// The framing, byte for byte, as `PROTOCOL.md` documents it. The client
    /// carries the same vector.
    #[test]
    fn the_marker_is_framed_exactly_as_the_protocol_says() {
        let mut rows = VecDeque::new();
        rows.push_back(PendingRow {
            index: 7,
            contents: b"hi".to_vec(),
            wrapped: true,
        });
        assert_eq!(
            scrollback_marker(&rows, 8),
            b"\x1b]777;zosh-scrollback;7;AQAAAAJoaQ\x07".to_vec()
        );
        // No rows, but a count that moved: the first index is the count.
        assert_eq!(
            scrollback_marker(&VecDeque::new(), 12),
            b"\x1b]777;zosh-scrollback;12;\x07".to_vec()
        );
    }
}
