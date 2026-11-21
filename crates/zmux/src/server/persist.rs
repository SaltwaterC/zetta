//! Writing a live session to disk and forgetting it again, for builds that
//! persist sessions across a reboot.

use super::*;

/// Writes `persisted` under `seal`, which is what its session is sealed to now.
pub(super) fn persist_session(
    daemon: &Arc<Daemon>,
    persisted: &PersistedSession,
    seal: &crate::persistence::Seal,
) -> Result<()> {
    if !daemon.persistence.enabled() {
        return Ok(());
    }
    if let Some(persistence) = daemon.persistence.lock().as_mut() {
        persistence.seal(persisted.id, seal)?;
        persistence.save_session(persisted)?;
    }
    Ok(())
}

pub(super) fn persist_live_session(daemon: &Arc<Daemon>, session: &Session) -> Result<()> {
    persist_session(daemon, &persisted_live_session(session), &seal_for(session))
}

pub(super) fn persisted_live_session(session: &Session) -> PersistedSession {
    PersistedSession {
        id: session.id,
        created_at: unix_now(),
        updated_at: unix_now(),
        summary: session.summary.clone(),
        state: session.state.clone(),
        shared_state: session.shared_state.clone(),
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
        snapshots: session
            .panes
            .iter()
            .map(|pane| {
                let (columns, lines) =
                    terminal_size(pane).unwrap_or((pane.size.columns, pane.size.lines));
                PersistedSnapshot {
                    pane_id: pane.id,
                    bytes: pane.retained.snapshot(),
                    columns: Some(columns),
                    lines: Some(lines),
                }
            })
            .collect(),
    }
}

pub(super) fn forget_persisted_session(daemon: &Arc<Daemon>, session_id: u64) -> Result<()> {
    if let Some(persistence) = daemon.persistence.lock().as_mut()
        && persistence
            .records()
            .iter()
            .any(|record| record.id == session_id)
    {
        persistence.forget(session_id)?;
    }
    Ok(())
}

/// Queues what the drain read for the encrypted store. Called under the
/// registry lock, so it must not wait on the store: see [`PersistenceQueue`].
pub(super) fn record_persistence_output(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    bytes: &[u8],
) {
    daemon.persistence.append(session_id, pane_id, bytes);
}

pub(super) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
