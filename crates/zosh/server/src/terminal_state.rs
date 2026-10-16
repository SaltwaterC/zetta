use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

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
const MAX_PENDING_QUERY_BYTES: usize = 2 * 1024 * 1024;
const MAX_PENDING_QUERIES: usize = 128;

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
    /// Shared, not copied: acknowledging a state makes its screen the base
    /// without another copy, and a state sent with the screen unchanged
    /// (an echo acknowledgement, an agent record) holds the one before it.
    screen: Arc<vt100::Screen>,
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

/// What a frame from some base carries for the terminal.
pub struct TerminalFrame<'a> {
    /// The size to tell the peer about first, when it differs from the base's.
    pub resize: Option<(u16, u16)>,
    pub host_bytes: Vec<u8>,
    /// Queries the base did not already carry.
    pub queries: &'a [TerminalQuery],
}

/// What a frame is diffed from: the parts of a held state a diff reads.
struct Base<'a> {
    screen: &'a vt100::Screen,
    scrollback_clear_count: u64,
    evicted_total: u64,
    title: Option<&'a str>,
    /// How many of the pending queries the base already carried.
    query_count: usize,
}

/// Authoritative server-side terminal state. Mosh synchronizes terminal state,
/// not a byte stream; `vt100::Screen::state_diff` gives us idempotent terminal
/// mutations suitable for HostBytes SSP states.
pub struct TerminalState {
    parser: vt100::Parser,
    base_screen: Arc<vt100::Screen>,
    base_num: u64,
    /// The screen as of the newest snapshot, while the parser's has not
    /// changed since; cleared by anything that changes it.
    unchanged_screen: Option<Arc<vt100::Screen>>,
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
    pending_query_bytes: usize,
    next_query_id: u64,
}

/// One escape relayed to the displaying terminal: a color query or clipboard
/// message. These events remain attached to cumulative terminal states until
/// the client acknowledges them, including clipboard writes with no response.
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
        let base_screen = Arc::new(parser.screen().clone());
        Self {
            parser,
            base_screen,
            base_num: 0,
            unchanged_screen: None,
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
            pending_query_bytes: 0,
            next_query_id: 1,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        let cleared = self.scrollback_detector.feed(bytes);
        self.scrollback_clear_count = self.scrollback_clear_count.wrapping_add(cleared);
        self.title_scanner.feed(bytes);
        if !bytes.is_empty() {
            self.unchanged_screen = None;
        }
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
        self.screen_mut().set_capture_evicted_rows(true);
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
        self.screen_mut().set_capture_evicted_rows(false);
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
        let (first, rows) = self.screen_mut().take_evicted_rows();
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
        self.screen_mut().set_size_reflow(rows, cols);
        self.collect_scrollback();
    }

    /// The parser's screen, for a change to it: the next snapshot has to be
    /// a fresh copy rather than the last one again.
    fn screen_mut(&mut self) -> &mut vt100::Screen {
        self.unchanged_screen = None;
        self.parser.screen_mut()
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
        if self.pending_queries.len() >= MAX_PENDING_QUERIES
            || self.pending_query_bytes.saturating_add(bytes.len()) > MAX_PENDING_QUERY_BYTES
        {
            return 0;
        }
        let id = self.next_query_id;
        self.next_query_id = id
            .checked_add(1)
            .expect("terminal query ID space exhausted");
        self.pending_query_bytes += bytes.len();
        self.pending_queries.push(TerminalQuery { id, bytes });
        id
    }

    /// Whether a frame can be diffed from `state`: one this side sent and
    /// still holds a snapshot of.
    pub fn has_snapshot(&self, state: u64) -> bool {
        self.snapshots.contains_key(&state)
    }

    /// What the peer holds if it holds `base`: the acknowledged state for
    /// `None`, otherwise a state this side sent. `None` for a state with no
    /// snapshot.
    fn base(&self, base: Option<u64>) -> Option<Base<'_>> {
        let Some(state) = base else {
            return Some(Base {
                screen: &self.base_screen,
                scrollback_clear_count: self.base_scrollback_clear_count,
                evicted_total: self.base_evicted_total,
                title: self.base_title.as_deref(),
                query_count: 0,
            });
        };
        let snapshot = self.snapshots.get(&state)?;
        Some(Base {
            screen: &snapshot.screen,
            scrollback_clear_count: snapshot.scrollback_clear_count,
            evicted_total: snapshot.evicted_total,
            title: snapshot.title.as_deref(),
            query_count: snapshot.query_count.min(self.pending_queries.len()),
        })
    }

    /// The cumulative transform from `base` — the acknowledged state for
    /// `None`, or a state this side sent — to the terminal as it is now, or
    /// `None` when there is no snapshot of `base` to diff from.
    ///
    /// Diffing from a state the peer has probably received rather than from
    /// the one it has acknowledged is what stock Mosh does, and it is what
    /// keeps a frame from carrying everything since the last acknowledgement
    /// again: on a slow link, with history being carried, that was every
    /// unacknowledged scrolled-off row in every frame.
    pub fn frame_from(&self, base: Option<u64>) -> Option<TerminalFrame<'_>> {
        let base = self.base(base)?;
        let current = self.parser.screen();
        let resize = (current.size() != base.screen.size()).then(|| current.size());
        let mut diff = if resize.is_none() {
            current.state_diff(base.screen)
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
        // must be told that something went missing. Rows `base` already
        // carried are left out.
        if self.scrollback_announced && self.evicted_total() != base.evicted_total {
            let rows = self
                .pending_scrollback
                .iter()
                .skip_while(|row| row.index < base.evicted_total);
            let mut marked = scrollback_marker(rows, self.evicted_total());
            marked.append(&mut diff);
            diff = marked;
        }

        if self.scrollback_clear_count != base.scrollback_clear_count {
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
        if title.is_some() && title != base.title {
            let mut titled = title_escape(title.unwrap_or_default());
            titled.append(&mut diff);
            diff = titled;
        }
        Some(TerminalFrame {
            resize,
            host_bytes: diff,
            queries: &self.pending_queries[base.query_count..],
        })
    }

    /// Queries not yet covered by the client's acknowledged terminal state.
    #[cfg(test)]
    pub fn queries_from_ack(&self) -> &[TerminalQuery] {
        &self.pending_queries
    }

    /// [`Self::frame_from`] the acknowledged state, screen bytes only.
    #[cfg(test)]
    pub fn diff_from_ack(&self) -> Vec<u8> {
        self.frame_from(None)
            .expect("the acknowledged state is always held")
            .host_bytes
    }

    pub fn snapshot_for_state(&mut self, state_num: u64) {
        let screen = Arc::clone(
            self.unchanged_screen
                .get_or_insert_with(|| Arc::new(self.parser.screen().clone())),
        );
        self.snapshots.insert(
            state_num,
            TerminalSnapshot {
                screen,
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
            self.base_screen = Arc::clone(&snapshot.screen);
            self.base_scrollback_clear_count = snapshot.scrollback_clear_count;
            self.base_title = snapshot.title.clone();
            // The rows that state carried are rows the peer now has, so they
            // stop being resent and stop counting against its budget.
            self.base_evicted_total = snapshot.evicted_total;
            self.retire_scrollback_below(snapshot.evicted_total);
        }
        if acknowledged_query_count > 0 {
            let query_count = acknowledged_query_count.min(self.pending_queries.len());
            self.pending_query_bytes -= self
                .pending_queries
                .drain(..query_count)
                .map(|query| query.bytes.len())
                .sum::<usize>();
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
    pub fn screen_contents(&self) -> String {
        self.parser.screen().contents()
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
fn scrollback_marker<'a>(rows: impl Iterator<Item = &'a PendingRow>, total: u64) -> Vec<u8> {
    let rows: Vec<&PendingRow> = rows.collect();
    let first = rows.first().map_or(total, |row| row.index);
    let mut payload = Vec::with_capacity(rows.iter().map(|row| row.cost()).sum::<usize>());
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

#[cfg(test)]
#[path = "tests/terminal_state.rs"]
mod tests;
