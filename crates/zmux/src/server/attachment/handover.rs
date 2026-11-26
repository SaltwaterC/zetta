//! Waiting for the client that holds a pane exclusively to hand it back
//! before another can attach, and answering the waiters once it has.

use super::*;

/// Whether `client_process_id` is the holder answering a handover the daemon
/// itself asked for.
///
/// That is not a new grant of access: that client is displaying the pane right
/// now, and was sent `Event::Revoke` a moment ago. Challenging it would make a
/// protected session impossible to share — the handover carries no secret,
/// because the user typed it into whichever *other* window is joining.
///
/// A process that still holds one of the session's panes is let through for the
/// same reason. The session is on its screen, not in the background, which is
/// what the secret protects; and a window whose detach the daemon refused has
/// already stopped reading its panes, and has to take them back without a
/// secret it never kept. The owner of a backgrounded session is deliberately
/// not exempt: reconnecting is exactly what the secret is asked for.
///
/// Only ever asked about a process the transport vouched for — see
/// [`Attacher::verified_process_id`]. Asked about a claimed one, it let any
/// process naming the holder join a protected session's relay while a
/// handover was open.
pub(super) fn answers_handover(session: &Session, client_process_id: u32) -> bool {
    session.panes.iter().any(|pane| {
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
    })
}

/// Sends an error once `sessions` has been released.
///
/// A client's connection may be an SSH forward that has stopped reading, and a
/// write that blocks under the sessions lock blocks every pane the daemon holds.
pub(super) fn respond_unlocked(
    sessions: MutexGuard<'_, Vec<Session>>,
    connection: &mut Connection,
    message: String,
) -> Result<()> {
    drop(sessions);
    connection.send(&Response::Error { message })
}

/// Waits for the client holding pane `pane_id` to hand it over, then attaches
/// this client to whatever the pane has become.
///
/// Entered with the pane's revoke already sent and this request counted in its
/// `handover_waiters`; every way out releases that count.
pub(super) fn await_handover(
    daemon: &Arc<Daemon>,
    mut sessions: MutexGuard<'_, Vec<Session>>,
    session_id: u64,
    pane_id: u64,
    attacher: &Attacher,
    connection: &mut Connection,
) -> Result<()> {
    let Attacher {
        client_process_id,
        ref client_id,
        stream_only,
        ..
    } = *attacher;
    let verified_process_id = attacher.verified_process_id();
    let deadline = Instant::now() + REVOKE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            if let Some(session) = sessions.iter_mut().find(|session| session.id == session_id)
                && let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id)
            {
                finish_handover_waiter(pane, true);
            }
            return respond_unlocked(
                sessions,
                connection,
                format!(
                    "the session's current viewer did not hand over pane {pane_id} within \
                     {REVOKE_TIMEOUT:?}"
                ),
            );
        }
        // A poisoned lock still hands the guard back; nothing panicked out of
        // the wait that a re-lock would fix.
        let (guard, _) = daemon
            .sessions_condvar
            .wait_timeout(sessions, remaining)
            .recover_poison();
        sessions = guard;
        let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
            return respond_unlocked(
                sessions,
                connection,
                format!("session {session_id} does not exist"),
            );
        };
        // Re-read rather than reusing what was read before the revoke. The
        // holder of a *live* session republishes its state as part of answering
        // the revoke, because a session shared while it is on screen goes on
        // changing — panes are split and closed, tabs are renamed — long after it
        // was first offered. Using the stale copy handed a joining client the
        // layout as of whenever sharing was switched on.
        let target = AttachTarget {
            session_id,
            pane_id,
            state: session.state.clone(),
            summary: Box::new(catalog_summary(session)),
        };
        let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
            return respond_unlocked(
                sessions,
                connection,
                format!("session {session_id} has no pane {pane_id}"),
            );
        };
        if pane.exited {
            finish_handover_waiter(pane, true);
            return respond_unlocked(
                sessions,
                connection,
                format!("session {session_id} pane {pane_id} has ended"),
            );
        }
        match pane.attachment {
            Attachment::Shared(_) => {
                finish_handover_waiter(pane, false);
                return attach_shared(daemon, sessions, target, attacher.clone(), connection, None);
            }
            Attachment::None => {
                finish_handover_waiter(pane, true);
                if stream_only {
                    pane.attachment = Attachment::Shared(Vec::new());
                    pane.attachment_client_id = None;
                    return attach_shared(
                        daemon,
                        sessions,
                        target,
                        attacher.clone(),
                        connection,
                        None,
                    );
                }
                return attach_exclusive(
                    daemon,
                    sessions,
                    target,
                    client_process_id,
                    client_id,
                    connection,
                );
            }
            Attachment::Exclusive(holder) => {
                // Reachable, and it used to panic here. A revoking pane whose
                // holder died is reset to unheld — by this loop or by the
                // liveness sweep — and whichever waiter wakes first takes it
                // exclusively, so a second waiter finds it exclusive again. That
                // is another client holding the pane, which is exactly the case
                // `attach` already handles from the top, so start over rather
                // than treating it as impossible.
                if !stream_only && Some(holder) == verified_process_id {
                    finish_handover_waiter(pane, true);
                    return attach_exclusive(
                        daemon,
                        sessions,
                        target,
                        client_process_id,
                        client_id,
                        connection,
                    );
                }
                pane.attachment = Attachment::Revoking { holder };
                let revoke = Event::Revoke {
                    session_id,
                    pane_id,
                };
                if let Some(subscriber) = subscriber_relay_for_process(daemon, holder) {
                    enqueue_subscriber_event(daemon, &subscriber, &revoke);
                }
            }
            // A grant whose taker died leaves nobody holding the descriptor: the
            // reply either never went out or went to a process that has gone.
            Attachment::Granting { holder } | Attachment::Revoking { holder }
                if !crate::process_status::is_running(holder) =>
            {
                pane.attachment = Attachment::None;
                pane.attachment_client_id = None;
                finish_handover_waiter(pane, true);
                return attach_exclusive(
                    daemon,
                    sessions,
                    target,
                    client_process_id,
                    client_id,
                    connection,
                );
            }
            Attachment::Revoking { .. } | Attachment::Granting { .. } => {}
        }
    }
}

/// Releases one attach request waiting for a revoke.
pub(in crate::server) fn finish_handover_waiter(pane: &mut Pane, collapse_empty_shared: bool) {
    pane.handover_waiters = pane.handover_waiters.saturating_sub(1);
    if collapse_empty_shared
        && pane.handover_waiters == 0
        && matches!(&pane.attachment, Attachment::Shared(clients) if clients.is_empty())
    {
        pane.attachment = Attachment::None;
        pane.attachment_client_id = None;
        pane.handed_over = None;
    }
}
