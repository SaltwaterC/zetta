//! Sharing an existing session: offering it to other clients, applying a
//! viewer's layout, leaving it, and changing who it is scoped to.

use super::*;

/// Offers, or withdraws, a session its client is still showing.
///
/// Everything a joining client needs is published here, exactly as a detach
/// publishes it: the summary the catalog lists and the state a joining client
/// rebuilds its tab from. The client checkpoints each exclusive pane with
/// [`Request::Snapshot`] immediately beforehand. What is *not* done is the rest
/// of detaching — no attachment is released and `keep` is left alone — so the
/// session carries on being displayed by the window that shared it.
pub(in crate::server) fn share(
    daemon: &Arc<Daemon>,
    request: crate::messages::ShareRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        client_process_id,
        peer_process_id,
        session_secret,
        ..
    } = caller;
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == request.session_id
    });
    let mut sessions = daemon.sessions();
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
        && session_control_authorized(session, peer_process_id, &proof);
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
        session.owner = Some(ProcessIdentity::of(client_process_id));
    }
    if let Some(verifier) = request.verifier {
        session.authentication = Some(SessionAuthentication::from_verifier(verifier)?);
        session.key_envelope = request.key_envelope;
        session.failed_authentications = 0;
        session.refuse_until = None;
        protected_by(daemon, session, peer_process_id);
    }
    session.summary.authentication_required = session.authentication.is_some();
    session.shared_state = request.offered.then(|| {
        let mut state = session.shared_state.clone().unwrap_or_else(|| {
            crate::messages::SharedSessionState::new(
                session.id,
                session.summary.clone(),
                session.state.clone(),
            )
        });
        state.summary = session.summary.clone();
        state.summary.id = session.id;
        state.state = session.state.clone();
        state
    });
    if request.offered {
        normalize_shared_state(daemon, session)?;
    }
    #[cfg(feature = "session-persistence")]
    if request.offered {
        persist_live_session(daemon, session)?;
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

/// Returns the authoritative collaboration snapshot after a client reconnects
/// or detects an event gap.
pub(in crate::server) fn shared_snapshot(
    daemon: &Arc<Daemon>,
    session_id: u64,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        peer_process_id,
        session_secret,
        ..
    } = caller;
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == session_id
    });
    let mut sessions = daemon.sessions();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    anyhow::ensure!(
        session.offered,
        "session {session_id} is not offered for shared collaboration"
    );
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, &proof),
        "shared session {session_id} is not authorized for this client"
    );
    let state = normalize_shared_state(daemon, session)?;
    drop(sessions);
    connection.send(&Response::SharedSnapshot { state })
}

/// Applies one collaboration operation. The sessions mutex is the daemon's
/// serialization point: whole-layout replacements remain exact-revision,
/// while operations naming panes or dividers are validated against and
/// applied to the latest canonical tree.
pub(in crate::server) fn apply_shared(
    daemon: &Arc<Daemon>,
    request: crate::messages::SharedSessionOperationRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        peer_process_id,
        session_secret,
        ..
    } = caller;
    let close_pane_id = match &request.operation {
        crate::messages::SharedSessionOperation::ClosePane { pane_id } => Some(*pane_id),
        _ => None,
    };
    let is_close = close_pane_id.is_some();
    let mut removed_pane = None;
    let state = {
        let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
            session.id == request.session_id
        });
        let mut sessions = daemon.sessions();
        let Some(session) = sessions
            .iter_mut()
            .find(|session| session.id == request.session_id)
        else {
            anyhow::bail!("session {} does not exist", request.session_id);
        };
        anyhow::ensure!(
            session.offered,
            "session {} is not offered for shared collaboration",
            request.session_id
        );
        anyhow::ensure!(
            session_control_authorized(session, peer_process_id, &proof),
            "shared session {} is not authorized for this client",
            request.session_id
        );
        let mut state = normalize_shared_state(daemon, session)?;
        if state.receipt(&request.operation_id).is_some() {
            drop(sessions);
            return connection.send(&Response::SharedOperationApplied { state });
        }
        let exact_revision = matches!(
            &request.operation,
            crate::messages::SharedSessionOperation::ReplaceTab { .. }
                | crate::messages::SharedSessionOperation::SetLayout { .. }
                | crate::messages::SharedSessionOperation::SetTabState { .. }
        );
        if exact_revision && state.revision != request.base_revision {
            drop(sessions);
            return connection.send(&Response::SharedConflict { state });
        }
        if let crate::messages::SharedSessionOperation::ReplaceTab { summary, .. } =
            &request.operation
        {
            anyhow::ensure!(
                summary_matches_daemon_panes(summary, session),
                "shared tab state must describe exactly the daemon pane set"
            );
        }
        if state.validate_operation(&request.operation).is_err() {
            drop(sessions);
            return connection.send(&Response::SharedConflict { state });
        }
        state.apply_operation(request.operation_id, &request.operation)?;
        clear_shared_size_reports(session);
        if let Some(pane_id) = close_pane_id {
            let index = session
                .panes
                .iter()
                .position(|pane| pane.id == pane_id)
                .expect("the canonical operation and daemon pane set agree");
            removed_pane = Some(session.panes.remove(index));
        }
        session.summary = state.summary.clone();
        session.summary.id = session.id;
        session.state = state.state.clone();
        session.shared_state = Some(state.clone());
        state
    };
    // Dropping the removed Pane closes the daemon's PTY and all shared relay
    // senders after the canonical state has been committed, so other viewers
    // never see a layout entry for a pane that the daemon still owns.
    drop(removed_pane);
    let event = if is_close {
        let pane_id = close_pane_id.expect("is_close records the closed pane");
        Event::SharedPanesChanged {
            session_id: request.session_id,
            added: Vec::new(),
            removed: vec![pane_id],
            state: state.clone(),
        }
    } else {
        Event::SharedSessionUpdated {
            state: state.clone(),
        }
    };
    broadcast(daemon, &event);
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::SharedOperationApplied { state })
}

/// Explicit leave for callers that want a request acknowledgement. Dropping a
/// shared data connection follows the same path through `remove_shared_attachment`;
/// this request is useful during orderly window shutdown and never kills a pane.
pub(in crate::server) fn leave_shared(
    daemon: &Arc<Daemon>,
    session_id: u64,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        peer_process_id,
        session_secret,
        ..
    } = caller;
    let client_id = caller.client_id.clone();
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == session_id
    });
    let mut sessions = daemon.sessions();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, &proof),
        "shared session {session_id} is not authorized for this client"
    );
    let revision = session
        .shared_state
        .as_ref()
        .map_or(crate::messages::SessionRevision::INITIAL, |state| {
            state.revision
        });
    for pane in &mut session.panes {
        let removed = match &mut pane.attachment {
            Attachment::Shared(clients) => {
                let before = clients.len();
                clients.retain(|client| client.client_id != client_id);
                clients.len() != before
            }
            _ => false,
        };
        settle_shared_departure(daemon, session_id, revision, pane, removed);
    }
    drop(sessions);
    daemon.sessions_condvar.notify_all();
    prune_exited_panes(daemon);
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
pub(in crate::server) fn set_session_scope(
    daemon: &Arc<Daemon>,
    request: crate::messages::SessionScopeRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let crate::messages::SessionScopeRequest {
        session_id,
        shared,
        verifier,
        key_envelope,
    } = request;
    let Caller {
        peer_process_id,
        session_secret,
        stream_only,
        ..
    } = caller;
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == session_id
    });
    let mut sessions = daemon.sessions();
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
        session_control_authorized(session, peer_process_id, &proof)
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
        protected_by(daemon, session, peer_process_id);
    }
    if !shared {
        let Some(owner) = session.owner.filter(|owner| owner.is_running()) else {
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
    if shared {
        let mut state = session.shared_state.clone().unwrap_or_else(|| {
            crate::messages::SharedSessionState::new(
                session.id,
                session.summary.clone(),
                session.state.clone(),
            )
        });
        state.summary = session.summary.clone();
        state.summary.id = session.id;
        state.state = session.state.clone();
        session.shared_state = Some(state);
        normalize_shared_state(daemon, session)?;
    } else if !shared {
        session.shared_state = None;
    }
    #[cfg(feature = "session-persistence")]
    if daemon.persistence.enabled() {
        persist_live_session(daemon, session)?;
    }
    drop(sessions);
    publish(daemon);
    connection.send(&Response::Ok)
}
