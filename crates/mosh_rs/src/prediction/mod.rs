//! Predictive local echo (`terminaloverlay.cc`): the thing mosh is
//! actually known for.
//!
//! On a link with any latency, a terminal that only shows what the
//! server has confirmed feels broken: you type, and the letter appears
//! a round trip later. mosh answers by GUESSING. When you type an
//! ordinary character it draws it immediately, remembers exactly what
//! it drew and where, and later checks that guess against the screen
//! the server eventually sends. A guess that comes true costs nothing;
//! a guess that turns out wrong is thrown away along with every other
//! guess made since, and the confirmed screen repaints over it.
//!
//! What keeps that from being a mess of flickering wrong characters is
//! that mosh is conservative about when it guesses at all:
//!
//! - **It only predicts what it understands.** A printable character,
//!   backspace, carriage return, and the left/right arrows. Anything
//!   else, an escape sequence, a control byte, a wide character, makes
//!   it go TENTATIVE: it keeps predicting, but stops DRAWING until a
//!   prediction is confirmed correct.
//! - **Epochs.** Every prediction is stamped with an epoch. A
//!   prediction is drawn only once its epoch has been confirmed by some
//!   earlier prediction coming true. So the first character you type
//!   into an unknown situation is not shown; the second one is, because
//!   by then the first has proven the echo works. One wrong prediction
//!   kills its whole epoch, so a single mis-guess cannot leave a trail.
//! - **It only shows up when it is needed.** Under `Adaptive`, nothing
//!   is drawn until the link is slow enough to be worth guessing about
//!   (`send_interval > 30 ms`), and predictions are underlined once it
//!   is slow enough that the user deserves to know they are guesses.
//!
//! The engine decides WHICH cells to paint and WITH WHAT;
//! [`PredictionEngine::overlay`] hands that list to the screen, which
//! decides how it gets there. A screen that owns its grid writes the
//! cells straight in, which is what mosh does; one that can only be fed
//! bytes renders them as escapes and has to hold back in two places
//! (the last column, where a write would arm the terminal's pending
//! wrap). Either way the decisions above are unchanged.

mod cell;

use crate::screen::{Cell, OverlayCell, Screen};
use cell::{CellPrediction, CursorMove, OverlayRow, Validity};
use unicode_width::UnicodeWidthChar as _;
use vte::{Params, Perform};

/// Link slow enough to start showing predictions, and the lower edge
/// that stops again. The gap is hysteresis: a link hovering around the
/// threshold must not flicker predictions on and off.
const SRTT_TRIGGER_LOW: u64 = 20;
const SRTT_TRIGGER_HIGH: u64 = 30;

/// Link slow enough that predictions get underlined, so the user can
/// see which characters are guesses.
const FLAG_TRIGGER_LOW: u64 = 50;
const FLAG_TRIGGER_HIGH: u64 = 80;

/// A prediction outstanding this long is a glitch: the link is not
/// as fast as the round-trip estimate claims, so start showing
/// predictions even though the estimate says not to.
const GLITCH_THRESHOLD: u64 = 250;
/// Quick confirmations needed to work off a glitch, and the minimum
/// spacing between them, so one fast burst cannot cure it.
const GLITCH_REPAIR_COUNT: u32 = 10;
const GLITCH_REPAIR_MININTERVAL: u64 = 150;
/// Outstanding THIS long and the predictions get underlined too.
const GLITCH_FLAG_THRESHOLD: u64 = 5000;

/// When predictions are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayPreference {
    /// Always draw them, however fast the link is.
    Always,
    /// Never predict at all.
    Never,
    /// Draw them once the link is slow enough to need them. mosh's
    /// default, and ours.
    #[default]
    Adaptive,
    /// mosh's experimental mode: predict from the confirmed epoch every
    /// time, and drop single wrong predictions instead of the epoch.
    /// More responsive, more willing to be visibly wrong.
    Experimental,
}

impl DisplayPreference {
    /// `MOSH_PREDICTION_DISPLAY`, which is how mosh-client is told;
    /// unrecognized values are refused rather than guessed at.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            "adaptive" => Some(Self::Adaptive),
            "experimental" => Some(Self::Experimental),
            _ => None,
        }
    }
}

/// The predictions in flight, and the machinery that decides which of
/// them the user gets to see.
pub struct PredictionEngine {
    display_preference: DisplayPreference,
    /// Predict overwriting rather than inserting. mosh's
    /// `MOSH_PREDICTION_OVERWRITE`: right for a shell in overwrite
    /// mode, wrong for the usual insert-mode line editor.
    predict_overwrite: bool,

    overlays: Vec<OverlayRow>,
    cursors: Vec<CursorMove>,

    /// The newest state we have sent, the newest the server has
    /// acknowledged, and the newest it says it has ECHOED. Only the
    /// last one can judge a prediction.
    local_frame_sent: u64,
    local_frame_acked: u64,
    local_frame_late_acked: u64,

    /// The epoch new predictions are made in, and the newest epoch some
    /// prediction has proven correct. Predictions from an unconfirmed
    /// epoch are made but not drawn.
    prediction_epoch: u64,
    confirmed_epoch: u64,

    flagging: bool,
    srtt_trigger: bool,
    glitch_trigger: u32,
    last_quick_confirmation: u64,
    send_interval: u64,
    last_height: u16,
    last_width: u16,

    /// Taken out of the struct while parsing, so the performer can hold
    /// the rest of the engine mutably.
    parser: Option<vte::Parser>,
    last_byte: u8,
    /// Whether the parser is part-way through an escape sequence, which
    /// is the one thing vte does not tell us and the DEL interception
    /// below needs to know.
    in_escape: bool,
}

impl Default for PredictionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PredictionEngine {
    /// An engine that has predicted nothing yet.
    pub fn new() -> Self {
        Self {
            display_preference: DisplayPreference::default(),
            predict_overwrite: false,
            overlays: Vec::new(),
            cursors: Vec::new(),
            local_frame_sent: 0,
            local_frame_acked: 0,
            local_frame_late_acked: 0,
            // Epoch 1 is unconfirmed by construction, so the very first
            // prediction is made silently and only shown once it has
            // proven the echo works.
            prediction_epoch: 1,
            confirmed_epoch: 0,
            flagging: false,
            srtt_trigger: false,
            glitch_trigger: 0,
            last_quick_confirmation: 0,
            send_interval: 250,
            last_height: 0,
            last_width: 0,
            parser: Some(vte::Parser::new()),
            last_byte: 0,
            in_escape: false,
        }
    }

    /// Choose when guesses are drawn.
    pub fn set_display_preference(&mut self, pref: DisplayPreference) {
        self.display_preference = pref;
    }

    /// The current setting.
    pub fn display_preference(&self) -> DisplayPreference {
        self.display_preference
    }

    /// Predict overwriting rather than inserting.
    pub fn set_predict_overwrite(&mut self, overwrite: bool) {
        self.predict_overwrite = overwrite;
    }

    /// The newest state we have sent. A guess made now expires at
    /// this plus one, which is the state its keystroke will travel in.
    pub fn set_local_frame_sent(&mut self, num: u64) {
        self.local_frame_sent = num;
    }

    /// The newest state the server has acknowledged RECEIVING, which
    /// is deliberately not what judges a guess. See
    /// [`Self::set_local_frame_late_acked`].
    pub fn set_local_frame_acked(&mut self, num: u64) {
        self.local_frame_acked = num;
    }

    /// The server's echo acknowledgement: the newest state of ours
    /// whose keystrokes it has run through its terminal. This is the
    /// clock every prediction is judged against.
    pub fn set_local_frame_late_acked(&mut self, num: u64) {
        self.local_frame_late_acked = num;
    }

    /// The transport's frame interval, which doubles as the engine's
    /// estimate of how slow the link is.
    pub fn set_send_interval(&mut self, interval: u64) {
        self.send_interval = interval;
    }

    /// Whether any prediction is outstanding.
    pub fn active(&self) -> bool {
        !self.cursors.is_empty()
            || self
                .overlays
                .iter()
                .any(|row| row.cells.iter().any(|c| c.base.active))
    }

    /// How long the caller may sleep before the display needs another
    /// look. A pending prediction has to be re-examined on a timer, not
    /// just when the network says something, because the glitch
    /// triggers fire on ELAPSED TIME: nothing arriving is exactly the
    /// case they exist to notice.
    pub fn wait_time_ms(&self) -> Option<u64> {
        let timing_tests_necessary = !(self.glitch_trigger > 0 && self.flagging);
        (timing_tests_necessary && self.active()).then_some(50)
    }

    /// Throw away every prediction. What happens when one turns out
    /// wrong: the guesses that followed it were all built on it.
    pub fn reset(&mut self) {
        self.cursors.clear();
        self.overlays.clear();
        self.become_tentative();
    }

    /// Stop DRAWING new predictions until one is confirmed correct.
    /// Predictions keep being made; they just wait for evidence that
    /// the situation is still one we understand.
    fn become_tentative(&mut self) {
        if self.display_preference != DisplayPreference::Experimental {
            self.prediction_epoch += 1;
        }
    }

    fn cursor(&self) -> CursorMove {
        *self.cursors.last().expect("init_cursor runs first")
    }

    fn cursor_mut(&mut self) -> &mut CursorMove {
        self.cursors.last_mut().expect("init_cursor runs first")
    }

    fn init_cursor<S: Screen>(&mut self, screen: &S) {
        let expiration = self.local_frame_sent + 1;
        if self.cursors.is_empty() {
            let (row, col) = screen.cursor();
            let mut cursor = CursorMove::new(expiration, row, col, self.prediction_epoch);
            cursor.base.active = true;
            self.cursors.push(cursor);
        } else if self.cursor().base.tentative_until_epoch != self.prediction_epoch {
            // The epoch moved on: carry the position forward into a new
            // prediction rather than editing one the old epoch owns.
            let (row, col) = (self.cursor().row, self.cursor().col);
            let mut cursor = CursorMove::new(expiration, row, col, self.prediction_epoch);
            cursor.base.active = true;
            self.cursors.push(cursor);
        }
    }

    /// The index of the row's overlay, allocating every column of it on
    /// first touch so an insert can shift cells across without
    /// allocating.
    fn get_or_make_row(&mut self, row_num: u16, num_cols: u16) -> usize {
        if let Some(idx) = self.overlays.iter().position(|r| r.row_num == row_num) {
            return idx;
        }
        let cells = (0..num_cols)
            .map(|col| CellPrediction::new(0, col, self.prediction_epoch))
            .collect();
        self.overlays.push(OverlayRow { row_num, cells });
        self.overlays.len() - 1
    }

    /// Retire everything an epoch owns, because one of its predictions
    /// was wrong while still tentative. The rest of the screen's
    /// predictions survive: this is the narrower answer, used when the
    /// bad guess had not been drawn yet.
    fn kill_epoch<S: Screen>(&mut self, epoch: u64, screen: &S) {
        let cutoff = epoch.saturating_sub(1);
        self.cursors.retain(|c| !c.base.tentative(cutoff));

        let (row, col) = screen.cursor();
        let mut cursor =
            CursorMove::new(self.local_frame_sent + 1, row, col, self.prediction_epoch);
        cursor.base.active = true;
        self.cursors.push(cursor);

        for overlay in &mut self.overlays {
            for cell in &mut overlay.cells {
                if cell.base.tentative(cutoff) {
                    cell.reset();
                }
            }
        }

        self.become_tentative();
    }

    // ------------------------------------------------------------ //
    // user input
    // ------------------------------------------------------------ //

    /// Feed a run of user input. Predict from it BEFORE it is handed to
    /// the transport: a prediction expires at `local_frame_sent + 1`,
    /// which is the number the state carrying these bytes will get.
    pub fn new_user_bytes<S: Screen>(&mut self, bytes: &[u8], screen: &S, now: u64) {
        for &byte in bytes {
            self.new_user_byte(byte, screen, now);
        }
    }

    /// Feed one byte of user input.
    pub fn new_user_byte<S: Screen>(&mut self, byte: u8, screen: &S, now: u64) {
        if self.display_preference == DisplayPreference::Never {
            return;
        }
        if self.display_preference == DisplayPreference::Experimental {
            self.prediction_epoch = self.confirmed_epoch;
        }

        self.cull(screen, now);

        // Application-mode cursor keys arrive as `ESC O A`..`D` where
        // the ordinary form is `ESC [ A`..`D`. Rewriting the byte lets
        // one branch below handle both, which is what mosh does.
        let mut byte = byte;
        if self.last_byte == 0x1b && byte == b'O' {
            byte = b'[';
        }
        self.last_byte = byte;

        // vte follows the VT500 tables, where DEL is ignored in every
        // state, so it never reaches `print`. mosh's own parser prints
        // it, and that is what makes Backspace predictable at all,
        // since the Backspace key sends DEL. Intercept it in the ground
        // state only, exactly where the tables would have printed it.
        if byte == 0x7f && !self.in_escape {
            self.print_char('\u{7f}', screen, now);
            return;
        }
        if byte == 0x1b {
            self.in_escape = true;
        }

        let mut parser = self.parser.take().unwrap_or_default();
        let mut performer = Predictor {
            engine: self,
            screen,
            now,
        };
        parser.advance(&mut performer, &[byte]);
        self.parser = Some(parser);
    }

    fn print_char<S: Screen>(&mut self, ch: char, screen: &S, now: u64) {
        self.init_cursor(screen);

        if ch == '\u{7f}' {
            self.backspace(screen, now);
        } else if (ch as u32) < 0x20 || ch.width() != Some(1) {
            // A control character we do not model, or one whose width
            // on screen we cannot be sure of. Guessing about a wide
            // character means guessing about two cells and where the
            // next one lands, which mosh declines to do.
            self.become_tentative();
        } else {
            self.insert_char(ch, screen, now);
        }
    }

    /// Predict what Backspace does: everything from the cursor leftward
    /// shifts one column left, and the far right becomes unknowable.
    fn backspace<S: Screen>(&mut self, screen: &S, now: u64) {
        let (height, width) = (screen.rows(), screen.cols());
        let row = self.cursor().row;
        if row >= height || width == 0 {
            self.become_tentative();
            return;
        }
        let ridx = self.get_or_make_row(row, width);

        if self.cursor().col == 0 {
            return;
        }
        let expiration = self.local_frame_sent + 1;
        self.cursor_mut().col -= 1;
        self.cursor_mut().base.expire(expiration, now);
        let col = self.cursor().col;

        if self.predict_overwrite {
            // Overwrite mode erases in place instead of pulling the
            // rest of the line back.
            let original = screen.cell(row, col);
            let cell = &mut self.overlays[ridx].cells[usize::from(col)];
            cell.reset_with_orig();
            cell.base.active = true;
            cell.base.tentative_until_epoch = self.prediction_epoch;
            cell.base.expire(expiration, now);
            cell.original_contents.push(original.clone());
            cell.replacement = Cell {
                contents: " ".into(),
                rendition: original.rendition,
            };
            return;
        }

        for i in col..width {
            // The cell to the right has not been rewritten yet this
            // pass (we walk rightward), so what it holds now is what
            // will shift into this one.
            let next = usize::from(i) + 1;
            let source = if u32::from(i) + 2 < u32::from(width) {
                let neighbour = &self.overlays[ridx].cells[next];
                if neighbour.base.active {
                    if neighbour.unknown {
                        None
                    } else {
                        Some(neighbour.replacement.clone())
                    }
                } else {
                    Some(screen.cell(row, i + 1))
                }
            } else {
                // Nothing is known about what lies beyond the right
                // edge, so the last two columns become unknown rather
                // than wrong.
                None
            };
            let original = screen.cell(row, i);

            let epoch = self.prediction_epoch;
            let cell = &mut self.overlays[ridx].cells[usize::from(i)];
            cell.reset_with_orig();
            cell.base.active = true;
            cell.base.tentative_until_epoch = epoch;
            cell.base.expire(expiration, now);
            cell.original_contents.push(original);
            match source {
                Some(replacement) => {
                    cell.unknown = false;
                    cell.replacement = replacement;
                }
                None => cell.unknown = true,
            }
        }
    }

    /// Predict what typing a character does: it appears at the cursor,
    /// everything to its right shifts one column over, and the cursor
    /// advances.
    fn insert_char<S: Screen>(&mut self, ch: char, screen: &S, now: u64) {
        let (height, width) = (screen.rows(), screen.cols());
        let (row, col) = (self.cursor().row, self.cursor().col);
        if row >= height || col >= width {
            // The screen moved under a prediction we were about to
            // build on. mosh asserts here; refusing to predict is the
            // same answer without taking the process down.
            self.become_tentative();
            return;
        }
        let ridx = self.get_or_make_row(row, width);

        if col + 1 >= width {
            // The last column is genuinely ambiguous: an editor draws a
            // wrap marker there, a shell just puts the character. Keep
            // predicting, stop showing it.
            self.become_tentative();
        }

        let expiration = self.local_frame_sent + 1;
        let epoch = self.prediction_epoch;

        // Shift the rest of the line right. Walking leftward means the
        // source cell has not been rewritten yet when it is read.
        let rightmost = if self.predict_overwrite {
            col
        } else {
            width - 1
        };
        for i in ((col + 1)..=rightmost).rev() {
            let prev = usize::from(i) - 1;
            let source = if i == width - 1 {
                // What gets pushed off the right edge is unknowable, so
                // the cell that receives it is too.
                None
            } else {
                let neighbour = &self.overlays[ridx].cells[prev];
                if neighbour.base.active {
                    if neighbour.unknown {
                        None
                    } else {
                        Some(neighbour.replacement.clone())
                    }
                } else {
                    Some(screen.cell(row, i - 1))
                }
            };
            let original = screen.cell(row, i);

            let cell = &mut self.overlays[ridx].cells[usize::from(i)];
            cell.reset_with_orig();
            cell.base.active = true;
            cell.base.tentative_until_epoch = epoch;
            cell.base.expire(expiration, now);
            cell.original_contents.push(original);
            match source {
                Some(replacement) => {
                    cell.unknown = false;
                    cell.replacement = replacement;
                }
                None => cell.unknown = true,
            }
        }

        // The character itself inherits the look of what is to its
        // left, which is how a prediction typed into coloured output
        // comes out the right colour.
        //
        // (mosh reads the terminal's live pen for the column-zero case;
        // vt100 does not expose one, so the cell already standing there
        // stands in for it. Same answer whenever the line has any
        // rendition of its own, which is the case that shows.)
        let rendition = if col > 0 {
            let neighbour = &self.overlays[ridx].cells[usize::from(col) - 1];
            if neighbour.base.active && !neighbour.unknown {
                neighbour.replacement.rendition
            } else {
                screen.cell(row, col - 1).rendition
            }
        } else {
            screen.cell(row, col).rendition
        };
        let original = screen.cell(row, col);

        let cell = &mut self.overlays[ridx].cells[usize::from(col)];
        cell.reset_with_orig();
        cell.base.active = true;
        cell.base.tentative_until_epoch = epoch;
        cell.base.expire(expiration, now);
        cell.replacement = Cell {
            contents: ch.to_string(),
            rendition,
        };
        cell.original_contents.push(original);

        self.cursor_mut().base.expire(expiration, now);

        if col < width - 1 {
            self.cursor_mut().col += 1;
        } else {
            self.become_tentative();
            self.newline_carriage_return(screen, now);
        }
    }

    /// Predict a return to the start of the next line. On the last row
    /// that would be a scroll, which mosh refuses to predict; it
    /// predicts the row goes blank instead, which is the part it can be
    /// sure of.
    fn newline_carriage_return<S: Screen>(&mut self, screen: &S, now: u64) {
        let (height, width) = (screen.rows(), screen.cols());
        self.init_cursor(screen);
        self.cursor_mut().col = 0;

        if height == 0 || self.cursor().row + 1 < height {
            self.cursor_mut().row += 1;
            return;
        }

        let expiration = self.local_frame_sent + 1;
        let epoch = self.prediction_epoch;
        let row = self.cursor().row;
        let ridx = self.get_or_make_row(row, width);
        for cell in &mut self.overlays[ridx].cells {
            cell.base.active = true;
            cell.base.tentative_until_epoch = epoch;
            cell.base.expire(expiration, now);
            // Blank the contents, keep the renditions: mosh clears the
            // cell rather than replacing it.
            cell.replacement.contents.clear();
        }
    }

    // ------------------------------------------------------------ //
    // judging what was predicted
    // ------------------------------------------------------------ //

    /// Check every outstanding prediction against the screen the server
    /// actually sent, retire the ones that have been answered, and
    /// adjust the triggers that decide whether predictions are shown.
    ///
    /// `screen` must be the CONFIRMED screen. Handing it the screen
    /// with the overlay already on it would let every prediction
    /// validate itself, and the display would drift with nothing ever
    /// reporting an error.
    pub fn cull<S: Screen>(&mut self, screen: &S, now: u64) {
        if self.display_preference == DisplayPreference::Never {
            return;
        }

        let (height, width) = (screen.rows(), screen.cols());
        if self.last_height != height || self.last_width != width {
            self.last_height = height;
            self.last_width = width;
            self.reset();
        }

        // Show predictions once the link is slow enough to want them,
        // and stop only once it is fast AND nothing is on screen, so
        // the change never happens under the user's eyes.
        if self.send_interval > SRTT_TRIGGER_HIGH {
            self.srtt_trigger = true;
        } else if self.srtt_trigger && self.send_interval <= SRTT_TRIGGER_LOW && !self.active() {
            self.srtt_trigger = false;
        }

        if self.send_interval > FLAG_TRIGGER_HIGH {
            self.flagging = true;
        } else if self.send_interval <= FLAG_TRIGGER_LOW {
            self.flagging = false;
        }

        // A link that keeps stalling gets underlining whatever the
        // round-trip estimate claims.
        if self.glitch_trigger > GLITCH_REPAIR_COUNT {
            self.flagging = true;
        }

        let experimental = self.display_preference == DisplayPreference::Experimental;
        let (early, late) = (self.local_frame_acked, self.local_frame_late_acked);

        let mut ri = 0;
        while ri < self.overlays.len() {
            let row_num = self.overlays[ri].row_num;
            if row_num >= height {
                self.overlays.remove(ri);
                continue;
            }

            let mut ci = 0;
            while ci < self.overlays[ri].cells.len() {
                let validity = {
                    let cell = &self.overlays[ri].cells[ci];
                    cell.validity(screen, row_num, early, late)
                };
                match validity {
                    Validity::IncorrectOrExpired => {
                        let cell = &self.overlays[ri].cells[ci];
                        let was_tentative = cell.base.tentative(self.confirmed_epoch);
                        let epoch = cell.base.tentative_until_epoch;
                        if experimental {
                            self.overlays[ri].cells[ci].reset();
                        } else if was_tentative {
                            // Wrong, but never shown: only its own
                            // epoch has to go.
                            self.kill_epoch(epoch, screen);
                        } else {
                            // Wrong AND on screen. Everything still
                            // outstanding was guessed on top of it.
                            self.reset();
                            return;
                        }
                    }
                    Validity::Correct => {
                        let (epoch, outstanding, col) = {
                            let cell = &self.overlays[ri].cells[ci];
                            (
                                cell.base.tentative_until_epoch,
                                cell.base.outstanding(now),
                                cell.col,
                            )
                        };
                        if epoch > self.confirmed_epoch {
                            self.confirmed_epoch = epoch;
                        }

                        // Predictions coming back fast slowly work off
                        // a glitch, but only one per interval, so a
                        // single quick burst cannot cure a bad link.
                        if outstanding < GLITCH_THRESHOLD
                            && self.glitch_trigger > 0
                            && now.saturating_sub(GLITCH_REPAIR_MININTERVAL)
                                >= self.last_quick_confirmation
                        {
                            self.glitch_trigger -= 1;
                            self.last_quick_confirmation = now;
                        }

                        // We now know how the server actually coloured
                        // this cell, so the rest of the line's guesses
                        // adopt it instead of the one they inherited.
                        let actual = screen.cell(row_num, col).rendition;
                        for later in &mut self.overlays[ri].cells[ci..] {
                            later.replacement.rendition = actual;
                        }

                        self.overlays[ri].cells[ci].reset();
                    }
                    Validity::CorrectNoCredit => self.overlays[ri].cells[ci].reset(),
                    Validity::Pending => {
                        // Nothing arriving is exactly the case the
                        // glitch triggers exist to notice.
                        let outstanding = self.overlays[ri].cells[ci].base.outstanding(now);
                        if outstanding >= GLITCH_FLAG_THRESHOLD {
                            self.glitch_trigger = GLITCH_REPAIR_COUNT * 2;
                        } else if outstanding >= GLITCH_THRESHOLD
                            && self.glitch_trigger < GLITCH_REPAIR_COUNT
                        {
                            self.glitch_trigger = GLITCH_REPAIR_COUNT;
                        }
                    }
                    Validity::Inactive => {}
                }
                ci += 1;
            }

            ri += 1;
        }

        // A cursor prediction that turned out wrong invalidates the
        // whole picture the same way a cell does.
        if let Some(cursor) = self.cursors.last()
            && cursor.validity(screen, early, late) == Validity::IncorrectOrExpired
        {
            if experimental {
                self.cursors.clear();
            } else {
                self.reset();
                return;
            }
        }
        self.cursors
            .retain(|c| c.validity(screen, early, late) == Validity::Pending);
    }

    // ------------------------------------------------------------ //
    // drawing
    // ------------------------------------------------------------ //

    /// Whether predictions are being drawn at all right now.
    pub fn showing(&self) -> bool {
        match self.display_preference {
            DisplayPreference::Never => false,
            DisplayPreference::Always | DisplayPreference::Experimental => true,
            DisplayPreference::Adaptive => self.srtt_trigger || self.glitch_trigger > 0,
        }
    }

    /// Whether a guess made RIGHT NOW would be drawn, rather than made
    /// and held back until it proves itself.
    ///
    /// False at the start of a session, and after anything the engine
    /// does not model, until some guess has come true. It stays false
    /// for longer than a round trip, and that is the server's doing:
    /// `Complete::set_echo_ack` only reports a keystroke echoed once it
    /// is `ECHO_TIMEOUT` (50 ms) old, so the very first character of a
    /// session cannot appear early however fast the link is.
    pub fn drawing_now(&self) -> bool {
        self.showing() && self.prediction_epoch <= self.confirmed_epoch
    }

    /// Every prediction currently worth showing, and where the cursor
    /// should appear, for the screen to paint however it paints.
    ///
    /// The engine decides WHICH cells and WITH WHAT; it does not decide
    /// how they get there. A screen that owns its grid writes them
    /// directly, which is what mosh does; one that can only be fed
    /// bytes renders them as escapes. Empty when there is nothing to
    /// show, which is the common case and the one that must cost
    /// nothing.
    pub fn overlay<S: Screen>(&self, screen: &S) -> (Vec<OverlayCell>, Option<(u16, u16)>) {
        if !self.showing() {
            return (Vec::new(), None);
        }
        let (height, width) = (screen.rows(), screen.cols());
        if height == 0 || width == 0 {
            return (Vec::new(), None);
        }

        let mut out = Vec::new();
        for row in &self.overlays {
            if row.row_num >= height {
                continue;
            }
            for cell in &row.cells {
                if !cell.base.active
                    || cell.col >= width
                    || cell.base.tentative(self.confirmed_epoch)
                {
                    continue;
                }
                let actual = screen.cell(row.row_num, cell.col);

                // Underlining a blank tells the user nothing and looks
                // like a stray mark.
                let flag = self.flagging && !(cell.replacement.is_blank() && actual.is_blank());

                if cell.unknown {
                    // Nothing to draw, but the user can still be told
                    // this cell is not to be trusted. Never in the last
                    // column, where an underline would be mistaken for
                    // a wrap marker.
                    if flag && cell.col + 1 < width {
                        out.push(OverlayCell {
                            row: row.row_num,
                            col: cell.col,
                            cell: actual,
                            underline: true,
                        });
                    }
                    continue;
                }

                if actual != cell.replacement {
                    out.push(OverlayCell {
                        row: row.row_num,
                        col: cell.col,
                        cell: cell.replacement.clone(),
                        underline: flag,
                    });
                }
            }
        }

        // The newest applicable cursor prediction wins, which is the
        // same as mosh applying them in order.
        let cursor = self
            .cursors
            .iter()
            .rev()
            .find(|c| c.base.active && !c.base.tentative(self.confirmed_epoch))
            .filter(|c| c.row < height && c.col < width)
            .map(|c| (c.row, c.col));

        (out, cursor)
    }
}

/// The bridge from vte's parser to the engine. mosh classifies user
/// input with its own terminal parser for exactly these four cases, and
/// vte's `Perform` callbacks are the same four.
struct Predictor<'a, S: Screen> {
    engine: &'a mut PredictionEngine,
    screen: &'a S,
    now: u64,
}

impl<S: Screen> Perform for Predictor<'_, S> {
    fn print(&mut self, ch: char) {
        // Printing only ever happens in the ground state, so reaching
        // here proves any escape sequence has ended.
        self.engine.in_escape = false;
        self.engine.print_char(ch, self.screen, self.now);
    }

    fn execute(&mut self, byte: u8) {
        // Deliberately does NOT clear `in_escape`: a C0 control
        // executes without ending the sequence it appears inside.
        if byte == 0x0d {
            self.engine.become_tentative();
            self.engine.newline_carriage_return(self.screen, self.now);
        } else {
            self.engine.become_tentative();
        }
    }

    fn csi_dispatch(
        &mut self,
        _params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        self.engine.in_escape = false;
        match action {
            'C' => {
                // Right arrow.
                self.engine.init_cursor(self.screen);
                let width = self.screen.cols();
                if self.engine.cursor().col + 1 < width {
                    let expiration = self.engine.local_frame_sent + 1;
                    self.engine.cursor_mut().col += 1;
                    self.engine.cursor_mut().base.expire(expiration, self.now);
                }
            }
            'D' => {
                // Left arrow.
                self.engine.init_cursor(self.screen);
                if self.engine.cursor().col > 0 {
                    let expiration = self.engine.local_frame_sent + 1;
                    self.engine.cursor_mut().col -= 1;
                    self.engine.cursor_mut().base.expire(expiration, self.now);
                }
            }
            _ => self.engine.become_tentative(),
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {
        self.engine.in_escape = false;
        self.engine.become_tentative();
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        self.engine.in_escape = false;
        self.engine.become_tentative();
    }

    fn unhook(&mut self) {
        self.engine.in_escape = false;
        self.engine.become_tentative();
    }
}

#[cfg(test)]
mod tests;
