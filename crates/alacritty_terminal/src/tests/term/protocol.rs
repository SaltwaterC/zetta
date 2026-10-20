//! Regression tests for streamed queries, cursor modes and rectangular edits.
use super::*;
use crate::event::WindowSize;
use crate::term::test::TermSize;
use crate::vte::ansi::{Processor, StdSyncHandler};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default)]
struct Listener(Rc<RefCell<Vec<u8>>>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(reply) => self.0.borrow_mut().extend(reply.bytes()),
            Event::PtyWriteRaw(reply) => self.0.borrow_mut().extend(reply),
            _ => (),
        }
    }
}

fn terminal() -> (Term<Listener>, Processor<StdSyncHandler>, Listener) {
    let listener = Listener::default();
    let term = Term::new(Config::default(), &TermSize::new(8, 4), listener.clone());
    (term, Processor::new(), listener)
}

#[test]
fn replies_preserve_c1_wire_bytes_and_cancelled_dcs_stays_silent() {
    let (mut term, mut parser, listener) = terminal();
    for &byte in b"\x1b G\x1bP$qm\x1b\\" {
        parser.advance(&mut term, &[byte]);
    }
    assert_eq!(&*listener.0.borrow(), b"\x901$r0m\x9c");
    listener.0.borrow_mut().clear();
    for sequence in [b"\x1bP$qz\x18".as_slice(), b"\x1bP$qz\x1b\x1b\\"] {
        parser.advance(&mut term, sequence);
    }
    assert!(listener.0.borrow().is_empty());
}

#[test]
fn soft_reset_keeps_text_and_c1_but_resets_templates_and_cursor_save() {
    let (mut term, mut parser, listener) = terminal();
    parser.advance(&mut term, b"hello\x1b7\x1b G\x1b[1;31m\x1b[!p\x1b8\x1bP$qm\x1b\\");
    assert_eq!(term.grid.cursor.point, Point::new(Line(0), Column(0)));
    assert_eq!(term.grid[Line(0)][Column(0)].c, 'h');
    assert_eq!(term.grid.cursor.template, Cell::default());
    assert_eq!(&*listener.0.borrow(), b"\x901$r0m\x9c");
}

#[test]
fn saved_modes_apply_transitions_instead_of_restoring_a_global_mask() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"\x1b[?1000h\x1b[?1000s\x1b[?1003h\x1b[?1000r");
    assert!(term.mode.contains(TermMode::MOUSE_REPORT_CLICK));
    assert!(!term.mode.contains(TermMode::MOUSE_MOTION));
    parser.advance(&mut term, b"\x1b[?7s\x1b[?7l\x1b[?1h\x1b[?7r");
    assert!(term.mode.contains(TermMode::LINE_WRAP | TermMode::APP_CURSOR));
}

#[test]
fn cursor_save_restores_origin_and_charset_on_each_screen() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(
        &mut term,
        b"\x1b[2;4r\x1b[?6h\x1b)0\x0e\x1b7\x1b[?47h\x1b[?6l\x0f\x1b7\x1b[?47l\x1b8",
    );
    assert!(term.mode.contains(TermMode::ORIGIN));
    assert_eq!(term.active_charset, CharsetIndex::G1);
    assert_eq!(term.grid.cursor.point, Point::new(Line(1), Column(0)));
    parser.advance(&mut term, b"\x1b[?47h\x1b8");
    assert!(!term.mode.contains(TermMode::ORIGIN));
    assert_eq!(term.active_charset, CharsetIndex::G0);
}

#[test]
fn selective_erasure_distinguishes_dec_protection_and_iso_guarded_cells() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"A\x1bVB\x1bWC\x1b[1\"qD\x1b[0\"qE\x1b[H\x1b[?2K");
    assert_eq!(term.grid[Line(0)][Column(1)].c, ' ');
    assert_eq!(term.grid[Line(0)][Column(3)].c, 'D');
    parser.advance(&mut term, b"\x1bcA\x1bVB\x1bWC\x1b[1\"qD\x1b[0\"qE\x1b[H\x1b[2K");
    assert_eq!(term.grid[Line(0)][Column(1)].c, 'B');
    assert_eq!(term.grid[Line(0)][Column(3)].c, ' ');
}

#[test]
fn erased_cells_keep_colors_and_clear_attributes_and_drawn_state() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"\x1b[1;4;31;42mabc\r\x1b[2X");
    let cell = &term.grid[Line(0)][Column(0)];
    assert_eq!(cell.fg, Color::Named(NamedColor::Red));
    assert_eq!(cell.bg, Color::Named(NamedColor::Green));
    assert!(cell.flags.is_empty());
}

#[test]
fn rectangular_copy_handles_overlap_and_does_not_change_the_cursor() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"ABCDEFGH\x1b[2;1Habcdefgh\x1b[1;1;2;3;1;2;3;1$v");
    assert_eq!(term.grid[Line(1)][..].iter().map(|c| c.c).collect::<String>(), "abABCfgh");
    assert_eq!(term.grid[Line(2)][..].iter().map(|c| c.c).collect::<String>(), "  abc   ");
    assert_eq!(term.grid.cursor.point, Point::new(Line(1), Column(7)));
}

#[test]
fn rectangle_attributes_preserve_layout_and_do_not_leak_into_cursor_style() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, "界\x1b[1;1;1;2;1;4:3;53;38:2:1:2:3$r".as_bytes());
    let cell = &term.grid[Line(0)][Column(0)];
    assert!(
        cell.flags.contains(Flags::WIDE_CHAR | Flags::BOLD | Flags::UNDERCURL | Flags::OVERLINE)
    );
    assert_eq!(cell.fg, Color::Spec(Rgb { r: 1, g: 2, b: 3 }));
    assert!(term.grid[Line(0)][Column(1)].flags.contains(Flags::WIDE_CHAR_SPACER));
    assert_eq!(term.grid.cursor.template, Cell::default());
}

#[test]
fn checksum_distinguishes_written_spaces_and_cleared_cells() {
    let (mut term, mut parser, listener) = terminal();
    parser.advance(&mut term, b" \x1b[9;1;1;1;1;2*y");
    assert_eq!(&*listener.0.borrow(), b"\x1bP9!~FFE0\x1b\\");
    listener.0.borrow_mut().clear();
    parser.advance(&mut term, b"\r\x1b[2X\x1b[9;1;1;1;1;2*y");
    assert_eq!(&*listener.0.borrow(), b"\x1bP9!~0000\x1b\\");
}

#[test]
fn reverse_wrapping_respects_hard_boundaries_until_extended_mode() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"12345678X\x1b[?45h\x1b[2;1H\x08");
    assert_eq!(term.grid.cursor.point, Point::new(Line(0), Column(7)));
    parser.advance(&mut term, b"\x1b[3;1H\x08");
    assert_eq!(term.grid.cursor.point, Point::new(Line(2), Column(0)));
    parser.advance(&mut term, b"\x1b[?1045h\x1b[2D");
    assert_eq!(term.grid.cursor.point, Point::new(Line(1), Column(6)));
}

#[test]
fn vt52_addressing_is_streamed_and_escape_d_changes_meaning() {
    let (mut term, mut parser, listener) = terminal();
    for &byte in b"\x1b[?2l\x1bY\x22\x24X\x1bD\x1bZ" {
        parser.advance(&mut term, &[byte]);
    }
    assert_eq!(term.grid[Line(2)][Column(4)].c, 'X');
    assert_eq!(term.grid.cursor.point, Point::new(Line(2), Column(4)));
    assert_eq!(&*listener.0.borrow(), b"\x1b/Z");
    listener.0.borrow_mut().clear();
    parser.advance(&mut term, b"\x1b<\x1b[?7$p\x1b[64;1\"p\x1b[?7$p");
    assert_eq!(&*listener.0.borrow(), b"\x1b[?7;1$y");
}

#[test]
fn latin1_c1_controls_restart_csi_and_terminate_ignored_strings() {
    let (mut term, mut parser, _) = terminal();
    for &byte in b"\x1b%@\x1b[123\x9b2;3HX\x9fignored\x9cY\xe9" {
        parser.advance(&mut term, &[byte]);
    }
    assert_eq!(term.grid[Line(1)][Column(2)].c, 'X');
    assert_eq!(term.grid[Line(1)][Column(3)].c, 'Y');
    assert_eq!(term.grid[Line(1)][Column(4)].c, 'é');
    parser.advance(&mut term, "\x1b%G界".as_bytes());
    assert_eq!(term.grid[Line(1)][Column(5)].c, '界');
}

#[test]
fn selective_sgr_pop_preserves_live_colors_and_protection() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"\x1b[1;3;31m\x1b[1;3#{\x1b[22;23;34m\x1b[1\"q\x1b[#}X");
    let cell = &term.grid[Line(0)][Column(0)];
    assert!(cell.flags.contains(Flags::BOLD | Flags::ITALIC | Flags::PROTECTED));
    assert_eq!(cell.fg, Color::Named(NamedColor::Blue));
    for _ in 0..20 {
        parser.advance(&mut term, b"\x1b[#{");
    }
    assert_eq!(term.protocol.sgr_stack.len(), 10);
}

#[test]
fn pending_wrap_survives_tab_but_plain_backspace_moves_left() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"12345678\tX");
    assert_eq!(term.grid[Line(0)][Column(7)].c, '8');
    assert_eq!(term.grid[Line(1)][Column(0)].c, 'X');
    parser.advance(&mut term, b"\x1b[H12345678\x08Y");
    assert_eq!(term.grid[Line(0)][Column(6)].c, 'Y');
    assert_eq!(term.grid[Line(0)][Column(7)].c, '8');
}

#[test]
fn unknown_underline_styles_preserve_the_previous_style() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"\x1b[4:3m\x1b[4:9mX\x1b[21mY");
    assert!(term.grid[Line(0)][Column(0)].flags.contains(Flags::UNDERCURL));
    assert!(term.grid[Line(0)][Column(1)].flags.contains(Flags::DOUBLE_UNDERLINE));
}

#[test]
fn primary_device_attributes_list_only_implemented_features() {
    let (mut term, mut parser, listener) = terminal();
    parser.advance(&mut term, b"\x1b[c\x1b[0c");
    let expected = PRIMARY_DEVICE_ATTRIBUTES.repeat(2);
    assert_eq!(&*listener.0.borrow(), expected.as_bytes());
}

#[test]
fn line_feeds_and_erases_cancel_a_pending_wrap() {
    // LF, IND and RI move from the last column without wrapping first.
    for (sequence, line) in [(&b"\n"[..], 2), (b"\x1bD", 2), (b"\x1bM", 0)] {
        let (mut term, mut parser, _) = terminal();
        parser.advance(&mut term, b"\x1b[2;1Habcdefgh");
        assert!(term.grid.cursor.input_needs_wrap);
        parser.advance(&mut term, sequence);
        assert!(!term.grid.cursor.input_needs_wrap, "{sequence:?}");
        assert_eq!(term.grid.cursor.point, Point::new(Line(line), Column(7)), "{sequence:?}");
    }
    // Every EL clears it; EL 0 at the last column erases that column.
    for (mode, kept) in [(0, 'g'), (1, ' '), (2, ' ')] {
        let (mut term, mut parser, _) = terminal();
        parser.advance(&mut term, format!("abcdefgh\x1b[{mode}KX").as_bytes());
        assert_eq!(term.grid[Line(0)][Column(6)].c, kept, "EL {mode}");
        assert_eq!(term.grid[Line(0)][Column(7)].c, 'X', "EL {mode}");
        assert_eq!(term.grid.cursor.point.line, Line(0), "EL {mode}");
    }
}

#[test]
fn national_sets_need_decnrcm_but_british_does_not() {
    let (mut term, mut parser, _) = terminal();
    parser.advance(&mut term, b"\x1b(K[\x1b(A#\x1b[?42h\x1b(K[{");
    let row: String = term.grid[Line(0)][..Column(4)].iter().map(|cell| cell.c).collect();
    assert_eq!(row, "[£Ää");
}

#[test]
fn decudk_lock_is_reported_by_the_udk_status() {
    let (mut term, mut parser, listener) = terminal();
    parser.advance(&mut term, b"\x1b[?25n\x1bP1;1|\x1b\\\x1b[?25n\x1bP1;0|\x1b\\\x1b[?25n");
    assert_eq!(&*listener.0.borrow(), b"\x1b[?20n\x1b[?20n\x1b[?21n");
}

#[test]
fn in_band_resize_reports_each_resize_at_the_frontend_geometry() {
    #[derive(Clone, Default)]
    struct Frontend(Rc<RefCell<Vec<String>>>);
    impl EventListener for Frontend {
        fn send_event(&self, event: Event) {
            if let Event::TextAreaSizeRequest(format) = event {
                let size = WindowSize { num_lines: 5, num_cols: 9, cell_width: 7, cell_height: 15 };
                self.0.borrow_mut().push(format(size));
            }
        }
    }
    let frontend = Frontend::default();
    let mut term = Term::new(Config::default(), &TermSize::new(8, 4), frontend.clone());
    let mut parser = Processor::<StdSyncHandler>::new();
    term.resize(TermSize::new(9, 5));
    assert!(frontend.0.borrow().is_empty());
    // Setting the mode reports at once, then every resize does until reset.
    parser.advance(&mut term, b"\x1b[?2048h");
    term.resize(TermSize::new(10, 6));
    parser.advance(&mut term, b"\x1b[?2048l");
    term.resize(TermSize::new(9, 5));
    assert_eq!(*frontend.0.borrow(), ["\x1b[48;5;9;75;63t"; 2]);
}
