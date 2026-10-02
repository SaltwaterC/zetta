//! Handing a terminal's grid from one reader to the next without blocking the
//! thread that owns the terminal.
//!
//! A pane changes readers when it is detached, shared, revoked into sharing or
//! granted back: a pty event loop gives way to a multiplexer relay, or the
//! other way round. The old reader has to *finish* before the new one starts —
//! its bytes are older than anything the new one will parse — and finishing is
//! a thread join or a drain that can take up to [`BYTE_STREAM_DRAIN_TIMEOUT`].
//!
//! [`Terminal::retire_pty_loop`] and [`Terminal::retire_byte_stream`] do only
//! the non-blocking half on the terminal's thread and return a
//! [`RetiredReader`] that owns everything left to wait for, so a background
//! worker can wait without borrowing the terminal. Until the next backend is
//! attached the terminal holds the input it is given rather than dropping it,
//! and [`GridSnapshotSource`] lets the same worker serialize the grid once the
//! reader has ended.
//!
//! Kept apart from `terminal.rs` because none of it has an upstream
//! counterpart.

use std::cell::RefCell;
use std::sync::Arc;

use anyhow::Result;

use crate::alacritty::{AlacrittyTermLock, PtyIo};
use crate::{BYTE_STREAM_DRAIN_TIMEOUT, ByteStreamHandle, Terminal, TerminalType, snapshot};

/// The most input a terminal holds between two readers. Typing during a
/// handover is a few keystrokes; this only bounds a pane whose handover never
/// completes and that nobody discards.
const HELD_INPUT_LIMIT: usize = 1 << 20;

/// A reader that no longer feeds the grid, and that may still be finishing.
///
/// Owned and `Send`, so whoever has to wait for it can do so off the thread
/// that owns the terminal. [`RetiredReader::finish`] is the barrier: once it
/// returns, nothing the retired reader read can still reach the grid.
///
/// Dropping one unfinished abandons a byte stream rather than draining it,
/// which is the only safe default: a stream left running would go on writing
/// into a grid that a new reader may already own.
#[must_use = "a retired reader may still be writing to the grid until it is finished"]
#[derive(Default)]
pub struct RetiredReader {
    pty_loop: Option<PtyIo>,
    byte_stream: Option<ByteStreamHandle>,
}

impl RetiredReader {
    /// Whether there is anything to wait for at all.
    pub fn is_empty(&self) -> bool {
        self.pty_loop.is_none() && self.byte_stream.is_none()
    }

    /// Both retirements, finished in the order they were made: the pty loop
    /// first, then the stream.
    pub fn and(mut self, mut other: RetiredReader) -> RetiredReader {
        debug_assert!(
            self.pty_loop.is_none() || other.pty_loop.is_none(),
            "a terminal has one pty loop to retire"
        );
        debug_assert!(
            self.byte_stream.is_none() || other.byte_stream.is_none(),
            "a terminal has one byte stream to retire"
        );
        self.pty_loop = self.pty_loop.take().or_else(|| other.pty_loop.take());
        self.byte_stream = self.byte_stream.take().or_else(|| other.byte_stream.take());
        self
    }

    /// Blocks until the retired reader has ended.
    ///
    /// A pty loop is joined. A byte stream is drained: its reader is given up
    /// to [`BYTE_STREAM_DRAIN_TIMEOUT`] to reach the end of what the
    /// multiplexer flushed, and is abandoned after that so a multiplexer that
    /// failed to close its end cannot hold the caller forever. The stream is
    /// drained even when the join fails, so neither reader outlives this call.
    pub fn finish(mut self) -> Result<()> {
        let joined = self.pty_loop.take().map_or(Ok(()), PtyIo::join);
        if let Some(mut stream) = self.byte_stream.take() {
            stream.finish_drain(BYTE_STREAM_DRAIN_TIMEOUT);
        }
        joined
    }
}

impl Drop for RetiredReader {
    fn drop(&mut self) {
        if let Some(stream) = &mut self.byte_stream {
            stream.stop();
        }
    }
}

/// A handle that serializes a terminal's grid from any thread.
///
/// Taken on the terminal's thread and used once its reader has finished, so a
/// handover's snapshot is a stable picture of what the next reader starts from
/// and is built without holding up the window that shows it.
#[derive(Clone)]
pub struct GridSnapshotSource {
    term: Arc<AlacrittyTermLock>,
}

impl GridSnapshotSource {
    /// The grid as ANSI, as [`Terminal::ansi_snapshot`] renders it.
    pub fn ansi_snapshot(&self, max_lines: usize) -> Vec<u8> {
        snapshot::ansi_snapshot(&self.term.lock_unfair(), max_lines)
    }
}

/// Input given to a terminal while it has no backend to write it to.
#[derive(Default)]
pub(crate) struct HeldInput {
    held: RefCell<Option<Vec<Vec<u8>>>>,
}

impl HeldInput {
    fn begin(&self) {
        self.held.borrow_mut().get_or_insert_with(Vec::new);
    }

    /// Keeps `bytes` for the next backend. Returns `false` when nothing is
    /// being held, in which case the caller's own fallback applies.
    pub(crate) fn hold(&self, bytes: &[u8]) -> bool {
        let mut held = self.held.borrow_mut();
        let Some(held) = held.as_mut() else {
            return false;
        };
        let size = held.iter().map(Vec::len).sum::<usize>();
        if size.saturating_add(bytes.len()) > HELD_INPUT_LIMIT {
            log::warn!("dropping terminal input typed during a reader handover that never ended");
        } else {
            held.push(bytes.to_vec());
        }
        true
    }

    fn take(&self) -> Vec<Vec<u8>> {
        self.held.borrow_mut().take().unwrap_or_default()
    }
}

impl Terminal {
    /// Stops this terminal's pty event loop from reading, without waiting for
    /// it to end.
    ///
    /// The non-blocking half of [`Terminal::stop_pty_loop`]. The grid stays
    /// intact, and input typed from here until the next backend is attached
    /// is held for it.
    pub fn retire_pty_loop(&mut self) -> RetiredReader {
        if let Some(mut input_worker) = self.input_worker.take() {
            input_worker.stop();
        }
        let TerminalType::Pty { pty_tx, io, info } = &mut self.terminal_type else {
            return RetiredReader::default();
        };
        self.replay_barrier.abort();
        if pty_tx.take().map(|pty_tx| pty_tx.shutdown()).is_some() {
            self.held_input.begin();
        }
        // Joining drops the loop's `EventLoop`, and with it the pty master this
        // borrows for foreground-process lookups.
        info.close_pty_handle();
        RetiredReader {
            pty_loop: io.take(),
            byte_stream: None,
        }
    }

    /// Stops this terminal's byte stream from accepting input and lets its
    /// reader run on to the end of what it was sent.
    ///
    /// The non-blocking half of the drain [`Terminal::attach_pty`] needs: the
    /// bytes the multiplexer flushed before closing its end are older than
    /// anything the pty will produce, so they still reach the grid, and
    /// [`RetiredReader::finish`] is what waits for them.
    pub fn retire_byte_stream(&mut self) -> RetiredReader {
        let Some(mut stream) = self.byte_stream.take() else {
            return RetiredReader::default();
        };
        stream.begin_drain();
        self.held_input.begin();
        RetiredReader {
            pty_loop: None,
            byte_stream: Some(stream),
        }
    }

    /// A handle that serializes this terminal's grid off its thread.
    pub fn grid_snapshot_source(&self) -> GridSnapshotSource {
        GridSnapshotSource {
            term: self.term.clone(),
        }
    }

    /// Drops the input held since the last retirement and stops holding more.
    ///
    /// For a handover that will not complete: the pane has no backend to send
    /// that input to, and holding it would only grow.
    pub fn discard_held_input(&mut self) {
        self.held_input.take();
    }

    /// Sends what was typed during a handover to the backend that has just
    /// been attached, in the order it was typed.
    pub(crate) fn flush_held_input(&mut self) {
        for bytes in self.held_input.take() {
            self.write_to_pty(bytes);
        }
    }
}

#[cfg(test)]
#[path = "tests/reader_handover.rs"]
mod tests;
