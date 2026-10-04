//! Invocation-time X11 selection ownership and replacement detection.
//!
//! A dedicated subscribed connection follows one read across TARGETS, fallback
//! and incremental transfer; changing owners invalidates that entire read.

use super::*;
use std::cell::Cell;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};

/// Watch ownership before capturing it. Any subsequent ownership notification
/// invalidates the entire conversion, including TARGETS fallback and INCR.
/// Same-window replacement is a change too; comparing owner IDs alone misses it.
pub(super) struct CapturedSelection {
    pub(super) context: XContext,
    owner: u32,
    pub(super) invalidated: Cell<bool>,
    pub(super) deadline: Instant,
}

impl CapturedSelection {
    pub(super) fn new(selection: Atom) -> Result<Self> {
        let deadline = Instant::now() + CLIPBOARD_READ_DEADLINE;
        let context = XContext::new()?;
        context
            .conn
            .xfixes_query_version(5, 0)
            .map_err(into_unknown)?
            .reply()
            .map_err(into_unknown)?;
        context
            .conn
            .xfixes_select_selection_input(
                context.win_id,
                selection,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )
            .map_err(into_unknown)?;
        let owner = context
            .conn
            .get_selection_owner(selection)
            .map_err(into_unknown)?
            .reply()
            .map_err(into_unknown)?
            .owner;
        Ok(Self {
            context,
            owner,
            invalidated: Cell::new(owner == NONE),
            deadline,
        })
    }

    pub(super) fn validate(&self, selection: Atom) -> Result<()> {
        // This round trip also puts all preceding ownership notifications in
        // this connection's event queue before we inspect it.
        let owner = self
            .context
            .conn
            .get_selection_owner(selection)
            .map_err(into_unknown)?
            .reply()
            .map_err(into_unknown)?
            .owner;
        if owner != self.owner {
            self.invalidated.set(true);
        }
        while let Some(event) = self.context.conn.poll_for_event().map_err(into_unknown)? {
            if matches!(event, Event::XfixesSelectionNotify(_)) {
                self.invalidated.set(true);
            }
        }
        if self.invalidated.get() {
            Err(Error::ContentNotAvailable)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/linux/x11/clipboard_capture.rs"]
mod captured_selection_tests;
