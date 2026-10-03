//! Keeps input in the order the user gave it while a paste waits on the
//! clipboard.
//!
//! An external clipboard read resolves later, on a foreground task, so a paste
//! asked for before a keystroke can be ready after it. A paste that cannot be
//! answered at once takes a [`PasteTicket`]; until that ticket is finished,
//! later input is held behind it rather than overtaking it. Input that arrives
//! when nothing is pending passes straight through, so the barrier costs
//! nothing for a paste the platform answered at once.
//!
//! [`PasteOrder`] is generic because the same order has to hold in two places:
//! the bytes [`Terminal`](crate::Terminal) writes to its pty, and the input
//! events a view rebroadcasts to other panes.

use std::collections::VecDeque;

/// One outstanding paste's place in a [`PasteOrder`]. Hand it back to
/// [`PasteOrder::resolve`] (or [`crate::Terminal::finish_paste`]) on every
/// outcome — an empty clipboard and a failed read included — or the input held
/// behind it is never released.
#[must_use = "input after the paste is held until the ticket is resolved"]
#[derive(Debug)]
pub struct PasteTicket(u64);

enum Held<T> {
    Paste(u64),
    Input(T),
}

pub struct PasteOrder<T> {
    next_ticket: u64,
    held: VecDeque<Held<T>>,
    /// Collects what a paste produces while it is being resolved, so it takes
    /// the paste's place in the queue instead of joining its back.
    capture: Option<Vec<T>>,
}

impl<T> Default for PasteOrder<T> {
    fn default() -> Self {
        Self {
            next_ticket: 0,
            held: VecDeque::new(),
            capture: None,
        }
    }
}

impl<T> PasteOrder<T> {
    /// Reserves a place for a paste whose contents are not known yet.
    pub fn begin(&mut self) -> PasteTicket {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.held.push_back(Held::Paste(ticket));
        PasteTicket(ticket)
    }

    /// Whether any paste is still waiting on its clipboard read.
    pub fn is_pending(&self) -> bool {
        !self.held.is_empty()
    }

    /// Returns `input` if it may be delivered now, or keeps it behind the
    /// pending pastes. While a paste is being resolved, its own input is
    /// collected for that paste instead.
    pub fn admit(&mut self, input: T) -> Option<T> {
        if let Some(capture) = &mut self.capture {
            capture.push(input);
            return None;
        }
        if self.held.is_empty() {
            return Some(input);
        }
        self.held.push_back(Held::Input(input));
        None
    }

    /// Starts collecting the input a paste produces; [`PasteOrder::resolve`]
    /// puts it in the paste's place.
    pub fn capture(&mut self) {
        debug_assert!(self.capture.is_none(), "pastes resolve one at a time");
        self.capture = Some(Vec::new());
    }

    /// Replaces `ticket` with what was admitted since [`PasteOrder::capture`]
    /// (nothing, if capture was not started), and returns everything that may
    /// now be delivered, in order. Input stays held while an earlier paste is
    /// still pending.
    pub fn resolve(&mut self, ticket: PasteTicket) -> Vec<T> {
        let produced = self.capture.take().unwrap_or_default();
        let Some(position) = self
            .held
            .iter()
            .position(|held| matches!(held, Held::Paste(pending) if *pending == ticket.0))
        else {
            debug_assert!(false, "paste ticket {} is not pending here", ticket.0);
            return produced;
        };
        self.held.remove(position);
        for (offset, input) in produced.into_iter().enumerate() {
            self.held.insert(position + offset, Held::Input(input));
        }
        let mut released = Vec::new();
        while let Some(Held::Input(_)) = self.held.front() {
            if let Some(Held::Input(input)) = self.held.pop_front() {
                released.push(input);
            }
        }
        released
    }
}

impl crate::Terminal {
    /// Reserves this pane's input order for a paste whose clipboard read has
    /// not resolved. Keyboard input queued before [`Self::finish_paste`] is
    /// written after the paste rather than before it. Replies to the program
    /// (terminal queries, mouse and focus reports) are not held: they answer
    /// the program, not the user.
    pub fn begin_paste(&mut self) -> PasteTicket {
        self.paste_order.begin()
    }

    /// Delivers a paste in the place [`Self::begin_paste`] reserved for it.
    /// `paste` runs now and may queue any input — [`Self::paste`],
    /// [`Self::paste_image`], or nothing for an empty clipboard; its input is
    /// written once every earlier paste has been delivered, followed by the
    /// input that was held behind it.
    pub fn finish_paste(&mut self, ticket: PasteTicket, paste: impl FnOnce(&mut Self)) {
        self.paste_order.capture();
        paste(self);
        for input in self.paste_order.resolve(ticket) {
            self.queue_input_now(input);
        }
    }

    /// Whether a paste is still holding this pane's input back.
    pub fn paste_pending(&self) -> bool {
        self.paste_order.is_pending()
    }
}

#[cfg(test)]
#[path = "tests/paste_order.rs"]
mod tests;
