//! Reading a request off a connection and answering it.
//!
//! `serve` is the daemon's whole request surface: one arm per [`Request`],
//! each delegating to the module that owns that operation. A request is
//! authenticated before it is decoded, so an unauthenticated peer cannot reach
//! any of them.

use super::*;

/// Serves one connection. `deadline` bounds the handshake — the request, and
/// on Windows the attestation that may precede it — but not what follows: a
/// subscription or a shared stream stays open as long as its client does.
pub(super) fn serve(
    daemon: &Arc<Daemon>,
    stream: Stream,
    token: &str,
    deadline: Instant,
) -> Result<()> {
    // `_peer` lives until the request has been served: on Windows it holds the
    // peer process open, so its id cannot pass to another process meanwhile.
    let (connection, envelope, _peer, peer_process_id) =
        accept_request(daemon, stream, token, deadline)?;
    let Envelope {
        request,
        client_process_id,
        client_id,
        stream_only,
        session_secret,
        ..
    } = envelope;
    let caller = Caller {
        client_process_id,
        peer_process_id,
        client_id: &client_id,
        stream_only,
        session_secret: session_secret.as_deref(),
    };
    dispatch(daemon, request, caller, connection)
}

/// Who sent the request being dispatched: what its envelope declared, and the
/// peer process the transport (or an attestation) established.
#[derive(Clone, Copy)]
pub(super) struct Caller<'a> {
    pub(super) client_process_id: u32,
    pub(super) peer_process_id: Option<u32>,
    pub(super) client_id: &'a ClientId,
    pub(super) stream_only: bool,
    pub(super) session_secret: Option<&'a str>,
}

/// Reads the request off a new connection and establishes who sent it.
fn accept_request(
    #[cfg_attr(
        not(windows),
        expect(
            unused_variables,
            reason = "only Windows has to decide whether to attest a peer"
        )
    )]
    daemon: &Arc<Daemon>,
    stream: Stream,
    token: &str,
    deadline: Instant,
) -> Result<(Connection, Envelope, PeerProcess, Option<u32>)> {
    // A terminal must never cross a user boundary. The socket's permissions
    // already restrict this; checking the peer as well means a mistake in the
    // directory's access control is not on its own enough.
    let peer = PeerProcess::of(&stream)?;
    let peer_process_id = peer.id();

    let mut connection = Connection::new(stream);
    let mut envelope = read_request(&mut connection, token, deadline)?;

    // A client may ask to be identified before it sends the request that needs
    // it, which is how a request that streams raw bytes after its message gets
    // an identity: the daemon cannot interject a challenge there, because the
    // bytes would arrive where it was expecting the answer.
    #[cfg(windows)]
    let peer_process_id = if matches!(envelope.request, Request::Attest) {
        let attested = match peer_process_id {
            attested @ Some(_) => attested,
            None => {
                attest_peer(&mut connection, envelope.client_process_id).unwrap_or_else(|error| {
                    log::debug!("peer attestation failed: {error:#}");
                    None
                })
            }
        };
        connection.send(&Response::Ok)?;
        envelope = read_request(&mut connection, token, deadline)?;
        attested
    } else {
        peer_process_id
    };
    // Nothing to establish where the kernel already answered the question, but
    // the exchange still has to be answered so a portable client can ask.
    #[cfg(unix)]
    if matches!(envelope.request, Request::Attest) {
        connection.send(&Response::Ok)?;
        envelope = read_request(&mut connection, token, deadline)?;
    }

    // Where the kernel reports no peer (an older Windows), a request whose answer
    // depends on who is asking has to establish that first — see
    // `transport::PeerChallenge`. A peer that will not or cannot answer is not
    // refused here: it simply proceeds without an identity, and the checks that
    // needed one refuse it with their own message.
    #[cfg(windows)]
    let peer_process_id = match peer_process_id {
        attested @ Some(_) => attested,
        None if attestation_needed(daemon, &envelope.request, envelope.stream_only) => {
            match attest_peer(&mut connection, envelope.client_process_id) {
                Ok(attested) => attested,
                Err(error) => {
                    log::debug!("peer attestation failed: {error:#}");
                    None
                }
            }
        }
        None => None,
    };

    Ok((connection, envelope, peer, peer_process_id))
}

/// Answers a request whose handler reports failure as an error: the request
/// was refused, but the connection served it.
fn answer(result: Result<()>, connection: &mut Connection) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => refuse(connection, format!("{error:#}")),
    }
}

/// [`answer`] for a handler that leaves the `Ok` reply to its caller.
fn answer_ok(result: Result<()>, connection: &mut Connection) -> Result<()> {
    match result {
        Ok(()) => connection.send(&Response::Ok),
        Err(error) => refuse(connection, format!("{error:#}")),
    }
}

fn refuse(connection: &mut Connection, message: impl Into<String>) -> Result<()> {
    connection.send(&Response::Error {
        message: message.into(),
    })
}

/// Routes a request to its handler. Every arm is one call; a handler that needs
/// more than routing has a name of its own below.
fn dispatch(
    daemon: &Arc<Daemon>,
    request: Request,
    caller: Caller<'_>,
    mut connection: Connection,
) -> Result<()> {
    match request {
        Request::Ping => connection.send(&Response::Ok),
        Request::Subscribe => subscribe(daemon, caller, connection),
        Request::Spawn(request) => serve_spawn(daemon, request, caller, &mut connection),
        Request::SpawnShared(request) => {
            serve_spawn_shared(daemon, request, caller, &mut connection)
        }
        Request::SpawnSharedBatch(request) => answer(
            spawn_shared_batch(daemon, request, caller, &mut connection),
            &mut connection,
        ),
        Request::CreateShared(request) => answer(
            create_shared(daemon, request, caller.peer_process_id, &mut connection),
            &mut connection,
        ),
        Request::ApplyShared(request) => apply_shared(daemon, request, caller, &mut connection),
        Request::SharedSnapshot { session_id } => {
            shared_snapshot(daemon, session_id, caller, &mut connection)
        }
        Request::LeaveShared { session_id } => {
            leave_shared(daemon, session_id, caller, &mut connection)
        }
        Request::Attach(request) => attach(daemon, request, caller, &mut connection),
        // A screen checkpoint from the client showing the pane. Its own
        // connection is distinct from the shared connection; a revoke answer
        // also wakes the attach that started the handover.
        Request::Snapshot(_) if caller.stream_only => refuse(
            &mut connection,
            "stream-only clients cannot perform exclusive handover",
        ),
        Request::Snapshot(request) => snapshot(daemon, request, caller, &mut connection),
        Request::StoreImage(request) => answer(
            store_image(daemon, request, caller, &mut connection),
            &mut connection,
        ),
        Request::Input { .. } => refuse(&mut connection, "input belongs on a shared connection"),
        Request::Attested { .. } | Request::Attest => refuse(
            &mut connection,
            "an attestation precedes a request, it does not replace one",
        ),
        Request::Detach(_) if caller.stream_only => refuse(
            &mut connection,
            "stream-only clients cannot detach sessions",
        ),
        Request::Detach(request) => answer(
            detach(daemon, request, caller, &mut connection),
            &mut connection,
        ),
        Request::Resume(_) if caller.stream_only => refuse(
            &mut connection,
            "stream-only clients cannot resume disk sessions",
        ),
        Request::Resume(request) => answer(
            resume(daemon, *request, caller, &mut connection),
            &mut connection,
        ),
        Request::TakeExclusive {
            session_id,
            pane_id,
        } => take_exclusive(daemon, session_id, pane_id, caller, &mut connection),
        Request::ReleaseExclusive {
            session_id,
            pane_id,
        } => release_exclusive(daemon, session_id, pane_id, caller, &mut connection),
        Request::Share(request) => share(daemon, request, caller, &mut connection),
        Request::Resize {
            session_id,
            pane_id,
            revision,
            columns,
            lines,
        } => answer_ok(
            resize_pane(
                daemon,
                ResizePaneContext {
                    session_id,
                    pane_id,
                    revision,
                    columns,
                    lines,
                    peer_process_id: caller.peer_process_id,
                    session_secret: caller.session_secret,
                },
            ),
            &mut connection,
        ),
        Request::SetConsolePalette {
            session_id,
            pane_id,
            palette,
        } => answer_ok(
            set_console_palette(daemon, session_id, pane_id, palette, caller),
            &mut connection,
        ),
        Request::List => list_sessions(daemon, caller.stream_only, &mut connection),
        Request::PaneStates { pane_ids } => {
            serve_pane_states(daemon, &pane_ids, caller, &mut connection)
        }
        Request::ClosePane { .. } if caller.stream_only => refuse(
            &mut connection,
            "stream-only clients release panes by closing the shared connection",
        ),
        Request::ClosePane {
            session_id,
            pane_id,
        } => answer_ok(
            close_pane(daemon, session_id, pane_id, caller),
            &mut connection,
        ),
        Request::DiscardPane { .. } if caller.stream_only => Err(anyhow::anyhow!(
            "stream-only clients close panes through shared collaboration"
        )),
        Request::DiscardPane {
            session_id,
            pane_id,
        } => answer_ok(
            discard_pane(daemon, session_id, pane_id, caller),
            &mut connection,
        ),
        Request::Kill { session_id } => answer_ok(
            kill(
                daemon,
                session_id,
                caller.peer_process_id,
                caller.session_secret,
            )
            .map(|()| publish(daemon)),
            &mut connection,
        ),
        Request::SetSessionScope(request) => answer(
            set_session_scope(daemon, request, caller, &mut connection),
            &mut connection,
        ),
        Request::Forget { session_id } => answer_ok(
            forget(
                daemon,
                session_id,
                caller.peer_process_id,
                caller.session_secret,
            )
            .map(|()| publish(daemon)),
            &mut connection,
        ),
        Request::Configure {
            retention,
            persistence_recipients,
        } if !caller.stream_only => answer_ok(
            configure_daemon(
                daemon,
                retention,
                persistence_recipients,
                caller.peer_process_id,
            ),
            &mut connection,
        ),
        Request::Upgrade if !caller.stream_only => serve_upgrade(daemon, &mut connection),
        Request::Shutdown if !caller.stream_only => shutdown(daemon, &mut connection),
        Request::Configure { .. } | Request::Upgrade | Request::Shutdown => {
            refuse(&mut connection, "this request is local-only")
        }
    }
}

fn serve_pane_states(
    daemon: &Arc<Daemon>,
    pane_ids: &[u64],
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let panes = pane_states(
        daemon,
        pane_ids,
        caller.peer_process_id,
        caller.session_secret,
    );
    connection.send(&Response::PaneStates { panes })
}

fn subscribe(daemon: &Arc<Daemon>, caller: Caller<'_>, connection: Connection) -> Result<()> {
    // Keyed by process so a revoke can be sent to the one client that
    // holds a pane rather than broadcast to every subscriber. A client
    // that subscribes twice is the same process, so the later
    // connection replaces the earlier one.
    let relay = SubscriberRelay::new(daemon, caller.client_id.clone(), connection);
    daemon.subscribers.lock().recover_poison().insert(
        caller.client_id.clone(),
        Subscriber {
            process_id: caller.client_process_id,
            relay: Arc::clone(&relay),
        },
    );
    // Only now, with the handle published: the reply is what tells the
    // client that an event broadcast after it acts has somewhere to go.
    // Delivery starts after it so the acknowledgement cannot be
    // interleaved with the first event.
    let acknowledged = relay.acknowledge();
    relay.start(daemon);
    acknowledged
}

fn spawn_context<'a>(
    caller: Caller<'a>,
    shared_request: Option<crate::messages::SharedSpawnRequest>,
    connection: &'a mut Connection,
) -> SpawnContext<'a> {
    SpawnContext {
        client_process_id: caller.client_process_id,
        peer_process_id: caller.peer_process_id,
        client_id: caller.client_id.clone(),
        stream_only: caller.stream_only,
        session_secret: caller.session_secret,
        shared_request,
        connection,
    }
}

/// Unlike every other request, a failed spawn is also returned as an error
/// after it has been answered, so the connection worker logs it.
fn serve_spawn(
    daemon: &Arc<Daemon>,
    request: SpawnRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    if caller.stream_only {
        return refuse(connection, "stream-only clients cannot spawn panes");
    }
    match spawn(daemon, request, spawn_context(caller, None, connection)) {
        Ok(SpawnOutcome::Complete) => Ok(()),
        Ok(SpawnOutcome::SharedConflict(state)) => {
            connection.send(&Response::SharedConflict { state: *state })
        }
        Err(error) => {
            let _ = refuse(connection, format!("{error:#}"));
            Err(error)
        }
    }
}

fn serve_spawn_shared(
    daemon: &Arc<Daemon>,
    shared_request: crate::messages::SharedSpawnRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let request = SpawnRequest {
        session_id: Some(shared_request.session_id),
        client_process_id: caller.client_process_id,
        program: shared_request.program.clone(),
        args: shared_request.args.clone(),
        env: shared_request.env.clone(),
        working_directory: shared_request.working_directory.clone(),
        size: shared_request.size,
        console_palette: shared_request.console_palette,
        console_backend: shared_request.console_backend,
    };
    match spawn(
        daemon,
        request,
        spawn_context(caller, Some(shared_request), connection),
    ) {
        Ok(SpawnOutcome::Complete) => Ok(()),
        Ok(SpawnOutcome::SharedConflict(state)) => {
            connection.send(&Response::SharedConflict { state: *state })
        }
        Err(error) => refuse(connection, format!("{error:#}")),
    }
}

fn list_sessions(
    daemon: &Arc<Daemon>,
    stream_only: bool,
    connection: &mut Connection,
) -> Result<()> {
    // Sanitized exactly as the published catalog is. The endpoint token
    // authenticates the channel, not a session: listing must not reveal
    // the commands, titles or directories of a session whose whole
    // point is that they stay private until its secret is presented.
    let sessions: Vec<_> = daemon
        .sessions()
        .iter()
        .filter(|session| session.is_available() && (!stream_only || session.offered))
        .map(|session| catalog_summary(session).for_public_catalog())
        .collect();
    #[cfg(feature = "session-persistence")]
    let live_session_ids = sessions
        .iter()
        .map(|session| session.id)
        .collect::<std::collections::HashSet<_>>();
    #[cfg(feature = "session-persistence")]
    let mut restorable: Vec<crate::protocol::RestorableSessionRecord> = daemon
        .persistence
        .lock()
        .as_ref()
        .map(|persistence| {
            persistence
                .records()
                .iter()
                .filter(|record| record.restorable && !live_session_ids.contains(&record.id))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    #[cfg(feature = "session-persistence")]
    for restored in daemon.restored.lock().recover_poison().iter() {
        restorable.push(crate::protocol::RestorableSessionRecord {
            id: restored.request.record_id,
            created_at: restored.restored_at,
            updated_at: restored.restored_at,
            metadata_bytes: 0,
            snapshot_bytes: 0,
            scrollback_bytes: 0,
            protected: restored.request.verifier.is_some(),
            auto_protected: restored.request.key_envelope.is_some(),
            restorable: false,
        });
    }
    #[cfg(not(feature = "session-persistence"))]
    let restorable = Vec::new();
    connection.send(&Response::Sessions {
        sessions,
        restorable,
    })
}

fn serve_upgrade(daemon: &Arc<Daemon>, connection: &mut Connection) -> Result<()> {
    #[cfg(any(unix, windows))]
    {
        // Only returns when the replacement was refused: a successful
        // upgrade never comes back, because this process becomes it.
        match upgrade_daemon(daemon, connection) {
            Ok(()) => Ok(()),
            // The reply is only reported here while the upgrade could
            // still be refused. Past the point of no return the request
            // has already been answered, and writing a second reply onto
            // the same connection would leave the client parsing one
            // message as the tail of another.
            Err(UpgradeRefused::Before(error)) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
            Err(UpgradeRefused::AfterAnswering(error)) => {
                log::error!("the multiplexer could not replace itself: {error:#}");
                Ok(())
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    connection.send(&Response::Error {
        message: "replacing the multiplexer in place is not supported on this platform; \
                  a pseudoconsole cannot be moved between processes"
            .to_owned(),
    })
}

fn shutdown(daemon: &Arc<Daemon>, connection: &mut Connection) -> Result<()> {
    let held = daemon.sessions().len();
    if held > 0 {
        // Answered rather than ignored. Replying `Ok` to a request that
        // was not honoured left the caller to guess, and what it guessed
        // was that the multiplexer had stopped.
        return connection.send(&Response::Error {
            message: format!(
                "the multiplexer is holding {held} session{}",
                if held == 1 { "" } else { "s" }
            ),
        });
    }
    // Ask the separate Windows host to exit before acknowledging the
    // request. The reply must be sent before `running` is cleared:
    // once the accept loop is woken, this process can finish while
    // this connection worker is still running, which would close the
    // socket before the caller received the successful stop.
    #[cfg(windows)]
    if let Err(error) = daemon.pty_host.shutdown() {
        return connection.send(&Response::Error {
            message: format!("could not stop the pseudoconsole host: {error:#}"),
        });
    }
    let response = connection.send(&Response::Ok);
    daemon.running.store(false, Ordering::SeqCst);
    // Unblock the accept loop so it observes the flag.
    let _ = Stream::connect(socket_path(&session_catalog_dir()));
    response
}

/// Applies settings from a client that may have been started after this
/// daemon. A daemon deliberately outlives its clients, so treating its startup
/// arguments as permanent makes editing the configuration appear to work while
/// leaving all subsequent sessions under the old retention policy.
#[cfg_attr(
    not(feature = "session-persistence"),
    expect(
        unused_variables,
        reason = "recipients and the requesting process matter only to a disk store"
    )
)]
pub(super) fn configure_daemon(
    daemon: &Arc<Daemon>,
    retention: Retention,
    persistence_recipients: Vec<String>,
    peer_process_id: Option<u32>,
) -> Result<()> {
    retention.validate()?;
    // Resolved before anything changes, so what a session is pinned to is
    // exactly what the store will be opened with.
    #[cfg(feature = "session-persistence")]
    let persistence_recipients = if matches!(retention, Retention::Disk) {
        crate::persistence::resolve_recipient_strings(&persistence_recipients)?
    } else {
        Vec::new()
    };

    let old_retention = *daemon.retention.lock().recover_poison();
    let mut sessions = daemon.sessions();
    if old_retention != retention {
        for session in sessions.iter_mut() {
            for pane in &mut session.panes {
                let snapshot = pane.retained.snapshot();
                let mut retained = retention.new_retained(pane.size.columns, pane.size.lines);
                retained.seed(snapshot);
                pane.retained = retained;
            }
        }
    }
    // Before anything is written under the new recipients: a protected session
    // this peer neither owns nor holds keeps the recipients it had.
    #[cfg(feature = "session-persistence")]
    {
        configure_seals(
            daemon,
            &mut sessions,
            peer_process_id,
            &persistence_recipients,
        );
        // Still under the session lock, so no session is protected between
        // being sealed in the new store and the store being installed.
        install_configured_store(daemon, &sessions, retention, &persistence_recipients)?;
    }
    drop(sessions);
    *daemon.retention.lock().recover_poison() = retention;
    wake_drain(daemon);
    publish(daemon);
    Ok(())
}

/// Opens the store a `Configure` selected, writes the kept and offered
/// sessions to it, and installs it. The caller holds the session lock.
#[cfg(feature = "session-persistence")]
fn install_configured_store(
    daemon: &Daemon,
    sessions: &[Session],
    retention: Retention,
    persistence_recipients: &[String],
) -> Result<()> {
    let mut next_persistence = {
        let mut persistence = daemon.persistence.lock();
        if let Some(persistence) = persistence.as_mut() {
            persistence
                .flush_segments()
                .context("flushing encrypted scrollback before changing retention")?;
        }
        if matches!(retention, Retention::Disk) && !persistence_recipients.is_empty() {
            // Recover the store into a local handle first. Live sessions are
            // rewritten as non-restorable below before the handle is exposed
            // through `daemon.persistence`, while records this memory-fallback
            // daemon does not own must become recoverable now that disk mode is
            // available again.
            PersistenceStore::open_with_recovery_state(
                &session_catalog_dir(),
                Some(persistence_recipients),
                false,
            )?
        } else {
            // Keep an existing disk catalog visible while a client has
            // temporarily selected memory mode (for example because a GitHub
            // recipient could not be resolved). This handle is read/cleanup
            // only; the enabled flag below prevents new memory sessions from
            // being written to it.
            PersistenceStore::open_with_recovery_state(&session_catalog_dir(), None, false)?
        }
    };
    if let Some(persistence) = next_persistence.as_mut() {
        // The new store starts knowing nothing, and withholds what it does not
        // know; it is told about every session before anything reaches it.
        reseal_all(persistence, sessions);
        if matches!(retention, Retention::Disk) {
            for session in sessions
                .iter()
                .filter(|session| session.keep || session.offered)
            {
                persistence.save_session(&persisted_live_session(session))?;
            }
        }
    }
    let mut persistence = daemon.persistence.lock();
    let persistence_enabled = matches!(retention, Retention::Disk)
        && !persistence_recipients.is_empty()
        && next_persistence.is_some();
    // Output the drain read after the flush above was still applied to
    // the outgoing store, and would be lost with it.
    if let Some(previous) = persistence.as_mut()
        && let Err(error) = previous.flush_segments()
    {
        log::warn!("could not flush encrypted scrollback before changing retention: {error:#}");
    }
    *persistence = next_persistence;
    daemon
        .persistence
        .set_enabled(&mut persistence, persistence_enabled);
    Ok(())
}
