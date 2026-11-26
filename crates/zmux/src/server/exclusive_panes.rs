//! Explicit removal of an exclusively held pane.
//!
//! Releasing a window's attachment leaves the daemon-owned PTY available for
//! background work. Closing a pane must remove that PTY from the session before
//! another catalog publication or restore can discover it again. Shared panes
//! are closed through the revisioned collaboration operations in `lifecycle`.

use super::*;

pub(super) fn discard_pane(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    caller: Caller<'_>,
) -> Result<()> {
    let Caller {
        client_process_id,
        peer_process_id,
        session_secret,
        ..
    } = caller;
    let client_process_id = control_process_id(client_process_id, peer_process_id);
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == session_id
    });
    let mut sessions = daemon.sessions.lock().recover_poison();
    let session = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .with_context(|| format!("session {session_id} does not exist"))?;
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, &proof),
        "session {session_id} is protected and can only be changed by its owner or current holder"
    );
    let index = session
        .panes
        .iter()
        .position(|pane| pane.id == pane_id)
        .with_context(|| format!("session {session_id} has no pane {pane_id}"))?;
    anyhow::ensure!(
        matches!(
            session.panes[index].attachment,
            Attachment::Exclusive(holder) | Attachment::Revoking { holder }
                if holder == client_process_id
        ),
        "client {client_process_id} does not hold pane {pane_id} exclusively"
    );

    let discarded = session.panes.remove(index);
    let ended = session.panes.is_empty();
    // Repair both the catalog and any canonical state before releasing the
    // lock. Keep cleanup unconditional even if persisting the new state fails.
    let updated = if ended {
        Ok(None)
    } else if session.offered {
        let state = normalize_shared_state(daemon, session);
        #[cfg(feature = "session-persistence")]
        let state = state.and_then(|state| {
            persist_live_session(daemon, session)?;
            Ok(state)
        });
        state.map(Some)
    } else {
        // A private session is the window's own until it detaches: the daemon
        // holds no tab state for it (`state` is still null) and no disk record
        // may exist, because attaching forgot it. Writing one here left a
        // record a dead daemon could not take back, restored later as a
        // session nobody kept and that no client can rebuild. Only the catalog
        // is repaired, and without installing a canonical shared state, which
        // a restore would read as the session having been offered.
        let previous = crate::messages::SharedSessionState::new(
            session.id,
            session.summary.clone(),
            serde_json::Value::Null,
        );
        summary_from_daemon_panes(session, &previous).map(|summary| {
            session.summary = summary;
            None
        })
    };
    if ended {
        sessions.retain(|session| session.id != session_id);
    }
    drop(sessions);

    #[cfg(windows)]
    let console_id = discarded.console_id;
    drop(discarded);
    remove_pane_agent_links(pane_id);
    #[cfg(windows)]
    close_host_console(daemon, console_id);
    let forgotten = if ended {
        forget_discarded_session(daemon, session_id)
    } else {
        Ok(())
    };
    daemon.sessions_condvar.notify_all();
    // The final pane removes its session and persisted record too, including
    // sessions previously marked keep-running.
    prune_exited_panes(daemon);
    if let Ok(Some(state)) = &updated {
        broadcast(
            daemon,
            &Event::SharedPanesChanged {
                session_id,
                added: Vec::new(),
                removed: vec![pane_id],
                state: state.clone(),
            },
        );
    }
    publish(daemon);
    wake_drain(daemon);
    updated.and(forgotten)
}

fn forget_discarded_session(daemon: &Arc<Daemon>, session_id: u64) -> Result<()> {
    remove_image_session(daemon, session_id);
    #[cfg(feature = "session-persistence")]
    if let Some(persistence) = daemon.persistence.lock().as_mut() {
        persistence.forget(session_id)?;
    }
    Ok(())
}
