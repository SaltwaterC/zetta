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
    client_process_id: u32,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Result<()> {
    let client_process_id = control_process_id(client_process_id, peer_process_id);
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == session_id
    });
    let mut sessions = daemon
        .sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
    } else {
        normalize_shared_state(daemon, session).and_then(|state| {
            #[cfg(feature = "session-persistence")]
            persist_live_session(daemon, session)?;
            Ok(Some(state))
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
