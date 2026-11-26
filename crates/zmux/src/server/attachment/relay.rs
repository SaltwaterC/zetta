//! The shared attachment's output relay: one bounded writer per viewer, its
//! backpressure and stall limits, and serving a shared viewer's connection.

use super::*;

/// One shared client's relay: the queue its frames go on, and how far behind it
/// is in bytes.
///
/// The backlog is measured in bytes rather than frames, because a frame count
/// punishes a viewer for the *shape* of the output instead of how far behind it
/// has fallen — a burst arriving as many small frames hit a frame limit while
/// barely a kilobyte outstanding, and the viewer was dropped for being slow for
/// an instant.
/// Hands out [`SharedClient::attachment`] identities. Process-wide and never
/// reused, so no two attachments the daemon has served compare equal.
pub(super) fn next_shared_attachment() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub(in crate::server) struct Relay {
    pub(in crate::server) frames: async_channel::Sender<Arc<[u8]>>,
    /// Set when the daemon gives up on this viewer rather than the viewer
    /// leaving. Read by the relay thread once its queue closes, to decide how
    /// the stream ends: see [`relay_loop`].
    pub(in crate::server) evicted: Arc<AtomicBool>,
    pub(in crate::server) queued: Arc<AtomicUsize>,
    /// Bytes this relay has actually written, ever increasing.
    ///
    /// This, and not the size of the backlog, is what says whether a viewer is
    /// making progress. A viewer slower than the program has a backlog that *grows*
    /// while it writes steadily, so "the backlog shrank" reads as no progress and
    /// evicted exactly the viewer that had to be waited for; a viewer whose socket
    /// is full writes nothing at all, which this shows plainly.
    pub(in crate::server) written: Arc<AtomicUsize>,
}

impl Relay {
    /// Marks this viewer as dropped by the daemon, before its queue is closed
    /// by removing it from the shared set.
    pub(in crate::server) fn evict(&self) {
        self.evicted.store(true, Ordering::Release);
    }
}

/// How far behind a viewer may fall before its pane stops being read.
///
/// Small on purpose: a backpressure threshold, not a buffer budget. Past it the
/// pane is left unread so the *program* waits, which is the rate limit a client
/// reading its own pty provides for free.
pub(in crate::server) const RELAY_BACKPRESSURE_BYTES: usize = 512 * 1024;

/// How long a viewer may accept nothing at all before the daemon gives up on it.
///
/// Deliberately far longer than anything a working window does. This is not a
/// performance policy — a viewer that is merely slow is *waited for*, which is what
/// backpressure is — it is a last resort for a window that has hung with a pane
/// still attached, so one frozen window cannot hold a session for ever.
///
/// It was one second, and that was a bad mistake: a window laying out a full-screen
/// redraw goes that long without draining its socket as a matter of course, so the
/// daemon dropped viewers that were working perfectly well and left them frozen
/// mid-repaint. Slowness is normal; not reading for half a minute is not.
pub(in crate::server) const RELAY_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Starts a shared client's relay: a queue, and a thread that writes it.
///
/// The thread owns a clone of the client's connection, so the serve loop keeps
/// reading input on the original while output goes out on the clone.
pub(in crate::server) fn spawn_relay(
    connection: &Connection,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    drain: std::sync::Weak<Daemon>,
) -> Result<Relay> {
    let writer = connection.try_clone()?;
    #[cfg(unix)]
    {
        writer
            .stream()
            .set_write_timeout(Some(RELAY_WRITE_TIMEOUT))
            .ok();
    }
    // Unbounded, with `queued` as the only bound: the limit that matters is how
    // many bytes are outstanding, and a channel can only count entries.
    let (sender, frames) = async_channel::unbounded::<Arc<[u8]>>();
    let queued = Arc::new(AtomicUsize::new(0));
    let written = Arc::new(AtomicUsize::new(0));
    let evicted = Arc::new(AtomicBool::new(false));
    let (loop_queued, loop_written, loop_evicted) =
        (queued.clone(), written.clone(), evicted.clone());
    std::thread::Builder::new()
        .name("zmux relay".to_owned())
        .spawn(move || {
            relay_loop(
                writer,
                frames,
                RelayProgress {
                    queued: loop_queued,
                    written: loop_written,
                    evicted: loop_evicted,
                    drain,
                },
                session_id,
                pane_id,
            );
        })
        .with_context(|| format!("starting the relay for client {client_process_id}"))?;
    Ok(Relay {
        frames: sender,
        evicted,
        queued,
        written,
    })
}

/// The relay thread's half of the counters in [`Relay`].
pub(in crate::server) struct RelayProgress {
    pub(super) drain: std::sync::Weak<Daemon>,
    pub(in crate::server) queued: Arc<AtomicUsize>,
    pub(in crate::server) written: Arc<AtomicUsize>,
    pub(in crate::server) evicted: Arc<AtomicBool>,
}

/// Writes one shared client's frames, off the sessions lock.
///
/// Ends when the client leaves the pane's shared set — dropping its
/// `SharedClient` drops the sender — or when the socket refuses a write, which
/// is what a viewer that has gone away looks like. [`RELAY_WRITE_TIMEOUT`] still
/// bounds a wedged write, but it now stalls this one client's relay and nothing
/// else.
///
/// How it ends is what the client acts on. [`Event::SharedClosed`] is a clean
/// end of stream — the pane was handed back, the client left, the pane went —
/// and a terminal reading it finishes. Anything else is a broken stream, which
/// a client recovers by attaching again. A viewer the daemon *evicted* is the
/// second kind even though its queue simply closed: it did not ask to go, and
/// telling it the stream ended cleanly is what left a pane with no reader and
/// its replacement stream unread.
pub(in crate::server) fn relay_loop(
    mut writer: Connection,
    frames: async_channel::Receiver<Arc<[u8]>>,
    progress: RelayProgress,
    session_id: u64,
    pane_id: u64,
) {
    let RelayProgress {
        queued,
        written,
        evicted,
        drain,
    } = progress;
    let mut failed = false;
    while let Ok(frame) = frames.recv_blocking() {
        let result = writer.write_all(&frame);
        if result.is_ok() {
            written.fetch_add(frame.len(), Ordering::Relaxed);
        }
        let before = queued.fetch_sub(frame.len(), Ordering::Relaxed);
        // The drain leaves this pane's output unregistered under pressure.
        // Wake only when it can resume, not on every relayed frame.
        if before >= RELAY_BACKPRESSURE_BYTES
            && before - frame.len() < RELAY_BACKPRESSURE_BYTES
            && let Some(daemon) = drain.upgrade()
        {
            wake_drain(&daemon);
        }
        if let Err(error) = result {
            log::warn!("a shared client stopped accepting output: {error:#}");
            failed = true;
            break;
        }
    }
    // However this relay ended — retired with the pane, or given up on — the
    // client has to be able to tell that nothing more is coming.
    //
    // Dropping the connection is not enough: the serve loop holds a clone of the
    // same socket, so the client goes on waiting for output that will never
    // arrive. Returning early on a write error skipped this, which is how a viewer
    // the daemon had given up on was left with a half-drawn screen, no message and
    // no end of stream — a pane frozen with nothing to say why.
    let evicted = evicted.load(Ordering::Acquire);
    if evicted {
        log::warn!(
            "shared viewer of session {session_id} pane {pane_id} was evicted; ending its \
             stream so it reattaches"
        );
    }
    if failed || evicted {
        // The serve loop owns another clone of this socket; closing only the
        // write half would leave it blocked forever waiting for a request, so
        // it could never remove the failed viewer or announce the recoverable
        // stream event on the subscription. For an evicted viewer that is the
        // point: the serve loop ending is what reports the stream failed, so
        // the client recovers now rather than on its next keystroke.
        let _ = writer.stream().shutdown(std::net::Shutdown::Both);
    } else {
        // A normal retirement has to be distinguishable from a broken relay.
        // The marker follows every queued output, so the reader can finish
        // cleanly while the daemon still reserves WouldBlock for recovery.
        if let Err(error) = writer.send(&Event::SharedClosed {
            session_id,
            pane_id,
        }) {
            log::debug!("could not mark a shared relay closed: {error:#}");
        }
        let _ = writer.stream().shutdown(std::net::Shutdown::Write);
    }
}

/// Serves one client's shared connection: its input is written to the pane
/// and its size reports feed the pane's size arbitration.
/// Which shared stream a serve loop is serving.
#[derive(Clone, Copy)]
pub(in crate::server) struct SharedStream {
    pub(in crate::server) session_id: u64,
    pub(in crate::server) pane_id: u64,
    pub(in crate::server) client_process_id: u32,
    /// The [`SharedClient::attachment`] this stream is, which is what its
    /// input and size reports are attributed to.
    pub(in crate::server) attachment: u64,
}

pub(in crate::server) fn serve_shared(
    daemon: &Arc<Daemon>,
    stream: SharedStream,
    connection: &mut Connection,
) -> Result<()> {
    let SharedStream {
        session_id,
        pane_id,
        client_process_id,
        attachment,
    } = stream;
    loop {
        let (request, _) = connection.receive::<Request>()?;
        match request {
            Request::Input { length } => {
                let bytes = connection.read_exact(length)?;
                let mut sessions = daemon.sessions();
                let Some(pane) = sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                    .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
                else {
                    anyhow::bail!("pane {pane_id} no longer exists");
                };
                match &mut pane.attachment {
                    Attachment::Shared(clients) => {
                        let Some(client) = clients
                            .iter_mut()
                            .find(|client| client.attachment == attachment)
                        else {
                            anyhow::bail!("client {client_process_id} is not shared on {pane_id}");
                        };
                        // Attribution: which clients typed into this pane is
                        // reported with its exit, because no single shared
                        // client can know what the others typed.
                        client.input_sent = true;
                    }
                    _ => anyhow::bail!("pane {pane_id} is no longer shared"),
                }
                // Queued rather than written outright. The master is
                // non-blocking, so a terminal whose buffer is full accepts part
                // of a write and refuses the rest; `write_all` reports that as
                // an error having already consumed some of the bytes, so
                // logging and moving on silently ate a shared client's
                // keystrokes — reliably, for anything larger than the buffer's
                // free space, such as a paste.
                pane.pending_input.extend_from_slice(&bytes);
                flush_pending_input(pane);
                drop(sessions);
                // Input is what the child reacts to; wake the drain so its
                // reply is relayed promptly rather than on the idle timer, and
                // so it finishes any input the terminal could not take yet.
                wake_drain(daemon);
            }
            Request::Resize {
                revision,
                columns,
                lines,
                ..
            } => {
                let mut sessions = daemon.sessions();
                let Some(session) = sessions.iter_mut().find(|session| session.id == session_id)
                else {
                    anyhow::bail!("session {session_id} no longer exists");
                };
                let current_revision = session
                    .shared_state
                    .as_ref()
                    .map_or(crate::messages::SessionRevision::INITIAL, |state| {
                        state.revision
                    });
                // A relay is exempt: what it reports is the size of a real
                // terminal, not a measurement of some layout, and it reports
                // again only when Mosh resizes that terminal. Dropping a report
                // that lost a race with an edit left it unmeasured for good.
                let relay = session
                    .panes
                    .iter()
                    .find(|pane| pane.id == pane_id)
                    .and_then(|pane| match &pane.attachment {
                        Attachment::Shared(clients) => clients
                            .iter()
                            .find(|client| client.attachment == attachment),
                        _ => None,
                    })
                    .is_some_and(|client| client.relaying_for.is_some());
                if revision != Some(current_revision) && !relay {
                    // Dropped rather than refused: the reporter has no reply
                    // channel here, and the size it measured belongs to a
                    // layout this session has moved on from. It is logged
                    // because nothing else says so, and a client that never
                    // learns the new revision goes silently deaf to resizing.
                    log::debug!(
                        "dropping a size report for pane {pane_id} of session {session_id} at \
                         revision {:?}; the session is on {}",
                        revision.map(|revision| revision.0),
                        current_revision.0
                    );
                    continue;
                }
                let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
                    anyhow::bail!("pane {pane_id} no longer exists");
                };
                let reported = if let Attachment::Shared(clients) = &mut pane.attachment {
                    if let Some(client) = clients
                        .iter_mut()
                        .find(|client| client.attachment == attachment)
                    {
                        client.size = Some((columns, lines));
                        Some((columns, lines))
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(reported) = reported {
                    let (columns, lines) = effective_size(pane);
                    let applied = (pane.size.columns, pane.size.lines);
                    if (columns, lines) != applied {
                        apply_size(daemon, pane, columns, lines);
                    }
                    if should_broadcast_size(reported, (columns, lines), applied) {
                        broadcast_size(
                            session_id,
                            pane_id,
                            current_revision,
                            &mut pane.attachment,
                            pane.handover_waiters,
                            columns,
                            lines,
                        );
                    }
                }
            }
            // A shared client that wants to leave the relay without touching
            // the session's state can detach: the connection closes and this
            // client is dropped from the shared set. Unlike [`crate::messages::DetachRequest`],
            // no summary or snapshots are exchanged, because the session
            // belongs to no single client any more.
            Request::Detach(_) => return connection.send(&Response::Detached),
            other => anyhow::bail!("unexpected request on a shared connection: {other:?}"),
        }
    }
}
