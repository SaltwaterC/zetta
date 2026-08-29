mod mappings;

mod alacritty;
mod pty_info;
mod snapshot;
pub mod terminal_settings;

use anyhow::{Context as _, Result, bail};
use futures_lite::future::yield_now;
use log::trace;

use futures::{
    FutureExt,
    channel::mpsc::{UnboundedReceiver, unbounded},
};

use itertools::Itertools as _;
use mappings::mouse::{
    alt_scroll, grid_point, grid_point_and_side, mouse_button_report, mouse_moved_report,
    scroll_report,
};

use async_channel::{Receiver, Sender};
use collections::{HashMap, VecDeque};
use futures::StreamExt;
use pty_info::{ProcessIdGetter, PtyProcessInfo, TerminalProcessIds};
use serde::{Deserialize, Serialize};
use settings::Settings;
use task::{HideStrategy, Shell, ShellKind, SpawnInTerminal};
use terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape, TerminalSettings};
use theme::{ActiveTheme, Theme};
use urlencoding;
#[cfg(windows)]
use util::paths::PathWithPosition;
use util::{ResultExt as _, paths::PathStyle, truncate_and_trailoff};

use alacritty_terminal::grid::Dimensions as _;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

/// A successful exit, built the way each platform spells one: Unix carries a
/// wait status and Windows an exit code, and the same number does not mean the
/// same thing to both.
#[cfg(test)]
fn successful_exit() -> ExitStatus {
    #[cfg(unix)]
    return ExitStatus::from_raw(0);
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt as _;
        ExitStatus::from_raw(0)
    }
}
use std::{
    borrow::Cow,
    cmp::{self, min},
    fmt::{self, Display, Formatter},
    future::Future,
    io::{Read, Write},
    mem,
    ops::{BitOr, BitOrAssign, Deref, Range as StdRange},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Arc, Once,
        atomic::{AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;
use vte::ansi::{Attr, Handler, Processor, StdSyncHandler};
pub use vte::ansi::{Color, NamedColor, Rgb};

use gpui::{
    App, AppContext as _, BackgroundExecutor, Bounds, ClipboardItem, Context, EventEmitter, Hsla,
    Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point as GpuiPoint, Priority, Rgba, ScrollWheelEvent, Size, Task, TouchPhase, Window, actions,
    black, px,
};

#[cfg(not(windows))]
use crate::alacritty::current_child_signal_mask;
use crate::alacritty::{
    AlacrittyCell, AlacrittyGridIterator, AlacrittyHyperlink, AlacrittyPty, AlacrittySearch,
    AlacrittyTerm, AlacrittyTermConfig, AlacrittyTermLock, HyperlinkMatch, PtyIo, PtySender,
    RegexSearches, ScrollbackSearch, WakeupGate, ZedListener, append_text_to_term, apply_config,
    clear_saved_screen, content_text, display_offset, display_only_term_config,
    find_from_terminal_point, full_content_range, last_non_empty_lines, make_content, new_term,
    open_pty, pty_options, pty_term_config, resize, screen_lines, scroll_display, scroll_to_point,
    selection_text, set_default_cursor_style, set_selection as set_term_selection, shrink_to_used,
    spawn_event_loop, toggle_vi_mode as toggle_term_vi_mode, total_lines,
    update_selection as update_term_selection, update_selection_to_vi_cursor,
    update_vi_cursor_for_scroll, vi_goto_point, vi_motion,
};
use crate::mappings::colors::to_vte_rgb;
use crate::mappings::keys::to_esc_str;

pub use alacritty_terminal::tty::{AttachedChildEvents, ConsolePalette};

const PROCESS_KILL_GRACE_PERIOD: Duration = Duration::from_millis(100);

/// Give the shell and its foreground job a short graceful-exit window, then
/// kill both process groups. This must capture the groups before the PTY is
/// shut down because foreground-group lookup uses the PTY descriptor.
fn terminate_processes_with_grace_period(
    info: Arc<PtyProcessInfo>,
    executor: BackgroundExecutor,
) -> impl Future<Output = ()> {
    let process_ids: TerminalProcessIds = info.capture_process_ids();
    process_ids.terminate();
    async move {
        executor.timer(PROCESS_KILL_GRACE_PERIOD).await;
        process_ids.kill();
        info.kill_child_process();
    }
}

/// Process-wide flag set by headless hosts (e.g. the eval CLI) that have no
/// controlling TTY. In such sandboxes PTY allocation and acquiring a
/// controlling terminal fail with `ENOTTY`, so when this is set terminals run
/// their command as a plain subprocess with piped output instead of through a
/// PTY. The normal editor leaves it unset to preserve the interactive PTY
/// experience.
#[derive(Clone, Copy, Default)]
pub struct HeadlessTerminal(pub bool);

impl gpui::Global for HeadlessTerminal {}

impl HeadlessTerminal {
    pub fn is_enabled(cx: &App) -> bool {
        cx.try_global::<Self>().is_some_and(|headless| headless.0)
    }
}

#[derive(Clone, Copy, Debug)]
enum Scroll {
    Delta(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug)]
enum ViMotion {
    Up,
    Down,
    Left,
    Right,
    First,
    Last,
    FirstOccupied,
    High,
    Middle,
    Low,
    WordLeft,
    WordRight,
    WordRightEnd,
    Bracket,
    ParagraphUp,
    ParagraphDown,
}

#[derive(Clone, Debug)]
pub struct Search {
    search: AlacrittySearch,
    literal: Option<String>,
}

pub struct SearchMatches {
    pub ranges: Vec<Range>,
    pub total_count: usize,
    pub limit_reached: bool,
}

#[derive(Clone, Debug)]
struct Selection {
    ty: SelectionType,
    start: SelectionAnchor,
    end: SelectionAnchor,
    head: Point,
}

#[derive(Clone, Copy, Debug)]
struct SelectionAnchor {
    point: Point,
    side: SelectionSide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionType {
    Simple,
    Semantic,
    Lines,
}

impl Selection {
    fn new(selection_type: SelectionType, point: Point, side: SelectionSide) -> Self {
        let anchor = SelectionAnchor { point, side };
        Self {
            ty: selection_type,
            start: anchor,
            end: anchor,
            head: point,
        }
    }

    fn simple_range(range: Range) -> Self {
        let mut selection = Self::new(SelectionType::Simple, range.start(), SelectionSide::Left);
        selection.update(range.end(), SelectionSide::Right);
        selection
    }

    fn update(&mut self, point: Point, side: SelectionSide) {
        self.end = SelectionAnchor { point, side };
        self.head = point;
    }
}

pub fn is_default_background_color(color: Color) -> bool {
    matches!(color, Color::Named(NamedColor::Background))
}

pub fn is_app_chosen_exact_color(color: Color) -> bool {
    matches!(color, Color::Spec(_) | Color::Indexed(16..=255))
}

pub type AnsiSpans = Vec<(StdRange<usize>, Option<Color>)>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedAnsiText {
    pub text: String,
    pub foreground_spans: AnsiSpans,
    pub background_spans: AnsiSpans,
}

pub fn parse_ansi_text(input: &[u8]) -> ParsedAnsiText {
    let mut handler = StyledAnsiTextHandler::default();
    let mut processor = Processor::<StdSyncHandler>::default();
    processor.advance(&mut handler, input);
    handler.finish()
}

pub fn strip_ansi_text(input: &[u8]) -> String {
    let mut handler = PlainAnsiTextHandler::default();
    let mut processor = Processor::<StdSyncHandler>::default();
    processor.advance(&mut handler, input);
    handler.text
}

#[derive(Default)]
struct StyledAnsiTextHandler {
    text: String,
    foreground_spans: AnsiSpans,
    background_spans: AnsiSpans,
    current_foreground_range_start: usize,
    current_background_range_start: usize,
    current_foreground_color: Option<Color>,
    current_background_color: Option<Color>,
}

impl StyledAnsiTextHandler {
    fn finish(mut self) -> ParsedAnsiText {
        if self.current_foreground_range_start < self.text.len() {
            self.foreground_spans.push((
                self.current_foreground_range_start..self.text.len(),
                self.current_foreground_color,
            ));
        }

        if self.current_background_range_start < self.text.len() {
            self.background_spans.push((
                self.current_background_range_start..self.text.len(),
                self.current_background_color,
            ));
        }

        ParsedAnsiText {
            text: self.text,
            foreground_spans: self.foreground_spans,
            background_spans: self.background_spans,
        }
    }

    fn break_foreground_span(&mut self, color: Option<Color>) {
        self.foreground_spans.push((
            self.current_foreground_range_start..self.text.len(),
            self.current_foreground_color,
        ));
        self.current_foreground_color = color;
        self.current_foreground_range_start = self.text.len();
    }

    fn break_background_span(&mut self, color: Option<Color>) {
        self.background_spans.push((
            self.current_background_range_start..self.text.len(),
            self.current_background_color,
        ));
        self.current_background_color = color;
        self.current_background_range_start = self.text.len();
    }
}

impl Handler for StyledAnsiTextHandler {
    fn input(&mut self, c: char) {
        self.text.push(c);
    }

    fn linefeed(&mut self) {
        self.text.push('\n');
    }

    fn put_tab(&mut self, count: u16) {
        self.text.extend(std::iter::repeat_n('\t', count as usize));
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        match attr {
            Attr::Foreground(color) => {
                self.break_foreground_span(Some(color));
            }
            Attr::Background(color) => {
                self.break_background_span(Some(color));
            }
            Attr::Reset => {
                self.break_foreground_span(None);
                self.break_background_span(None);
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct PlainAnsiTextHandler {
    text: String,
    line_start: usize,
}

impl Handler for PlainAnsiTextHandler {
    fn input(&mut self, c: char) {
        self.text.push(c);
    }

    fn linefeed(&mut self) {
        self.text.push('\n');
        self.line_start = self.text.len();
    }

    fn carriage_return(&mut self) {
        self.text.truncate(self.line_start);
    }

    fn put_tab(&mut self, count: u16) {
        self.text.extend(std::iter::repeat_n('\t', count as usize));
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Hyperlink {
    data: HyperlinkData,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum HyperlinkData {
    Alacritty(AlacrittyHyperlink),
    Owned { id: Option<Arc<str>>, uri: Arc<str> },
}

#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub struct Cell {
    cell: AlacrittyCell,
}

pub struct RenderableCells<'a> {
    cells: AlacrittyGridIterator<'a>,
}

#[derive(Debug, Clone)]
pub struct IndexedCell {
    pub point: Point,
    pub cell: Cell,
}

impl Deref for IndexedCell {
    type Target = Cell;

    #[inline]
    fn deref(&self) -> &Cell {
        &self.cell
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modes(u32);

impl Modes {
    pub const NONE: Self = Self(0);
    pub const APP_CURSOR: Self = Self(1 << 0);
    pub const APP_KEYPAD: Self = Self(1 << 1);
    pub const SHOW_CURSOR: Self = Self(1 << 2);
    pub const LINE_WRAP: Self = Self(1 << 3);
    pub const ORIGIN: Self = Self(1 << 4);
    pub const INSERT: Self = Self(1 << 5);
    pub const LINE_FEED_NEW_LINE: Self = Self(1 << 6);
    pub const FOCUS_IN_OUT: Self = Self(1 << 7);
    pub const ALTERNATE_SCROLL: Self = Self(1 << 8);
    pub const BRACKETED_PASTE: Self = Self(1 << 9);
    pub const SGR_MOUSE: Self = Self(1 << 10);
    pub const UTF8_MOUSE: Self = Self(1 << 11);
    pub const ALT_SCREEN: Self = Self(1 << 12);
    pub const MOUSE_REPORT_CLICK: Self = Self(1 << 13);
    pub const MOUSE_DRAG: Self = Self(1 << 14);
    pub const MOUSE_MOTION: Self = Self(1 << 15);
    pub const VI: Self = Self(1 << 16);
    pub const WIN32_INPUT: Self = Self(1 << 17);
    pub const MOUSE_MODE: Self =
        Self(Self::MOUSE_REPORT_CLICK.0 | Self::MOUSE_DRAG.0 | Self::MOUSE_MOTION.0);

    pub const fn empty() -> Self {
        Self::NONE
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

impl BitOr for Modes {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Modes {
    fn bitor_assign(&mut self, rhs: Self) {
        self.insert(rhs);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursor {
    pub shape: CursorShape,
    pub point: Point,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
    HollowBlock,
    Hidden,
}

impl From<SettingsCursorShape> for CursorShape {
    fn from(shape: SettingsCursorShape) -> Self {
        match shape {
            SettingsCursorShape::Block => Self::Block,
            SettingsCursorShape::Underline => Self::Underline,
            SettingsCursorShape::Bar => Self::Bar,
            SettingsCursorShape::Hollow => Self::HollowBlock,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: i32,
    pub column: usize,
}

impl Point {
    pub fn new(line: i32, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Self {
        Self { start, end }
    }

    pub fn start(&self) -> Point {
        self.start
    }

    pub fn end(&self) -> Point {
        self.end
    }

    pub fn contains(&self, point: Point) -> bool {
        self.start <= point && point <= self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionRange {
    pub start: Point,
    pub end: Point,
    pub is_block: bool,
}

impl SelectionRange {
    pub fn point_range(self) -> Range {
        Range::new(self.start, self.end)
    }
}

// TODO: Un-pub
#[derive(Clone)]
pub struct Content {
    pub cells: Vec<IndexedCell>,
    pub mode: Modes,
    pub total_lines: usize,
    pub display_offset: usize,
    pub columns: usize,
    pub screen_lines: usize,
    pub selection_text: Option<String>,
    pub selection: Option<SelectionRange>,
    pub cursor: Cursor,
    pub cursor_char: char,
    pub terminal_bounds: TerminalBounds,
    pub last_hovered_word: Option<HoveredWord>,
    /// Whether the lines this snapshot shows are the same lines the previous one
    /// showed. A hovered word is only carried across a snapshot that is
    /// `Unchanged`; otherwise it names a position that has moved.
    pub grid_lines_change: GridLinesChange,
    pub scrolled_to_top: bool,
    pub scrolled_to_bottom: bool,
    pub bottom_row_occupied: bool,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub enum GridLinesChange {
    #[default]
    Unchanged,
    Changed,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HoveredWord {
    pub word: String,
    pub word_match: Range,
    pub id: usize,
}

impl Default for Content {
    fn default() -> Self {
        Content {
            cells: Default::default(),
            mode: Default::default(),
            total_lines: Default::default(),
            display_offset: Default::default(),
            columns: Default::default(),
            screen_lines: Default::default(),
            selection_text: Default::default(),
            selection: Default::default(),
            cursor: Cursor {
                shape: CursorShape::Block,
                point: Point::new(0, 0),
            },
            cursor_char: Default::default(),
            terminal_bounds: Default::default(),
            last_hovered_word: None,
            grid_lines_change: Default::default(),
            scrolled_to_top: false,
            scrolled_to_bottom: false,
            bottom_row_occupied: false,
        }
    }
}

#[derive(PartialEq, Eq)]
enum SelectionPhase {
    Selecting,
    Ended,
}

#[cfg(test)]
mod domain_tests {
    use super::*;

    #[test]
    fn strip_ansi_text_removes_ansi_and_handles_carriage_returns() {
        let cases = [
            ("no escape codes here\n", "no escape codes here\n"),
            ("\x1b[31mhello\x1b[0m", "hello"),
            ("\x1b[1;32mfoo\x1b[0m bar", "foo bar"),
            ("progress 10%\rprogress 100%\n", "progress 100%\n"),
        ];

        for (input, expected) in cases {
            assert_eq!(strip_ansi_text(input.as_bytes()), expected);
        }
    }

    #[test]
    fn parse_ansi_text_records_foreground_and_background_spans() {
        let parsed = parse_ansi_text(b"\x1b[31mred\x1b[44mblue-bg\x1b[0mplain");

        assert_eq!(parsed.text, "redblue-bgplain");
        assert_eq!(
            parsed.foreground_spans,
            vec![
                (0..0, None),
                (0..10, Some(Color::Named(NamedColor::Red))),
                (10..15, None),
            ]
        );
        assert_eq!(
            parsed.background_spans,
            vec![
                (0..3, None),
                (3..10, Some(Color::Named(NamedColor::Blue))),
                (10..15, None),
            ]
        );
    }

    #[test]
    fn terminal_cell_clone_shares_extra_storage() {
        let mut cell = Cell::default();
        cell.push_zerowidth('a');

        let clone = cell.clone();

        match (&cell.cell.extra, &clone.cell.extra) {
            (Some(extra), Some(clone_extra)) => assert!(Arc::ptr_eq(extra, clone_extra)),
            _ => panic!("expected extra storage on both cells"),
        }
    }
}

actions!(
    terminal,
    [
        /// Clears the terminal screen.
        Clear,
        /// Copies selected text to the clipboard.
        Copy,
        /// Pastes from the clipboard.
        Paste,
        /// Pastes the text from the clipboard.
        PasteText,
        /// Trims leading and trailing whitespace before pasting clipboard text.
        PasteTrimmed,
        /// Shows the character palette for special characters.
        ShowCharacterPalette,
        /// Searches for text in the terminal.
        SearchTest,
        /// Scrolls up by one line.
        ScrollLineUp,
        /// Scrolls down by one line.
        ScrollLineDown,
        /// Scrolls up by one page.
        ScrollPageUp,
        /// Scrolls down by one page.
        ScrollPageDown,
        /// Scrolls up by half a page.
        ScrollHalfPageUp,
        /// Scrolls down by half a page.
        ScrollHalfPageDown,
        /// Scrolls to the top of the terminal buffer.
        ScrollToTop,
        /// Scrolls to the bottom of the terminal buffer.
        ScrollToBottom,
        /// Toggles vi mode in the terminal.
        ToggleViMode,
        /// Selects all text in the terminal.
        SelectAll,
    ]
);

const DEBUG_TERMINAL_WIDTH: Pixels = px(500.);
const DEBUG_TERMINAL_HEIGHT: Pixels = px(30.);
const DEBUG_CELL_WIDTH: Pixels = px(5.);
const DEBUG_LINE_HEIGHT: Pixels = px(5.);

/// Inserts Zetta-specific environment variables for terminal sessions.
pub fn insert_zetta_terminal_env<S: std::hash::BuildHasher>(
    env: &mut std::collections::HashMap<String, String, S>,
    version: &impl std::fmt::Display,
) {
    env.remove("ZED_TERM");
    env.insert("ZETTA_TERM".to_string(), "true".to_string());
    env.insert("TERM_PROGRAM".to_string(), "zetta".to_string());
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    env.insert("TERM_PROGRAM_VERSION".to_string(), version.to_string());
}

///Upward flowing events, for changing the title and such
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    TitleChanged,
    BreadcrumbsChanged,
    CloseTerminal,
    Bell,
    Wakeup,
    /// Reports completion of a terminal backed by a task. Unlike
    /// [`Event::CloseTerminal`], this event is emitted even when the task is
    /// configured to remain visible after it exits.
    TaskFinished {
        exit_code: Option<i32>,
    },
    /// Reports the one-shot exit classification for an interactive terminal.
    ///
    /// Task-backed terminals continue to report [`Event::TaskFinished`] and
    /// retain their existing hide/close behavior instead of using this event.
    TerminalExited(TerminalExited),
    BlinkChanged(bool),
    ResizeRequested {
        rows: usize,
        columns: usize,
    },
    /// The grid's dimensions changed, after a layout-driven resize or a font
    /// size change. Distinct from [`Event::Wakeup`], which also fires for
    /// ordinary output: this is for the chrome around the terminal, which
    /// displays the grid size and must not repaint on every byte written.
    GridSizeChanged,
    SelectionsChanged,
    NewNavigationTarget(Option<MaybeNavigationTarget>),
    Open(MaybeNavigationTarget),
}

/// Where the terminal learned that its child had stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalExitSource {
    /// The child watcher supplied a normal exit status.
    Child,
    /// The child stopped, but the operating system did not provide a usable
    /// status (or only the terminal's final event was observed).
    StatusUnavailable,
    /// The watcher channel disconnected before a status was delivered.
    WatcherDisconnected,
    /// The PTY backend stopped because of an infrastructure failure.
    BackendShutdown,
}

/// Why an interactive terminal exit should remain visible instead of closing
/// its pane automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalExitReason {
    StatusUnavailable,
    WatcherDisconnected,
    BackendShutdown,
    ExitedBeforeInput,
    ForegroundCommand,
}

/// The complete, one-shot exit observation for an interactive terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalExited {
    pub exit_code: Option<i32>,
    pub source: TerminalExitSource,
    pub child_pid: Option<u32>,
    pub input_sent: bool,
    /// Whether the last available process metadata identified the shell as
    /// the foreground process. `None` means that the platform had no reliable
    /// answer at exit time.
    pub foreground_is_shell: Option<bool>,
    /// A normalized foreground command name when one was available. This is
    /// deliberately only a name, not a shell output line or full argv.
    pub foreground_command: Option<String>,
}

impl TerminalExited {
    pub fn unexpected_reason(&self) -> Option<TerminalExitReason> {
        // A clean exit status means the process itself terminated normally.
        // Whatever the foreground metadata shows, there is nothing unexpected
        // about the session ending.
        if self.exit_code == Some(0) {
            return None;
        }
        let source_reason = match self.source {
            TerminalExitSource::Child if self.exit_code.is_none() => {
                Some(TerminalExitReason::StatusUnavailable)
            }
            TerminalExitSource::Child => None,
            TerminalExitSource::StatusUnavailable => Some(TerminalExitReason::StatusUnavailable),
            TerminalExitSource::WatcherDisconnected => {
                Some(TerminalExitReason::WatcherDisconnected)
            }
            TerminalExitSource::BackendShutdown => Some(TerminalExitReason::BackendShutdown),
        };
        source_reason
            .or_else(|| (!self.input_sent).then_some(TerminalExitReason::ExitedBeforeInput))
            .or_else(|| {
                (self.foreground_is_shell == Some(false)
                    && !matches!(self.foreground_command.as_deref(), Some("exit" | "logout")))
                .then_some(TerminalExitReason::ForegroundCommand)
            })
    }

    pub fn is_unexpected(&self) -> bool {
        self.unexpected_reason().is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathLikeTarget {
    /// File system path, absolute or relative, existing or not.
    /// Might have line and column number(s) attached as `file.rs:1:23`
    pub maybe_path: String,
    /// Current working directory of the terminal
    pub terminal_dir: Option<PathBuf>,
    /// Syntax of paths emitted by the terminal shell.
    pub path_style: PathStyle,
}

/// A string inside terminal, potentially useful as a URI that can be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaybeNavigationTarget {
    /// HTTP, git, etc. string determined by the `URL_REGEX` regex.
    Url(String),
    /// File system path, absolute or relative, existing or not.
    /// Might have line and column number(s) attached as `file.rs:1:23`
    PathLike(PathLikeTarget),
}

fn editor_invocation_command(
    zetta_command: &str,
    path_argument: &str,
    delete_after: bool,
) -> String {
    let delete_after = delete_after
        .then_some("--delete-after ")
        .unwrap_or_default();
    format!("{zetta_command} edit {delete_after}-- {path_argument}")
}

fn editor_path_argument(
    shell_kind: ShellKind,
    shell: &Shell,
    path: &Path,
    path_style: PathStyle,
) -> Option<String> {
    let path = path.to_str()?;
    #[cfg(windows)]
    match posix_host(shell) {
        Some(PosixHost::Wsl) => return wsl_editor_path_argument(path, path_style),
        Some(PosixHost::Cygwin) => {
            return cygwin_editor_path_argument(path, path_style, shell_kind);
        }
        _ => {}
    }
    #[cfg(not(windows))]
    let _ = (shell, path_style);
    shell_kind.try_quote(path).map(Into::into)
}

#[cfg(windows)]
fn wsl_editor_path_argument(path: &str, path_style: PathStyle) -> Option<String> {
    if path.starts_with('/') {
        let path = ShellKind::Posix.try_quote(path)?;
        return Some(format!("\"$(wslpath -w {path})\""));
    }

    if path_style.is_posix() && !PathStyle::Windows.is_absolute(path) {
        return wsl_relative_editor_path_argument(path);
    }

    ShellKind::Posix.try_quote(path).map(Into::into)
}

#[cfg(windows)]
fn wsl_relative_editor_path_argument(path: &str) -> Option<String> {
    if path == "~" {
        return Some("\"$(wslpath -w \"$HOME\")\"".to_owned());
    }

    if let Some(path) = path.strip_prefix("~/") {
        let path = ShellKind::Posix.try_quote(path)?;
        return Some(format!("\"$(wslpath -w \"$HOME/$(printf %s {path})\")\""));
    }

    let path = ShellKind::Posix.try_quote(path)?;
    Some(format!(
        "\"$(wslpath -w \"$(pwd -P)/$(printf %s {path})\")\""
    ))
}

#[cfg(windows)]
fn cygwin_editor_path_argument(
    path: &str,
    path_style: PathStyle,
    shell_kind: ShellKind,
) -> Option<String> {
    if path.starts_with('/') {
        let path = shell_kind.try_quote(path)?;
        return Some(match shell_kind {
            ShellKind::Nushell => format!("(cygpath -w {path})"),
            ShellKind::Fish => format!("(cygpath -w {path})"),
            _ => format!("\"$(cygpath -w {path})\""),
        });
    }

    if path_style.is_posix() && !PathStyle::Windows.is_absolute(path) {
        if path == "~" {
            return Some(match shell_kind {
                ShellKind::Nushell => "(cygpath -w $env.HOME)".to_owned(),
                ShellKind::Fish => "(cygpath -w $HOME)".to_owned(),
                _ => "\"$(cygpath -w \"$HOME\")\"".to_owned(),
            });
        }
        if let Some(path) = path.strip_prefix("~/") {
            let path = shell_kind.try_quote(path)?;
            return Some(match shell_kind {
                ShellKind::Nushell => format!("(cygpath -w ($env.HOME | path join {path}))"),
                ShellKind::Fish => {
                    format!("(cygpath -w (string join / $HOME {path}))")
                }
                _ => format!("\"$(cygpath -w \"$HOME/$(printf %s {path})\")\""),
            });
        }
        let path = shell_kind.try_quote(path)?;
        return Some(match shell_kind {
            ShellKind::Nushell => format!("(cygpath -w ((pwd) | path join {path}))"),
            ShellKind::Fish => format!("(cygpath -w (string join / (pwd -P) {path}))"),
            _ => format!("\"$(cygpath -w \"$(pwd -P)/$(printf %s {path})\")\""),
        });
    }

    shell_kind.try_quote(path).map(Into::into)
}

fn interaction_shell_kind(shell: &Shell, path_style: PathStyle) -> ShellKind {
    #[cfg(windows)]
    if let Some(host) = posix_host(shell) {
        return match host {
            PosixHost::Cygwin => cygwin_shell_kind(shell),
            PosixHost::Msys2 | PosixHost::Wsl => ShellKind::Posix,
        };
    }
    shell.shell_kind(path_style.is_windows())
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PosixHost {
    Msys2,
    Wsl,
    Cygwin,
}

#[cfg(windows)]
fn cygwin_root_from_program(program: &str) -> Option<PathBuf> {
    let program = Path::new(program);
    let bin = program.parent()?;
    if !bin
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
    {
        return None;
    }
    bin.parent().map(Path::to_path_buf)
}

#[cfg(windows)]
fn cygwin_program_name(program: &str) -> bool {
    let name = windows_shell_program_name(program);
    [
        "bash",
        "bash.exe",
        "zsh",
        "zsh.exe",
        "fish",
        "fish.exe",
        "nu",
        "nu.exe",
        "nushell",
        "nushell.exe",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

#[cfg(windows)]
fn cygwin_title(title: Option<&str>) -> bool {
    title.is_some_and(|title| {
        ["Cygwin", "Cygwin: Zsh", "Cygwin: Fish", "Cygwin: Nushell"]
            .iter()
            .any(|candidate| title.eq_ignore_ascii_case(candidate))
    })
}

#[cfg(windows)]
fn cygwin_shell_kind(shell: &Shell) -> ShellKind {
    let program = shell.program();
    let name = windows_shell_program_name(&program);
    let name = name.to_ascii_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "fish" => ShellKind::Fish,
        "nu" | "nushell" => ShellKind::Nushell,
        _ => ShellKind::Posix,
    }
}

#[cfg(windows)]
fn posix_host(shell: &Shell) -> Option<PosixHost> {
    let (program, arguments, title) = match shell {
        Shell::System => return None,
        Shell::Program(program) => (program, &[][..], None),
        Shell::WithArguments {
            program,
            args,
            title_override,
        } => (program, args.as_slice(), title_override.as_deref()),
    };
    if program.rsplit(['/', '\\']).next().is_some_and(|name| {
        name.eq_ignore_ascii_case("wsl.exe") || name.eq_ignore_ascii_case("wsl")
    }) {
        return Some(PosixHost::Wsl);
    }
    if arguments
        .iter()
        .any(|argument| argument.to_ascii_lowercase().contains("msys2_shell.cmd"))
    {
        return Some(PosixHost::Msys2);
    }

    let root = cygwin_root_from_program(program)?;
    if cygwin_program_name(program)
        && (cygwin_title(title) || root.join("bin").join("cygwin1.dll").is_file())
    {
        Some(PosixHost::Cygwin)
    } else {
        None
    }
}

fn zetta_command_for_shell(shell: &Shell) -> Option<String> {
    #[cfg(windows)]
    if let Some(host) = posix_host(shell) {
        return match host {
            PosixHost::Wsl => Some("\"$ZETTA_HOST_EXECUTABLE\"".to_owned()),
            PosixHost::Msys2 => native_zetta_command_for_msys2(&std::env::current_exe().ok()?),
            PosixHost::Cygwin => {
                let executable = std::env::current_exe().ok()?;
                let shell_kind = interaction_shell_kind(shell, PathStyle::local());
                if shell_kind == ShellKind::Posix {
                    native_zetta_command_for_cygwin(&executable)
                } else {
                    native_zetta_command_for_cygwin_with_shell(&executable, shell_kind)
                }
            }
        };
    }
    #[cfg(not(windows))]
    let _ = shell;
    Some("zetta".to_owned())
}

#[cfg(windows)]
fn native_zetta_command_for_msys2(executable: &Path) -> Option<String> {
    native_zetta_command_for_cygwin_with_shell(executable, ShellKind::Posix)
}

#[cfg(windows)]
fn native_zetta_command_for_cygwin(executable: &Path) -> Option<String> {
    native_zetta_command_for_cygwin_with_shell(executable, ShellKind::Posix)
}

#[cfg(windows)]
fn native_zetta_command_for_cygwin_with_shell(
    executable: &Path,
    shell_kind: ShellKind,
) -> Option<String> {
    let executable = ShellKind::Posix.try_quote(executable.to_str()?)?;
    Some(match shell_kind {
        ShellKind::Nushell => format!("let zetta = (^cygpath -u {executable}); ^$zetta"),
        ShellKind::Fish => format!("(cygpath -u {executable})"),
        _ => format!("\"$(cygpath -u {executable})\""),
    })
}

#[cfg(windows)]
fn wsl_editor_working_directory(shell: &Shell, directory: Option<&str>) -> Option<PathBuf> {
    if !matches!(posix_host(shell), Some(PosixHost::Wsl)) {
        return None;
    }
    directory
        .filter(|directory| directory.starts_with('/'))
        .map(PathBuf::from)
}

#[cfg(windows)]
fn cygwin_path_to_windows(root: &Path, directory: &str) -> Option<PathBuf> {
    if !directory.starts_with('/') || directory.chars().any(char::is_control) {
        return None;
    }
    let parts = directory
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| matches!(*part, "." | "..") || part.contains(['\\', ':']))
    {
        return None;
    }
    if directory.starts_with("//") {
        return (parts.len() >= 2)
            .then(|| PathBuf::from(format!(r"\\{}\{}", parts[0], parts[1..].join(r"\"))));
    }
    if parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case("cygdrive"))
    {
        let drive = parts.get(1)?;
        if drive.len() != 1 || !drive.as_bytes()[0].is_ascii_alphabetic() {
            return None;
        }
        let mut path = PathBuf::from(format!(r"{}:\", drive.to_ascii_uppercase()));
        path.extend(&parts[2..]);
        return Some(path);
    }
    let mut path = root.to_path_buf();
    path.extend(parts);
    Some(path)
}

#[cfg(windows)]
fn cygwin_path_like_to_windows(shell: &Shell, value: &str) -> Option<String> {
    if !matches!(posix_host(shell), Some(PosixHost::Cygwin)) {
        return None;
    }
    let root = cygwin_root_from_program(&shell.program())?;
    let value = PathWithPosition::parse_str(value);
    let path = value.path.to_str()?;
    let path = cygwin_path_to_windows(&root, path)?;
    Some(
        PathWithPosition {
            path,
            row: value.row,
            column: value.column,
        }
        .to_string(&|path| path.to_string_lossy().into_owned()),
    )
}

#[cfg(windows)]
fn cygwin_editor_working_directory(shell: &Shell, directory: Option<&str>) -> Option<PathBuf> {
    if !matches!(posix_host(shell), Some(PosixHost::Cygwin)) {
        return None;
    }
    let program = shell.program();
    let root = cygwin_root_from_program(&program)?;
    cygwin_path_to_windows(&root, directory?)
}

/// Whether the modifiers should activate a terminal hyperlink.
///
/// Control-click is supported on every platform. Command-click remains
/// supported on macOS as the platform-standard equivalent.
pub fn is_hyperlink_modifier(modifiers: &Modifiers) -> bool {
    modifiers.control || modifiers.secondary()
}

#[derive(Clone)]
enum InternalEvent {
    Resize {
        bounds: TerminalBounds,
        reflow: bool,
    },
    Clear,
    // FocusNextMatch,
    Scroll(Scroll),
    ScrollToPoint(Point),
    SetSelection(Option<Selection>),
    UpdateSelection(GpuiPoint<Pixels>),
    FindHyperlink(GpuiPoint<Pixels>, bool),
    ProcessHyperlink(HyperlinkMatch, bool),
    // Whether keep selection when copy
    Copy(Option<bool>),
    // Vi mode events
    ToggleViMode,
    ViMotion(ViMotion),
    MoveViCursorToPoint(Point),
}

type ClipboardFormatter = Arc<dyn Fn(&str) -> String + Sync + Send + 'static>;
type ColorFormatter = Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>;
type TextAreaSizeFormatter = Arc<dyn Fn(TerminalBounds) -> String + Sync + Send + 'static>;

#[derive(Clone)]
pub(crate) enum TerminalBackendEvent {
    MouseCursorDirty,
    Title(String),
    ResetTitle,
    ClipboardStore(String),
    ClipboardLoad(ClipboardFormatter),
    ColorRequest(usize, ColorFormatter),
    PtyWrite(String),
    TextAreaSizeRequest(TextAreaSizeFormatter),
    ResizeRequest { rows: usize, columns: usize },
    CursorBlinkingChange,
    Wakeup,
    Bell,
    Exit,
    ChildExit(ExitStatus),
    ChildExitStatusUnavailable,
    ChildWatcherDisconnected,
    BackendShutdown,
}

const REPORTED_WORKING_DIRECTORY_TITLE_PREFIX: &str = "zetta-cwd:";
const REPORTED_FOREGROUND_COMMAND_TITLE_PREFIX: &str = "zetta-cmd:";

#[cfg(any(windows, test))]
const POWERSHELL_CWD_TRACKER: &str = include_str!("terminal/powershell_cwd_tracker.ps1");

#[cfg(any(windows, test))]
fn visible_process_argv(argv: &[String]) -> &[String] {
    let Some(command_index) = argv.len().checked_sub(2) else {
        return argv;
    };
    if !argv[command_index].eq_ignore_ascii_case("-Command")
        || argv[command_index + 1] != POWERSHELL_CWD_TRACKER
    {
        return argv;
    }

    let visible_end = command_index
        .checked_sub(1)
        .filter(|index| *index > 0 && argv[*index].eq_ignore_ascii_case("-NoExit"))
        .unwrap_or(command_index);
    &argv[..visible_end]
}

#[cfg(not(any(windows, test)))]
fn visible_process_argv(argv: &[String]) -> &[String] {
    argv
}

#[cfg(windows)]
fn windows_shell_program_name(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

#[cfg(windows)]
fn is_windows_shell_program(program: &str, names: &[&str]) -> bool {
    let program = windows_shell_program_name(program);
    names.iter().any(|name| program.eq_ignore_ascii_case(name))
}

#[cfg(windows)]
fn powershell_has_command_arguments(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| {
        matches!(
            argument.to_ascii_lowercase().as_str(),
            "-c" | "-command"
                | "-commandwithargs"
                | "-cwa"
                | "-encodedarguments"
                | "-encodedcommand"
                | "-ec"
                | "-f"
                | "-file"
        )
    })
}

#[cfg(windows)]
fn cmd_prompt_with_cwd_tracking(existing: Option<&str>) -> String {
    format!(
        "$E]2;zetta-cwd:$P$E\\{}",
        existing
            .filter(|prompt| !prompt.is_empty())
            .unwrap_or("$P$G")
    )
}

#[cfg(windows)]
fn install_windows_cwd_tracking(
    program: &str,
    arguments: &mut Option<Vec<String>>,
    environment: &mut HashMap<String, String>,
) {
    if is_windows_shell_program(program, &["cmd", "cmd.exe"]) {
        let inherited_prompt = environment
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("PROMPT"))
            .map(|(_, prompt)| prompt.clone())
            .or_else(|| std::env::var("PROMPT").ok());
        environment.insert(
            "PROMPT".to_owned(),
            cmd_prompt_with_cwd_tracking(inherited_prompt.as_deref()),
        );
        return;
    }

    if !is_windows_shell_program(
        program,
        &["powershell", "powershell.exe", "pwsh", "pwsh.exe"],
    ) {
        return;
    }

    // This tracker is injected into ordinary interactive PowerShell panes, so
    // CWD and prompt-style reporting does not depend on the optional profile
    // integration installed by `zetta init powershell`.
    let arguments = arguments.get_or_insert_default();
    if powershell_has_command_arguments(arguments) {
        return;
    }
    if !arguments
        .iter()
        .any(|argument| argument.eq_ignore_ascii_case("-NoExit"))
    {
        arguments.push("-NoExit".to_owned());
    }
    arguments.extend(["-Command".to_owned(), POWERSHELL_CWD_TRACKER.to_owned()]);
}

fn reported_working_directory_from_title(title: &str) -> Option<String> {
    let directory = title.strip_prefix(REPORTED_WORKING_DIRECTORY_TITLE_PREFIX)?;
    if directory.chars().any(char::is_control) {
        return None;
    }
    let is_unix_absolute = directory.starts_with('/');
    let is_native_absolute = Path::new(directory).is_absolute();
    (is_unix_absolute || is_native_absolute).then(|| directory.to_owned())
}

/// Parses the `zetta-cmd:<command>` marker that WSL, MSYS2, and Cygwin sessions report
/// via prompt/preexec shell hooks. Windows-side process inspection cannot see
/// into WSL and does not reliably represent MSYS2's POSIX process hierarchy.
fn reported_foreground_command_from_title(title: &str) -> Option<String> {
    let command = title.strip_prefix(REPORTED_FOREGROUND_COMMAND_TITLE_PREFIX)?;
    if command.chars().any(char::is_control) {
        return None;
    }
    Some(command.to_owned())
}

/// Reduces a shell-reported command marker to a safe command name for exit
/// diagnostics. The full marker is intentionally not copied into failure
/// messages because it may contain user arguments or secrets.
#[cfg(windows)]
fn reported_foreground_command_name(command: &str) -> Option<String> {
    let command = command.split_whitespace().next()?;
    // A relative path is not a command name. In particular, do not turn a
    // shell-reported `./program` into a seemingly trustworthy bare name after
    // stripping its path; absolute Windows paths are still reduced to their
    // executable name below.
    if command.starts_with('.') {
        return None;
    }
    let command = command.rsplit(['/', '\\']).next().unwrap_or(command);
    normalize_path_command_name(command)
}

impl fmt::Debug for TerminalBackendEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MouseCursorDirty => f.write_str("MouseCursorDirty"),
            Self::Title(title) => write!(f, "Title({title})"),
            Self::ResetTitle => f.write_str("ResetTitle"),
            Self::ClipboardStore(data) => write!(f, "ClipboardStore({data})"),
            Self::ClipboardLoad(_) => f.write_str("ClipboardLoad"),
            Self::ColorRequest(index, _) => write!(f, "ColorRequest({index})"),
            Self::PtyWrite(output) => write!(f, "PtyWrite({output})"),
            Self::TextAreaSizeRequest(_) => f.write_str("TextAreaSizeRequest"),
            Self::ResizeRequest { rows, columns } => {
                write!(f, "ResizeRequest({columns}x{rows})")
            }
            Self::CursorBlinkingChange => f.write_str("CursorBlinkingChange"),
            Self::Wakeup => f.write_str("Wakeup"),
            Self::Bell => f.write_str("Bell"),
            Self::Exit => f.write_str("Exit"),
            Self::ChildExit(status) => write!(f, "ChildExit({status})"),
            Self::ChildExitStatusUnavailable => f.write_str("ChildExitStatusUnavailable"),
            Self::ChildWatcherDisconnected => f.write_str("ChildWatcherDisconnected"),
            Self::BackendShutdown => f.write_str("BackendShutdown"),
        }
    }
}

enum PtyEvent {
    Event(TerminalBackendEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalBounds {
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub bounds: Bounds<Pixels>,
}

impl TerminalBounds {
    pub fn new(line_height: Pixels, cell_width: Pixels, bounds: Bounds<Pixels>) -> Self {
        TerminalBounds {
            cell_width,
            line_height,
            bounds,
        }
    }

    pub fn num_lines(&self) -> usize {
        // Tolerance to prevent f32 precision from losing a row:
        // `N * line_height / line_height` can be N-epsilon, which floor()
        // would round down, pushing the first line into invisible scrollback.
        let raw = self.bounds.size.height / self.line_height;
        raw.next_up().floor() as usize
    }

    pub fn num_columns(&self) -> usize {
        let raw = self.bounds.size.width / self.cell_width;
        raw.next_up().floor() as usize
    }

    pub fn height(&self) -> Pixels {
        self.bounds.size.height
    }

    pub fn width(&self) -> Pixels {
        self.bounds.size.width
    }

    pub fn cell_width(&self) -> Pixels {
        self.cell_width
    }

    pub fn line_height(&self) -> Pixels {
        self.line_height
    }
}

impl Default for TerminalBounds {
    fn default() -> Self {
        TerminalBounds::new(
            DEBUG_LINE_HEIGHT,
            DEBUG_CELL_WIDTH,
            Bounds {
                origin: GpuiPoint::default(),
                size: Size {
                    width: DEBUG_TERMINAL_WIDTH,
                    height: DEBUG_TERMINAL_HEIGHT,
                },
            },
        )
    }
}

fn normalize_terminal_bounds(mut bounds: TerminalBounds) -> TerminalBounds {
    bounds.bounds.size.height = cmp::max(bounds.line_height, bounds.height());
    bounds.bounds.size.width = cmp::max(bounds.cell_width, bounds.width());
    bounds
}

#[derive(Error, Debug)]
pub struct TerminalError {
    pub directory: Option<PathBuf>,
    pub program: Option<String>,
    pub args: Option<Vec<String>>,
    pub title_override: Option<String>,
    pub source: std::io::Error,
}

impl TerminalError {
    fn fmt_directory(&self) -> String {
        self.directory
            .clone()
            .map(|path| {
                match path
                    .into_os_string()
                    .into_string()
                    .map_err(|os_str| format!("<non-utf8 path> {}", os_str.to_string_lossy()))
                {
                    Ok(s) => s,
                    Err(s) => s,
                }
            })
            .unwrap_or_else(|| "<none specified>".to_string())
    }

    fn fmt_shell(&self) -> String {
        if let Some(title_override) = &self.title_override {
            format!(
                "{} {} ({})",
                self.program.as_deref().unwrap_or("<system defined shell>"),
                self.args.as_ref().into_iter().flatten().format(" "),
                title_override
            )
        } else {
            format!(
                "{} {}",
                self.program.as_deref().unwrap_or("<system defined shell>"),
                self.args.as_ref().into_iter().flatten().format(" ")
            )
        }
    }
}

impl Display for TerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dir_string: String = self.fmt_directory();
        let shell = self.fmt_shell();

        write!(
            f,
            "Working directory: {} Shell command: `{}`, IOError: {}",
            dir_string, shell, self.source
        )
    }
}

// Alacritty's grid stores line coordinates as i32 and grows history lazily.
const DEFAULT_SCROLL_HISTORY_LINES: usize = i32::MAX as usize;
pub const MAX_SCROLL_HISTORY_LINES: usize = i32::MAX as usize;
// Preserve upstream's immediate-first-event behavior and short bounded drains. Deferring the
// first event to a display-frame cadence couples PTY progress to rendering and can make a busy
// terminal monopolize the foreground executor.
#[cfg(not(any(test, feature = "test-support")))]
const TERMINAL_EVENT_DRAIN_INTERVAL: Duration = Duration::from_millis(4);
const MAX_TERMINAL_EVENTS_PER_BATCH: usize = 100;
// Reflow cost scales with every retained cell and runs synchronously during paint. Above this
// budget, preserve logical rows during width changes so dragging a window cannot monopolize the
// UI thread for a large scrollback buffer.
const MAX_SYNCHRONOUS_REFLOW_CELLS: usize = 1_000_000;
const TERMINAL_SYNC_STALL_THRESHOLD: Duration = Duration::from_secs(2);
const TERMINAL_SYNC_IDLE: u8 = 0;
const TERMINAL_SYNC_WAITING_FOR_GRID: u8 = 1;
const TERMINAL_SYNC_BUILDING_CONTENT: u8 = 2;
static TERMINAL_SYNC_WATCHDOG: Once = Once::new();
static TERMINAL_SYNC_STARTED_AT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static TERMINAL_SYNC_STARTED_MS: AtomicU64 = AtomicU64::new(0);
static TERMINAL_SYNC_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const SEARCH_CHUNK_LINES: usize = 2_048;
// Rendering thousands of highlighted terminal ranges monopolizes the UI thread
// and prevents subsequent query keystrokes from being handled. This is a
// highlight/navigation cap, not a minimum-query-length restriction: selective
// queries still scan the complete snapshot and report their exact match count.
const MAX_SEARCH_MATCHES: usize = 256;
static TERMINAL_SYNC_PHASE: AtomicU8 = AtomicU8::new(TERMINAL_SYNC_IDLE);
static NEXT_INIT_COMMAND_STARTUP_MARKER_ID: AtomicU64 = AtomicU64::new(1);

fn synchronous_reflow_is_bounded(history_lines: usize, columns: usize) -> bool {
    history_lines.saturating_mul(columns) <= MAX_SYNCHRONOUS_REFLOW_CELLS
}

fn terminal_diagnostic_millis() -> u64 {
    TERMINAL_SYNC_STARTED_AT
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX - 1)) as u64
        + 1
}

fn terminal_sync_phase_name(phase: u8) -> &'static str {
    match phase {
        TERMINAL_SYNC_WAITING_FOR_GRID => "waiting for the terminal grid lock",
        TERMINAL_SYNC_BUILDING_CONTENT => "building the renderable terminal snapshot",
        _ => "idle",
    }
}

fn start_terminal_sync_watchdog() {
    TERMINAL_SYNC_WATCHDOG.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("terminal-watchdog".to_owned())
            .spawn(|| {
                let mut reported_sequence = 0;
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    let phase = TERMINAL_SYNC_PHASE.load(Ordering::Acquire);
                    if phase == TERMINAL_SYNC_IDLE {
                        continue;
                    }
                    let sequence = TERMINAL_SYNC_SEQUENCE.load(Ordering::Acquire);
                    if sequence == reported_sequence {
                        continue;
                    }
                    let started_ms = TERMINAL_SYNC_STARTED_MS.load(Ordering::Acquire);
                    let elapsed_ms = terminal_diagnostic_millis().saturating_sub(started_ms);
                    if elapsed_ms >= TERMINAL_SYNC_STALL_THRESHOLD.as_millis() as u64 {
                        eprintln!(
                            "Zetta diagnostic: terminal sync stalled for {elapsed_ms} ms while {} (sequence {sequence})",
                            terminal_sync_phase_name(phase),
                        );
                        reported_sequence = sequence;
                    }
                }
            });
    });
}

struct TerminalSyncDiagnostic;

impl TerminalSyncDiagnostic {
    fn begin() -> Self {
        start_terminal_sync_watchdog();
        TERMINAL_SYNC_SEQUENCE.fetch_add(1, Ordering::AcqRel);
        TERMINAL_SYNC_STARTED_MS.store(terminal_diagnostic_millis(), Ordering::Release);
        TERMINAL_SYNC_PHASE.store(TERMINAL_SYNC_WAITING_FOR_GRID, Ordering::Release);
        Self
    }

    fn acquired_grid(&self) {
        TERMINAL_SYNC_PHASE.store(TERMINAL_SYNC_BUILDING_CONTENT, Ordering::Release);
    }
}

impl Drop for TerminalSyncDiagnostic {
    fn drop(&mut self) {
        TERMINAL_SYNC_PHASE.store(TERMINAL_SYNC_IDLE, Ordering::Release);
    }
}

const INIT_COMMAND_STARTUP_MARKER_PREFIX: &str = "__zed_init_command_ready_";
const INIT_COMMAND_STARTUP_MARKER_SUFFIX: &str = "__";
const INIT_COMMAND_STARTUP_MARKER_SEARCH_LINES: usize = 64;

#[cfg(windows)]
struct WslStartupTiming {
    started_at: Instant,
    pty_ready_at: Instant,
}

#[cfg(windows)]
fn log_wsl_startup_phase(
    phase: &str,
    started_at: Instant,
    pty_ready_at: Instant,
    observed_at: Instant,
) {
    log::debug!(
        "WSL terminal startup phase={phase} spawn_to_pty_ready_ms={} pty_ready_to_marker_ms={} total_ms={}",
        pty_ready_at
            .saturating_duration_since(started_at)
            .as_millis(),
        observed_at
            .saturating_duration_since(pty_ready_at)
            .as_millis(),
        observed_at
            .saturating_duration_since(started_at)
            .as_millis(),
    );
}

fn init_command_startup_marker(marker_id: u64) -> String {
    format!("{INIT_COMMAND_STARTUP_MARKER_PREFIX}{marker_id}{INIT_COMMAND_STARTUP_MARKER_SUFFIX}")
}

fn init_command_startup_marker_command(shell_kind: ShellKind, marker_id: u64) -> String {
    // Split the marker across the command so its echo can't satisfy the
    // handshake; only the command's output contains the contiguous marker.
    match shell_kind {
        ShellKind::PowerShell | ShellKind::Pwsh => format!(
            "Write-Output ('{INIT_COMMAND_STARTUP_MARKER_PREFIX}' + '{marker_id}' + '{INIT_COMMAND_STARTUP_MARKER_SUFFIX}')"
        ),
        ShellKind::Cmd => {
            format!(
                "<nul set /p zed_init_ready={INIT_COMMAND_STARTUP_MARKER_PREFIX}&echo {marker_id}{INIT_COMMAND_STARTUP_MARKER_SUFFIX}"
            )
        }
        ShellKind::Nushell => {
            format!(
                "print $\"{INIT_COMMAND_STARTUP_MARKER_PREFIX}({marker_id}){INIT_COMMAND_STARTUP_MARKER_SUFFIX}\""
            )
        }
        ShellKind::Posix
        | ShellKind::Csh
        | ShellKind::Tcsh
        | ShellKind::Rc
        | ShellKind::Fish
        | ShellKind::Xonsh
        | ShellKind::Elvish => format!(
            "printf '%s%s%s\\n' {INIT_COMMAND_STARTUP_MARKER_PREFIX} {marker_id} {INIT_COMMAND_STARTUP_MARKER_SUFFIX}"
        ),
    }
}

pub struct TerminalBuilder {
    terminal: Terminal,
    events_tx: futures::channel::mpsc::UnboundedSender<PtyEvent>,
    events_rx: UnboundedReceiver<PtyEvent>,
    /// Present when the PTY came from a provider. The caller has to route the
    /// provider's exit reports into this, or the terminal will never learn that
    /// its process ended.
    child_events: Option<alacritty_terminal::tty::AttachedChildEvents>,
    /// Set when the multiplexer was meant to open this terminal and could not,
    /// so it was opened locally instead.
    multiplexer_error: Option<String>,
}

impl TerminalBuilder {
    /// Takes the reporting end of an attached PTY's child events, if this
    /// terminal has one.
    pub fn take_child_events(&mut self) -> Option<alacritty_terminal::tty::AttachedChildEvents> {
        self.child_events.take()
    }

    /// Why this terminal was opened locally despite a multiplexer being asked
    /// for. `None` when the multiplexer opened it, or was never asked.
    pub fn multiplexer_error(&self) -> Option<&str> {
        self.multiplexer_error.as_deref()
    }
}

/// Asks `provider` for a PTY and turns it into one this process can drive.
///
/// The replay is processed into the grid here, before the event loop starts, so
/// the first frame already shows what the provider had retained rather than
/// painting a blank terminal and filling it in a frame later.
#[cfg(unix)]
fn open_provided_pty(
    provider: &dyn PtyProvider,
    pty_options: &alacritty_terminal::tty::Options,
    shell: Option<(String, Vec<String>)>,
    working_directory: Option<PathBuf>,
    env: HashMap<String, String>,
    term: &Arc<AlacrittyTermLock>,
    output_processor: &mut Processor<StdSyncHandler>,
    console_palette: ConsolePalette,
) -> Result<(
    AlacrittyPty,
    Option<alacritty_terminal::tty::AttachedChildEvents>,
    Vec<u8>,
    Option<Arc<dyn PtyControl>>,
)> {
    let _ = pty_options;
    let (program, args) = match shell {
        Some((program, args)) => (Some(program), args),
        None => (None, Vec::new()),
    };
    let handover = provider.open(PtySpawnRequest {
        program,
        args,
        env,
        working_directory,
        console_palette,
    })?;
    let (pty, child_events) =
        alacritty_terminal::tty::attach(handover.descriptor, handover.child_pid)?;
    let _ = (term, output_processor);
    Ok((
        pty,
        Some(child_events),
        handover.replay,
        Some(handover.control),
    ))
}

#[cfg(windows)]
fn open_provided_pty(
    provider: &dyn PtyProvider,
    pty_options: &alacritty_terminal::tty::Options,
    shell: Option<(String, Vec<String>)>,
    working_directory: Option<PathBuf>,
    env: HashMap<String, String>,
    term: &Arc<AlacrittyTermLock>,
    output_processor: &mut Processor<StdSyncHandler>,
    console_palette: ConsolePalette,
) -> Result<(
    AlacrittyPty,
    Option<alacritty_terminal::tty::AttachedChildEvents>,
    Vec<u8>,
    Option<Arc<dyn PtyControl>>,
)> {
    let _ = pty_options;
    let (program, args) = match shell {
        Some((program, args)) => (Some(program), args),
        None => (None, Vec::new()),
    };
    let handover = provider.open(PtySpawnRequest {
        program,
        args,
        env,
        working_directory,
        console_palette,
    })?;
    let (pty, child_events) =
        alacritty_terminal::tty::attach(handover.conout, handover.conin, handover.child_pid)?;
    if !handover.replay.is_empty() {
        output_processor.advance(&mut *term.lock(), &handover.replay);
    }
    // Windows has already replayed the retained output into the terminal,
    // unlike Unix, which defers it until the terminal has been laid out.
    Ok((pty, Some(child_events), Vec::new(), Some(handover.control)))
}

#[cfg(not(any(unix, windows)))]
fn open_provided_pty(
    _provider: &dyn PtyProvider,
    _pty_options: &alacritty_terminal::tty::Options,
    _shell: Option<(String, Vec<String>)>,
    _working_directory: Option<PathBuf>,
    _env: HashMap<String, String>,
    _term: &Arc<AlacrittyTermLock>,
    _output_processor: &mut Processor<StdSyncHandler>,
    _console_palette: ConsolePalette,
) -> Result<(
    AlacrittyPty,
    Option<alacritty_terminal::tty::AttachedChildEvents>,
    Vec<u8>,
    Option<Arc<dyn PtyControl>>,
)> {
    anyhow::bail!("the multiplexer cannot hand over a console on this platform")
}

/// Opens the PTY a terminal runs on.
///
/// Exists so that spawning through the multiplexer changes exactly one step.
/// Everything around it — resolving the shell, the environment, WSL and MSYS2
/// handling, the activation script — is identical either way and stays in one
/// place; duplicating it for a second spawn path is how the two would drift.
///
/// The application implements this over its multiplexer client, which is also
/// why it is a trait: this crate has no business knowing how that client talks
/// to anything.
pub trait PtyProvider: Send + Sync {
    fn open(&self, request: PtySpawnRequest) -> Result<PtyHandover>;
}

/// Operations that must be routed to the process which owns one specific PTY.
///
/// A handover carries this controller with the handles, so reattachments and
/// shared panes never depend on mutable provider state from a previous open.
pub trait PtyControl: Send + Sync {
    fn resize(&self, columns: u16, lines: u16);
    fn set_console_palette(&self, palette: ConsolePalette);
}

impl PtyControl for PtySender {
    fn resize(&self, columns: u16, lines: u16) {
        self.resize_cells(columns, lines);
    }

    fn set_console_palette(&self, palette: ConsolePalette) {
        #[cfg(windows)]
        PtySender::set_console_palette(self, palette);
        #[cfg(not(windows))]
        let _ = palette;
    }
}

pub struct PtySpawnRequest {
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub working_directory: Option<PathBuf>,
    pub console_palette: ConsolePalette,
}

/// A PTY opened elsewhere, handed to this process to read and write.
///
/// What crosses the boundary differs by platform: a single descriptor for the
/// terminal on Unix, and the pseudoconsole's two pipes on Windows, where the
/// console handle itself cannot be shared and stays with its creator.
pub struct PtyHandover {
    #[cfg(unix)]
    pub descriptor: std::os::fd::OwnedFd,
    #[cfg(windows)]
    pub conout: std::os::windows::io::OwnedHandle,
    #[cfg(windows)]
    pub conin: std::os::windows::io::OwnedHandle,
    pub child_pid: u32,
    /// Anything the provider retained before handing over, replayed into the
    /// grid before the terminal is shown.
    pub replay: Vec<u8>,
    pub control: Arc<dyn PtyControl>,
}

/// What a terminal needs to know about a session the multiplexer resolved on
/// its behalf. The shell and environment are carried through rather than
/// re-resolved, so a reattached pane reports the same command it was started
/// with and duplicating it produces the same terminal.
pub struct AttachedOptions {
    pub shell: Shell,
    pub env: HashMap<String, String>,
    pub cursor_shape: SettingsCursorShape,
    pub alternate_scroll: AlternateScroll,
    pub max_scroll_history_lines: Option<usize>,
    pub path_hyperlink_regexes: Vec<String>,
    pub path_hyperlink_timeout_ms: u64,
    pub window_id: u64,
}

pub struct AttachedTerminal {
    pub builder: TerminalBuilder,
    /// How the multiplexer's report of the process's exit reaches this
    /// terminal. Dropping it without reporting makes the terminal treat the
    /// session's watcher as lost, which is the honest outcome.
    pub child_events: alacritty_terminal::tty::AttachedChildEvents,
}

impl TerminalBuilder {
    pub fn new_display_only(
        cursor_shape: SettingsCursorShape,
        alternate_scroll: AlternateScroll,
        max_scroll_history_lines: Option<usize>,
        window_id: u64,
        background_executor: &BackgroundExecutor,
        path_style: PathStyle,
    ) -> TerminalBuilder {
        Self::new_display_only_with_bounds(
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            window_id,
            background_executor,
            path_style,
            TerminalBounds::default(),
        )
    }

    pub fn new_display_only_with_bounds(
        cursor_shape: SettingsCursorShape,
        alternate_scroll: AlternateScroll,
        max_scroll_history_lines: Option<usize>,
        window_id: u64,
        background_executor: &BackgroundExecutor,
        path_style: PathStyle,
        terminal_bounds: TerminalBounds,
    ) -> TerminalBuilder {
        let terminal_bounds = normalize_terminal_bounds(terminal_bounds);

        let scrolling_history = max_scroll_history_lines
            .unwrap_or(DEFAULT_SCROLL_HISTORY_LINES)
            .min(MAX_SCROLL_HISTORY_LINES);
        let config = display_only_term_config(scrolling_history, cursor_shape);

        let (events_tx, events_rx) = unbounded();
        let wakeup_gate = WakeupGate::new();
        let listener = ZedListener::new(events_tx.clone(), wakeup_gate.clone());
        let term = new_term(&config, terminal_bounds, listener, alternate_scroll);

        let terminal = Terminal {
            task: None,
            terminal_type: TerminalType::DisplayOnly,
            subprocess: None,
            byte_stream: None,
            pty_control: None,
            pty_control_is_local: false,
            console_palette_enabled: false,
            last_console_palette: None,
            events_tx: events_tx.clone(),
            completion_tx: None,
            term,
            wakeup_gate,
            term_config: config,
            output_processor: Processor::<StdSyncHandler>::new(),
            title_override: None,
            events: VecDeque::with_capacity(10),
            last_content: Content {
                terminal_bounds,
                ..Default::default()
            },
            last_mouse: None,
            mouse_down_position: None,
            matches: Arc::new(Vec::new()),
            content_dirty: true,
            content_revision: 0,
            reflow_on_next_resize: true,

            selection_head: None,
            breadcrumb_text: String::new(),
            scroll_px: px(0.),
            next_link_id: 0,
            selection_phase: SelectionPhase::Ended,
            hyperlink_regex_searches: RegexSearches::default(),
            vi_mode_enabled: false,
            is_remote_terminal: false,
            last_mouse_move_time: Instant::now(),
            last_hyperlink_search_position: None,
            mouse_down_hyperlink: None,
            editor_click_started: false,
            #[cfg(windows)]
            shell_program: None,
            #[cfg(windows)]
            wsl_startup_timing: None,
            activation_script: Vec::new(),
            template: CopyTemplate {
                pty_provider: None,
                shell: Shell::System,
                env: HashMap::default(),
                cursor_shape,
                alternate_scroll,
                max_scroll_history_lines,
                path_hyperlink_regexes: Vec::default(),
                path_hyperlink_timeout_ms: 0,
                window_id,
            },
            child_is_the_multiplexers: false,
            pending_replay: None,
            child_exited: None,
            child_process_ended: false,
            terminal_exit_reported: false,
            task_exit_code: None,
            keyboard_input_sent: false,
            init_command_startup_marker: None,
            init_command_startup_tx: None,
            event_loop_task: Task::ready(Ok(())),
            background_executor: background_executor.clone(),
            path_style,
            cwd_history: Vec::new(),
            pending_cwd_boundary: None,
            reported_theme: None,
            reported_working_directory: None,
            restored_working_directory: None,
            reported_foreground_command: None,
            reported_shell_command: None,
            #[cfg(any(test, feature = "test-support"))]
            input_log: Vec::new(),
            #[cfg(any(test, feature = "test-support"))]
            pty_write_log: Default::default(),
        };

        TerminalBuilder {
            terminal,
            events_tx,
            events_rx,
            child_events: None,
            multiplexer_error: None,
        }
    }

    /// Creates a terminal around a PTY the multiplexer already owns.
    ///
    /// Everything past this point is identical to a terminal this process
    /// spawned: the same event loop reads the same kind of descriptor, so
    /// output, input, resize and foreground-process tracking are unchanged and
    /// cost the same. Only two things differ, and both are in [`tty::attach`]:
    /// dropping this terminal leaves the process running, and its exit status
    /// arrives over [`AttachedTerminal::child_events`] rather than `waitpid`.
    ///
    /// `replay` is processed into the grid *before* the event loop starts, so
    /// the first frame already shows the restored screen rather than painting
    /// a blank terminal and filling it in afterwards.
    pub fn new_attached(
        handover: PtyHandover,
        options: AttachedOptions,
        background_executor: &BackgroundExecutor,
        path_style: PathStyle,
    ) -> Result<AttachedTerminal> {
        let AttachedOptions {
            shell,
            env,
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            path_hyperlink_regexes,
            path_hyperlink_timeout_ms,
            window_id,
        } = options;

        let mut builder = Self::new_display_only(
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            window_id,
            background_executor,
            path_style,
        );

        // A real terminal, not a display-only one: it must answer the
        // sequences a program expects a PTY-backed terminal to answer.
        let scrolling_history = max_scroll_history_lines
            .unwrap_or(DEFAULT_SCROLL_HISTORY_LINES)
            .min(MAX_SCROLL_HISTORY_LINES);
        builder.terminal.term_config = pty_term_config(scrolling_history, cursor_shape);
        apply_config(&builder.terminal.term, &builder.terminal.term_config);

        // Held rather than replayed now. The grid is still the placeholder
        // size at this point, so writing the restored screen into it would
        // wrap every line at the wrong width and then reflow again once the
        // pane is laid out — which is what turns a restored screen into a
        // mangled one.
        builder.terminal.pending_replay = (!handover.replay.is_empty()).then_some(handover.replay);

        let control = handover.control.clone();
        #[cfg(unix)]
        let (pty, child_events) =
            alacritty_terminal::tty::attach(handover.descriptor, handover.child_pid)
                .context("adopting the multiplexer's terminal")?;
        #[cfg(windows)]
        let (pty, child_events) =
            alacritty_terminal::tty::attach(handover.conout, handover.conin, handover.child_pid)
                .context("adopting the multiplexer's terminal")?;
        let info = PtyProcessInfo::new(ProcessIdGetter::from(&pty));
        let listener = ZedListener::new(
            builder.events_tx.clone(),
            builder.terminal.wakeup_gate.clone(),
        );
        let (pty_tx, io) = spawn_event_loop(builder.terminal.term.clone(), listener, pty, true)?;

        builder.terminal.terminal_type = TerminalType::Pty {
            pty_tx: Some(pty_tx),
            io: Some(io),
            info: Arc::new(info),
        };
        builder.terminal.pty_control = Some(control);
        builder.terminal.pty_control_is_local = false;
        builder.terminal.console_palette_enabled = cfg!(windows);
        builder.terminal.hyperlink_regex_searches =
            RegexSearches::new(&path_hyperlink_regexes, path_hyperlink_timeout_ms);
        // The daemon forked this child and is still its parent: this window is
        // borrowing the descriptor, and detaching the pane again — or quitting
        // — has to leave the session running.
        builder.terminal.child_is_the_multiplexers = true;
        builder.terminal.template = CopyTemplate {
            pty_provider: None,
            shell,
            env,
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            path_hyperlink_regexes,
            path_hyperlink_timeout_ms,
            window_id,
        };
        builder.terminal.content_dirty = true;

        Ok(AttachedTerminal {
            builder,
            child_events,
        })
    }

    /// Creates a terminal backed by an arbitrary blocking byte stream.
    ///
    /// The reader must periodically return (for example by using an I/O timeout)
    /// so dropping the terminal can stop its worker thread promptly.
    pub fn new_byte_stream(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        title: String,
        cursor_shape: SettingsCursorShape,
        alternate_scroll: AlternateScroll,
        max_scroll_history_lines: Option<usize>,
        window_id: u64,
        background_executor: &BackgroundExecutor,
        path_style: PathStyle,
    ) -> TerminalBuilder {
        let mut builder = Self::new_display_only(
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            window_id,
            background_executor,
            path_style,
        );
        builder.terminal.title_override = Some(title);
        builder.terminal.byte_stream = Some(spawn_byte_stream(
            reader,
            writer,
            builder.terminal.term.clone(),
            builder.events_tx.clone(),
            builder.terminal.wakeup_gate.clone(),
        ));
        builder
    }

    /// Replays `bytes` into the grid once it has been sized, like
    /// [`TerminalBuilder::new_attached`] replays a handover's retained output.
    ///
    /// The byte-stream constructor carries no handover, so a terminal built
    /// for a shared pane replays through this instead.
    pub fn with_replay(mut self, bytes: Vec<u8>) -> Self {
        self.terminal.pending_replay = (!bytes.is_empty()).then_some(bytes);
        self
    }

    /// Seeds the last known directory of a restored session. Live shell OSC
    /// metadata or foreground-process inspection takes precedence as soon as
    /// either becomes available, so this fills only the reconstruction gap.
    pub fn with_working_directory(mut self, directory: Option<PathBuf>) -> Self {
        self.terminal.restored_working_directory = directory.filter(|path| path.is_absolute());
        self
    }

    /// Installs the fixed-pane controller for a byte-stream-backed shared pane.
    pub fn with_pty_control(mut self, control: Arc<dyn PtyControl>) -> Self {
        self.terminal.pty_control = Some(control);
        self.terminal.console_palette_enabled = cfg!(windows);
        self
    }

    pub fn new(
        working_directory: Option<PathBuf>,
        task: Option<TaskState>,
        shell: Shell,
        env: HashMap<String, String>,
        cursor_shape: SettingsCursorShape,
        alternate_scroll: AlternateScroll,
        max_scroll_history_lines: Option<usize>,
        path_hyperlink_regexes: Vec<String>,
        path_hyperlink_timeout_ms: u64,
        is_remote_terminal: bool,
        window_id: u64,
        completion_tx: Option<Sender<Option<ExitStatus>>>,
        cx: &App,
        activation_script: Vec<String>,
        path_style: PathStyle,
        pty_provider: Option<Arc<dyn PtyProvider>>,
    ) -> Task<Result<TerminalBuilder>> {
        Self::new_with_console_palette(
            working_directory,
            task,
            shell,
            env,
            cursor_shape,
            alternate_scroll,
            max_scroll_history_lines,
            path_hyperlink_regexes,
            path_hyperlink_timeout_ms,
            is_remote_terminal,
            window_id,
            completion_tx,
            cx,
            activation_script,
            path_style,
            pty_provider,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_console_palette(
        working_directory: Option<PathBuf>,
        task: Option<TaskState>,
        shell: Shell,
        mut env: HashMap<String, String>,
        cursor_shape: SettingsCursorShape,
        alternate_scroll: AlternateScroll,
        max_scroll_history_lines: Option<usize>,
        path_hyperlink_regexes: Vec<String>,
        path_hyperlink_timeout_ms: u64,
        is_remote_terminal: bool,
        window_id: u64,
        completion_tx: Option<Sender<Option<ExitStatus>>>,
        cx: &App,
        activation_script: Vec<String>,
        path_style: PathStyle,
        pty_provider: Option<Arc<dyn PtyProvider>>,
        initial_console_palette: Option<ConsolePalette>,
    ) -> Task<Result<TerminalBuilder>> {
        let version = release_channel::AppVersion::global(cx);
        let background_executor = cx.background_executor().clone();
        #[cfg(windows)]
        let wsl_startup_started_at = Instant::now();
        #[cfg(windows)]
        let is_wsl_startup = matches!(posix_host(&shell), Some(PosixHost::Wsl));
        #[cfg(windows)]
        let console_palette_enabled = initial_console_palette.is_some() && !is_wsl_startup;
        #[cfg(not(windows))]
        let console_palette_enabled = false;
        let console_palette = initial_console_palette.unwrap_or_default();
        // Headless hosts (e.g. the eval CLI) have no controlling TTY, so PTY
        // allocation / acquiring a controlling terminal fails with `ENOTTY`.
        // When set, run the command as a plain subprocess instead.
        let no_pty = HeadlessTerminal::is_enabled(cx);
        #[cfg(not(windows))]
        let child_signal_mask = match current_child_signal_mask()
            .context("failed to capture terminal child signal mask")
        {
            Ok(signal_mask) => Some(signal_mask),
            Err(error) => return Task::ready(Err(error)),
        };
        let fut = async move {
            // Remove SHLVL so the spawned shell initializes it to 1, matching
            // the behavior of standalone terminal emulators like iTerm2/Kitty/Alacritty.
            env.remove("SHLVL");

            // If the parent environment doesn't have a locale set
            // (As is the case when launched from a .app on MacOS),
            // and the Project doesn't have a locale set, then
            // set a fallback for our child environment to use.
            if std::env::var("LANG").is_err() {
                env.entry("LANG".to_string())
                    .or_insert_with(|| "en_US.UTF-8".to_string());
            }

            insert_zetta_terminal_env(&mut env, &version);

            #[derive(Default)]
            struct ShellParams {
                program: String,
                args: Option<Vec<String>>,
                title_override: Option<String>,
            }

            impl ShellParams {
                fn new(
                    program: String,
                    args: Option<Vec<String>>,
                    title_override: Option<String>,
                ) -> Self {
                    log::debug!("Using {program} as shell");
                    Self {
                        program,
                        args,
                        title_override,
                    }
                }
            }

            let shell_params = match shell.clone() {
                Shell::System => {
                    if cfg!(windows) {
                        Some(ShellParams::new(
                            util::shell::get_windows_system_shell(),
                            None,
                            None,
                        ))
                    } else {
                        None
                    }
                }
                Shell::Program(program) => Some(ShellParams::new(program, None, None)),
                Shell::WithArguments {
                    program,
                    args,
                    title_override,
                } => Some(ShellParams::new(program, Some(args), title_override)),
            };
            #[cfg(windows)]
            let mut shell_params = shell_params;
            #[cfg(windows)]
            if let Some(shell_params) = shell_params.as_mut() {
                install_windows_cwd_tracking(
                    &shell_params.program,
                    &mut shell_params.args,
                    &mut env,
                );
            }
            let terminal_title_override =
                shell_params.as_ref().and_then(|e| e.title_override.clone());

            #[cfg(windows)]
            let shell_program = shell_params.as_ref().map(|params| {
                use util::ResultExt;

                Self::resolve_path(&params.program)
                    .log_err()
                    .unwrap_or(params.program.clone())
            });

            // Note: when remoting, this shell_kind will scrutinize `ssh` or
            // `wsl.exe` as a shell and fall back to posix or powershell based on
            // the compilation target. This is fine right now due to the restricted
            // way we use the return value, but would become incorrect if we
            // supported remoting into windows.
            let shell_kind = shell.shell_kind(cfg!(windows));

            let scrolling_history = if task.is_some() {
                // Tasks like `cargo build --all` may produce a lot of output, ergo allow maximum scrolling.
                // After the task finishes, we do not allow appending to that terminal, so small tasks output should not
                // cause excessive memory usage over time.
                MAX_SCROLL_HISTORY_LINES
            } else {
                max_scroll_history_lines
                    .unwrap_or(DEFAULT_SCROLL_HISTORY_LINES)
                    .min(MAX_SCROLL_HISTORY_LINES)
            };
            let config = pty_term_config(scrolling_history, cursor_shape);

            //Spawn a task so the Alacritty EventLoop (or the subprocess reader) can communicate with us
            //TODO: Remove with a bounded sender which can be dispatched on &self
            let (events_tx, events_rx) = unbounded();
            let builder_events_tx = events_tx.clone();
            let wakeup_gate = WakeupGate::new();
            let listener = ZedListener::new(events_tx.clone(), wakeup_gate.clone());
            let mut output_processor = Processor::<StdSyncHandler>::new();
            // Set by the multiplexer-backed branch below: an attached child's
            // exit status can only come from the process that owns it.
            let mut child_events = None;
            // Written into the grid only once the pane has been laid out; see
            // `pending_replay` on `Terminal`.
            let mut pending_replay = None;
            // Why the multiplexer was not used, when it was meant to be. The
            // terminal still opens; the caller decides how loudly to say so.
            let mut multiplexer_error = None;
            //Set up the terminal...
            let term = new_term(
                &config,
                TerminalBounds::default(),
                listener.clone(),
                alternate_scroll,
            );

            // When `no_pty` is set (headless hosts), run the task as a plain
            // subprocess and pump its piped output into the same emulator the
            // PTY path would feed.
            #[cfg(windows)]
            let mut wsl_startup_timing = None;
            let (terminal_type, subprocess, pty_control, pty_control_is_local) = if no_pty {
                let (program, args) = match &shell_params {
                    Some(params) => (
                        params.program.clone(),
                        params.args.clone().unwrap_or_default(),
                    ),
                    None => (util::shell::get_system_shell(), Vec::new()),
                };
                let subprocess = match spawn_task_subprocess(
                    program,
                    args,
                    env.clone(),
                    working_directory.clone(),
                    term.clone(),
                    events_tx.clone(),
                    wakeup_gate.clone(),
                    &background_executor,
                ) {
                    Ok(subprocess) => subprocess,
                    Err(error) => {
                        bail!(TerminalError {
                            directory: working_directory,
                            program: shell_params.as_ref().map(|params| params.program.clone()),
                            args: shell_params.as_ref().and_then(|params| params.args.clone()),
                            title_override: terminal_title_override,
                            source: std::io::Error::other(format!("{error:#}")),
                        });
                    }
                };
                #[cfg(windows)]
                if is_wsl_startup {
                    let pty_ready_at = Instant::now();
                    wsl_startup_timing = Some(WslStartupTiming {
                        started_at: wsl_startup_started_at,
                        pty_ready_at,
                    });
                    log_wsl_startup_phase(
                        "subprocess_ready",
                        wsl_startup_started_at,
                        pty_ready_at,
                        pty_ready_at,
                    );
                }
                (TerminalType::DisplayOnly, Some(subprocess), None, false)
            } else {
                let alacritty_shell = shell_params.as_ref().map(|params| {
                    (
                        params.program.clone(),
                        params.args.clone().unwrap_or_default(),
                    )
                });
                let pty_options = pty_options(
                    alacritty_shell.clone(),
                    working_directory.clone(),
                    env.clone(),
                    // We pass in the foreground thread's signal mask to the child process via pty_options,
                    // so terminal construction can run on a background thread without breaking Ctrl-C and other signals
                    // otherwise the terminal would inherit the background executor's signal mask which blocks
                    // some terminal signals
                    #[cfg(not(windows))]
                    child_signal_mask,
                    #[cfg(windows)]
                    shell_kind.tty_escape_args(),
                    #[cfg(windows)]
                    console_palette,
                    #[cfg(windows)]
                    console_palette_enabled
                        .then(|| {
                            std::env::current_exe().ok().and_then(|path| {
                                path.parent().map(|parent| parent.join("zmux-pty.exe"))
                            })
                        })
                        .flatten(),
                );

                //Setup the pty...
                let mut opened = match &pty_provider {
                    // The multiplexer opens it and owns the child, so this
                    // process gets a descriptor rather than a process. What
                    // comes back is otherwise an ordinary PTY.
                    Some(provider) => open_provided_pty(
                        provider.as_ref(),
                        &pty_options,
                        alacritty_shell.clone(),
                        working_directory.clone(),
                        env.clone(),
                        &term,
                        &mut output_processor,
                        console_palette,
                    ),
                    None => open_pty(&pty_options, TerminalBounds::default(), window_id)
                        .map(|pty| (pty, None, Vec::new(), None))
                        .map_err(anyhow::Error::from),
                };
                // A multiplexer that cannot open the terminal must not stop it
                // from opening. Falling back to a local process costs the
                // session its ability to outlive this window, which is a far
                // smaller loss than a terminal that refuses to start.
                if let (Err(error), Some(_)) = (&opened, &pty_provider) {
                    log::warn!(
                        "the multiplexer could not open this terminal, starting it locally \
                         instead: {error:#}"
                    );
                    multiplexer_error = Some(format!("{error:#}"));
                    opened = open_pty(&pty_options, TerminalBounds::default(), window_id)
                        .map(|pty| (pty, None, Vec::new(), None))
                        .map_err(anyhow::Error::from);
                }
                let (pty, attached_child_events, replay, provided_control) = match opened {
                    Ok(opened) => opened,
                    Err(error) => {
                        bail!(TerminalError {
                            directory: working_directory,
                            program: shell_params.as_ref().map(|params| params.program.clone()),
                            args: shell_params.as_ref().and_then(|params| params.args.clone()),
                            title_override: terminal_title_override,
                            source: std::io::Error::other(format!("{error:#}")),
                        });
                    }
                };
                child_events = attached_child_events;
                pending_replay = (!replay.is_empty()).then_some(replay);

                #[cfg(windows)]
                if is_wsl_startup {
                    let pty_ready_at = Instant::now();
                    wsl_startup_timing = Some(WslStartupTiming {
                        started_at: wsl_startup_started_at,
                        pty_ready_at,
                    });
                    log_wsl_startup_phase(
                        "pty_ready",
                        wsl_startup_started_at,
                        pty_ready_at,
                        pty_ready_at,
                    );
                }

                let pty_info = PtyProcessInfo::new(ProcessIdGetter::from(&pty));

                //And connect them together
                let (pty_tx, io) =
                    spawn_event_loop(term.clone(), listener, pty, pty_options.drain_on_exit)?;
                let pty_control_is_local = provided_control.is_none();
                let pty_control = provided_control
                    .unwrap_or_else(|| Arc::new(pty_tx.clone()) as Arc<dyn PtyControl>);

                (
                    TerminalType::Pty {
                        pty_tx: Some(pty_tx),
                        io: Some(io),
                        info: Arc::new(pty_info),
                    },
                    None,
                    Some(pty_control),
                    pty_control_is_local,
                )
            };

            let no_task = task.is_none();
            let terminal = Terminal {
                task,
                terminal_type,
                subprocess,
                byte_stream: None,
                pty_control,
                pty_control_is_local,
                console_palette_enabled,
                last_console_palette: console_palette_enabled.then_some(console_palette),
                events_tx: events_tx.clone(),
                completion_tx,
                term,
                wakeup_gate,
                term_config: config,
                output_processor,
                title_override: terminal_title_override,
                events: VecDeque::with_capacity(10), //Should never get this high.
                last_content: Default::default(),
                last_mouse: None,
                mouse_down_position: None,
                matches: Arc::new(Vec::new()),
                content_dirty: true,
                content_revision: 0,
                reflow_on_next_resize: true,

                selection_head: None,
                breadcrumb_text: String::new(),
                scroll_px: px(0.),
                next_link_id: 0,
                selection_phase: SelectionPhase::Ended,
                hyperlink_regex_searches: RegexSearches::new(
                    &path_hyperlink_regexes,
                    path_hyperlink_timeout_ms,
                ),
                vi_mode_enabled: false,
                is_remote_terminal,
                last_mouse_move_time: Instant::now(),
                last_hyperlink_search_position: None,
                mouse_down_hyperlink: None,
                editor_click_started: false,
                #[cfg(windows)]
                shell_program,
                #[cfg(windows)]
                wsl_startup_timing,
                activation_script: activation_script.clone(),
                template: CopyTemplate {
                    pty_provider: pty_provider.clone(),
                    shell,
                    env,
                    cursor_shape,
                    alternate_scroll,
                    max_scroll_history_lines,
                    path_hyperlink_regexes,
                    path_hyperlink_timeout_ms,
                    window_id,
                },
                // The pty was opened here (or by the provider, which
                // `owns_child` reads from the template).
                child_is_the_multiplexers: false,
                pending_replay,
                child_exited: None,
                child_process_ended: false,
                terminal_exit_reported: false,
                task_exit_code: None,
                keyboard_input_sent: false,
                init_command_startup_marker: None,
                init_command_startup_tx: None,
                event_loop_task: Task::ready(Ok(())),
                background_executor,
                path_style,
                cwd_history: initial_cwd_history(is_remote_terminal, working_directory.as_ref()),
                pending_cwd_boundary: None,
                reported_theme: None,
                reported_working_directory: None,
                restored_working_directory: None,
                reported_foreground_command: None,
                reported_shell_command: None,
                #[cfg(any(test, feature = "test-support"))]
                input_log: Vec::new(),
                #[cfg(any(test, feature = "test-support"))]
                pty_write_log: Default::default(),
            };

            if !activation_script.is_empty() && no_task {
                for activation_script in activation_script {
                    terminal.write_to_pty(activation_script.into_bytes());
                    // Simulate enter key press
                    // NOTE(PowerShell): using `\r\n` will put PowerShell in a continuation mode (infamous >> character)
                    // and generally mess up the rendering.
                    terminal.write_to_pty(b"\x0d");
                }
                // In order to clear the screen at this point, we have two options:
                // 1. We can send a shell-specific command such as "clear" or "cls"
                // 2. We can "echo" a marker message that we will then catch when handling a Wakeup event
                //    and clear the screen using `terminal.clear()` method
                // We cannot issue a `terminal.clear()` command at this point as alacritty is evented
                // and while we have sent the activation script to the pty, it will be executed asynchronously.
                // Therefore, we somehow need to wait for the activation script to finish executing before we
                // can proceed with clearing the screen.
                terminal.write_to_pty(shell_kind.clear_screen_command().as_bytes());
                // Simulate enter key press
                terminal.write_to_pty(b"\x0d");
            }

            Ok(TerminalBuilder {
                terminal,
                events_tx: builder_events_tx,
                events_rx,
                child_events,
                multiplexer_error,
            })
        };
        cx.background_spawn(fut)
    }

    pub fn subscribe(mut self, cx: &Context<Terminal>) -> Terminal {
        // Keep the escalation alive during application shutdown. A detached
        // task from `Drop` may not run once GPUI begins terminating executors.
        let app_quit_subscription = cx.on_app_quit(|terminal, cx| {
            let kill_processes = match &terminal.terminal_type {
                // A session the multiplexer owns must survive Zetta quitting —
                // outliving the window is the entire point of backgrounding
                // it, and killing it here would empty every held session the
                // moment the application closed.
                // A child that has already ended has nothing left to signal, and
                // its process group ids may since have been reused.
                TerminalType::Pty { info, .. }
                    if terminal.owns_child() && !terminal.child_process_ended =>
                {
                    Some(terminate_processes_with_grace_period(
                        info.clone(),
                        cx.background_executor().clone(),
                    ))
                }
                TerminalType::Pty { .. } | TerminalType::DisplayOnly => None,
            };
            async move {
                if let Some(kill_processes) = kill_processes {
                    kill_processes.await;
                }
            }
        });
        cx.on_release(move |_, _| drop(app_quit_subscription))
            .detach();

        //Event loop
        self.terminal.event_loop_task = cx.spawn(async move |terminal, cx| {
            while let Some(event) = self.events_rx.next().await {
                terminal.update(cx, |terminal, cx| {
                    terminal.process_pty_event(event, cx);
                })?;

                'drain: loop {
                    let mut events = Vec::new();
                    let mut wakeup = false;
                    #[cfg(any(test, feature = "test-support"))]
                    let mut timer = cx.background_executor().simulate_random_delay().fuse();
                    #[cfg(not(any(test, feature = "test-support")))]
                    let mut timer = cx
                        .background_executor()
                        .timer(TERMINAL_EVENT_DRAIN_INTERVAL)
                        .fuse();

                    loop {
                        futures::select_biased! {
                            _ = timer => break,
                            event = self.events_rx.next() => {
                                if let Some(event) = event {
                                    if matches!(event, PtyEvent::Event(TerminalBackendEvent::Wakeup))
                                    {
                                        wakeup = true;
                                    } else {
                                        events.push(event);
                                    }

                                    if events.len() >= MAX_TERMINAL_EVENTS_PER_BATCH {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            },
                        }
                    }

                    if events.is_empty() && !wakeup {
                        yield_now().await;
                        break 'drain;
                    }

                    terminal.update(cx, |this, cx| {
                        if wakeup {
                            this.process_event(TerminalBackendEvent::Wakeup, cx);
                        }

                        for event in events {
                            this.process_pty_event(event, cx);
                        }
                    })?;
                    yield_now().await;
                }
            }
            anyhow::Ok(())
        });
        self.terminal
    }

    #[cfg(windows)]
    fn resolve_path(path: &str) -> Result<String> {
        use windows::Win32::Storage::FileSystem::SearchPathW;
        use windows::core::HSTRING;

        let path = if path.starts_with(r"\\?\") || !path.contains(&['/', '\\']) {
            path.to_string()
        } else {
            r"\\?\".to_string() + path
        };

        let required_length = unsafe { SearchPathW(None, &HSTRING::from(&path), None, None, None) };
        let mut buf = vec![0u16; required_length as usize];
        let size = unsafe { SearchPathW(None, &HSTRING::from(&path), None, Some(&mut buf), None) };

        Ok(String::from_utf16(&buf[..size as usize])?)
    }
}

enum TerminalType {
    Pty {
        /// `None` once the pty has been released — see
        /// [`Terminal::release_pty_resources`]. The variant stays `Pty` because
        /// the pane is still a pty pane: it keeps its grid, its exit status and
        /// the process metadata in `info`.
        pty_tx: Option<PtySender>,
        /// The thread reading this pty, kept so converting the terminal to
        /// another backend can stop it synchronously. Taken by
        /// [`Terminal::stop_pty_loop`].
        ///
        /// It also owns the pty for as long as it is held: the thread returns
        /// its `EventLoop` rather than dropping it, and a `JoinHandle` keeps
        /// the value its thread returned alive. Holding this past the child's
        /// exit therefore holds the pty master descriptor, the poller and the
        /// loop's buffers open.
        io: Option<PtyIo>,
        info: Arc<PtyProcessInfo>,
    },
    DisplayOnly,
}

pub struct Terminal {
    terminal_type: TerminalType,
    /// Set for non-PTY terminals (see [`HeadlessTerminal`]); owns the spawned
    /// subprocess and the task pumping its output into the grid.
    subprocess: Option<SubprocessHandle>,
    /// Set for terminals connected to a blocking bidirectional byte stream.
    byte_stream: Option<ByteStreamHandle>,
    /// True when `pty_control` is this terminal's own pty sender rather than a
    /// control the multiplexer provided. Only the former is released along with
    /// the rest of the pty's resources.
    pty_control_is_local: bool,
    /// Operations routed to the owner of this specific PTY.
    pty_control: Option<Arc<dyn PtyControl>>,
    /// False for WSL and non-PTY streams, whose color state is intentionally unchanged.
    console_palette_enabled: bool,
    /// Suppresses repeated theme notifications carrying the same palette.
    last_console_palette: Option<ConsolePalette>,
    /// Where this terminal's events go. Carried past construction so a
    /// terminal that is converted to a byte stream after it started (the
    /// multiplexer handover) can keep feeding the same channel.
    events_tx: futures::channel::mpsc::UnboundedSender<PtyEvent>,
    completion_tx: Option<Sender<Option<ExitStatus>>>,
    term: Arc<AlacrittyTermLock>,
    wakeup_gate: WakeupGate,
    term_config: AlacrittyTermConfig,
    output_processor: Processor<StdSyncHandler>,
    events: VecDeque<InternalEvent>,
    /// This is only used for mouse mode cell change detection
    last_mouse: Option<(Point, SelectionSide)>,
    /// Window-relative position of the most recent left mouse-down. Used to
    /// apply a drag threshold before starting a selection (see #58970).
    mouse_down_position: Option<GpuiPoint<Pixels>>,
    pub matches: Arc<Vec<Range>>,
    pub last_content: Content,
    content_dirty: bool,
    content_revision: u64,
    reflow_on_next_resize: bool,
    pub selection_head: Option<Point>,

    pub breadcrumb_text: String,
    title_override: Option<String>,
    scroll_px: Pixels,
    next_link_id: usize,
    selection_phase: SelectionPhase,
    hyperlink_regex_searches: RegexSearches,
    task: Option<TaskState>,
    vi_mode_enabled: bool,
    is_remote_terminal: bool,
    last_mouse_move_time: Instant,
    last_hyperlink_search_position: Option<GpuiPoint<Pixels>>,
    mouse_down_hyperlink: Option<HyperlinkMatch>,
    editor_click_started: bool,
    #[cfg(windows)]
    shell_program: Option<String>,
    #[cfg(windows)]
    wsl_startup_timing: Option<WslStartupTiming>,
    template: CopyTemplate,
    /// Whether the child this terminal's pty runs belongs to the multiplexer.
    ///
    /// Set when a pane is attached, or handed back, from the multiplexer: the
    /// daemon forked that child and is still its parent, so this window must
    /// leave it running when the pane goes away. Nothing else can be inferred
    /// from the terminal itself — an attached pty looks exactly like one this
    /// process opened.
    child_is_the_multiplexers: bool,
    activation_script: Vec<String>,
    /// A restored screen waiting for the grid to be the right size.
    ///
    /// Replaying it before the pane is laid out wraps it at the placeholder
    /// width and then reflows it again, which is how a restored session ends
    /// up looking corrupted rather than resumed.
    pending_replay: Option<Vec<u8>>,
    child_exited: Option<ExitStatus>,
    /// Set once the pty's child is known to have ended, as opposed to the pty
    /// merely having become unusable. Its process group ids are free to be
    /// reused from that moment, so teardown must not signal them.
    child_process_ended: bool,
    terminal_exit_reported: bool,
    task_exit_code: Option<i32>,
    keyboard_input_sent: bool,
    init_command_startup_marker: Option<String>,
    init_command_startup_tx: Option<Sender<()>>,
    event_loop_task: Task<Result<(), anyhow::Error>>,
    background_executor: BackgroundExecutor,
    path_style: PathStyle,
    /// Where the shell was, at each point in the scrollback where it moved.
    /// A relative path printed by an old command names a file relative to the
    /// directory the shell was in *then*, not the one it is in now.
    cwd_history: Vec<CwdHistoryEntry>,
    /// The scrollback position of the last command submitted, held until the
    /// cwd change that command caused is observed. A `cd` is noticed a refresh
    /// interval after the fact, by which point the shell has printed its next
    /// prompt; recording the boundary instead attributes the new directory to
    /// the command that changed it.
    pending_cwd_boundary: Option<i32>,
    reported_theme: Option<Arc<Theme>>,
    reported_working_directory: Option<String>,
    restored_working_directory: Option<PathBuf>,
    reported_foreground_command: Option<String>,
    /// The first command reported by the WSL/MSYS2 shell integration is its
    /// idle shell marker. Later markers can then be classified without a
    /// platform-specific shell-name list.
    reported_shell_command: Option<String>,
    #[cfg(any(test, feature = "test-support"))]
    input_log: Vec<Vec<u8>>,
    #[cfg(any(test, feature = "test-support"))]
    pty_write_log: std::cell::RefCell<Vec<Vec<u8>>>,
}

/// Where the shell was when a given stretch of scrollback was printed.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CwdHistoryEntry {
    /// Line offset in the retained scrollback buffer.
    scrollback_position: i32,
    working_directory: PathBuf,
}

/// Seeds the history so lines printed before the first observed `cd` still
/// resolve. A remote terminal records nothing: its shell's directory is on the
/// other host and this window cannot observe it.
fn initial_cwd_history(
    is_remote_terminal: bool,
    working_directory: Option<&PathBuf>,
) -> Vec<CwdHistoryEntry> {
    if is_remote_terminal {
        return Vec::new();
    }
    working_directory
        .map(|working_directory| {
            vec![CwdHistoryEntry {
                scrollback_position: i32::MIN,
                working_directory: working_directory.clone(),
            }]
        })
        .unwrap_or_default()
}

struct CopyTemplate {
    /// How the original's PTY was opened. Carried so that duplicating a pane
    /// produces a terminal owned by the same thing the original was, rather
    /// than silently spawning a local process the multiplexer knows nothing
    /// about.
    pty_provider: Option<Arc<dyn PtyProvider>>,
    shell: Shell,
    env: HashMap<String, String>,
    cursor_shape: SettingsCursorShape,
    alternate_scroll: AlternateScroll,
    max_scroll_history_lines: Option<usize>,
    path_hyperlink_regexes: Vec<String>,
    path_hyperlink_timeout_ms: u64,
    window_id: u64,
}

#[derive(Debug)]
pub struct TaskState {
    pub status: TaskStatus,
    pub completion_rx: Receiver<Option<ExitStatus>>,
    pub spawned_task: SpawnInTerminal,
}

/// A status of the current terminal tab's task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// The task had been started, but got cancelled or somehow otherwise it did not
    /// report its exit code before the terminal event loop was shut down.
    Unknown,
    /// The task is started and running currently.
    Running,
    /// After the start, the task stopped running and reported its error code back.
    Completed { success: bool },
}

impl TaskStatus {
    fn register_terminal_exit(&mut self) {
        if self == &Self::Running {
            *self = Self::Unknown;
        }
    }

    fn register_task_exit(&mut self, error_code: i32) {
        *self = TaskStatus::Completed {
            success: error_code == 0,
        };
    }
}

/// How long to wait for a retired byte stream to finish delivering what it had
/// already read. Bounded so a multiplexer that failed to close its end of the
/// relay cannot hang the window; exceeded, the tail of that output is lost, which
/// is only worse than a stall if the stall would have ended.
const BYTE_STREAM_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

const FIND_HYPERLINK_THROTTLE_PX: Pixels = px(5.0);

/// Minimum pointer movement before a left click begins a selection. This keeps
/// a click that jitters by a pixel or two (such as the window-focusing click)
/// from starting a selection and, with `copy_on_select` enabled, clobbering the
/// clipboard. Mirrors the drag threshold used by gpui's `div` element.
const SELECTION_DRAG_THRESHOLD: f64 = 2.0;

impl Terminal {
    /// Enable UI wakeups while this terminal is visible.
    ///
    /// PTY parsing and scrollback collection continue while disabled; only the high-frequency
    /// foreground notifications are suppressed. Re-enabling emits one consolidated wakeup so
    /// the next render catches up with all background output.
    pub fn set_ui_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let was_visible = self.wakeup_gate.set_enabled(visible);
        if visible && !was_visible {
            self.process_event(TerminalBackendEvent::Wakeup, cx);
        }
    }

    pub fn set_reported_theme(&mut self, theme: Option<Arc<Theme>>) {
        self.reported_theme = theme;
    }

    /// Synchronizes the legacy Win32 console palette with the rendered theme.
    /// Identical updates are discarded before they reach either the local PTY
    /// thread or the multiplexer worker.
    pub fn set_console_palette(&mut self, palette: ConsolePalette) {
        if !self.console_palette_enabled || self.last_console_palette == Some(palette) {
            return;
        }
        self.last_console_palette = Some(palette);
        if let Some(control) = &self.pty_control {
            control.set_console_palette(palette);
        }
    }

    pub fn reported_working_directory(&self) -> Option<&str> {
        self.reported_working_directory.as_deref()
    }

    fn process_pty_event(&mut self, event: PtyEvent, cx: &mut Context<Self>) {
        match event {
            PtyEvent::Event(event) => self.process_event(event, cx),
        }
    }

    fn process_event(&mut self, event: TerminalBackendEvent, cx: &mut Context<Self>) {
        match event {
            TerminalBackendEvent::Title(title) => {
                if let Some(directory) = reported_working_directory_from_title(&title) {
                    let changed =
                        self.reported_working_directory.as_deref() != Some(directory.as_str());
                    self.reported_working_directory = Some(directory);
                    if changed {
                        cx.emit(Event::TitleChanged);
                    }
                    return;
                }

                if let Some(command) = reported_foreground_command_from_title(&title) {
                    // Only the Windows startup-timing log below reads this.
                    #[cfg(windows)]
                    let first_shell_marker = self.reported_shell_command.is_none();
                    self.reported_shell_command
                        .get_or_insert_with(|| command.clone());
                    #[cfg(windows)]
                    if first_shell_marker {
                        if let Some(timing) = self.wsl_startup_timing.take() {
                            let marker_at = Instant::now();
                            log_wsl_startup_phase(
                                "first_shell_marker",
                                timing.started_at,
                                timing.pty_ready_at,
                                marker_at,
                            );
                        }
                    }
                    if self.reported_foreground_command.as_deref() != Some(command.as_str()) {
                        self.reported_foreground_command = Some(command);
                        cx.emit(Event::TitleChanged);
                    }
                    return;
                }

                // ignore default shell program title change as windows always sends those events
                // and it would end up showing the shell executable path in breadcrumbs
                #[cfg(windows)]
                if self
                    .shell_program
                    .as_ref()
                    .map(|e| *e == title)
                    .unwrap_or(false)
                {
                    return;
                }

                self.breadcrumb_text = title;
                cx.emit(Event::BreadcrumbsChanged);
            }
            TerminalBackendEvent::ResetTitle => {
                self.breadcrumb_text = String::new();
                cx.emit(Event::BreadcrumbsChanged);
            }
            TerminalBackendEvent::ClipboardStore(data) => {
                cx.write_to_clipboard(ClipboardItem::new_string(data))
            }
            TerminalBackendEvent::ClipboardLoad(format) => {
                self.write_to_pty(
                    match &cx.read_from_clipboard().and_then(|item| item.text()) {
                        // The terminal only supports pasting strings, not images.
                        Some(text) => format(text),
                        _ => format(""),
                    }
                    .into_bytes(),
                )
            }
            TerminalBackendEvent::PtyWrite(out) => self.write_to_pty(out.into_bytes()),
            TerminalBackendEvent::TextAreaSizeRequest(format) => {
                self.write_to_pty(format(self.last_content.terminal_bounds).into_bytes())
            }
            TerminalBackendEvent::ResizeRequest { rows, columns } => {
                cx.emit(Event::ResizeRequested { rows, columns });
            }
            TerminalBackendEvent::CursorBlinkingChange => {
                let terminal = self.term.lock();
                let blinking = terminal.cursor_style().blinking;
                cx.emit(Event::BlinkChanged(blinking));
            }
            TerminalBackendEvent::Bell => {
                cx.emit(Event::Bell);
            }
            TerminalBackendEvent::Exit => {
                self.register_terminal_exit(None, TerminalExitSource::StatusUnavailable, cx)
            }
            TerminalBackendEvent::MouseCursorDirty => {
                //NOOP, Handled in render
            }
            TerminalBackendEvent::Wakeup => {
                self.content_dirty = true;
                self.detect_init_command_startup_marker();
                cx.emit(Event::Wakeup);

                if let TerminalType::Pty { info, .. } = &self.terminal_type {
                    info.emit_title_changed_if_changed(cx);
                }
            }
            TerminalBackendEvent::ColorRequest(index, format) => {
                // It's important that the color request is processed here to retain relative order
                // with other PTY writes. Otherwise applications might witness out-of-order
                // responses to requests. For example: An application sending `OSC 11 ; ? ST`
                // (color request) followed by `CSI c` (request device attributes) would receive
                // the response to `CSI c` first.
                // Instead of locking, we could store the colors in `self.last_content`. But then
                // we might respond with out of date value if a "set color" sequence is immediately
                // followed by a color request sequence.

                let theme = self
                    .reported_theme
                    .clone()
                    .unwrap_or_else(|| cx.theme().clone());
                let color = self.term.lock().colors()[index]
                    .unwrap_or_else(|| to_vte_rgb(get_color_at_index(index, theme.as_ref())));
                self.write_to_pty(format(color).into_bytes());
            }
            TerminalBackendEvent::ChildExit(exit_status) => {
                self.register_terminal_exit(Some(exit_status), TerminalExitSource::Child, cx);
            }
            TerminalBackendEvent::ChildExitStatusUnavailable => {
                self.register_terminal_exit(None, TerminalExitSource::StatusUnavailable, cx);
            }
            TerminalBackendEvent::ChildWatcherDisconnected => {
                // The child's fate is genuinely unknown here: the multiplexer
                // owns the process, so losing contact with it says nothing about
                // whether the process ended. What *is* known is that this
                // terminal can no longer be driven — the event loop stops on a
                // child event — so the exit is registered rather than
                // suppressed, but under its own source, so the pane can say it
                // lost contact instead of claiming the shell died.
                self.register_terminal_exit(None, TerminalExitSource::WatcherDisconnected, cx);
            }
            TerminalBackendEvent::BackendShutdown => {
                self.register_terminal_exit(None, TerminalExitSource::BackendShutdown, cx);
            }
        }
    }

    pub fn selection_started(&self) -> bool {
        self.selection_phase == SelectionPhase::Selecting
    }

    fn process_terminal_event(
        &mut self,
        event: &InternalEvent,
        term: &mut AlacrittyTerm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.content_dirty = true;
        match event {
            &InternalEvent::Resize {
                bounds: new_bounds,
                reflow,
            } => {
                let new_bounds = normalize_terminal_bounds(new_bounds);
                trace!("Resizing: new_bounds={new_bounds:?}");

                // Compare against the grid rather than `last_content`, which
                // `set_size` already advanced when it queued this event.
                let grid_size_changed = term.screen_lines() != new_bounds.num_lines()
                    || term.columns() != new_bounds.num_columns();
                let columns_changed = term.columns() != new_bounds.num_columns();
                self.last_content.terminal_bounds = new_bounds;

                #[cfg(windows)]
                if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
                    if let Some(control) = &self.pty_control {
                        control.resize(
                            new_bounds.num_columns() as u16,
                            new_bounds.num_lines() as u16,
                        );
                    } else if let Some(pty_tx) = pty_tx {
                        pty_tx.resize(new_bounds);
                    }
                }
                #[cfg(not(windows))]
                if let TerminalType::Pty {
                    pty_tx: Some(pty_tx),
                    ..
                } = &self.terminal_type
                {
                    pty_tx.resize(new_bounds);
                }

                let reflow =
                    reflow && synchronous_reflow_is_bounded(term.history_size(), term.columns());
                resize(term, new_bounds, reflow);
                // The grid is now the size the pane actually has, which is the
                // first moment a restored screen can be written without being
                // wrapped at the wrong width.
                if let Some(replay) = self.pending_replay.take() {
                    self.output_processor.advance(term, &replay);
                }
                if grid_size_changed {
                    cx.emit(Event::GridSizeChanged);
                }
                // A reflow rewraps the scrollback, which moves every line the
                // recorded directories are keyed by.
                if columns_changed {
                    self.reset_cwd_history();
                }
                // If there are matches we need to emit a wake up event to
                // invalidate the matches and recalculate their locations
                // in the new terminal layout
                if !self.matches.is_empty() {
                    cx.emit(Event::Wakeup);
                }
            }
            InternalEvent::Clear => {
                trace!("Clearing");
                clear_saved_screen(term);
                self.reset_cwd_history();
                cx.emit(Event::Wakeup);
            }
            InternalEvent::Scroll(scroll) => {
                trace!("Scrolling: scroll={scroll:?}");
                scroll_display(term, *scroll);
                self.refresh_hovered_word(window);

                if self.vi_mode_enabled {
                    update_vi_cursor_for_scroll(term, *scroll);
                    if let Some(selection_head) = update_selection_to_vi_cursor(term) {
                        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                        if let Some(selection_text) = selection_text(term) {
                            cx.write_to_primary(ClipboardItem::new_string(selection_text));
                        }

                        self.selection_head = Some(selection_head);
                        cx.emit(Event::SelectionsChanged)
                    }
                }
            }
            InternalEvent::SetSelection(selection) => {
                trace!("Setting selection: selection={selection:?}");
                set_term_selection(term, selection.as_ref());

                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                if let Some(selection_text) = selection_text(term) {
                    cx.write_to_primary(ClipboardItem::new_string(selection_text));
                }

                if let Some(selection) = selection {
                    self.selection_head = Some(selection.head);
                }
                cx.emit(Event::SelectionsChanged)
            }
            InternalEvent::UpdateSelection(position) => {
                trace!("Updating selection: position={position:?}");
                let (point, side) = grid_point_and_side(
                    *position,
                    self.last_content.terminal_bounds,
                    display_offset(term),
                );

                if update_term_selection(term, point, side) {
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    if let Some(selection_text) = selection_text(term) {
                        cx.write_to_primary(ClipboardItem::new_string(selection_text));
                    }

                    self.selection_head = Some(point);
                    cx.emit(Event::SelectionsChanged)
                }
            }

            InternalEvent::Copy(keep_selection) => {
                trace!("Copying selection: keep_selection={keep_selection:?}");
                if let Some(txt) = selection_text(term) {
                    cx.write_to_clipboard(ClipboardItem::new_string(txt));
                    if !keep_selection.unwrap_or_else(|| {
                        let settings = TerminalSettings::get_global(cx);
                        settings.keep_selection_on_copy
                    }) {
                        self.events.push_back(InternalEvent::SetSelection(None));
                    }
                }
            }
            InternalEvent::ScrollToPoint(point) => {
                trace!("Scrolling to point: point={point:?}");
                scroll_to_point(term, *point);
                self.refresh_hovered_word(window);
            }
            InternalEvent::MoveViCursorToPoint(point) => {
                trace!("Move vi cursor to point: point={point:?}");
                vi_goto_point(term, *point);
                self.refresh_hovered_word(window);
            }
            InternalEvent::ToggleViMode => {
                trace!("Toggling vi mode");
                self.vi_mode_enabled = !self.vi_mode_enabled;
                toggle_term_vi_mode(term);
            }
            InternalEvent::ViMotion(motion) => {
                trace!("Performing vi motion: motion={motion:?}");
                vi_motion(term, *motion);
            }
            InternalEvent::FindHyperlink(position, open) => {
                trace!("Finding hyperlink at position: position={position:?}, open={open:?}");

                let point = grid_point(
                    *position,
                    self.last_content.terminal_bounds,
                    display_offset(term),
                );
                let path_style = self.hyperlink_path_style();

                match find_from_terminal_point(
                    term,
                    point,
                    &mut self.hyperlink_regex_searches,
                    path_style,
                ) {
                    Some(hyperlink) => {
                        let history_size = term.history_size();
                        self.process_hyperlink(hyperlink, *open, history_size, cx);
                    }
                    None => {
                        self.last_content.last_hovered_word = None;
                        cx.emit(Event::NewNavigationTarget(None));
                    }
                }
            }
            InternalEvent::ProcessHyperlink(hyperlink, open) => {
                // Read here rather than in `process_hyperlink`, which cannot lock
                // the term: `sync` already holds the lock while dispatching.
                let history_size = term.history_size();
                self.process_hyperlink(hyperlink.clone(), *open, history_size, cx);
            }
        }
    }

    fn process_hyperlink(
        &mut self,
        hyperlink: HyperlinkMatch,
        open: bool,
        history_size: usize,
        cx: &mut Context<Self>,
    ) {
        let HyperlinkMatch {
            text: maybe_url_or_path,
            is_url,
            range,
        } = hyperlink;
        let prev_hovered_word = self.last_content.last_hovered_word.take();
        let terminal_dir = self.cwd_at_line(range.start().line, history_size);
        let target_path = if !is_url {
            #[cfg(windows)]
            {
                cygwin_path_like_to_windows(&self.template.shell, &maybe_url_or_path)
                    .unwrap_or_else(|| maybe_url_or_path.clone())
            }
            #[cfg(not(windows))]
            {
                maybe_url_or_path.clone()
            }
        } else {
            maybe_url_or_path.clone()
        };

        let target = if is_url {
            if let Some(path) = target_path.strip_prefix("file://") {
                let decoded_path = urlencoding::decode(path)
                    .map(|decoded| decoded.into_owned())
                    .unwrap_or(path.to_owned());

                MaybeNavigationTarget::PathLike(PathLikeTarget {
                    maybe_path: decoded_path,
                    terminal_dir,
                    path_style: self.path_style,
                })
            } else {
                MaybeNavigationTarget::Url(target_path)
            }
        } else {
            MaybeNavigationTarget::PathLike(PathLikeTarget {
                maybe_path: target_path,
                terminal_dir,
                path_style: self.path_style,
            })
        };

        if open {
            cx.emit(Event::Open(target));
        } else {
            self.update_selected_word(prev_hovered_word, range, maybe_url_or_path, target, cx);
        }
    }

    fn find_hyperlink_at_point_with_path_style(
        &mut self,
        point: Point,
        path_style: PathStyle,
    ) -> Option<HyperlinkMatch> {
        let term_lock = self.term.lock();
        find_from_terminal_point(
            &term_lock,
            point,
            &mut self.hyperlink_regex_searches,
            path_style,
        )
    }

    fn find_hyperlink_at_point(&mut self, point: Point) -> Option<HyperlinkMatch> {
        self.find_hyperlink_at_point_with_path_style(point, self.hyperlink_path_style())
    }

    fn update_selected_word(
        &mut self,
        prev_word: Option<HoveredWord>,
        word_match: Range,
        word: String,
        navigation_target: MaybeNavigationTarget,
        cx: &mut Context<Self>,
    ) {
        if let Some(prev_word) = prev_word
            && prev_word.word == word
            && prev_word.word_match == word_match
        {
            self.last_content.last_hovered_word = Some(HoveredWord {
                word,
                word_match,
                id: prev_word.id,
            });
            return;
        }

        self.last_content.last_hovered_word = Some(HoveredWord {
            word,
            word_match,
            id: self.next_link_id(),
        });
        cx.emit(Event::NewNavigationTarget(Some(navigation_target)));
        cx.notify()
    }

    fn next_link_id(&mut self) -> usize {
        let res = self.next_link_id;
        self.next_link_id = self.next_link_id.wrapping_add(1);
        res
    }

    pub fn last_content(&self) -> &Content {
        &self.last_content
    }

    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    pub fn set_cursor_shape(&mut self, cursor_shape: SettingsCursorShape) {
        set_default_cursor_style(&mut self.term_config, cursor_shape);
        apply_config(&self.term, &self.term_config);
        let content = {
            let terminal = self.term.lock_unfair();
            make_content(&terminal, &mut self.last_content)
        };
        self.last_content = content;
        self.content_revision = self.content_revision.wrapping_add(1);
    }

    pub fn write_output(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        // Inject bytes directly into the terminal emulator and refresh the UI.
        // This bypasses the PTY/event loop for display-only terminals.
        let mut previous_byte_was_cr = false;
        let converted = convert_lf_to_crlf(bytes, &mut previous_byte_was_cr);

        let mut term = self.term.lock();
        self.output_processor.advance(&mut *term, &converted);
        drop(term);
        self.content_dirty = true;
        self.detect_init_command_startup_marker();
        cx.emit(Event::Wakeup);
    }

    /// Whether this terminal's processes are its own to end.
    ///
    /// A terminal whose PTY came from the multiplexer is a *view* of a session
    /// the multiplexer owns. Dropping it is exactly what detaching does, so
    /// ending its processes there would kill the session the user asked to
    /// keep — and leave the multiplexer holding a terminal whose shell is
    /// already dead, which is what a reattached session that cannot be typed
    /// into actually is.
    /// A pane *attached* from the multiplexer is the same view arrived at from
    /// the other direction, and has no provider to say so: its descriptor was
    /// handed over rather than opened here. Detaching such a pane again — or
    /// simply quitting the window — killed the session it was handing back.
    fn owns_child(&self) -> bool {
        self.template.pty_provider.is_none() && !self.child_is_the_multiplexers
    }

    /// Serializes the scrollback and screen as the escape sequences that would
    /// reproduce them, most recent `max_lines` lines.
    ///
    /// Taken when a session is handed to the multiplexer, which has not been
    /// reading this pane while this window was showing it: the screen is here,
    /// and this is what carries it across so the multiplexer's own grid starts
    /// from what the user was looking at.
    pub fn ansi_snapshot(&self, max_lines: usize) -> Vec<u8> {
        snapshot::ansi_snapshot(&self.term.lock_unfair(), max_lines)
    }

    pub fn total_lines(&self) -> usize {
        total_lines(&self.term.lock_unfair())
    }

    pub fn viewport_lines(&self) -> usize {
        screen_lines(&self.term.lock_unfair())
    }

    //To test:
    //- Activate match on terminal (scrolling and selection)
    //- Editor search snapping behavior

    pub fn activate_match(&mut self, index: usize) {
        if let Some(search_match) = self.matches.get(index).cloned() {
            self.set_selection(Some(Selection::simple_range(search_match)));
            if self.vi_mode_enabled {
                self.events
                    .push_back(InternalEvent::MoveViCursorToPoint(search_match.end()));
            } else {
                self.events
                    .push_back(InternalEvent::ScrollToPoint(search_match.start()));
            }
        }
    }

    pub fn select_matches(&mut self, matches: &[Range]) {
        let matches_to_select = self
            .matches
            .iter()
            .filter(|self_match| matches.contains(self_match))
            .cloned()
            .collect::<Vec<_>>();
        for match_to_select in matches_to_select {
            self.set_selection(Some(Selection::simple_range(match_to_select)));
        }
    }

    pub fn select_all(&mut self) {
        let term = self.term.lock();
        let range = full_content_range(&term);
        drop(term);
        self.set_selection(Some(Selection::simple_range(range)));
    }

    fn set_selection(&mut self, selection: Option<Selection>) {
        self.events
            .push_back(InternalEvent::SetSelection(selection));
    }

    pub fn copy(&mut self, keep_selection: Option<bool>) {
        self.events.push_back(InternalEvent::Copy(keep_selection));
    }

    pub fn clear(&mut self) {
        self.events.push_back(InternalEvent::Clear)
    }

    pub fn shrink_to_used(&mut self) {
        shrink_to_used(&mut self.term.lock());
    }

    pub fn scroll_line_up(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::Delta(1)));
    }

    pub fn scroll_up_by(&mut self, lines: usize) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::Delta(lines as i32)));
    }

    pub fn scroll_line_down(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::Delta(-1)));
    }

    pub fn scroll_down_by(&mut self, lines: usize) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::Delta(-(lines as i32))));
    }

    pub fn scroll_page_up(&mut self) {
        self.events.push_back(InternalEvent::Scroll(Scroll::PageUp));
    }

    pub fn scroll_page_down(&mut self) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::PageDown));
    }

    pub fn scroll_to_top(&mut self) {
        self.events.push_back(InternalEvent::Scroll(Scroll::Top));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.events.push_back(InternalEvent::Scroll(Scroll::Bottom));
    }

    pub fn scrolled_to_top(&self) -> bool {
        self.last_content.scrolled_to_top
    }

    pub fn scrolled_to_bottom(&self) -> bool {
        self.last_content.scrolled_to_bottom
    }

    ///Resize the terminal and the PTY.
    pub fn set_size(&mut self, new_bounds: TerminalBounds) {
        let new_bounds = normalize_terminal_bounds(new_bounds);
        let old_bounds = self.last_content.terminal_bounds;
        self.last_content.terminal_bounds = new_bounds;

        // Avoid spamming PTY resizes on pixel-level size changes (e.g. while dragging edges),
        // since those can generate excessive SIGWINCH/reflows and cause visible flicker.
        let requires_resize = old_bounds.num_lines() != new_bounds.num_lines()
            || old_bounds.num_columns() != new_bounds.num_columns()
            || old_bounds.cell_width != new_bounds.cell_width
            || old_bounds.line_height != new_bounds.line_height;

        if !requires_resize {
            // Identical bounds mean this is the layout pass the truncate request
            // was armed for, landing without resizing this grid at all. Callers
            // arm whole sets of terminals speculatively — every pane in a tab
            // being switched to, every survivor of a closed pane — so leaving
            // the request standing would hand it to the next genuine window
            // resize, which would truncate a grid that should have reflowed.
            //
            // A sub-cell pixel change is not that layout pass. It is the drag
            // guard above, which exists to be transparent, so it leaves the
            // request alone.
            if old_bounds == new_bounds {
                self.reflow_on_next_resize = true;
            }
            return;
        }

        let reflow = mem::replace(&mut self.reflow_on_next_resize, true);

        match self.events.back_mut() {
            Some(InternalEvent::Resize {
                bounds: pending_bounds,
                reflow: pending_reflow,
            }) => {
                *pending_bounds = new_bounds;
                *pending_reflow &= reflow;
            }
            _ => self.events.push_back(InternalEvent::Resize {
                bounds: new_bounds,
                reflow,
            }),
        }
    }

    /// Truncate instead of reflowing the primary grid during the next layout-driven resize.
    ///
    /// The request is dropped again if the layout settles on the size this
    /// terminal already had, so it cannot reach a later window resize.
    pub fn truncate_on_next_resize(&mut self) {
        self.reflow_on_next_resize = false;
    }

    /// Write input to the interactive backend, if applicable.
    /// (This is a no-op for display-only terminals.)
    fn write_to_pty(&self, input: impl Into<Cow<'static, [u8]>>) {
        let input = input.into();
        #[cfg(any(test, feature = "test-support"))]
        self.pty_write_log.borrow_mut().push(input.to_vec());
        // The byte stream comes first, and has to. A pane handed over to the
        // multiplexer in shared mode keeps its `TerminalType::Pty` — only the
        // loop behind it is shut down, by `stop_pty_loop` — so testing the
        // terminal type first sent every keystroke into a channel nothing was
        // reading any more. The window that was revoked into shared mode went
        // silently mute, which showed up as only the client that joined *last*
        // being able to type.
        if let Some(byte_stream) = &self.byte_stream {
            byte_stream.write(input.into_owned());
        } else if let TerminalType::Pty {
            pty_tx: Some(pty_tx),
            ..
        } = &self.terminal_type
        {
            if log::log_enabled!(log::Level::Debug) {
                if let Ok(str) = str::from_utf8(&input) {
                    log::debug!("Writing to PTY: {:?}", str);
                } else {
                    log::debug!("Writing to PTY: {:?}", input);
                }
            }
            pty_tx.notify(input);
        }
    }

    pub fn input(&mut self, input: impl Into<Cow<'static, [u8]>>) {
        self.keyboard_input_sent = true;
        self.complete_init_command_startup_handshake();
        self.write_input(input);
    }

    /// Runs Zetta's editor dispatcher through the active shell so it inherits
    /// this pane's current environment and remains attached to this pane's PTY.
    pub fn open_path_in_editor(&mut self, path: &Path) {
        self.open_path_in_editor_with_path_style(path, self.path_style);
    }

    /// The path syntax native to this pane's shell, for callers building an
    /// editor command from a path that did not come from the shell itself.
    pub fn native_path_style(&self) -> PathStyle {
        self.path_style
    }

    /// Builds the shell command used to open a path in Zetta's editor.
    pub fn editor_command_for_path(&self, path: &Path, path_style: PathStyle) -> Option<String> {
        let shell_kind = interaction_shell_kind(&self.template.shell, self.path_style);
        let zetta_command = zetta_command_for_shell(&self.template.shell)?;
        let path_argument =
            editor_path_argument(shell_kind, &self.template.shell, path, path_style)?;
        Some(editor_invocation_command(
            &zetta_command,
            &path_argument,
            false,
        ))
    }

    /// Opens a path emitted by a shell that may use a different path syntax
    /// than the native Zetta host, such as WSL on Windows.
    pub fn open_path_in_editor_with_path_style(&mut self, path: &Path, path_style: PathStyle) {
        let Some(command) = self.editor_command_for_path(path, path_style) else {
            log::error!("cannot determine the native Zetta executable for the pane editor");
            return;
        };
        self.submit_editor_command(command);
    }

    /// Builds the shell command used to open and remove a managed scrollback
    /// snapshot in Zetta's editor.
    pub fn editor_command_for_temporary_path(&self, path: &Path) -> Option<String> {
        let shell_kind = interaction_shell_kind(&self.template.shell, self.path_style);
        let zetta_command = zetta_command_for_shell(&self.template.shell)?;
        let path_argument =
            editor_path_argument(shell_kind, &self.template.shell, path, self.path_style)?;
        Some(editor_invocation_command(
            &zetta_command,
            &path_argument,
            true,
        ))
    }

    /// Opens a managed scrollback snapshot and asks the editor dispatcher to
    /// remove it as soon as the editor command returns.
    pub fn open_temporary_path_in_editor(&mut self, path: &Path) {
        let Some(command) = self.editor_command_for_temporary_path(path) else {
            log::error!("cannot determine the native Zetta executable for the pane editor");
            return;
        };
        self.submit_editor_command(command);
    }

    /// Returns whether sending an editor command to this terminal would send
    /// it to a foreground program instead of the shell.
    pub fn editor_should_open_in_new_pane(&self) -> bool {
        if !self.is_pty() || self.last_content.mode.contains(Modes::ALT_SCREEN) {
            return true;
        }

        !self.foreground_process_is_shell()
    }

    /// Returns whether the interactive shell currently owns the terminal's
    /// foreground. Unknown process state and display-only terminals are
    /// treated as non-shell.
    pub fn foreground_process_is_shell(&self) -> bool {
        self.foreground_process_is_shell_context() == Some(true)
    }

    /// Sends an already-built editor command through this terminal's shell.
    pub fn submit_editor_command(&mut self, command: String) {
        self.submit_editor_command_inner(command);
    }

    fn submit_editor_command_inner(&mut self, command: String) {
        self.input(command.into_bytes());
        // Keep Enter in its own write. WSL's ConPTY path can otherwise leave
        // a generated command at the prompt for the next editor action to
        // concatenate with it.
        self.write_to_pty(b"\x0d");
    }

    /// Sends a shell-level marker command and returns a task that completes when
    /// the marker appears in terminal output. Already complete for non-PTY
    /// terminals or those whose child has exited.
    ///
    /// Call at most once per terminal: a second handshake drops the previous
    /// `Sender`, which would write the init command twice.
    pub fn start_init_command_startup_handshake(&mut self) -> Task<()> {
        if !self.is_pty() || self.child_exited.is_some() || self.terminal_exit_reported {
            return Task::ready(());
        }

        debug_assert!(
            self.init_command_startup_tx.is_none(),
            "start_init_command_startup_handshake called while a handshake is already in flight"
        );

        let (startup_tx, startup_rx) = async_channel::bounded(1);
        let startup_task = self.background_executor.spawn(async move {
            match startup_rx.recv().await {
                Ok(()) | Err(_) => {}
            }
        });

        let marker_id = NEXT_INIT_COMMAND_STARTUP_MARKER_ID.fetch_add(1, Ordering::Relaxed);
        self.init_command_startup_marker = Some(init_command_startup_marker(marker_id));
        self.init_command_startup_tx = Some(startup_tx);

        let shell_kind = interaction_shell_kind(&self.template.shell, self.path_style);
        let mut input = init_command_startup_marker_command(shell_kind, marker_id).into_bytes();
        input.push(b'\x0d');
        self.write_to_pty(input);

        startup_task
    }

    fn detect_init_command_startup_marker(&mut self) {
        let Some(marker) = self.init_command_startup_marker.as_deref() else {
            return;
        };

        let has_marker = {
            let term = self.term.lock_unfair();
            last_non_empty_lines(&term, INIT_COMMAND_STARTUP_MARKER_SEARCH_LINES)
                .iter()
                .any(|line| line.contains(marker))
        };

        if has_marker {
            self.complete_init_command_startup_handshake();
        }
    }

    fn complete_init_command_startup_handshake(&mut self) {
        self.init_command_startup_marker = None;
        if let Some(startup_tx) = self.init_command_startup_tx.take() {
            match startup_tx.try_send(()) {
                Ok(()) | Err(async_channel::TrySendError::Full(())) => {}
                Err(async_channel::TrySendError::Closed(())) => {}
            }
        }
    }

    /// Write a programmatically-generated command to the PTY as if it had been
    /// typed, without marking the terminal as having received user keyboard
    /// input.
    pub fn write_init_command(&mut self, input: impl Into<Cow<'static, [u8]>>) {
        self.write_input(input);
    }

    pub fn is_pty(&self) -> bool {
        matches!(self.terminal_type, TerminalType::Pty { .. })
    }

    pub fn write_init_command_after_startup(
        &mut self,
        input: impl Into<Cow<'static, [u8]>>,
        cx: &mut Context<Self>,
    ) -> bool {
        // Ends the handshake even if the marker was never seen (timeout
        // fallback), so detection stops scanning on every wakeup.
        self.complete_init_command_startup_handshake();

        if self.keyboard_input_sent || self.child_exited.is_some() || self.terminal_exit_reported {
            return false;
        }

        self.clear_for_init_command(cx);
        self.write_init_command(input);
        true
    }

    fn clear_for_init_command(&mut self, cx: &mut Context<Self>) {
        let mut term = self.term.lock_unfair();
        clear_saved_screen(&mut term);
        self.last_content = make_content(&term, &mut self.last_content);
        self.content_revision = self.content_revision.wrapping_add(1);
        drop(term);
        self.reset_cwd_history();
        cx.emit(Event::Wakeup);
    }

    fn write_input(&mut self, input: impl Into<Cow<'static, [u8]>>) {
        let input: Cow<'static, [u8]> = input.into();
        // A submitted command is the boundary a directory change belongs to.
        if !self.is_remote_terminal && input.contains(&b'\r') {
            let term = self.term.lock_unfair();
            self.pending_cwd_boundary = Some(Self::scrollback_position(
                term.grid().cursor.point.line.0,
                term.history_size(),
            ));
        }

        self.events.push_back(InternalEvent::Scroll(Scroll::Bottom));
        self.events.push_back(InternalEvent::SetSelection(None));

        #[cfg(any(test, feature = "test-support"))]
        self.input_log.push(input.to_vec());

        self.write_to_pty(input);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn take_input_log(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.input_log)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn take_pty_write_log(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(self.pty_write_log.get_mut())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn keyboard_input_sent(&self) -> bool {
        self.keyboard_input_sent
    }

    pub fn toggle_vi_mode(&mut self) {
        self.events.push_back(InternalEvent::ToggleViMode);
    }

    pub fn vi_motion(&mut self, keystroke: &Keystroke) {
        if !self.vi_mode_enabled {
            return;
        }

        let key: Cow<'_, str> = if keystroke.modifiers.shift {
            Cow::Owned(keystroke.key.to_uppercase())
        } else {
            Cow::Borrowed(keystroke.key.as_str())
        };

        let motion: Option<ViMotion> = match key.as_ref() {
            "h" | "left" => Some(ViMotion::Left),
            "j" | "down" => Some(ViMotion::Down),
            "k" | "up" => Some(ViMotion::Up),
            "l" | "right" => Some(ViMotion::Right),
            "w" => Some(ViMotion::WordRight),
            "b" if !keystroke.modifiers.control => Some(ViMotion::WordLeft),
            "e" => Some(ViMotion::WordRightEnd),
            "%" => Some(ViMotion::Bracket),
            "$" => Some(ViMotion::Last),
            "0" => Some(ViMotion::First),
            "^" => Some(ViMotion::FirstOccupied),
            "H" => Some(ViMotion::High),
            "M" => Some(ViMotion::Middle),
            "L" => Some(ViMotion::Low),
            "{" => Some(ViMotion::ParagraphUp),
            "}" => Some(ViMotion::ParagraphDown),
            _ => None,
        };

        if let Some(motion) = motion {
            let cursor = self.last_content.cursor.point;
            let cursor_pos = GpuiPoint {
                x: cursor.column as f32 * self.last_content.terminal_bounds.cell_width,
                y: cursor.line as f32 * self.last_content.terminal_bounds.line_height,
            };
            self.events
                .push_back(InternalEvent::UpdateSelection(cursor_pos));
            self.events.push_back(InternalEvent::ViMotion(motion));
            return;
        }

        let scroll_motion = match key.as_ref() {
            "g" => Some(Scroll::Top),
            "G" => Some(Scroll::Bottom),
            "b" if keystroke.modifiers.control => Some(Scroll::PageUp),
            "f" if keystroke.modifiers.control => Some(Scroll::PageDown),
            "d" if keystroke.modifiers.control => {
                let amount = self.last_content.terminal_bounds.line_height().to_f64() as i32 / 2;
                Some(Scroll::Delta(-amount))
            }
            "u" if keystroke.modifiers.control => {
                let amount = self.last_content.terminal_bounds.line_height().to_f64() as i32 / 2;
                Some(Scroll::Delta(amount))
            }
            _ => None,
        };

        if let Some(scroll_motion) = scroll_motion {
            self.events.push_back(InternalEvent::Scroll(scroll_motion));
            return;
        }

        match key.as_ref() {
            "v" => {
                let point = self.last_content.cursor.point;
                let selection_type = SelectionType::Simple;
                let side = SelectionSide::Right;
                let selection = Selection::new(selection_type, point, side);
                self.events
                    .push_back(InternalEvent::SetSelection(Some(selection)));
            }

            "V" => {
                let point = self.last_content.cursor.point;
                let selection_type = SelectionType::Lines;
                let side = SelectionSide::Right;
                let selection = Selection::new(selection_type, point, side);
                self.events
                    .push_back(InternalEvent::SetSelection(Some(selection)));
            }

            "escape" => {
                self.events.push_back(InternalEvent::SetSelection(None));
            }

            "y" => {
                self.copy(Some(false));
            }

            "i" => {
                self.scroll_to_bottom();
                self.toggle_vi_mode();
            }
            _ => {}
        }
    }

    pub fn try_keystroke(&mut self, keystroke: &Keystroke, option_as_meta: bool) -> bool {
        if self.vi_mode_enabled {
            self.vi_motion(keystroke);
            return true;
        }

        // Keep default terminal behavior
        let esc = to_esc_str(keystroke, self.last_content.mode, option_as_meta);
        if let Some(esc) = esc {
            match esc {
                Cow::Borrowed(string) => self.input(string.as_bytes()),
                Cow::Owned(string) => self.input(string.into_bytes()),
            };
            true
        } else {
            false
        }
    }

    pub fn try_modifiers_change(
        &mut self,
        modifiers: &Modifiers,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .last_content
            .terminal_bounds
            .bounds
            .contains(&window.mouse_position())
            && is_hyperlink_modifier(modifiers)
        {
            self.refresh_hovered_word(window);
        }
        cx.notify();
    }

    ///Paste text into the terminal
    pub fn paste(&mut self, text: &str) {
        let paste_text = if self.last_content.mode.contains(Modes::BRACKETED_PASTE) {
            format!("{}{}{}", "\x1b[200~", text.replace('\x1b', ""), "\x1b[201~")
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };

        self.input(paste_text.into_bytes());
    }

    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let diagnostic = TerminalSyncDiagnostic::begin();
        let term = self.term.clone();
        let mut terminal = term.lock_unfair();
        diagnostic.acquired_grid();
        //Note that the ordering of events matters for event processing
        while let Some(e) = self.events.pop_front() {
            self.process_terminal_event(&e, &mut terminal, window, cx)
        }

        if self.content_dirty {
            self.last_content = make_content(&terminal, &mut self.last_content);
            self.content_dirty = false;
            self.content_revision = self.content_revision.wrapping_add(1);

            if self.last_content.grid_lines_change == GridLinesChange::Changed {
                debug_assert!(self.last_content.last_hovered_word.is_none());
                self.refresh_hovered_word(window);

                // The search it queued is drained by the next `sync`, and
                // nothing else here schedules a frame for it to run in.
                if !self.events.is_empty() {
                    cx.emit(Event::Wakeup);
                }
            }
        }
    }

    pub fn with_renderable_cells<R>(&self, f: impl for<'a> FnOnce(RenderableCells<'a>) -> R) -> R {
        let term = self.term.lock_unfair();
        let content = term.renderable_content();
        f(RenderableCells::new(content.display_iter))
    }

    pub fn get_content(&self) -> String {
        let term = self.term.lock_unfair();
        content_text(&term)
    }

    /// Takes a plain-text snapshot of the complete retained terminal buffer on
    /// the background executor so grid traversal and text construction do not
    /// run on the UI thread.
    pub fn get_content_async(&self) -> Task<String> {
        let term = self.term.clone();
        self.background_executor.spawn(async move {
            let term = term.lock_unfair();
            content_text(&term)
        })
    }

    pub fn last_n_non_empty_lines(&self, n: usize) -> Vec<String> {
        let terminal = self.term.lock_unfair();
        last_non_empty_lines(&terminal, n)
    }

    pub fn focus_in(&self) {
        if self.last_content.mode.contains(Modes::FOCUS_IN_OUT) {
            self.write_to_pty("\x1b[I".as_bytes());
        }
    }

    pub fn focus_out(&mut self) {
        if self.last_content.mode.contains(Modes::FOCUS_IN_OUT) {
            self.write_to_pty("\x1b[O".as_bytes());
        }
    }

    fn mouse_changed(&mut self, point: Point, side: SelectionSide) -> bool {
        match self.last_mouse {
            Some((old_point, old_side)) => {
                if old_point == point && old_side == side {
                    false
                } else {
                    self.last_mouse = Some((point, side));
                    true
                }
            }
            None => {
                self.last_mouse = Some((point, side));
                true
            }
        }
    }

    pub fn mouse_mode(&self, shift: bool) -> bool {
        self.last_content.mode.intersects(Modes::MOUSE_MODE) && !shift
    }

    /// Pointer motion arrives at the mouse's polling rate, which is routinely
    /// several times the display refresh rate. Notifying unconditionally would
    /// re-run grid layout and text shaping for the whole viewport on every
    /// sample, so each branch reports whether it actually changed something the
    /// next frame would draw differently.
    pub fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        let needs_redraw = if self.mouse_mode(e.modifiers.shift) {
            // A ctrl/cmd press on a link suppressed its button-press report in
            // `mouse_down`. Since the app never saw the press, we must swallow
            // the whole gesture rather than forward later motion/release
            // reports, which would be a press-less (malformed) sequence.
            // `mouse_up` resolves it: release on the same link opens it,
            // otherwise the gesture is dropped.
            if self.mouse_down_hyperlink.is_none() {
                let (point, side) = grid_point_and_side(
                    position,
                    self.last_content.terminal_bounds,
                    self.last_content.display_offset,
                );

                if self.mouse_changed(point, side) {
                    let bytes = mouse_moved_report(
                        point,
                        e.pressed_button,
                        e.modifiers,
                        self.last_content.mode,
                    );

                    if let Some(bytes) = bytes {
                        self.write_to_pty(bytes);
                    }
                    // The cell under the pointer changed, so anything keyed to
                    // it (such as the mouse cursor style) may need repainting.
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            self.schedule_find_hyperlink(e.modifiers, e.position)
        };

        if needs_redraw {
            cx.notify();
        }
    }

    /// Returns whether hover state changed such that the next frame would
    /// differ. Clearing an already-empty hover, or a sample the throttle
    /// rejects, leaves the rendered output untouched.
    fn schedule_find_hyperlink(
        &mut self,
        modifiers: Modifiers,
        position: GpuiPoint<Pixels>,
    ) -> bool {
        if self.selection_phase == SelectionPhase::Selecting
            || !is_hyperlink_modifier(&modifiers)
            || !self.last_content.terminal_bounds.bounds.contains(&position)
        {
            return self.last_content.last_hovered_word.take().is_some();
        }

        // Throttle hyperlink searches to avoid excessive processing
        let now = Instant::now();
        if self
            .last_hyperlink_search_position
            .map_or(true, |last_pos| {
                // Only search if mouse moved significantly or enough time passed
                let distance_moved = ((position.x - last_pos.x).abs()
                    + (position.y - last_pos.y).abs())
                    > FIND_HYPERLINK_THROTTLE_PX;
                let time_elapsed = now.duration_since(self.last_mouse_move_time).as_millis() > 100;
                distance_moved || time_elapsed
            })
        {
            self.last_mouse_move_time = now;
            self.last_hyperlink_search_position = Some(position);
            self.events.push_back(InternalEvent::FindHyperlink(
                position - self.last_content.terminal_bounds.bounds.origin,
                false,
            ));
            // The queued search is drained by the next `sync`, so a frame has
            // to be scheduled for it to run at all.
            return true;
        }

        false
    }

    pub fn select_word_at_event_position(&mut self, e: &MouseDownEvent) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        let (point, side) = grid_point_and_side(
            position,
            self.last_content.terminal_bounds,
            self.last_content.display_offset,
        );
        let selection = Selection::new(SelectionType::Semantic, point, side);
        self.events
            .push_back(InternalEvent::SetSelection(Some(selection)));
    }

    /// Finds a local path-like target at a mouse position without changing the
    /// terminal selection or hyperlink hover state.
    pub fn path_like_target_at_event_position(
        &mut self,
        e: &MouseDownEvent,
    ) -> Option<PathLikeTarget> {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        let point = grid_point(
            position,
            self.last_content.terminal_bounds,
            self.last_content.display_offset,
        );
        let hyperlink =
            self.find_hyperlink_at_point_with_path_style(point, self.hyperlink_path_style())?;
        let match_line = hyperlink.range.start().line;
        let history_size = self.term.lock().history_size();
        let maybe_path = if hyperlink.is_url {
            let path = hyperlink.text.strip_prefix("file://")?;
            urlencoding::decode(path)
                .map(|decoded| decoded.into_owned())
                .unwrap_or_else(|_| path.to_owned())
        } else {
            hyperlink.text
        };
        #[cfg(windows)]
        let maybe_path =
            cygwin_path_like_to_windows(&self.template.shell, &maybe_path).unwrap_or(maybe_path);

        Some(PathLikeTarget {
            maybe_path,
            terminal_dir: self.editor_path_working_directory_at_line(match_line, history_size),
            path_style: self.editor_path_style(),
        })
    }

    /// The directory a path printed on `line` should be resolved against.
    fn editor_path_working_directory_at_line(
        &self,
        line: i32,
        history_size: usize,
    ) -> Option<PathBuf> {
        // The WSL and Cygwin translations map the shell's *currently* reported
        // directory into a Windows path, and the recorded history is not in that
        // form, so those hosts keep resolving against the current directory.
        #[cfg(windows)]
        if posix_host(&self.template.shell).is_some() {
            return self.editor_path_working_directory();
        }
        self.cwd_at_line(line, history_size)
    }

    /// Only WSL and Cygwin need this: elsewhere the shell's directory needs no
    /// translation, and [`Self::cwd_at_line`] resolves it per line instead.
    #[cfg(windows)]
    fn editor_path_working_directory(&self) -> Option<PathBuf> {
        #[cfg(windows)]
        match posix_host(&self.template.shell) {
            Some(PosixHost::Wsl) => {
                return wsl_editor_working_directory(
                    &self.template.shell,
                    self.reported_working_directory.as_deref(),
                );
            }
            Some(PosixHost::Cygwin) => {
                return cygwin_editor_working_directory(
                    &self.template.shell,
                    self.reported_working_directory.as_deref(),
                );
            }
            _ => {}
        }
        self.working_directory()
    }

    fn editor_path_style(&self) -> PathStyle {
        #[cfg(windows)]
        if matches!(posix_host(&self.template.shell), Some(PosixHost::Wsl)) {
            return PathStyle::Unix;
        }
        self.path_style
    }

    fn hyperlink_path_style(&self) -> PathStyle {
        #[cfg(windows)]
        if matches!(posix_host(&self.template.shell), Some(PosixHost::Cygwin)) {
            return PathStyle::Unix;
        }
        self.path_style
    }

    pub fn begin_editor_click(&mut self) {
        self.editor_click_started = true;
    }

    pub fn editor_click_started(&self) -> bool {
        self.editor_click_started
    }

    pub fn mouse_drag(
        &mut self,
        e: &MouseMoveEvent,
        region: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        if !self.mouse_mode(e.modifiers.shift) {
            if let Some(hyperlink) = &self.mouse_down_hyperlink {
                let point = grid_point(
                    position,
                    self.last_content.terminal_bounds,
                    self.last_content.display_offset,
                );

                if !hyperlink.range.contains(point) {
                    self.mouse_down_hyperlink = None;
                } else {
                    return;
                }
            }

            // Ignore tiny pointer movements so that a click that jitters by a
            // pixel or two (e.g. the window-focusing click) does not begin a
            // selection. Mirrors the drag threshold used by gpui's `div`.
            if self.selection_phase != SelectionPhase::Selecting
                && let Some(mouse_down_position) = self.mouse_down_position
                && (e.position - mouse_down_position).magnitude() <= SELECTION_DRAG_THRESHOLD
            {
                return;
            }

            self.selection_phase = SelectionPhase::Selecting;
            // Alacritty has the same ordering, of first updating the selection
            // then scrolling 15ms later
            self.events
                .push_back(InternalEvent::UpdateSelection(position));

            // Doesn't make sense to scroll the alt screen
            if !self.last_content.mode.contains(Modes::ALT_SCREEN) {
                let scroll_lines = match self.drag_line_delta(e, region) {
                    Some(value) => value,
                    None => return,
                };

                self.events
                    .push_back(InternalEvent::Scroll(Scroll::Delta(scroll_lines)));
            }

            cx.notify();
        }
    }

    fn drag_line_delta(&self, e: &MouseMoveEvent, region: Bounds<Pixels>) -> Option<i32> {
        let top = region.origin.y;
        let bottom = region.bottom_left().y;

        let scroll_lines = if e.position.y < top {
            let scroll_delta = (top - e.position.y).pow(1.1);
            (scroll_delta / self.last_content.terminal_bounds.line_height).ceil() as i32
        } else if e.position.y > bottom {
            let scroll_delta = -((e.position.y - bottom).pow(1.1));
            (scroll_delta / self.last_content.terminal_bounds.line_height).floor() as i32
        } else {
            return None;
        };

        Some(scroll_lines.clamp(-3, 3))
    }

    pub fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        self.editor_click_started = false;
        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        let point = grid_point(
            position,
            self.last_content.terminal_bounds,
            self.last_content.display_offset,
        );

        if e.button == MouseButton::Left
            && is_hyperlink_modifier(&e.modifiers)
            && (TerminalSettings::get_global(cx).open_links_in_mouse_mode
                || !self.mouse_mode(e.modifiers.shift))
        {
            self.mouse_down_hyperlink = self.find_hyperlink_at_point(point);

            if self.mouse_down_hyperlink.is_some() {
                return;
            }
        }

        if self.mouse_mode(e.modifiers.shift) {
            let bytes =
                mouse_button_report(point, e.button, e.modifiers, true, self.last_content.mode);

            if let Some(bytes) = bytes {
                self.write_to_pty(bytes);
            }
        } else {
            match e.button {
                MouseButton::Left => {
                    self.mouse_down_position = Some(e.position);
                    let (point, side) = grid_point_and_side(
                        position,
                        self.last_content.terminal_bounds,
                        self.last_content.display_offset,
                    );

                    let selection_type = match e.click_count {
                        0 => return, //This is a release
                        1 => Some(SelectionType::Simple),
                        2 => Some(SelectionType::Semantic),
                        3 => Some(SelectionType::Lines),
                        _ => None,
                    };

                    if selection_type == Some(SelectionType::Simple) && e.modifiers.shift {
                        if self.last_content.selection.is_some() {
                            // Shift+click extends the existing selection to this point.
                            self.events
                                .push_back(InternalEvent::UpdateSelection(position));
                        } else {
                            // With no selection yet, Shift is the escape hatch for
                            // selecting text while an app has mouse tracking enabled,
                            // so anchor a selection here for the drag to extend.
                            self.events.push_back(InternalEvent::SetSelection(Some(
                                Selection::new(SelectionType::Simple, point, side),
                            )));
                        }
                        return;
                    }

                    let selection = selection_type
                        .map(|selection_type| Selection::new(selection_type, point, side));

                    if let Some(selection) = selection {
                        self.events
                            .push_back(InternalEvent::SetSelection(Some(selection)));
                    }
                }
                MouseButton::Middle => {
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    let text = cx
                        .read_from_primary()
                        .and_then(|item| item.text())
                        .filter(|text| !text.is_empty())
                        .or_else(|| cx.read_from_clipboard().and_then(|item| item.text()));
                    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
                    let text = cx.read_from_clipboard().and_then(|item| item.text());

                    if let Some(text) = text {
                        self.paste(&text);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn mouse_up(&mut self, e: &MouseUpEvent, cx: &Context<Self>) {
        if self.editor_click_started {
            self.editor_click_started = false;
            self.selection_phase = SelectionPhase::Ended;
            self.last_mouse = None;
            self.mouse_down_position = None;
            return;
        }
        let setting = TerminalSettings::get_global(cx);

        let position = e.position - self.last_content.terminal_bounds.bounds.origin;
        if let Some(mouse_down_hyperlink) = self.mouse_down_hyperlink.take() {
            let point = grid_point(
                position,
                self.last_content.terminal_bounds,
                self.last_content.display_offset,
            );

            if self
                .find_hyperlink_at_point(point)
                .is_some_and(|mouse_up_hyperlink| mouse_up_hyperlink == mouse_down_hyperlink)
            {
                self.events
                    .push_back(InternalEvent::ProcessHyperlink(mouse_down_hyperlink, true));
                self.selection_phase = SelectionPhase::Ended;
                self.last_mouse = None;
                self.mouse_down_position = None;
                return;
            }

            if self.mouse_mode(e.modifiers.shift) {
                self.selection_phase = SelectionPhase::Ended;
                self.last_mouse = None;
                self.mouse_down_position = None;
                return;
            }
        }

        if self.mouse_mode(e.modifiers.shift) {
            let point = grid_point(
                position,
                self.last_content.terminal_bounds,
                self.last_content.display_offset,
            );

            let bytes =
                mouse_button_report(point, e.button, e.modifiers, false, self.last_content.mode);

            if let Some(bytes) = bytes {
                self.write_to_pty(bytes);
            }
        } else {
            if e.button == MouseButton::Left && setting.copy_on_select {
                self.copy(Some(true));
            }

            //Hyperlinks
            //
            // An OSC 8 hyperlink carries a URI chosen by whatever wrote to the
            // terminal, and the visible text need not resemble it. Requiring the
            // same modifier as a detected URL means output cannot turn an
            // ordinary click into a system-handled open of, say, a `file://` or
            // custom-scheme target.
            if self.selection_phase == SelectionPhase::Ended && is_hyperlink_modifier(&e.modifiers)
            {
                let mouse_cell_index =
                    content_index_for_mouse(position, &self.last_content.terminal_bounds);
                if let Some(link) = self
                    .last_content
                    .cells
                    .get(mouse_cell_index)
                    .and_then(|cell| cell.hyperlink())
                {
                    cx.open_url(link.uri());
                } else {
                    self.events
                        .push_back(InternalEvent::FindHyperlink(position, true));
                }
            }
        }

        self.selection_phase = SelectionPhase::Ended;
        self.last_mouse = None;
        self.mouse_down_position = None;
    }

    ///Scroll the terminal
    pub fn scroll_wheel(&mut self, e: &ScrollWheelEvent, scroll_multiplier: f32) {
        let mouse_mode = self.mouse_mode(e.shift);
        let scroll_multiplier = if mouse_mode { 1. } else { scroll_multiplier };

        if let Some(scroll_lines) = self.determine_scroll_lines(e, scroll_multiplier)
            && scroll_lines != 0
        {
            if mouse_mode {
                let point = grid_point(
                    e.position - self.last_content.terminal_bounds.bounds.origin,
                    self.last_content.terminal_bounds,
                    self.last_content.display_offset,
                );

                if let Some(scrolls) = scroll_report(point, scroll_lines, e, self.last_content.mode)
                {
                    for scroll in scrolls {
                        self.write_to_pty(scroll);
                    }
                };
            } else if self
                .last_content
                .mode
                .contains(Modes::ALT_SCREEN | Modes::ALTERNATE_SCROLL)
                && !e.shift
            {
                self.write_to_pty(alt_scroll(scroll_lines));
            } else {
                self.events
                    .push_back(InternalEvent::Scroll(Scroll::Delta(scroll_lines)));
            }
        }
    }

    fn refresh_hovered_word(&mut self, window: &Window) {
        self.schedule_find_hyperlink(window.modifiers(), window.mouse_position());
    }

    fn determine_scroll_lines(
        &mut self,
        e: &ScrollWheelEvent,
        scroll_multiplier: f32,
    ) -> Option<i32> {
        let line_height = self.last_content.terminal_bounds.line_height;
        match e.touch_phase {
            /* Reset scroll state on started */
            TouchPhase::Started => {
                self.scroll_px = px(0.);
                None
            }
            /* Calculate the appropriate scroll lines */
            TouchPhase::Moved => {
                let old_offset = (self.scroll_px / line_height) as i32;

                self.scroll_px += e.delta.pixel_delta(line_height).y * scroll_multiplier;

                let new_offset = (self.scroll_px / line_height) as i32;

                // Whenever we hit the edges, reset our stored scroll to 0
                // so we can respond to changes in direction quickly
                self.scroll_px %= self.last_content.terminal_bounds.height();

                Some(new_offset - old_offset)
            }
            // Cancellation does not commit a scroll, same as a plain end.
            TouchPhase::Ended | TouchPhase::Cancelled => None,
        }
    }

    pub fn find_matches(&self, searcher: Search, cx: &Context<Self>) -> Task<SearchMatches> {
        // Snapshotting copies only the bounded directly mutable live prefix and shares sealed
        // immutable history chunks. Searching that snapshot keeps the live PTY/render lock free
        // for input and drawing.
        let term = self.term.lock().clone();
        let executor = cx.background_executor().clone();
        executor.spawn_with_priority(Priority::Low, async move {
            let mut search = ScrollbackSearch::new(&term, searcher);
            loop {
                let finished = search.advance(&term, SEARCH_CHUNK_LINES, MAX_SEARCH_MATCHES);
                if finished {
                    return search.finish();
                }
                // Dropping the owning GPUI task (for example when the query or tab closes)
                // cancels between chunks. The search runs at low priority against an immutable
                // snapshot, so no live terminal lock needs an artificial scheduling delay.
                yield_now().await;
            }
        })
    }

    /// Records that the shell moved, attributing the move to the command that
    /// caused it rather than to wherever the cursor happens to be when the
    /// change is finally observed.
    pub(crate) fn record_cwd_change(&mut self, new_working_directory: PathBuf) {
        // `ProcessInfo::cwd` is an empty path when the process's directory could
        // not be read; recording that would resolve later clicks against the
        // filesystem root.
        if self.is_remote_terminal || new_working_directory.as_os_str().is_empty() {
            return;
        }

        let scrollback_position = self.pending_cwd_boundary.take().unwrap_or_else(|| {
            let term = self.term.lock_unfair();
            Self::scrollback_position(term.grid().cursor.point.line.0, term.history_size())
        });
        self.cwd_history.push(CwdHistoryEntry {
            scrollback_position,
            working_directory: new_working_directory,
        });
    }

    /// Drops the recorded history, for the cases that invalidate the line
    /// offsets it is keyed by: a reflow moves every line, and a clear discards
    /// them.
    fn reset_cwd_history(&mut self) {
        self.pending_cwd_boundary = None;
        self.cwd_history =
            initial_cwd_history(self.is_remote_terminal, self.working_directory().as_ref());
    }

    /// The directory the shell was in when `line` was printed, falling back to
    /// the current one when that cannot be established.
    fn cwd_at_line(&self, line: i32, history_size: usize) -> Option<PathBuf> {
        // Once the scrollback cap is reached, evictions move retained lines
        // without changing `history_size`, so stored offsets no longer identify
        // their original lines.
        if self.is_remote_terminal
            || self.cwd_history.is_empty()
            || history_size >= self.term_config.scrolling_history
        {
            return self.working_directory();
        }
        let scrollback_position = Self::scrollback_position(line, history_size);
        self.cwd_history
            .iter()
            .rev()
            .find(|entry| entry.scrollback_position <= scrollback_position)
            .map(|entry| entry.working_directory.clone())
            .or_else(|| self.working_directory())
    }

    fn scrollback_position(line: i32, history_size: usize) -> i32 {
        let history_size = i32::try_from(history_size).unwrap_or(i32::MAX);
        history_size.saturating_add(line)
    }

    pub fn working_directory(&self) -> Option<PathBuf> {
        if self.is_remote_terminal {
            // We can't yet reliably detect the working directory of a shell on the
            // SSH host. Until we can do that, it doesn't make sense to display
            // the working directory on the client and persist that.
            None
        } else {
            self.client_side_working_directory()
        }
    }

    /// Returns the cached working directory of the foreground process,
    /// without consulting a shell-reported CWD marker.
    pub fn process_working_directory(&self) -> Option<PathBuf> {
        if self.is_remote_terminal {
            return None;
        }
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => info
                .current
                .read()
                .as_ref()
                .map(|process| process.cwd.clone())
                .filter(|directory| !directory.as_os_str().is_empty()),
            TerminalType::DisplayOnly => None,
        }
    }

    /// Normalizes the command name of the foreground process, if one is known.
    pub fn foreground_process_command_name(&self) -> Option<String> {
        #[cfg(windows)]
        if posix_host(&self.template.shell).is_some() {
            return self
                .reported_foreground_command
                .as_deref()
                .and_then(reported_foreground_command_name);
        }
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => info
                .current
                .read()
                .as_ref()
                .and_then(|process| foreground_process_command_from_argv(&process.argv)),
            TerminalType::DisplayOnly => None,
        }
    }

    /// Returns the full argument vector of the foreground process, if one is known.
    pub fn foreground_process_command_line(&self) -> Option<Vec<String>> {
        #[cfg(windows)]
        if posix_host(&self.template.shell).is_some() {
            return self
                .reported_foreground_command
                .clone()
                .map(|command| vec![command]);
        }
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => info.current.read().as_ref().map(|process| {
                if process.argv.is_empty() {
                    vec![process.name.clone()]
                } else {
                    visible_process_argv(&process.argv).to_vec()
                }
            }),
            TerminalType::DisplayOnly => None,
        }
    }

    /// Refreshes cached foreground process metadata without requiring a rendered view.
    pub fn refresh_foreground_process(&mut self, cx: &mut Context<Self>) {
        if let TerminalType::Pty { info, .. } = &self.terminal_type {
            info.emit_title_changed_if_changed(cx);
        }
    }

    /// Returns the best available answer to whether the shell owns the
    /// foreground at this instant. Unknown process state is preserved as
    /// `None` so an exit is not mislabeled as a running-command failure.
    pub fn foreground_process_is_shell_context(&self) -> Option<bool> {
        #[cfg(windows)]
        if posix_host(&self.template.shell).is_some() {
            return self
                .reported_foreground_command
                .as_deref()
                .zip(self.reported_shell_command.as_deref())
                .map(|(foreground, shell)| foreground == shell);
        }
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => info.foreground_process_is_shell_context(),
            TerminalType::DisplayOnly => None,
        }
    }

    /// Returns the working directory of the process that's connected to the PTY.
    /// That means it returns the working directory of the local shell or program
    /// that's running inside the terminal.
    ///
    /// This does *not* return the working directory of the shell that runs on the
    /// remote host, in case Zed is connected to a remote host.
    fn client_side_working_directory(&self) -> Option<PathBuf> {
        if let Some(directory) = self.reported_working_directory.as_deref() {
            #[cfg(windows)]
            if matches!(posix_host(&self.template.shell), Some(PosixHost::Cygwin)) {
                if let Some(root) = cygwin_root_from_program(&self.template.shell.program())
                    && let Some(directory) = cygwin_path_to_windows(&root, directory)
                {
                    return Some(directory);
                }
            }
            let directory = PathBuf::from(directory);
            if directory.is_absolute() {
                return Some(directory);
            }
        }
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => info
                .current
                .read()
                .as_ref()
                .map(|process| process.cwd.clone())
                .filter(|directory| !directory.as_os_str().is_empty())
                .or_else(|| self.restored_working_directory.clone()),
            TerminalType::DisplayOnly => self.restored_working_directory.clone(),
        }
    }

    pub fn title(&self, truncate: bool) -> String {
        const MAX_CHARS: usize = 25;
        match &self.task {
            Some(task_state) => {
                if truncate {
                    truncate_and_trailoff(&task_state.spawned_task.label, MAX_CHARS)
                } else {
                    task_state.spawned_task.full_label.clone()
                }
            }
            // A profile's `title_override` (e.g. WSL/MSYS2/Cygwin's static profile title)
            // is only a placeholder for before any live info is available - it must
            // not mask a live-reported or foreground-process-derived title once one
            // exists, or the tab name would never update to reflect what's running.
            None => self
                .reported_foreground_command
                .as_deref()
                .filter(|command| !command.is_empty())
                .map(|command| {
                    if truncate {
                        truncate_and_trailoff(command, MAX_CHARS)
                    } else {
                        command.to_string()
                    }
                })
                .or_else(|| match &self.terminal_type {
                    TerminalType::Pty { info, .. } => info.current.read().as_ref().map(|fpi| {
                        let argv = visible_process_argv(&fpi.argv);
                        let process_name = format!(
                            "{}{}",
                            fpi.name,
                            if !argv.is_empty() {
                                format!(" {}", (argv[1..]).join(" "))
                            } else {
                                "".to_string()
                            }
                        );
                        if truncate {
                            truncate_and_trailoff(&process_name, MAX_CHARS)
                        } else {
                            process_name
                        }
                    }),
                    TerminalType::DisplayOnly => None,
                })
                .or_else(|| self.title_override.clone())
                .unwrap_or_else(|| "Terminal".to_string()),
        }
    }

    pub fn kill_active_task(&mut self) {
        if let Some(task) = self.task()
            && task.status == TaskStatus::Running
        {
            match &self.terminal_type {
                TerminalType::Pty { info, .. } => {
                    // First kill the foreground process group (the command running in the shell)
                    info.kill_current_process();
                    // Then kill the shell itself so that the terminal exits properly
                    // and wait_for_completed_task can complete
                    info.kill_child_process();
                }
                TerminalType::DisplayOnly => {
                    // Non-PTY task terminals own their subprocess directly.
                    if let Some(subprocess) = &self.subprocess {
                        subprocess.kill();
                    }
                }
            }
        }
    }

    pub fn pid(&self) -> Option<sysinfo::Pid> {
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => info.pid(),
            TerminalType::DisplayOnly => None,
        }
    }

    pub fn pid_getter(&self) -> Option<&ProcessIdGetter> {
        match &self.terminal_type {
            TerminalType::Pty { info, .. } => Some(info.pid_getter()),
            TerminalType::DisplayOnly => None,
        }
    }

    pub fn task(&self) -> Option<&TaskState> {
        self.task.as_ref()
    }

    /// Returns the numeric exit code reported by a completed task, if the
    /// task exited normally. A signal or another termination without a code
    /// is represented by `None`.
    pub fn task_exit_code(&self) -> Option<i32> {
        self.task_exit_code
    }

    pub fn wait_for_completed_task(&self, cx: &App) -> Task<Option<ExitStatus>> {
        if let Some(task) = self.task() {
            if task.status == TaskStatus::Running {
                let completion_receiver = task.completion_rx.clone();
                return cx.spawn(async move |_| completion_receiver.recv().await.ok().flatten());
            } else if let Ok(status) = task.completion_rx.try_recv() {
                return Task::ready(status);
            }
        }
        Task::ready(None)
    }

    fn register_terminal_exit(
        &mut self,
        exit_status: Option<ExitStatus>,
        source: TerminalExitSource,
        cx: &mut Context<Terminal>,
    ) {
        // `WatcherDisconnected` and `BackendShutdown` say the pty stopped being
        // usable, not that the child ended, so they keep the pty open: the
        // process may still be running, and closing the master would hang it up.
        if matches!(
            source,
            TerminalExitSource::Child | TerminalExitSource::StatusUnavailable
        ) {
            self.child_process_ended = true;
            self.release_pty_resources();
        }
        if self.task.is_some() {
            // Task terminals retain their established completion and hide
            // semantics. The richer interactive classification is for PTY
            // shells whose pane would otherwise disappear on CloseTerminal.
            self.register_task_finished(exit_status, cx);
            return;
        }
        if self.terminal_exit_reported {
            return;
        }
        self.terminal_exit_reported = true;
        // A caller that asked to be told when the child finished is told,
        // whether or not this terminal is running a task. The completion
        // channel is the only way it learns; the interactive classification
        // below decides what the *pane* does, which is a separate question.
        if let Some(tx) = &self.completion_tx {
            tx.try_send(exit_status).ok();
        }
        #[cfg(windows)]
        if let Some(timing) = self.wsl_startup_timing.take() {
            let observed_at = Instant::now();
            log_wsl_startup_phase(
                "exit_before_first_shell_marker",
                timing.started_at,
                timing.pty_ready_at,
                observed_at,
            );
        }
        if let Some(exit_status) = exit_status.as_ref() {
            self.child_exited = Some(exit_status.clone());
        }
        self.complete_init_command_startup_handshake();

        let exited = TerminalExited {
            exit_code: exit_status.as_ref().and_then(|status| status.code()),
            source,
            child_pid: self
                .pid_getter()
                .map(|pid_getter| pid_getter.fallback_pid())
                .filter(|pid| pid.as_u32() != 0)
                .map(|pid| pid.as_u32()),
            input_sent: self.keyboard_input_sent,
            foreground_is_shell: self.foreground_process_is_shell_context(),
            foreground_command: self.foreground_process_command_name(),
        };
        cx.emit(Event::TerminalExited(exited.clone()));
        if !exited.is_unexpected() {
            cx.emit(Event::CloseTerminal);
        }
    }

    /// Releases everything the pty event loop held, once its child has ended.
    ///
    /// The pane stays open after a shell exits so its output remains readable,
    /// and it used to keep the whole event loop alive with it: the loop thread
    /// returns its `EventLoop` rather than dropping it, and an un-joined
    /// `JoinHandle` keeps that value alive, so the pty master descriptor, the
    /// poller's descriptor and the loop's buffers were all retained per exited
    /// pane until the pane itself was closed.
    ///
    /// Idempotent, and a no-op for a terminal that is not pty-backed.
    fn release_pty_resources(&mut self) {
        let TerminalType::Pty { pty_tx, io, info } = &mut self.terminal_type else {
            return;
        };
        if let Some(pty_tx) = pty_tx.take() {
            pty_tx.shutdown();
        }
        // The loop's own control, which holds a reference to its poller. A
        // control the multiplexer provided is somebody else's and is left alone.
        if self.pty_control_is_local {
            self.pty_control = None;
            self.pty_control_is_local = false;
        }
        // Dropping the handle rather than joining it lets the loop thread finish
        // on its own; the terminal must not block on it. The master descriptor
        // goes with it, so stop reading foreground process groups through it.
        info.close_pty_handle();
        drop(io.take());
    }

    /// Stops this terminal's pty event loop, synchronously.
    ///
    /// Used when a pane is handed back to the multiplexer: the loop thread is
    /// the only other reader of the pty, and the next reader has to be able to
    /// wait for it to actually end, or the two would consume the pty's output
    /// between them. The grid stays intact; the terminal just stops being
    /// fed until [`Terminal::attach_byte_stream`] reconnects it.
    pub fn stop_pty_loop(&mut self) -> Result<()> {
        let TerminalType::Pty { pty_tx, io, info } = &mut self.terminal_type else {
            return Ok(());
        };
        if let Some(pty_tx) = pty_tx.take() {
            pty_tx.shutdown();
        }
        // Joining drops the loop's `EventLoop`, and with it the pty master this
        // borrows for foreground-process lookups.
        info.close_pty_handle();
        match io.take() {
            Some(io) => io.join(),
            None => Ok(()),
        }
    }

    /// Connects this terminal to a blocking bidirectional byte stream,
    /// replacing its pty event loop.
    ///
    /// Used when a pane is handed over to the multiplexer in shared mode: the
    /// terminal keeps its grid, and the stream relays what the multiplexer
    /// broadcast to the pane. The pty loop is stopped first if it is still
    /// running.
    pub fn attach_byte_stream(
        &mut self,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
    ) -> Result<()> {
        self.stop_pty_loop()?;
        self.byte_stream = Some(spawn_byte_stream(
            reader,
            writer,
            self.term.clone(),
            self.events_tx.clone(),
            self.wakeup_gate.clone(),
        ));
        Ok(())
    }

    /// Replaces this terminal's byte stream with a pty it now owns.
    ///
    /// The other half of [`Terminal::attach_byte_stream`], for a shared pane the
    /// multiplexer has handed back: the grid, its scrollback and everything on
    /// screen stay exactly as they are, and only what feeds them changes.
    ///
    /// Two orderings matter. The retired stream is drained first, because the
    /// bytes it still holds are *older* than anything the pty will produce and
    /// arriving after them would interleave the pane's output. And the pty event
    /// loop is only started once that is done, so the two never write to the grid
    /// at the same time.
    ///
    /// Works from either shape a shared pane can be in — a terminal that was
    /// revoked into sharing and kept its pty type, and one that was built as a
    /// byte stream and never had one — because it installs the pty backend
    /// wholesale rather than repairing what is there.
    pub fn attach_pty(
        &mut self,
        handover: PtyHandover,
        options: AttachedOptions,
        cx: &mut Context<Terminal>,
    ) -> Result<alacritty_terminal::tty::AttachedChildEvents> {
        anyhow::ensure!(
            handover.replay.is_empty(),
            "a pty handed back to its viewer must carry no replay: the viewer already has it"
        );
        let control = handover.control.clone();
        // Everything the relay had already read, into the grid, before the pty can
        // add to it.
        if let Some(mut stream) = self.byte_stream.take() {
            stream.drain_and_stop(BYTE_STREAM_DRAIN_TIMEOUT);
        }
        // A pty-backed terminal has to answer the sequences a program expects of
        // one; a byte-stream terminal was configured as display-only.
        let scrolling_history = options
            .max_scroll_history_lines
            .unwrap_or(DEFAULT_SCROLL_HISTORY_LINES)
            .min(MAX_SCROLL_HISTORY_LINES);
        self.term_config = pty_term_config(scrolling_history, options.cursor_shape);
        apply_config(&self.term, &self.term_config);

        #[cfg(unix)]
        let (pty, child_events) =
            alacritty_terminal::tty::attach(handover.descriptor, handover.child_pid)
                .context("adopting the multiplexer's terminal")?;
        #[cfg(windows)]
        let (pty, child_events) =
            alacritty_terminal::tty::attach(handover.conout, handover.conin, handover.child_pid)
                .context("adopting the multiplexer's terminal")?;
        let info = PtyProcessInfo::new(ProcessIdGetter::from(&pty));
        let listener = ZedListener::new(self.events_tx.clone(), self.wakeup_gate.clone());
        let (pty_tx, io) = spawn_event_loop(self.term.clone(), listener, pty, true)?;
        // Whatever was there before is replaced, including a stopped pty loop left
        // behind by the revoke that made this pane shared in the first place.
        self.terminal_type = TerminalType::Pty {
            pty_tx: Some(pty_tx),
            io: Some(io),
            info: Arc::new(info),
        };
        // This pane has a running child again, whatever became of the last one.
        self.child_process_ended = false;
        if let Some(palette) = self.last_console_palette {
            control.set_console_palette(palette);
        }
        self.pty_control = Some(control);
        // The multiplexer drives this pty now, so the control is no longer this
        // terminal's own sender and must not be released with the pty.
        self.pty_control_is_local = false;
        self.console_palette_enabled = cfg!(windows);
        // Handed back rather than opened here: the daemon is still this child's
        // parent, so this window must not end it. A pane that joined a shared
        // session has no provider to say so, and closing its tab killed the
        // session it had just been given.
        self.child_is_the_multiplexers = true;
        self.template.shell = options.shell;
        self.template.env = options.env;
        self.content_dirty = true;
        cx.notify();
        Ok(child_events)
    }

    /// Reports that the child ended without the multiplexer being able to say
    /// how.
    ///
    /// Rare but real: the multiplexer observed the process end without a status
    /// — or a client asked what it missed and was told only that the pane is
    /// gone. Either way the pane has to be told *something*. Saying nothing
    /// leaves a shared pane waiting for a report that has already been and gone,
    /// which is a terminal that can never be closed, with nothing on screen to
    /// explain why.
    pub fn report_child_exit_status_unavailable(
        &mut self,
        input_sent: bool,
        cx: &mut Context<Terminal>,
    ) {
        self.keyboard_input_sent = input_sent;
        self.register_terminal_exit(None, TerminalExitSource::StatusUnavailable, cx);
    }

    /// Reports the child's exit status as the multiplexer observed it.
    ///
    /// A shared pane's terminal no longer reads the pty itself, so it never
    /// learns of an exit from the event loop; the multiplexer is the child's
    /// parent and reports what it reaped. The daemon's attribution of input
    /// replaces this terminal's own, which only counted keys typed at it.
    pub fn report_child_exit(
        &mut self,
        exit_status: ExitStatus,
        input_sent: bool,
        cx: &mut Context<Terminal>,
    ) {
        self.keyboard_input_sent = input_sent;
        self.register_terminal_exit(Some(exit_status), TerminalExitSource::Child, cx);
    }

    fn register_task_finished(
        &mut self,
        exit_status: Option<ExitStatus>,
        cx: &mut Context<Terminal>,
    ) {
        if let Some(tx) = &self.completion_tx {
            tx.try_send(exit_status).ok();
        }
        if let Some(e) = exit_status {
            self.child_exited = Some(e);
        }
        self.complete_init_command_startup_handshake();
        let task = match &mut self.task {
            Some(task) => task,
            None => {
                // For interactive shells (no task), we need to differentiate:
                // 1. User-initiated exits (typed "exit", Ctrl+D, etc.) - always close,
                //    even if the shell exits with a non-zero code (e.g. after `false`).
                // 2. Shell spawn failures (bad $SHELL) - don't close, so the user sees
                //    the error. Spawn failures never receive keyboard input.
                let should_close = if self.keyboard_input_sent {
                    true
                } else {
                    self.child_exited.is_none_or(|e| e.code() == Some(0))
                };
                if should_close {
                    cx.emit(Event::CloseTerminal);
                }
                return;
            }
        };
        if task.status != TaskStatus::Running {
            return;
        }
        match exit_status.and_then(|e| e.code()) {
            Some(error_code) => {
                task.status.register_task_exit(error_code);
            }
            None => {
                task.status.register_terminal_exit();
            }
        };
        self.task_exit_code = exit_status.and_then(|status| status.code());
        cx.emit(Event::TaskFinished {
            exit_code: self.task_exit_code,
        });

        let (finished_successfully, task_line, command_line) = task_summary(task, exit_status);
        let mut lines_to_show = Vec::new();
        if task.spawned_task.show_summary {
            lines_to_show.push(task_line.as_str());
        }
        if task.spawned_task.show_command {
            lines_to_show.push(command_line.as_str());
        }
        let hide = task.spawned_task.hide;

        if !lines_to_show.is_empty() {
            // SAFETY: the invocation happens on non `TaskStatus::Running` tasks, once,
            // after either `AlacTermEvent::Exit` or `AlacTermEvent::ChildExit` events that are spawned
            // when Zed task finishes and no more output is made.
            // After the task summary is output once, no more text is appended to the terminal.
            unsafe { append_text_to_term(&mut self.term.lock(), &lines_to_show) };
        }

        match hide {
            HideStrategy::Never => {}
            HideStrategy::Always => {
                cx.emit(Event::CloseTerminal);
            }
            HideStrategy::OnSuccess => {
                if finished_successfully {
                    cx.emit(Event::CloseTerminal);
                }
            }
        }
    }

    pub fn vi_mode_enabled(&self) -> bool {
        self.vi_mode_enabled
    }

    pub fn clone_builder(&self, cx: &App, cwd: Option<PathBuf>) -> Task<Result<TerminalBuilder>> {
        let working_directory = self.working_directory().or_else(|| cwd);
        TerminalBuilder::new(
            working_directory,
            None,
            self.template.shell.clone(),
            self.template.env.clone(),
            self.template.cursor_shape,
            self.template.alternate_scroll,
            self.template.max_scroll_history_lines,
            self.template.path_hyperlink_regexes.clone(),
            self.template.path_hyperlink_timeout_ms,
            self.is_remote_terminal,
            self.template.window_id,
            None,
            cx,
            self.activation_script.clone(),
            self.path_style,
            self.template.pty_provider.clone(),
        )
    }
}

const TASK_DELIMITER: &str = "⏵ ";
fn task_summary(task: &TaskState, exit_status: Option<ExitStatus>) -> (bool, String, String) {
    let escaped_full_label = task
        .spawned_task
        .full_label
        .replace("\r\n", "\r")
        .replace('\n', "\r");
    let task_label = |suffix: &str| format!("{TASK_DELIMITER}Task `{escaped_full_label}` {suffix}");
    let (success, task_line) = match exit_status {
        Some(status) => {
            let code = status.code();
            #[cfg(unix)]
            let signal = status.signal();
            #[cfg(not(unix))]
            let signal: Option<i32> = None;

            match (code, signal) {
                (Some(0), _) => (true, task_label("finished successfully")),
                (Some(code), _) => (
                    false,
                    task_label(&format!("finished with exit code: {code}")),
                ),
                (None, Some(signal)) => (
                    false,
                    task_label(&format!("terminated by signal: {signal}")),
                ),
                (None, None) => (false, task_label("finished")),
            }
        }
        None => (false, task_label("finished")),
    };
    let escaped_command_label = task
        .spawned_task
        .command_label
        .replace("\r\n", "\r")
        .replace('\n', "\r");
    let command_line = format!("{TASK_DELIMITER}Command: {escaped_command_label}");
    (success, task_line, command_line)
}

/// Converts bare LFs into CRLFs so output captured from a pipe (rather than a
/// PTY) wraps correctly in Alacritty. A PTY's line discipline performs this
/// `ONLCR` translation for us; piped output (e.g. `ls` run outside a PTY) only
/// emits `\n`, which moves Alacritty's cursor down without returning it to
/// column zero and makes the rendered output look misaligned. Alacritty has no
/// setting for this, so we insert a `\r` before each `\n` that lacks one.
fn convert_lf_to_crlf(bytes: &[u8], previous_byte_was_cr: &mut bool) -> Vec<u8> {
    let mut converted = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        if byte == b'\n' && !*previous_byte_was_cr {
            converted.push(b'\r');
        }
        converted.push(byte);
        *previous_byte_was_cr = byte == b'\r';
    }
    converted
}

/// Owns a non-PTY task subprocess and the background task pumping its output
/// into the terminal emulator. Used by headless hosts (e.g. the eval CLI) where
/// PTY allocation fails with `ENOTTY`. Dropping this kills the child.
struct SubprocessHandle {
    child: Arc<parking_lot::Mutex<Option<util::process::Child>>>,
    _reader: Task<()>,
}

/// Owns the workers used by a blocking bidirectional byte stream.
struct ByteStreamHandle {
    input_tx: Option<mpsc::Sender<Vec<u8>>>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
    /// Signalled by the reader thread as it returns, so a caller can tell
    /// "everything the stream held is in the grid" from "the thread is still
    /// working through it".
    finished: mpsc::Receiver<()>,
    _reader: thread::JoinHandle<()>,
    _writer: thread::JoinHandle<()>,
}

impl ByteStreamHandle {
    fn write(&self, bytes: Vec<u8>) {
        if let Some(input_tx) = &self.input_tx {
            input_tx.send(bytes).ok();
        }
    }

    fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.input_tx.take();
    }

    /// Stops the stream once it has read everything still in flight.
    ///
    /// Order matters, and it is the whole reason this exists. When a shared pane
    /// is handed back, the multiplexer flushes what it had queued and closes its
    /// end; those bytes are *older* than anything the terminal will read from the
    /// pty next, so they have to reach the grid first. Setting the stop flag
    /// straight away would abandon whatever was still buffered and lose it.
    ///
    /// Waits for the reader's own end-of-stream, then falls back to the flag so a
    /// multiplexer that failed to close its end cannot hang the caller.
    fn drain_and_stop(&mut self, patience: Duration) {
        self.input_tx.take();
        // `Disconnected` is the success case: the sender is dropped as the reader
        // thread returns, which is precisely "it read everything there was".
        match self.finished.recv_timeout(patience) {
            Err(mpsc::RecvTimeoutError::Timeout) => {
                log::warn!("a byte stream did not end within {patience:?}; abandoning it");
                self.stopped.store(true, Ordering::Release);
            }
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
    }
}

fn spawn_byte_stream(
    mut reader: Box<dyn Read + Send>,
    mut writer: Box<dyn Write + Send>,
    term: Arc<AlacrittyTermLock>,
    events_tx: futures::channel::mpsc::UnboundedSender<PtyEvent>,
    wakeup_gate: WakeupGate,
) -> ByteStreamHandle {
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader_stopped = stopped.clone();
    let reader_events = events_tx.clone();
    let (finished_tx, finished) = mpsc::channel::<()>();
    let reader_thread = thread::Builder::new()
        .name("terminal-byte-stream-reader".to_owned())
        .spawn(move || {
            // Dropped as the thread returns, whichever way it returns, which is
            // what `drain_and_stop` waits on.
            let _finished = finished_tx;
            let mut processor = Processor::<StdSyncHandler>::new();
            let mut buffer = [0u8; 8192];
            let mut previous_byte_was_cr = false;
            while !reader_stopped.load(Ordering::Acquire) {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) => {}
                    Err(error) => {
                        log::warn!("byte stream read failed: {error}");
                        let message = format!("\r\n[Connection error: {error}]\r\n");
                        let mut terminal = term.lock();
                        processor.advance(&mut *terminal, message.as_bytes());
                        drop(terminal);
                        if wakeup_gate.is_enabled() {
                            reader_events
                                .unbounded_send(PtyEvent::Event(TerminalBackendEvent::Wakeup))
                                .ok();
                        }
                        break;
                    }
                    Ok(count) => {
                        let converted =
                            convert_lf_to_crlf(&buffer[..count], &mut previous_byte_was_cr);
                        let mut terminal = term.lock();
                        processor.advance(&mut *terminal, &converted);
                        drop(terminal);
                        if wakeup_gate.is_enabled() {
                            reader_events
                                .unbounded_send(PtyEvent::Event(TerminalBackendEvent::Wakeup))
                                .ok();
                        }
                    }
                }
            }
        })
        .expect("spawning a terminal byte-stream reader");

    let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
    let writer_stopped = stopped.clone();
    let writer_thread = thread::Builder::new()
        .name("terminal-byte-stream-writer".to_owned())
        .spawn(move || {
            while !writer_stopped.load(Ordering::Acquire) {
                let Ok(bytes) = input_rx.recv() else { break };
                if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
                    log::warn!("byte stream write failed: {error}");
                    break;
                }
            }
        })
        .expect("spawning a terminal byte-stream writer");

    ByteStreamHandle {
        input_tx: Some(input_tx),
        stopped,
        finished,
        _reader: reader_thread,
        _writer: writer_thread,
    }
}

impl SubprocessHandle {
    fn kill(&self) {
        if let Some(child) = self.child.lock().as_mut() {
            child.kill().log_err();
        }
    }
}

/// Spawns `program`/`args` as a plain subprocess with piped stdout/stderr and
/// drives its output into `term`, mirroring what the Alacritty event loop does
/// for a PTY but without one. Used when [`HeadlessTerminal`] is enabled.
fn spawn_task_subprocess(
    program: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    working_directory: Option<PathBuf>,
    term: Arc<AlacrittyTermLock>,
    events_tx: futures::channel::mpsc::UnboundedSender<PtyEvent>,
    wakeup_gate: WakeupGate,
    executor: &BackgroundExecutor,
) -> Result<SubprocessHandle> {
    use futures::io::AsyncReadExt as _;
    use std::process::Stdio;

    let mut command = util::command::new_std_command(&program);
    command.args(&args);
    command.envs(&env);
    if let Some(directory) = &working_directory {
        command.current_dir(directory);
    }

    let mut child =
        util::process::Child::spawn(command, Stdio::null(), Stdio::piped(), Stdio::piped())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = Arc::new(parking_lot::Mutex::new(Some(child)));

    let reader = executor.spawn({
        let child = child.clone();
        let executor = executor.clone();
        async move {
            // stdout and stderr are pumped concurrently, each through its own
            // parser; the shared term mutex serializes grid mutation.
            type BoxedReader = Box<dyn futures::io::AsyncRead + Unpin + Send>;
            let pump = |reader: Option<BoxedReader>| {
                let term = term.clone();
                let events_tx = events_tx.clone();
                let wakeup_gate = wakeup_gate.clone();
                async move {
                    let Some(mut reader) = reader else { return };
                    let mut processor = Processor::<StdSyncHandler>::new();
                    let mut buffer = [0u8; 8192];
                    let mut previous_byte_was_cr = false;
                    loop {
                        match reader.read(&mut buffer).await {
                            Ok(0) => return,
                            Err(error) => {
                                log::warn!("failed to read subprocess output: {error}");
                                return;
                            }
                            Ok(count) => {
                                let converted =
                                    convert_lf_to_crlf(&buffer[..count], &mut previous_byte_was_cr);
                                {
                                    let mut term = term.lock();
                                    processor.advance(&mut *term, &converted);
                                }
                                if wakeup_gate.is_enabled() {
                                    events_tx
                                        .unbounded_send(PtyEvent::Event(
                                            TerminalBackendEvent::Wakeup,
                                        ))
                                        .ok();
                                }
                            }
                        }
                    }
                }
            };
            let stdout = stdout.map(|reader| Box::new(reader) as BoxedReader);
            let stderr = stderr.map(|reader| Box::new(reader) as BoxedReader);
            futures::future::join(pump(stdout), pump(stderr)).await;

            // Both pipes are closed, so the child has exited or is about to.
            // Poll for its status without holding the lock across an await.
            let status = loop {
                let status = match child.lock().as_mut() {
                    Some(child) => match child.try_status() {
                        Ok(status) => status,
                        Err(error) => {
                            log::warn!("failed to get subprocess exit status: {error}");
                            break None;
                        }
                    },
                    None => Some(ExitStatus::default()),
                };
                match status {
                    Some(status) => break Some(status),
                    None => executor.timer(Duration::from_millis(20)).await,
                }
            };
            child.lock().take();
            let event = match status {
                Some(status) => TerminalBackendEvent::ChildExit(status),
                None => TerminalBackendEvent::Exit,
            };
            events_tx.unbounded_send(PtyEvent::Event(event)).ok();
        }
    });

    Ok(SubprocessHandle {
        child,
        _reader: reader,
    })
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(mut byte_stream) = self.byte_stream.take() {
            byte_stream.stop();
        }
        if let Some(subprocess) = self.subprocess.take() {
            subprocess.kill();
        }
        let owns_child = self.owns_child();
        let child_already_exited = self.child_process_ended;
        if let TerminalType::Pty { pty_tx, info, .. } =
            std::mem::replace(&mut self.terminal_type, TerminalType::DisplayOnly)
        {
            // Capture the process groups before shutting the pty down, not after:
            // the foreground group is read from the pty master, and shutting the
            // loop down is what closes it. Nothing is captured for a child that
            // has already exited — its group ids are free to be reused, and
            // signalling a reused one would kill an unrelated process.
            let kill_processes = (owns_child && !child_already_exited).then(|| {
                terminate_processes_with_grace_period(info, self.background_executor.clone())
            });
            // Stop reading either way: this terminal is going away.
            if let Some(pty_tx) = pty_tx {
                pty_tx.shutdown();
            }
            if let Some(kill_processes) = kill_processes {
                self.background_executor.spawn(kill_processes).detach();
            }
        }
    }
}

impl EventEmitter<Event> for Terminal {}

fn normalize_path_command_name(command: &str) -> Option<String> {
    const MAX_COMMAND_NAME_LENGTH: usize = 64;

    let command = command.trim();
    if command.is_empty()
        || command.len() > MAX_COMMAND_NAME_LENGTH
        || command.starts_with('.')
        || command.starts_with('-')
        || command.contains('/')
        || command.contains('\\')
    {
        return None;
    }

    let mut command = command.to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1"] {
        if command.ends_with(suffix) {
            command.truncate(command.len() - suffix.len());
            break;
        }
    }

    if command.is_empty()
        || !command.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return None;
    }

    Some(command)
}

fn foreground_process_command_from_argv(argv: &[String]) -> Option<String> {
    let command = argv
        .first()
        .and_then(|command| normalize_path_command_name(command));

    if !matches!(
        command.as_deref(),
        Some("node" | "python" | "python3" | "bun" | "deno")
    ) {
        return command;
    }

    argv.iter()
        .skip(1)
        .filter_map(|argument| normalize_script_command_name(argument))
        .next()
        .or(command)
}

fn normalize_script_command_name(argument: &str) -> Option<String> {
    let path = Path::new(argument);
    let file_stem = path
        .file_stem()
        .and_then(|file_stem| file_stem.to_str())
        .and_then(normalize_path_command_name)?;

    if file_stem != "index" {
        return Some(file_stem);
    }

    path.parent()
        .and_then(|parent| parent.parent())
        .and_then(|package_path| package_path.file_name())
        .and_then(|package_name| package_name.to_str())
        .and_then(|package_name| package_name.strip_suffix("-cli").or(Some(package_name)))
        .and_then(normalize_path_command_name)
}

fn content_index_for_mouse(pos: GpuiPoint<Pixels>, terminal_bounds: &TerminalBounds) -> usize {
    let col = (pos.x / terminal_bounds.cell_width()).round() as usize;
    let clamped_col = min(col, terminal_bounds.num_columns().saturating_sub(1));
    let row = (pos.y / terminal_bounds.line_height()).round() as usize;
    let clamped_row = min(row, terminal_bounds.num_lines().saturating_sub(1));
    clamped_row * terminal_bounds.num_columns() + clamped_col
}

/// Converts an 8 bit ANSI color to its GPUI equivalent.
/// Accepts `usize` for compatibility with the `alacritty::Colors` interface,
/// Other than that use case, should only be called with values in the `[0,255]` range
pub fn get_color_at_index(index: usize, theme: &Theme) -> Hsla {
    let colors = theme.colors();

    match index {
        // 0-15 are the same as the named colors above
        0 => colors.terminal_ansi_black,
        1 => colors.terminal_ansi_red,
        2 => colors.terminal_ansi_green,
        3 => colors.terminal_ansi_yellow,
        4 => colors.terminal_ansi_blue,
        5 => colors.terminal_ansi_magenta,
        6 => colors.terminal_ansi_cyan,
        7 => colors.terminal_ansi_white,
        8 => colors.terminal_ansi_bright_black,
        9 => colors.terminal_ansi_bright_red,
        10 => colors.terminal_ansi_bright_green,
        11 => colors.terminal_ansi_bright_yellow,
        12 => colors.terminal_ansi_bright_blue,
        13 => colors.terminal_ansi_bright_magenta,
        14 => colors.terminal_ansi_bright_cyan,
        15 => colors.terminal_ansi_bright_white,
        // 16-231 are a 6x6x6 RGB color cube, mapped to 0-255 using steps defined by XTerm.
        // See: https://github.com/xterm-x11/xterm-snapshots/blob/master/256colres.pl
        16..=231 => {
            let (r, g, b) = rgb_for_index(index as u8);
            rgba_color(
                if r == 0 { 0 } else { r * 40 + 55 },
                if g == 0 { 0 } else { g * 40 + 55 },
                if b == 0 { 0 } else { b * 40 + 55 },
            )
        }
        // 232-255 are a 24-step grayscale ramp from (8, 8, 8) to (238, 238, 238).
        232..=255 => {
            let i = index as u8 - 232; // Align index to 0..24
            let value = i * 10 + 8;
            rgba_color(value, value, value)
        }
        // For compatibility with the alacritty::Colors interface
        // See: https://github.com/alacritty/alacritty/blob/master/alacritty_terminal/src/term/color.rs
        256 => colors.terminal_foreground,
        257 => colors.terminal_background,
        258 => theme.players().local().cursor,
        259 => colors.terminal_ansi_dim_black,
        260 => colors.terminal_ansi_dim_red,
        261 => colors.terminal_ansi_dim_green,
        262 => colors.terminal_ansi_dim_yellow,
        263 => colors.terminal_ansi_dim_blue,
        264 => colors.terminal_ansi_dim_magenta,
        265 => colors.terminal_ansi_dim_cyan,
        266 => colors.terminal_ansi_dim_white,
        267 => colors.terminal_bright_foreground,
        268 => colors.terminal_ansi_black, // 'Dim Background', non-standard color

        _ => black(),
    }
}

/// Builds the Win32 console palette from the same theme colors used for OSC
/// replies. The default foreground/background attributes select the closest
/// ANSI entry in OKLab, which tracks perceived color difference better than
/// distance in gamma-encoded RGB.
pub fn console_palette_for_theme(theme: &Theme) -> ConsolePalette {
    console_palette_from_colors(
        std::array::from_fn(|index| get_color_at_index(index, theme)),
        get_color_at_index(256, theme),
        get_color_at_index(257, theme),
    )
}

fn console_palette_from_colors(
    ansi: [Hsla; 16],
    foreground: Hsla,
    background: Hsla,
) -> ConsolePalette {
    let colors = ansi.map(color_bytes);
    let foreground = color_bytes(foreground);
    let background = color_bytes(background);
    ConsolePalette {
        colors,
        foreground_index: nearest_palette_index(foreground, &colors),
        background_index: nearest_palette_index(background, &colors),
    }
}

fn color_bytes(color: Hsla) -> [u8; 3] {
    let color = color.to_rgb();
    [
        ((color.r * color.a) * 255.) as u8,
        ((color.g * color.a) * 255.) as u8,
        ((color.b * color.a) * 255.) as u8,
    ]
}

fn nearest_palette_index(target: [u8; 3], colors: &[[u8; 3]; 16]) -> u8 {
    let target = rgb_to_oklab(target);
    colors
        .iter()
        .enumerate()
        .map(|(index, color)| {
            let color = rgb_to_oklab(*color);
            let distance = (target.0 - color.0).powi(2)
                + (target.1 - color.1).powi(2)
                + (target.2 - color.2).powi(2);
            (index as u8, distance)
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map_or(0, |(index, _)| index)
}

fn rgb_to_oklab([red, green, blue]: [u8; 3]) -> (f32, f32, f32) {
    fn linear(channel: u8) -> f32 {
        let channel = f32::from(channel) / 255.;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    let red = linear(red);
    let green = linear(green);
    let blue = linear(blue);
    let l = 0.412_221_46 * red + 0.536_332_55 * green + 0.051_445_995 * blue;
    let m = 0.211_903_5 * red + 0.680_699_5 * green + 0.107_396_96 * blue;
    let s = 0.088_302_46 * red + 0.281_718_85 * green + 0.629_978_7 * blue;
    let l = l.cbrt();
    let m = m.cbrt();
    let s = s.cbrt();
    (
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    )
}

/// Generates the RGB channels in [0, 5] for a given index into the 6x6x6 ANSI color cube.
///
/// See: [8 bit ANSI color](https://en.wikipedia.org/wiki/ANSI_escape_code#8-bit).
///
/// Wikipedia gives a formula for calculating the index for a given color:
///
/// ```text
/// index = 16 + 36 × r + 6 × g + b (0 ≤ r, g, b ≤ 5)
/// ```
///
/// This function does the reverse, calculating the `r`, `g`, and `b` components from a given index.
fn rgb_for_index(i: u8) -> (u8, u8, u8) {
    debug_assert!((16..=231).contains(&i));
    let i = i - 16;
    let r = (i - (i % 36)) / 36;
    let g = ((i % 36) - (i % 6)) / 6;
    let b = (i % 36) % 6;
    (r, g, b)
}

pub fn rgba_color(r: u8, g: u8, b: u8) -> Hsla {
    Rgba {
        r: (r as f32 / 255.),
        g: (g as f32 / 255.),
        b: (b as f32 / 255.),
        a: 1.,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        Cell, Content, IndexedCell, TerminalBounds, TerminalBuilder, content_index_for_mouse,
        rgb_for_index,
    };
    use async_channel::Receiver;
    use collections::HashMap;
    use gpui::MouseMoveEvent;
    use gpui::{
        ClipboardItem, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels,
        TestAppContext, VisualContext, bounds, point, size,
    };
    use parking_lot::Mutex;
    use rand::{Rng, distr, rngs::StdRng};
    use task::{Shell, ShellBuilder};

    #[cfg(unix)]
    struct NoopPtyControl;

    #[cfg(unix)]
    impl PtyControl for NoopPtyControl {
        fn resize(&self, _: u16, _: u16) {}
        fn set_console_palette(&self, _: ConsolePalette) {}
    }

    #[cfg(windows)]
    #[derive(Default)]
    struct RecordingPtyControl {
        palettes: std::sync::Mutex<Vec<ConsolePalette>>,
    }

    #[cfg(windows)]
    impl PtyControl for RecordingPtyControl {
        fn resize(&self, _: u16, _: u16) {}

        fn set_console_palette(&self, palette: ConsolePalette) {
            self.palettes.lock().unwrap().push(palette);
        }
    }

    fn make_display_only_terminal(cx: &mut TestAppContext) -> Terminal {
        TerminalBuilder::new_display_only(
            SettingsCursorShape::default(),
            AlternateScroll::On,
            None,
            0,
            &cx.background_executor,
            PathStyle::local(),
        )
        .terminal
    }

    #[gpui::test]
    fn cwd_at_line_without_history_falls_back_to_the_current_directory(cx: &mut TestAppContext) {
        let terminal = make_display_only_terminal(cx);
        assert_eq!(terminal.cwd_at_line(0, 0), None);
    }

    #[gpui::test]
    fn cwd_at_line_selects_the_directory_active_when_the_line_was_printed(cx: &mut TestAppContext) {
        let mut terminal = make_display_only_terminal(cx);
        let project_a = PathBuf::from("/home/user/project_a");
        let project_b = PathBuf::from("/home/user/project_b");
        let project_c = PathBuf::from("/home/user/project_c");
        for (scrollback_position, working_directory) in
            [(0, &project_a), (10, &project_b), (20, &project_c)]
        {
            terminal.cwd_history.push(CwdHistoryEntry {
                scrollback_position,
                working_directory: working_directory.clone(),
            });
        }

        assert_eq!(terminal.cwd_at_line(5, 0), Some(project_a));
        assert_eq!(terminal.cwd_at_line(15, 0), Some(project_b));
        assert_eq!(terminal.cwd_at_line(25, 0), Some(project_c));
    }

    #[gpui::test]
    fn cwd_at_line_falls_back_for_a_line_printed_before_any_recorded_directory(
        cx: &mut TestAppContext,
    ) {
        let mut terminal = make_display_only_terminal(cx);
        terminal.cwd_history.push(CwdHistoryEntry {
            scrollback_position: 10,
            working_directory: PathBuf::from("/home/user/project_a"),
        });

        assert_eq!(terminal.cwd_at_line(3, 0), None);
    }

    /// Once the scrollback is capped, evictions move retained lines without
    /// changing the history size, so the recorded offsets stop identifying the
    /// lines they were taken from.
    #[gpui::test]
    fn cwd_at_line_ignores_history_once_the_scrollback_cap_is_reached(cx: &mut TestAppContext) {
        let mut terminal = make_display_only_terminal(cx);
        terminal.term_config.scrolling_history = 10;
        terminal.cwd_history.push(CwdHistoryEntry {
            scrollback_position: 0,
            working_directory: PathBuf::from("/stale/cwd"),
        });

        assert_eq!(terminal.cwd_at_line(-5, 10), None);
    }

    #[gpui::test]
    fn record_cwd_change_attributes_the_move_to_the_command_that_caused_it(
        cx: &mut TestAppContext,
    ) {
        let mut terminal = make_display_only_terminal(cx);
        terminal.write_input(b"cd elsewhere\r".to_vec());
        assert_eq!(terminal.pending_cwd_boundary, Some(0));

        let working_directory = PathBuf::from("/tmp/elsewhere");
        terminal.record_cwd_change(working_directory.clone());

        assert_eq!(terminal.pending_cwd_boundary, None);
        assert_eq!(
            terminal.cwd_history,
            vec![CwdHistoryEntry {
                scrollback_position: 0,
                working_directory,
            }]
        );
    }

    #[gpui::test]
    fn record_cwd_change_ignores_an_unreadable_directory(cx: &mut TestAppContext) {
        let mut terminal = make_display_only_terminal(cx);
        terminal.record_cwd_change(PathBuf::new());
        assert!(terminal.cwd_history.is_empty());
    }

    #[gpui::test]
    fn remote_terminals_record_no_local_directory(cx: &mut TestAppContext) {
        let mut terminal = make_display_only_terminal(cx);
        terminal.is_remote_terminal = true;
        terminal.write_input(b"cd elsewhere\r".to_vec());
        terminal.record_cwd_change(PathBuf::from("/local/ssh/cwd"));

        assert_eq!(terminal.pending_cwd_boundary, None);
        assert!(terminal.cwd_history.is_empty());
        assert_eq!(terminal.cwd_at_line(0, 0), None);
    }

    #[gpui::test]
    fn vi_mode_visual_selection_types(cx: &mut TestAppContext) {
        let selection_type_for = |keystroke: &str| {
            let mut terminal = TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                &cx.background_executor,
                PathStyle::local(),
            )
            .terminal;
            terminal.vi_mode_enabled = true;
            terminal.vi_motion(&Keystroke::parse(keystroke).unwrap());
            terminal.events.iter().find_map(|event| match event {
                InternalEvent::SetSelection(Some(selection)) => Some(selection.ty),
                _ => None,
            })
        };

        assert_eq!(selection_type_for("v"), Some(SelectionType::Simple));
        assert_eq!(selection_type_for("shift-v"), Some(SelectionType::Lines));
    }

    #[cfg(windows)]
    #[gpui::test]
    fn console_palette_updates_are_fixed_to_a_handover_and_deduplicated(cx: &mut TestAppContext) {
        let first = Arc::new(RecordingPtyControl::default());
        let second = Arc::new(RecordingPtyControl::default());
        let make_terminal = |control: Arc<RecordingPtyControl>| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                &cx.background_executor,
                PathStyle::local(),
            )
            .with_pty_control(control)
            .terminal
        };
        let mut first_terminal = make_terminal(first.clone());
        let mut second_terminal = make_terminal(second.clone());
        let palette = ConsolePalette::default();

        first_terminal.set_console_palette(palette);
        first_terminal.set_console_palette(palette);
        second_terminal.set_console_palette(palette);

        assert_eq!(first.palettes.lock().unwrap().as_slice(), &[palette]);
        assert_eq!(second.palettes.lock().unwrap().as_slice(), &[palette]);
    }

    #[test]
    fn console_palette_preserves_ansi_order_and_exact_rgb_bytes() {
        let ansi = std::array::from_fn(|index| {
            rgba_color(
                index as u8,
                (index as u8).wrapping_add(16),
                255 - index as u8,
            )
        });
        let palette = console_palette_from_colors(ansi, ansi[12], ansi[2]);

        for (index, color) in palette.colors.iter().enumerate() {
            let expected = to_vte_rgb(ansi[index]);
            assert_eq!(*color, [expected.r, expected.g, expected.b]);
        }
        assert_eq!(palette.foreground_index, 12);
        assert_eq!(palette.background_index, 2);
    }

    #[test]
    fn console_palette_nearest_index_uses_oklab_and_lowest_tie() {
        let mut colors = [[0, 0, 0]; 16];
        colors[1] = [255, 0, 0];
        colors[2] = [255, 255, 255];
        assert_eq!(nearest_palette_index([245, 245, 245], &colors), 2);

        colors[4] = [31, 47, 63];
        colors[9] = colors[4];
        assert_eq!(nearest_palette_index(colors[4], &colors), 4);
    }

    #[test]
    fn terminal_sync_diagnostics_name_the_blocking_phase() {
        assert_eq!(
            terminal_sync_phase_name(TERMINAL_SYNC_WAITING_FOR_GRID),
            "waiting for the terminal grid lock"
        );
        assert_eq!(
            terminal_sync_phase_name(TERMINAL_SYNC_BUILDING_CONTENT),
            "building the renderable terminal snapshot"
        );
        assert_eq!(terminal_sync_phase_name(TERMINAL_SYNC_IDLE), "idle");
    }

    #[test]
    fn reported_working_directory_titles_require_safe_absolute_paths() {
        assert_eq!(
            reported_working_directory_from_title("zetta-cwd:/home/user/project"),
            Some("/home/user/project".to_owned())
        );
        assert_eq!(
            reported_working_directory_from_title("ordinary title"),
            None
        );
        assert_eq!(
            reported_working_directory_from_title("zetta-cwd:relative"),
            None
        );
        assert_eq!(
            reported_working_directory_from_title("zetta-cwd:/home/user\nproject"),
            None
        );
        #[cfg(windows)]
        {
            assert_eq!(
                reported_working_directory_from_title(r"zetta-cwd:C:\source\zetta"),
                Some(r"C:\source\zetta".to_owned())
            );
            assert_eq!(
                reported_working_directory_from_title(r"zetta-cwd:\\server\share\zetta"),
                Some(r"\\server\share\zetta".to_owned())
            );
        }
    }

    #[gpui::test]
    fn restored_working_directory_is_available_until_live_metadata_replaces_it(
        cx: &mut TestAppContext,
    ) {
        let restored = PathBuf::from(if cfg!(windows) {
            r"C:\saved\project"
        } else {
            "/saved/project"
        });
        let live = if cfg!(windows) {
            r"C:\live\project"
        } else {
            "/live/project"
        };
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .with_working_directory(Some(restored.clone()))
        });
        let terminal = cx.new(|cx| builder.subscribe(cx));

        assert_eq!(
            terminal.read_with(cx, |terminal, _| terminal.working_directory()),
            Some(restored)
        );
        terminal.update(cx, |terminal, _| {
            terminal.reported_working_directory = Some(live.to_owned());
        });
        assert_eq!(
            terminal.read_with(cx, |terminal, _| terminal.working_directory()),
            Some(PathBuf::from(live))
        );
    }

    #[gpui::test]
    fn reported_working_directory_changes_emit_title_events(cx: &mut TestAppContext) {
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
        });
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let (events_tx, events_rx) = async_channel::unbounded();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, event: &Event, _| {
                events_tx.send_blocking(event.clone()).unwrap();
            })
        })
        .detach();

        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                TerminalBackendEvent::Title("zetta-cwd:/tmp/project".to_owned()),
                cx,
            );
        });
        assert_eq!(events_rx.try_recv().unwrap(), Event::TitleChanged);

        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                TerminalBackendEvent::Title("zetta-cwd:/tmp/project".to_owned()),
                cx,
            );
        });
        assert!(events_rx.try_recv().is_err());

        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                TerminalBackendEvent::Title("zetta-cwd:/tmp/other".to_owned()),
                cx,
            );
        });
        assert_eq!(events_rx.try_recv().unwrap(), Event::TitleChanged);
    }

    #[test]
    fn reported_foreground_command_titles_are_parsed_from_the_marker() {
        assert_eq!(
            reported_foreground_command_from_title("zetta-cmd:npm run build"),
            Some("npm run build".to_owned())
        );
        assert_eq!(
            reported_foreground_command_from_title("zetta-cmd:bash"),
            Some("bash".to_owned())
        );
        assert_eq!(
            reported_foreground_command_from_title("ordinary title"),
            None
        );
        assert_eq!(
            reported_foreground_command_from_title("zetta-cmd:with\ncontrol"),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn foreground_exit_diagnostics_keep_only_a_safe_command_name() {
        assert_eq!(
            reported_foreground_command_name(r"C:\\Tools\\htop.exe --secret value"),
            Some("htop".to_owned())
        );
        assert_eq!(
            reported_foreground_command_name("exit 1"),
            Some("exit".to_owned())
        );
        assert_eq!(
            reported_foreground_command_name("./local-agent secret=value"),
            None
        );
    }

    fn exit_observation(
        source: TerminalExitSource,
        input_sent: bool,
        foreground_is_shell: Option<bool>,
    ) -> TerminalExited {
        TerminalExited {
            exit_code: Some(1),
            source,
            child_pid: Some(42),
            input_sent,
            foreground_is_shell,
            foreground_command: Some("htop".to_owned()),
        }
    }

    #[test]
    fn terminal_exit_classification_preserves_unknown_foreground_state() {
        let user_exit = exit_observation(TerminalExitSource::Child, true, None);
        assert!(!user_exit.is_unexpected());

        let shell_builtin_exit = TerminalExited {
            foreground_command: Some("exit".to_owned()),
            foreground_is_shell: Some(false),
            ..user_exit.clone()
        };
        assert!(!shell_builtin_exit.is_unexpected());

        let startup_failure = exit_observation(TerminalExitSource::Child, false, None);
        assert_eq!(
            startup_failure.unexpected_reason(),
            Some(TerminalExitReason::ExitedBeforeInput)
        );

        let foreground_failure = exit_observation(TerminalExitSource::Child, true, Some(false));
        assert_eq!(
            foreground_failure.unexpected_reason(),
            Some(TerminalExitReason::ForegroundCommand)
        );

        let clean_exit_with_stale_foreground = TerminalExited {
            exit_code: Some(0),
            ..foreground_failure.clone()
        };
        assert!(
            !clean_exit_with_stale_foreground.is_unexpected(),
            "a clean exit is never unexpected, even with stale foreground metadata"
        );

        let clean_exit_before_input = TerminalExited {
            exit_code: Some(0),
            input_sent: false,
            ..foreground_failure
        };
        assert!(
            !clean_exit_before_input.is_unexpected(),
            "a clean exit is never unexpected, even before any input was sent"
        );

        let unavailable = exit_observation(TerminalExitSource::StatusUnavailable, true, Some(true));
        assert_eq!(
            unavailable.unexpected_reason(),
            Some(TerminalExitReason::StatusUnavailable)
        );

        let unknown_status = TerminalExited {
            exit_code: None,
            ..exit_observation(TerminalExitSource::Child, true, Some(true))
        };
        assert_eq!(
            unknown_status.unexpected_reason(),
            Some(TerminalExitReason::StatusUnavailable)
        );

        for source in [
            TerminalExitSource::WatcherDisconnected,
            TerminalExitSource::BackendShutdown,
        ] {
            assert!(exit_observation(source, true, Some(true)).is_unexpected());
        }
    }

    #[gpui::test]
    fn terminal_exit_events_are_one_shot(cx: &mut TestAppContext) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        let (events_tx, events_rx) = async_channel::unbounded();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, event: &Event, _| {
                events_tx.send_blocking(event.clone()).unwrap();
            })
        })
        .detach();

        terminal.update(cx, |terminal, cx| {
            terminal.keyboard_input_sent = true;
            terminal.process_event(TerminalBackendEvent::ChildExit(successful_exit()), cx);
            // The event loop emits a final Exit notification after ChildExit.
            terminal.process_event(TerminalBackendEvent::Exit, cx);
        });

        let events = std::iter::from_fn(|| events_rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::TerminalExited(_)))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::CloseTerminal))
                .count(),
            1
        );
    }

    /// The pane a multiplexer-backed terminal shows when contact with the
    /// multiplexer is lost, rather than when its process ends.
    ///
    /// This has to be a *pty* terminal, which is the case the bug was in:
    /// suppressing the disconnect for a pty left the event loop's final `Exit`
    /// to classify the pane instead, as `StatusUnavailable` — "the shell exited
    /// but its status was unavailable" — for a shell that was running fine.
    #[cfg(unix)]
    #[gpui::test]
    async fn losing_the_multiplexer_is_reported_as_that_and_not_as_a_missing_status(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        cx.executor().allow_parking();

        let shell = (
            "/bin/sh".to_owned(),
            vec!["-c".to_owned(), "printf 'ready\n'; sleep 300".to_owned()],
        );
        let options = pty_options(
            Some(shell),
            None,
            std::iter::empty::<(String, String)>(),
            None,
        );
        let pty = alacritty_terminal::tty::new(
            &options,
            alacritty_terminal::event::WindowSize {
                num_lines: 24,
                num_cols: 80,
                cell_width: 1,
                cell_height: 1,
            },
            0,
        )
        .unwrap();
        let child_pid = pty.child_pid();
        let attached = TerminalBuilder::new_attached(
            PtyHandover {
                descriptor: pty.file().try_clone().unwrap().into(),
                child_pid,
                replay: Vec::new(),
                control: Arc::new(NoopPtyControl),
            },
            AttachedOptions {
                shell: Shell::System,
                env: HashMap::default(),
                cursor_shape: SettingsCursorShape::default(),
                alternate_scroll: AlternateScroll::On,
                max_scroll_history_lines: None,
                path_hyperlink_regexes: Vec::new(),
                path_hyperlink_timeout_ms: 0,
                window_id: 0,
            },
            &cx.background_executor,
            PathStyle::local(),
        )
        .unwrap();
        let terminal = cx.new(|cx| attached.builder.subscribe(cx));
        assert_content_eventually(&terminal, "ready", cx).await;

        let (events_tx, events_rx) = async_channel::unbounded();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, event: &Event, _| {
                events_tx.send_blocking(event.clone()).unwrap();
            })
        })
        .detach();

        terminal.update(cx, |terminal, cx| {
            assert!(
                matches!(terminal.terminal_type, TerminalType::Pty { .. }),
                "this has to be the pty case, which is where the bug was"
            );
            terminal.keyboard_input_sent = true;
            terminal.process_event(TerminalBackendEvent::ChildWatcherDisconnected, cx);
            // The event loop stops on a child event, so its final notification
            // arrives right behind this one.
            terminal.process_event(TerminalBackendEvent::Exit, cx);
        });

        let events = std::iter::from_fn(|| events_rx.try_recv().ok()).collect::<Vec<_>>();
        let exits = events
            .iter()
            .filter_map(|event| match event {
                Event::TerminalExited(exit) => Some(exit),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(exits.len(), 1, "one exit observation, whichever came first");
        assert_eq!(
            exits[0].source,
            TerminalExitSource::WatcherDisconnected,
            "losing contact with the multiplexer must not be reported as the process ending"
        );
        assert_eq!(
            exits[0].unexpected_reason(),
            Some(TerminalExitReason::WatcherDisconnected)
        );
        // Retained rather than closed, so the pane can say what happened.
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::CloseTerminal))
        );

        // The process is untouched by any of this, which is the point.
        assert!(unsafe { libc::kill(child_pid as libc::pid_t, 0) } == 0);
        unsafe { libc::kill(-(child_pid as libc::pid_t), libc::SIGKILL) };
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn a_pty_terminal_converts_to_a_byte_stream_keeping_its_grid(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        // Spawn a pty and adopt it the way the multiplexer handover does: an
        // attached terminal's process outlives the terminal by design, which
        // is exactly the property the conversion must preserve.
        let shell = (
            "/bin/sh".to_owned(),
            vec!["-c".to_owned(), "printf 'before\n'; sleep 300".to_owned()],
        );
        let options = pty_options(
            Some(shell),
            None,
            std::iter::empty::<(String, String)>(),
            None,
        );
        let pty = alacritty_terminal::tty::new(
            &options,
            alacritty_terminal::event::WindowSize {
                num_lines: 24,
                num_cols: 80,
                cell_width: 1,
                cell_height: 1,
            },
            0,
        )
        .unwrap();
        let child_pid = pty.child_pid();
        let descriptor = pty.file().try_clone().unwrap().into();
        let handover = PtyHandover {
            descriptor,
            child_pid,
            replay: Vec::new(),
            control: Arc::new(NoopPtyControl),
        };
        let attached = TerminalBuilder::new_attached(
            handover,
            AttachedOptions {
                shell: Shell::System,
                env: HashMap::default(),
                cursor_shape: SettingsCursorShape::default(),
                alternate_scroll: AlternateScroll::On,
                max_scroll_history_lines: None,
                path_hyperlink_regexes: Vec::new(),
                path_hyperlink_timeout_ms: 0,
                window_id: 0,
            },
            &cx.background_executor,
            PathStyle::local(),
        )
        .unwrap();
        let terminal = cx.new(|cx| attached.builder.subscribe(cx));
        assert_content_eventually(&terminal, "before", cx).await;

        // Stop the pty loop and reconnect the terminal to a canned stream, the
        // way the multiplexer handover does. The loop must actually end, not
        // just be asked to.
        let reader: Box<dyn Read + Send> = Box::new(CannedReader {
            bytes: b"after\n".to_vec(),
        });
        let writer: Box<dyn Write + Send> = Box::new(std::io::sink());
        terminal.update(cx, |terminal, _| {
            terminal.stop_pty_loop().unwrap();
            terminal.attach_byte_stream(reader, writer).unwrap();
        });

        assert_content_eventually(&terminal, "after", cx).await;
        let content = terminal.update(cx, |terminal, _| terminal.get_content());
        assert!(
            content.contains("before"),
            "the grid must survive the conversion: {content}"
        );

        // Stopping the loop must not have killed the process the pty served.
        let alive = unsafe { libc::kill(child_pid as libc::pid_t, 0) } == 0;
        assert!(
            alive,
            "the pty's child died when its event loop was stopped"
        );
        // The shell was spawned as a session leader, so its group covers the
        // `sleep` it forked. Reap it so the test does not leave processes
        // behind; the attached pty in the test is deliberately not dropped.
        unsafe { libc::killpg(child_pid as libc::pid_t, libc::SIGKILL) };
        unsafe { libc::waitpid(child_pid as libc::pid_t, std::ptr::null_mut(), 0) };
    }

    /// A writer that records what a terminal sends, so a test can tell whether
    /// input reached the byte stream or vanished into a stopped pty loop.
    #[cfg(unix)]
    struct RecordingWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    #[cfg(unix)]
    impl std::io::Write for RecordingWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A pane revoked into shared mode keeps typing into the multiplexer.
    ///
    /// The conversion leaves `TerminalType::Pty` in place and only shuts the
    /// loop down, so a write path that checks the terminal type before the byte
    /// stream posts every keystroke to a channel nobody reads. The window that
    /// was revoked went mute while the one that joined afterwards worked, which
    /// is what "only the last client can type" was.
    #[cfg(unix)]
    #[gpui::test]
    async fn a_converted_terminals_input_reaches_the_byte_stream(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let shell = (
            "/bin/sh".to_owned(),
            vec!["-c".to_owned(), "printf 'before\n'; sleep 300".to_owned()],
        );
        let options = pty_options(
            Some(shell),
            None,
            std::iter::empty::<(String, String)>(),
            None,
        );
        let pty = alacritty_terminal::tty::new(
            &options,
            alacritty_terminal::event::WindowSize {
                num_lines: 24,
                num_cols: 80,
                cell_width: 1,
                cell_height: 1,
            },
            0,
        )
        .unwrap();
        let child_pid = pty.child_pid();
        let descriptor = pty.file().try_clone().unwrap().into();
        let attached = TerminalBuilder::new_attached(
            PtyHandover {
                descriptor,
                child_pid,
                replay: Vec::new(),
                control: Arc::new(NoopPtyControl),
            },
            AttachedOptions {
                shell: Shell::System,
                env: HashMap::default(),
                cursor_shape: SettingsCursorShape::default(),
                alternate_scroll: AlternateScroll::On,
                max_scroll_history_lines: None,
                path_hyperlink_regexes: Vec::new(),
                path_hyperlink_timeout_ms: 0,
                window_id: 0,
            },
            &cx.background_executor,
            PathStyle::local(),
        )
        .unwrap();
        let terminal = cx.new(|cx| attached.builder.subscribe(cx));
        assert_content_eventually(&terminal, "before", cx).await;

        let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        terminal.update(cx, |terminal, _| {
            terminal
                .attach_byte_stream(
                    Box::new(CannedReader { bytes: Vec::new() }),
                    Box::new(RecordingWriter(written.clone())),
                )
                .unwrap();
        });
        terminal.update(cx, |terminal, _| terminal.input(b"typed\n".to_vec()));

        // The byte stream's writer is its own thread, so the write lands
        // shortly after `input` returns rather than during it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while written.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            written.lock().unwrap().as_slice(),
            b"typed\n",
            "a converted terminal's input must reach the multiplexer, not the stopped pty loop"
        );

        unsafe { libc::kill(-(child_pid as libc::pid_t), libc::SIGKILL) };
    }

    /// A restored screen is written once, and not until the pane has a real size.
    ///
    /// A grid starts at its placeholder size, so writing the screen into it wraps
    /// every line at the wrong width and loses all but the last few rows. That is
    /// what `with_replay` defers for — and it is why a caller must not *also* feed
    /// the replay through the stream: doing both drew the screen twice, the first
    /// time into the placeholder, leaving stray rows at the top that a full-screen
    /// program never repaints because it does not believe they changed.
    #[gpui::test]
    async fn a_restored_screen_is_replayed_once_and_only_after_layout(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        // A line long enough to wrap at the placeholder's 100 columns but not at
        // the 200 the pane is laid out at, so a replay written too early is
        // visible as two lines rather than one.
        let restored = format!("restored-screen{}\r\n", "-".repeat(140));
        let builder = cx.update(|cx| {
            TerminalBuilder::new_byte_stream(
                Box::new(CannedReader { bytes: Vec::new() }),
                Box::new(std::io::sink()),
                String::new(),
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .with_replay(restored.as_bytes().to_vec())
        });
        let window = cx.add_empty_window();
        let terminal = window.new(|cx| builder.subscribe(cx));
        window.run_until_parked();
        assert!(
            !terminal
                .update(window, |terminal, _| terminal.get_content())
                .contains("restored-screen"),
            "the screen must wait for a real size rather than be wrapped at the placeholder's"
        );

        window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.set_size(TerminalBounds::new(
                Pixels::from(10.),
                Pixels::from(8.),
                bounds(
                    GpuiPoint::default(),
                    size(Pixels::from(200. * 8.), Pixels::from(24. * 10.)),
                ),
            ));
            terminal.sync(window, cx);
        });
        window.run_until_parked();
        let content = terminal.update(window, |terminal, _| terminal.get_content());
        assert!(
            content.contains(restored.trim_end()),
            "the screen must be replayed at the laid-out width, unwrapped: {content:?}"
        );
        assert_eq!(
            content.matches("restored-screen").count(),
            1,
            "the screen must be written exactly once: {content:?}"
        );
        assert_eq!(
            content.matches("restored-screen").count(),
            1,
            "the screen must be replayed exactly once: {content:?}"
        );
    }

    /// A pane the multiplexer handed over outlives the terminal showing it.
    ///
    /// Dropping a terminal ends the processes it started, which is right for a
    /// pane this window opened and wrong for one it merely attached: the daemon
    /// forked that child and is still its parent. Backgrounding a session from
    /// the window that attached it is a drop, so getting this wrong killed the
    /// session instead of handing it back — as did quitting that window.
    #[cfg(unix)]
    #[gpui::test]
    async fn an_attached_pane_leaves_its_child_running(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        // Stands in for the multiplexer's pty: a child this process forked and
        // is still the parent of, whose descriptor is handed over.
        let options = pty_options(
            Some((
                "/bin/sh".to_owned(),
                vec!["-c".to_owned(), "exec sleep 120".to_owned()],
            )),
            None,
            std::iter::empty::<(String, String)>(),
            None,
        );
        let pty = alacritty_terminal::tty::new(
            &options,
            alacritty_terminal::event::WindowSize {
                num_lines: 24,
                num_cols: 80,
                cell_width: 1,
                cell_height: 1,
            },
            0,
        )
        .unwrap();
        let child_pid = pty.child_pid();
        let handover = PtyHandover {
            descriptor: pty.file().try_clone().unwrap().into(),
            child_pid,
            replay: Vec::new(),
            control: Arc::new(NoopPtyControl),
        };
        let attached = TerminalBuilder::new_attached(
            handover,
            AttachedOptions {
                shell: Shell::System,
                env: HashMap::default(),
                cursor_shape: SettingsCursorShape::default(),
                alternate_scroll: AlternateScroll::On,
                max_scroll_history_lines: None,
                path_hyperlink_regexes: Vec::new(),
                path_hyperlink_timeout_ms: 0,
                window_id: 0,
            },
            &cx.executor(),
            PathStyle::local(),
        )
        .expect("attaching the multiplexer's terminal");
        let _child_events = attached.child_events;
        let terminal = cx.new(|cx| attached.builder.subscribe(cx));

        // Detaching the session is exactly this: the tab goes away and its
        // terminals go with it.
        let weak = terminal.downgrade();
        drop(terminal);
        // The value itself is dropped by GPUI's release pass, which runs when
        // effects are flushed — not merely when the last handle goes.
        cx.update(|_| {});
        cx.run_until_parked();
        assert!(weak.upgrade().is_none(), "the terminal must be released");
        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut status = 0;
        let reaped = unsafe {
            libc::waitpid(
                child_pid as libc::pid_t,
                &mut status,
                libc::WNOHANG | libc::WUNTRACED,
            )
        };
        assert_eq!(
            reaped, 0,
            "the multiplexer's child must still be running: waitpid reported {reaped} \
             (status {status})"
        );

        unsafe { libc::kill(-(child_pid as libc::pid_t), libc::SIGKILL) };
    }

    /// A pane handed back by the multiplexer reads its own terminal again.
    ///
    /// The reverse of `attach_byte_stream`, and it has to work from either shape a
    /// shared pane can be in: one that was revoked into sharing and kept its pty
    /// type, and one that was built as a byte stream and never had one. Both end up
    /// here, so the conversion installs the pty backend wholesale rather than
    /// repairing whatever was there.
    #[cfg(unix)]
    #[gpui::test]
    async fn a_pane_handed_back_reads_its_own_terminal(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        // A byte-stream terminal, as a window that *joined* a shared session has.
        let relayed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_byte_stream(
                Box::new(CannedReader {
                    bytes: b"before-the-handover\r\n".to_vec(),
                }),
                Box::new(RecordingWriter(relayed.clone())),
                String::new(),
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                &cx.background_executor().clone(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        assert_content_eventually(&terminal, "before-the-handover", cx).await;
        assert!(
            terminal.read_with(cx, |terminal, _| matches!(
                terminal.terminal_type,
                TerminalType::DisplayOnly
            )),
            "the joined shape is display-only, which is what makes this the hard direction"
        );

        // What the multiplexer hands back: a live pty running a real program.
        let shell = (
            "/bin/sh".to_owned(),
            vec!["-c".to_owned(), "exec cat".to_owned()],
        );
        let options = pty_options(
            Some(shell),
            None,
            std::iter::empty::<(String, String)>(),
            None,
        );
        let pty = alacritty_terminal::tty::new(
            &options,
            alacritty_terminal::event::WindowSize {
                num_lines: 24,
                num_cols: 80,
                cell_width: 1,
                cell_height: 1,
            },
            0,
        )
        .unwrap();
        let child_pid = pty.child_pid();
        let handover = PtyHandover {
            descriptor: pty.file().try_clone().unwrap().into(),
            child_pid,
            replay: Vec::new(),
            control: Arc::new(NoopPtyControl),
        };
        // Held for as long as the pane is: it is the channel the multiplexer's
        // exit report arrives on, and dropping it tells the event loop its watcher
        // has gone — which stops the loop dead, so the pane reads nothing at all.
        let _child_events = terminal.update(cx, |terminal, cx| {
            terminal
                .attach_pty(
                    handover,
                    AttachedOptions {
                        shell: Shell::System,
                        env: HashMap::default(),
                        cursor_shape: SettingsCursorShape::default(),
                        alternate_scroll: AlternateScroll::On,
                        max_scroll_history_lines: None,
                        path_hyperlink_regexes: Vec::new(),
                        path_hyperlink_timeout_ms: 0,
                        window_id: 0,
                    },
                    cx,
                )
                .expect("taking the pane back")
        });

        // The grid survives: this is the same pane, not a fresh one, and nothing
        // was written into it on the way. A retired stream that reported itself as
        // a failure printed a connection error here, which shifted a full-screen
        // program's display by the lines it took.
        let content = terminal.update(cx, |terminal, _| terminal.get_content());
        assert!(
            content.contains("before-the-handover"),
            "the grid must survive the conversion: {content}"
        );
        assert!(
            !content.contains("Connection error"),
            "retiring the stream must write nothing into the grid: {content}"
        );
        // And input now goes to the pty rather than the retired relay, which `cat`
        // echoing it proves from the far side.
        terminal.update(cx, |terminal, _| {
            terminal.input(b"after-the-handover\n".to_vec())
        });
        assert_content_eventually(&terminal, "after-the-handover", cx).await;
        assert!(
            relayed.lock().unwrap().is_empty(),
            "input must go to the terminal, not to the relay that was retired"
        );

        // And the pane handed back is still the multiplexer's: closing the tab
        // it is in has to leave the session running, exactly as it would have
        // before the hand-back.
        drop(terminal);
        cx.update(|_| {});
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut status = 0;
        let reaped = unsafe { libc::waitpid(child_pid as libc::pid_t, &mut status, libc::WNOHANG) };
        assert_eq!(
            reaped, 0,
            "a handed-back child belongs to the multiplexer: waitpid reported {reaped}"
        );

        unsafe { libc::kill(-(child_pid as libc::pid_t), libc::SIGKILL) };
    }

    /// A shared pane closes when its shell exits, like any other.
    ///
    /// Only the multiplexer is the child's parent, so a shared pane learns of
    /// the exit from a report rather than from its own event loop. If that
    /// report does not end in `CloseTerminal` the pane simply stays there, with
    /// no shell behind it and nothing to say so.
    #[cfg(unix)]
    #[gpui::test]
    async fn a_shared_panes_reported_exit_closes_it(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let shell = (
            "/bin/sh".to_owned(),
            vec!["-c".to_owned(), "printf 'ready\n'; sleep 300".to_owned()],
        );
        let options = pty_options(
            Some(shell),
            None,
            std::iter::empty::<(String, String)>(),
            None,
        );
        let pty = alacritty_terminal::tty::new(
            &options,
            alacritty_terminal::event::WindowSize {
                num_lines: 24,
                num_cols: 80,
                cell_width: 1,
                cell_height: 1,
            },
            0,
        )
        .unwrap();
        let child_pid = pty.child_pid();
        let attached = TerminalBuilder::new_attached(
            PtyHandover {
                descriptor: pty.file().try_clone().unwrap().into(),
                child_pid,
                replay: Vec::new(),
                control: Arc::new(NoopPtyControl),
            },
            AttachedOptions {
                shell: Shell::System,
                env: HashMap::default(),
                cursor_shape: SettingsCursorShape::default(),
                alternate_scroll: AlternateScroll::On,
                max_scroll_history_lines: None,
                path_hyperlink_regexes: Vec::new(),
                path_hyperlink_timeout_ms: 0,
                window_id: 0,
            },
            &cx.background_executor,
            PathStyle::local(),
        )
        .unwrap();
        let terminal = cx.new(|cx| attached.builder.subscribe(cx));
        assert_content_eventually(&terminal, "ready", cx).await;

        // Converted the way the revoke handover converts it.
        terminal.update(cx, |terminal, _| {
            terminal
                .attach_byte_stream(
                    Box::new(CannedReader { bytes: Vec::new() }),
                    Box::new(std::io::sink()),
                )
                .unwrap();
        });

        let (events_tx, events_rx) = async_channel::unbounded();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, event: &Event, _| {
                events_tx.send_blocking(event.clone()).unwrap();
            })
        })
        .detach();

        // What the multiplexer reports for a shell the user typed `exit` into.
        terminal.update(cx, |terminal, cx| {
            terminal.report_child_exit(std::process::ExitStatus::from_raw(0), true, cx)
        });
        cx.run_until_parked();

        let events = std::iter::from_fn(|| events_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::TerminalExited(_))),
            "the exit must be observed: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::CloseTerminal)),
            "a clean exit must close the pane rather than leaving it hanging: {events:?}"
        );

        unsafe { libc::kill(-(child_pid as libc::pid_t), libc::SIGKILL) };
    }

    /// A shared pane whose exit the multiplexer could not put a status to is
    /// still told that it ended.
    ///
    /// Only the multiplexer can observe the status, and it observes it once. A
    /// client that is handed "it ended, I cannot say how" has no other route
    /// back to that fact, so dropping the report — as the shared path did — left
    /// the terminal waiting for something that had already happened. Retained
    /// with a reason rather than closed, because an exit nobody can describe is
    /// exactly what the unexpected-exit pane exists to show.
    #[cfg(unix)]
    #[gpui::test]
    async fn a_shared_pane_told_only_that_its_child_ended_stops_waiting(cx: &mut TestAppContext) {
        init_test(cx);
        cx.executor().allow_parking();

        let terminal = cx.new(|cx| {
            TerminalBuilder::new_byte_stream(
                Box::new(CannedReader { bytes: Vec::new() }),
                Box::new(std::io::sink()),
                String::new(),
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                &cx.background_executor().clone(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        let (events_tx, events_rx) = async_channel::unbounded();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, event: &Event, _| {
                events_tx.send_blocking(event.clone()).unwrap();
            })
        })
        .detach();

        terminal.update(cx, |terminal, cx| {
            terminal.report_child_exit_status_unavailable(true, cx)
        });
        cx.run_until_parked();

        let events = std::iter::from_fn(|| events_rx.try_recv().ok()).collect::<Vec<_>>();
        let exit = events
            .iter()
            .find_map(|event| match event {
                Event::TerminalExited(exit) => Some(exit),
                _ => None,
            })
            .expect("the pane must be told its process ended");
        assert_eq!(
            exit.unexpected_reason(),
            Some(TerminalExitReason::StatusUnavailable),
            "an exit nobody could describe has to say so, not close silently"
        );
    }

    /// A reader that yields its payload once and then ends the stream.
    struct CannedReader {
        bytes: Vec<u8>,
    }

    impl std::io::Read for CannedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.bytes.is_empty() {
                return Ok(0);
            }
            let n = self.bytes.len().min(buffer.len());
            buffer[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes.drain(..n);
            Ok(n)
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_shells_install_cwd_reporting() {
        let mut arguments = None;
        let mut environment = HashMap::default();
        install_windows_cwd_tracking("pwsh.exe", &mut arguments, &mut environment);

        assert!(environment.is_empty());
        let arguments = arguments.unwrap();
        assert_eq!(arguments[..2], ["-NoExit", "-Command"]);
        assert_eq!(arguments[2], POWERSHELL_CWD_TRACKER);
        assert!(POWERSHELL_CWD_TRACKER.contains("CurrentFileSystemLocation.ProviderPath"));
        assert!(POWERSHELL_CWD_TRACKER.contains("__ZettaCwdTrackerInstalled"));
        assert!(POWERSHELL_CWD_TRACKER.contains("__ZettaOriginalPrompt"));
        assert!(POWERSHELL_CWD_TRACKER.contains("$([char]27)[0m"));
        assert!(!POWERSHELL_CWD_TRACKER.contains("-NoProfile"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_preserves_existing_no_exit_argument() {
        for program in ["powershell.exe", "pwsh.exe"] {
            let mut arguments = Some(vec!["-NoLogo".to_owned(), "-NoExit".to_owned()]);
            let mut environment = HashMap::default();

            install_windows_cwd_tracking(program, &mut arguments, &mut environment);

            let arguments = arguments.unwrap();
            assert_eq!(
                arguments,
                ["-NoLogo", "-NoExit", "-Command", POWERSHELL_CWD_TRACKER],
                "{program}"
            );
            assert_eq!(
                arguments
                    .iter()
                    .filter(|argument| argument.eq_ignore_ascii_case("-NoExit"))
                    .count(),
                1,
                "{program}"
            );
        }
    }

    #[test]
    fn powershell_cwd_tracker_is_hidden_from_process_metadata() {
        let argv = vec![
            "pwsh.exe".to_owned(),
            "-NoLogo".to_owned(),
            "-NoExit".to_owned(),
            "-Command".to_owned(),
            POWERSHELL_CWD_TRACKER.to_owned(),
        ];

        assert_eq!(visible_process_argv(&argv), ["pwsh.exe", "-NoLogo"]);
    }

    #[test]
    fn user_powershell_commands_remain_visible_in_process_metadata() {
        let argv = vec![
            "pwsh.exe".to_owned(),
            "-NoExit".to_owned(),
            "-Command".to_owned(),
            "Get-Date".to_owned(),
        ];

        assert_eq!(visible_process_argv(&argv), argv);
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_commands_are_not_rewritten() {
        for program in ["powershell.exe", "pwsh.exe"] {
            for user_arguments in [
                vec!["-NoExit", "-Command", "Get-Date"],
                vec!["-NoLogo", "-File", "startup.ps1", "argument"],
            ] {
                let mut arguments = Some(
                    user_arguments
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<Vec<_>>(),
                );
                let original = arguments.clone();
                let mut environment = HashMap::default();

                install_windows_cwd_tracking(program, &mut arguments, &mut environment);
                assert_eq!(arguments, original, "{program}");
                assert!(environment.is_empty());
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_prompt_reports_its_dynamic_path_before_the_user_prompt() {
        let mut arguments = None;
        let mut environment = HashMap::default();
        environment.insert("PROMPT".to_owned(), "$N$G".to_owned());
        install_windows_cwd_tracking("cmd.exe", &mut arguments, &mut environment);

        assert_eq!(arguments, None);
        assert_eq!(
            environment.get("PROMPT").map(String::as_str),
            Some("$E]2;zetta-cwd:$P$E\\$N$G")
        );
        assert_eq!(
            cmd_prompt_with_cwd_tracking(None),
            "$E]2;zetta-cwd:$P$E\\$P$G"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_tracker_is_idempotent_with_profile_integration() {
        const POWERSHELL_INTEGRATION: &str =
            include_str!("../../../src/shell_integration/powershell.ps1");

        let profile_tracker = POWERSHELL_INTEGRATION
            .split_once("\n\nif (-not (Test-Path Env:EDITOR))")
            .expect("the profile tracker must remain the integration preamble")
            .0;
        let directory = std::env::current_dir().unwrap().join("src");
        let escaped_directory = directory.to_string_lossy().replace('\'', "''");
        let expected_marker = format!("\u{1b}]2;zetta-cwd:{}\u{1b}\\", directory.display());

        for program in ["powershell.exe", "pwsh.exe"] {
            if std::process::Command::new(program)
                .args(["-NoLogo", "-NoProfile", "-Command", "exit"])
                .output()
                .is_err()
            {
                continue;
            }

            for (profile_name, profile) in [("absent", ""), ("present", profile_tracker)] {
                let script = format!(
                    "function global:prompt {{ 'original-profile-prompt' }}\n{profile}\n{POWERSHELL_CWD_TRACKER}\nSet-Location -LiteralPath '{escaped_directory}'\nprompt"
                );
                let output = std::process::Command::new(program)
                    .args(["-NoLogo", "-NoProfile", "-Command", &script])
                    .output()
                    .unwrap();

                assert!(
                    output.status.success(),
                    "{program} with profile {profile_name} failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert_eq!(
                    stdout.matches(&expected_marker).count(),
                    1,
                    "{program} with profile {profile_name} emitted the wrong marker count: {stdout:?}"
                );
                assert_eq!(
                    stdout.matches("original-profile-prompt").count(),
                    1,
                    "{program} with profile {profile_name} did not preserve the original prompt: {stdout:?}"
                );
                let expected_prompt = format!("{expected_marker}\u{1b}[0moriginal-profile-prompt");
                assert!(
                    stdout.contains(&expected_prompt),
                    "{program} with profile {profile_name} did not reset before the prompt: {stdout:?}"
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_trackers_report_the_directory_after_cd() {
        let directory = std::env::current_dir().unwrap().join("src");
        let expected_marker = format!("\u{1b}]2;zetta-cwd:{}\u{1b}\\", directory.display());

        let mut arguments = None;
        let mut environment = HashMap::default();
        install_windows_cwd_tracking("cmd.exe", &mut arguments, &mut environment);
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/d", "/q"])
            .envs(environment)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("cd /d \"{}\"\r\nexit\r\n", directory.display()).as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&expected_marker),
            "Command Prompt did not report its CWD after cd: {stdout:?}"
        );
    }

    #[cfg(windows)]
    #[gpui::test]
    async fn windows_powershell_terminal_reports_cwd_after_cd(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        init_test(cx);

        let directory = std::env::current_dir().unwrap().join("src");
        let escaped_directory = directory.to_string_lossy().replace('\'', "''");
        let command = format!("Set-Location -LiteralPath '{escaped_directory}'");
        let mut available_shell = false;

        for program in ["powershell.exe", "pwsh.exe"] {
            if std::process::Command::new(program)
                .args(["-NoLogo", "-NoProfile", "-Command", "exit"])
                .output()
                .is_err()
            {
                continue;
            }
            available_shell = true;

            let (terminal, completion_rx) =
                build_test_terminal_with_arguments(cx, program.to_owned(), vec!["-NoLogo".into()])
                    .await;
            terminal.update(cx, |terminal, _| {
                terminal.input(format!("{command}\r").into_bytes());
            });

            let mut reported = None;
            for _ in 0..200 {
                reported = terminal.update(cx, |terminal, _| {
                    terminal.reported_working_directory().map(PathBuf::from)
                });
                if reported.as_deref() == Some(directory.as_path()) {
                    break;
                }
                cx.background_executor
                    .timer(Duration::from_millis(10))
                    .await;
            }

            terminal.update(cx, |terminal, _| terminal.input(b"exit\r".to_vec()));
            let _ = completion_rx.recv().await;
            assert_eq!(reported.as_deref(), Some(directory.as_path()), "{program}");
        }

        if !available_shell {
            eprintln!("neither powershell.exe nor pwsh.exe is installed");
        }
    }

    #[cfg(windows)]
    #[gpui::test]
    async fn windows_powershell_automatic_tracker_resets_native_attributes(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        init_test(cx);

        let mut available_shell = false;
        for program in ["powershell.exe", "pwsh.exe"] {
            if std::process::Command::new(program)
                .args(["-NoLogo", "-NoProfile", "-Command", "exit"])
                .output()
                .is_err()
            {
                continue;
            }
            available_shell = true;

            let (terminal, completion_rx) = build_test_terminal_with_arguments(
                cx,
                program.to_owned(),
                vec!["-NoLogo".into(), "-NoProfile".into()],
            )
            .await;
            terminal.update(cx, |terminal, _| {
                terminal.input(
                    b"cmd.exe /d /c color 4; Write-Output ZETTA_NATIVE_COLOR_DONE\r".to_vec(),
                );
            });

            let mut completed = false;
            for _ in 0..200 {
                cx.run_until_parked();
                completed = terminal.read_with(cx, |terminal, _| {
                    terminal.get_content().contains("ZETTA_NATIVE_COLOR_DONE")
                        && terminal.term.lock().grid().cursor.template.fg
                            == Color::Named(NamedColor::Foreground)
                });
                if completed {
                    break;
                }
                cx.background_executor
                    .timer(Duration::from_millis(10))
                    .await;
            }
            assert!(
                completed,
                "{program} did not complete the native color transition"
            );

            let foreground = terminal.read_with(cx, |terminal, _| {
                terminal.term.lock().grid().cursor.template.fg
            });
            assert_eq!(
                foreground,
                Color::Named(NamedColor::Foreground),
                "{program}'s automatically injected tracker must reset before its prompt"
            );

            terminal.update(cx, |terminal, _| terminal.input(b"exit\r".to_vec()));
            let _ = completion_rx.recv().await;
        }

        if !available_shell {
            eprintln!("neither powershell.exe nor pwsh.exe is installed");
        }
    }

    #[test]
    fn test_init_command_startup_marker_commands_do_not_contain_marker() {
        let marker_id = 42;
        let marker = init_command_startup_marker(marker_id);

        for shell_kind in [
            ShellKind::Posix,
            ShellKind::Csh,
            ShellKind::Tcsh,
            ShellKind::Rc,
            ShellKind::Fish,
            ShellKind::PowerShell,
            ShellKind::Pwsh,
            ShellKind::Nushell,
            ShellKind::Cmd,
            ShellKind::Xonsh,
            ShellKind::Elvish,
        ] {
            let command = init_command_startup_marker_command(shell_kind, marker_id);
            assert!(
                !command.contains(&marker),
                "startup marker command for {shell_kind:?} should not contain the full marker, got {command:?}"
            );
        }
    }

    #[test]
    fn editor_invocation_quotes_paths_for_the_active_shell() {
        let path = Path::new("/tmp/zetta scrollback.txt");
        let shell = Shell::System;
        let posix_path_argument =
            editor_path_argument(ShellKind::Posix, &shell, path, PathStyle::local()).unwrap();
        assert_eq!(
            editor_invocation_command("zetta", &posix_path_argument, false),
            "zetta edit -- '/tmp/zetta scrollback.txt'"
        );
        let powershell_path_argument =
            editor_path_argument(ShellKind::PowerShell, &shell, path, PathStyle::local()).unwrap();
        assert_eq!(
            editor_invocation_command("zetta", &powershell_path_argument, false),
            "zetta edit -- '/tmp/zetta scrollback.txt'"
        );
        let cmd_path_argument = editor_path_argument(
            ShellKind::Cmd,
            &shell,
            Path::new(r"C:\Temp\zetta scrollback.txt"),
            PathStyle::local(),
        )
        .unwrap();
        assert_eq!(
            editor_invocation_command("zetta", &cmd_path_argument, false),
            "zetta edit -- ^\"C:\\Temp\\zetta scrollback.txt^\""
        );
        assert_eq!(
            editor_invocation_command("zetta", &posix_path_argument, true),
            "zetta edit --delete-after -- '/tmp/zetta scrollback.txt'"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_posix_host_shells_invoke_the_native_zetta_executable() {
        let executable = Path::new(r"C:\Program Files\Zetta\zetta.exe");
        assert_eq!(
            native_zetta_command_for_msys2(executable),
            Some("\"$(cygpath -u \"C:\\\\Program Files\\\\Zetta\\\\zetta.exe\")\"".to_owned())
        );

        let wsl = Shell::Program(r"C:\Windows\System32\wsl.exe".to_owned());
        assert_eq!(
            zetta_command_for_shell(&wsl),
            Some("\"$ZETTA_HOST_EXECUTABLE\"".to_owned())
        );
        assert_eq!(
            interaction_shell_kind(&wsl, PathStyle::local()),
            ShellKind::Posix
        );
        assert_eq!(
            wsl_editor_working_directory(&wsl, Some("/home/saltw/source/zetta")),
            Some(PathBuf::from("/home/saltw/source/zetta"))
        );
        assert_eq!(
            wsl_editor_working_directory(&wsl, Some("/etc")),
            Some(PathBuf::from("/etc"))
        );
        let msys2 = Shell::WithArguments {
            program: "cmd.exe".to_owned(),
            args: vec![r#""C:\msys64\msys2_shell.cmd" -shell bash"#.to_owned()],
            title_override: None,
        };
        assert_eq!(
            interaction_shell_kind(&msys2, PathStyle::local()),
            ShellKind::Posix
        );
        let cygwin_bash = Shell::WithArguments {
            program: r"C:\cygwin64\bin\bash.exe".to_owned(),
            args: vec!["-l".to_owned()],
            title_override: Some("Cygwin".to_owned()),
        };
        assert_eq!(posix_host(&cygwin_bash), Some(PosixHost::Cygwin));
        assert_eq!(
            interaction_shell_kind(&cygwin_bash, PathStyle::local()),
            ShellKind::Posix
        );
        assert_eq!(
            native_zetta_command_for_cygwin(executable),
            Some("\"$(cygpath -u \"C:\\\\Program Files\\\\Zetta\\\\zetta.exe\")\"".to_owned())
        );
        let cygwin_fish = Shell::WithArguments {
            program: r"C:\cygwin64\bin\fish.exe".to_owned(),
            args: vec!["-l".to_owned()],
            title_override: Some("Cygwin: Fish".to_owned()),
        };
        assert_eq!(
            interaction_shell_kind(&cygwin_fish, PathStyle::local()),
            ShellKind::Fish
        );
        let cygwin_nu = Shell::WithArguments {
            program: r"C:\cygwin64\bin\nu.exe".to_owned(),
            args: vec!["-l".to_owned()],
            title_override: Some("Cygwin: Nushell".to_owned()),
        };
        assert_eq!(
            interaction_shell_kind(&cygwin_nu, PathStyle::local()),
            ShellKind::Nushell
        );
        assert_eq!(
            cygwin_path_like_to_windows(
                &cygwin_bash,
                "/cygdrive/c/Users/saltw/source/zetta/main.rs:12:4"
            ),
            Some(r"C:\Users\saltw\source\zetta\main.rs:12:4".to_owned())
        );
        let wsl_path_argument = editor_path_argument(
            ShellKind::Posix,
            &wsl,
            Path::new("/home/saltw/source/zetta/LICENSE-APACHE"),
            PathStyle::Unix,
        )
        .unwrap();
        assert_eq!(
            wsl_path_argument,
            "\"$(wslpath -w /home/saltw/source/zetta/LICENSE-APACHE)\""
        );
        assert_eq!(
            editor_path_argument(
                ShellKind::Posix,
                &wsl,
                Path::new("/etc/ssh/sshd_config"),
                PathStyle::Unix,
            ),
            Some("\"$(wslpath -w /etc/ssh/sshd_config)\"".to_owned())
        );
        for path in [
            "/",
            "/etc/hosts",
            "/usr/local/bin/zsh",
            "/opt/service/config.toml",
            "/var/log/messages",
            "/mnt/c/Users/saltw/Desktop/notes.txt",
        ] {
            assert_eq!(
                editor_path_argument(ShellKind::Posix, &wsl, Path::new(path), PathStyle::Unix),
                Some(format!("\"$(wslpath -w {path})\""))
            );
        }
        assert_eq!(
            editor_path_argument(
                ShellKind::Posix,
                &wsl,
                Path::new("../etc/hosts"),
                PathStyle::Unix,
            ),
            Some("\"$(wslpath -w \"$(pwd -P)/$(printf %s ../etc/hosts)\")\"".to_owned())
        );
        assert_eq!(
            editor_path_argument(
                ShellKind::Posix,
                &wsl,
                Path::new("~/source with spaces/README.md"),
                PathStyle::Unix,
            ),
            Some(
                "\"$(wslpath -w \"$HOME/$(printf %s 'source with spaces/README.md')\")\""
                    .to_owned()
            )
        );
        assert_eq!(
            editor_path_argument(ShellKind::Posix, &wsl, Path::new("~"), PathStyle::Unix),
            Some("\"$(wslpath -w \"$HOME\")\"".to_owned())
        );
        assert_eq!(
            editor_invocation_command("\"$ZETTA_HOST_EXECUTABLE\"", &wsl_path_argument, false),
            "\"$ZETTA_HOST_EXECUTABLE\" edit -- \"$(wslpath -w /home/saltw/source/zetta/LICENSE-APACHE)\""
        );

        let scrollback_path_argument = editor_path_argument(
            ShellKind::Posix,
            &wsl,
            Path::new(r"C:\Users\saltw\AppData\Local\Temp\zetta\scrollback.txt"),
            PathStyle::Windows,
        )
        .unwrap();
        assert_eq!(
            scrollback_path_argument,
            r#""C:\\Users\\saltw\\AppData\\Local\\Temp\\zetta\\scrollback.txt""#
        );
        let scrollback_command = editor_invocation_command(
            &zetta_command_for_shell(&wsl).unwrap(),
            &scrollback_path_argument,
            true,
        );
        assert_eq!(
            scrollback_command,
            r#""$ZETTA_HOST_EXECUTABLE" edit --delete-after -- "C:\\Users\\saltw\\AppData\\Local\\Temp\\zetta\\scrollback.txt""#
        );
        assert!(!scrollback_command.contains("wslpath"));
        let cygwin_bash_path = editor_path_argument(
            ShellKind::Posix,
            &cygwin_bash,
            Path::new("/cygdrive/c/Users/saltw/source/zetta/file with spaces.txt"),
            PathStyle::Unix,
        )
        .unwrap();
        assert_eq!(
            cygwin_bash_path,
            "\"$(cygpath -w '/cygdrive/c/Users/saltw/source/zetta/file with spaces.txt')\""
        );
        let cygwin_fish_path = editor_path_argument(
            ShellKind::Fish,
            &cygwin_fish,
            Path::new("/cygdrive/c/Users/saltw/file.txt"),
            PathStyle::Unix,
        )
        .unwrap();
        assert_eq!(
            cygwin_fish_path,
            "(cygpath -w /cygdrive/c/Users/saltw/file.txt)"
        );
        let cygwin_nu_path = editor_path_argument(
            ShellKind::Nushell,
            &cygwin_nu,
            Path::new("/cygdrive/c/Users/saltw/file.txt"),
            PathStyle::Unix,
        )
        .unwrap();
        assert_eq!(
            cygwin_nu_path,
            "(cygpath -w /cygdrive/c/Users/saltw/file.txt)"
        );
        assert_eq!(
            editor_path_argument(
                ShellKind::Fish,
                &cygwin_fish,
                Path::new("~/source with spaces/file.txt"),
                PathStyle::Unix,
            ),
            Some("(cygpath -w (string join / $HOME 'source with spaces/file.txt'))".to_owned())
        );
    }

    #[gpui::test]
    async fn test_init_command_startup_marker_ignores_echoed_command(cx: &mut TestAppContext) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        let marker_id = 4242;
        let marker = init_command_startup_marker(marker_id);
        let command = init_command_startup_marker_command(ShellKind::Posix, marker_id);
        let (startup_tx, startup_rx) = async_channel::bounded(1);

        terminal.update(cx, |terminal, cx| {
            terminal.init_command_startup_marker = Some(marker.clone());
            terminal.init_command_startup_tx = Some(startup_tx);
            terminal.write_output(command.as_bytes(), cx);
        });
        assert!(matches!(
            startup_rx.try_recv(),
            Err(async_channel::TryRecvError::Empty)
        ));

        terminal.update(cx, |terminal, cx| {
            terminal.write_output(marker.as_bytes(), cx);
        });
        assert!(startup_rx.try_recv().is_ok());
    }

    #[test]
    fn test_normalize_path_command_name() {
        assert_eq!(normalize_path_command_name("claude"), Some("claude".into()));
        assert_eq!(normalize_path_command_name("Cargo"), Some("cargo".into()));
        assert_eq!(normalize_path_command_name("node.exe"), Some("node".into()));
        assert_eq!(
            normalize_path_command_name("my-agent_cli.1"),
            Some("my-agent_cli.1".into())
        );
        assert_eq!(normalize_path_command_name("./local-agent"), None);
        assert_eq!(normalize_path_command_name("../local-agent"), None);
        assert_eq!(normalize_path_command_name("/usr/local/bin/cargo"), None);
        assert_eq!(
            normalize_path_command_name("target\\debug\\agent.exe"),
            None
        );
        assert_eq!(normalize_path_command_name(".hidden-agent"), None);
        assert_eq!(normalize_path_command_name("agent with spaces"), None);
        assert_eq!(normalize_path_command_name("zsh"), Some("zsh".into()));
        assert_eq!(normalize_path_command_name("-zsh"), None);
        assert_eq!(normalize_path_command_name("pwsh.exe"), Some("pwsh".into()));
    }

    #[gpui::test]
    async fn display_only_terminals_require_a_new_editor_pane(cx: &mut TestAppContext) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        assert!(terminal.read_with(cx, |terminal, _| {
            terminal.editor_should_open_in_new_pane()
        }));
        assert!(!terminal.read_with(cx, |terminal, _| { terminal.foreground_process_is_shell() }));
    }

    #[test]
    fn test_foreground_process_command_from_interpreter_wrapper() {
        assert_eq!(
            foreground_process_command_from_argv(&[
                "node".to_string(),
                "/opt/homebrew/lib/node_modules/@google/gemini-cli/dist/index.js".to_string(),
            ]),
            Some("gemini".to_string())
        );
        assert_eq!(
            foreground_process_command_from_argv(&[
                "python3".to_string(),
                "/Users/me/.local/bin/codex.py".to_string(),
            ]),
            Some("codex".to_string())
        );
        assert_eq!(
            foreground_process_command_from_argv(&[
                "node".to_string(),
                "/Users/me/private-project/scripts/customer-data-export.js".to_string(),
            ]),
            Some("customer-data-export".to_string())
        );
    }

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    /// Helper to build a test terminal running a shell command.
    /// Returns the terminal entity and a receiver for the completion signal.
    async fn build_test_terminal(
        cx: &mut TestAppContext,
        command: &str,
        args: &[&str],
    ) -> (Entity<Terminal>, Receiver<Option<ExitStatus>>) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (program, args) =
            ShellBuilder::new(&Shell::System, false).build(Some(command.to_owned()), &args);
        build_test_terminal_with_arguments(cx, program, args).await
    }

    async fn build_test_terminal_with_arguments(
        cx: &mut TestAppContext,
        program: String,
        args: Vec<String>,
    ) -> (Entity<Terminal>, Receiver<Option<ExitStatus>>) {
        let (completion_tx, completion_rx) = async_channel::unbounded();
        let builder = cx
            .update(|cx| {
                TerminalBuilder::new(
                    None,
                    None,
                    task::Shell::WithArguments {
                        program,
                        args,
                        title_override: None,
                    },
                    HashMap::default(),
                    SettingsCursorShape::default(),
                    AlternateScroll::On,
                    None,
                    vec![],
                    0,
                    false,
                    0,
                    Some(completion_tx),
                    cx,
                    vec![],
                    PathStyle::local(),
                    None,
                )
            })
            .await
            .unwrap();
        let terminal = cx.new(|cx| builder.subscribe(cx));
        (terminal, completion_rx)
    }

    #[cfg(unix)]
    async fn build_test_task_terminal(
        cx: &mut TestAppContext,
        program: String,
        args: Vec<String>,
        command: String,
    ) -> (Entity<Terminal>, Receiver<Option<ExitStatus>>) {
        let (completion_tx, completion_rx) = async_channel::unbounded();
        let task_state = TaskState {
            status: TaskStatus::Running,
            completion_rx: completion_rx.clone(),
            spawned_task: SpawnInTerminal {
                command: Some(command),
                ..Default::default()
            },
        };
        let builder = cx
            .update(|cx| {
                TerminalBuilder::new(
                    None,
                    Some(task_state),
                    task::Shell::WithArguments {
                        program,
                        args,
                        title_override: None,
                    },
                    HashMap::default(),
                    SettingsCursorShape::default(),
                    AlternateScroll::On,
                    None,
                    vec![],
                    0,
                    false,
                    0,
                    Some(completion_tx),
                    cx,
                    vec![],
                    PathStyle::local(),
                    None,
                )
            })
            .await
            .unwrap();
        let terminal = cx.new(|cx| builder.subscribe(cx));
        (terminal, completion_rx)
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn task_pty_forwards_ctrl_c_to_the_foreground_command(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let command = "sleep 30".to_owned();
        let (program, args) =
            ShellBuilder::new(&Shell::System, false).build(Some(command.clone()), &[]);
        let (terminal, completion_rx) = build_test_task_terminal(cx, program, args, command).await;

        // The PTY line discipline turns `^C` into a signal for whatever process
        // group is in the foreground *at the moment it is written*. Waiting a
        // fixed span here raced the shell's fork of `sleep`: under load the
        // keystroke could land while the shell was still starting, so the signal
        // had no `sleep` to reach and the task ran its full 30 seconds. Wait for
        // the process the test intends to interrupt to actually be in front.
        assert_foreground_process_command_eventually(&terminal, "sleep", cx).await;
        terminal.update(cx, |terminal, _| {
            assert!(terminal.try_keystroke(&Keystroke::parse("ctrl-c").unwrap(), false));
        });
        let status = completion_rx.recv().await.unwrap();
        assert!(status.is_some(), "Ctrl-C should terminate the task PTY");
        assert_ne!(status.and_then(|status| status.code()), Some(0));
    }

    /// Builds a non-PTY (`no_pty`) task terminal, exercising the path used by
    /// headless hosts (e.g. the eval CLI) where PTY allocation fails with
    /// `ENOTTY`. The command runs as a plain subprocess whose piped output is
    /// pumped into the emulator.
    #[cfg(not(target_os = "windows"))]
    async fn build_test_subprocess_terminal(
        cx: &mut TestAppContext,
        program: String,
        args: Vec<String>,
    ) -> (Entity<Terminal>, Receiver<Option<ExitStatus>>) {
        let (completion_tx, completion_rx) = async_channel::unbounded();
        let task_state = TaskState {
            status: TaskStatus::Running,
            completion_rx: completion_rx.clone(),
            spawned_task: SpawnInTerminal {
                command: Some(program.clone()),
                args: args.clone(),
                ..Default::default()
            },
        };
        let builder = cx
            .update(|cx| {
                cx.set_global(HeadlessTerminal(true));
                TerminalBuilder::new(
                    None,
                    Some(task_state),
                    task::Shell::WithArguments {
                        program,
                        args,
                        title_override: None,
                    },
                    HashMap::default(),
                    SettingsCursorShape::default(),
                    AlternateScroll::On,
                    None,
                    vec![],
                    0,
                    false,
                    0,
                    Some(completion_tx),
                    cx,
                    vec![],
                    PathStyle::local(),
                    None,
                )
            })
            .await
            .unwrap();
        let terminal = cx.new(|cx| builder.subscribe(cx));
        (terminal, completion_rx)
    }

    #[test]
    fn test_convert_lf_to_crlf_preserves_split_crlf() {
        let mut previous_byte_was_cr = false;
        assert_eq!(
            convert_lf_to_crlf(b"one\n", &mut previous_byte_was_cr),
            b"one\r\n"
        );
        assert!(!previous_byte_was_cr);

        let mut previous_byte_was_cr = false;
        assert_eq!(
            convert_lf_to_crlf(b"two\r", &mut previous_byte_was_cr),
            b"two\r"
        );
        assert!(previous_byte_was_cr);
        assert_eq!(
            convert_lf_to_crlf(b"\nthree", &mut previous_byte_was_cr),
            b"\nthree"
        );
        assert!(!previous_byte_was_cr);
    }

    /// Regression test for the agent terminal failing with `Not a tty (os error
    /// 25)` in headless/eval sandboxes: a `no_pty` task terminal must run
    /// without a PTY, capture stdout, and report its exit status.
    #[cfg(not(target_os = "windows"))]
    #[gpui::test]
    async fn test_no_pty_task_terminal_captures_output(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let (program, args) = ShellBuilder::new(&Shell::System, false)
            .non_interactive()
            .build(Some("echo hello-from-subprocess".to_owned()), &[]);
        let (terminal, completion_rx) = build_test_subprocess_terminal(cx, program, args).await;

        assert!(
            !terminal.update(cx, |term, _| term.is_pty()),
            "no_pty terminal should not be PTY-backed"
        );
        assert_eq!(
            completion_rx.recv().await.unwrap(),
            Some(ExitStatus::default())
        );
        assert_content_eventually(&terminal, "hello-from-subprocess", cx).await;
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn test_task_finished_reports_success_nonzero_and_signal_exit_codes(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();

        for (command, expected_code) in [
            ("exit 0", Some(0)),
            ("exit 7", Some(7)),
            ("kill -TERM $$", None),
        ] {
            let (terminal, completion_rx) = build_test_subprocess_terminal(
                cx,
                "sh".to_owned(),
                vec!["-c".to_owned(), command.to_owned()],
            )
            .await;
            let (event_tx, event_rx) = async_channel::unbounded();
            cx.update(|cx| {
                cx.subscribe(&terminal, move |_, event: &Event, _| {
                    event_tx.send_blocking(event.clone()).unwrap();
                })
            })
            .detach();

            let completion = completion_rx.recv().await.unwrap();
            let event_code = loop {
                match event_rx.recv().await.unwrap() {
                    Event::TaskFinished { exit_code } => break exit_code,
                    _ => {}
                }
            };
            assert_eq!(completion.and_then(|status| status.code()), expected_code);
            assert_eq!(event_code, expected_code);
            assert_eq!(
                terminal.read_with(cx, |terminal, _| terminal.task_exit_code()),
                expected_code
            );
        }
    }

    fn init_ctrl_click_hyperlink_test(cx: &mut TestAppContext, output: &[u8]) -> Entity<Terminal> {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
        });

        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        terminal.update(cx, |terminal, cx| {
            terminal.write_output(output, cx);
        });

        cx.run_until_parked();

        terminal.update(cx, |terminal, _cx| {
            let term_lock = terminal.term.lock();
            terminal.last_content = make_content(&term_lock, &mut terminal.last_content);
            drop(term_lock);

            let terminal_bounds = TerminalBounds::new(
                px(20.0),
                px(10.0),
                bounds(point(px(0.0), px(0.0)), size(px(400.0), px(400.0))),
            );
            terminal.last_content.terminal_bounds = terminal_bounds;
            terminal.events.clear();
            terminal.take_pty_write_log();
        });

        terminal
    }

    fn ctrl_mouse_down_at(
        terminal: &mut Terminal,
        position: GpuiPoint<Pixels>,
        cx: &mut Context<Terminal>,
    ) {
        let mouse_down = MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            click_count: 1,
            first_mouse: true,
        };
        terminal.mouse_down(&mouse_down, cx);
    }

    fn ctrl_mouse_move_to(
        terminal: &mut Terminal,
        position: GpuiPoint<Pixels>,
        cx: &mut Context<Terminal>,
    ) {
        let terminal_bounds = terminal.last_content.terminal_bounds.bounds;
        let drag_event = MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
        };
        terminal.mouse_drag(&drag_event, terminal_bounds, cx);
    }

    fn ctrl_mouse_up_at(
        terminal: &mut Terminal,
        position: GpuiPoint<Pixels>,
        cx: &mut Context<Terminal>,
    ) {
        let mouse_up = MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            click_count: 1,
        };
        terminal.mouse_up(&mouse_up, cx);
    }

    fn left_mouse_down_at(
        terminal: &mut Terminal,
        position: GpuiPoint<Pixels>,
        cx: &mut Context<Terminal>,
    ) {
        let mouse_down = MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: true,
        };
        terminal.mouse_down(&mouse_down, cx);
    }

    fn left_mouse_up_at(
        terminal: &mut Terminal,
        position: GpuiPoint<Pixels>,
        cx: &mut Context<Terminal>,
    ) {
        let mouse_up = MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::none(),
            click_count: 1,
        };
        terminal.mouse_up(&mouse_up, cx);
    }

    fn left_mouse_drag_to(
        terminal: &mut Terminal,
        position: GpuiPoint<Pixels>,
        cx: &mut Context<Terminal>,
    ) {
        let region = terminal.last_content.terminal_bounds.bounds;
        let drag_event = MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::none(),
        };
        terminal.mouse_drag(&drag_event, region, cx);
    }

    /// A left click that jitters by a pixel or two (e.g. the window-focusing
    /// click) must not begin a selection, otherwise `copy_on_select` would
    /// overwrite the clipboard. Regression test for #58970.
    #[gpui::test]
    async fn test_terminal_click_jitter_does_not_start_selection(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"hello world\r\n");

        terminal.update(cx, |terminal, cx| {
            left_mouse_down_at(terminal, point(px(50.0), px(10.0)), cx);
            terminal.events.clear();

            // One pixel of movement is below the drag threshold.
            left_mouse_drag_to(terminal, point(px(51.0), px(10.0)), cx);

            assert!(
                !terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::UpdateSelection(_))),
                "a sub-threshold click jitter should not start a selection"
            );
            assert!(terminal.selection_phase == SelectionPhase::Ended);
        });
    }

    /// Pointer motion is sampled far more often than the display refreshes, so
    /// a move that changes nothing observable must not schedule a frame. Only
    /// clearing a live hover or queueing a hyperlink search may request one.
    #[gpui::test]
    async fn mouse_move_only_requests_a_redraw_when_hover_state_changes(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"https://example.com\r\n");

        terminal.update(cx, |terminal, _cx| {
            let inside = point(px(50.0), px(10.0));

            // No hyperlink modifier and no hover to clear: nothing to redraw.
            assert!(
                !terminal.schedule_find_hyperlink(Modifiers::none(), inside),
                "a plain move with no live hover should not request a redraw"
            );

            // Holding the modifier queues a search, which needs a frame to run.
            assert!(
                terminal.schedule_find_hyperlink(Modifiers::secondary_key(), inside),
                "a queued hyperlink search should request a redraw"
            );
            assert!(
                terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::FindHyperlink(_, _))),
                "the search should have been queued"
            );

            // A second sample at the same spot is throttled away, so there is
            // nothing new to draw.
            terminal.events.clear();
            assert!(
                !terminal.schedule_find_hyperlink(Modifiers::secondary_key(), inside),
                "a throttled repeat sample should not request a redraw"
            );

            // Releasing the modifier must clear a live hover exactly once.
            terminal.last_content.last_hovered_word = Some(HoveredWord {
                word: "https://example.com".to_owned(),
                word_match: Range::new(Point::new(0, 0), Point::new(0, 18)),
                id: 0,
            });
            assert!(
                terminal.schedule_find_hyperlink(Modifiers::none(), inside),
                "clearing a live hover should request a redraw"
            );
            assert!(terminal.last_content.last_hovered_word.is_none());
            assert!(
                !terminal.schedule_find_hyperlink(Modifiers::none(), inside),
                "the hover is already cleared, so no further redraw is needed"
            );
        });
    }

    /// A deliberate drag past the threshold must still start a selection.
    #[gpui::test]
    async fn test_terminal_deliberate_drag_starts_selection(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"hello world\r\n");

        terminal.update(cx, |terminal, cx| {
            left_mouse_down_at(terminal, point(px(50.0), px(10.0)), cx);
            terminal.events.clear();

            // Well beyond the drag threshold.
            left_mouse_drag_to(terminal, point(px(90.0), px(10.0)), cx);

            assert!(
                terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::UpdateSelection(_))),
                "a deliberate drag should start a selection"
            );
            assert!(terminal.selection_phase == SelectionPhase::Selecting);
        });
    }

    #[gpui::test]
    async fn test_terminal_middle_click_pastes_selection_clipboard(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"");

        cx.update(|cx| {
            let item = ClipboardItem::new_string("middle-click paste".to_owned());
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            {
                cx.write_to_primary(ClipboardItem::new_string(String::new()));
                cx.write_to_clipboard(item);
            }
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            cx.write_to_clipboard(item);
        });

        terminal.update(cx, |terminal, cx| {
            terminal.mouse_down(
                &MouseDownEvent {
                    button: MouseButton::Middle,
                    position: point(px(10.0), px(10.0)),
                    modifiers: Modifiers::none(),
                    click_count: 1,
                    first_mouse: true,
                },
                cx,
            );

            assert_eq!(
                terminal.take_pty_write_log(),
                vec![b"middle-click paste".to_vec()]
            );
        });
    }

    /// With mouse tracking active (e.g. htop), Shift is the escape hatch to
    /// select terminal text. Shift+drag must start a selection rather than being
    /// swallowed as a "extend existing selection" no-op. Regression test for #60254.
    #[gpui::test]
    async fn test_terminal_shift_drag_selects_while_mouse_tracking(cx: &mut TestAppContext) {
        // `?1002h` enables button-event mouse tracking, `?1006h` selects SGR encoding.
        let terminal = init_ctrl_click_hyperlink_test(cx, b"\x1b[?1002h\x1b[?1006hhello world\r\n");

        terminal.update(cx, |terminal, cx| {
            assert!(
                terminal.last_content.mode.intersects(Modes::MOUSE_MODE),
                "mouse tracking should be active"
            );

            let shift = Modifiers {
                shift: true,
                ..Modifiers::none()
            };
            terminal.mouse_down(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(50.0), px(10.0)),
                    modifiers: shift,
                    click_count: 1,
                    first_mouse: true,
                },
                cx,
            );

            // With no selection yet, the shift press must anchor a new selection
            // so the following drag has something to extend.
            assert!(
                terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::SetSelection(Some(_)))),
                "shift+click with no existing selection should anchor a selection"
            );
            terminal.events.clear();

            let region = terminal.last_content.terminal_bounds.bounds;
            terminal.mouse_drag(
                &MouseMoveEvent {
                    position: point(px(90.0), px(10.0)),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: shift,
                },
                region,
                cx,
            );

            assert!(
                terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::UpdateSelection(_))),
                "shift+drag should extend the selection while mouse tracking is active"
            );
            assert!(terminal.selection_phase == SelectionPhase::Selecting);
        });
    }

    /// Shift+click with a selection already on screen must keep extending it
    /// (the behavior added in #25143), not re-anchor a fresh one.
    #[gpui::test]
    async fn test_terminal_shift_click_extends_existing_selection(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"hello world\r\n");

        terminal.update(cx, |terminal, cx| {
            // A visible selection, as a sync would have populated in production.
            terminal.last_content.selection = Some(SelectionRange {
                start: Point::new(0, 0),
                end: Point::new(0, 5),
                is_block: false,
            });
            terminal.events.clear();

            terminal.mouse_down(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(90.0), px(10.0)),
                    modifiers: Modifiers {
                        shift: true,
                        ..Modifiers::none()
                    },
                    click_count: 1,
                    first_mouse: true,
                },
                cx,
            );

            assert!(
                terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::UpdateSelection(_))),
                "shift+click with an existing selection should extend it"
            );
            assert!(
                !terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::SetSelection(Some(_)))),
                "shift+click should extend, not re-anchor, an existing selection"
            );
        });
    }

    #[gpui::test]
    async fn test_basic_terminal(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let (terminal, completion_rx) = build_test_terminal(cx, "echo", &["hello"]).await;
        assert_eq!(
            completion_rx.recv().await.unwrap(),
            Some(ExitStatus::default())
        );
        assert_content_eventually(&terminal, "hello", cx).await;

        // Inject additional output directly into the emulator (display-only path)
        terminal.update(cx, |term, cx| {
            term.write_output(b"\nfrom_injection", cx);
        });

        let content_after = terminal.update(cx, |term, _| term.get_content());
        assert!(
            content_after.contains("from_injection"),
            "expected injected output to appear, got: {content_after}"
        );
    }

    #[cfg(windows)]
    #[gpui::test]
    #[ignore = "manual optimized-build ConPTY throughput check"]
    async fn windows_pty_output_throughput_benchmark(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let executable = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/release/zetta.exe")
            .to_string_lossy()
            .into_owned();
        assert!(
            Path::new(&executable).is_file(),
            "build target/release/zetta.exe before running this benchmark"
        );

        let started_at = Instant::now();
        let (terminal, completion_rx) = build_test_terminal_with_arguments(
            cx,
            executable,
            vec!["benchmark".to_owned(), "output".to_owned()],
        )
        .await;
        let status = completion_rx.recv().await.unwrap();
        let elapsed = started_at.elapsed();

        eprintln!(
            "ConPTY benchmark completed in {:.3} s",
            elapsed.as_secs_f64()
        );
        assert_eq!(status, Some(ExitStatus::default()));
        assert!(
            elapsed < Duration::from_secs(10),
            "ConPTY output took {:.3} s",
            elapsed.as_secs_f64()
        );
        let history_size = terminal.update(cx, |terminal, _| terminal.term.lock().history_size());
        assert!(
            history_size > 100_000,
            "expected the complete benchmark payload in scrollback, got {history_size} lines"
        );
    }

    #[cfg(windows)]
    #[gpui::test]
    async fn windows_conpty_preserves_ctrl_j_in_win32_input_mode(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });

        let output_path =
            std::env::temp_dir().join(format!("zetta-ctrl-j-{}.txt", std::process::id()));
        let escaped_output_path = output_path.to_string_lossy().replace('\'', "''");
        let command = format!(
            "$key = [Console]::ReadKey($true); [IO.File]::WriteAllText('{}', ('{{0}}|{{1}}|{{2}}' -f [int]$key.KeyChar, $key.Key, $key.Modifiers))",
            escaped_output_path
        );
        let (terminal, completion_rx) = build_test_terminal_with_arguments(
            cx,
            "powershell.exe".to_owned(),
            vec!["-NoProfile".to_owned(), "-Command".to_owned(), command],
        )
        .await;

        let mut negotiated = false;
        for _ in 0..100 {
            negotiated = terminal.update(cx, |terminal, _| {
                terminal
                    .term
                    .lock()
                    .mode()
                    .contains(alacritty_terminal::term::TermMode::WIN32_INPUT)
            });
            if negotiated {
                break;
            }
            cx.executor().timer(Duration::from_millis(50)).await;
        }
        assert!(negotiated, "terminal did not process DECSET 9001");

        let handled = terminal.update(cx, |terminal, _| {
            terminal.last_content.mode.insert(Modes::WIN32_INPUT);
            terminal.try_keystroke(&Keystroke::parse("ctrl-j").unwrap(), false)
        });
        assert!(handled);

        assert_eq!(
            completion_rx.recv().await.unwrap(),
            Some(ExitStatus::default())
        );
        let observed = std::fs::read_to_string(&output_path).unwrap();
        let _ = std::fs::remove_file(output_path);
        assert_eq!(observed, "10|J|Control");
    }

    #[gpui::test]
    async fn test_async_content_snapshot_captures_complete_output(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                Some(100),
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        let output = (0..20)
            .map(|line| format!("retained line {line}\n"))
            .collect::<String>();
        terminal.update(cx, |terminal, cx| {
            terminal.write_output(output.as_bytes(), cx);
        });

        let snapshot = terminal.update(cx, |terminal, _| terminal.get_content_async());
        let snapshot = snapshot.await;

        assert!(
            snapshot.contains("retained line 0"),
            "snapshot was {snapshot:?}"
        );
        assert!(
            snapshot.contains("retained line 19"),
            "snapshot was {snapshot:?}"
        );
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn test_foreground_process_command_tracks_path_command(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let (terminal, completion_rx) =
            build_test_terminal_with_arguments(cx, "sleep".to_string(), vec!["1".to_string()])
                .await;

        assert_foreground_process_command_eventually(&terminal, "sleep", cx).await;
        let command_line = terminal
            .update(cx, |terminal, _| terminal.foreground_process_command_line())
            .expect("foreground process should expose its argument vector");
        assert_eq!(command_line.last().map(String::as_str), Some("1"));

        assert!(
            completion_rx.recv().await.is_ok(),
            "expected terminal completion after sleep exits"
        );
    }

    // TODO should be tested on Linux too, but does not work there well
    #[cfg(target_os = "macos")]
    #[gpui::test(iterations = 10)]
    async fn test_terminal_eof(cx: &mut TestAppContext) {
        init_test(cx);

        cx.executor().allow_parking();

        let (completion_tx, completion_rx) = async_channel::unbounded();
        let builder = cx
            .update(|cx| {
                TerminalBuilder::new(
                    None,
                    None,
                    task::Shell::System,
                    HashMap::default(),
                    SettingsCursorShape::default(),
                    AlternateScroll::On,
                    None,
                    vec![],
                    0,
                    false,
                    0,
                    Some(completion_tx),
                    cx,
                    Vec::new(),
                    PathStyle::local(),
                    None,
                )
            })
            .await
            .unwrap();
        // Build an empty command, which will result in a tty shell spawned.
        let terminal = cx.new(|cx| builder.subscribe(cx));

        let (event_tx, event_rx) = async_channel::unbounded::<Event>();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, e, _| {
                event_tx.send_blocking(e.clone()).unwrap();
            })
        })
        .detach();
        cx.background_spawn(async move {
            assert_eq!(
                completion_rx.recv().await.unwrap(),
                Some(ExitStatus::default()),
                "EOF should result in the tty shell exiting successfully",
            );
        })
        .detach();

        let first_event = event_rx.recv().await.expect("No wakeup event received");

        terminal.update(cx, |terminal, _| {
            let success = terminal.try_keystroke(&Keystroke::parse("ctrl-d").unwrap(), false);
            assert!(success, "Should have registered ctrl-d sequence");
        });

        let mut all_events = vec![first_event];
        while let Ok(new_event) = event_rx.recv().await {
            all_events.push(new_event.clone());
            if new_event == Event::CloseTerminal {
                break;
            }
        }
        assert!(
            all_events.contains(&Event::CloseTerminal),
            "EOF command sequence should have triggered a TTY terminal exit, but got events: {all_events:?}",
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[gpui::test(iterations = 10)]
    async fn test_terminal_closes_after_nonzero_exit(cx: &mut TestAppContext) {
        init_test(cx);

        cx.executor().allow_parking();

        let builder = cx
            .update(|cx| {
                TerminalBuilder::new(
                    None,
                    None,
                    task::Shell::System,
                    HashMap::default(),
                    SettingsCursorShape::default(),
                    AlternateScroll::On,
                    None,
                    vec![],
                    0,
                    false,
                    0,
                    None,
                    cx,
                    Vec::new(),
                    PathStyle::local(),
                    None,
                )
            })
            .await
            .unwrap();
        let terminal = cx.new(|cx| builder.subscribe(cx));

        let (event_tx, event_rx) = async_channel::unbounded::<Event>();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, e, _| {
                event_tx.send_blocking(e.clone()).unwrap();
            })
        })
        .detach();

        let first_event = event_rx.recv().await.expect("No wakeup event received");

        terminal.update(cx, |terminal, _| {
            terminal.input(b"false\r".to_vec());
        });
        cx.executor().timer(Duration::from_millis(500)).await;
        terminal.update(cx, |terminal, _| {
            terminal.input(b"exit\r".to_vec());
        });

        let mut all_events = vec![first_event];
        while let Ok(new_event) = event_rx.recv().await {
            all_events.push(new_event.clone());
            if new_event == Event::CloseTerminal {
                break;
            }
        }
        assert!(
            all_events.contains(&Event::CloseTerminal),
            "Shell exiting after `false && exit` should close terminal, but got events: {all_events:?}",
        );
    }

    #[gpui::test(iterations = 10)]
    async fn test_terminal_no_exit_on_spawn_failure(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let (completion_tx, completion_rx) = async_channel::unbounded();
        let (program, args) = ShellBuilder::new(&Shell::System, false)
            .build(Some("asdasdasdasd".to_owned()), &["@@@@@".to_owned()]);
        let builder = cx
            .update(|cx| {
                TerminalBuilder::new(
                    None,
                    None,
                    task::Shell::WithArguments {
                        program,
                        args,
                        title_override: None,
                    },
                    HashMap::default(),
                    SettingsCursorShape::default(),
                    AlternateScroll::On,
                    None,
                    Vec::new(),
                    0,
                    false,
                    0,
                    Some(completion_tx),
                    cx,
                    Vec::new(),
                    PathStyle::local(),
                    None,
                )
            })
            .await
            .unwrap();
        let terminal = cx.new(|cx| builder.subscribe(cx));

        let all_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        cx.update({
            let all_events = all_events.clone();
            |cx| {
                cx.subscribe(&terminal, move |_, e, _| {
                    all_events.lock().push(e.clone());
                })
            }
        })
        .detach();
        let completion_check_task = cx.background_spawn(async move {
            // The channel may be closed if the terminal is dropped before sending
            // the completion signal, which can happen with certain task scheduling orders.
            let exit_status = completion_rx.recv().await.ok().flatten();
            if let Some(exit_status) = exit_status {
                assert!(
                    !exit_status.success(),
                    "Wrong shell command should result in a failure"
                );
                #[cfg(target_os = "windows")]
                assert_eq!(exit_status.code(), Some(1));
                #[cfg(not(target_os = "windows"))]
                assert_eq!(exit_status.code(), Some(127)); // code 127 means "command not found" on Unix
            }
        });

        completion_check_task.await;
        cx.executor().timer(Duration::from_millis(500)).await;

        assert!(
            !all_events
                .lock()
                .iter()
                .any(|event| event == &Event::CloseTerminal),
            "Wrong shell command should update the title but not should not close the terminal to show the error message, but got events: {all_events:?}",
        );
    }

    #[test]
    fn test_rgb_for_index() {
        // Test every possible value in the color cube.
        for i in 16..=231 {
            let (r, g, b) = rgb_for_index(i);
            assert_eq!(i, 16 + 36 * r + 6 * g + b);
        }
    }

    #[gpui::test]
    fn test_mouse_to_cell_test(mut rng: StdRng) {
        const ITERATIONS: usize = 10;
        const PRECISION: usize = 1000;

        for _ in 0..ITERATIONS {
            let viewport_cells = rng.random_range(15..20);
            let cell_size =
                rng.random_range(5 * PRECISION..20 * PRECISION) as f32 / PRECISION as f32;

            let size = crate::TerminalBounds {
                cell_width: Pixels::from(cell_size),
                line_height: Pixels::from(cell_size),
                bounds: bounds(
                    GpuiPoint::default(),
                    size(
                        Pixels::from(cell_size * (viewport_cells as f32)),
                        Pixels::from(cell_size * (viewport_cells as f32)),
                    ),
                ),
            };

            let cells = get_cells(size, &mut rng);
            let content = convert_cells_to_content(size, &cells);

            for row in 0..(viewport_cells - 1) {
                let row = row as usize;
                for col in 0..(viewport_cells - 1) {
                    let col = col as usize;

                    let row_offset = rng.random_range(0..PRECISION) as f32 / PRECISION as f32;
                    let col_offset = rng.random_range(0..PRECISION) as f32 / PRECISION as f32;

                    let mouse_pos = point(
                        Pixels::from(col as f32 * cell_size + col_offset),
                        Pixels::from(row as f32 * cell_size + row_offset),
                    );

                    let content_index =
                        content_index_for_mouse(mouse_pos, &content.terminal_bounds);
                    let mouse_cell = content.cells[content_index].character();
                    let real_cell = cells[row][col];

                    assert_eq!(mouse_cell, real_cell);
                }
            }
        }
    }

    #[gpui::test]
    fn test_mouse_to_cell_clamp(mut rng: StdRng) {
        let size = crate::TerminalBounds {
            cell_width: Pixels::from(10.),
            line_height: Pixels::from(10.),
            bounds: bounds(
                GpuiPoint::default(),
                size(Pixels::from(100.), Pixels::from(100.)),
            ),
        };

        let cells = get_cells(size, &mut rng);
        let content = convert_cells_to_content(size, &cells);

        assert_eq!(
            content.cells[content_index_for_mouse(
                point(Pixels::from(-10.), Pixels::from(-10.)),
                &content.terminal_bounds,
            )]
            .character(),
            cells[0][0]
        );
        assert_eq!(
            content.cells[content_index_for_mouse(
                point(Pixels::from(1000.), Pixels::from(1000.)),
                &content.terminal_bounds,
            )]
            .character(),
            cells[9][9]
        );
    }

    #[gpui::test]
    async fn test_set_size_coalesces_pixel_only_changes(cx: &mut TestAppContext) {
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
        });
        let mut terminal = builder.terminal;

        let base_bounds = TerminalBounds {
            cell_width: Pixels::from(10.),
            line_height: Pixels::from(10.),
            bounds: bounds(
                GpuiPoint::default(),
                size(Pixels::from(100.), Pixels::from(100.)),
            ),
        };

        terminal.set_size(base_bounds);
        terminal.events.clear();
        assert_eq!(terminal.last_content.terminal_bounds, base_bounds);

        // Pixel-only change: height grows by 1px but still the same number of rows/cols.
        let mut pixel_changed = base_bounds;
        pixel_changed.bounds.size.height = Pixels::from(101.);
        terminal.set_size(pixel_changed);
        assert!(terminal.events.is_empty());
        assert_eq!(terminal.last_content.terminal_bounds, pixel_changed);

        // Grid change: height increases enough to add a row.
        let mut grid_changed = base_bounds;
        grid_changed.bounds.size.height = Pixels::from(110.);
        terminal.set_size(grid_changed);
        assert!(matches!(
            terminal.events.back(),
            Some(InternalEvent::Resize { .. })
        ));
    }

    #[gpui::test]
    async fn grid_size_changes_are_reported_separately_from_output(cx: &mut TestAppContext) {
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
        });
        let window = cx.add_empty_window();
        let terminal = window.new(|cx| builder.subscribe(cx));

        let grid_size_changes = std::rc::Rc::new(std::cell::Cell::new(0_usize));
        window
            .update({
                let grid_size_changes = grid_size_changes.clone();
                let terminal = terminal.clone();
                move |_, cx| {
                    cx.subscribe(&terminal, move |_, event: &Event, _| {
                        if matches!(event, Event::GridSizeChanged) {
                            grid_size_changes.set(grid_size_changes.get() + 1);
                        }
                    })
                }
            })
            .detach();

        let base_bounds = TerminalBounds {
            cell_width: Pixels::from(10.),
            line_height: Pixels::from(10.),
            bounds: bounds(
                GpuiPoint::default(),
                size(Pixels::from(100.), Pixels::from(100.)),
            ),
        };
        window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.set_size(base_bounds);
            terminal.sync(window, cx);
        });
        grid_size_changes.set(0);

        // Output must not report a size change. The chrome that listens for this
        // renders inside a cached boundary, and reporting on output would put it
        // back into every frame the terminal causes — the whole cost this event
        // exists to avoid.
        window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.write_output(b"output", cx);
            terminal.sync(window, cx);
        });
        assert_eq!(grid_size_changes.get(), 0, "output is not a size change");

        // Neither is a resize too small to add or remove a row or column.
        let mut pixel_only = base_bounds;
        pixel_only.bounds.size.height = Pixels::from(101.);
        window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.set_size(pixel_only);
            terminal.sync(window, cx);
        });
        assert_eq!(
            grid_size_changes.get(),
            0,
            "a sub-cell resize leaves the reported grid size unchanged"
        );

        let mut grid_changed = base_bounds;
        grid_changed.bounds.size.height = Pixels::from(140.);
        window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.set_size(grid_changed);
            terminal.sync(window, cx);
        });
        assert_eq!(
            grid_size_changes.get(),
            1,
            "a resize that changes the number of rows is reported once"
        );
    }

    #[gpui::test]
    async fn test_layout_resize_can_disable_reflow_once(cx: &mut TestAppContext) {
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
        });
        let mut terminal = builder.terminal;
        let base_bounds = terminal.last_content.terminal_bounds;
        let mut pixel_only = base_bounds;
        pixel_only.bounds.size.width += Pixels::from(1.);

        terminal.truncate_on_next_resize();
        terminal.set_size(pixel_only);
        assert!(terminal.events.is_empty());

        let mut resized = pixel_only;
        resized.bounds.size.width += resized.cell_width;
        terminal.set_size(resized);
        assert!(
            matches!(
                terminal.events.back(),
                Some(InternalEvent::Resize { reflow: false, .. })
            ),
            "a sub-cell pixel change must leave the truncate request standing"
        );

        terminal.events.clear();
        resized.bounds.size.width += resized.cell_width;
        terminal.set_size(resized);
        assert!(matches!(
            terminal.events.back(),
            Some(InternalEvent::Resize { reflow: true, .. })
        ));
    }

    /// Callers arm whole sets of terminals without knowing which of them will
    /// actually change size. A request that outlives the layout it was armed for
    /// would truncate the next window resize instead of reflowing it.
    #[gpui::test]
    async fn test_a_settled_layout_drops_an_unused_truncate_request(cx: &mut TestAppContext) {
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
        });
        let mut terminal = builder.terminal;
        let base_bounds = terminal.last_content.terminal_bounds;

        terminal.truncate_on_next_resize();
        terminal.set_size(base_bounds);
        assert!(terminal.events.is_empty());

        let mut resized = base_bounds;
        resized.bounds.size.width += resized.cell_width;
        terminal.set_size(resized);
        assert!(matches!(
            terminal.events.back(),
            Some(InternalEvent::Resize { reflow: true, .. })
        ));
    }

    #[gpui::test]
    async fn test_coalesced_layout_resizes_preserve_non_reflow_decision(cx: &mut TestAppContext) {
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
        });
        let mut terminal = builder.terminal;
        let base_bounds = terminal.last_content.terminal_bounds;

        terminal.truncate_on_next_resize();
        let mut first_resize = base_bounds;
        first_resize.bounds.size.width += first_resize.cell_width;
        terminal.set_size(first_resize);

        let mut coalesced_resize = first_resize;
        coalesced_resize.bounds.size.width += coalesced_resize.cell_width;
        terminal.set_size(coalesced_resize);

        assert_eq!(terminal.events.len(), 1);
        assert!(matches!(
            terminal.events.back(),
            Some(InternalEvent::Resize {
                bounds,
                reflow: false,
            }) if *bounds == coalesced_resize
        ));
    }

    #[test]
    fn synchronous_history_reflow_has_a_cell_budget() {
        assert!(synchronous_reflow_is_bounded(10_000, 100));
        assert!(!synchronous_reflow_is_bounded(10_001, 100));
        assert!(!synchronous_reflow_is_bounded(usize::MAX, 2));
    }

    #[gpui::test]
    async fn test_sync_reuses_renderable_content_until_terminal_changes(cx: &mut TestAppContext) {
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
        });
        let window = cx.add_empty_window();
        let terminal = window.new(|cx| builder.subscribe(cx));

        let first_revision = window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.sync(window, cx);
            terminal.content_revision()
        });
        let second_revision = window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.sync(window, cx);
            terminal.content_revision()
        });
        assert_eq!(second_revision, first_revision);

        terminal.update(window, |terminal, cx| terminal.write_output(b"changed", cx));
        let changed_revision = window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.sync(window, cx);
            terminal.content_revision()
        });
        assert!(changed_revision > second_revision);
    }

    fn get_cells(size: TerminalBounds, rng: &mut StdRng) -> Vec<Vec<char>> {
        let mut cells = Vec::new();

        for _ in 0..size.num_lines() {
            let mut row_vec = Vec::new();
            for _ in 0..size.num_columns() {
                let cell_char = rng.sample(distr::Alphanumeric) as char;
                row_vec.push(cell_char)
            }
            cells.push(row_vec)
        }

        cells
    }

    fn convert_cells_to_content(terminal_bounds: TerminalBounds, cells: &[Vec<char>]) -> Content {
        let mut ic = Vec::new();

        for (index, row) in cells.iter().enumerate() {
            for (cell_index, cell_char) in row.iter().enumerate() {
                let mut cell = Cell::default();
                cell.set_character(*cell_char);
                ic.push(IndexedCell {
                    point: Point::new(index as i32, cell_index),
                    cell,
                });
            }
        }

        Content {
            cells: ic,
            terminal_bounds,
            ..Default::default()
        }
    }

    #[gpui::test]
    async fn test_write_init_command_after_startup_clears_without_shell_command(
        cx: &mut TestAppContext,
    ) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"startup output\nprompt", cx);
        });

        let wrote = terminal.update(cx, |terminal, cx| {
            terminal.write_init_command_after_startup(b"agent\r".to_vec(), cx)
        });
        assert!(wrote);
        let content = terminal.update(cx, |terminal, _| terminal.get_content());
        assert!(
            !content.contains("startup output"),
            "startup output should be cleared internally before writing the init command"
        );
        let input_log = terminal.update(cx, |terminal, _| terminal.take_input_log());
        assert_eq!(input_log, vec![b"agent\r".to_vec()]);
    }

    #[gpui::test]
    async fn test_write_init_command_after_startup_skips_after_keyboard_input(
        cx: &mut TestAppContext,
    ) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        let wrote = terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"startup output\nprompt", cx);
            terminal.input(b"user input".to_vec());
            terminal.write_init_command_after_startup(b"agent\r".to_vec(), cx)
        });
        assert!(!wrote);
        let content = terminal.update(cx, |terminal, _| terminal.get_content());
        assert!(
            content.contains("startup output"),
            "startup output should be left alone when the init command is skipped"
        );
        let input_log = terminal.update(cx, |terminal, _| terminal.take_input_log());
        assert_eq!(input_log, vec![b"user input".to_vec()]);
    }

    #[gpui::test]
    async fn test_write_init_command_after_startup_skips_after_child_exit(cx: &mut TestAppContext) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"shell failed to start\nprompt", cx);
            #[cfg(unix)]
            let exit_status =
                <ExitStatus as std::os::unix::process::ExitStatusExt>::from_raw(1 << 8);
            #[cfg(windows)]
            let exit_status = <ExitStatus as std::os::windows::process::ExitStatusExt>::from_raw(1);
            terminal.register_task_finished(Some(exit_status), cx);
        });

        let wrote = terminal.update(cx, |terminal, cx| {
            terminal.write_init_command_after_startup(b"agent\r".to_vec(), cx)
        });
        assert!(!wrote);
        let content = terminal.update(cx, |terminal, _| terminal.get_content());
        assert!(
            content.contains("shell failed to start"),
            "startup failure output should be preserved when the init command is skipped"
        );
        let input_log = terminal.update(cx, |terminal, _| terminal.take_input_log());
        assert!(
            input_log.is_empty(),
            "init command should not be written after the child has exited, got {input_log:?}"
        );
    }

    #[gpui::test]
    async fn test_write_output_converts_lf_to_crlf(cx: &mut TestAppContext) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        // Test simple LF conversion
        terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"line1\nline2\n", cx);
        });

        // Get the content by directly accessing the term
        let content = terminal.update(cx, |terminal, _cx| {
            let term = terminal.term.lock_unfair();
            make_content(&term, &mut terminal.last_content)
        });

        // If LF is properly converted to CRLF, each line should start at column 0
        // The diagonal staircase bug would cause increasing column positions

        // Get the cells and check that lines start at column 0
        let cells = &content.cells;
        let mut line1_col0 = false;
        let mut line2_col0 = false;

        for cell in cells {
            if cell.character() == 'l' && cell.point.column == 0 {
                if cell.point.line == 0 && !line1_col0 {
                    line1_col0 = true;
                } else if cell.point.line == 1 && !line2_col0 {
                    line2_col0 = true;
                }
            }
        }

        assert!(line1_col0, "First line should start at column 0");
        assert!(line2_col0, "Second line should start at column 0");
    }

    #[gpui::test]
    async fn test_write_output_preserves_existing_crlf(cx: &mut TestAppContext) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        // Test that existing CRLF doesn't get doubled
        terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"line1\r\nline2\r\n", cx);
        });

        // Get the content by directly accessing the term
        let content = terminal.update(cx, |terminal, _cx| {
            let term = terminal.term.lock_unfair();
            make_content(&term, &mut terminal.last_content)
        });

        let cells = &content.cells;

        // Check that both lines start at column 0
        let mut found_lines_at_column_0 = 0;
        for cell in cells {
            if cell.character() == 'l' && cell.point.column == 0 {
                found_lines_at_column_0 += 1;
            }
        }

        assert!(
            found_lines_at_column_0 >= 2,
            "Both lines should start at column 0"
        );
    }

    #[gpui::test]
    async fn test_write_output_preserves_bare_cr(cx: &mut TestAppContext) {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        // Test that bare CR (without LF) is preserved
        terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"hello\rworld", cx);
        });

        // Get the content by directly accessing the term
        let content = terminal.update(cx, |terminal, _cx| {
            let term = terminal.term.lock_unfair();
            make_content(&term, &mut terminal.last_content)
        });

        let cells = &content.cells;

        // Check that we have "world" at the beginning of the line
        let mut text = String::new();
        for cell in cells.iter().take(5) {
            if cell.point.line == 0 {
                text.push(cell.character());
            }
        }

        assert!(
            text.starts_with("world"),
            "Bare CR should allow overwriting: got '{}'",
            text
        );
    }

    #[gpui::test]
    async fn test_display_only_write_output_ignores_osc52(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            cx.write_to_clipboard(ClipboardItem::new_string("original".to_string()));
        });

        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });

        terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"\x1b]52;c;b3ZlcndyaXR0ZW4=\x07", cx);
        });
        cx.run_until_parked();

        let clipboard_text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(clipboard_text.as_deref(), Some("original"));
    }

    #[gpui::test]
    async fn test_hyperlink_ctrl_click_same_position(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"Visit https://zed.dev/ for more\r\n");

        terminal.update(cx, |terminal, cx| {
            let click_position = point(px(80.0), px(10.0));
            ctrl_mouse_down_at(terminal, click_position, cx);
            ctrl_mouse_up_at(terminal, click_position, cx);

            assert!(
                terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::ProcessHyperlink(_, true))),
                "Should have ProcessHyperlink event when ctrl+clicking on same hyperlink position"
            );
        });
    }

    #[gpui::test]
    async fn editor_click_resolves_file_url_and_writes_pane_command(cx: &mut TestAppContext) {
        let terminal =
            init_ctrl_click_hyperlink_test(cx, b"Visit file:///tmp/notes.txt for more\r\n");

        terminal.update(cx, |terminal, _| {
            let event = MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(80.0), px(10.0)),
                modifiers: Modifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                },
                click_count: 1,
                first_mouse: true,
            };
            let target = terminal
                .path_like_target_at_event_position(&event)
                .expect("file URL should be recognized under the editor click");
            assert_eq!(target.maybe_path, "/tmp/notes.txt");

            terminal.open_path_in_editor(Path::new(&target.maybe_path));
            assert_eq!(
                terminal.take_pty_write_log(),
                vec![b"zetta edit -- /tmp/notes.txt".to_vec(), b"\r".to_vec()]
            );
        });
    }

    #[cfg(windows)]
    #[gpui::test]
    async fn editor_click_uses_the_reported_wsl_directory(cx: &mut TestAppContext) {
        let terminal =
            init_ctrl_click_hyperlink_test(cx, b"Visit file:///tmp/notes.txt for more\r\n");

        terminal.update(cx, |terminal, _| {
            terminal.template.shell = Shell::Program(r"C:\Windows\System32\wsl.exe".to_owned());
            terminal.reported_working_directory = Some("/etc".to_owned());
            let event = MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(80.0), px(10.0)),
                modifiers: Modifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                },
                click_count: 1,
                first_mouse: true,
            };
            let target = terminal
                .path_like_target_at_event_position(&event)
                .expect("file URL should be recognized under the editor click");
            assert_eq!(target.terminal_dir, Some(PathBuf::from("/etc")));
        });
    }

    #[cfg(windows)]
    #[gpui::test]
    async fn editor_click_marks_wsl_paths_without_a_reported_directory(cx: &mut TestAppContext) {
        let terminal =
            init_ctrl_click_hyperlink_test(cx, b"Visit file:///etc/ssh/sshd_config for more\r\n");

        terminal.update(cx, |terminal, _| {
            terminal.template.shell = Shell::Program(r"C:\Windows\System32\wsl.exe".to_owned());
            let event = MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(80.0), px(10.0)),
                modifiers: Modifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                },
                click_count: 1,
                first_mouse: true,
            };
            let target = terminal
                .path_like_target_at_event_position(&event)
                .expect("file URL should be recognized under the editor click");
            assert_eq!(target.maybe_path, "/etc/ssh/sshd_config");
            assert_eq!(target.terminal_dir, None);
            assert_eq!(target.path_style, PathStyle::Unix);
        });
    }

    #[cfg(windows)]
    #[gpui::test]
    async fn editor_click_dispatches_unreported_wsl_relative_paths(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"README.md\r\n");

        terminal.update(cx, |terminal, _| {
            terminal.template.shell = Shell::Program(r"C:\Windows\System32\wsl.exe".to_owned());
            let zetta_command = zetta_command_for_shell(&terminal.template.shell).unwrap();
            terminal.open_path_in_editor_with_path_style(Path::new("README.md"), PathStyle::Unix);
            assert_eq!(
                terminal.take_pty_write_log(),
                vec![
                    format!(
                        "{zetta_command} edit -- \"$(wslpath -w \"$(pwd -P)/$(printf %s README.md)\")\""
                    )
                    .into_bytes(),
                    b"\r".to_vec(),
                ]
            );
        });
    }

    #[gpui::test]
    async fn editor_scrollback_command_submits_enter_separately(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"scrollback\r\n");

        terminal.update(cx, |terminal, _| {
            terminal.open_temporary_path_in_editor(Path::new("/tmp/zetta-scrollback.txt"));
            assert_eq!(
                terminal.take_pty_write_log(),
                vec![
                    b"zetta edit --delete-after -- /tmp/zetta-scrollback.txt".to_vec(),
                    b"\r".to_vec(),
                ]
            );
        });
    }

    #[gpui::test]
    async fn test_hyperlink_ctrl_click_same_position_in_mouse_mode(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"Visit https://zed.dev/ for more\r\n");

        terminal.update(cx, |terminal, cx| {
            terminal.last_content.mode = Modes::MOUSE_MODE;

            let click_position = point(px(80.0), px(10.0));
            ctrl_mouse_down_at(terminal, click_position, cx);
            ctrl_mouse_up_at(terminal, click_position, cx);

            assert!(
                terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::ProcessHyperlink(_, true))),
                "Should have ProcessHyperlink event when ctrl+clicking on same hyperlink position in mouse mode"
            );
            assert!(
                terminal.take_pty_write_log().is_empty(),
                "a consumed link click must not be reported to the PTY"
            );
        });
    }

    #[gpui::test]
    async fn test_hyperlink_ctrl_click_mismatch_in_mouse_mode_consumes_gesture(
        cx: &mut TestAppContext,
    ) {
        let terminal = init_ctrl_click_hyperlink_test(
            cx,
            b"Visit https://zed.dev/ for more\r\nThis is another line\r\n",
        );

        terminal.update(cx, |terminal, cx| {
            terminal.last_content.mode = Modes::MOUSE_MODE;
            terminal.take_pty_write_log();

            let down_position = point(px(80.0), px(10.0));
            let up_position = point(px(10.0), px(30.0));

            ctrl_mouse_down_at(terminal, down_position, cx);
            terminal.mouse_move(
                &MouseMoveEvent {
                    position: up_position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::secondary_key(),
                },
                cx,
            );
            ctrl_mouse_up_at(terminal, up_position, cx);

            assert!(
                !terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::ProcessHyperlink(_, _))),
                "Should NOT open a link when press and release land on different hyperlinks"
            );
            let pty_writes = terminal.take_pty_write_log();
            assert!(
                pty_writes.is_empty(),
                "a captured press must consume the whole gesture, but reports leaked to the PTY: {pty_writes:?}"
            );
        });
    }

    #[gpui::test]
    async fn test_plain_click_on_hyperlink_in_mouse_mode_is_reported(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"Visit https://zed.dev/ for more\r\n");

        terminal.update(cx, |terminal, cx| {
            terminal.last_content.mode = Modes::MOUSE_MODE;
            terminal.take_pty_write_log();

            let click_position = point(px(80.0), px(10.0));
            left_mouse_down_at(terminal, click_position, cx);
            left_mouse_up_at(terminal, click_position, cx);

            assert!(
                !terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::ProcessHyperlink(_, _))),
                "a plain click must not open a link"
            );
            let pty_writes = terminal.take_pty_write_log();
            assert_eq!(
                pty_writes.len(),
                2,
                "expected press and release reports, got {pty_writes:?}"
            );
        });
    }

    #[gpui::test]
    async fn test_ctrl_click_on_non_hyperlink_in_mouse_mode_is_reported(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"Visit https://zed.dev/ for more\r\n");

        terminal.update(cx, |terminal, cx| {
            terminal.last_content.mode = Modes::MOUSE_MODE;
            terminal.take_pty_write_log();

            // Past the end of the line: nothing link-like under the cursor.
            let click_position = point(px(370.0), px(10.0));
            ctrl_mouse_down_at(terminal, click_position, cx);
            ctrl_mouse_up_at(terminal, click_position, cx);

            assert!(
                !terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::ProcessHyperlink(_, _))),
                "a secondary click off a link must not open anything"
            );
            let pty_writes = terminal.take_pty_write_log();
            assert_eq!(
                pty_writes.len(),
                2,
                "expected press and release reports, got {pty_writes:?}"
            );
        });
    }

    #[gpui::test]
    async fn test_ctrl_click_in_mouse_mode_forwards_when_setting_disabled(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"Visit https://zed.dev/ for more\r\n");

        cx.update_global(|store: &mut settings::SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings
                    .terminal
                    .get_or_insert_default()
                    .open_links_in_mouse_mode = Some(false);
            });
        });

        terminal.update(cx, |terminal, cx| {
            terminal.last_content.mode = Modes::MOUSE_MODE;

            let click_position = point(px(80.0), px(10.0));
            ctrl_mouse_down_at(terminal, click_position, cx);
            ctrl_mouse_up_at(terminal, click_position, cx);

            assert!(
                !terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::ProcessHyperlink(_, _))),
                "with the setting disabled, ctrl+click must not open links in mouse mode"
            );
            let pty_writes = terminal.take_pty_write_log();
            assert_eq!(
                pty_writes.len(),
                2,
                "expected press and release reports, got {pty_writes:?}"
            );
        });
    }

    #[gpui::test]
    async fn test_hyperlink_ctrl_click_drag_outside_bounds(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(
            cx,
            b"Visit https://zed.dev/ for more\r\nThis is another line\r\n",
        );

        terminal.update(cx, |terminal, cx| {
            let down_position = point(px(80.0), px(10.0));
            let up_position = point(px(10.0), px(50.0));

            ctrl_mouse_down_at(terminal, down_position, cx);
            ctrl_mouse_move_to(terminal, up_position, cx);
            ctrl_mouse_up_at(terminal, up_position, cx);

            assert!(
                !terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::ProcessHyperlink(_, _))),
                "Should NOT have ProcessHyperlink event when dragging outside the hyperlink"
            );
        });
    }

    #[gpui::test]
    async fn test_hyperlink_ctrl_click_drag_within_bounds(cx: &mut TestAppContext) {
        let terminal = init_ctrl_click_hyperlink_test(cx, b"Visit https://zed.dev/ for more\r\n");

        terminal.update(cx, |terminal, cx| {
            let down_position = point(px(70.0), px(10.0));
            let up_position = point(px(130.0), px(10.0));

            ctrl_mouse_down_at(terminal, down_position, cx);
            ctrl_mouse_move_to(terminal, up_position, cx);
            ctrl_mouse_up_at(terminal, up_position, cx);

            assert!(
                terminal
                    .events
                    .iter()
                    .any(|event| matches!(event, InternalEvent::ProcessHyperlink(_, true))),
                "Should have ProcessHyperlink event when dragging within hyperlink bounds"
            );
        });
    }

    /// Polls the terminal content until `expected` appears, or panics after ~1s.
    /// The PTY IO thread writes into the terminal grid independently of the
    /// GPUI executor, so we need a real-time polling loop to synchronize.
    async fn assert_content_eventually(
        terminal: &Entity<Terminal>,
        expected: &str,
        cx: &mut TestAppContext,
    ) {
        let mut content = String::new();
        for _ in 0..100 {
            content = terminal.update(cx, |term, _| term.get_content());
            if content.contains(expected) {
                return;
            }
            cx.background_executor
                .timer(Duration::from_millis(10))
                .await;
        }
        panic!("Expected terminal content to contain {expected:?}, got: {content}");
    }

    #[cfg(unix)]
    async fn assert_foreground_process_command_eventually(
        terminal: &Entity<Terminal>,
        expected: &str,
        cx: &mut TestAppContext,
    ) {
        // Spawning a shell and letting it fork the command is at the mercy of
        // whatever else the machine is doing, and this returns as soon as the
        // command appears, so the budget only needs to be generous enough that a
        // loaded machine is not mistaken for a broken one.
        const ATTEMPTS: usize = 500;

        let mut command_name = None;
        for _ in 0..ATTEMPTS {
            terminal.update(cx, |terminal, _| {
                if let TerminalType::Pty { info, .. } = &terminal.terminal_type {
                    info.load_for_test();
                }
            });
            command_name =
                terminal.update(cx, |terminal, _| terminal.foreground_process_command_name());
            if command_name.as_deref() == Some(expected) {
                return;
            }
            cx.background_executor
                .timer(Duration::from_millis(10))
                .await;
        }
        let process_info = terminal.update(cx, |terminal, _| match &terminal.terminal_type {
            TerminalType::Pty { info, .. } => format!(
                "pid={:?}, fallback_pid={:?}, has_current_info={}",
                info.pid(),
                info.pid_getter().fallback_pid(),
                info.current.read().is_some()
            ),
            TerminalType::DisplayOnly => "display-only".to_string(),
        });
        panic!(
            "Expected foreground process command name to be {expected:?}, got {command_name:?}; process info: {process_info:?}"
        );
    }

    /// Test that kill_active_task properly terminates both the foreground process
    /// and the shell, allowing wait_for_completed_task to complete and output to be captured.
    #[cfg(unix)]
    #[gpui::test]
    async fn test_kill_active_task_completes_and_captures_output(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        // Run a command that prints output then sleeps for a long time
        // The echo ensures we have output to capture before killing
        let (terminal, completion_rx) =
            build_test_terminal(cx, "echo", &["test_output_before_kill; sleep 60"]).await;

        assert_content_eventually(&terminal, "test_output_before_kill", cx).await;

        // Kill the active task
        terminal.update(cx, |term, _cx| {
            term.kill_active_task();
        });

        // wait_for_completed_task should complete within a reasonable time (not hang)
        let completion_result = completion_rx.recv().await;
        assert!(
            completion_result.is_ok(),
            "wait_for_completed_task should complete after kill_active_task, but it timed out"
        );

        // The exit status should indicate the process was killed (not a clean exit)
        let exit_status = completion_result.unwrap();
        assert!(
            exit_status.is_some(),
            "Should have received an exit status after killing"
        );

        // Verify that output captured before killing is still available
        let content = terminal.update(cx, |term, _| term.get_content());
        assert!(
            content.contains("test_output_before_kill"),
            "Output from before kill should be captured, got: {content}"
        );
    }

    /// Test that kill_active_task on a task that's not running is a no-op
    /// A pane stays open after its child ends so its output stays readable, and
    /// it used to keep the whole pty event loop alive with it: the loop thread
    /// returns its `EventLoop` and an un-joined `JoinHandle` keeps that value
    /// alive, so the pty master descriptor, the poller and the loop's buffers
    /// were all retained until the pane itself was closed.
    #[gpui::test]
    async fn test_exited_terminal_releases_its_pty_resources(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let (terminal, completion_rx) = build_test_terminal(cx, "echo", &["released"]).await;
        completion_rx
            .recv()
            .await
            .expect("Should receive exit status");

        terminal.update(cx, |terminal, _| {
            assert!(
                terminal.child_process_ended,
                "the child ending is what releases the pty"
            );
            let TerminalType::Pty { pty_tx, io, info } = &terminal.terminal_type else {
                panic!("an exited pty pane stays a pty pane");
            };
            assert!(pty_tx.is_none(), "the pty sender should be released");
            assert!(io.is_none(), "the pty event loop should be released");
            assert!(
                terminal.pty_control.is_none(),
                "the local control holds a reference to the loop's poller"
            );
            #[cfg(unix)]
            assert!(
                !info.pty_handle_is_open(),
                "the borrowed pty master descriptor must not be read after it is closed"
            );
            let _ = info;
        });
    }

    #[gpui::test]
    async fn test_kill_active_task_on_completed_task_is_noop(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        // Run a command that exits immediately
        let (terminal, completion_rx) = build_test_terminal(cx, "echo", &["done"]).await;

        // Wait for the command to complete naturally
        let exit_status = completion_rx
            .recv()
            .await
            .expect("Should receive exit status");
        assert_eq!(exit_status, Some(ExitStatus::default()));

        assert_content_eventually(&terminal, "done", cx).await;

        // Now try to kill - should be a no-op since task already completed
        terminal.update(cx, |term, _cx| {
            term.kill_active_task();
        });

        // Content should still be there
        let content = terminal.update(cx, |term, _| term.get_content());
        assert!(
            content.contains("done"),
            "Output should still be present after no-op kill, got: {content}"
        );
    }

    mod perf {
        use super::super::*;
        use gpui::{
            Entity, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualContext,
            VisualTestContext, point,
        };
        use util::default;
        use util_macros::perf;

        async fn init_scroll_perf_test(
            cx: &mut TestAppContext,
        ) -> (Entity<Terminal>, &mut VisualTestContext) {
            cx.update(|cx| {
                let settings_store = settings::SettingsStore::test(cx);
                cx.set_global(settings_store);
            });

            cx.executor().allow_parking();

            let window = cx.add_empty_window();
            let builder = window
                .update(|window, cx| {
                    let settings = TerminalSettings::get_global(cx);
                    let test_path_hyperlink_timeout_ms = 100;
                    TerminalBuilder::new(
                        None,
                        None,
                        task::Shell::System,
                        HashMap::default(),
                        SettingsCursorShape::default(),
                        AlternateScroll::On,
                        None,
                        settings.path_hyperlink_regexes.clone(),
                        test_path_hyperlink_timeout_ms,
                        false,
                        window.window_handle().window_id().as_u64(),
                        None,
                        cx,
                        vec![],
                        PathStyle::local(),
                        None,
                    )
                })
                .await
                .unwrap();
            let terminal = window.new(|cx| builder.subscribe(cx));

            terminal.update(window, |term, cx| {
                term.write_output("long line ".repeat(1000).as_bytes(), cx);
            });

            (terminal, window)
        }

        #[perf]
        #[gpui::test]
        async fn scroll_long_line_benchmark(cx: &mut TestAppContext) {
            let (terminal, window) = init_scroll_perf_test(cx).await;
            let wobble = point(FIND_HYPERLINK_THROTTLE_PX, px(0.0));
            let mut scroll_by = |lines: i32| {
                window.update_window_entity(&terminal, |terminal, window, cx| {
                    let bounds = terminal.last_content.terminal_bounds.bounds;
                    let center = bounds.origin + bounds.center();
                    let position = center + wobble * lines as f32;

                    terminal.mouse_move(
                        &MouseMoveEvent {
                            position,
                            ..default()
                        },
                        cx,
                    );

                    terminal.scroll_wheel(
                        &ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Lines(GpuiPoint::new(0.0, lines as f32)),
                            ..default()
                        },
                        1.0,
                    );

                    assert!(
                        terminal
                            .events
                            .iter()
                            .any(|event| matches!(event, InternalEvent::Scroll(_))),
                        "Should have Scroll event when scrolling within terminal bounds"
                    );
                    terminal.sync(window, cx);
                });
            };

            for _ in 0..20000 {
                scroll_by(1);
                scroll_by(-1);
            }
        }

        #[test]
        fn test_num_lines_float_precision() {
            let line_heights = [
                20.1f32, 16.7, 18.3, 22.9, 14.1, 15.6, 17.8, 19.4, 21.3, 23.7,
            ];
            for &line_height in &line_heights {
                for n in 1..=100 {
                    let height = n as f32 * line_height;
                    let bounds = TerminalBounds::new(
                        px(line_height),
                        px(8.0),
                        Bounds {
                            origin: GpuiPoint::default(),
                            size: Size {
                                width: px(800.0),
                                height: px(height),
                            },
                        },
                    );
                    assert_eq!(
                        bounds.num_lines(),
                        n,
                        "num_lines() should be {n} for height={height}, line_height={line_height}"
                    );
                }
            }
        }

        #[test]
        fn test_num_columns_float_precision() {
            let cell_widths = [8.1f32, 7.3, 9.7, 6.9, 10.1];
            for &cell_width in &cell_widths {
                for n in 1..=200 {
                    let width = n as f32 * cell_width;
                    let bounds = TerminalBounds::new(
                        px(20.0),
                        px(cell_width),
                        Bounds {
                            origin: GpuiPoint::default(),
                            size: Size {
                                width: px(width),
                                height: px(400.0),
                            },
                        },
                    );
                    assert_eq!(
                        bounds.num_columns(),
                        n,
                        "num_columns() should be {n} for width={width}, cell_width={cell_width}"
                    );
                }
            }
        }
    }
}
