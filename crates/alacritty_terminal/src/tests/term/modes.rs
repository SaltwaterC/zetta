//! Mode transitions, and what DECRQM reports for them.
use super::*;
use crate::term::test::TermSize;
use crate::vte::ansi::{Processor, StdSyncHandler};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default)]
struct Listener(Rc<RefCell<Vec<u8>>>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(reply) = event {
            self.0.borrow_mut().extend(reply.bytes());
        }
    }
}

fn terminal() -> (Term<Listener>, Processor<StdSyncHandler>, Listener) {
    let listener = Listener::default();
    let term = Term::new(Config::default(), &TermSize::new(8, 4), listener.clone());
    (term, Processor::new(), listener)
}

/// The DECRQM state (1 set, 2 reset) of each mode, ANSI ones unprefixed.
fn reported(
    term: &mut Term<Listener>,
    parser: &mut Processor<StdSyncHandler>,
    listener: &Listener,
    modes: &[&str],
) -> Vec<u8> {
    modes
        .iter()
        .map(|mode| {
            let private = mode.strip_prefix('?');
            let number = private.unwrap_or(mode);
            let prefix = if private.is_some() { "?" } else { "" };
            parser.advance(term, format!("\x1b[{prefix}{number}$p").as_bytes());
            let reply = String::from_utf8(listener.0.take()).unwrap();
            let expected = format!("\x1b[{prefix}{number};");
            assert!(reply.starts_with(&expected), "{reply:?}");
            reply.as_bytes()[expected.len()] - b'0'
        })
        .collect()
}

#[test]
fn keyboard_action_and_send_receive_modes_lock_input_and_echo_it() {
    let (mut term, mut parser, listener) = terminal();
    assert_eq!(reported(&mut term, &mut parser, &listener, &["2", "12"]), [2, 1]);
    assert!(!term.mode.intersects(TermMode::KEYBOARD_LOCKED | TermMode::LOCAL_ECHO));
    parser.advance(&mut term, b"\x1b[2h\x1b[12l");
    assert!(term.mode.contains(TermMode::KEYBOARD_LOCKED | TermMode::LOCAL_ECHO));
    assert_eq!(reported(&mut term, &mut parser, &listener, &["2", "12"]), [1, 2]);
    parser.advance(&mut term, b"\x1bc");
    assert!(!term.mode.intersects(TermMode::KEYBOARD_LOCKED | TermMode::LOCAL_ECHO));
}

#[test]
fn input_modes_report_their_flags_including_the_inverted_one() {
    let (mut term, mut parser, listener) = terminal();
    let modes = ["?67", "?1034", "?1036", "?1039"];
    // Alt sends ESC by default, so 1036 and 1039 start set.
    assert_eq!(reported(&mut term, &mut parser, &listener, &modes), [2, 2, 1, 1]);
    parser.advance(&mut term, b"\x1b[?67h\x1b[?1034h\x1b[?1039l");
    assert!(term.mode.contains(TermMode::BACKARROW_SENDS_BS | TermMode::META_SETS_EIGHTH_BIT));
    assert!(term.mode.contains(TermMode::ALT_ESC_DISABLED));
    // 1036 and 1039 are one state under two numbers.
    assert_eq!(reported(&mut term, &mut parser, &listener, &modes), [1, 1, 2, 2]);
    parser.advance(&mut term, b"\x1b[?1036h");
    assert!(!term.mode.contains(TermMode::ALT_ESC_DISABLED));
}

#[test]
fn sgr_pixel_mouse_replaces_the_other_extended_encodings() {
    let (mut term, mut parser, listener) = terminal();
    parser.advance(&mut term, b"\x1b[?1006h\x1b[?1016h");
    assert!(term.mode.contains(TermMode::SGR_PIXEL_MOUSE));
    assert!(!term.mode.intersects(TermMode::SGR_MOUSE | TermMode::UTF8_MOUSE));
    assert_eq!(reported(&mut term, &mut parser, &listener, &["?1006", "?1016"]), [2, 1]);
    parser.advance(&mut term, b"\x1b[?1005h");
    assert!(!term.mode.contains(TermMode::SGR_PIXEL_MOUSE));
}

#[test]
fn nrcs_and_in_band_resize_are_reported() {
    let (mut term, mut parser, listener) = terminal();
    assert_eq!(reported(&mut term, &mut parser, &listener, &["?42", "?2048"]), [2, 2]);
    parser.advance(&mut term, b"\x1b[?42h");
    assert_eq!(reported(&mut term, &mut parser, &listener, &["?42"]), [1]);
}
