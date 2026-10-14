use super::*;
use alacritty_terminal::index::{Column, Line};
#[test]
fn parser_retains_vt_state_across_write_requests() {
    let mut sink = Sink::new(false, None, 500).unwrap();
    sink.write(b"\x1b[3").unwrap();
    sink.write(b"1mA\nB").unwrap();
    let term = sink.term.as_ref().unwrap();
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'A');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, 'B');
    assert_eq!(sink.cursor(), (1, 1));
}
