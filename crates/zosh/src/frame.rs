//! When a session loop may paint, and what it paints on a geometry change.
//!
//! Two loops drive a Mosh session in this crate: the standalone terminal
//! client in [`crate::client`], and the embedded [`crate::stream::PaneSession`]
//! an application drives without a tty. They differ in where their bytes come
//! from and go to. They must not differ in *when* a frame may be painted: a
//! screen that changed shape has to be cleared and repainted whole, and a
//! resize the server has not acknowledged yet must not be painted at all,
//! because a frame describing the old geometry would land on a terminal that
//! has already changed shape. That decision lives here so the two cannot drift
//! apart on it.

use std::io::{self, Write};

use mosh_rs::{HostEvent, MoshSession, Screen as _};

use crate::display::{self, DisplayScreen};

pub(crate) type ClientSession = MoshSession<DisplayScreen>;

/// What a loop should paint this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// The screen changed shape: clear and repaint it whole.
    /// `resolves_pending` marks the repaint the client's own resize was
    /// waiting for, as opposed to one the server started.
    Repaint { resolves_pending: bool },
    /// An ordinary incremental paint.
    Paint,
    /// A resize is in flight and this frame still describes the old geometry.
    Skip,
}

pub(crate) fn next_frame(
    session: &ClientSession,
    events: &[HostEvent],
    pending_resize: Option<(u16, u16)>,
) -> Frame {
    let server_size = (session.displayed().cols(), session.displayed().rows());
    let resolves_pending = pending_resize.is_some_and(|expected| expected == server_size);
    let server_reported_resize = events
        .iter()
        .any(|event| matches!(event, HostEvent::Resize { .. }));
    if resolves_pending || (server_reported_resize && pending_resize.is_none()) {
        Frame::Repaint { resolves_pending }
    } else if pending_resize.is_none() {
        Frame::Paint
    } else {
        Frame::Skip
    }
}

/// Clear the display and paint the complete protocol screen into its new
/// shape, carrying the input modes and any scrollback clear the server asked
/// for across the resize.
pub(crate) fn write_repaint(
    session: &mut ClientSession,
    output: &mut impl Write,
) -> io::Result<()> {
    let previous = session.displayed().clone();
    let repaint = session.repaint();
    let input_modes = session.displayed().input_modes_diff(&previous);
    let scrollback_clear = if session.displayed().clears_scrollback_since(&previous) {
        display::clear_scrollback_sequence()
    } else {
        &[]
    };
    // Before the clear, not after: these are the rows that scrolled past
    // between the last frame and this one, and the repaint that follows is
    // what would otherwise erase them unseen.
    if let Some(replay) = session.displayed().scrollback_replay(&previous) {
        output.write_all(&replay)?;
    }
    display::repaint_after_resize(output, &repaint, &input_modes, scrollback_clear)
}

#[cfg(test)]
#[path = "tests/frame.rs"]
mod tests;
