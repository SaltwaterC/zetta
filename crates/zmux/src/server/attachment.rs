//! Handing a pane to a client, exclusively or shared.
//!
//! An exclusive attachment gives the client the pty itself, so the daemon
//! stops reading it; a shared attachment keeps the daemon reading and relays
//! output to every viewer over its connection. Everything that converts
//! between the two — the handover, the grant offer, and the relay's
//! backpressure — is here.

use super::*;

pub(super) fn attach(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: Option<u64>,
    secret: Option<String>,
    client_process_id: u32,
    client_id: ClientId,
    stream_only: bool,
    connection: &mut Connection,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };

    if stream_only && !session.offered {
        return connection.send(&Response::Error {
            message: format!(
                "session {session_id} is not shared; a remote client may only attach to an offered session"
            ),
        });
    }

    // Whose session it is, before anything is revealed about it. A backgrounded
    // session belongs to the window that put it away; another process may only
    // have it once somebody has said so.
    if !session.is_in_scope_for(client_process_id) {
        let owner = session.owner.unwrap_or_default();
        // Two different situations, and the way out differs: that window can
        // share the session itself, but a window that has exited cannot, so say
        // which one this is rather than suggesting something impossible.
        let route = if process_is_running(owner) {
            format!("share it from that window, or with `zmux share {session_id}`")
        } else {
            format!(
                "that window has exited, and a backgrounded session does not become another \
                 window's on its own; `zmux share {session_id}` opens it up"
            )
        };
        return connection.send(&Response::Error {
            message: format!(
                "session {session_id} is scoped to the Zetta process that backgrounded it \
                 (process {owner}): {route}"
            ),
        });
    }
    // Held here from now on: whoever is showing a session is who it goes back to
    // when it is backgrounded again, and who may scope it back after sharing.
    if !session.offered {
        session.owner = Some(client_process_id);
    }

    // The holder answering a handover the daemon itself asked for is not a new
    // grant of access: that client is displaying the pane right now, and was
    // sent `Event::Revoke` a moment ago. Challenging it would make a protected
    // session impossible to share — the handover carries no secret, because the
    // user typed it into whichever *other* window is joining.
    let mid_handover = session.panes.iter().any(|pane| {
        // Either half of the handshake: the revoke has been sent and not
        // answered yet, or it has been answered — which is what
        // `handed_over` records — and this is the answering client coming
        // back to join. Any pane of the session counts, because the client
        // that is mid-handover on one is the client displaying them all.
        matches!(pane.attachment, Attachment::Revoking { holder } | Attachment::Granting { holder } if holder == client_process_id)
            || pane
                .handed_over
                .as_ref()
                .is_some_and(|handover| handover.client_process_id == client_process_id)
    });
    if let Some(authentication) = session.authentication.clone().filter(|_| !mid_handover) {
        let Some(secret) = secret else {
            return connection.send(&Response::AuthenticationRequired);
        };
        // An attempt inside the backoff window is refused without being
        // evaluated, and reports the same failure as a wrong secret so the
        // window cannot be probed.
        let refused = session
            .refuse_until
            .is_some_and(|until| std::time::Instant::now() < until);
        if refused || authentication.verify(&secret).is_none() {
            if !refused {
                session.failed_authentications = session.failed_authentications.saturating_add(1);
                session.refuse_until = std::time::Instant::now().checked_add(
                    crate::auth::failed_authentication_delay(session.failed_authentications),
                );
            }
            return connection.send(&Response::AuthenticationFailed);
        }
        session.failed_authentications = 0;
        session.refuse_until = None;
    }

    let state = session.state.clone();
    let summary = Box::new(session.summary.clone());
    // Resolved only now, after the secret has been checked: which panes a
    // protected session has is part of what its secret protects.
    let pane_id = match pane_id {
        Some(pane_id) => pane_id,
        None => match session.panes.first() {
            Some(pane) => pane.id,
            None => {
                return connection.send(&Response::Error {
                    message: format!("session {session_id} has no panes"),
                });
            }
        },
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    if pane.exited {
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} has ended"),
        });
    }

    // A pane nobody is reading, or that this same process already holds, can
    // be handed over directly. The latter preserves the original behaviour:
    // a client re-attaching — after a crash of its window, say — takes its
    // pane back rather than starting a revoke against itself.
    // Stream-only clients always join a shared relay. An offered pane may be
    // idle, so initialize the shared attachment here rather than ever letting
    // a remote request fall through to descriptor handover.
    if stream_only && matches!(pane.attachment, Attachment::None) {
        pane.attachment = Attachment::Shared(Vec::new());
    }
    if stream_only && matches!(pane.attachment, Attachment::Shared(_)) {
        return attach_shared(
            daemon,
            sessions,
            session_id,
            pane_id,
            client_process_id,
            client_id,
            true,
            state,
            summary,
            connection,
        );
    }
    if !stream_only
        && (matches!(pane.attachment, Attachment::None)
            || matches!(pane.attachment, Attachment::Exclusive(holder) if holder == client_process_id))
    {
        return attach_exclusive(
            daemon,
            sessions,
            session_id,
            pane_id,
            client_process_id,
            state,
            summary,
            connection,
        );
    }
    if matches!(pane.attachment, Attachment::Shared(_)) {
        return attach_shared(
            daemon,
            sessions,
            session_id,
            pane_id,
            client_process_id,
            client_id,
            stream_only,
            state,
            summary,
            connection,
        );
    }

    // Another client holds the pane, or is already handing it over. From here
    // on the pane is committed to shared mode, whether or not this client ends
    // up attaching: the holder's own re-attach joins the same shared set.
    match pane.attachment {
        Attachment::Exclusive(holder) => {
            // Ask the holder to hand the terminal over, and wait for its
            // snapshot. The wait releases the sessions lock, so the snapshot
            // handler can change the pane's attachment and notify us.
            pane.attachment = Attachment::Revoking { holder };
            let revoke = Event::Revoke {
                session_id,
                pane_id,
            };
            let subscribers = daemon.subscribers.lock().unwrap();
            let subscriber = subscribers
                .values()
                .find(|subscriber| subscriber.process_id == holder)
                .map(|subscriber| subscriber.connection.try_clone());
            drop(subscribers);
            if let Some(Ok(mut subscriber)) = subscriber
                && let Err(error) = subscriber.send(&revoke)
            {
                log::debug!("revoke delivery to client {holder} failed: {error:#}");
            }
        }
        // Mid-handover in either direction. Wait: a revoke resolves to shared,
        // and a grant resolves to exclusive, which the wait loop then revokes.
        Attachment::Revoking { .. } | Attachment::Granting { .. } => {}
        Attachment::None | Attachment::Shared(_) => {
            unreachable!("the pane's attachment was decided above");
        }
    }
    // Keep the shared-empty state alive until this request has joined. A
    // holder can re-attach concurrently with this waiter and its connection
    // may close before the waiter gets the condvar wake-up.
    pane.handover_waiters = pane.handover_waiters.saturating_add(1);
    let _ = pane;

    let deadline = Instant::now() + REVOKE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            if let Some(session) = sessions.iter_mut().find(|session| session.id == session_id)
                && let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id)
            {
                finish_handover_waiter(pane, true);
            }
            return connection.send(&Response::Error {
                message: format!(
                    "the session's current viewer did not hand over pane {pane_id} within \
                     {REVOKE_TIMEOUT:?}"
                ),
            });
        }
        let (guard, _) = match daemon.sessions_condvar.wait_timeout(sessions, remaining) {
            Ok(result) => result,
            // A poisoned lock still hands the guard back; nothing panicked
            // out of the wait that a re-lock would fix.
            Err(poisoned) => poisoned.into_inner(),
        };
        sessions = guard;
        let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
            return connection.send(&Response::Error {
                message: format!("session {session_id} does not exist"),
            });
        };
        // Re-read rather than reusing what was read before the revoke. The
        // holder of a *live* session republishes its state as part of answering
        // the revoke, because a session shared while it is on screen goes on
        // changing — panes are split and closed, tabs are renamed — long after it
        // was first offered. Using the stale copy handed a joining client the
        // layout as of whenever sharing was switched on.
        let state = session.state.clone();
        let summary = Box::new(session.summary.clone());
        let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
            return connection.send(&Response::Error {
                message: format!("session {session_id} has no pane {pane_id}"),
            });
        };
        if pane.exited {
            finish_handover_waiter(pane, true);
            return connection.send(&Response::Error {
                message: format!("session {session_id} pane {pane_id} has ended"),
            });
        }
        match pane.attachment {
            Attachment::Shared(_) => {
                finish_handover_waiter(pane, false);
                return attach_shared(
                    daemon,
                    sessions,
                    session_id,
                    pane_id,
                    client_process_id,
                    client_id.clone(),
                    stream_only,
                    state,
                    summary,
                    connection,
                );
            }
            Attachment::None => {
                finish_handover_waiter(pane, true);
                if stream_only {
                    pane.attachment = Attachment::Shared(Vec::new());
                    return attach_shared(
                        daemon,
                        sessions,
                        session_id,
                        pane_id,
                        client_process_id,
                        client_id.clone(),
                        true,
                        state,
                        summary,
                        connection,
                    );
                }
                return attach_exclusive(
                    daemon,
                    sessions,
                    session_id,
                    pane_id,
                    client_process_id,
                    state,
                    summary,
                    connection,
                );
            }
            Attachment::Exclusive(holder) => {
                // Reachable, and it used to panic here. A revoking pane whose
                // holder died is reset to unheld — by this loop or by the
                // liveness sweep — and whichever waiter wakes first takes it
                // exclusively, so a second waiter finds it exclusive again. That
                // is another client holding the pane, which is exactly the case
                // this function already handles from the top, so start over
                // rather than treating it as impossible.
                if !stream_only && holder == client_process_id {
                    finish_handover_waiter(pane, true);
                    return attach_exclusive(
                        daemon,
                        sessions,
                        session_id,
                        pane_id,
                        client_process_id,
                        state,
                        summary,
                        connection,
                    );
                }
                pane.attachment = Attachment::Revoking { holder };
                let revoke = Event::Revoke {
                    session_id,
                    pane_id,
                };
                let subscriber = daemon
                    .subscribers
                    .lock()
                    .unwrap()
                    .values()
                    .find(|subscriber| subscriber.process_id == holder)
                    .map(|subscriber| subscriber.connection.try_clone());
                if let Some(Ok(mut subscriber)) = subscriber
                    && let Err(error) = subscriber.send(&revoke)
                {
                    log::debug!("revoke delivery to client {holder} failed: {error:#}");
                }
            }
            // A grant whose taker died leaves nobody holding the descriptor: the
            // reply either never went out or went to a process that has gone.
            Attachment::Granting { holder } | Attachment::Revoking { holder }
                if !process_is_running(holder) =>
            {
                pane.attachment = Attachment::None;
                finish_handover_waiter(pane, true);
                return attach_exclusive(
                    daemon,
                    sessions,
                    session_id,
                    pane_id,
                    client_process_id,
                    state,
                    summary,
                    connection,
                );
            }
            Attachment::Revoking { .. } | Attachment::Granting { .. } => {}
        }
    }
}

/// Releases one attach request waiting for a revoke.
pub(super) fn finish_handover_waiter(pane: &mut Pane, collapse_empty_shared: bool) {
    pane.handover_waiters = pane.handover_waiters.saturating_sub(1);
    if collapse_empty_shared
        && pane.handover_waiters == 0
        && matches!(&pane.attachment, Attachment::Shared(clients) if clients.is_empty())
    {
        pane.attachment = Attachment::None;
        pane.handed_over = None;
    }
}

/// An exclusive attach: the client takes the descriptor and reads it directly.
///
/// The daemon stops reading before handing the descriptor over, and sends
/// everything already read with it. Any bytes still in the PTY buffer stay
/// there for the client to read, so nothing is lost or duplicated across the
/// switch.
///
/// Takes the sessions guard by value: the publish that announces the attach
/// must not run while the lock is still held — the caller's guard would dead
/// lock against itself.
#[expect(
    clippy::too_many_arguments,
    reason = "the attach request's decoded fields, plus the daemon, the sessions \
              guard this must take by value, and the connection to answer on"
)]
pub(super) fn attach_exclusive(
    daemon: &Arc<Daemon>,
    mut sessions: MutexGuard<'_, Vec<Session>>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    state: serde_json::Value,
    summary: Box<BackgroundSessionSummary>,
    connection: &mut Connection,
) -> Result<()> {
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    if let Err(error) = pause_pane_reader(pane) {
        return connection.send(&Response::Error {
            message: format!("could not stop the pane reader before attaching: {error:#}"),
        });
    }
    pane.attachment = exclusive_attachment(client_process_id);
    // The screen, and then nothing: the client reads the terminal itself from
    // here, so what the daemon holds stops describing this pane and must not be
    // served to a later attach as though it did.
    let replay = pane.retained.snapshot();
    pane.retained.clear();
    let child_pid = pane.pty.child_pid();
    let handles = handover_handles(daemon, pane, client_process_id)?;
    // The guard is this function's to release: publish re-locks the sessions
    // mutex, and the persistence catalogue must be updated before the reply is
    // visible to a client that may immediately refresh it.
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    if let Err(error) = forget_persisted_session(daemon, session_id) {
        log::warn!("could not remove the attached session's persisted record: {error:#}");
    }
    connection.send_with(
        &Response::Attached {
            pane_id,
            child_pid,
            replay_length: replay.len(),
            state,
            summary,
            handles: handles.values,
        },
        &handles.attachments,
    )?;
    if !replay.is_empty() {
        connection.write_all(&replay)?;
    }
    publish(daemon);
    wake_drain(daemon);
    Ok(())
}

/// Joins a pane's shared set, keeping `connection` open as this client's
/// shared data plane: output and size events arrive on it, and input and size
/// reports go back over it.
///
/// Takes the sessions guard by value: after the handshake the guard is
/// dropped, because this function then serves the client's connection for its
/// whole lifetime, and that serve loop re-locks the sessions mutex on every
/// input and resize.
#[expect(
    clippy::too_many_arguments,
    reason = "the attach request's decoded fields, plus the daemon, the sessions \
              guard this must take by value, and the connection to answer on"
)]
pub(super) fn attach_shared(
    daemon: &Arc<Daemon>,
    mut sessions: MutexGuard<'_, Vec<Session>>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    client_id: ClientId,
    stream_only: bool,
    state: serde_json::Value,
    summary: Box<BackgroundSessionSummary>,
    connection: &mut Connection,
) -> Result<()> {
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    anyhow::ensure!(
        matches!(pane.attachment, Attachment::Shared(_)),
        "pane {pane_id} is not shared"
    );
    // Everything that can fail happens before the attachment is touched. Taking
    // the shared set out first and then failing dropped every client already in
    // it — silently, since a relayed viewer learns nothing until its output
    // stops.
    //
    // The relay is a queue and its own thread, not a write from wherever the
    // output was read: the reader holds the sessions lock, and a socket write
    // under that lock is a viewer's stall becoming everybody's.
    let relay = spawn_relay(connection, client_process_id)?;
    let Attachment::Shared(clients) = &mut pane.attachment else {
        unreachable!("the attachment was just checked to be shared");
    };
    // A shared client starts at the size everyone is showing, and reports its
    // own over Resize right after attaching; that report is what may change
    // the pane's effective size.
    clients.push(SharedClient {
        process_id: client_process_id,
        client_id: client_id.clone(),
        stream_only,
        relay,
        written_seen: 0,
        wrote_at: Instant::now(),
        columns: pane.size.columns,
        lines: pane.size.lines,
        input_sent: false,
    });
    let (columns, lines) = effective_size(pane);
    // Kept, not consumed, and only the client that handed the pane over is
    // spared the screen it handed over. Taking the buffer left everybody who
    // joined afterwards with nothing but the redraws the program happened to
    // make since — and a full-screen program redraws only what changed, so that
    // is a screen of holes where its static text should be. Matched by client
    // rather than by being first, because the joining client that triggered the
    // revoke races the holder's own re-attach.
    let replay = match pane
        .handed_over
        .take_if(|handover| handover.client_process_id == client_process_id)
    {
        // The client that handed the pane over is still showing the screen it
        // handed over, so it gets the difference rather than the screen.
        Some(handover) => handover.output,
        None => pane.retained.snapshot(),
    };
    let child_pid = pane.pty.child_pid();
    connection.send(&Response::SharedAttached {
        pane_id,
        child_pid,
        replay_length: replay.len(),
        state,
        summary,
        columns,
        lines,
    })?;
    if !replay.is_empty() {
        connection.write_all(&replay)?;
    }

    drop(sessions);
    publish(daemon);
    wake_drain(daemon);

    // Serve the shared connection until the client goes away.
    let result = serve_shared(
        daemon,
        session_id,
        pane_id,
        client_process_id,
        client_id.clone(),
        connection,
    );
    remove_shared_client(daemon, session_id, pane_id, &client_id);
    result
}

/// One shared client's relay: the queue its frames go on, and how far behind it
/// is in bytes.
///
/// The backlog is measured in bytes rather than frames, because a frame count
/// punishes a viewer for the *shape* of the output instead of how far behind it
/// has fallen — a burst arriving as many small frames hit a frame limit while
/// barely a kilobyte outstanding, and the viewer was dropped for being slow for
/// an instant.
pub(super) struct Relay {
    pub(super) frames: async_channel::Sender<Arc<[u8]>>,
    pub(super) queued: Arc<AtomicUsize>,
    /// Bytes this relay has actually written, ever increasing.
    ///
    /// This, and not the size of the backlog, is what says whether a viewer is
    /// making progress. A viewer slower than the program has a backlog that *grows*
    /// while it writes steadily, so "the backlog shrank" reads as no progress and
    /// evicted exactly the viewer that had to be waited for; a viewer whose socket
    /// is full writes nothing at all, which this shows plainly.
    pub(super) written: Arc<AtomicUsize>,
}

/// How far behind a viewer may fall before its pane stops being read.
///
/// Small on purpose: a backpressure threshold, not a buffer budget. Past it the
/// pane is left unread so the *program* waits, which is the rate limit a client
/// reading its own pty provides for free.
pub(super) const RELAY_BACKPRESSURE_BYTES: usize = 512 * 1024;

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
pub(super) const RELAY_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Starts a shared client's relay: a queue, and a thread that writes it.
///
/// The thread owns a clone of the client's connection, so the serve loop keeps
/// reading input on the original while output goes out on the clone.
pub(super) fn spawn_relay(connection: &Connection, client_process_id: u32) -> Result<Relay> {
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
    let (loop_queued, loop_written) = (queued.clone(), written.clone());
    std::thread::Builder::new()
        .name("zmux relay".to_owned())
        .spawn(move || relay_loop(writer, frames, loop_queued, loop_written))
        .with_context(|| format!("starting the relay for client {client_process_id}"))?;
    Ok(Relay {
        frames: sender,
        queued,
        written,
    })
}

/// Writes one shared client's frames, off the sessions lock.
///
/// Ends when the client leaves the pane's shared set — dropping its
/// `SharedClient` drops the sender — or when the socket refuses a write, which
/// is what a viewer that has gone away looks like. [`RELAY_WRITE_TIMEOUT`] still
/// bounds a wedged write, but it now stalls this one client's relay and nothing
/// else.
pub(super) fn relay_loop(
    mut writer: Connection,
    frames: async_channel::Receiver<Arc<[u8]>>,
    queued: Arc<AtomicUsize>,
    written: Arc<AtomicUsize>,
) {
    while let Ok(frame) = frames.recv_blocking() {
        let result = writer.write_all(&frame);
        queued.fetch_sub(frame.len(), Ordering::Relaxed);
        if result.is_ok() {
            written.fetch_add(frame.len(), Ordering::Relaxed);
        }
        if let Err(error) = result {
            log::debug!("a shared client stopped accepting output: {error:#}");
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
    let _ = writer.stream().shutdown(std::net::Shutdown::Write);
}

/// Serves one client's shared connection: its input is written to the pane
/// and its size reports feed the pane's size arbitration.
pub(super) fn serve_shared(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    client_id: ClientId,
    connection: &mut Connection,
) -> Result<()> {
    loop {
        let (request, _) = connection.receive::<Request>()?;
        match request {
            Request::Input { length } => {
                let bytes = connection.read_exact(length)?;
                let mut sessions = daemon.sessions.lock().unwrap();
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
                            .find(|client| client.client_id == client_id)
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
            Request::Resize { columns, lines, .. } => {
                let mut sessions = daemon.sessions.lock().unwrap();
                let Some(pane) = sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                    .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
                else {
                    anyhow::bail!("pane {pane_id} no longer exists");
                };
                if let Attachment::Shared(clients) = &mut pane.attachment {
                    if let Some(client) = clients
                        .iter_mut()
                        .find(|client| client.client_id == client_id)
                    {
                        client.columns = columns;
                        client.lines = lines;
                    }
                    let (columns, lines) = effective_size(pane);
                    if (columns, lines) != (pane.size.columns, pane.size.lines) {
                        apply_size(daemon, pane, columns, lines);
                        broadcast_size(
                            session_id,
                            pane_id,
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

/// How long to wait for a granted pane's last relayed frames to reach the client
/// before giving up on the handover.
///
/// The wait exists because those frames have to arrive *before* the descriptor
/// does: the client starts reading the terminal itself the moment it has it, and
/// output that was still queued would then arrive after output that came later.
pub(super) const GRANT_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// Hands a shared pane's terminal back to its last remaining viewer.
///
/// The reverse of the revoke handover, and the reason it exists: a pane with one
/// viewer is being read by the daemon and relayed over a socket for no benefit,
/// which costs a quarter of the throughput of the client reading it itself.
///
/// The ordering is the whole difficulty. Everything already read has been queued
/// to this very client, so none of it may be replayed — but all of it has to land
/// before the descriptor does. So the pane stops being read, the queue is allowed
/// to empty, this end of the relay is shut down so the client's reader sees the
/// end of it, and only then does the descriptor go out.
pub(super) fn take_exclusive(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    client_id: ClientId,
    stream_only: bool,
    connection: &mut Connection,
) -> Result<()> {
    if stream_only {
        return connection.send(&Response::Error {
            message: "stream-only clients cannot take an exclusive pane".to_owned(),
        });
    }
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(pane) = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
    else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    if !matches!(&pane.attachment, Attachment::Shared(clients) if clients.len() == 1 && (clients[0].client_id == client_id || (!stream_only && clients[0].process_id == client_process_id)))
    {
        return connection.send(&Response::Error {
            message: format!("pane {pane_id} is not shared with client {client_process_id} alone"),
        });
    }
    if let Err(error) = pause_pane_reader(pane) {
        return connection.send(&Response::Error {
            message: format!("could not stop the pane reader before taking it: {error:#}"),
        });
    }
    // Only the *sole* viewer may take it. With anybody else still attached the
    // pane has to keep being relayed, and handing the descriptor over would stop
    // the others being read to.
    let relay = match &mut pane.attachment {
        Attachment::Shared(clients)
            if clients.len() == 1
                && (clients[0].client_id == client_id
                    || (!stream_only && clients[0].process_id == client_process_id)) =>
        {
            let Attachment::Shared(mut clients) = std::mem::replace(
                &mut pane.attachment,
                Attachment::Granting {
                    holder: client_process_id,
                },
            ) else {
                unreachable!("the attachment was just matched as shared");
            };
            clients.pop().expect("exactly one client was matched")
        }
        _ => {
            #[cfg(windows)]
            pane.pty.resume_reader();
            return connection.send(&Response::Error {
                message: format!(
                    "pane {pane_id} is not shared with client {client_process_id} alone"
                ),
            });
        }
    }
    .relay;
    // Released while the queue drains: the relay thread is what empties it, and
    // it must not be waiting on this lock. Nothing more is queued meanwhile —
    // `drain_reads` is false for a granting pane, and the size broadcast only
    // queues to a shared one.
    drop(sessions);

    let deadline = Instant::now() + GRANT_FLUSH_TIMEOUT;
    while relay.queued.load(Ordering::Relaxed) > 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    let flushed = relay.queued.load(Ordering::Relaxed) == 0;
    // Ends the relay: dropping the sender stops its thread, and closing this end
    // of the socket is what tells the client's reader it has everything.
    drop(relay);

    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} went away mid-handover"),
        });
    };
    let summary = Box::new(session.summary.clone());
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} went away mid-handover"),
        });
    };
    if !flushed {
        // The client never drained what it was already sent, so it cannot be
        // trusted to read the terminal either. Leave the pane unheld rather than
        // handing the descriptor to a client that is not reading.
        pane.attachment = Attachment::None;
        #[cfg(windows)]
        pane.pty.resume_reader();
        drop(sessions);
        daemon.sessions_condvar.notify_all();
        publish(daemon);
        wake_drain(daemon);
        return connection.send(&Response::Error {
            message: format!("client {client_process_id} did not drain pane {pane_id}'s relay"),
        });
    }
    pane.attachment = exclusive_attachment(client_process_id);
    // Deliberately no replay. Everything read so far went to this client over the
    // relay; sending it again would print the pane's recent output twice. What is
    // left is still in the terminal, which the client now reads itself.
    pane.retained.clear();
    pane.handed_over = None;
    let child_pid = pane.pty.child_pid();
    let handles = handover_handles(daemon, pane, client_process_id)?;
    // Neither is read on this path: the client already has the tab this pane
    // belongs to and is only swapping what feeds it. They travel because the
    // exclusive attach's reply is the shape that carries a descriptor.
    let state = serde_json::Value::Null;
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    if let Err(error) = forget_persisted_session(daemon, session_id) {
        log::warn!("could not remove the attached session's persisted record: {error:#}");
    }
    connection.send_with(
        &Response::Attached {
            pane_id,
            child_pid,
            replay_length: 0,
            state,
            summary,
            handles: handles.values,
        },
        &handles.attachments,
    )?;
    daemon.sessions_condvar.notify_all();
    publish(daemon);
    wake_drain(daemon);
    Ok(())
}

/// Offers a pane back to its viewer when a *departure* has left it the only one.
///
/// Called where a viewer leaves, and deliberately not from a scan of how many
/// viewers a pane has. A revoke handover passes through one viewer on its way to
/// two — the holder re-attaches before the client that triggered the revoke joins —
/// so a count-based offer raced it: the grant went to the holder mid-handover, and
/// the transition that mattered a moment later was then treated as already offered
/// and never announced at all.
///
/// An offer rather than an instruction: a client that cannot take a pty ignores it
/// and stays shared, which is also what makes it safe to send more than once.
pub(super) fn offer_exclusive_if_alone(daemon: &Arc<Daemon>, session_id: u64, pane: &Pane) {
    let Attachment::Shared(clients) = &pane.attachment else {
        return;
    };
    let [client] = clients.as_slice() else {
        return;
    };
    // A stream-only viewer must remain shared forever. It cannot receive a
    // descriptor or complete the exclusive grant handshake.
    if client.stream_only {
        return;
    }
    let client_id = client.client_id.clone();
    let grant = Event::Grant {
        session_id,
        pane_id: pane.id,
    };
    let subscriber = subscriber_connection(daemon, &client_id, client.process_id);
    if let Some(mut subscriber) = subscriber
        && let Err(error) = subscriber.send(&grant)
    {
        log::debug!(
            "offering pane {} back to client {} failed: {error:#}",
            pane.id,
            client_id.as_str()
        );
    }
}

/// Finds the subscription for a client event. Logical IDs are the primary key
/// for remote clients, whose forwarded socket peer process is shared by every
/// connection. Local ownership remains PID-based, though: a local process can
/// create a short-lived `Client` for a handover while its long-lived
/// subscription was opened by another `Client` value.
pub(super) fn subscriber_connection(
    daemon: &Arc<Daemon>,
    client_id: &ClientId,
    process_id: u32,
) -> Option<Connection> {
    let subscribers = daemon.subscribers.lock().unwrap();
    subscribers
        .get(client_id)
        .or_else(|| {
            subscribers
                .values()
                .find(|subscriber| subscriber.process_id == process_id)
        })
        .and_then(|subscriber| subscriber.connection.try_clone().ok())
}

/// Drops a client from a pane's shared set, ending shared mode when it was
/// the last one.
pub(super) fn remove_shared_client(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    client_id: &ClientId,
) {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return;
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return;
    };
    if let Attachment::Shared(clients) = &mut pane.attachment {
        let before = clients.len();
        clients.retain(|client| &client.client_id != client_id);
        if clients.is_empty() && pane.handover_waiters == 0 {
            pane.attachment = Attachment::None;
            // Nobody is left to claim the handover, and the next one records
            // its own.
            pane.handed_over = None;
        } else if clients.len() != before {
            // A smaller set may want a bigger pane.
            let (columns, lines) = effective_size(pane);
            if (columns, lines) != (pane.size.columns, pane.size.lines) {
                apply_size(daemon, pane, columns, lines);
                broadcast_size(
                    session_id,
                    pane_id,
                    &mut pane.attachment,
                    pane.handover_waiters,
                    columns,
                    lines,
                );
            }
            // Down to one viewer: relaying to a single client is the daemon doing
            // work the client can do better itself, so offer it the terminal.
            offer_exclusive_if_alone(daemon, session_id, pane);
        }
    }
    drop(sessions);
    prune_exited_panes(daemon);
    publish(daemon);
    wake_drain(daemon);
}

/// A client gives the daemon the screen it is showing.
///
/// During a revoke handover, the snapshot seeds retention, the pane becomes
/// shared (even with no clients yet — the revoke is committed), and every
/// attach waiting on the handover is woken to join. During a live share, the
/// exclusive attachment remains in place; the following share request then
/// persists the checkpoint while the original client keeps reading the PTY.
#[expect(
    clippy::too_many_arguments,
    reason = "the snapshot request's seven decoded fields, plus the daemon and \
              the connection to answer on"
)]
pub(super) fn snapshot(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    length: usize,
    columns: u16,
    lines: u16,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    anyhow::ensure!(
        length <= crate::retention::MAX_SNAPSHOT_BYTES,
        "a pane snapshot exceeded the retention limit"
    );
    // The raw bytes can be large; read them before taking the lock.
    let bytes = connection.read_exact(length)?;

    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    // On platforms with peer credentials, use the identity the socket vouched
    // for whenever the caller is allowed to change a protected session. The
    // envelope remains the fallback for unprotected sessions and for Windows
    // builds where an unprotected request does not need an attestation.
    let caller_process_id = control_process_id(client_process_id, peer_process_id);
    let Some((revoking, holder)) = (match &pane.attachment {
        Attachment::Revoking { holder } => Some((true, *holder)),
        Attachment::Exclusive(holder) => Some((false, *holder)),
        _ => None,
    }) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} is not showing exclusively"),
        });
    };
    if holder != caller_process_id {
        return connection.send(&Response::Error {
            message: if revoking {
                format!(
                    "session {session_id} pane {pane_id} is being handed over by another client"
                )
            } else {
                format!("session {session_id} pane {pane_id} is held by another client")
            },
        });
    }
    if !revoking {
        // A live share leaves the descriptor with this client. Seed the
        // daemon's retained screen without changing the attachment, so a
        // following Share request can persist it and the live tab keeps
        // reading the PTY directly.
        seed_retained_screen(pane, bytes);
        pane.size = TerminalSize {
            columns,
            lines,
            cell_width: 0,
            cell_height: 0,
        };
        return connection.send(&Response::Ok);
    }
    // The holder is still showing this screen, so it is the one client that
    // must not be sent it back.
    pane.handed_over = Some(RevokeHandover {
        client_process_id: holder,
        output: Vec::new(),
    });
    seed_retained_screen(pane, bytes);
    pane.attachment = Attachment::Shared(Vec::new());
    // The holder's size is what it was showing the pane at; shared clients
    // join at that size until their own reports refine it.
    pane.size = TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    };
    #[cfg(feature = "session-persistence")]
    let persisted = session.offered.then(|| persisted_live_session(session));
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    if let Some(persisted) = persisted
        && let Err(error) = persist_session(daemon, &persisted)
    {
        // The handover is already committed in memory. A persistence failure
        // must not strand the pane in a revoke state; the next publication can
        // retry the disk write.
        log::warn!("could not persist the shared pane snapshot: {error:#}");
    }
    daemon.sessions_condvar.notify_all();
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::Ok)
}
