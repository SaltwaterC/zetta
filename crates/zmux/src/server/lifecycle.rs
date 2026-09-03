//! What a session is made of over its life: spawning it, resuming it,
//! detaching it, sharing it, and ending it.
//!
//! Every operation here decides authorization first — who may hold, take, or
//! end a session — because a session outlives the client that created it and
//! may be reached by any client that can open the socket.

use super::*;

pub(super) fn spawn(
    daemon: &Arc<Daemon>,
    request: SpawnRequest,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    #[cfg(not(feature = "session-persistence"))]
    let _ = (client_process_id, peer_process_id);

    // Keep the lease locked from authorization through the descriptor handoff.
    // Two panes may be spawned concurrently during restore, but only the first
    // successful handoff may materialize the saved session. A failed handoff
    // leaves the lease in place for the next attempt.
    #[cfg(feature = "session-persistence")]
    let restored_guard = request.session_id.map(|_| daemon.restored.lock().unwrap());
    #[cfg(feature = "session-persistence")]
    let restored = request.session_id.and_then(|session_id| {
        let restored_guard = restored_guard.as_ref()?;
        restored_guard
            .iter()
            .find(|restored| restored.request.record_id == session_id)
    });
    #[cfg(feature = "session-persistence")]
    if let Some(restored) = restored {
        anyhow::ensure!(
            restored.request.verifier.is_none() || restored.authorized_peer == peer_process_id,
            "protected restored session can only be spawned by the peer that resumed it"
        );
    }
    #[cfg(not(feature = "session-persistence"))]
    let restored: Option<()> = None;

    if let Some(session_id) = request.session_id {
        let live = daemon
            .sessions
            .lock()
            .unwrap()
            .iter()
            .any(|session| session.id == session_id);
        anyhow::ensure!(
            live || restored.is_some(),
            "session {session_id} does not exist"
        );
    }

    #[cfg(unix)]
    let options = tty::Options {
        shell: request
            .program
            .map(|program| tty::Shell::new(program, request.args)),
        working_directory: request.working_directory,
        drain_on_exit: true,
        env: request.env,
        #[cfg(not(windows))]
        child_signal_mask: None,
        // Arguments are passed through as the client resolved them, so the
        // multiplexer must not re-quote what has already been quoted.
        #[cfg(windows)]
        escape_args: false,
    };
    #[cfg(unix)]
    let pty = tty::new(&options, window_size(request.size), 0)
        .context("starting the terminal process")?;
    #[cfg(unix)]
    let child_pid = pty.child_pid();
    #[cfg(windows)]
    let (console_id, child_pid, pty, child_events) = {
        let (console_id, child_pid, handles) = daemon.pty_host.open(
            request.program,
            request.args,
            request.env,
            request.working_directory,
            request.size,
            request.console_palette,
            std::process::id(),
        )?;
        let mut handles = crate::transport::claim_duplicated(&handles);
        if handles.len() != 2 {
            let _ = daemon.pty_host.close(console_id);
            anyhow::bail!("the pseudoconsole host did not return two pipe handles");
        }
        let conin = handles.remove(1);
        let conout = handles.remove(0);
        match tty::attach(conout, conin, child_pid) {
            Ok((pty, child_events)) => (console_id, child_pid, pty, child_events),
            Err(error) => {
                let _ = daemon.pty_host.close(console_id);
                return Err(error).context("attaching the pseudoconsole to the daemon");
            }
        }
    };
    let pane_id = daemon.next_pane_id.fetch_add(1, Ordering::Relaxed);
    let retention = *daemon.retention.lock().unwrap();

    let mut sessions = daemon.sessions.lock().unwrap();
    // `session.panes.len()` is the zero-based index of the pane being added
    // until the pane is pushed below. Keep it before the push so the first
    // restored shell receives snapshot zero rather than being shifted past it.
    #[cfg(feature = "session-persistence")]
    let restored_snapshot_index = sessions
        .iter()
        .find(|session| {
            request
                .session_id
                .is_some_and(|session_id| session.id == session_id)
        })
        .map_or(0, |session| session.panes.len());
    let session_id = match request.session_id {
        Some(id) if sessions.iter().any(|session| session.id == id) => id,
        #[cfg(feature = "session-persistence")]
        Some(id) if restored.is_some() => {
            let restored = restored.as_ref().expect("restore lease was checked");
            let mut summary = restored.request.summary.clone();
            summary.id = id;
            let authentication = restored
                .request
                .verifier
                .clone()
                .map(SessionAuthentication::from_verifier)
                .transpose()?;
            sessions.push(Session {
                id,
                summary,
                state: restored.request.state.clone(),
                authentication,
                key_envelope: restored.request.key_envelope.clone(),
                failed_authentications: restored.request.failed_authentications,
                refuse_until: (restored.request.backoff_seconds > 0).then(|| {
                    Instant::now() + Duration::from_secs(restored.request.backoff_seconds)
                }),
                panes: Vec::new(),
                keep: false,
                offered: false,
                owner: Some(client_process_id),
            });
            id
        }
        _ => {
            let id = daemon.next_session_id.fetch_add(1, Ordering::Relaxed);
            sessions.push(Session {
                id,
                summary: BackgroundSessionSummary {
                    id,
                    title: String::new(),
                    authentication_required: false,
                    active_pane: pane_id,
                    layout: BackgroundPaneLayout::Pane { pane_id },
                    panes: Vec::new(),
                    held: false,
                    scoped_to: None,
                    key_envelope: None,
                },
                state: serde_json::Value::Null,
                authentication: None,
                key_envelope: None,
                failed_authentications: 0,
                refuse_until: None,
                panes: Vec::new(),
                keep: false,
                offered: false,
                // The window that spawned it owns it from the start, so a tab
                // backgrounded later is already this process's and needs no
                // separate claim.
                owner: Some(request.client_process_id),
            });
            id
        }
    };
    let session = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .expect("the session was just located or created");

    // Record the pane before answering. A handover that fails after the
    // session exists but before the pane does would otherwise leave a session
    // holding nothing: unattachable, and — because "every pane is unattached"
    // is vacuously true of no panes — liable to be published as though the
    // user could pick it up.
    session.panes.push(Pane {
        id: pane_id,
        pty,
        #[cfg(windows)]
        console_id,
        #[cfg(windows)]
        child_events,
        attachment: exclusive_attachment(request.client_process_id),
        size: request.size,
        retained: {
            let retained = retention.new_retained(request.size.columns, request.size.lines);
            #[cfg(feature = "session-persistence")]
            let retained = {
                let mut retained = retained;
                if let Some(snapshot) =
                    restored.and_then(|restored| restored.snapshots.get(restored_snapshot_index))
                {
                    retained.seed(snapshot.bytes.clone());
                }
                retained
            };
            retained
        },
        handed_over: None,
        handover_waiters: 0,
        exited: false,
        exit_status: None,
        pending_input: Vec::new(),
    });
    let pane = session.panes.last().expect("the pane was just pushed");
    let handover = handover_handles(daemon, pane, request.client_process_id).and_then(|handles| {
        connection.send_with(
            &Response::Spawned {
                session_id,
                pane_id,
                child_pid,
                handles: handles.values,
            },
            &handles.attachments,
        )
    });
    if let Err(error) = handover {
        // Nobody received this terminal, so nothing can ever read it.
        session.panes.retain(|pane| pane.id != pane_id);
        sessions.retain(|session| !session.panes.is_empty());
        #[cfg(windows)]
        let _ = daemon.pty_host.close(console_id);
        return Err(error);
    }
    #[cfg(feature = "session-persistence")]
    if let Some(session_id) = request.session_id
        && restored.is_some()
        && let Some(restored_guard) = restored_guard
    {
        let mut leases = restored_guard;
        if let Some(lease) = leases
            .iter_mut()
            .find(|restored| restored.request.record_id == session_id)
        {
            lease.spawned_panes = lease.spawned_panes.saturating_add(1);
            let expected_panes = lease.request.summary.panes.len().max(1);
            if lease.spawned_panes >= expected_panes {
                leases.retain(|restored| restored.request.record_id != session_id);
            }
        }
    }
    Ok(())
}

pub(super) fn resume(
    daemon: &Arc<Daemon>,
    request: crate::messages::ResumeRequest,
    _client_process_id: u32,
    peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    #[cfg(not(feature = "session-persistence"))]
    {
        let _ = (daemon, request, peer_process_id);
        connection.send(&Response::Error {
            message: "resume needs the session-persistence feature".to_owned(),
        })
    }
    #[cfg(feature = "session-persistence")]
    {
        let mut request = request;
        // The metadata frame intentionally contains lengths only. Read the
        // potentially large screen bytes through the raw side of the same
        // connection, just as detach does, so a record's scrollback cannot
        // exceed the control-message ceiling.
        let mut snapshots = HashMap::new();
        for snapshot in &request.snapshots {
            anyhow::ensure!(
                snapshot.length <= crate::retention::MAX_SNAPSHOT_BYTES,
                "a pane snapshot exceeded the retention limit"
            );
            snapshots.insert(snapshot.pane_id, connection.read_exact(snapshot.length)?);
        }
        // A daemon that was started before the memory-fallback recovery path
        // was available may still be running without a persistence handle.
        // Open a temporary recovery handle in that case so a disk record can
        // still be resumed; it is deliberately not installed on the daemon,
        // and therefore cannot make a memory-mode session durable.
        let mut fallback_persistence = if daemon.persistence.lock().unwrap().is_none() {
            PersistenceStore::open_with_recovery_state(&daemon.directory, None, false)?
        } else {
            None
        };
        let daemon_has_record =
            daemon
                .persistence
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|persistence| {
                    persistence
                        .records()
                        .iter()
                        .any(|record| record.id == request.record_id && record.restorable)
                });
        let fallback_has_record = fallback_persistence.as_ref().is_some_and(|persistence| {
            persistence
                .records()
                .iter()
                .any(|record| record.id == request.record_id && record.restorable)
        });
        anyhow::ensure!(
            daemon_has_record || fallback_has_record,
            "restorable session {} does not exist",
            request.record_id
        );
        anyhow::ensure!(
            !daemon
                .sessions
                .lock()
                .unwrap()
                .iter()
                .any(|session| session.id == request.record_id),
            "session {} is already live",
            request.record_id
        );
        anyhow::ensure!(
            !daemon
                .restored
                .lock()
                .unwrap()
                .iter()
                .any(|restored| restored.request.record_id == request.record_id),
            "session {} is already being resumed",
            request.record_id
        );
        if let Some(verifier) = request.verifier.as_ref() {
            anyhow::ensure!(
                peer_process_id.is_some(),
                "protected restores require a verified peer process"
            );
            let authentication = SessionAuthentication::from_verifier(verifier.clone())?;
            let elapsed = unix_now().saturating_sub(request.updated_at);
            if request.backoff_seconds > elapsed {
                return connection.send(&Response::AuthenticationFailed);
            }
            let Some(secret) = request.secret.as_deref() else {
                return connection.send(&Response::AuthenticationRequired);
            };
            if authentication.verify(secret).is_none() {
                request.secret = None;
                request.failed_authentications = request.failed_authentications.saturating_add(1);
                request.backoff_seconds =
                    crate::auth::failed_authentication_delay(request.failed_authentications)
                        .as_secs();
                request.updated_at = unix_now();
                let mut persistence = daemon.persistence.lock().unwrap();
                if let Some(persistence) = persistence.as_mut() {
                    persistence.update_authentication(
                        request.record_id,
                        request.updated_at,
                        request.failed_authentications,
                        request.backoff_seconds,
                    )?;
                } else if let Some(persistence) = fallback_persistence.as_mut() {
                    persistence.update_authentication(
                        request.record_id,
                        request.updated_at,
                        request.failed_authentications,
                        request.backoff_seconds,
                    )?;
                }
                return connection.send(&Response::AuthenticationFailed);
            }
        }
        // The secret is only a request credential. It must not survive in the
        // daemon's restored-session memory after the verifier has been checked.
        request.secret = None;
        // The decrypted record is now represented by the authenticated lease.
        // Removing the disk copy keeps it from being offered a second time or
        // mistaken for a separate restore while the first handoff is in
        // flight. A failed Spawn leaves the lease itself intact.
        let mut persistence = daemon.persistence.lock().unwrap();
        if let Some(persistence) = persistence.as_mut() {
            persistence.forget(request.record_id)?;
        } else if let Some(persistence) = fallback_persistence.as_mut() {
            persistence.forget(request.record_id)?;
        }
        drop(persistence);
        let authorized_peer = request.verifier.as_ref().and(peer_process_id);
        let snapshots = request
            .snapshots
            .iter()
            .filter_map(|snapshot| {
                snapshots.get(&snapshot.pane_id).map(|bytes| RestoredPane {
                    bytes: bytes.clone(),
                })
            })
            .collect();
        let resumed_session_id = request.record_id;
        daemon.restored.lock().unwrap().push(RestoredSession {
            request,
            restored_at: unix_now(),
            authorized_peer,
            snapshots,
            spawned_panes: 0,
        });
        connection.send(&Response::Resumed {
            session_id: resumed_session_id,
        })
    }
}

/// How long a shared client's socket write may stall before its relay gives up
/// and stops relaying to it.
///
/// A backstop, and deliberately the same patience as [`RELAY_STALL_TIMEOUT`]: both
/// answer "this viewer has accepted nothing for a while", so they should not
/// disagree about how long a while is. When this was five times more patient it won
/// the race often enough to matter — a viewer that had stopped reading held its
/// pane for four and a half seconds instead of the one the stall rule intends,
/// because the write blocked before the backlog had grown enough to be noticed.
///
/// It bounds that one client's relay thread and nothing else. It used to bound the
/// *drain* thread, because the relay wrote to every viewer's socket while holding
/// the sessions lock — so a viewer that stopped reading froze every pane the daemon
/// holds. The queue in [`SharedClient::relay`] is what took the drain out of that
/// path.
#[cfg(unix)]
pub(super) const RELAY_WRITE_TIMEOUT: Duration = RELAY_STALL_TIMEOUT;

pub(super) fn detach(
    daemon: &Arc<Daemon>,
    request: crate::messages::DetachRequest,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    // Read the snapshots before taking the lock: they arrive as raw bytes on
    // this connection and can be large.
    let mut snapshots = HashMap::new();
    for snapshot in &request.snapshots {
        anyhow::ensure!(
            snapshot.length <= crate::retention::MAX_SNAPSHOT_BYTES,
            "a pane snapshot exceeded the retention limit"
        );
        snapshots.insert(snapshot.pane_id, connection.read_exact(snapshot.length)?);
    }
    #[cfg(feature = "session-persistence")]
    let persisted_snapshots = request
        .snapshots
        .iter()
        .filter_map(|snapshot| {
            snapshots
                .get(&snapshot.pane_id)
                .map(|bytes| PersistedSnapshot {
                    pane_id: snapshot.pane_id,
                    bytes: bytes.clone(),
                    columns: None,
                    lines: None,
                })
        })
        .collect::<Vec<_>>();

    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions
        .iter_mut()
        .find(|session| session.id == request.session_id)
    else {
        return connection.send(&Response::Error {
            message: format!("session {} does not exist", request.session_id),
        });
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, None),
        "session {} is protected and can only be detached by its owner or current holder",
        request.session_id
    );
    // The session's identity belongs to the daemon, not to whatever the
    // client puts in the summary. A client that published a summary whose id
    // disagreed with the session's own id (a tab id instead of the mux
    // session id, say) would make the catalog list a session under one id
    // while attach, kill and resize looked for it under another: listed, but
    // neither attachable nor killable.
    session.summary = request.summary;
    session.summary.id = session.id;
    session.state = request.state;
    session.keep = true;
    // A plain detach is private, but a tab that was already shared asked for
    // both properties: keep it after this window goes and leave it available to
    // another process. Preserve the offer through the handoff so closing a
    // keep-running tab does not narrow it back to the process that just closed.
    if !session.offered {
        session.owner = Some(client_process_id);
    }
    if let Some(verifier) = request.verifier {
        session.authentication = Some(SessionAuthentication::from_verifier(verifier)?);
        // Paired with the verifier it arrived beside, so a session reprotected
        // with a typed secret loses the stale envelope rather than keeping a way
        // in that no longer opens anything.
        session.key_envelope = request.key_envelope;
        session.failed_authentications = 0;
        session.refuse_until = None;
    }
    session.summary.authentication_required = session.authentication.is_some();
    for pane in &mut session.panes {
        // Only this client lets go. Clearing every attachment evicted whichever
        // *other* windows were sharing the pane — silently, since they learn
        // nothing until their relay stops — and let this client's snapshot
        // overwrite a screen those windows were still driving. A pane the
        // detaching client was not holding is simply left alone.
        match pane.attachment {
            Attachment::Exclusive(holder) | Attachment::Revoking { holder }
                if holder == client_process_id =>
            {
                pane.attachment = Attachment::None;
                #[cfg(windows)]
                pane.pty.resume_reader();
            }
            // A shared viewer detaching the session gives up the session, not
            // the relay; its own connection closing is what removes it from the
            // shared set.
            Attachment::Shared(_) => continue,
            Attachment::None => {}
            // Held by somebody else, so not this client's to release.
            _ => continue,
        }
        if let Some(snapshot) = snapshots.remove(&pane.id) {
            seed_retained_screen(pane, snapshot);
        }
    }
    #[cfg(feature = "session-persistence")]
    let persisted = daemon
        .persistence_enabled
        .load(Ordering::Acquire)
        .then(|| PersistedSession {
            id: session.id,
            created_at: unix_now(),
            updated_at: unix_now(),
            summary: session.summary.clone(),
            state: session.state.clone(),
            verifier: session
                .authentication
                .as_ref()
                .map(|authentication| authentication.verifier().to_owned()),
            key_envelope: session.key_envelope.clone(),
            failed_authentications: session.failed_authentications,
            backoff_seconds: session
                .refuse_until
                .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
                .unwrap_or_default(),
            snapshots: persisted_snapshots
                .into_iter()
                .map(|mut snapshot| {
                    if let Some(pane) = session
                        .panes
                        .iter()
                        .find(|pane| pane.id == snapshot.pane_id)
                    {
                        let (columns, lines) =
                            terminal_size(pane).unwrap_or((pane.size.columns, pane.size.lines));
                        snapshot.columns = Some(columns);
                        snapshot.lines = Some(lines);
                    }
                    snapshot
                })
                .collect(),
        });
    #[cfg(feature = "session-persistence")]
    if let Some(persisted) = persisted {
        persist_session(daemon, &persisted)?;
    }
    drop(sessions);

    prune_exited_panes(daemon);
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::Detached)
}

/// Offers, or withdraws, a session its client is still showing.
///
/// Everything a joining client needs is published here, exactly as a detach
/// publishes it: the summary the catalog lists and the state a joining client
/// rebuilds its tab from. The client checkpoints each exclusive pane with
/// [`Request::Snapshot`] immediately beforehand. What is *not* done is the rest
/// of detaching — no attachment is released and `keep` is left alone — so the
/// session carries on being displayed by the window that shared it.
pub(super) fn share(
    daemon: &Arc<Daemon>,
    request: crate::messages::ShareRequest,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
    connection: &mut Connection,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions
        .iter_mut()
        .find(|session| session.id == request.session_id)
    else {
        return connection.send(&Response::Error {
            message: format!("session {} does not exist", request.session_id),
        });
    };
    // Only a client that is showing the session may offer it. Two reasons, and
    // the second is the one that matters: a client cannot describe a session it
    // is not displaying, and this request rewrites the verifier, so letting any
    // client send it would be a way to reprotect a session without knowing the
    // secret it already has.
    // A protected session's holder has to be an identity something vouched
    // for. Taking the client's word for it would let anyone who can reach the
    // socket name the real holder and rewrite the verifier from there.
    let holder_process_id = if session.authentication.is_some() {
        peer_process_id
    } else {
        Some(client_process_id)
    };
    let holder_authorized =
        holder_process_id.is_some_and(|process_id| session_is_held_by(session, process_id));
    let secret_authorized = session.authentication.is_some()
        && session_control_authorized(session, peer_process_id, session_secret);
    if !holder_authorized && !secret_authorized {
        return connection.send(&Response::Error {
            message: format!(
                "session {} cannot be shared by a client that is not showing it",
                request.session_id
            ),
        });
    }
    // Withdrawing means scoping the session back to one window, and that can only
    // be done while one window has it. There is no way to take a pane away from a
    // viewer that is still relaying it — a grant goes to the *last* viewer, not to
    // a chosen one — so a request that cannot be honoured is refused rather than
    // half-applied. Half-applying it was the old behaviour: the session stopped
    // being listed while other windows carried on driving it, so "not shared" and
    // "shared" looked the same from the tab that asked.
    if !request.offered
        && let Some(pane) = session
            .panes
            .iter()
            .find(|pane| shared_viewer_count(&pane.attachment) > 1)
    {
        let viewers = shared_viewer_count(&pane.attachment);
        return connection.send(&Response::Error {
            message: format!(
                "this session is still open in {} windows, so it cannot be scoped back to one; \
                 close it in the others first",
                viewers
            ),
        });
    }
    #[cfg(feature = "session-persistence")]
    if !request.offered {
        forget_persisted_session(daemon, request.session_id)?;
    }
    // The session's identity belongs to the daemon, as in `detach`: a summary
    // published under a tab id rather than the mux session id would be listed
    // under one id and attachable under another.
    session.summary = request.summary;
    session.summary.id = session.id;
    session.state = request.state;
    session.offered = request.offered;
    if !request.offered {
        // Scoped back to the window that asked, which the check above has
        // established is the one showing it.
        session.owner = Some(client_process_id);
    }
    if let Some(verifier) = request.verifier {
        session.authentication = Some(SessionAuthentication::from_verifier(verifier)?);
        session.key_envelope = request.key_envelope;
        session.failed_authentications = 0;
        session.refuse_until = None;
    }
    session.summary.authentication_required = session.authentication.is_some();
    #[cfg(feature = "session-persistence")]
    if request.offered {
        persist_session(daemon, &persisted_live_session(session))?;
    }
    if !request.offered {
        // Scoped to one window now, so a pane still being relayed to its single
        // viewer should stop being relayed. Offered from here rather than left to a
        // departure, because this *is* the moment the session became one window's.
        for pane in &session.panes {
            offer_exclusive_if_alone(daemon, request.session_id, pane);
        }
    }
    drop(sessions);

    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::Ok)
}

/// Scopes a session to one process, or shares it with every process.
///
/// The CLI's half of the sharing toggle, for a session in the background. It is
/// deliberately not [`share`]: that request comes from the window displaying the
/// session and rewrites what the catalog says about it, so it insists the caller
/// is showing it. Nobody is showing a backgrounded session, so this asks only for
/// the flag — and, because the caller is a command that exits a moment later,
/// scoping back means scoping to the process that last held the session rather
/// than to whoever asked.
pub(super) fn set_session_scope(
    daemon: &Arc<Daemon>,
    session_id: u64,
    shared: bool,
    verifier: Option<String>,
    key_envelope: Option<String>,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
    stream_only: bool,
    connection: &mut Connection,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };
    // A session on screen has a window that can toggle this itself, and that
    // route enforces a rule this one cannot: a session may only be scoped back
    // while a single window has it. Refusing here keeps the two from
    // disagreeing about a session several windows are driving.
    if session.panes.iter().any(|pane| !pane.attachment.is_none()) {
        return connection.send(&Response::Error {
            message: format!(
                "session {session_id} is on screen; share or unshare it from the window showing it"
            ),
        });
    }
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, session_secret)
            || (!stream_only
                && stranded_session_may_be_offered(session, shared, verifier.is_some())),
        "session {session_id} is protected and can only be changed by its owner or current holder"
    );
    if let Some(verifier) = verifier {
        session.authentication = Some(SessionAuthentication::from_verifier(verifier)?);
        session.key_envelope = key_envelope;
        session.failed_authentications = 0;
        session.refuse_until = None;
        session.summary.authentication_required = true;
    }
    if !shared {
        let Some(owner) = session.owner.filter(|owner| process_is_running(*owner)) else {
            return connection.send(&Response::Error {
                message: format!(
                    "session {session_id} has no window to scope it back to; attach it first, \
                     and it becomes that window's again"
                ),
            });
        };
        session.owner = Some(owner);
    }
    session.offered = shared;
    #[cfg(feature = "session-persistence")]
    if daemon.persistence_enabled.load(Ordering::Acquire) {
        persist_session(daemon, &persisted_live_session(session))?;
    }
    drop(sessions);
    publish(daemon);
    connection.send(&Response::Ok)
}

/// The identity a control request acts as: what the transport vouched for, or
/// the envelope's own claim where nothing could.
///
/// Only for checks whose failure is inconvenient rather than a privilege
/// boundary — "are you the window holding this pane" for a session with no
/// secret, which any same-user process could attach for itself anyway. A
/// protected session's controls go through [`session_control_authorized`] or
/// [`protected_holder_authorized`], neither of which accepts a claim.
pub(super) fn control_process_id(client_process_id: u32, peer_process_id: Option<u32>) -> u32 {
    peer_process_id.unwrap_or(client_process_id)
}

/// Whether a protected session may be acted on by this peer as one of the
/// clients currently showing it.
///
/// Distinct from [`session_control_authorized`] in refusing the owner: resizing
/// a pane or repainting its palette is something only a window displaying it
/// can sensibly ask for.
pub(super) fn protected_holder_authorized(session: &Session, peer_process_id: Option<u32>) -> bool {
    peer_process_id.is_some_and(|process_id| session_is_held_by(session, process_id))
}

/// Whether a peer that is neither the owner nor a holder may still *offer* a
/// protected session.
///
/// The way out of a deadlock that would otherwise be permanent. Attaching a
/// scoped session refuses and says to share it — see `attach`, which spells out
/// that a window which has exited cannot share it itself — and sharing refused
/// in turn because the owner it named no longer exists. A live session, and
/// whatever is still running inside it, was unreachable for good.
///
/// Safe because offering changes no secret. It makes the session *listed* and
/// attachable; whoever attaches still has to present the secret already set, and
/// a protected session's catalog entry is stripped either way. Replacing the
/// verifier is the half that hands a session over, and that stays with the owner.
///
/// Restricted to an owner that is gone, so it can never overrule a window that
/// is running and has deliberately kept its session to itself.
pub(super) fn stranded_session_may_be_offered(
    session: &Session,
    shared: bool,
    replaces_verifier: bool,
) -> bool {
    shared && !replaces_verifier && !session.owner.is_some_and(process_is_running)
}

pub(super) fn session_control_authorized(
    session: &mut Session,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> bool {
    if session.authentication.is_none() {
        return true;
    }
    if peer_process_id.is_some_and(|process_id| {
        session.owner == Some(process_id) || session_is_held_by(session, process_id)
    }) {
        return true;
    }
    // Remote stream-only clients cannot prove a local PID through the SSH
    // socket. Their session secret is the explicit authorization for safe
    // administration, checked with the same constant-time Argon2 verifier and
    // exponential refusal window used by attach/resume.
    let Some(secret) = session_secret else {
        return false;
    };
    let refused = session
        .refuse_until
        .is_some_and(|until| Instant::now() < until);
    let verified = !refused
        && session
            .authentication
            .as_ref()
            .is_some_and(|authentication| authentication.verify(secret).is_some());
    if verified {
        session.failed_authentications = 0;
        session.refuse_until = None;
        return true;
    }
    if !refused {
        session.failed_authentications = session.failed_authentications.saturating_add(1);
        session.refuse_until = Instant::now().checked_add(
            crate::auth::failed_authentication_delay(session.failed_authentications),
        );
    }
    false
}

/// Whether this request's answer could depend on who the peer is, on a platform
/// that cannot tell without asking.
///
/// Narrow on purpose, because asking costs a round trip: only requests that can
/// act on a session somebody else owns, and only when the session they name is
/// actually protected. Nothing else has an authorization decision to make — a
/// session with no secret can be attached by any same-user process for itself,
/// so confirming which process is asking would protect nothing.
/// `Detach` and `Resume` are absent on purpose: they stream raw bytes after
/// their message, so there is nowhere to interject a challenge. Those ask to be
/// identified first, with [`Request::Attest`].
#[cfg(windows)]
pub(super) fn attestation_needed(daemon: &Arc<Daemon>, request: &Request) -> bool {
    #[cfg(feature = "session-persistence")]
    if let Request::Spawn(SpawnRequest {
        session_id: Some(session_id),
        ..
    }) = request
    {
        if daemon.restored.lock().unwrap().iter().any(|restored| {
            restored.request.record_id == *session_id && restored.request.verifier.is_some()
        }) {
            return true;
        }
    }
    let session_id = match request {
        Request::Share(request) => Some(request.session_id),
        Request::Snapshot { session_id, .. } => Some(*session_id),
        Request::Resize { session_id, .. }
        | Request::SetConsolePalette { session_id, .. }
        | Request::ClosePane { session_id, .. }
        | Request::Kill { session_id }
        | Request::SetSessionScope { session_id, .. }
        | Request::Forget { session_id } => Some(*session_id),
        // Names panes rather than a session, and whose sessions those are is
        // exactly what it must not disclose, so it is decided against whatever
        // protected sessions exist.
        Request::PaneStates { .. } => None,
        _ => return false,
    };
    let sessions = daemon.sessions.lock().unwrap();
    match session_id {
        Some(session_id) => sessions
            .iter()
            .any(|session| session.id == session_id && session.authentication.is_some()),
        None => sessions
            .iter()
            .any(|session| session.authentication.is_some()),
    }
}

/// Asks the peer to prove it is the process its envelope named.
///
/// Answered means the identity is as good as a kernel-reported one for this
/// connection; unanswered, or answered wrongly, means the request goes ahead
/// with no identity and every protected-session check refuses it.
#[cfg(windows)]
pub(super) fn attest_peer(
    connection: &mut Connection,
    client_process_id: u32,
) -> Result<Option<u32>> {
    /// Bounded, unlike most of the daemon's reads: until the answer arrives or
    /// the attempt is abandoned, the challenge is holding a handle inside
    /// another process. A client that names somebody else and then says nothing
    /// must not be able to leave it there.
    const ANSWER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    let challenge = crate::transport::PeerChallenge::issue(client_process_id)?;
    connection.send(&Response::AttestationRequired {
        handle: challenge.handle(),
    })?;
    connection.set_read_timeout(Some(ANSWER_TIMEOUT))?;
    let answer = connection
        .receive::<Request>()
        .context("reading a peer attestation answer");
    connection.set_read_timeout(None)?;
    let Request::Attested { nonce } = answer?.0 else {
        anyhow::bail!("expected an attestation answer");
    };
    Ok(challenge.matches(&nonce).then_some(client_process_id))
}

/// How many windows are being relayed this pane. Zero for a pane one window holds
/// outright, which is the state unsharing is trying to reach.
pub(super) fn shared_viewer_count(attachment: &Attachment) -> usize {
    match attachment {
        Attachment::Shared(clients) => clients.len(),
        Attachment::None
        | Attachment::Exclusive(_)
        | Attachment::Revoking { .. }
        | Attachment::Granting { .. } => 0,
    }
}

/// Whether this client is one of the clients showing any of the session's panes.
pub(super) fn session_is_held_by(session: &Session, client_process_id: u32) -> bool {
    session
        .panes
        .iter()
        .any(|pane| pane_is_held_by(pane, client_process_id))
}

pub(super) fn pane_is_held_by(pane: &Pane, client_process_id: u32) -> bool {
    match &pane.attachment {
        Attachment::Exclusive(holder)
        | Attachment::Revoking { holder }
        | Attachment::Granting { holder } => *holder == client_process_id,
        Attachment::Shared(clients) => clients
            .iter()
            .any(|client| client.process_id == client_process_id),
        Attachment::None => false,
    }
}

/// Applies a client's new size to a pane's terminal, recording it as the
/// pane's size so a later shared attach starts from it.
pub(super) fn resize_pane(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    columns: u16,
    lines: u16,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Result<()> {
    use alacritty_terminal::event::OnResize as _;
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    if session.authentication.is_some()
        && !protected_holder_authorized(session, peer_process_id)
        && !session_control_authorized(session, peer_process_id, session_secret)
    {
        anyhow::bail!(
            "session {session_id} is protected and can only be resized by its current holder or session secret"
        );
    }
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        anyhow::bail!("session {session_id} has no pane {pane_id}");
    };
    pane.pty.on_resize(window_size(TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    }));
    #[cfg(windows)]
    daemon
        .pty_host
        .resize(pane.console_id, columns, lines)
        .context("resizing the pseudoconsole")?;
    pane.size = TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    };
    // What is kept of the pane wraps where the pane wraps, or a reattach would
    // show the session rewrapped at a width nothing was drawn at.
    pane.retained.resize(columns, lines);
    Ok(())
}

pub(super) fn set_console_palette(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    palette: ConsolePalette,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    let protected = session.authentication.is_some();
    let holder_authorized = if protected {
        protected_holder_authorized(session, peer_process_id)
    } else {
        Some(control_process_id(client_process_id, peer_process_id))
            .is_some_and(|process_id| session_is_held_by(session, process_id))
    };
    let secret_authorized = protected
        && !holder_authorized
        && session_control_authorized(session, peer_process_id, session_secret);
    if !holder_authorized && !secret_authorized {
        if protected {
            anyhow::bail!(
                "session {session_id} is protected and can only be changed by its current holder or session secret"
            );
        }
        anyhow::bail!("session {session_id} can only be changed by its current holder");
    }
    let Some(pane) = session.panes.iter().find(|pane| pane.id == pane_id) else {
        anyhow::bail!("session {session_id} has no pane {pane_id}");
    };
    #[cfg(windows)]
    daemon
        .pty_host
        .set_console_palette(pane.console_id, palette)
        .context("updating the pseudoconsole palette")?;
    #[cfg(not(windows))]
    let _ = (daemon, pane, palette);
    Ok(())
}

pub(super) fn kill(
    daemon: &Arc<Daemon>,
    session_id: u64,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        drop(sessions);
        #[cfg(feature = "session-persistence")]
        {
            let mut persistence = daemon.persistence.lock().unwrap();
            if persistence.as_ref().is_some_and(|persistence| {
                persistence
                    .records()
                    .iter()
                    .any(|record| record.id == session_id)
            }) {
                persistence
                    .as_mut()
                    .expect("the persistence store was checked above")
                    .forget(session_id)?;
                return Ok(());
            }
        }
        anyhow::bail!("session {session_id} does not exist");
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, session_secret),
        "session {session_id} is protected and can only be ended by its owner or current holder"
    );
    // Dropping the panes drops their PTYs, which hangs up the children.
    #[cfg(windows)]
    let consoles = session
        .panes
        .iter()
        .map(|pane| pane.console_id)
        .collect::<Vec<_>>();
    sessions.retain(|session| session.id != session_id);
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
        persistence.forget(session_id)?;
    }
    #[cfg(windows)]
    for console_id in consoles {
        close_host_console(daemon, console_id);
    }
    Ok(())
}

pub(super) fn forget(
    daemon: &Arc<Daemon>,
    session_id: u64,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    if let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) {
        anyhow::ensure!(
            session_control_authorized(session, peer_process_id, session_secret),
            "session {session_id} is protected and can only be forgotten by its owner or current holder"
        );
        #[cfg(windows)]
        let consoles = session
            .panes
            .iter()
            .map(|pane| pane.console_id)
            .collect::<Vec<_>>();
        sessions.retain(|session| session.id != session_id);
        drop(sessions);
        #[cfg(feature = "session-persistence")]
        if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
            persistence.forget(session_id)?;
        }
        #[cfg(windows)]
        for console_id in consoles {
            close_host_console(daemon, console_id);
        }
        return Ok(());
    }
    drop(sessions);

    #[cfg(feature = "session-persistence")]
    {
        let has_record = daemon
            .persistence
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|persistence| {
                persistence
                    .records()
                    .iter()
                    .any(|record| record.id == session_id)
            });
        if has_record {
            if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
                persistence.forget(session_id)?;
            }
            return Ok(());
        }
        let mut restored = daemon.restored.lock().unwrap();
        if restored
            .iter()
            .any(|session| session.request.record_id == session_id)
        {
            restored.retain(|session| session.request.record_id != session_id);
            return Ok(());
        }
    }

    anyhow::bail!("session {session_id} does not exist")
}

/// Reports what is known about each requested pane, in the order asked.
///
/// A pane the daemon is not holding is reported as `unknown` rather than
/// omitted: the caller is a client trying to find out whether it may stop
/// waiting, and "I have never heard of it" and "it is still running" must not
/// look the same to it.
pub(super) fn pane_states(
    daemon: &Arc<Daemon>,
    pane_ids: &[u64],
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Vec<crate::messages::PaneStateReport> {
    let mut sessions = daemon.sessions.lock().unwrap();
    pane_ids
        .iter()
        .map(|&pane_id| {
            let session_index = sessions
                .iter()
                .position(|session| session.panes.iter().any(|pane| pane.id == pane_id));
            if let Some(index) = session_index {
                let protected = {
                    let session = &mut sessions[index];
                    session.authentication.is_some()
                        && !session_control_authorized(session, peer_process_id, session_secret)
                };
                if protected {
                    return crate::messages::PaneStateReport {
                        pane_id,
                        unknown: true,
                        exited: false,
                        raw_status: None,
                        input_sent: false,
                    };
                }
                let pane = sessions[index].panes.iter().find(|pane| pane.id == pane_id);
                return match pane {
                    Some(pane) => crate::messages::PaneStateReport {
                        pane_id,
                        unknown: false,
                        exited: pane.exited,
                        raw_status: pane.exit_status,
                        input_sent: shared_input_sent(&pane.attachment),
                    },
                    None => unreachable!("the session index was selected by pane id"),
                };
            }
            crate::messages::PaneStateReport {
                pane_id,
                unknown: true,
                exited: false,
                raw_status: None,
                input_sent: false,
            }
        })
        .collect()
}

/// Hands a pane back because the client showing it has closed that pane.
///
/// Only the holder may do this, and only for the pane it holds: a client that
/// could release another's pane could stop that client's terminal from ever
/// being read again. Once released the pane is drained like any unheld one, and
/// a session that nobody asked to keep and that nobody is holding any more ends
/// with the window it belonged to — the same rule the liveness reclaim applies,
/// reached here promptly instead of on a two-second timer.
pub(super) fn close_pane(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Result<()> {
    let client_process_id = control_process_id(client_process_id, peer_process_id);
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, session_secret),
        "session {session_id} is protected and can only be changed by its owner or current holder"
    );
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        anyhow::bail!("session {session_id} has no pane {pane_id}");
    };
    match pane.attachment {
        Attachment::Exclusive(holder) | Attachment::Revoking { holder }
            if holder == client_process_id =>
        {
            pane.attachment = Attachment::None;
        }
        // A shared client leaves by closing its own data-plane connection,
        // which is what `remove_shared_client` acts on; releasing the whole
        // pane here would evict the other viewers too.
        _ => anyhow::bail!("client {client_process_id} does not hold pane {pane_id}"),
    }
    let (released, _consoles) = end_abandoned_sessions(&mut sessions);
    drop(sessions);
    #[cfg(windows)]
    for console_id in _consoles {
        close_host_console(daemon, console_id);
    }
    // An attach may have been waiting on this pane's revoke handover.
    daemon.sessions_condvar.notify_all();
    prune_exited_panes(daemon);
    publish(daemon);
    wake_drain(daemon);
    if released {
        log::debug!("ended a session whose last pane was closed with its window");
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "../tests/server/lifecycle.rs"]
mod tests;
