use super::*;

use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use futures::channel::mpsc::unbounded;

use crate::{
    TerminalBounds,
    alacritty::{WakeupGate, ZedListener, display_only_term_config, new_term},
    terminal_settings::{AlternateScroll, CursorShape},
};

/// A terminal with `input` already processed into its grid.
fn term_showing(input: &[u8]) -> std::sync::Arc<crate::alacritty::AlacrittyTermLock> {
    let config = display_only_term_config(100, CursorShape::Block);
    let (events, _receiver) = unbounded();
    let listener = ZedListener::new(events, WakeupGate::new());
    let term = new_term(
        &config,
        TerminalBounds::default(),
        listener,
        AlternateScroll::On,
    );
    let mut processor = Processor::<StdSyncHandler>::new();
    processor.advance(&mut *term.lock(), input);
    term
}

fn snapshot_of(input: &[u8]) -> String {
    let term = term_showing(input);
    let snapshot = ansi_snapshot(&term.lock(), 1000);
    String::from_utf8(snapshot).expect("a snapshot must be valid UTF-8")
}

#[test]
fn plain_text_round_trips_line_by_line() {
    assert_eq!(snapshot_of(b"first\r\nsecond"), "first\r\nsecond");
}

#[test]
fn trailing_blank_lines_are_not_replayed() {
    // A terminal's grid is always full height. Replaying its blank remainder
    // would push the restored screen up by the number of unused rows.
    assert_eq!(snapshot_of(b"only line\r\n"), "only line");
    assert_eq!(snapshot_of(b"a\r\n\r\n\r\nb"), "a\r\n\r\n\r\nb");
}

#[test]
fn colour_is_carried_across_and_reset_at_the_end() {
    let snapshot = snapshot_of(b"\x1b[31mred\x1b[0m plain");

    assert!(snapshot.contains(";31"), "{snapshot:?}");
    assert!(snapshot.contains("red"));
    assert!(snapshot.contains("plain"));
    // Leaving the style set would tint whatever the session prints next.
    assert!(snapshot.ends_with("\x1b[0m") || snapshot.contains("\x1b[0m plain"));
}

#[test]
fn a_run_of_one_style_costs_one_sequence() {
    let snapshot = snapshot_of(b"\x1b[32mgreengreengreen");

    // One sequence per cell would multiply a full screen of coloured output by
    // an order of magnitude, and this is written on every detach.
    assert_eq!(snapshot.matches("\x1b[").count(), 2, "{snapshot:?}");
}

#[test]
fn attributes_are_reproduced() {
    let snapshot = snapshot_of(b"\x1b[1mbold\x1b[0m\x1b[3mitalic\x1b[0m\x1b[4munder");

    assert!(snapshot.contains(";1m"), "{snapshot:?}");
    assert!(snapshot.contains(";3m"), "{snapshot:?}");
    assert!(snapshot.contains(";4m"), "{snapshot:?}");
}

#[test]
fn true_colour_survives_as_true_colour() {
    // Rounding a 24-bit colour to the nearest palette entry would visibly
    // change a restored screen.
    let snapshot = snapshot_of(b"\x1b[38;2;10;20;30mshaded");
    assert!(snapshot.contains("38:2::10:20:30"), "{snapshot:?}");
}

/// The background of every cell in a row, which is what a bar drawn out of
/// blanks consists of.
fn row_backgrounds(term: &AlacrittyTerm, line: i32) -> Vec<Color> {
    let row = &term.grid()[Line(line)];
    (0..row.len())
        .map(|column| row[Column(column)].bg)
        .collect()
}

#[test]
fn a_bar_of_blanks_keeps_the_width_its_colour_covers() {
    // A label followed by an erase-to-end-of-line under a background colour is
    // how htop draws its column header and its F-key footer, and how any status
    // bar is drawn. The blanks past the label carry the colour, so a snapshot
    // that ends at the text restores a bar only as wide as its labels — which
    // is what a window that joined a shared session showed.
    let bar = b"\x1b[46mF1Help\x1b[K";
    let original = term_showing(bar);
    let replayed = term_showing(snapshot_of(bar).as_bytes());

    assert_eq!(
        row_backgrounds(&replayed.lock(), 0),
        row_backgrounds(&original.lock(), 0),
        "the bar must be restored across every cell it covered"
    );
}

#[test]
fn blanks_that_show_nothing_are_still_dropped() {
    // The counterpart: a foreground colour on a space paints nothing, so a run
    // of them is as blank as an untouched row and stays dropped. Keeping it
    // would pad lines out to the full width for no visible difference.
    let snapshot = snapshot_of(b"\x1b[31m   ");
    assert!(
        !snapshot.contains(' '),
        "nothing of the row itself is worth replaying: {snapshot:?}"
    );
}

#[test]
fn the_snapshot_is_bounded_by_line_count() {
    let mut input = Vec::new();
    for line in 0..200 {
        input.extend_from_slice(format!("line{line}\r\n").as_bytes());
    }
    let term = term_showing(&input);
    let snapshot = String::from_utf8(ansi_snapshot(&term.lock(), 5)).unwrap();

    // The most recent lines are the ones worth keeping.
    assert!(snapshot.contains("line199"), "{snapshot:?}");
    assert!(!snapshot.contains("line194"), "{snapshot:?}");
    // A ceiling, not an exact count: the row the cursor sits on is blank and
    // is dropped with the other trailing blanks.
    assert!(snapshot.matches("\r\n").count() < 5, "{snapshot:?}");
}

#[test]
fn an_empty_terminal_produces_nothing() {
    assert_eq!(snapshot_of(b""), "");
}

#[test]
fn an_alternate_screen_session_starts_with_alt_screen_entry() {
    // A full-screen program draws into the alternate screen. Reattaching into
    // the primary one would leave its drawing in the wrong buffer.
    let snapshot = snapshot_of(b"\x1b[?1049hfull\r\nscreen");
    assert!(snapshot.starts_with("\x1b[?1049h"), "{snapshot:?}");

    // And replaying the snapshot puts the fresh terminal back on the
    // alternate screen the session actually ran in.
    let replayed = term_showing(snapshot.as_bytes());
    let mode = replayed.lock().mode().clone();
    assert!(mode.contains(TermMode::ALT_SCREEN), "{mode:?}");
}

#[test]
fn a_hidden_cursor_stays_hidden() {
    // A fresh terminal shows its cursor; a session that hid its own needs the
    // sequence that re-hides it, or a blinking block follows the restored
    // screen around.
    let snapshot = snapshot_of(b"\x1b[?25lno cursor");
    assert!(snapshot.ends_with("\x1b[?25l"), "{snapshot:?}");
}

#[test]
fn a_cursor_off_the_natural_end_is_repositioned() {
    // The plain replay leaves the cursor at the end of the emitted text. A
    // program that parked its cursor elsewhere — a TUI after a redraw — has to
    // get that position back.
    let snapshot = snapshot_of(b"first\r\nsecond\r\nthird\x1b[2;3H");
    assert!(snapshot.contains("\x1b[2;3H"), "{snapshot:?}");
}

#[test]
fn a_wide_character_is_emitted_once() {
    // The grid stores a spacer cell after a double-width character; emitting it
    // as text would duplicate the character on replay.
    let snapshot = snapshot_of("日本語".as_bytes());
    assert_eq!(snapshot, "日本語");
}

/// Renders a terminal's visible text, so two grids can be compared by what
/// they show rather than by the bytes that produced them.
fn visible_text(term: &crate::alacritty::AlacrittyTerm) -> Vec<String> {
    use alacritty_terminal::grid::Dimensions as _;
    let grid = term.grid();
    (0..grid.screen_lines())
        .map(|line| {
            let row = &grid[alacritty_terminal::index::Line(line as i32)];
            (0..row.line_length().0)
                .map(|column| row[alacritty_terminal::index::Column(column)].c)
                .collect::<String>()
        })
        .collect()
}

fn styles_of(term: &crate::alacritty::AlacrittyTerm) -> Vec<(char, Color, Color, Flags)> {
    use alacritty_terminal::grid::Dimensions as _;
    let grid = term.grid();
    let mut cells = Vec::new();
    for line in 0..grid.screen_lines() {
        let row = &grid[alacritty_terminal::index::Line(line as i32)];
        for column in 0..row.line_length().0 {
            let cell = &row[alacritty_terminal::index::Column(column)];
            cells.push((cell.c, cell.fg, cell.bg, cell.flags & RENDERED_FLAGS));
        }
    }
    cells
}

#[test]
fn replaying_a_snapshot_reproduces_the_screen_it_was_taken_from() {
    // The property the whole exchange rests on: what the user sees after
    // reattaching is what they saw before detaching.
    let original_input = "plain\r\n\x1b[1;31mbold red\x1b[0m\r\n\x1b[42mgreen bg\x1b[0m \
                          \x1b[38;2;1;2;3mtrue colour\x1b[0m\r\nlast";
    let original = term_showing(original_input.as_bytes());
    let snapshot = ansi_snapshot(&original.lock(), 1000);

    let replayed = term_showing(&snapshot);

    assert_eq!(
        visible_text(&replayed.lock()),
        visible_text(&original.lock())
    );
    assert_eq!(styles_of(&replayed.lock()), styles_of(&original.lock()));
}

#[test]
fn a_snapshot_leaves_no_style_set_for_what_follows() {
    // The replay is followed immediately by live output from the session. A
    // style left set would tint it.
    let original = term_showing(b"\x1b[1;35mstyled to the end");
    let mut snapshot = ansi_snapshot(&original.lock(), 1000);
    snapshot.extend_from_slice(b"\r\nafterwards");

    let replayed = term_showing(&snapshot);
    let cells = styles_of(&replayed.lock());
    let (_, foreground, _, flags) = cells
        .iter()
        .rev()
        .find(|(character, ..)| *character == 'a')
        .copied()
        .expect("the text printed after the replay");

    assert_eq!(foreground, Color::Named(NamedColor::Foreground));
    assert!(flags.is_empty(), "{flags:?}");
}

#[test]
fn replaying_an_alternate_screen_snapshot_restores_the_session_state() {
    // The full-screen exchange: reattaching to a TUI restores the alternate
    // screen it ran in, the cursor position it left, and its hidden cursor —
    // not just its text.
    let original = term_showing(b"\x1b[?1049h\x1b[?25lpanel\r\nitem 1\x1b[2;3H");
    let snapshot = ansi_snapshot(&original.lock(), 1000);

    let replayed = term_showing(&snapshot);
    let term = replayed.lock();
    let mode = term.mode().clone();

    assert!(mode.contains(TermMode::ALT_SCREEN), "{mode:?}");
    assert!(!mode.contains(TermMode::SHOW_CURSOR), "{mode:?}");
    assert_eq!(
        term.grid().cursor.point,
        alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(1),
            alacritty_terminal::index::Column(2),
        ),
        "{snapshot:?}",
    );
    drop(term);
    assert_eq!(
        visible_text(&replayed.lock()),
        visible_text(&original.lock())
    );
}

/// A mode is an agreement between the program and its terminal, reached before a
/// second viewer arrives. The viewer has to be holding up the same end of it.
#[test]
fn the_sessions_modes_survive_the_snapshot() {
    // What ncurses' `smkx` sends: application cursor keys, then the keypad.
    // `APP_CURSOR` alone decides whether an arrow key is sent as `ESC [ A` or
    // `ESC O A`, so a viewer that joined without it sent a full-screen program
    // sequences it had not asked for — htop read them as its nice-value keys,
    // and every arrow raised the value because every arrow delivered the same
    // stray byte.
    let restored = round_trip_modes(b"\x1b[?1h\x1b=running");
    assert!(
        restored.contains(TermMode::APP_CURSOR),
        "app cursor: {restored:?}"
    );
    assert!(
        restored.contains(TermMode::APP_KEYPAD),
        "app keypad: {restored:?}"
    );

    // The rest of the same bargain: without these a joined viewer's clicks go
    // unreported and its pastes arrive unbracketed.
    let restored = round_trip_modes(b"\x1b[?1002h\x1b[?1006h\x1b[?2004h");
    assert!(
        restored.contains(TermMode::MOUSE_DRAG),
        "mouse drag: {restored:?}"
    );
    assert!(
        restored.contains(TermMode::SGR_MOUSE),
        "sgr mouse: {restored:?}"
    );
    assert!(
        restored.contains(TermMode::BRACKETED_PASTE),
        "bracketed paste: {restored:?}"
    );

    // Modes a fresh terminal has *on* have to be turned back off, not assumed.
    let restored = round_trip_modes(b"\x1b[?7l\x1b[?1007l");
    assert!(
        !restored.contains(TermMode::LINE_WRAP),
        "line wrap: {restored:?}"
    );
    assert!(
        !restored.contains(TermMode::ALTERNATE_SCROLL),
        "alternate scroll: {restored:?}"
    );

    // An ordinary session carries nothing extra, so its snapshot is unchanged.
    assert_eq!(snapshot_of(b"just a shell"), "just a shell");
}

/// The modes a fresh terminal ends up in after replaying `input`'s snapshot.
fn round_trip_modes(input: &[u8]) -> TermMode {
    let snapshot = ansi_snapshot(&term_showing(input).lock(), 1000);
    let replayed = term_showing(&snapshot);
    let mode = replayed.lock().mode().to_owned();
    mode
}
