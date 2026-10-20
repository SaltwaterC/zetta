//! Regression coverage for character edits and the wide-cell invariant they preserve.

use super::*;
use crate::event::VoidListener;
use crate::term::test::TermSize;
use crate::vte::ansi::Processor;

fn terminal(columns: usize, rows: usize, bytes: &[u8]) -> Term<VoidListener> {
    let mut terminal = Term::new(Config::default(), &TermSize::new(columns, rows), VoidListener);
    Processor::<crate::vte::ansi::StdSyncHandler>::new().advance(&mut terminal, bytes);
    terminal
}

fn assert_complete_wide_cells(term: &Term<VoidListener>) {
    for cell in term.grid.display_iter() {
        let point = cell.point;
        if cell.flags.contains(Flags::WIDE_CHAR) {
            assert!(point.column < term.last_column());
            assert!(
                term.grid[point.line][point.column + 1].flags.contains(Flags::WIDE_CHAR_SPACER)
            );
        }
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            assert!(point.column > 0);
            assert!(term.grid[point.line][point.column - 1].flags.contains(Flags::WIDE_CHAR));
        }
    }
    // Cursor rendering must also be safe after editing at column zero.
    let _ = term.renderable_content();
}

#[test]
fn editing_wide_characters_repairs_both_clipped_boundaries() {
    for operation in ["@", "P", "X"] {
        for column in 1..=8 {
            for count in [1, 2, 7, 8, usize::MAX] {
                let mut term = terminal(8, 3, "A界BCDEF".as_bytes());
                term.goto(0, column - 1);
                match operation {
                    "@" => term.insert_blank(count),
                    "P" => term.delete_chars(count),
                    "X" => term.erase_chars(count),
                    _ => unreachable!(),
                }
                assert_complete_wide_cells(&term);
            }
        }
    }
}

#[test]
fn deletion_at_right_edge_does_not_erase_cells_before_the_cursor() {
    let term = terminal(6, 2, b"ABCDEF\x1b[1;6H\x1b[99P");
    let text: String = term.grid[Line(0)][..].iter().map(|cell| cell.c).collect();
    assert_eq!(text, "ABCDE ");
}

#[test]
fn deleting_half_a_wide_character_clears_the_retained_half() {
    for (input, expected) in
        [("A橋123\x1b[1;3H\x1b[P", "A 123 "), ("橋123\x1b[1;1H\x1b[P", " 123  ")]
    {
        let term = terminal(6, 2, input.as_bytes());
        let text: String = term.grid[Line(0)][..].iter().map(|cell| cell.c).collect();
        assert_eq!(text, expected);
        assert_complete_wide_cells(&term);
    }
}

#[test]
fn character_edits_cancel_pending_wrap() {
    for operation in *b"@PX" {
        let mut term = terminal(4, 2, b"abcd");
        Processor::<crate::vte::ansi::StdSyncHandler>::new()
            .advance(&mut term, &[0x1b, b'[', operation, b'Z']);
        assert_eq!(term.grid[Line(0)][Column(3)].c, 'Z');
        assert_eq!(term.grid.cursor.point.line, Line(0));
    }
}

#[test]
fn single_column_wide_input_stays_inside_the_grid() {
    for setup in ["", "\x1b[4h", "\x1b[?1049h", "\x1b[?7l"] {
        let term = terminal(1, 2, format!("{setup}😀😀a").as_bytes());
        assert_complete_wide_cells(&term);
        assert_eq!(term.grid.cursor.point.column, Column(0));
    }
    let term = terminal(1, 2, "😀".as_bytes());
    assert_eq!(term.grid[Line(1)][Column(0)].c, '😀');
}

#[test]
fn shrink_to_one_column_terminates_and_discards_unrepresentable_wide_cells() {
    for rows in [1, 2, 3] {
        for setup in ["", "\x1b[?1049h"] {
            let mut term = terminal(2, rows, format!("{setup}😀").as_bytes());
            term.resize(TermSize::new(1, rows));
            assert_complete_wide_cells(&term);
            for cell in term.grid.display_iter() {
                assert_eq!(cell.c, ' ');
            }
        }
    }
}

#[test]
fn line_and_display_erase_repair_wide_boundaries() {
    for column in 1..=8 {
        for operation in ["K", "1K", "2K", "J", "1J"] {
            let bytes = format!("A界BC界Z\x1b[1;{column}H\x1b[{operation}");
            let term = terminal(8, 3, bytes.as_bytes());
            assert_complete_wide_cells(&term);
        }
    }
}

#[test]
fn insert_mode_repairs_wide_characters_cut_off_at_the_right_edge() {
    for column in 1..=8 {
        let bytes = format!("abcdef界\x1b[4h\x1b[1;{column}HX");
        let term = terminal(8, 3, bytes.as_bytes());
        assert_complete_wide_cells(&term);
    }
}

#[test]
fn erase_above_on_the_second_row_also_clears_the_first_row() {
    let term = terminal(8, 3, b"first\r\nsecond\x1b[1J");
    assert!(term.grid[Line(0)][..].iter().all(|cell| cell.c == ' '));
    assert!(term.grid[Line(1)][..].iter().all(|cell| cell.c == ' '));
}

#[test]
fn stale_spacer_at_column_zero_cannot_underflow_render_or_selection() {
    let mut term = terminal(4, 2, b"");
    term.grid[Line(0)][Column(0)].flags.insert(Flags::WIDE_CHAR_SPACER);
    assert_eq!(term.renderable_content().cursor.point.column, Column(0));
    let point = Point::new(Line(0), Column(0));
    assert_eq!(term.expand_wide(point, Direction::Left), point);
}
