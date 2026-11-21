//! A pane of a shared session, and the reader its viewers receive output
//! through.

use super::*;
use zetta_sync::RecoverPoison as _;

/// A pane attached in shared mode.
///
/// No descriptor changes hands: the connection stays open and is the pane's
/// data plane — output and size events arrive on it, and input and size
/// reports go back over it. The writer is shared with the terminal's
/// byte-stream worker only at the stream level: the reader owns its cloned
/// connection, so an idle read never holds up input or resize reports.
pub struct SharedPane {
    pub(super) session_id: u64,
    pub(super) pane_id: u64,
    pub child_pid: u32,
    pub(super) writer: Arc<Mutex<Connection>>,
    /// The sizes the multiplexer arbitrated, recorded by the reader as
    /// [`Event::Size`] arrives. The holder of the pane applies the latest
    /// each time it is woken.
    pub(super) sizes: Arc<Mutex<Vec<(SessionRevision, u16, u16)>>>,
    /// The grid the daemon advertised when this stream attached. Unlike an
    /// [`Event::Size`], this is not a revisioned update and does not mean this
    /// viewer has measured or reported its own capacity. Its holder consumes
    /// it once, after the terminal has real layout bounds.
    pub(super) initial_viewport: Arc<Mutex<Option<(u16, u16)>>>,
    /// Signalled by the reader whenever it records a size, so the pane's holder
    /// can wait for one instead of asking on a timer. Bounded at one: a pending
    /// signal already means "there are sizes to take".
    pub(super) size_signal: (async_channel::Sender<()>, async_channel::Receiver<()>),
    /// Holds the byte stream immediately after a daemon size frame. The frame
    /// precedes output drawn for the new grid, so letting the reader consume
    /// that output before its terminal applies the size irreversibly corrupts
    /// a full-screen application's incremental redraw.
    pub(super) size_application_pending: Arc<AtomicBool>,
    /// The exact size frame holding the stream. A delayed wake-up for an
    /// older, non-blocking handoff size must not release a newer frame.
    pub(super) pending_size_frame: Arc<Mutex<Option<(SessionRevision, u16, u16)>>>,
    /// Replacements are handed to the reader after the current connection has
    /// reported a transport failure. The queue lock is held only while moving
    /// a ready connection between the pane and its reader; no network read is
    /// performed while it is held.
    pub(super) reader_handoffs: Arc<Mutex<VecDeque<SharedReaderHandoff>>>,
    /// Set by the reader when its connection broke and no replacement was
    /// waiting, cleared when one is installed. A window's recovery is usually
    /// told by the daemon and supplies the replacement itself; a holder without
    /// that subscription — `zmux relay-pane` — has only this to go on.
    pub(super) stream_lost: Arc<AtomicBool>,
    /// Signalled by the reader each time [`Self::stream_lost`] becomes set.
    ///
    /// The daemon's [`Event::SharedStreamFailed`] is not enough on its own:
    /// it comes from the serve loop of the daemon that owned the connection,
    /// and a daemon that replaced itself took every relay with it and has no
    /// serve loop left to report from. Bounded at one, like `size_signal`.
    pub(super) lost_signal: (async_channel::Sender<()>, async_channel::Receiver<()>),
    /// Output produced while the pane was detached or shared, to be replayed
    /// into a fresh terminal before it is shown.
    pub replay: Vec<u8>,
}

impl SharedPane {
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn pane_id(&self) -> u64 {
        self.pane_id
    }

    /// Sends this client's input, to be written to the pane by the
    /// multiplexer.
    pub fn send_input(&self, bytes: &[u8]) -> Result<()> {
        let mut connection = self.writer.lock().recover_poison();
        connection.send(&Request::Input {
            length: bytes.len(),
        })?;
        connection.write_all(bytes)
    }

    /// Reports this client's size, so the multiplexer can keep every shared
    /// client at the smallest of them.
    pub fn send_resize(&self, columns: u16, lines: u16) -> Result<()> {
        self.send_resize_for_revision(SessionRevision::INITIAL, columns, lines)
    }

    pub fn send_resize_for_revision(
        &self,
        revision: SessionRevision,
        columns: u16,
        lines: u16,
    ) -> Result<()> {
        let mut connection = self.writer.lock().recover_poison();
        connection.send(&Request::Resize {
            session_id: self.session_id,
            pane_id: self.pane_id,
            revision: Some(revision),
            columns,
            lines,
        })
    }

    /// The reader side of the shared connection, for a terminal's byte-stream
    /// worker: parses [`Event::Output`] into plain bytes, records
    /// [`Event::Size`], and returns an I/O error when the multiplexer goes
    /// away so the terminal can report the connection lost.
    ///
    /// The multiplexer sends output in bursts around long idle stretches, so
    /// the reader uses a read timeout and translates it into `WouldBlock`,
    /// which a byte-stream worker treats as "nothing yet" rather than an
    /// error — that is also what lets a dropped terminal's worker thread
    /// notice it was asked to stop.
    pub fn reader(&self) -> SharedReader {
        let cloned = self.writer.lock().recover_poison().try_clone();
        let (connection, initialization_error) = match cloned {
            Ok(connection) => match connection.set_read_timeout(Some(SHARED_READ_TIMEOUT)) {
                Ok(()) => (Some(connection), None),
                Err(error) => (None, Some(io::Error::other(error))),
            },
            Err(error) => (None, Some(io::Error::other(error))),
        };
        SharedReader {
            connection,
            initialization_error,
            pending: Vec::new(),
            offset: 0,
            sizes: self.sizes.clone(),
            size_signal: self.size_signal.0.clone(),
            size_application_pending: self.size_application_pending.clone(),
            pending_size_frame: self.pending_size_frame.clone(),
            reader_handoffs: self.reader_handoffs.clone(),
            stream_lost: self.stream_lost.clone(),
            lost_signal: self.lost_signal.0.clone(),
            reconnect_replay: VecDeque::new(),
        }
    }

    /// Whether the stream broke with nothing to replace it yet. See
    /// [`Self::replace_connection_from`] for what repairs it.
    pub fn stream_lost(&self) -> bool {
        self.stream_lost.load(Ordering::Acquire)
    }

    /// Resolves each time the stream breaks with nothing to replace it, so a
    /// holder can wait for that instead of polling [`Self::stream_lost`].
    pub fn stream_lost_signal(&self) -> async_channel::Receiver<()> {
        self.lost_signal.1.clone()
    }

    /// Whether a replacement is queued that the reader has not installed yet.
    ///
    /// The reader installs one only once its current connection fails, so a
    /// failure reported while one waits is already answered: attaching another
    /// would leave two queued, and the writer on the one nobody reads.
    pub fn replacement_pending(&self) -> bool {
        !self.reader_handoffs.lock().recover_poison().is_empty()
    }

    /// Replaces a failed relay while the terminal's reader remains alive. The
    /// replacement's retained replay is queued ahead of its live frames.
    pub fn replace_connection_from(&self, replacement: &SharedPane) -> Result<()> {
        let replacement_writer = replacement.writer.lock().recover_poison().try_clone()?;
        let reader_connection = replacement_writer.try_clone()?;
        reader_connection.set_read_timeout(Some(SHARED_READ_TIMEOUT))?;
        *self.writer.lock().recover_poison() = replacement_writer;
        self.reader_handoffs
            .lock()
            .recover_poison()
            .push_back(SharedReaderHandoff {
                connection: reader_connection,
                replay: replacement.replay.clone(),
            });
        let replacement_sizes = replacement
            .sizes
            .lock()
            .recover_poison()
            .drain(..)
            .collect::<Vec<_>>();
        if !replacement_sizes.is_empty() {
            self.sizes.lock().recover_poison().extend(replacement_sizes);
            let _ = self.size_signal.0.try_send(());
        }
        let replacement_viewport = replacement.initial_viewport.lock().recover_poison().take();
        if let Some(viewport) = replacement_viewport {
            *self.initial_viewport.lock().recover_poison() = Some(viewport);
            // The shared-pane task uses the same wake-up for an advertised
            // viewport and revisioned size events, but keeps their values in
            // separate queues so the former can never be reported as the
            // latter.
            let _ = self.size_signal.0.try_send(());
        }
        Ok(())
    }

    /// The daemon's advertised grid for this attachment, if it has not
    /// already been applied by the client terminal.
    pub fn take_initial_viewport(&self) -> Option<(u16, u16)> {
        self.initial_viewport.lock().recover_poison().take()
    }

    /// The sizes the multiplexer arbitrated since the last call, oldest
    /// first.
    pub fn take_revisioned_sizes(&self) -> Vec<(SessionRevision, u16, u16)> {
        self.sizes.lock().recover_poison().drain(..).collect()
    }

    pub fn take_sizes(&self) -> Vec<(u16, u16)> {
        self.take_revisioned_sizes()
            .into_iter()
            .map(|(_, columns, lines)| (columns, lines))
            .collect()
    }

    /// Resolves once the reader has recorded a size.
    ///
    /// The holder used to ask every 500 ms, which woke the thread that draws
    /// that often for every shared pane, almost always to find nothing.
    pub fn size_arrived(&self) -> async_channel::Receiver<()> {
        self.size_signal.1.clone()
    }

    /// Releases output held behind an arbitrated size frame after the UI has
    /// applied that frame to its terminal grid.
    pub fn finish_size_application(&self, size: (SessionRevision, u16, u16)) {
        let mut pending = self.pending_size_frame.lock().recover_poison();
        if *pending == Some(size) {
            *pending = None;
            self.size_application_pending
                .store(false, Ordering::Release);
        }
    }

    /// The size frame the reader is holding the stream behind, if any.
    pub fn held_size_frame(&self) -> Option<(SessionRevision, u16, u16)> {
        *self.pending_size_frame.lock().recover_poison()
    }

    /// Releases the stream whatever size frame holds it, and says which one
    /// did. For a holder that applies no grid of its own — `zmux relay-pane`,
    /// whose terminal Mosh sizes — there is nothing to wait for, and waiting on
    /// an exact match is a hold that one missed release makes permanent.
    pub fn release_size_hold(&self) -> Option<(SessionRevision, u16, u16)> {
        let mut pending = self.pending_size_frame.lock().recover_poison();
        let released = pending.take();
        self.size_application_pending
            .store(false, Ordering::Release);
        released
    }

    pub(super) fn from_connection(
        session_id: u64,
        pane_id: u64,
        child_pid: u32,
        connection: Connection,
        replay: Vec<u8>,
        initial_viewport: Option<(u16, u16)>,
    ) -> Self {
        Self {
            session_id,
            pane_id,
            child_pid,
            writer: Arc::new(Mutex::new(connection)),
            sizes: Arc::new(Mutex::new(Vec::new())),
            // A daemon must advertise a real grid, but older or interrupted
            // handovers can leave its dimensions unset. Treat that as no
            // initial viewport rather than turning a pane into a 1x1 grid.
            initial_viewport: Arc::new(Mutex::new(
                initial_viewport.filter(|(columns, lines)| *columns > 0 && *lines > 0),
            )),
            size_signal: async_channel::bounded(1),
            size_application_pending: Arc::new(AtomicBool::new(false)),
            pending_size_frame: Arc::new(Mutex::new(None)),
            reader_handoffs: Arc::new(Mutex::new(VecDeque::new())),
            stream_lost: Arc::new(AtomicBool::new(false)),
            lost_signal: async_channel::bounded(1),
            replay,
        }
    }
}

/// How long the shared reader blocks on the connection before returning
/// `WouldBlock` so the byte-stream worker can check whether it was stopped.
pub(super) const SHARED_READ_TIMEOUT: Duration = Duration::from_millis(500);

/// Reads a shared pane's output off its connection.
///
/// This is what a terminal's byte-stream worker reads from: [`Event::Output`]
/// frames become plain bytes, [`Event::Size`] frames are recorded for the
/// pane's holder to apply, and a dead multiplexer surfaces as an I/O error.
pub struct SharedReader {
    pub(super) connection: Option<Connection>,
    pub(super) initialization_error: Option<io::Error>,
    pub(super) pending: Vec<u8>,
    pub(super) offset: usize,
    pub(super) sizes: Arc<Mutex<Vec<(SessionRevision, u16, u16)>>>,
    pub(super) size_signal: async_channel::Sender<()>,
    pub(super) size_application_pending: Arc<AtomicBool>,
    pub(super) pending_size_frame: Arc<Mutex<Option<(SessionRevision, u16, u16)>>>,
    pub(super) reader_handoffs: Arc<Mutex<VecDeque<SharedReaderHandoff>>>,
    pub(super) stream_lost: Arc<AtomicBool>,
    pub(super) lost_signal: async_channel::Sender<()>,
    pub(super) reconnect_replay: VecDeque<u8>,
}

pub(super) struct SharedReaderHandoff {
    pub(super) connection: Connection,
    pub(super) replay: Vec<u8>,
}

impl io::Read for SharedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if let Some(error) = self.initialization_error.take() {
            return Err(error);
        }
        if self.size_application_pending.load(Ordering::Acquire) {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        loop {
            if self.offset < self.pending.len() {
                let available = self.pending.len() - self.offset;
                let count = available.min(buffer.len());
                buffer[..count].copy_from_slice(&self.pending[self.offset..self.offset + count]);
                self.offset += count;
                if self.offset == self.pending.len() {
                    self.pending.clear();
                    self.offset = 0;
                }
                return Ok(count);
            }
            let count = self.reconnect_replay.len().min(buffer.len());
            if count != 0 {
                for (destination, byte) in buffer[..count]
                    .iter_mut()
                    .zip(self.reconnect_replay.drain(..count))
                {
                    *destination = byte;
                }
                return Ok(count);
            }
            let Some(connection) = self.connection.as_mut() else {
                return Err(io::Error::from(io::ErrorKind::NotConnected));
            };
            match connection.receive::<Event>() {
                Ok((Event::Output { length, .. }, _)) => {
                    let bytes = match connection.read_exact(length) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            if self.install_next_handoff() {
                                continue;
                            }
                            self.note_failure(&error);
                            return Err(io::Error::from(io::ErrorKind::WouldBlock));
                        }
                    };
                    let count = bytes.len().min(buffer.len());
                    buffer[..count].copy_from_slice(&bytes[..count]);
                    self.pending = bytes[count..].to_vec();
                    return Ok(count);
                }
                Ok((
                    Event::Size {
                        revision,
                        columns,
                        lines,
                        ..
                    },
                    _,
                )) => {
                    // Set the barrier before publishing the size to the UI:
                    // it may already be awake for an earlier event.
                    self.size_application_pending.store(true, Ordering::Release);
                    *self.pending_size_frame.lock().recover_poison() =
                        Some((revision, columns, lines));
                    self.sizes
                        .lock()
                        .recover_poison()
                        .push((revision, columns, lines));
                    // The daemon queues this before bytes produced for the
                    // resized PTY. Stop here; the terminal's UI task releases
                    // us only after its emulator has the same grid.
                    // Full means a signal is already pending, which says the
                    // same thing.
                    let _ = self.size_signal.try_send(());
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
                Ok((Event::SharedClosed { .. }, _)) => {
                    // A replacement already waiting outranks the end of the
                    // stream it replaces. Ending here instead left the
                    // terminal with no reader, its replacement queued where
                    // nothing would ever read it, and the pane frozen.
                    if self.install_next_handoff() {
                        continue;
                    }
                    return Ok(0);
                }
                Ok(_) => {}
                Err(error) => {
                    // A relay failure is recoverable. Install a replacement
                    // without taking a lock around the next network read.
                    if self.install_next_handoff() {
                        continue;
                    }
                    self.note_failure(&error);
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
            }
        }
    }
}

impl SharedReader {
    /// Installs one replacement after the current connection has stopped
    /// producing frames. This is intentionally a short queue operation: the
    /// replacement's `receive`/`read_exact` calls happen after the lock is
    /// released, on the reader's worker thread.
    pub(super) fn install_next_handoff(&mut self) -> bool {
        let handoff = self.reader_handoffs.lock().recover_poison().pop_front();
        let Some(handoff) = handoff else {
            return false;
        };
        self.connection = Some(handoff.connection);
        self.reconnect_replay.extend(handoff.replay);
        self.stream_lost.store(false, Ordering::Release);
        true
    }

    /// Records a connection that failed for real rather than timing out.
    /// Either way the caller is told `WouldBlock`, so a holder waiting for a
    /// replacement keeps polling; this is how one that has to make its own
    /// replacement finds out it must.
    ///
    /// Signals only the transition: the reader keeps failing on the same dead
    /// connection until a replacement arrives, and that is one loss, not one
    /// per read.
    pub(super) fn note_failure(&self, error: &anyhow::Error) {
        if !is_would_block(error) && !self.stream_lost.swap(true, Ordering::AcqRel) {
            // Full means a signal is already pending, which says the same thing.
            let _ = self.lost_signal.try_send(());
        }
    }
}

/// Whether a connection error is really "no data arrived within the read
/// timeout", which the receive loop reports as an anyhow error with the
/// underlying I/O error somewhere in its chain.
pub(super) fn is_would_block(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            )
        })
    })
}

/// Whether a connection error is the peer having finished, rather than anything
/// having gone wrong.
#[cfg(unix)]
pub(super) fn is_closed(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::UnexpectedEof)
    })
}
