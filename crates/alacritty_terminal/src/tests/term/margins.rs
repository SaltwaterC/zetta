//! Rectangular scrolling and origin-relative cursor regressions.
use super::*;
use crate::event::VoidListener;
use crate::term::test::TermSize;
use crate::vte::ansi::{Processor, StdSyncHandler};

fn parse(bytes: &[u8]) -> Term<VoidListener> {
    let mut term = Term::new(Config::default(), &TermSize::new(6, 3), VoidListener);
    Processor::<StdSyncHandler>::new().advance(&mut term, bytes);
    term
}

#[test]
fn rectangular_scroll_preserves_outside_columns_and_has_no_history() {
    let term = parse(b"ABCDEF\x1b[2;1Habcdef\x1b[3;1H123456\x1b[?69h\x1b[2;5s\x1b[S");
    let lines: Vec<String> =
        (0..3).map(|line| term.grid[Line(line)][..].iter().map(|c| c.c).collect()).collect();
    assert_eq!(lines, ["AbcdeF", "a2345f", "1    6"]);
    assert_eq!(term.history_size(), 0);
}

#[test]
fn relative_motion_does_not_add_the_origin_again() {
    let term = parse(b"\x1b[2;3r\x1b[?69h\x1b[2;5s\x1b[?6h\x1b[1;1H\x1b[B\x1b[C");
    assert_eq!(term.grid.cursor.point, Point::new(Line(2), Column(2)));
}
