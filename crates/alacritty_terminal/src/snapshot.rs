//! Serializing a terminal's grid back into the escape sequences that produced
//! it.
//!
//! Zetta-authored, and here rather than in `crates/terminal` because both sides
//! of a session need it: the window takes a snapshot when it hands a session
//! over, and the multiplexer — which keeps a grid of its own for every pane it
//! reads — takes one when it hands a session back. Without it, reattaching would
//! show whatever the program happens to redraw next and nothing underneath.
//!
//! Its terminal-facing tests live in `crates/terminal/src/tests/snapshot.rs`,
//! where the harness for building a terminal from a byte stream already is;
//! this module also keeps a small buffer-only regression test below.
//!
//! The output is SGR attributes, text, and newlines, plus whatever it takes to
//! restore a session that was not in the state a fresh terminal starts in: the
//! alternate screen entry, the cursor position, and the cursor visibility. A
//! fresh terminal starts on the primary screen with a visible cursor at the end
//! of whatever it has replayed, so a session that matches that stays plain. A
//! full-screen program — which hides the cursor and addresses it directly —
//! is restored into the buffer it actually ran in, while the primary buffer is
//! restored before entering the alternate one so leaving the program reveals
//! the shell screen it had before the handover.

use std::io::Write as _;

use crate::{
    grid::{Dimensions as _, Grid, Row},
    index::{Column, Line},
    term::{
        Term, TermMode,
        cell::{Cell, Flags, LineLength as _},
    },
    vte::ansi::{Color, NamedColor},
};

/// The attributes an SGR sequence has to reproduce. Compared between cells so
/// that a run of identically styled text costs one sequence, not one per cell.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Style {
    foreground: Color,
    background: Color,
    flags: Flags,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            foreground: Color::Named(NamedColor::Foreground),
            background: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        }
    }
}

impl Style {
    fn of(cell: &Cell) -> Self {
        Self {
            foreground: cell.fg,
            background: cell.bg,
            // Only the attributes SGR can express. Layout flags such as
            // `WRAPLINE` and the wide-character spacers describe the grid, not
            // the text, and are handled by how lines are emitted.
            flags: cell.flags & RENDERED_FLAGS,
        }
    }
}

pub const RENDERED_FLAGS: Flags = Flags::from_bits_truncate(
    Flags::INVERSE.bits()
        | Flags::BOLD.bits()
        | Flags::ITALIC.bits()
        | Flags::DIM.bits()
        | Flags::HIDDEN.bits()
        | Flags::STRIKEOUT.bits()
        | Flags::ALL_UNDERLINES.bits(),
);

/// How much of a row has to be replayed.
///
/// Not `line_length`, which ends at the last cell holding a character: a blank
/// cell still paints, so a bar drawn as coloured spaces running out to the right
/// edge is content even though none of it is text. Stopping at the text cut
/// htop's column header and its F-key footer off after their last label and left
/// the rest of those rows in the default background.
///
/// Only what a blank cell can actually show counts — its background, and the
/// attributes that draw rather than tint. A foreground colour on a space is
/// invisible, and a shell that erases to the end of a line mid-colour leaves a
/// row of exactly those, so counting it would pad most of a scrollback's lines
/// out to the full width for nothing.
fn painted_length(row: &Row<Cell>) -> usize {
    let text = row.line_length().0;
    let mut length = row.len();
    while length > text {
        let cell = &row[Column(length - 1)];
        let paints = cell.bg != Color::Named(NamedColor::Background)
            || cell.flags.intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
        if paints {
            break;
        }
        length -= 1;
    }
    length
}

/// Serializes `term`'s scrollback and screen, most recent `max_lines` lines.
///
/// Bounded by line count rather than bytes because that is what the user sees;
/// the caller bounds the result again by whatever it is willing to retain.
pub fn ansi_snapshot<T>(term: &Term<T>, max_lines: usize) -> Vec<u8> {
    if term.grid().total_lines() == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mode = term.mode();
    let in_alternate_screen = mode.contains(TermMode::ALT_SCREEN);
    let cursor_hidden = !mode.contains(TermMode::SHOW_CURSOR);

    let mut out = Vec::new();
    if in_alternate_screen {
        // The inactive grid is the primary screen while a full-screen program
        // owns the alternate one. Restore it first: when the program exits,
        // its 1049l must reveal the shell screen that was underneath it rather
        // than the blank primary buffer of the fresh terminal we replay into.
        let primary = term.inactive_grid();
        let primary_cursor_needs_line_advance = primary.cursor.point.column.0 > 0;
        append_grid_snapshot(&mut out, primary, max_lines, false);
        if primary_cursor_needs_line_advance {
            // Windows PowerShell can leave the cursor after the command that
            // launched a TUI. Its next prompt does not always emit a line
            // ending after the alternate screen closes, so preserve the
            // command line and put the restored prompt on the following row.
            out.extend_from_slice(b"\r\n");
        }

        // A full-screen program owns the alternate screen; enter it before
        // replaying the active grid so its drawing and subsequent input remain
        // in the buffer the program expects.
        out.extend_from_slice(b"\x1b[?1049h");
    }

    append_grid_snapshot(&mut out, term.grid(), max_lines, cursor_hidden);

    // Last, so the replay above happens in a fresh terminal's modes and the
    // session's own modes are what the *program* then talks to. `ORIGIN` in
    // particular changes how the cursor position just emitted would be read.
    write_modes(&mut out, *mode);
    out
}

/// Appends one screen buffer's contents and cursor position to a replay.
///
/// `hide_cursor` applies only to the active buffer. Cursor visibility is a
/// terminal-wide mode, so hiding it while restoring the primary buffer would
/// also hide it before the alternate-screen program has been replayed; emit it
/// once, after the active buffer instead.
fn append_grid_snapshot(out: &mut Vec<u8>, grid: &Grid<Cell>, max_lines: usize, hide_cursor: bool) {
    // Walk from the oldest retained line to the bottom of the screen. Lines
    // above the requested limit are dropped from the top, which is where a
    // terminal loses history anyway.
    let first = grid
        .bottommost_line()
        .0
        .saturating_sub(max_lines.saturating_sub(1) as i32)
        .max(grid.topmost_line().0);
    let last = grid.bottommost_line().0;

    let mut style = Style::default();
    // Trailing blank lines at the bottom of the screen are not worth replaying;
    // buffering the newlines means they are only emitted once something
    // follows them.
    let mut pending_newlines = 0;
    // Where a plain replay of the emitted text leaves the cursor, so the
    // original position only has to be emitted when it differs.
    let mut natural_line = 0;
    let mut natural_column = 0;

    for line in first..=last {
        let row = &grid[Line(line)];
        let length = painted_length(row);
        if length == 0 {
            pending_newlines += 1;
            continue;
        }
        for _ in 0..pending_newlines {
            out.extend_from_slice(b"\r\n");
            natural_line += 1;
            natural_column = 0;
        }
        pending_newlines = 0;

        for column in 0..length {
            let cell = &row[Column(column)];
            // The second half of a wide character is a placeholder in the grid
            // and has no text of its own.
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }

            let cell_style = Style::of(cell);
            if cell_style != style {
                write_sgr(out, &cell_style);
                style = cell_style;
            }
            let mut buffer = [0; 4];
            out.extend_from_slice(cell.c.encode_utf8(&mut buffer).as_bytes());
            for zerowidth in cell.zerowidth().into_iter().flatten() {
                out.extend_from_slice(zerowidth.encode_utf8(&mut buffer).as_bytes());
            }
            natural_column += if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        }
        pending_newlines += 1;
    }

    if style != Style::default() {
        out.extend_from_slice(b"\x1b[0m");
    }
    // Whatever newlines are still pending belong to trailing blank rows. A
    // grid is always full height, so emitting them would push the restored
    // screen up by however many rows the session was not using, and leave the
    // cursor below the content rather than at it.
    let _ = pending_newlines;

    // The cursor the session left behind. The position only needs to be
    // emitted when it differs from where the plain replay lands. A cursor
    // below the content belongs to a dropped trailing blank row, which stays
    // dropped — restoring it would move the cursor below the content rather
    // than leaving it at it. The visibility is emitted whenever it differs
    // from a fresh terminal, which shows the cursor.
    let cursor = grid.cursor.point;
    let cursor_row = cursor.line.0 - first;
    let mut cursor_column = cursor.column.0;
    if grid[cursor].flags.contains(Flags::WIDE_CHAR_SPACER) {
        cursor_column = cursor_column.saturating_sub(1);
    }
    let on_replayed_content = cursor.line.0 >= first && cursor_row <= natural_line;
    if on_replayed_content && (cursor_row, cursor_column) != (natural_line, natural_column) {
        write!(out, "\x1b[{};{}H", cursor_row + 1, cursor_column + 1).expect("write to Vec");
    }
    if hide_cursor {
        out.extend_from_slice(b"\x1b[?25l");
    }
}

/// The modes a session was in that a fresh terminal is not.
///
/// Not decoration: a mode is an agreement between the program and its terminal,
/// reached before a second viewer ever arrives, and the viewer has to be holding
/// up the same end of it. `APP_CURSOR` alone decides whether an arrow key is sent
/// as `ESC [ A` or `ESC O A`, so a client that joined a full-screen program
/// without it sent sequences the program had not asked for and read them as
/// whatever else those bytes meant — htop took an arrow for its nice-value keys,
/// and every arrow raised the nice value because every arrow delivered the same
/// stray byte. Mouse reporting, bracketed paste and the keyboard protocol are the
/// same bargain: without them a joined viewer's clicks go unreported and its
/// pastes arrive unbracketed.
///
/// The kitty keyboard protocol's flags are deliberately absent: this terminal
/// never enables `kitty_keyboard`, so they cannot be set and restoring them would
/// be writing sequences for a state no session here can reach.
///
/// Only the differences from a fresh terminal are emitted, so an ordinary shell's
/// snapshot carries nothing extra.
fn write_modes(out: &mut Vec<u8>, mode: TermMode) {
    let default = TermMode::default();
    for (flag, number) in DEC_PRIVATE_MODES {
        if mode.contains(*flag) == default.contains(*flag) {
            continue;
        }
        let action = if mode.contains(*flag) { 'h' } else { 'l' };
        write!(out, "\x1b[?{number}{action}").expect("write to Vec");
    }
    for (flag, number) in ANSI_MODES {
        if mode.contains(*flag) == default.contains(*flag) {
            continue;
        }
        let action = if mode.contains(*flag) { 'h' } else { 'l' };
        write!(out, "\x1b[{number}{action}").expect("write to Vec");
    }
    // The application keypad is not a DEC private mode; it has its own pair.
    if mode.contains(TermMode::APP_KEYPAD) {
        out.extend_from_slice(b"\x1b=");
    }
}

/// The private modes worth restoring, by their DEC number.
///
/// `SHOW_CURSOR` and `ALT_SCREEN` are absent because they are emitted where they
/// belong: the alternate screen before the content that lives in it, the cursor
/// with the rest of the cursor's state. `VI` is this terminal's own mode, not
/// something the session's program agreed to, so it is not restored.
const DEC_PRIVATE_MODES: &[(TermMode, &str)] = &[
    (TermMode::APP_CURSOR, "1"),
    (TermMode::ORIGIN, "6"),
    (TermMode::LINE_WRAP, "7"),
    (TermMode::MOUSE_REPORT_CLICK, "1000"),
    (TermMode::MOUSE_DRAG, "1002"),
    (TermMode::MOUSE_MOTION, "1003"),
    (TermMode::FOCUS_IN_OUT, "1004"),
    (TermMode::UTF8_MOUSE, "1005"),
    (TermMode::SGR_MOUSE, "1006"),
    (TermMode::ALTERNATE_SCROLL, "1007"),
    (TermMode::URGENCY_HINTS, "1042"),
    (TermMode::BRACKETED_PASTE, "2004"),
    (TermMode::WIN32_INPUT, "9001"),
];

/// Modes set with `CSI Pm h` rather than `CSI ? Pm h`.
const ANSI_MODES: &[(TermMode, &str)] =
    &[(TermMode::INSERT, "4"), (TermMode::LINE_FEED_NEW_LINE, "20")];

fn write_sgr(out: &mut Vec<u8>, style: &Style) {
    // Always from a known state: emitting only the differences would need the
    // reader to track which attributes each sequence cleared.
    out.extend_from_slice(b"\x1b[0");
    if style.flags.contains(Flags::BOLD) {
        out.extend_from_slice(b";1");
    }
    if style.flags.contains(Flags::DIM) {
        out.extend_from_slice(b";2");
    }
    if style.flags.contains(Flags::ITALIC) {
        out.extend_from_slice(b";3");
    }
    if style.flags.intersects(Flags::ALL_UNDERLINES) {
        out.extend_from_slice(if style.flags.contains(Flags::DOUBLE_UNDERLINE) {
            b";21".as_slice()
        } else {
            b";4".as_slice()
        });
    }
    if style.flags.contains(Flags::INVERSE) {
        out.extend_from_slice(b";7");
    }
    if style.flags.contains(Flags::HIDDEN) {
        out.extend_from_slice(b";8");
    }
    if style.flags.contains(Flags::STRIKEOUT) {
        out.extend_from_slice(b";9");
    }
    write_color(out, style.foreground, true);
    write_color(out, style.background, false);
    out.push(b'm');
}

fn write_color(out: &mut Vec<u8>, color: Color, foreground: bool) {
    use std::io::Write as _;
    match color {
        // The default is already covered by the leading reset.
        Color::Named(NamedColor::Foreground) if foreground => {},
        Color::Named(NamedColor::Background) if !foreground => {},
        Color::Named(named) => {
            if let Some(index) = named_index(named) {
                write_indexed(out, index, foreground);
            }
        },
        Color::Indexed(index) => write_indexed(out, index, foreground),
        Color::Spec(rgb) => {
            let _ = write!(
                out,
                ";{}:2::{}:{}:{}",
                if foreground { 38 } else { 48 },
                rgb.r,
                rgb.g,
                rgb.b
            );
        },
    }
}

fn write_indexed(out: &mut Vec<u8>, index: u8, foreground: bool) {
    use std::io::Write as _;
    // The first sixteen have their own codes, which more terminals honour than
    // the indexed form.
    let _ = match (index, foreground) {
        (0..=7, true) => write!(out, ";{}", 30 + index),
        (0..=7, false) => write!(out, ";{}", 40 + index),
        (8..=15, true) => write!(out, ";{}", 90 + index - 8),
        (8..=15, false) => write!(out, ";{}", 100 + index - 8),
        (_, true) => write!(out, ";38:5:{index}"),
        (_, false) => write!(out, ";48:5:{index}"),
    };
}

fn named_index(named: NamedColor) -> Option<u8> {
    Some(match named {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        // Cursor and dim variants have no portable SGR spelling; leaving them
        // to the reset keeps the replay readable rather than approximating.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::ansi_snapshot;

    use crate::{
        event::VoidListener,
        grid::Dimensions,
        index::{Column, Line},
        term::{Config, Term, cell::LineLength as _},
        vte::ansi::{Processor, StdSyncHandler},
    };

    struct Size {
        columns: usize,
        lines: usize,
    }

    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.lines
        }

        fn screen_lines(&self) -> usize {
            self.lines
        }

        fn columns(&self) -> usize {
            self.columns
        }
    }

    fn term_showing(input: &[u8]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size { columns: 80, lines: 24 }, VoidListener);
        Processor::<StdSyncHandler>::new().advance(&mut term, input);
        term
    }

    fn line_text(term: &Term<VoidListener>, line: i32) -> String {
        let row = &term.grid()[Line(line)];
        (0..row.line_length().0).map(|column| row[Column(column)].c).collect()
    }

    #[test]
    fn leaving_an_alternate_screen_restores_the_primary_buffer() {
        let original = term_showing(b"prompt> htop\x1b[?1049h\x1b[2Jpanel");
        let snapshot = ansi_snapshot(&original, 1000);
        let mut replayed = term_showing(&snapshot);

        Processor::<StdSyncHandler>::new()
            .advance(&mut replayed, b"\x1b[?1049lPS C:\\Users\\saltw> ");

        assert_eq!(line_text(&replayed, 0), "prompt> htop");
        assert_eq!(line_text(&replayed, 1), "PS C:\\Users\\saltw>");
    }
}
