//! Spawning a pane: into a new session, a live one, a restored one, or a
//! shared one. Each phase below runs in the order `spawn` calls it; the child
//! starts with no lock held, and everything decided before that is checked
//! again under the sessions lock before the pane is recorded.

use super::*;

pub(in crate::server) struct SpawnContext<'a> {
    pub(in crate::server) client_process_id: u32,
    pub(in crate::server) peer_process_id: Option<u32>,
    pub(in crate::server) client_id: ClientId,
    pub(in crate::server) stream_only: bool,
    pub(in crate::server) session_secret: Option<&'a str>,
    pub(in crate::server) shared_request: Option<crate::messages::SharedSpawnRequest>,
    pub(in crate::server) connection: &'a mut Connection,
}

pub(in crate::server) fn spawn(
    daemon: &Arc<Daemon>,
    mut request: SpawnRequest,
    context: SpawnContext<'_>,
) -> Result<SpawnOutcome> {
    let SpawnContext {
        client_process_id,
        peer_process_id,
        client_id,
        stream_only,
        session_secret,
        shared_request,
        connection,
    } = context;
    #[cfg(not(feature = "session-persistence"))]
    let _ = (client_process_id, peer_process_id);

    if let Some(shared_request) = &shared_request
        && let Some(conflict) =
            check_shared_spawn(daemon, shared_request, peer_process_id, session_secret)?
    {
        return Ok(conflict);
    }

    let requested_program = request.program.clone();
    let requested_working_directory = request.working_directory.clone();

    // Keep the lease locked from authorization through the descriptor handoff.
    // Two panes may be spawned concurrently during restore, but only the first
    // successful handoff may materialize the saved session. A failed handoff
    // leaves the lease in place for the next attempt.
    #[cfg(feature = "session-persistence")]
    let restored_guard = request
        .session_id
        .map(|_| daemon.restored.lock().recover_poison());
    #[cfg(feature = "session-persistence")]
    let restored = restore_lease(
        restored_guard.as_deref(),
        request.session_id,
        peer_process_id,
    )?;
    #[cfg(not(feature = "session-persistence"))]
    let restored: Option<()> = None;

    let live_proof = authorize_live_spawn(
        daemon,
        &request,
        shared_request.is_some(),
        peer_process_id,
        session_secret,
        restored.is_some(),
    )?;

    let started = start_pane_process(daemon, &mut request)?;
    let child_pid = started.child_pid;
    #[cfg(windows)]
    let console_id = started.console_id;
    let pane_id = daemon.next_pane_id.fetch_add(1, Ordering::Relaxed);
    let retention = *daemon.retention.lock().recover_poison();

    let mut sessions = daemon.sessions();
    // `session.panes.len()` is the zero-based index of the pane being added
    // until the pane is pushed below. Keep it before the push so the first
    // restored shell receives snapshot zero rather than being shifted past it.
    let joined = request
        .session_id
        .and_then(|id| sessions.iter().find(|session| session.id == id));
    #[cfg(feature = "session-persistence")]
    let restored_snapshot_index = joined.map_or(0, |session| session.panes.len());
    let joins_live_session = joined.is_some();
    let session_id = session_for_spawn(
        daemon,
        &mut sessions,
        &request,
        joins_live_session,
        #[cfg(feature = "session-persistence")]
        restored,
        pane_id,
        client_process_id,
    )?;
    let session = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .expect("the session was just located or created");
    let recheck = recheck_before_commit(
        daemon,
        session,
        shared_request.as_ref(),
        joins_live_session,
        peer_process_id,
        &live_proof,
    );
    if !matches!(recheck, Ok(None)) {
        // Nothing names the pane yet, so the child goes with the refusal
        // rather than becoming a daemon-owned process nobody can reach.
        drop(sessions);
        drop(started);
        #[cfg(windows)]
        let _ = daemon.pty_host.close(console_id);
        return recheck.map(|conflict| conflict.expect("only a conflict reaches here"));
    }

    // Record the pane before answering. A handover that fails after the
    // session exists but before the pane does would otherwise leave a session
    // holding nothing: unattachable, and — because "every pane is unattached"
    // is vacuously true of no panes — liable to be published as though the
    // user could pick it up.
    let retained = retention.new_retained(request.size.columns, request.size.lines);
    #[cfg(feature = "session-persistence")]
    let retained = seed_restored_output(retained, restored, restored_snapshot_index);
    let attachment = if shared_request.is_some() {
        Attachment::Shared(Vec::new())
    } else {
        exclusive_attachment(request.client_process_id)
    };
    let attachment_client_id =
        (shared_request.is_none() && client_process_id != 0).then(|| client_id.clone());
    session.panes.push(spawned_pane(
        pane_id,
        started,
        attachment,
        attachment_client_id,
        request.size,
        retained,
    ));

    if let Some(shared_request) = shared_request {
        let response_state = record_shared_pane(
            session,
            pane_id,
            requested_program,
            requested_working_directory,
            shared_request.operation_id,
        );
        let response_summary = Box::new(session.summary.clone());
        announce_shared_pane(daemon, &response_state, pane_id, child_pid, &client_id);
        // A shared spawn serves its connection for the lifetime of the pane.
        // Do not retain the restored-session lease while doing that: a second
        // shared mutation would otherwise wait forever for this connection to
        // leave, even though the lease is irrelevant once the live session was
        // found above.
        #[cfg(feature = "session-persistence")]
        drop(restored_guard);
        let attacher = Attacher {
            client_process_id,
            peer_process_id,
            client_id,
            stream_only,
            // A spawn is the client asking for a pane of its own, never a
            // relay standing in for somebody else's view of one.
            relaying_for: None,
        };
        return attach_shared(
            daemon,
            sessions,
            session_id,
            pane_id,
            attacher,
            response_state.state.clone(),
            response_summary,
            connection,
            Some(response_state),
        )
        .map(|()| SpawnOutcome::Complete);
    }
    let handover = hand_over_spawned_pane(
        daemon,
        sessions,
        session_id,
        pane_id,
        child_pid,
        request.client_process_id,
        connection,
    );
    if let Err(error) = handover {
        discard_unhanded_pane(daemon, session_id, pane_id);
        #[cfg(windows)]
        let _ = daemon.pty_host.close(console_id);
        return Err(error);
    }
    #[cfg(feature = "session-persistence")]
    if let Some(session_id) = request.session_id
        && restored.is_some()
        && let Some(restored_guard) = restored_guard
    {
        count_restored_spawn(restored_guard, session_id);
    }
    Ok(SpawnOutcome::Complete)
}

/// A restored pane's scrollback starts from the snapshot saved at its position
/// in the session, so the first restored shell gets snapshot zero.
#[cfg(feature = "session-persistence")]
fn seed_restored_output(
    mut retained: crate::retention::Retained,
    restored: Option<&RestoredSession>,
    snapshot_index: usize,
) -> crate::retention::Retained {
    if let Some(snapshot) = restored.and_then(|restored| restored.snapshots.get(snapshot_index)) {
        retained.seed(snapshot.bytes.clone());
    }
    retained
}

/// The authorization half of a shared spawn, checked before the child starts
/// and with no lock held while the secret is proved. `Some` is the conflict
/// the caller answers with when the client's view of the session is stale.
pub(super) fn check_shared_spawn(
    daemon: &Arc<Daemon>,
    shared_request: &crate::messages::SharedSpawnRequest,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Result<Option<SpawnOutcome>> {
    let sessions = daemon.sessions();
    let Some(session) = sessions
        .iter()
        .find(|session| session.id == shared_request.session_id)
    else {
        anyhow::bail!("session {} does not exist", shared_request.session_id);
    };
    anyhow::ensure!(
        session.offered,
        "session {} is not offered for shared collaboration",
        shared_request.session_id
    );
    // Check the authorization without consuming the lock needed by the
    // actual spawn. The revision is checked again immediately before the
    // new pane is committed, closing the race with another mutation. The
    // secret is proved in the gap, with no lock held, so the session may
    // have ended by the time it is found again.
    drop(sessions);
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == shared_request.session_id
    });
    let mut sessions = daemon.sessions();
    let Some(session) = sessions
        .iter_mut()
        .find(|session| session.id == shared_request.session_id)
    else {
        anyhow::bail!("session {} does not exist", shared_request.session_id);
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, &proof),
        "shared session {} is not authorized for this client",
        shared_request.session_id
    );
    let state = normalize_shared_state(daemon, session)?;
    if state.revision != shared_request.base_revision {
        return Ok(Some(SpawnOutcome::SharedConflict(Box::new(state))));
    }
    Ok(None)
}

/// The proof a plain spawn into a live session must carry, checked against
/// the session as it is now. `restored` is whether a restore lease stands in
/// for a session that is not live yet.
pub(super) fn authorize_live_spawn(
    daemon: &Arc<Daemon>,
    request: &SpawnRequest,
    shared: bool,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
    restored: bool,
) -> Result<SecretProof> {
    // A plain spawn into a live session adds a pane running whatever the caller
    // names, and that pane keeps the session alive: for a protected one it is
    // the same control `SpawnShared` checks above, so it takes the same proof.
    // Proved now, with no lock held, and checked both before the child starts
    // and again where the pane is committed, in case the session was protected
    // in between.
    let live_proof = match (request.session_id, shared) {
        (Some(session_id), false) => {
            prove_session_secret(daemon, peer_process_id, session_secret, |session| {
                session.id == session_id
            })
        }
        _ => SecretProof::NONE,
    };
    if let Some(session_id) = request.session_id {
        let sessions = daemon.sessions();
        let live = sessions.iter().find(|session| session.id == session_id);
        anyhow::ensure!(
            live.is_some() || restored,
            "session {session_id} does not exist"
        );
        if !shared && let Some(session) = live {
            anyhow::ensure!(
                session_control_authorized(session, peer_process_id, &live_proof),
                "session {session_id} is not authorized for this client"
            );
        }
    }
    Ok(live_proof)
}

/// Checks a spawn again under the sessions lock, now that its child has
/// started: the session may have been protected, or a shared session changed,
/// in between. `Some` is the conflict a shared spawn answers with.
pub(super) fn recheck_before_commit(
    daemon: &Arc<Daemon>,
    session: &mut Session,
    shared_request: Option<&crate::messages::SharedSpawnRequest>,
    joins_live_session: bool,
    peer_process_id: Option<u32>,
    live_proof: &SecretProof,
) -> Result<Option<SpawnOutcome>> {
    let session_id = session.id;
    let Some(shared_request) = shared_request else {
        anyhow::ensure!(
            !joins_live_session || session_control_authorized(session, peer_process_id, live_proof),
            "session {session_id} is not authorized for this client"
        );
        return Ok(None);
    };
    anyhow::ensure!(
        session.offered,
        "session {session_id} is not offered for shared collaboration"
    );
    let state = normalize_shared_state(daemon, session)?;
    // The revision can change while the daemon is starting the child.
    Ok((state.revision != shared_request.base_revision)
        .then(|| SpawnOutcome::SharedConflict(Box::new(state))))
}

/// The record of a pane the daemon has just started.
pub(super) fn spawned_pane(
    pane_id: u64,
    started: StartedPane,
    attachment: Attachment,
    attachment_client_id: Option<ClientId>,
    size: TerminalSize,
    retained: crate::retention::Retained,
) -> Pane {
    Pane {
        id: pane_id,
        pty: started.pty,
        #[cfg(windows)]
        console_id: started.console_id,
        #[cfg(windows)]
        child_events: started.child_events,
        attachment,
        attachment_client_id,
        size,
        retained,
        handed_over: None,
        handover_waiters: 0,
        exited: false,
        exit_status: None,
        pending_input: Vec::new(),
        clipboard_scanner: zclip::protocol::Scanner::default(),
    }
}

/// Tells every other viewer of a shared session about a pane one of them
/// added. The requester receives the shared data stream on its own request;
/// everyone else learns the new stable pane id here and opens a data stream of
/// their own, so one client's connection is never duplicated or passed through
/// the daemon.
pub(super) fn announce_shared_pane(
    daemon: &Arc<Daemon>,
    state: &crate::messages::SharedSessionState,
    pane_id: u64,
    child_pid: u32,
    requester: &ClientId,
) {
    broadcast_except(
        daemon,
        &Event::SharedPaneAdded {
            session_id: state.session_id,
            pane_id,
            child_pid,
            state: state.clone(),
        },
        requester,
    );
}

/// The restore lease a spawn into `session_id` materializes, if there is one,
/// checked against the peer that resumed it.
#[cfg(feature = "session-persistence")]
pub(super) fn restore_lease(
    leases: Option<&Vec<RestoredSession>>,
    session_id: Option<u64>,
    peer_process_id: Option<u32>,
) -> Result<Option<&RestoredSession>> {
    let restored = session_id.and_then(|session_id| {
        leases?
            .iter()
            .find(|restored| restored.request.record_id == session_id)
    });
    if let Some(restored) = restored {
        anyhow::ensure!(
            restored.request.verifier.is_none() || restored.authorized_peer == peer_process_id,
            "protected restored session can only be spawned by the peer that resumed it"
        );
    }
    Ok(restored)
}

/// Sends the requester the terminal of the pane just recorded in
/// `session_id`, releasing the sessions lock before the write.
pub(super) fn hand_over_spawned_pane(
    daemon: &Arc<Daemon>,
    sessions: MutexGuard<'_, Vec<Session>>,
    session_id: u64,
    pane_id: u64,
    child_pid: u32,
    client_process_id: u32,
    connection: &mut Connection,
) -> Result<()> {
    let pane = sessions
        .iter()
        .find(|session| session.id == session_id)
        .and_then(|session| session.panes.iter().find(|pane| pane.id == pane_id))
        .expect("the pane was just pushed");
    let handles = handover_handles(daemon, pane, client_process_id);
    // The reply travels on the connection that asked for it, which may be
    // arbitrarily slow to read — a burst of concurrent spawns is exactly when
    // that matters. Holding the sessions lock across that write would
    // serialize every other session-mutating request behind this one
    // client's socket; `handover_handles`'s borrowed descriptor stays valid
    // regardless, because nothing removes this pane except the rollback
    // below, which re-locks first.
    drop(sessions);
    handles.and_then(|handles| {
        connection.send_with(
            &Response::Spawned {
                session_id,
                pane_id,
                child_pid,
                handles: handles.values,
            },
            &handles.attachments,
        )
    })
}

/// The session a spawned pane joins: the live one the request names, the one
/// its restore lease materializes, or a new one owned by the spawning window.
pub(super) fn session_for_spawn(
    daemon: &Arc<Daemon>,
    sessions: &mut Vec<Session>,
    request: &SpawnRequest,
    joins_live_session: bool,
    #[cfg(feature = "session-persistence")] restored: Option<&RestoredSession>,
    pane_id: u64,
    client_process_id: u32,
) -> Result<u64> {
    #[cfg(not(feature = "session-persistence"))]
    let _ = client_process_id;
    Ok(match request.session_id {
        Some(id) if joins_live_session => id,
        #[cfg(feature = "session-persistence")]
        Some(id) if restored.is_some() => {
            let restored = restored.expect("restore lease was checked");
            sessions.push(restored_session(id, restored, client_process_id)?);
            let restored_session = sessions.last_mut().expect("the session was just pushed");
            pin_protected(daemon, restored_session, restored.authorized_peer);
            reseal(daemon, restored_session);
            id
        }
        _ => {
            let id = daemon.next_session_id.fetch_add(1, Ordering::Relaxed);
            sessions.push(spawned_session(id, pane_id, request.client_process_id));
            #[cfg(feature = "session-persistence")]
            reseal(
                daemon,
                sessions.last().expect("the session was just pushed"),
            );
            id
        }
    })
}

/// A pane's process, started but not yet recorded in any session.
pub(super) struct StartedPane {
    pty: tty::Pty,
    child_pid: u32,
    #[cfg(windows)]
    console_id: u64,
    #[cfg(windows)]
    child_events: tty::AttachedChildEvents,
}

/// Starts the process `request` names, taking its program, arguments and
/// environment out of it.
pub(super) fn start_pane_process(
    daemon: &Arc<Daemon>,
    request: &mut SpawnRequest,
) -> Result<StartedPane> {
    let start_directory = pane_start_directory(request.working_directory.take());
    #[cfg(unix)]
    let options = tty::Options {
        shell: request
            .program
            .take()
            .map(|program| tty::Shell::new(program, std::mem::take(&mut request.args))),
        working_directory: start_directory.clone(),
        drain_on_exit: true,
        env: std::mem::take(&mut request.env),
        #[cfg(not(windows))]
        child_signal_mask: None,
        // Arguments are passed through as the client resolved them, so the
        // multiplexer must not re-quote what has already been quoted.
        #[cfg(windows)]
        escape_args: false,
    };
    #[cfg(unix)]
    let pty =
        start_terminal(daemon, &options, request.size).context("starting the terminal process")?;
    #[cfg(unix)]
    let child_pid = pty.child_pid();
    #[cfg(windows)]
    let (console_id, child_pid, pty, child_events) = {
        let (console_id, child_pid, handles) =
            daemon.pty_host.open(crate::pty_host::OpenConsoleRequest {
                program: request.program.take(),
                args: std::mem::take(&mut request.args),
                env: std::mem::take(&mut request.env),
                working_directory: start_directory,
                size: request.size,
                palette: request.console_palette,
                console_backend: request.console_backend,
                target_process_id: std::process::id(),
            })?;
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
    Ok(StartedPane {
        pty,
        child_pid,
        #[cfg(windows)]
        console_id,
        #[cfg(windows)]
        child_events,
    })
}

/// The session a restore lease describes, materialized by its first pane.
#[cfg(feature = "session-persistence")]
pub(super) fn restored_session(
    id: u64,
    restored: &RestoredSession,
    client_process_id: u32,
) -> Result<Session> {
    let mut summary = restored.request.summary.clone();
    summary.id = id;
    let authentication = restored
        .request
        .verifier
        .clone()
        .map(SessionAuthentication::from_verifier)
        .transpose()?;
    Ok(Session {
        id,
        summary,
        state: restored.request.state.clone(),
        shared_state: restored.request.shared_state.clone(),
        authentication,
        key_envelope: restored.request.key_envelope.clone(),
        failed_authentications: restored.request.failed_authentications,
        refuse_until: (restored.request.backoff_seconds > 0)
            .then(|| Instant::now() + Duration::from_secs(restored.request.backoff_seconds)),
        panes: Vec::new(),
        keep: false,
        offered: restored.request.shared_state.is_some(),
        owner: Some(ProcessIdentity::of(client_process_id)),
        sealed_to: None,
    })
}

/// A new session holding nothing yet, owned by the window that spawned it.
pub(super) fn spawned_session(id: u64, pane_id: u64, client_process_id: u32) -> Session {
    Session {
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
        shared_state: None,
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
        owner: Some(ProcessIdentity::of(client_process_id)),
        sealed_to: None,
    }
}

/// Adds a spawned pane to a shared session's state, splitting it beside the
/// existing layout, and returns the state every viewer is sent.
pub(super) fn record_shared_pane(
    session: &mut Session,
    pane_id: u64,
    application: Option<String>,
    working_directory: Option<PathBuf>,
    operation_id: crate::messages::SharedOperationId,
) -> crate::messages::SharedSessionState {
    let mut state = session.shared_state.clone().unwrap_or_else(|| {
        crate::messages::SharedSessionState::new(
            session.id,
            session.summary.clone(),
            session.state.clone(),
        )
    });
    let mut summary = state.summary.clone();
    summary.panes.push(crate::protocol::BackgroundPaneSummary {
        id: pane_id,
        label: format!("pane-{pane_id}"),
        profile: String::new(),
        configured_command: String::new(),
        application: application.unwrap_or_default(),
        foreground_command: None,
        terminal_title: None,
        working_directory,
        state: crate::protocol::BackgroundPaneState::Running,
        exit: None,
    });
    summary.layout = BackgroundPaneLayout::Split {
        axis: "horizontal".to_owned(),
        first_ratio: crate::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
        first: Box::new(summary.layout),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id }),
    };
    summary.active_pane = pane_id;
    state.summary = summary;
    state.presentation.layout = state.summary.layout.clone();
    state.presentation.active_pane = pane_id;
    state.revision = state.revision.next();
    state.last_operation_id = Some(operation_id.clone());
    state.record_receipt(crate::messages::SharedOperationReceipt {
        operation_id,
        draft_mappings: Vec::new(),
    });
    clear_shared_size_reports(session);
    session.summary = state.summary.clone();
    session.state = state.state.clone();
    session.shared_state = Some(state.clone());
    state
}

/// Drops a pane whose handover failed, and its session if that leaves it
/// empty. Nobody received this terminal, so nothing can ever read it.
pub(super) fn discard_unhanded_pane(daemon: &Arc<Daemon>, session_id: u64, pane_id: u64) {
    // Nobody received this terminal, so nothing can ever read it.
    let mut sessions = daemon.sessions();
    let discarded = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .and_then(|session| {
            session
                .panes
                .iter()
                .position(|pane| pane.id == pane_id)
                .map(|index| session.panes.remove(index))
        });
    sessions.retain(|session| !session.panes.is_empty());
    drop(sessions);
    drop(discarded);
    remove_pane_agent_links(pane_id);
}

/// Counts a successful handoff against the restore lease, releasing the lease
/// once every pane it describes has been spawned.
#[cfg(feature = "session-persistence")]
pub(super) fn count_restored_spawn(
    mut leases: MutexGuard<'_, Vec<RestoredSession>>,
    session_id: u64,
) {
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
