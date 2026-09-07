//! What a prediction IS: one cell, one cursor position, and the rule
//! for deciding afterwards whether it was right.
//!
//! A prediction carries the contents and renditions it wants; HOW that
//! reaches the display is the screen's business, not the engine's. What
//! lives here is when a prediction is made, when it may be shown, and
//! how it is judged afterwards, which is where all the subtlety is.

use crate::screen::{Cell, Screen};

/// No frame, no epoch: mosh stores `uint64_t(-1)` in both, so a reset
/// prediction is tentative until an epoch that never arrives.
pub(super) const NEVER: u64 = u64::MAX;

/// What became of a prediction once the server's answer arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Validity {
    /// The server has not echoed the state that carried it yet.
    Pending,
    /// Right, and it proves the link echoes what we predict: this is
    /// what confirms an epoch and lets later predictions show at once.
    Correct,
    /// Right, but proves nothing. A blank prediction on a blank cell,
    /// or one that matches what was already there, would have "come
    /// true" whatever the server did.
    CorrectNoCredit,
    /// Wrong, or the cell moved out from under it.
    IncorrectOrExpired,
    /// Not a prediction at all.
    Inactive,
}

/// The fields every prediction shares: when it can be judged, and
/// whether it may be shown before then.
#[derive(Debug, Clone, Copy)]
pub(super) struct Conditional {
    /// The state number that must be echoed before this can be judged.
    pub(super) expiration_frame: u64,
    /// Whether this is a prediction at all.
    pub(super) active: bool,
    /// The epoch this belongs to. Until an epoch is confirmed correct,
    /// its predictions are made but not drawn.
    pub(super) tentative_until_epoch: u64,
    /// When it was made, which is how a long-pending prediction is
    /// noticed and turned into a reason to start showing predictions.
    pub(super) prediction_time: u64,
}

impl Default for Conditional {
    fn default() -> Self {
        Self {
            expiration_frame: NEVER,
            active: false,
            tentative_until_epoch: NEVER,
            prediction_time: NEVER,
        }
    }
}

impl Conditional {
    fn new(expiration_frame: u64, tentative_until_epoch: u64) -> Self {
        Self {
            expiration_frame,
            active: false,
            tentative_until_epoch,
            prediction_time: NEVER,
        }
    }

    pub(super) fn tentative(&self, confirmed_epoch: u64) -> bool {
        self.tentative_until_epoch > confirmed_epoch
    }

    pub(super) fn expire(&mut self, expiration_frame: u64, now: u64) {
        self.expiration_frame = expiration_frame;
        self.prediction_time = now;
    }

    fn reset(&mut self) {
        self.expiration_frame = NEVER;
        self.tentative_until_epoch = NEVER;
        self.active = false;
    }

    /// How long this has been waiting for the server. A prediction that
    /// was never expired has not been made, so it counts as forever
    /// rather than as instantaneous.
    pub(super) fn outstanding(&self, now: u64) -> u64 {
        if self.prediction_time == NEVER {
            u64::MAX
        } else {
            now.saturating_sub(self.prediction_time)
        }
    }
}

/// A predicted cell at one column of one row.
#[derive(Debug, Clone)]
pub(super) struct CellPrediction {
    pub(super) base: Conditional,
    pub(super) col: u16,
    pub(super) replacement: Cell,
    /// We predict this cell CHANGED but not into what: a character
    /// shifted in from beyond the right edge, where nothing is known.
    /// An unknown cell is never drawn, only underlined when flagging.
    pub(super) unknown: bool,
    /// What stood here before. A prediction that merely reproduces one
    /// of these earns no credit, because it would have looked right
    /// even if the server had ignored us.
    pub(super) original_contents: Vec<Cell>,
}

impl CellPrediction {
    pub(super) fn new(expiration_frame: u64, col: u16, tentative_until_epoch: u64) -> Self {
        Self {
            base: Conditional::new(expiration_frame, tentative_until_epoch),
            col,
            replacement: Cell::default(),
            unknown: false,
            original_contents: Vec::new(),
        }
    }

    pub(super) fn reset(&mut self) {
        self.unknown = false;
        self.original_contents.clear();
        self.base.reset();
    }

    /// Retire a prediction while remembering what it claimed, so a
    /// later prediction that reproduces it gets no credit for it. An
    /// inactive or unknown cell has nothing worth remembering.
    pub(super) fn reset_with_orig(&mut self) {
        if !self.base.active || self.unknown {
            self.reset();
            return;
        }
        self.original_contents.push(self.replacement.clone());
        self.base.reset();
    }

    /// Judge the prediction against the screen the server actually
    /// sent.
    ///
    /// `_early_ack` is the ordinary acknowledgement, and it is
    /// deliberately unused: mosh passes it and ignores it too. The
    /// question is not whether the server RECEIVED the keystroke but
    /// whether it has ECHOED it, which only `late_ack` (the echo
    /// acknowledgement) answers. Judging on the ordinary ack would
    /// declare every prediction wrong in the window between the server
    /// reading a keystroke and the shell printing it.
    pub(super) fn validity<S: Screen>(
        &self,
        screen: &S,
        row: u16,
        _early_ack: u64,
        late_ack: u64,
    ) -> Validity {
        if !self.base.active {
            return Validity::Inactive;
        }
        if row >= screen.rows() || self.col >= screen.cols() {
            return Validity::IncorrectOrExpired;
        }
        if late_ack < self.base.expiration_frame {
            return Validity::Pending;
        }
        if self.unknown {
            return Validity::CorrectNoCredit;
        }
        if self.replacement.is_blank() {
            // Far too easy to be right about by accident.
            return Validity::CorrectNoCredit;
        }
        let current = screen.cell(row, self.col);
        if current.contents_match(&self.replacement) {
            if self
                .original_contents
                .iter()
                .any(|orig| orig.contents_match(&self.replacement))
            {
                Validity::CorrectNoCredit
            } else {
                Validity::Correct
            }
        } else {
            Validity::IncorrectOrExpired
        }
    }
}

/// Every column of one row. mosh allocates the whole row at once so an
/// insert can shift cells across it without allocating mid-keystroke.
#[derive(Debug, Clone)]
pub(super) struct OverlayRow {
    pub(super) row_num: u16,
    pub(super) cells: Vec<CellPrediction>,
}

/// A predicted cursor position.
#[derive(Debug, Clone, Copy)]
pub(super) struct CursorMove {
    pub(super) base: Conditional,
    pub(super) row: u16,
    pub(super) col: u16,
}

impl CursorMove {
    pub(super) fn new(
        expiration_frame: u64,
        row: u16,
        col: u16,
        tentative_until_epoch: u64,
    ) -> Self {
        Self {
            base: Conditional::new(expiration_frame, tentative_until_epoch),
            row,
            col,
        }
    }

    pub(super) fn validity<S: Screen>(
        &self,
        screen: &S,
        _early_ack: u64,
        late_ack: u64,
    ) -> Validity {
        if !self.base.active {
            return Validity::Inactive;
        }
        if self.row >= screen.rows() || self.col >= screen.cols() {
            return Validity::IncorrectOrExpired;
        }
        if late_ack >= self.base.expiration_frame {
            if screen.cursor() == (self.row, self.col) {
                Validity::Correct
            } else {
                Validity::IncorrectOrExpired
            }
        } else {
            Validity::Pending
        }
    }
}

#[cfg(all(test, feature = "vt100-screen"))]
mod tests {
    use super::*;
    use crate::screen::Vt100Screen;

    fn screen_of(bytes: &[u8], rows: u16, cols: u16) -> Vt100Screen {
        let mut screen = Vt100Screen::new(rows, cols);
        screen.feed(bytes);
        screen
    }

    fn predicted(contents: &str) -> Cell {
        Cell {
            contents: contents.into(),
            rendition: Default::default(),
        }
    }

    #[test]
    fn a_prediction_is_pending_until_the_server_echoes_that_state() {
        let screen = screen_of(b"a", 3, 10);
        let mut cell = CellPrediction::new(0, 0, 1);
        cell.base.active = true;
        cell.base.expiration_frame = 5;
        cell.replacement = predicted("a");
        // Right on the screen, but the server has only echoed state 4.
        assert_eq!(cell.validity(&screen, 0, 4, 4), Validity::Pending);
        assert_eq!(cell.validity(&screen, 0, 5, 5), Validity::Correct);
    }

    #[test]
    fn matching_what_was_already_there_earns_no_credit() {
        let screen = screen_of(b"a", 3, 10);
        let mut cell = CellPrediction::new(0, 0, 1);
        cell.base.active = true;
        cell.base.expiration_frame = 1;
        cell.replacement = predicted("a");
        cell.original_contents.push(predicted("a"));
        assert_eq!(cell.validity(&screen, 0, 1, 1), Validity::CorrectNoCredit);
    }

    #[test]
    fn a_blank_prediction_never_confirms_an_epoch() {
        let screen = screen_of(b"", 3, 10);
        let mut cell = CellPrediction::new(0, 0, 1);
        cell.base.active = true;
        cell.base.expiration_frame = 1;
        // Blank on blank: true, and worth nothing.
        assert_eq!(cell.validity(&screen, 0, 1, 1), Validity::CorrectNoCredit);
    }

    #[test]
    fn a_wrong_character_is_incorrect() {
        let screen = screen_of(b"b", 3, 10);
        let mut cell = CellPrediction::new(0, 0, 1);
        cell.base.active = true;
        cell.base.expiration_frame = 1;
        cell.replacement = predicted("a");
        assert_eq!(
            cell.validity(&screen, 0, 1, 1),
            Validity::IncorrectOrExpired
        );
    }

    #[test]
    fn a_prediction_off_the_screen_is_expired() {
        let screen = screen_of(b"", 3, 10);
        let mut cell = CellPrediction::new(0, 40, 1);
        cell.base.active = true;
        cell.base.expiration_frame = 1;
        cell.replacement = predicted("a");
        assert_eq!(
            cell.validity(&screen, 0, 1, 1),
            Validity::IncorrectOrExpired
        );
        assert_eq!(
            cell.validity(&screen, 9, 1, 1),
            Validity::IncorrectOrExpired
        );
    }

    #[test]
    fn retiring_a_prediction_remembers_what_it_claimed() {
        let mut cell = CellPrediction::new(0, 0, 1);
        cell.base.active = true;
        cell.replacement = predicted("x");
        cell.reset_with_orig();
        assert!(!cell.base.active);
        assert_eq!(cell.original_contents.len(), 1);
        assert_eq!(cell.original_contents[0].contents, "x");

        // An unknown one has nothing to remember, and says so by
        // clearing the history rather than adding to it.
        cell.base.active = true;
        cell.unknown = true;
        cell.reset_with_orig();
        assert!(cell.original_contents.is_empty());
        assert!(!cell.unknown);
    }

    #[test]
    fn a_reset_prediction_is_tentative_forever() {
        let mut cell = CellPrediction::new(0, 0, 1);
        cell.base.active = true;
        cell.reset();
        // No epoch will ever confirm it, so it can never be drawn even
        // if something reactivated it by mistake.
        assert!(cell.base.tentative(u64::MAX - 1));
    }

    #[test]
    fn an_unmade_prediction_has_been_outstanding_forever() {
        let mut cell = CellPrediction::new(0, 0, 1);
        assert_eq!(cell.base.outstanding(1_000), u64::MAX);
        cell.base.expire(1, 900);
        assert_eq!(cell.base.outstanding(1_000), 100);
    }

    #[test]
    fn the_cursor_is_correct_only_where_the_server_put_it() {
        let screen = screen_of(b"abc", 3, 10);
        assert_eq!(screen.cursor(), (0, 3));
        let mut cursor = CursorMove::new(1, 0, 3, 1);
        cursor.base.active = true;
        assert_eq!(cursor.validity(&screen, 1, 1), Validity::Correct);
        cursor.col = 4;
        assert_eq!(cursor.validity(&screen, 1, 1), Validity::IncorrectOrExpired);
        // And it is not judged at all until the state is echoed.
        cursor.base.expiration_frame = 9;
        assert_eq!(cursor.validity(&screen, 1, 1), Validity::Pending);
    }
}
