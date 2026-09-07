//! The prediction engine's decisions, driven the way a session drives
//! it: type bytes, let a pretend server answer, look at what would be
//! drawn.

use super::*;
use crate::screen::{OverlayCell, OverlayCursor, Vt100Screen};

/// A client and the server it is guessing about. The screen IS the
/// confirmed one, so `server()` is the only thing that can make a
/// prediction come true.
struct Harness {
    engine: PredictionEngine,
    screen: Vt100Screen,
    now: u64,
    cols: u16,
}

impl Harness {
    fn new(pref: DisplayPreference) -> Self {
        Self::sized(pref, 5, 20)
    }

    fn sized(pref: DisplayPreference, rows: u16, cols: u16) -> Self {
        let mut engine = PredictionEngine::new();
        engine.set_display_preference(pref);
        // A link slow enough for Adaptive to want predictions, so the
        // preference under test is the only thing deciding.
        engine.set_send_interval(100);
        Self {
            engine,
            screen: Vt100Screen::new(rows, cols),
            now: 1_000,
            cols,
        }
    }

    /// The user types.
    fn types(&mut self, input: &str) {
        self.engine
            .new_user_bytes(input.as_bytes(), &self.screen, self.now);
    }

    fn types_bytes(&mut self, input: &[u8]) {
        self.engine.new_user_bytes(input, &self.screen, self.now);
    }

    /// The state carrying what was just typed goes out.
    fn sent(&mut self, num: u64) {
        self.engine.set_local_frame_sent(num);
    }

    /// The server answers: it paints `bytes` and reports having echoed
    /// our state `echo_ack`.
    fn server(&mut self, bytes: &[u8], echo_ack: u64) {
        self.screen.feed(bytes);
        self.engine.set_local_frame_acked(echo_ack);
        self.engine.set_local_frame_late_acked(echo_ack);
        self.engine.cull(&self.screen, self.now);
    }

    fn cells(&self) -> Vec<OverlayCell> {
        self.engine.overlay(&self.screen).0
    }

    fn nothing_drawn(&self) -> bool {
        let (cells, _) = self.engine.overlay(&self.screen);
        cells.is_empty()
    }

    /// Is this character among the guesses being shown?
    fn shows(&self, ch: char) -> bool {
        self.cells()
            .iter()
            .any(|c| c.cell.contents == ch.to_string())
    }

    /// The guess painted at a character, if any.
    fn paint_of(&self, ch: char) -> Option<OverlayCell> {
        self.cells()
            .into_iter()
            .find(|c| c.cell.contents == ch.to_string())
    }

    /// Is anything being marked as a guess?
    fn underlines(&self) -> bool {
        self.cells().iter().any(|c| c.underline)
    }

    /// What the user would see: the confirmed screen with the overlay
    /// painted over it.
    fn painted_screen(&self) -> Vt100Screen {
        let (cells, cursor) = self.engine.overlay(&self.screen);
        let mut mirror = self.screen.clone();
        mirror.draw_overlay(
            &cells,
            cursor.map_or(OverlayCursor::Unchanged, |(r, c)| OverlayCursor::At(r, c)),
        );
        mirror
    }

    fn painted(&self) -> String {
        self.painted_screen().text()
    }

    fn painted_line(&self, row: u16) -> String {
        let screen = self.painted_screen();
        (0..self.cols)
            .map(|col| {
                let cell = screen.cell(row, col);
                if cell.contents.is_empty() {
                    " ".to_string()
                } else {
                    cell.contents
                }
            })
            .collect::<String>()
    }

    /// Type one character and have the server echo it, which is what
    /// confirms the epoch and unlocks drawing for everything after.
    fn warm_up(&mut self) {
        self.types("a");
        self.sent(1);
        self.server(b"a", 1);
        assert!(
            self.engine.confirmed_epoch >= 1,
            "the first prediction should have confirmed its epoch"
        );
    }
}

#[test]
fn the_first_prediction_is_made_but_not_shown() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.types("a");
    // The guess exists...
    assert!(h.engine.active());
    // ...but nothing is drawn, because nothing has yet proven that
    // typing into this situation echoes at all.
    assert!(h.nothing_drawn());
    assert_eq!(h.painted().trim(), "");
}

#[test]
fn a_confirmed_epoch_lets_the_next_character_appear_at_once() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();

    // Now the second character is drawn immediately, with the server
    // still knowing nothing about it.
    h.types("b");
    assert!(!h.nothing_drawn(), "a confirmed epoch should draw");
    assert_eq!(h.painted().trim(), "ab");
    // And the confirmed screen has not changed.
    assert_eq!(h.screen.text().trim(), "a");
}

#[test]
fn the_predicted_cursor_leads_the_confirmed_one() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bcd");
    assert_eq!(h.screen.cursor(), (0, 1));
    assert_eq!(
        h.painted_screen().cursor(),
        (0, 4),
        "the cursor should lead by three"
    );
}

#[test]
fn a_wrong_prediction_takes_every_prediction_with_it() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bcd");
    assert!(!h.nothing_drawn());

    // The server did something else entirely with those keystrokes.
    h.sent(2);
    h.server(b"\r\nZZZ", 2);

    assert!(
        !h.engine.active(),
        "one wrong guess should retire the ones built on it"
    );
    assert!(h.nothing_drawn());
    // What the user sees is the server's screen, nothing else.
    assert_eq!(h.painted(), h.screen.text());
}

#[test]
fn backspace_pulls_the_rest_of_the_line_left() {
    let mut h = Harness::new(DisplayPreference::Always);
    // The screen reads "abc" with the cursor after the 'a'.
    h.server(b"abc\x1b[1;2H", 0);

    // Insert a Z there and have the server agree, which confirms the
    // epoch on a line that has content to shift.
    h.types("Z");
    h.sent(1);
    h.server(b"\x1b[1;1HaZbc\x1b[1;3H", 1);
    // The first cull always bumps the epoch (the engine learns the
    // screen size), so what matters is that SOME epoch got confirmed.
    assert!(h.engine.confirmed_epoch >= 1);

    // Backspace over the Z: everything right of the cursor comes back
    // one column, which is what an insert-mode line editor does.
    h.types("\x7f");
    assert!(!h.nothing_drawn(), "backspace should draw something");
    assert_eq!(
        h.painted_line(0).trim_end(),
        "abc",
        "the line should have been pulled left"
    );
    // The server still believes otherwise.
    assert_eq!(h.screen.text().trim_end(), "aZbc");
}

#[test]
fn del_reaches_the_engine_even_though_the_parser_ignores_it() {
    // vte follows the VT500 tables, where DEL is ignored in every
    // state, so a prediction engine that only listened to `print`
    // would never predict Backspace at all. This is the regression
    // test for the interception.
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bc");
    let before = h.engine.cursor().col;
    h.types_bytes(&[0x7f]);
    assert_eq!(
        h.engine.cursor().col,
        before - 1,
        "DEL should have moved the predicted cursor back"
    );
}

#[test]
fn del_inside_an_escape_sequence_is_not_a_backspace() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bc");
    let before = h.engine.cursor().col;
    // Half an escape sequence, then DEL: the tables ignore it there,
    // and so must we.
    h.types_bytes(&[0x1b, b'[', 0x7f]);
    assert_eq!(
        h.engine.cursor().col,
        before,
        "DEL mid-sequence is not a key"
    );
}

#[test]
fn carriage_return_returns_the_cursor_and_stops_new_guesses() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bc");
    assert!(!h.nothing_drawn());

    h.types("\r");
    assert_eq!(h.engine.cursor().col, 0);
    assert_eq!(h.engine.cursor().row, 1);
    // A return means a command is about to run, and what it prints is
    // not something we can guess at: what was already drawn stays, but
    // nothing new is shown until a prediction proves itself again.
    h.types("q");
    assert!(
        !h.shows('q'),
        "a guess made after a return must wait for proof: {:?}",
        format!("{:?}", h.cells())
    );
}

#[test]
fn an_escape_sequence_stops_new_guesses() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("b");
    assert!(h.shows('b'));
    // Anything we do not model, here a clear-screen, makes the engine
    // stop trusting its picture of what typing does.
    h.types("\x1b[2J");
    h.types("q");
    assert!(!h.shows('q'));
}

#[test]
fn the_arrow_keys_move_the_predicted_cursor() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bcd");
    let col = h.engine.cursor().col;
    h.types("\x1b[D");
    assert_eq!(h.engine.cursor().col, col - 1, "left arrow");
    h.types("\x1b[C");
    assert_eq!(h.engine.cursor().col, col, "right arrow");
}

#[test]
fn application_mode_arrows_are_the_same_keys() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bcd");
    let col = h.engine.cursor().col;
    // `ESC O D` is what a terminal in application cursor mode sends.
    h.types("\x1bOD");
    assert_eq!(h.engine.cursor().col, col - 1);
}

#[test]
fn the_left_arrow_stops_at_the_edge() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("\x1b[D\x1b[D\x1b[D\x1b[D");
    assert_eq!(h.engine.cursor().col, 0);
}

#[test]
fn a_control_byte_we_do_not_model_stops_new_guesses() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("b");
    assert!(h.shows('b'));
    h.types_bytes(&[0x03]); // ^C
    h.types("q");
    assert!(!h.shows('q'));
}

#[test]
fn a_wide_character_is_not_guessed_at() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("b");
    // Two cells wide: where the next character lands is a second guess
    // on top of the first, which mosh declines to make.
    h.types("世");
    assert!(!h.shows('世'));
    h.types("q");
    assert!(!h.shows('q'));
}

#[test]
fn an_accented_character_is_guessed_at_like_any_other() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    // One cell wide, so it predicts: the UTF-8 bytes have to be
    // decoded before the width test, which is vte's job here.
    h.types("é");
    assert!(h.painted().contains('é'));
}

#[test]
fn never_predicts_nothing_at_all() {
    let mut h = Harness::new(DisplayPreference::Never);
    h.types("abc");
    assert!(!h.engine.active(), "Never should not even make predictions");
    assert!(h.nothing_drawn());
}

#[test]
fn adaptive_stays_out_of_the_way_on_a_fast_link() {
    let mut h = Harness::new(DisplayPreference::Adaptive);
    // A local link: the round trip is faster than any prediction could
    // be useful for.
    h.engine.set_send_interval(20);
    h.warm_up();
    h.types("b");
    assert!(h.engine.active(), "the prediction is still made");
    assert!(
        h.nothing_drawn(),
        "but a fast link has nothing to hide, so nothing is drawn"
    );
}

#[test]
fn adaptive_starts_drawing_once_the_link_is_slow() {
    let mut h = Harness::new(DisplayPreference::Adaptive);
    h.engine.set_send_interval(100);
    h.warm_up();
    h.types("b");
    assert!(h.shows('b'));
}

#[test]
fn a_slow_link_underlines_what_it_is_guessing() {
    let mut h = Harness::new(DisplayPreference::Adaptive);
    h.engine.set_send_interval(200); // past the flagging threshold
    h.warm_up();
    h.types("b");
    assert!(
        h.underlines(),
        "a guess on a slow link should be underlined: {:?}",
        format!("{:?}", h.cells())
    );
}

#[test]
fn a_fast_link_that_stalls_starts_drawing_anyway() {
    let mut h = Harness::new(DisplayPreference::Adaptive);
    h.engine.set_send_interval(20);
    h.warm_up();
    h.types("b");
    assert!(h.nothing_drawn(), "fast link, nothing drawn");

    // The round-trip estimate says fast, but the guess has been sitting
    // unanswered for a third of a second. That is exactly the case the
    // estimate is wrong about.
    h.now += GLITCH_THRESHOLD + 10;
    h.engine.cull(&h.screen, h.now);
    assert!(
        h.shows('b'),
        "a stalled prediction should turn the display on"
    );
}

#[test]
fn a_resize_throws_away_every_prediction() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bcd");
    assert!(h.engine.active());

    h.screen.resize(10, 40);
    h.engine.cull(&h.screen, h.now);
    assert!(
        !h.engine.active(),
        "predictions were made about a screen that no longer exists"
    );
}

#[test]
fn the_last_column_is_never_painted() {
    let mut h = Harness::sized(DisplayPreference::Always, 3, 6);
    h.types("a");
    h.sent(1);
    h.server(b"a", 1);
    // Walk to the far right and keep typing.
    h.types("bcde");
    // The engine still PREDICTS the last column; what declines to
    // paint it is the escape-byte screen, because a write there would
    // arm the terminal's pending wrap. A screen that owns its grid has
    // no such constraint and may paint it.
    assert!(
        h.painted_screen().cell(0, 5).is_blank(),
        "painting by escape must leave the last column to the server"
    );
    // The columns before it are painted.
    assert_eq!(h.painted_line(0).trim_end(), "abcde");
}

#[test]
fn a_prediction_that_only_repeats_what_was_there_confirms_nothing() {
    let mut h = Harness::new(DisplayPreference::Always);
    // The screen already reads "a"; typing 'a' over it would look
    // right whatever the server did, so it must not unlock drawing.
    h.server(b"a\x1b[1;1H", 0);
    h.types("a");
    h.sent(1);
    h.server(b"", 1);
    assert_eq!(
        h.engine.confirmed_epoch, 0,
        "a guess that could not have been wrong proves nothing"
    );
}

#[test]
fn the_overlay_paints_over_the_confirmed_screen_without_changing_it() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.warm_up();
    h.types("bc");
    let confirmed = h.screen.text();
    let _ = h.painted();
    assert_eq!(
        h.screen.text(),
        confirmed,
        "drawing must not touch the screen the server sent"
    );
}

#[test]
fn a_prediction_inherits_the_colour_of_the_text_it_follows() {
    let mut h = Harness::new(DisplayPreference::Always);
    // Red text, cursor just after it.
    h.server(b"\x1b[31mabc", 0);
    h.types("d");
    h.sent(1);
    h.server(b"d", 1);
    // The first cull always bumps the epoch (the engine learns the
    // screen size), so what matters is that SOME epoch got confirmed.
    assert!(h.engine.confirmed_epoch >= 1);

    h.types("e");
    let painted = h.paint_of('e').expect("the guess should be drawn");
    assert_eq!(
        painted.cell.rendition.fg,
        crate::screen::Color::Indexed(1),
        "the guess should take the colour of the line it lands on"
    );
}

#[test]
fn the_display_preference_reads_the_names_mosh_uses() {
    assert_eq!(
        DisplayPreference::parse("always"),
        Some(DisplayPreference::Always)
    );
    assert_eq!(
        DisplayPreference::parse("never"),
        Some(DisplayPreference::Never)
    );
    assert_eq!(
        DisplayPreference::parse("adaptive"),
        Some(DisplayPreference::Adaptive)
    );
    assert_eq!(
        DisplayPreference::parse("experimental"),
        Some(DisplayPreference::Experimental)
    );
    // Anything else is refused rather than guessed at.
    assert_eq!(DisplayPreference::parse("yes"), None);
    assert_eq!(DisplayPreference::default(), DisplayPreference::Adaptive);
}

#[test]
fn experimental_keeps_predicting_after_a_miss() {
    let mut h = Harness::new(DisplayPreference::Experimental);
    h.types("ab");
    h.sent(1);
    // The server disagrees about what those keystrokes did.
    h.server(b"ZZ", 1);

    // Ordinary mode would have gone tentative and hidden everything
    // until a guess proved itself again. Experimental predicts from the
    // confirmed epoch every time, so it draws straight away.
    h.types("c");
    assert!(
        h.shows('c'),
        "experimental mode keeps drawing after a miss: {:?}",
        format!("{:?}", h.cells())
    );
}

#[test]
fn a_pending_prediction_asks_to_be_looked_at_again() {
    let mut h = Harness::new(DisplayPreference::Adaptive);
    assert_eq!(h.engine.wait_time_ms(), None, "nothing outstanding");
    h.types("a");
    assert_eq!(
        h.engine.wait_time_ms(),
        Some(50),
        "an outstanding prediction has to be re-examined on a timer"
    );
}

#[test]
fn overwrite_mode_erases_in_place_instead_of_pulling_the_line_back() {
    let mut h = Harness::new(DisplayPreference::Always);
    h.engine.set_predict_overwrite(true);
    // "abc" with the cursor on the 'c'.
    h.server(b"abc\x1b[1;3H", 0);
    h.types("Z");
    h.sent(1);
    h.server(b"\x1b[1;3HZ", 1);
    // The first cull always bumps the epoch (the engine learns the
    // screen size), so what matters is that SOME epoch got confirmed.
    assert!(h.engine.confirmed_epoch >= 1);

    h.types("\x7f");
    assert_eq!(
        h.painted_line(0).trim_end(),
        "ab",
        "overwrite backspace blanks a cell rather than shifting the line"
    );
}
