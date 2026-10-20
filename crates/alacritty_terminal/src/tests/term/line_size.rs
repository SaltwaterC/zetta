//! DECDWL and DECDHL, after Shitty's DEC line-attribute cases.
use super::*;
use crate::event::VoidListener;
use crate::term::test::TermSize;
use crate::vte::ansi::{Processor, StdSyncHandler};

fn terminal(columns: usize, lines: usize, input: &[u8]) -> Term<VoidListener> {
    let mut term = Term::new(Config::default(), &TermSize::new(columns, lines), VoidListener);
    Processor::<StdSyncHandler>::new().advance(&mut term, input);
    term
}

fn text(term: &Term<VoidListener>, line: i32) -> String {
    term.grid[Line(line)][..].iter().map(|cell| cell.c).collect()
}

fn size(term: &Term<VoidListener>, line: i32) -> LineSize {
    term.grid[Line(line)].line_size()
}

#[test]
fn each_designator_sets_its_size_and_a_line_erase_keeps_it() {
    let term =
        terminal(8, 4, b"AB\x1b#6\x1b[2K\x1b[2;1H\x1b#3\x1b[3;1H\x1b#4\x1b[4;1H\x1b#6\x1b#5");
    assert_eq!(size(&term, 0), LineSize::DoubleWidth);
    assert_eq!(size(&term, 1), LineSize::DoubleHeightTop);
    assert_eq!(size(&term, 2), LineSize::DoubleHeightBottom);
    assert_eq!(size(&term, 3), LineSize::Single);
}

#[test]
fn sizes_scroll_with_their_rows_and_new_rows_are_single() {
    let term =
        terminal(8, 4, b"\x1b[2;1H\x1b#3\x1b[3;1H\x1b#4\x1b[4;1H\x1b#6\x1b[2;4r\x1b[2;1H\x1bM");
    assert_eq!(
        [1, 2, 3].map(|line| size(&term, line)),
        [LineSize::Single, LineSize::DoubleHeightTop, LineSize::DoubleHeightBottom]
    );
}

#[test]
fn erasing_whole_rows_resets_them() {
    let term = terminal(8, 3, b"\x1b[2;1H\x1b#3\x1b[2J");
    assert_eq!(size(&term, 1), LineSize::Single);
    let term = terminal(8, 3, b"\x1b[2;1H\x1b#6\x1b[1;1H\x1b#6\x1b[J");
    assert_eq!([size(&term, 0), size(&term, 1)], [LineSize::Single; 2]);
    // Erasing from mid-row keeps that row's size; the rows wholly above go.
    let term = terminal(8, 3, b"\x1b#6\x1b[1;3H\x1b[J");
    assert_eq!(size(&term, 0), LineSize::DoubleWidth);
    let term = terminal(8, 3, b"\x1b#6\x1b[2;8H\x1b#6\x1b[1J");
    assert_eq!([size(&term, 0), size(&term, 1)], [LineSize::Single, LineSize::DoubleWidth]);
    let term = terminal(8, 3, b"\x1b#6\x1b#8");
    assert_eq!(size(&term, 0), LineSize::Single);
}

#[test]
fn writing_stops_at_the_visible_half() {
    let term = terminal(8, 2, b"\x1b#6\x1b[8GAB");
    assert_eq!([text(&term, 0), text(&term, 1)], ["   A    ", "B       "]);
    let term = terminal(8, 2, b"\x1b#6\x1b[8GA\x1b[2b");
    assert_eq!([text(&term, 0), text(&term, 1)], ["   A    ", "AA      "]);
}

#[test]
fn a_cursor_in_the_hidden_half_is_clamped_and_hidden_cells_kept() {
    let term = terminal(20, 4, b"\x1b[1;15H\x1b#6abc\x1b[2;15Ha\x1b[2;15H\x1b#6\x1b[3b");
    assert_eq!(text(&term, 0), "         a          ");
    assert_eq!(text(&term, 1), "bc       a    a     ");
    assert_eq!(text(&term, 2), "aa                  ");
}

#[test]
fn a_wide_character_that_does_not_fit_the_half_wraps() {
    let term = terminal(20, 2, "\x1b#6\x1b[1;15H日".as_bytes());
    assert!(term.grid[Line(0)][Column(9)].flags.contains(Flags::LEADING_WIDE_CHAR_SPACER));
    assert_eq!(term.grid[Line(1)][Column(0)].c, '日');
}

#[test]
fn a_bulk_run_changes_width_with_each_row_it_reaches() {
    let term = terminal(10, 4, &[b"\x1b[2;1H\x1b#6\x1b[1;1H".as_slice(), &[b'a'; 25]].concat());
    assert_eq!([0, 1, 2].map(|line| text(&term, line)), ["aaaaaaaaaa", "aaaaa     ", "aaaaaaaaaa"]);
}

/// Printing skips the cursor row's size while no row of the screen is doubled, so whatever
/// brings a doubled row back has to say so, and scrolling the last one away has to notice.
#[test]
fn doubled_rows_are_tracked_while_on_screen() {
    let mut term = terminal(8, 2, b"\x1b#6\n\n");
    assert_eq!(term.grid.history_size(), 1);
    assert!(!term.doubled_rows, "scrolled into history");

    // Growing the screen brings the row back from history.
    term.resize(TermSize::new(8, 3));
    assert_eq!(size(&term, 0), LineSize::DoubleWidth);
    assert!(term.doubled_rows, "back from history");
    Processor::<StdSyncHandler>::new().advance(&mut term, b"\x1b[1;8HA");
    assert_eq!(text(&term, 0), "   A    ");

    // Leaving the alternate screen brings back the primary's.
    let mut term = terminal(8, 3, b"\x1b#6\x1b[?1049h");
    assert!(!term.doubled_rows, "alternate screen");
    Processor::<StdSyncHandler>::new().advance(&mut term, b"\x1b[?1049l\x1b[1;8HA");
    assert!(term.doubled_rows, "primary screen");
    assert_eq!(text(&term, 0), "   A    ");
}
