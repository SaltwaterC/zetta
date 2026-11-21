//! Handing a pane to a client, exclusively or shared.
//!
//! An exclusive attachment gives the client the pty itself, so the daemon
//! stops reading it; a shared attachment keeps the daemon reading and relays
//! output to every viewer over its connection. Everything that converts
//! between the two — the handover, the grant offer, and the relay's
//! backpressure — is here.

use super::*;

mod handover;
pub(super) use handover::*;
mod relay;
pub(super) use relay::*;
mod exclusive;
pub(super) use exclusive::*;

/// Who is attaching, carried through the phases of one attach.
#[derive(Clone)]
pub(super) struct Attacher {
    /// The process the envelope names. Recorded as the pane's holder or viewer,
    /// and where a revoke is sent, but never on its own proof of anything: any
    /// same-user process can name any other.
    pub(super) client_process_id: u32,
    /// What the transport vouched for — the kernel's peer credentials, or a
    /// Windows attestation — and the only identity a check that grants access
    /// may rely on.
    pub(super) peer_process_id: Option<u32>,
    pub(super) client_id: ClientId,
    pub(super) stream_only: bool,
    pub(super) relaying_for: Option<ClientId>,
}

impl Attacher {
    /// The envelope's process, where the transport confirmed it is the one
    /// asking; `None` where it could not, or where it found someone else.
    ///
    /// What "is this the window that holds the pane" has to be answered
    /// against. Answered from the claim alone, any process holding the endpoint
    /// token could name a window mid-handover and skip that session's secret.
    pub(super) fn verified_process_id(&self) -> Option<u32> {
        verified_process_id(self.client_process_id, self.peer_process_id)
    }
}

pub(super) fn attach(
    daemon: &Arc<Daemon>,
    request: crate::messages::AttachRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let crate::messages::AttachRequest {
        session_id,
        pane_id,
        secret,
        force_shared,
        relaying_for,
    } = request;
    let attacher = Attacher {
        client_process_id: caller.client_process_id,
        peer_process_id: caller.peer_process_id,
        client_id: caller.client_id.clone(),
        stream_only: caller.stream_only,
        relaying_for,
    };
    let client_process_id = attacher.client_process_id;
    let stream_only = attacher.stream_only;
    let mut sessions = match authorize_attach(daemon, session_id, secret.as_deref(), &attacher) {
        Ok(sessions) => sessions,
        Err(refusal) => return connection.send(&*refusal),
    };
    let session = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .expect("authorize_attach returns the lock it found the session under");

    if session.offered {
        normalize_shared_state(daemon, session)?;
    }

    let state = session.state.clone();
    let summary = Box::new(catalog_summary(session));
    // Resolved only now, after the secret has been checked: which panes a
    // protected session has is part of what its secret protects.
    let pane_id = match pane_id {
        Some(pane_id) => pane_id,
        None => match session.panes.iter().find(|pane| !pane.exited) {
            Some(pane) => pane.id,
            None => {
                return respond_unlocked(
                    sessions,
                    connection,
                    format!("session {session_id} has no panes"),
                );
            }
        },
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return respond_unlocked(
            sessions,
            connection,
            format!("session {session_id} has no pane {pane_id}"),
        );
    };
    if pane.exited {
        return respond_unlocked(
            sessions,
            connection,
            format!("session {session_id} pane {pane_id} has ended"),
        );
    }

    // A pane nobody is reading, or that this same process already holds, can
    // be handed over directly. The latter preserves the original behaviour:
    // a client re-attaching — after a crash of its window, say — takes its
    // pane back rather than starting a revoke against itself.
    // Stream-only clients always join a shared relay. An offered pane may be
    // idle, so initialize the shared attachment here rather than ever letting
    // a remote request fall through to descriptor handover.
    if (stream_only || force_shared) && matches!(pane.attachment, Attachment::None) {
        pane.attachment = Attachment::Shared(Vec::new());
        pane.attachment_client_id = None;
    }
    if (stream_only || force_shared) && matches!(pane.attachment, Attachment::Shared(_)) {
        return attach_shared(
            daemon, sessions, session_id, pane_id, attacher, state, summary, connection, None,
        );
    }
    if force_shared {
        return respond_unlocked(
            sessions,
            connection,
            format!("session {session_id} pane {pane_id} is not available as a shared stream"),
        );
    }
    if !stream_only
        && (matches!(pane.attachment, Attachment::None)
            || matches!(pane.attachment, Attachment::Exclusive(holder) if Some(holder) == attacher.verified_process_id()))
    {
        return attach_exclusive(
            daemon,
            sessions,
            session_id,
            pane_id,
            client_process_id,
            &attacher.client_id,
            state,
            summary,
            connection,
        );
    }
    if matches!(pane.attachment, Attachment::Shared(_)) {
        return attach_shared(
            daemon, sessions, session_id, pane_id, attacher, state, summary, connection, None,
        );
    }

    // Another client holds the pane, or is already handing it over. From here
    // on the pane is committed to shared mode, whether or not this client ends
    // up attaching: the holder's own re-attach joins the same shared set.
    match pane.attachment {
        Attachment::Exclusive(holder) => {
            // The one way a pane nobody shared becomes shared: a second
            // process asking for it. Named in full, because the process that
            // asks is not otherwise recorded anywhere, and a pane silently
            // turned shared is how a window ended up relaying panes it had
            // only ever split.
            log::warn!(
                "{} asked for session {session_id} pane {pane_id}, held by {}; \
                 revoking it so the pane becomes shared (client {}, stream only: \
                 {stream_only}, relaying for: {})",
                crate::process_status::describe(client_process_id),
                crate::process_status::describe(holder),
                attacher.client_id.as_str(),
                attacher
                    .relaying_for
                    .as_ref()
                    .map_or("nobody", ClientId::as_str),
            );
            // Ask the holder to hand the terminal over, and wait for its
            // snapshot. The wait releases the sessions lock, so the snapshot
            // handler can change the pane's attachment and notify us.
            pane.attachment = Attachment::Revoking { holder };
            let revoke = Event::Revoke {
                session_id,
                pane_id,
            };
            if let Some(subscriber) = subscriber_relay_for_process(daemon, holder) {
                enqueue_subscriber_event(daemon, &subscriber, &revoke);
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

    await_handover(daemon, sessions, session_id, pane_id, &attacher, connection)
}

/// Whether this client may have session `session_id` at all, decided before
/// anything about the session is revealed to it: that the session exists, that
/// a remote client reaches only an offered one, that a backgrounded one is in
/// scope for it, and that its secret is right.
///
/// Returns the sessions lock with every check passed against the state it
/// guards, so the attach that follows acts on the session they were made
/// against. The secret is checked with that lock released — see
/// [`secret_check`] — so the checks are made again on whatever the session has
/// become by then: rescoped, reprotected, or ended.
///
/// A refusal comes back as the response to send, for the caller to send after
/// the lock is gone.
fn authorize_attach<'a>(
    daemon: &'a Daemon,
    session_id: u64,
    secret: Option<&str>,
    attacher: &Attacher,
) -> Result<MutexGuard<'a, Vec<Session>>, Box<Response>> {
    let client_process_id = attacher.client_process_id;
    let verified_process_id = attacher.verified_process_id();
    let mut proof = None;
    loop {
        let mut sessions = daemon.sessions.lock().recover_poison();
        let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
            return Err(Box::new(Response::Error {
                message: format!("session {session_id} does not exist"),
            }));
        };

        if attacher.stream_only && !session.offered {
            return Err(Box::new(Response::Error {
                message: format!(
                    "session {session_id} is not shared; a remote client may only attach to an offered session"
                ),
            }));
        }

        // Whose session it is, before anything is revealed about it. A
        // backgrounded session belongs to the window that put it away; another
        // process may only have it once somebody has said so.
        if !session.is_in_scope_for(verified_process_id) {
            return Err(Box::new(out_of_scope_refusal(session_id, session.owner)));
        }

        let authentication = session.authentication.as_ref().filter(|_| {
            !verified_process_id.is_some_and(|holder| {
                answers_handover(session, holder) || session_is_held_by(session, holder)
            })
        });
        let authorized = authentication.is_none_or(|authentication| {
            proof
                .as_ref()
                .is_some_and(|proof| authentication.authorizes(proof))
        });
        if authorized {
            // Held here from now on: whoever is showing a session is who it
            // goes back to when it is backgrounded again, and who may scope it
            // back after sharing. Only once authorized — claiming it before the
            // secret was checked let a wrong secret, or none, take an
            // unclaimed protected session's owner controls.
            if !session.offered {
                session.owner = Some(ProcessIdentity::of(client_process_id));
            }
            return Ok(sessions);
        }
        let Some(secret) = secret else {
            return Err(Box::new(Response::AuthenticationRequired));
        };
        drop(sessions);
        // The backoff, and its rule that an attempt inside the window reports
        // the same failure as a wrong secret, are `check_session_secret`'s.
        match check_session_secret(daemon, session_id, secret) {
            SecretCheck::Verified(verified) => proof = Some(verified),
            SecretCheck::Failed => return Err(Box::new(Response::AuthenticationFailed)),
            // Ended or unprotected meanwhile; the next pass says which.
            SecretCheck::NotApplicable => {}
        }
    }
}

/// Why a backgrounded session cannot be attached by this process.
fn out_of_scope_refusal(session_id: u64, owner: Option<ProcessIdentity>) -> Response {
    // Two different situations, and the way out differs: that window can
    // share the session itself, but a window that has exited cannot, so say
    // which one this is rather than suggesting something impossible.
    let running = owner.is_some_and(ProcessIdentity::is_running);
    let owner = owner.map_or(0, |owner| owner.pid);
    let route = if running {
        format!("share it from that window, or with `zmux share {session_id}`")
    } else {
        format!(
            "that window has exited, and a backgrounded session does not become another \
             window's on its own; `zmux share {session_id}` opens it up"
        )
    };
    Response::Error {
        message: format!(
            "session {session_id} is scoped to the Zetta process that backgrounded it \
             (process {owner}): {route}"
        ),
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
    client_id: &ClientId,
    state: serde_json::Value,
    summary: Box<BackgroundSessionSummary>,
    connection: &mut Connection,
) -> Result<()> {
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return respond_unlocked(
            sessions,
            connection,
            format!("session {session_id} does not exist"),
        );
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return respond_unlocked(
            sessions,
            connection,
            format!("session {session_id} has no pane {pane_id}"),
        );
    };
    if let Err(error) = pause_pane_reader(pane) {
        return respond_unlocked(
            sessions,
            connection,
            format!("could not stop the pane reader before attaching: {error:#}"),
        );
    }
    pane.attachment = exclusive_attachment(client_process_id);
    pane.attachment_client_id = (client_process_id != 0).then(|| client_id.clone());
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
    if let Err(error) = connection.send_with(
        &Response::Attached {
            pane_id,
            child_pid,
            replay_length: replay.len(),
            state,
            summary,
            handles: handles.values,
        },
        &handles.attachments,
    ) {
        release_failed_handover(daemon, daemon.sessions(), session_id, pane_id, client_id);
        return Err(error);
    }
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
    attacher: Attacher,
    state: serde_json::Value,
    summary: Box<BackgroundSessionSummary>,
    connection: &mut Connection,
    spawned_state: Option<crate::messages::SharedSessionState>,
) -> Result<()> {
    let Attacher {
        client_process_id,
        peer_process_id,
        client_id,
        stream_only,
        relaying_for,
    } = attacher;
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return respond_unlocked(
            sessions,
            connection,
            format!("session {session_id} does not exist"),
        );
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return respond_unlocked(
            sessions,
            connection,
            format!("session {session_id} has no pane {pane_id}"),
        );
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
    let relay = spawn_relay(
        connection,
        session_id,
        pane_id,
        client_process_id,
        Arc::downgrade(daemon),
    )?;
    let attachment = next_shared_attachment();
    let Attachment::Shared(clients) = &mut pane.attachment else {
        unreachable!("the attachment was just checked to be shared");
    };
    // A shared client is unmeasured until its first initialized layout report.
    // The response below carries the current effective size as advice, but an
    // unlaid-out terminal must not constrain the other viewers with it.
    clients.push(SharedClient {
        process_id: client_process_id,
        peer_process_id,
        client_id: client_id.clone(),
        attachment,
        stream_only,
        relaying_for,
        relay,
        written_seen: 0,
        wrote_at: Instant::now(),
        size: None,
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
    // Matched against the verified peer: the replay is the holder's own screen
    // diff, and handing it to whoever names the holder left the holder itself
    // re-attaching to a screen of holes.
    let verified = verified_process_id(client_process_id, peer_process_id);
    let replay = match pane
        .handed_over
        .take_if(|handover| Some(handover.client_process_id) == verified)
    {
        // The client that handed the pane over is still showing the screen it
        // handed over, so it gets the difference rather than the screen.
        Some(handover) => handover.output,
        None => pane.retained.snapshot(),
    };
    let child_pid = pane.pty.child_pid();
    let response = match spawned_state {
        Some(shared_state) => Response::SharedSpawned {
            session_id,
            pane_id,
            child_pid,
            replay_length: replay.len(),
            state,
            summary,
            shared_state,
        },
        None => Response::SharedAttached {
            pane_id,
            child_pid,
            replay_length: replay.len(),
            state,
            summary,
            columns,
            lines,
        },
    };
    connection.send(&response)?;
    if !replay.is_empty() {
        connection.write_all(&replay)?;
    }

    drop(sessions);
    publish(daemon);
    wake_drain(daemon);

    // Serve the shared connection until the client goes away.
    let result = serve_shared(
        daemon,
        SharedStream {
            session_id,
            pane_id,
            client_process_id,
            attachment,
        },
        connection,
    );
    remove_shared_attachment(daemon, session_id, pane_id, attachment);
    if let Err(error) = &result {
        log::debug!(
            "shared stream for session {session_id} pane {pane_id} failed for client {}: {error:#}",
            client_id.as_str()
        );
        crate::server::broadcast_to(
            daemon,
            &Event::SharedStreamFailed {
                session_id,
                pane_id,
            },
            &client_id,
        );
    }
    result
}

/// Finds the subscription for a client event. Logical IDs are the primary key
/// for remote clients, whose forwarded socket peer process is shared by every
/// connection. Local ownership remains PID-based, though: a local process can
/// create a short-lived `Client` for a handover while its long-lived
/// subscription was opened by another `Client` value.
pub(super) fn subscriber_relay(
    daemon: &Arc<Daemon>,
    client_id: &ClientId,
    process_id: u32,
) -> Option<Arc<SubscriberRelay>> {
    let subscribers = daemon.subscribers.lock().recover_poison();
    subscribers
        .get(client_id)
        .or_else(|| {
            subscribers
                .values()
                .find(|subscriber| subscriber.process_id == process_id)
        })
        .map(|subscriber| Arc::clone(&subscriber.relay))
}

/// Finds the client that currently owns an exclusive pane. A revoke must not
/// prefer the attaching client's logical ID: both windows subscribe, and that
/// would send the handover request back to the requester instead of its holder.
pub(super) fn subscriber_relay_for_process(
    daemon: &Arc<Daemon>,
    process_id: u32,
) -> Option<Arc<SubscriberRelay>> {
    daemon
        .subscribers
        .lock()
        .recover_poison()
        .values()
        .find(|subscriber| subscriber.process_id == process_id)
        .map(|subscriber| Arc::clone(&subscriber.relay))
}

/// Drops one attachment from a pane's shared set, ending shared mode when it
/// was the last one. Other attachments of the same client — a replacement
/// for this very stream, say — stay.
pub(super) fn remove_shared_attachment(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    attachment: u64,
) {
    let mut sessions = daemon.sessions();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return;
    };
    let revision = session
        .shared_state
        .as_ref()
        .map_or(crate::messages::SessionRevision::INITIAL, |state| {
            state.revision
        });
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return;
    };
    let removed = match &mut pane.attachment {
        Attachment::Shared(clients) => {
            let before = clients.len();
            clients.retain(|client| client.attachment != attachment);
            clients.len() != before
        }
        _ => false,
    };
    settle_shared_departure(daemon, session_id, revision, pane, removed);
    drop(sessions);
    prune_exited_panes(daemon);
    publish(daemon);
    wake_drain(daemon);
}

/// Settles a shared pane after viewers were taken out of its set.
///
/// Every way a viewer leaves has to come through here: closing its data stream
/// and an explicit leave both race to remove it, and whichever arrives second
/// finds nothing to remove. Only the explicit leave used to skip this, so a tab
/// closed on the smallest client left the pane at that client's size for good,
/// because its data stream then dropped out of a set it was no longer in.
pub(super) fn settle_shared_departure(
    daemon: &Arc<Daemon>,
    session_id: u64,
    revision: crate::messages::SessionRevision,
    pane: &mut Pane,
    removed: bool,
) {
    let Attachment::Shared(clients) = &pane.attachment else {
        return;
    };
    if clients.is_empty() && pane.handover_waiters == 0 {
        pane.attachment = Attachment::None;
        pane.attachment_client_id = None;
        // Nobody is left to claim the handover, and the next one records
        // its own.
        pane.handed_over = None;
    } else if removed {
        // A smaller set may want a bigger pane.
        let (columns, lines) = effective_size(pane);
        if (columns, lines) != (pane.size.columns, pane.size.lines) {
            apply_size(daemon, pane, columns, lines);
            broadcast_size(
                session_id,
                pane.id,
                revision,
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

/// A client gives the daemon the screen it is showing.
///
/// During a revoke handover, the snapshot seeds retention, the pane becomes
/// shared (even with no clients yet — the revoke is committed), and every
/// attach waiting on the handover is woken to join. During a live share, the
/// exclusive attachment remains in place; the following share request then
/// persists the checkpoint while the original client keeps reading the PTY.
pub(super) fn snapshot(
    daemon: &Arc<Daemon>,
    request: crate::messages::SnapshotRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let crate::messages::SnapshotRequest {
        session_id,
        pane_id,
        length,
        columns,
        lines,
    } = request;
    let Caller {
        client_process_id,
        peer_process_id,
        ..
    } = caller;
    // The raw bytes can be large; read them before taking the lock, and
    // within the same limits as a detach's.
    let mut received = read_snapshots(&daemon.snapshot_budget, connection, &[(pane_id, length)])?;
    let bytes = received.bytes.remove(&pane_id).unwrap_or_default();

    let mut sessions = daemon.sessions();
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
    // A protected session takes only the identity the transport vouched for:
    // answering a revoke as its holder decides the screen a joining client is
    // shown. The envelope remains the fallback for unprotected sessions and for
    // Windows builds where an unprotected request does not need an attestation.
    let caller_process_id = if session.authentication.is_some() {
        verified_process_id(client_process_id, peer_process_id)
    } else {
        Some(control_process_id(client_process_id, peer_process_id))
    };
    let Some((revoking, holder)) = (match &pane.attachment {
        Attachment::Revoking { holder } => Some((true, *holder)),
        Attachment::Exclusive(holder) => Some((false, *holder)),
        _ => None,
    }) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} is not showing exclusively"),
        });
    };
    if Some(holder) != caller_process_id {
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
        seed_retained_screen_with_fallback(pane, bytes, (columns, lines));
        return connection.send(&Response::Ok);
    }
    // The holder is still showing this screen, so it is the one client that
    // must not be sent it back.
    pane.handed_over = Some(RevokeHandover {
        client_process_id: holder,
        output: Vec::new(),
    });
    seed_retained_screen_with_fallback(pane, bytes, (columns, lines));
    pane.attachment = Attachment::Shared(Vec::new());
    pane.attachment_client_id = None;
    // Written under the session lock, so the seal it is written under cannot
    // be overtaken by the session being protected meanwhile.
    #[cfg(feature = "session-persistence")]
    if session.offered
        && let Err(error) = persist_live_session(daemon, session)
    {
        // The handover is already committed in memory. A persistence failure
        // must not strand the pane in a revoke state; the next publication can
        // retry the disk write.
        log::warn!("could not persist the shared pane snapshot: {error:#}");
    }
    drop(sessions);
    daemon.sessions_condvar.notify_all();
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::Ok)
}

#[cfg(test)]
#[path = "../tests/server/attachment.rs"]
mod tests;
