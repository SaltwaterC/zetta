#[cfg(target_os = "windows")]
use std::num::NonZeroU32;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::{
    borrow::Cow,
    io, mem,
    ops::RangeInclusive,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

mod hyperlinks;

use alacritty_terminal::{
    event::{Event as AlacTermEvent, EventListener, Notify, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::{Dimensions, Grid, GridIterator, Row, Scroll as AlacScroll},
    index::{Boundary, Column, Direction as AlacDirection, Line, Point as AlacPoint},
    selection::{
        Selection as AlacSelection, SelectionRange as AlacSelectionRange,
        SelectionType as AlacSelectionType,
    },
    sync::FairMutex,
    term::{
        Config, Osc52, RenderableCursor, Term, TermMode,
        cell::{Cell as AlacCell, Flags, Hyperlink as AlacHyperlink, LineLength},
        search::{RegexIter, RegexSearch},
    },
    tty,
    vi_mode::{ViModeCursor, ViMotion as AlacViMotion},
    vte::ansi::{
        ClearMode, CursorShape as AlacCursorShape, CursorStyle as AlacCursorStyle,
        NamedPrivateMode, PrivateMode,
    },
};
use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use util::paths::PathStyle;
use vte::ansi::Handler;
#[cfg(target_os = "windows")]
use windows::Win32::{Foundation::HANDLE, System::Threading::GetProcessId};

use crate::{
    Cell, Color, Content, Cursor, CursorShape, Hyperlink, HyperlinkData, IndexedCell, Modes, Point,
    PtyEvent, Range, RenderableCells, Scroll, Search, Selection, SelectionRange, SelectionSide,
    SelectionType, TerminalBackendEvent, TerminalBounds, ViMotion,
    pty_info::ProcessIdGetter,
    terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape},
};

pub(super) use hyperlinks::{HyperlinkMatch, RegexSearches};

pub(super) type AlacrittyPty = tty::Pty;
pub(super) type AlacrittyTerm = Term<ZedListener>;
pub(super) type AlacrittyTermConfig = Config;
pub(super) type AlacrittyTermLock = FairMutex<AlacrittyTerm>;
pub(super) type AlacrittyCell = AlacCell;
pub(super) type AlacrittyGridIterator<'a> = GridIterator<'a, AlacCell>;
pub(super) type AlacrittyHyperlink = AlacHyperlink;

const HIDDEN_TERMINAL_READ_PAUSE: Duration = Duration::from_millis(8);

#[derive(Clone)]
pub(super) struct WakeupGate(Arc<AtomicBool>);

impl WakeupGate {
    pub(super) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }

    pub(super) fn set_enabled(&self, enabled: bool) -> bool {
        self.0.swap(enabled, Ordering::AcqRel)
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub(super) struct ZedListener {
    events_tx: UnboundedSender<PtyEvent>,
    wakeup_gate: WakeupGate,
}

impl ZedListener {
    pub(super) fn new(events_tx: UnboundedSender<PtyEvent>, wakeup_gate: WakeupGate) -> Self {
        Self {
            events_tx,
            wakeup_gate,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct AlacrittySearch {
    search: RegexSearch,
}

#[cfg(unix)]
impl From<&AlacrittyPty> for ProcessIdGetter {
    fn from(pty: &AlacrittyPty) -> Self {
        Self::new(pty.file().as_raw_fd(), pty.child().id())
    }
}

#[cfg(windows)]
impl From<&AlacrittyPty> for ProcessIdGetter {
    fn from(pty: &AlacrittyPty) -> Self {
        let child = pty.child_watcher();
        let handle = child.raw_handle();
        let fallback_pid = child.pid().unwrap_or_else(|| unsafe {
            NonZeroU32::new_unchecked(GetProcessId(HANDLE(handle as _)))
        });

        Self::new(handle as i32, u32::from(fallback_pid))
    }
}

pub(super) struct PtySender {
    notifier: Notifier,
}

impl PtySender {
    pub(super) fn notify(&self, input: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(input);
    }

    pub(super) fn resize(&self, bounds: TerminalBounds) {
        if let Err(error) = self
            .notifier
            .0
            .send(Msg::Resize(window_size_from_terminal_bounds(bounds)))
        {
            log::error!("failed to resize alacritty pty: {error}");
        }
    }

    pub(super) fn shutdown(&self) {
        if let Err(error) = self.notifier.0.send(Msg::Shutdown) {
            log::debug!("failed to shut down alacritty pty loop: {error}");
        }
    }
}

fn window_size_from_terminal_bounds(bounds: TerminalBounds) -> WindowSize {
    WindowSize {
        num_lines: bounds.num_lines() as u16,
        num_cols: bounds.num_columns() as u16,
        cell_width: f32::from(bounds.cell_width()) as u16,
        cell_height: f32::from(bounds.line_height()) as u16,
    }
}

pub(super) fn display_only_term_config(
    scrolling_history: usize,
    cursor_shape: SettingsCursorShape,
) -> AlacrittyTermConfig {
    Config {
        scrolling_history,
        default_cursor_style: alacritty_cursor_style(cursor_shape),
        osc52: Osc52::Disabled,
        ..Config::default()
    }
}

pub(super) fn pty_term_config(
    scrolling_history: usize,
    cursor_shape: SettingsCursorShape,
) -> AlacrittyTermConfig {
    Config {
        scrolling_history,
        default_cursor_style: alacritty_cursor_style(cursor_shape),
        ..Config::default()
    }
}

pub(super) fn set_default_cursor_style(
    config: &mut AlacrittyTermConfig,
    cursor_shape: SettingsCursorShape,
) {
    config.default_cursor_style = alacritty_cursor_style(cursor_shape);
}

pub(super) fn apply_config(term: &AlacrittyTermLock, config: &AlacrittyTermConfig) {
    term.lock().set_options(config.clone());
}

#[cfg(not(windows))]
pub(super) fn current_child_signal_mask() -> io::Result<tty::SignalMask> {
    tty::SignalMask::current()
}

pub(super) fn pty_options(
    shell: Option<(String, Vec<String>)>,
    working_directory: Option<PathBuf>,
    env: impl IntoIterator<Item = (String, String)>,
    #[cfg(not(windows))] child_signal_mask: Option<tty::SignalMask>,
    #[cfg(windows)] escape_args: bool,
) -> tty::Options {
    tty::Options {
        shell: shell.map(|(program, args)| tty::Shell::new(program, args)),
        working_directory,
        drain_on_exit: true,
        env: env.into_iter().collect(),
        #[cfg(not(windows))]
        child_signal_mask,
        #[cfg(windows)]
        escape_args,
    }
}

pub(super) fn open_pty(
    options: &tty::Options,
    bounds: TerminalBounds,
    window_id: u64,
) -> io::Result<AlacrittyPty> {
    tty::new(options, window_size_from_terminal_bounds(bounds), window_id)
}

pub(super) fn new_term(
    config: &AlacrittyTermConfig,
    bounds: TerminalBounds,
    listener: ZedListener,
    alternate_scroll: AlternateScroll,
) -> Arc<AlacrittyTermLock> {
    let mut term = Term::new(config.clone(), &bounds, listener);

    if let AlternateScroll::Off = alternate_scroll {
        term.unset_private_mode(PrivateMode::Named(NamedPrivateMode::AlternateScroll));
    }

    Arc::new(FairMutex::new(term))
}

pub(super) fn spawn_event_loop(
    term: Arc<AlacrittyTermLock>,
    listener: ZedListener,
    pty: AlacrittyPty,
    drain_on_exit: bool,
) -> Result<PtySender> {
    let event_loop = EventLoop::new(term, listener, pty, drain_on_exit, false)
        .context("failed to create event loop")?;
    let pty_tx = event_loop.channel();
    let _io_thread = event_loop.spawn();

    Ok(PtySender {
        notifier: Notifier(pty_tx),
    })
}

pub(super) fn resize(term: &mut AlacrittyTerm, bounds: TerminalBounds, reflow: bool) {
    if reflow || term.mode().contains(TermMode::ALT_SCREEN) {
        term.resize(bounds);
        return;
    }

    // Alacritty always reflows the primary grid. For discrete layout changes this can move a
    // live multi-line shell prompt before the shell handles SIGWINCH, so its redraw uses a stale
    // cursor-relative line and leaves duplicated prompt fragments behind. Resize the primary
    // grid without reflow, while still letting Term::resize update its inactive grid, tab stops,
    // selection, scroll region, and damage state.
    let old_lines = term.screen_lines();
    let old_columns = term.columns();
    let mut placeholder = Grid::new(old_lines, old_columns, 0);
    placeholder.cursor = term.grid().cursor.clone();
    placeholder.saved_cursor = term.grid().saved_cursor.clone();
    let mut grid = mem::replace(term.grid_mut(), placeholder);
    grid.resize(false, bounds.num_lines(), bounds.num_columns());
    term.resize(bounds);
    *term.grid_mut() = grid;
}

pub(super) fn display_offset(term: &AlacrittyTerm) -> usize {
    term.grid().display_offset()
}

pub(super) fn scroll_display(term: &mut AlacrittyTerm, scroll: Scroll) {
    term.scroll_display(scroll.to_alacritty());
}

pub(super) fn set_selection(term: &mut AlacrittyTerm, selection: Option<&Selection>) {
    term.selection = selection.map(Selection::to_alacritty);
}

pub(super) fn update_selection(
    term: &mut AlacrittyTerm,
    point: Point,
    side: SelectionSide,
) -> bool {
    let Some(mut selection) = term.selection.take() else {
        return false;
    };
    selection.update(point.to_alacritty(), side.to_alacritty());
    term.selection = Some(selection);
    true
}

pub(super) fn selection_text(term: &AlacrittyTerm) -> Option<String> {
    term.selection_to_string()
}

pub(super) fn scroll_to_point(term: &mut AlacrittyTerm, point: Point) {
    term.scroll_to_point(point.to_alacritty());
}

pub(super) fn vi_goto_point(term: &mut AlacrittyTerm, point: Point) {
    term.vi_goto_point(point.to_alacritty());
}

pub(super) fn toggle_vi_mode(term: &mut AlacrittyTerm) {
    term.toggle_vi_mode();
}

pub(super) fn vi_motion(term: &mut AlacrittyTerm, motion: ViMotion) {
    term.vi_motion(motion.to_alacritty());
}

fn alacritty_cursor_style(cursor_shape: SettingsCursorShape) -> AlacCursorStyle {
    AlacCursorStyle {
        shape: alacritty_cursor_shape(cursor_shape),
        blinking: false,
    }
}

fn alacritty_cursor_shape(cursor_shape: SettingsCursorShape) -> AlacCursorShape {
    match cursor_shape {
        SettingsCursorShape::Block => AlacCursorShape::Block,
        SettingsCursorShape::Underline => AlacCursorShape::Underline,
        SettingsCursorShape::Bar => AlacCursorShape::Beam,
        SettingsCursorShape::Hollow => AlacCursorShape::HollowBlock,
    }
}

impl Dimensions for TerminalBounds {
    /// Note: this is supposed to be for the back buffer's length,
    /// but we exclusively use it to resize the terminal, which does not
    /// use this method. We still have to implement it for the trait though,
    /// hence, this comment.
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines()
    }

    fn columns(&self) -> usize {
        self.num_columns()
    }
}

impl From<AlacTermEvent> for TerminalBackendEvent {
    fn from(event: AlacTermEvent) -> Self {
        match event {
            AlacTermEvent::MouseCursorDirty => Self::MouseCursorDirty,
            AlacTermEvent::Title(title) => Self::Title(title),
            AlacTermEvent::ResetTitle => Self::ResetTitle,
            AlacTermEvent::ClipboardStore(_, data) => Self::ClipboardStore(data),
            AlacTermEvent::ClipboardLoad(_, format) => Self::ClipboardLoad(format),
            AlacTermEvent::ColorRequest(index, format) => Self::ColorRequest(index, format),
            AlacTermEvent::PtyWrite(output) => Self::PtyWrite(output),
            AlacTermEvent::TextAreaSizeRequest(format) => {
                Self::TextAreaSizeRequest(Arc::new(move |bounds| {
                    format(window_size_from_terminal_bounds(bounds))
                }))
            }
            AlacTermEvent::CursorBlinkingChange => Self::CursorBlinkingChange,
            AlacTermEvent::Wakeup => Self::Wakeup,
            AlacTermEvent::Bell => Self::Bell,
            AlacTermEvent::Exit => Self::Exit,
            AlacTermEvent::ChildExit(status) => Self::ChildExit(status),
        }
    }
}

impl EventListener for ZedListener {
    fn send_event(&self, event: AlacTermEvent) {
        if matches!(event, AlacTermEvent::Wakeup) && !self.wakeup_gate.is_enabled() {
            // Apply backpressure between read batches once a terminal is hidden. Without this,
            // a benchmark-producing background tab can monopolize a CPU core and the allocator
            // even though none of its redraws reach the UI.
            thread::sleep(HIDDEN_TERMINAL_READ_PAUSE);
            return;
        }
        self.events_tx
            .unbounded_send(PtyEvent::Event(event.into()))
            .ok();
    }
}

impl Scroll {
    fn to_alacritty(self) -> AlacScroll {
        match self {
            Self::Delta(delta) => AlacScroll::Delta(delta),
            Self::PageUp => AlacScroll::PageUp,
            Self::PageDown => AlacScroll::PageDown,
            Self::Top => AlacScroll::Top,
            Self::Bottom => AlacScroll::Bottom,
        }
    }
}

impl ViMotion {
    fn to_alacritty(self) -> AlacViMotion {
        match self {
            Self::Up => AlacViMotion::Up,
            Self::Down => AlacViMotion::Down,
            Self::Left => AlacViMotion::Left,
            Self::Right => AlacViMotion::Right,
            Self::First => AlacViMotion::First,
            Self::Last => AlacViMotion::Last,
            Self::FirstOccupied => AlacViMotion::FirstOccupied,
            Self::High => AlacViMotion::High,
            Self::Middle => AlacViMotion::Middle,
            Self::Low => AlacViMotion::Low,
            Self::WordLeft => AlacViMotion::WordLeft,
            Self::WordRight => AlacViMotion::WordRight,
            Self::WordRightEnd => AlacViMotion::WordRightEnd,
            Self::Bracket => AlacViMotion::Bracket,
            Self::ParagraphUp => AlacViMotion::ParagraphUp,
            Self::ParagraphDown => AlacViMotion::ParagraphDown,
        }
    }
}

impl Search {
    pub fn new(search: &str) -> Option<Self> {
        Some(Self {
            search: AlacrittySearch {
                search: RegexSearch::new(search).ok()?,
            },
            literal: None,
        })
    }

    pub fn new_literal(search: &str) -> Option<Self> {
        let mut result = Self::new(&regex::escape(search))?;
        result.literal = (!search.is_empty() && search.is_ascii()).then(|| search.to_owned());
        Some(result)
    }

    fn into_alacritty(self) -> (RegexSearch, Option<String>) {
        (self.search.search, self.literal)
    }
}

impl SelectionSide {
    fn to_alacritty(self) -> AlacDirection {
        match self {
            Self::Left => AlacDirection::Left,
            Self::Right => AlacDirection::Right,
        }
    }
}

impl SelectionType {
    fn to_alacritty(self) -> AlacSelectionType {
        match self {
            Self::Simple => AlacSelectionType::Simple,
            Self::Semantic => AlacSelectionType::Semantic,
            Self::Lines => AlacSelectionType::Lines,
        }
    }
}

impl Selection {
    fn to_alacritty(&self) -> AlacSelection {
        let mut selection = AlacSelection::new(
            self.ty.to_alacritty(),
            self.start.point.to_alacritty(),
            self.start.side.to_alacritty(),
        );
        if self.start.point != self.end.point || self.start.side != self.end.side {
            selection.update(self.end.point.to_alacritty(), self.end.side.to_alacritty());
        }
        selection
    }
}

impl Hyperlink {
    pub fn new<T: ToString>(id: Option<T>, uri: String) -> Self {
        Self {
            data: HyperlinkData::Owned {
                id: id.map(|id| Arc::from(id.to_string())),
                uri: Arc::from(uri),
            },
        }
    }

    pub fn id(&self) -> Option<&str> {
        match &self.data {
            HyperlinkData::Alacritty(hyperlink) => Some(hyperlink.id()),
            HyperlinkData::Owned { id, .. } => id.as_deref(),
        }
    }

    pub fn uri(&self) -> &str {
        match &self.data {
            HyperlinkData::Alacritty(hyperlink) => hyperlink.uri(),
            HyperlinkData::Owned { uri, .. } => uri,
        }
    }

    fn from_alacritty(hyperlink: AlacHyperlink) -> Self {
        Self {
            data: HyperlinkData::Alacritty(hyperlink),
        }
    }
}

fn terminal_hyperlink_from_alacritty(hyperlink: AlacHyperlink) -> Hyperlink {
    Hyperlink::from_alacritty(hyperlink)
}

impl From<Hyperlink> for AlacHyperlink {
    fn from(hyperlink: Hyperlink) -> Self {
        match hyperlink.data {
            HyperlinkData::Alacritty(hyperlink) => hyperlink,
            HyperlinkData::Owned { id, uri } => Self::new(id.as_deref(), uri.to_string()),
        }
    }
}

fn terminal_cell_from_alacritty(cell: &AlacCell) -> Cell {
    Cell { cell: cell.clone() }
}

impl Cell {
    #[inline]
    pub fn character(&self) -> char {
        self.cell.c
    }

    #[cfg(test)]
    pub(crate) fn set_character(&mut self, character: char) {
        self.cell.c = character;
    }

    #[inline]
    pub fn foreground(&self) -> Color {
        self.cell.fg
    }

    #[inline]
    pub fn background(&self) -> Color {
        self.cell.bg
    }

    #[inline]
    pub fn zerowidth(&self) -> Option<&[char]> {
        self.cell.zerowidth()
    }

    #[cfg(test)]
    pub(crate) fn push_zerowidth(&mut self, character: char) {
        self.cell.push_zerowidth(character);
    }

    #[inline]
    pub fn hyperlink(&self) -> Option<Hyperlink> {
        self.cell.hyperlink().map(terminal_hyperlink_from_alacritty)
    }

    #[inline]
    pub fn is_inverse(&self) -> bool {
        self.cell.flags.contains(Flags::INVERSE)
    }

    #[inline]
    pub fn is_wide_char_spacer(&self) -> bool {
        self.cell.flags.contains(Flags::WIDE_CHAR_SPACER)
    }

    #[inline]
    pub fn is_dim(&self) -> bool {
        self.cell.flags.intersects(Flags::DIM)
    }

    #[inline]
    pub fn has_underline(&self) -> bool {
        self.cell.flags.intersects(Flags::ALL_UNDERLINES)
    }

    #[inline]
    pub fn has_undercurl(&self) -> bool {
        self.cell.flags.contains(Flags::UNDERCURL)
    }

    #[inline]
    pub fn has_strikeout(&self) -> bool {
        self.cell.flags.intersects(Flags::STRIKEOUT)
    }

    #[inline]
    pub fn is_bold(&self) -> bool {
        self.cell.flags.intersects(Flags::BOLD)
    }

    #[inline]
    pub fn is_italic(&self) -> bool {
        self.cell.flags.intersects(Flags::ITALIC)
    }

    #[inline]
    pub fn has_visible_style_modifier(&self) -> bool {
        self.cell
            .flags
            .intersects(Flags::ALL_UNDERLINES | Flags::INVERSE | Flags::STRIKEOUT)
    }
}

impl<'a> RenderableCells<'a> {
    pub(super) fn new(cells: GridIterator<'a, AlacCell>) -> Self {
        Self { cells }
    }
}

impl Iterator for RenderableCells<'_> {
    type Item = IndexedCell;

    fn next(&mut self) -> Option<Self::Item> {
        self.cells.next().map(|cell| IndexedCell {
            point: terminal_point_from_alacritty(cell.point),
            cell: terminal_cell_from_alacritty(cell.cell),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.cells.size_hint()
    }
}

impl Modes {
    #[cfg(test)]
    fn to_alacritty(self) -> TermMode {
        let mut mode = TermMode::empty();
        add_alacritty_mode(&mut mode, self, Self::APP_CURSOR, TermMode::APP_CURSOR);
        add_alacritty_mode(&mut mode, self, Self::APP_KEYPAD, TermMode::APP_KEYPAD);
        add_alacritty_mode(&mut mode, self, Self::SHOW_CURSOR, TermMode::SHOW_CURSOR);
        add_alacritty_mode(&mut mode, self, Self::LINE_WRAP, TermMode::LINE_WRAP);
        add_alacritty_mode(&mut mode, self, Self::ORIGIN, TermMode::ORIGIN);
        add_alacritty_mode(&mut mode, self, Self::INSERT, TermMode::INSERT);
        add_alacritty_mode(
            &mut mode,
            self,
            Self::LINE_FEED_NEW_LINE,
            TermMode::LINE_FEED_NEW_LINE,
        );
        add_alacritty_mode(&mut mode, self, Self::FOCUS_IN_OUT, TermMode::FOCUS_IN_OUT);
        add_alacritty_mode(
            &mut mode,
            self,
            Self::ALTERNATE_SCROLL,
            TermMode::ALTERNATE_SCROLL,
        );
        add_alacritty_mode(
            &mut mode,
            self,
            Self::BRACKETED_PASTE,
            TermMode::BRACKETED_PASTE,
        );
        add_alacritty_mode(&mut mode, self, Self::SGR_MOUSE, TermMode::SGR_MOUSE);
        add_alacritty_mode(&mut mode, self, Self::UTF8_MOUSE, TermMode::UTF8_MOUSE);
        add_alacritty_mode(&mut mode, self, Self::ALT_SCREEN, TermMode::ALT_SCREEN);
        add_alacritty_mode(
            &mut mode,
            self,
            Self::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_REPORT_CLICK,
        );
        add_alacritty_mode(&mut mode, self, Self::MOUSE_DRAG, TermMode::MOUSE_DRAG);
        add_alacritty_mode(&mut mode, self, Self::MOUSE_MOTION, TermMode::MOUSE_MOTION);
        add_alacritty_mode(&mut mode, self, Self::VI, TermMode::VI);
        mode
    }
}

fn terminal_modes_from_alacritty(mode: TermMode) -> Modes {
    let mut terminal_modes = Modes::empty();
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::APP_CURSOR,
        Modes::APP_CURSOR,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::APP_KEYPAD,
        Modes::APP_KEYPAD,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::SHOW_CURSOR,
        Modes::SHOW_CURSOR,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::LINE_WRAP,
        Modes::LINE_WRAP,
    );
    add_terminal_mode(&mut terminal_modes, mode, TermMode::ORIGIN, Modes::ORIGIN);
    add_terminal_mode(&mut terminal_modes, mode, TermMode::INSERT, Modes::INSERT);
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::LINE_FEED_NEW_LINE,
        Modes::LINE_FEED_NEW_LINE,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::FOCUS_IN_OUT,
        Modes::FOCUS_IN_OUT,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::ALTERNATE_SCROLL,
        Modes::ALTERNATE_SCROLL,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::BRACKETED_PASTE,
        Modes::BRACKETED_PASTE,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::SGR_MOUSE,
        Modes::SGR_MOUSE,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::UTF8_MOUSE,
        Modes::UTF8_MOUSE,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::ALT_SCREEN,
        Modes::ALT_SCREEN,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::MOUSE_REPORT_CLICK,
        Modes::MOUSE_REPORT_CLICK,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::MOUSE_DRAG,
        Modes::MOUSE_DRAG,
    );
    add_terminal_mode(
        &mut terminal_modes,
        mode,
        TermMode::MOUSE_MOTION,
        Modes::MOUSE_MOTION,
    );
    add_terminal_mode(&mut terminal_modes, mode, TermMode::VI, Modes::VI);
    terminal_modes
}

fn add_terminal_mode(
    terminal_modes: &mut Modes,
    alacritty_modes: TermMode,
    alacritty_mode: TermMode,
    terminal_mode: Modes,
) {
    if alacritty_modes.contains(alacritty_mode) {
        terminal_modes.insert(terminal_mode);
    }
}

#[cfg(test)]
fn add_alacritty_mode(
    alacritty_modes: &mut TermMode,
    terminal_modes: Modes,
    terminal_mode: Modes,
    alacritty_mode: TermMode,
) {
    if terminal_modes.contains(terminal_mode) {
        alacritty_modes.insert(alacritty_mode);
    }
}

impl Cursor {
    fn from_alacritty(cursor: RenderableCursor) -> Self {
        Self {
            shape: terminal_cursor_shape_from_alacritty(cursor.shape),
            point: terminal_point_from_alacritty(cursor.point),
        }
    }
}

fn terminal_cursor_shape_from_alacritty(shape: AlacCursorShape) -> CursorShape {
    match shape {
        AlacCursorShape::Block => CursorShape::Block,
        AlacCursorShape::Underline => CursorShape::Underline,
        AlacCursorShape::Beam => CursorShape::Bar,
        AlacCursorShape::HollowBlock => CursorShape::HollowBlock,
        AlacCursorShape::Hidden => CursorShape::Hidden,
    }
}

impl Point {
    fn to_alacritty(self) -> AlacPoint {
        AlacPoint::new(Line(self.line), Column(self.column))
    }
}

fn terminal_point_from_alacritty(point: AlacPoint) -> Point {
    Point {
        line: point.line.0,
        column: point.column.0,
    }
}

impl Range {
    #[cfg(test)]
    fn to_alacritty(self) -> RangeInclusive<AlacPoint> {
        self.start.to_alacritty()..=self.end.to_alacritty()
    }

    fn from_alacritty(range: RangeInclusive<AlacPoint>) -> Self {
        Self {
            start: terminal_point_from_alacritty(*range.start()),
            end: terminal_point_from_alacritty(*range.end()),
        }
    }
}

fn terminal_selection_range_from_alacritty(range: AlacSelectionRange) -> SelectionRange {
    SelectionRange {
        start: terminal_point_from_alacritty(range.start),
        end: terminal_point_from_alacritty(range.end),
        is_block: range.is_block,
    }
}

pub(super) fn clear_saved_screen(term: &mut Term<ZedListener>) {
    term.clear_screen(ClearMode::Saved);

    let cursor = term.grid().cursor.point;

    term.grid_mut().reset_region(..cursor.line);

    let line = term.grid()[cursor.line][..Column(term.grid().columns())]
        .iter()
        .cloned()
        .enumerate()
        .collect::<Vec<(usize, AlacCell)>>();

    for (index, cell) in line {
        term.grid_mut()[Line(0)][Column(index)] = cell;
    }

    term.grid_mut().cursor.point = AlacPoint::new(Line(0), term.grid_mut().cursor.point.column);
    let new_cursor = term.grid().cursor.point;

    if (new_cursor.line.0 as usize) < term.screen_lines() - 1 {
        term.grid_mut().reset_region((new_cursor.line + 1)..);
    }
}

pub(super) fn shrink_to_used(term: &mut Term<ZedListener>) {
    term.grid_mut().truncate();
}

pub(super) fn make_content(term: &Term<ZedListener>, last_content: &Content) -> Content {
    let content = term.renderable_content();

    let estimated_size = content.display_iter.size_hint().0;
    let mut cells = Vec::with_capacity(estimated_size);

    cells.extend(content.display_iter.map(|ic| IndexedCell {
        point: terminal_point_from_alacritty(ic.point),
        cell: terminal_cell_from_alacritty(ic.cell),
    }));

    let selection_text = if content.selection.is_some() {
        term.selection_to_string()
    } else {
        None
    };

    let bottom_line = term.screen_lines() as i32 - 1 - content.display_offset as i32;
    let bottom_row_occupied = content.cursor.point.line.0 >= bottom_line
        || cells
            .iter()
            .rev()
            .take_while(|cell| cell.point.line >= bottom_line)
            .any(|cell| cell.cell.character() != ' ');

    Content {
        cells,
        mode: terminal_modes_from_alacritty(content.mode),
        display_offset: content.display_offset,
        selection_text,
        selection: content
            .selection
            .map(terminal_selection_range_from_alacritty),
        cursor: Cursor::from_alacritty(content.cursor),
        cursor_char: term.grid()[content.cursor.point].c,
        terminal_bounds: last_content.terminal_bounds,
        last_hovered_word: last_content.last_hovered_word.clone(),
        scrolled_to_top: content.display_offset == term.history_size(),
        scrolled_to_bottom: content.display_offset == 0,
        bottom_row_occupied,
    }
}

pub(super) fn content_text(term: &Term<ZedListener>) -> String {
    let start = AlacPoint::new(term.topmost_line(), Column(0));
    let end = AlacPoint::new(term.bottommost_line(), term.last_column());
    term.bounds_to_string(start, end)
}

pub(super) fn total_lines(term: &Term<ZedListener>) -> usize {
    term.total_lines()
}

pub(super) fn screen_lines(term: &Term<ZedListener>) -> usize {
    term.screen_lines()
}

pub(super) fn full_content_range(term: &Term<ZedListener>) -> Range {
    let start = AlacPoint::new(term.topmost_line(), Column(0));
    let end = AlacPoint::new(term.bottommost_line(), term.last_column());
    Range::from_alacritty(start..=end)
}

pub(super) fn last_non_empty_lines(term: &Term<ZedListener>, line_count: usize) -> Vec<String> {
    let grid = term.grid();
    let mut lines = Vec::new();

    let mut current_line = grid.bottommost_line().0;
    let topmost_line = grid.topmost_line().0;

    while current_line >= topmost_line && lines.len() < line_count {
        let (logical_line_start, logical_line) =
            logical_line_for_row(grid, current_line, topmost_line);

        if let Some(line) = process_line(logical_line) {
            lines.push(line);
        }

        current_line = logical_line_start - 1;
    }

    lines.reverse();
    lines
}

pub(super) fn update_vi_cursor_for_scroll(term: &mut Term<ZedListener>, scroll: Scroll) {
    match scroll {
        Scroll::Delta(delta) => {
            term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, delta);
        }
        Scroll::PageUp => {
            let lines = term.screen_lines() as i32;
            term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, lines);
        }
        Scroll::PageDown => {
            let lines = -(term.screen_lines() as i32);
            term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, lines);
        }
        Scroll::Top => {
            let point = AlacPoint::new(term.topmost_line(), Column(0));
            term.vi_mode_cursor = ViModeCursor::new(point);
        }
        Scroll::Bottom => {
            let point = AlacPoint::new(term.bottommost_line(), Column(0));
            term.vi_mode_cursor = ViModeCursor::new(point);
        }
    }
}

pub(super) fn update_selection_to_vi_cursor(term: &mut Term<ZedListener>) -> Option<Point> {
    let mut selection = term.selection.take()?;
    let point = term.vi_mode_cursor.point;
    selection.update(point, AlacDirection::Right);
    term.selection = Some(selection);
    Some(terminal_point_from_alacritty(point))
}

pub(super) fn find_from_terminal_point(
    term: &AlacrittyTerm,
    point: Point,
    regex_searches: &mut RegexSearches,
    path_style: PathStyle,
) -> Option<HyperlinkMatch> {
    let point = point.to_alacritty().grid_clamp(term, Boundary::Grid);
    hyperlinks::find_from_grid_point(term, point, regex_searches, path_style)
}

fn logical_line_for_row(grid: &Grid<AlacCell>, current: i32, topmost: i32) -> (i32, String) {
    let start = find_logical_line_start(grid, current, topmost);
    let mut line = String::new();
    for row in start..=current {
        line.push_str(&row_to_string(&grid[Line(row)]));
    }
    (start, line)
}

fn find_logical_line_start(grid: &Grid<AlacCell>, current: i32, topmost: i32) -> i32 {
    let mut line_start = current;
    while line_start > topmost {
        let previous_line = Line(line_start - 1);
        let last_cell = &grid[previous_line][Column(grid.columns() - 1)];
        if !last_cell.flags.contains(Flags::WRAPLINE) {
            break;
        }
        line_start -= 1;
    }
    line_start
}

fn row_to_string(row: &Row<AlacCell>) -> String {
    row[..Column(row.len())]
        .iter()
        .map(|cell| cell.c)
        .collect::<String>()
}

fn process_line(line: String) -> Option<String> {
    let trimmed = line.trim_end().to_string();
    if !trimmed.is_empty() {
        Some(trimmed)
    } else {
        None
    }
}

/// Appends a stringified task summary to the terminal, after its output.
///
/// SAFETY: This function should only be called after terminal's PTY is no longer alive.
/// New text being added to the terminal here, uses "less public" APIs,
/// which are not maintaining the entire terminal state intact.
///
///
/// The library
///
/// * does not increment inner grid cursor's _lines_ on `input` calls
///   (but displaying the lines correctly and incrementing cursor's columns)
///
/// * ignores `\n` and \r` character input, requiring the `newline` call instead
///
/// * does not alter grid state after `newline` call
///   so its `bottommost_line` is always the same additions, and
///   the cursor's `point` is not updated to the new line and column values
///
/// * ??? there could be more consequences, and any further "proper" streaming from the PTY might bug and/or panic.
///   Still, subsequent `append_text_to_term` invocations are possible and display the contents correctly.
///
/// Despite the quirks, this is the simplest approach to appending text to the terminal: its alternative, `grid_mut` manipulations,
/// do not properly set the scrolling state and display odd text after appending; also those manipulations are more tedious and error-prone.
/// The function achieves proper display and scrolling capabilities, at a cost of grid state not properly synchronized.
/// This is enough for printing moderately-sized texts like task summaries, but might break or perform poorly for larger texts.
pub(super) unsafe fn append_text_to_term(term: &mut Term<ZedListener>, text_lines: &[&str]) {
    term.newline();
    term.grid_mut().cursor.point.column = Column(0);
    for line in text_lines {
        for character in line.chars() {
            term.input(character);
        }
        term.newline();
        term.grid_mut().cursor.point.column = Column(0);
    }
}

pub(super) struct ScrollbackSearch {
    searcher: RegexSearch,
    literal: Option<LiteralSearch>,
    next_line: Line,
    oldest_line: Line,
    newest_line: Line,
    matches: Vec<Range>,
    total_count: usize,
}

struct LiteralSearch {
    query: Vec<char>,
    case_insensitive: bool,
    scratch: Vec<(char, AlacPoint)>,
    cached_row_id: Option<usize>,
    cached_row_matches: Vec<(Column, Column)>,
    #[cfg(test)]
    physical_rows_scanned: usize,
}

impl LiteralSearch {
    fn new(query: String) -> Self {
        let case_insensitive = !query
            .chars()
            .any(|character| character.is_ascii_uppercase());
        Self {
            query: query.chars().collect(),
            case_insensitive,
            scratch: Vec::new(),
            cached_row_id: None,
            cached_row_matches: Vec::new(),
            #[cfg(test)]
            physical_rows_scanned: 0,
        }
    }

    fn characters_equal(&self, left: char, right: char) -> bool {
        left == right
            || (self.case_insensitive
                && left.is_ascii()
                && right.is_ascii()
                && left.eq_ignore_ascii_case(&right))
    }

    /// Search complete logical lines from newest to oldest.
    fn advance(
        &mut self,
        term: &AlacrittyTerm,
        newest_line: Line,
        oldest_line: Line,
        matches: &mut Vec<Range>,
        total_count: &mut usize,
        match_limit: usize,
    ) {
        let grid = term.grid();
        let last_column = grid.last_column();
        let mut logical_newest = newest_line;

        while logical_newest >= oldest_line {
            let mut logical_oldest = logical_newest;
            while logical_oldest > oldest_line
                && grid[Line(logical_oldest.0 - 1)][last_column]
                    .flags
                    .contains(Flags::WRAPLINE)
            {
                logical_oldest.0 -= 1;
            }

            if logical_oldest == logical_newest {
                self.append_physical_row_matches(
                    term,
                    logical_newest,
                    matches,
                    total_count,
                    match_limit,
                );
                logical_newest.0 -= 1;
                continue;
            }

            self.scratch.clear();
            for line in logical_oldest.0..=logical_newest.0 {
                let line = Line(line);
                let row = &grid[line];
                for column in 0..row.line_length().0 {
                    let column = Column(column);
                    let cell = &row[column];
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        continue;
                    }
                    self.scratch.push((cell.c, AlacPoint::new(line, column)));
                }
            }

            if self.scratch.len() >= self.query.len() {
                for start in (0..=self.scratch.len() - self.query.len()).rev() {
                    if self.query.iter().enumerate().all(|(offset, expected)| {
                        self.characters_equal(self.scratch[start + offset].0, *expected)
                    }) {
                        *total_count = total_count.saturating_add(1);
                        if matches.len() < match_limit {
                            let start_point = self.scratch[start].1;
                            let end_point = self.scratch[start + self.query.len() - 1].1;
                            matches.push(Range::from_alacritty(start_point..=end_point));
                        }
                    }
                }
            }

            logical_newest = Line(logical_oldest.0 - 1);
        }
    }

    fn append_physical_row_matches(
        &mut self,
        term: &AlacrittyTerm,
        line: Line,
        matches: &mut Vec<Range>,
        total_count: &mut usize,
        match_limit: usize,
    ) {
        let grid = term.grid();
        let row_id = grid.row_storage_id(line);
        if self.cached_row_id != Some(row_id) {
            #[cfg(test)]
            {
                self.physical_rows_scanned += 1;
            }
            self.cached_row_id = Some(row_id);
            self.cached_row_matches.clear();
            self.scratch.clear();

            let row = &grid[line];
            for column in 0..row.line_length().0 {
                let column = Column(column);
                let cell = &row[column];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                self.scratch.push((cell.c, AlacPoint::new(line, column)));
            }

            if self.scratch.len() >= self.query.len() {
                for start in (0..=self.scratch.len() - self.query.len()).rev() {
                    if self.query.iter().enumerate().all(|(offset, expected)| {
                        self.characters_equal(self.scratch[start + offset].0, *expected)
                    }) {
                        self.cached_row_matches.push((
                            self.scratch[start].1.column,
                            self.scratch[start + self.query.len() - 1].1.column,
                        ));
                    }
                }
            }
        }

        for (start, end) in &self.cached_row_matches {
            *total_count = total_count.saturating_add(1);
            if matches.len() < match_limit {
                matches.push(Range::from_alacritty(
                    AlacPoint::new(line, *start)..=AlacPoint::new(line, *end),
                ));
            }
        }
    }
}

impl ScrollbackSearch {
    pub(super) fn new(term: &AlacrittyTerm, searcher: Search) -> Self {
        let (searcher, literal) = searcher.into_alacritty();
        Self {
            searcher,
            literal: literal.map(LiteralSearch::new),
            next_line: term.grid().bottommost_line(),
            oldest_line: term.grid().topmost_line(),
            newest_line: term.grid().bottommost_line(),
            matches: Vec::new(),
            total_count: 0,
        }
    }

    /// Search a bounded number of physical rows, starting with the newest output.
    ///
    /// Chunk boundaries include the start of a logical line when it is nearby. Extremely long
    /// wrapped lines are split after one additional chunk budget to keep lock time bounded.
    pub(super) fn advance(
        &mut self,
        term: &AlacrittyTerm,
        chunk_lines: usize,
        match_limit: usize,
    ) -> bool {
        if self.next_line < self.oldest_line {
            return true;
        }

        let current_oldest = term.grid().topmost_line();
        let current_newest = term.grid().bottommost_line();
        if current_oldest != self.oldest_line || current_newest != self.newest_line {
            // History was cleared, grew, or resized between lock slices. Previously collected
            // coordinates no longer identify the same cells, so restart from the current grid.
            self.next_line = current_newest;
            self.oldest_line = current_oldest;
            self.newest_line = current_newest;
            self.matches.clear();
            self.total_count = 0;
        }

        let candidate = Line(
            self.next_line
                .0
                .saturating_sub(chunk_lines.saturating_sub(1) as i32)
                .max(self.oldest_line.0),
        );
        let extension_limit = Line(
            candidate
                .0
                .saturating_sub(chunk_lines as i32)
                .max(self.oldest_line.0),
        );
        let last_column = term.grid().last_column();
        let mut chunk_end_line = candidate;
        while chunk_end_line > extension_limit
            && term.grid()[Line(chunk_end_line.0 - 1)][last_column]
                .flags
                .contains(Flags::WRAPLINE)
        {
            chunk_end_line.0 -= 1;
        }
        let chunk_end = AlacPoint::new(chunk_end_line, Column(0));
        let chunk_start = AlacPoint::new(self.next_line, term.grid().last_column());
        if let Some(literal) = &mut self.literal {
            literal.advance(
                term,
                self.next_line,
                chunk_end_line,
                &mut self.matches,
                &mut self.total_count,
                match_limit,
            );
        } else {
            let matches = RegexIter::new(
                chunk_start,
                chunk_end,
                AlacDirection::Left,
                term,
                &mut self.searcher,
            );
            for range in matches {
                self.total_count = self.total_count.saturating_add(1);
                if self.matches.len() < match_limit {
                    self.matches.push(Range::from_alacritty(range));
                }
            }
        }
        self.next_line = Line(chunk_end.line.0 - 1);

        self.next_line < self.oldest_line
    }

    pub(super) fn finish(mut self) -> crate::SearchMatches {
        // Leftward searches produce newest-first results, while the terminal selection and
        // navigation APIs expect the original oldest-first ordering.
        self.matches.reverse();
        crate::SearchMatches {
            limit_reached: self.total_count > self.matches.len(),
            total_count: self.total_count,
            ranges: self.matches,
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;
    use std::sync::Arc;

    use super::*;

    #[test]
    fn disabled_wakeup_gate_suppresses_wakeups_without_suppressing_other_events() {
        let (events_tx, mut events_rx) = futures::channel::mpsc::unbounded();
        let wakeup_gate = WakeupGate::new();
        let listener = ZedListener::new(events_tx, wakeup_gate.clone());
        wakeup_gate.set_enabled(false);

        listener.send_event(AlacTermEvent::Wakeup);
        listener.send_event(AlacTermEvent::Bell);

        assert!(matches!(
            events_rx.try_recv(),
            Ok(PtyEvent::Event(TerminalBackendEvent::Bell))
        ));
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn scrollback_search_yields_between_chunks_and_limits_to_newest_matches() {
        let bounds = TerminalBounds::new(
            px(10.),
            px(10.),
            gpui::bounds(gpui::point(px(0.), px(0.)), gpui::size(px(40.), px(40.))),
        );
        let (events_tx, _) = futures::channel::mpsc::unbounded();
        let listener = ZedListener::new(events_tx, WakeupGate::new());
        let mut term = Term::new(Config::default(), &bounds, listener);
        for line in 0..4 {
            term.grid_mut()[Line(line)][Column(0)].c = 'x';
        }

        let mut search = ScrollbackSearch::new(&term, Search::new("x").unwrap());
        let mut chunks = 1;
        while !search.advance(&term, 1, 2) {
            chunks += 1;
        }
        let result = search.finish();

        assert!(chunks > 1);
        assert!(result.limit_reached);
        assert_eq!(result.ranges.len(), 2);
        assert_eq!(result.total_count, 4);
        assert_eq!(result.ranges[0].start().line, 2);
        assert_eq!(result.ranges[1].start().line, 3);
    }

    #[test]
    fn scrollback_search_narrows_from_capped_character_matches_to_exact_word_matches() {
        let bounds = TerminalBounds::new(
            px(10.),
            px(10.),
            gpui::bounds(gpui::point(px(0.), px(0.)), gpui::size(px(800.), px(40.))),
        );
        let (events_tx, _) = futures::channel::mpsc::unbounded();
        let listener = ZedListener::new(events_tx, WakeupGate::new());
        let mut term = Term::new(Config::default(), &bounds, listener);
        for _ in 0..101 {
            for character in "zzzz Zetta benchmark output".chars() {
                term.input(character);
            }
            term.newline();
            term.grid_mut().cursor.point.column = Column(0);
        }

        let mut broad = ScrollbackSearch::new(&term, Search::new_literal("z").unwrap());
        while !broad.advance(&term, 7, crate::MAX_SEARCH_MATCHES) {}
        let broad = broad.finish();
        assert!(broad.limit_reached);
        assert_eq!(broad.ranges.len(), crate::MAX_SEARCH_MATCHES);
        assert_eq!(broad.total_count, 505);

        let mut narrow = ScrollbackSearch::new(&term, Search::new_literal("zetta").unwrap());
        while !narrow.advance(&term, 7, crate::MAX_SEARCH_MATCHES) {}
        assert!(
            narrow.literal.as_ref().unwrap().physical_rows_scanned < 10,
            "deduplicated physical rows should reuse literal search results"
        );
        let narrow = narrow.finish();
        assert!(!narrow.limit_reached);
        assert_eq!(narrow.total_count, 101);
        assert_eq!(narrow.ranges.len(), 101);
    }

    #[test]
    fn terminal_hyperlink_from_alacritty_keeps_alacritty_storage() {
        let hyperlink = AlacHyperlink::new(Some("id"), "https://example.com".to_string());
        let hyperlink = terminal_hyperlink_from_alacritty(hyperlink);

        assert!(matches!(&hyperlink.data, HyperlinkData::Alacritty(_)));
        assert_eq!(hyperlink.id(), Some("id"));
        assert_eq!(hyperlink.uri(), "https://example.com");
    }

    #[test]
    fn terminal_cell_from_alacritty_shares_extra_storage() {
        let mut cell = AlacCell::default();
        cell.push_zerowidth('a');

        let converted = terminal_cell_from_alacritty(&cell);

        match (&cell.extra, &converted.cell.extra) {
            (Some(extra), Some(converted_extra)) => assert!(Arc::ptr_eq(extra, converted_extra)),
            _ => panic!("expected extra storage on both cells"),
        }
    }

    #[test]
    fn terminal_modes_round_trip_alacritty_flags() {
        let alacritty_modes = TermMode::APP_CURSOR
            | TermMode::BRACKETED_PASTE
            | TermMode::ALT_SCREEN
            | TermMode::MOUSE_DRAG
            | TermMode::SGR_MOUSE
            | TermMode::VI;

        let terminal_modes = terminal_modes_from_alacritty(alacritty_modes);
        assert!(terminal_modes.contains(Modes::APP_CURSOR));
        assert!(terminal_modes.contains(Modes::BRACKETED_PASTE));
        assert!(terminal_modes.contains(Modes::ALT_SCREEN));
        assert!(terminal_modes.contains(Modes::MOUSE_DRAG));
        assert!(terminal_modes.intersects(Modes::MOUSE_MODE));
        assert!(terminal_modes.contains(Modes::SGR_MOUSE));
        assert!(terminal_modes.contains(Modes::VI));
        assert!(!terminal_modes.contains(Modes::MOUSE_REPORT_CLICK));

        let alacritty_modes = terminal_modes.to_alacritty();
        assert!(alacritty_modes.contains(TermMode::APP_CURSOR));
        assert!(alacritty_modes.contains(TermMode::BRACKETED_PASTE));
        assert!(alacritty_modes.contains(TermMode::ALT_SCREEN));
        assert!(alacritty_modes.contains(TermMode::MOUSE_DRAG));
        assert!(alacritty_modes.contains(TermMode::SGR_MOUSE));
        assert!(alacritty_modes.contains(TermMode::VI));
        assert!(!alacritty_modes.contains(TermMode::MOUSE_REPORT_CLICK));
    }

    #[test]
    fn non_reflow_resize_truncates_primary_grid_lines() {
        let initial_bounds = TerminalBounds::new(
            px(10.),
            px(10.),
            gpui::bounds(gpui::point(px(0.), px(0.)), gpui::size(px(40.), px(20.))),
        );
        let resized_bounds = TerminalBounds::new(
            px(10.),
            px(10.),
            gpui::bounds(gpui::point(px(0.), px(0.)), gpui::size(px(20.), px(20.))),
        );
        let (events_tx, _) = futures::channel::mpsc::unbounded();
        let listener = ZedListener::new(events_tx, WakeupGate::new());
        let mut term = Term::new(Config::default(), &initial_bounds, listener);
        for (column, character) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
            term.grid_mut()[Line(0)][Column(column)].c = character;
        }

        resize(&mut term, resized_bounds, false);

        assert_eq!(term.columns(), 2);
        assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
        assert_eq!(term.grid()[Line(0)][Column(1)].c, 'b');
        assert_eq!(term.grid()[Line(1)][Column(0)].c, ' ');
        assert_eq!(term.grid()[Line(1)][Column(1)].c, ' ');
    }

    #[test]
    fn terminal_selection_range_round_trip_alacritty_range() {
        let alacritty_range = AlacSelectionRange {
            start: AlacPoint::new(Line(-2), Column(3)),
            end: AlacPoint::new(Line(4), Column(8)),
            is_block: true,
        };

        let terminal_range = terminal_selection_range_from_alacritty(alacritty_range);
        assert_eq!(
            terminal_range,
            SelectionRange {
                start: Point {
                    line: -2,
                    column: 3
                },
                end: Point { line: 4, column: 8 },
                is_block: true,
            }
        );
    }
}
