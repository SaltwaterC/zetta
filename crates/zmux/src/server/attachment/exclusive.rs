//! Converting a pane between shared and exclusive: taking it for the sole
//! viewer, releasing it back to the daemon, and offering it when a viewer is
//! left alone.

use super::*;

/// How long to wait for a granted pane's last relayed frames to reach the client
/// before giving up on the handover.
///
/// The wait exists because those frames have to arrive *before* the descriptor
/// does: the client starts reading the terminal itself the moment it has it, and
/// output that was still queued would then arrive after output that came later.
pub(in crate::server) const GRANT_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

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
///
/// Only the viewer itself may ask, and that is decided on the process the
/// transport vouched for, against the one it vouched for when the viewer
/// attached. Nothing else authorizes this: the viewer's attach was where any
/// secret was checked, so being that viewer is what being allowed means, and
/// the pane's terminal is what a protected session's secret protects. Matched
/// on a claimed process or client ID instead, any process holding the endpoint
/// token could take a protected pane from a relay viewer.
pub(in crate::server) fn take_exclusive(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        client_process_id,
        peer_process_id,
        stream_only,
        ..
    } = caller;
    let client_id = caller.client_id.clone();
    if stream_only {
        return connection.send(&Response::Error {
            message: "stream-only clients cannot take an exclusive pane".to_owned(),
        });
    }
    let Some(caller) = verified_process_id(client_process_id, peer_process_id) else {
        return connection.send(&Response::Error {
            message: format!(
                "could not confirm that process {client_process_id} is the one asking for \
                 pane {pane_id}"
            ),
        });
    };
    let mut sessions = daemon.sessions();
    let Some(pane) = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
    else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    if !matches!(&pane.attachment, Attachment::Shared(clients) if is_sole_viewer(clients, caller)) {
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
        Attachment::Shared(clients) if is_sole_viewer(clients, caller) => {
            let Attachment::Shared(mut clients) = std::mem::replace(
                &mut pane.attachment,
                Attachment::Granting {
                    holder: client_process_id,
                },
            ) else {
                unreachable!("the attachment was just matched as shared");
            };
            pane.attachment_client_id = Some(client_id.clone());
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

    let mut sessions = daemon.sessions();
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
        pane.attachment_client_id = None;
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
    pane.attachment_client_id = Some(client_id.clone());
    // Deliberately no replay. Everything read so far went to this client over the
    // relay; sending it again would print the pane's recent output twice. What is
    // left is still in the terminal, which the client now reads itself.
    pane.retained.clear();
    pane.handed_over = None;
    let child_pid = pane.pty.child_pid();
    let handles = match handover_handles(daemon, pane, client_process_id) {
        Ok(handles) => handles,
        Err(error) => {
            release_failed_handover(daemon, sessions, session_id, pane_id, &client_id);
            return Err(error);
        }
    };
    // Neither is read on this path: the client already has the tab this pane
    // belongs to and is only swapping what feeds it. They travel because the
    // exclusive attach's reply is the shape that carries a descriptor.
    let state = serde_json::Value::Null;
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    if let Err(error) = forget_persisted_session(daemon, session_id) {
        log::warn!("could not remove the attached session's persisted record: {error:#}");
    }
    if let Err(error) = connection.send_with(
        &Response::Attached {
            pane_id,
            child_pid,
            replay_length: 0,
            state,
            summary,
            handles: handles.values,
        },
        &handles.attachments.borrowed(),
    ) {
        release_failed_handover(daemon, daemon.sessions(), session_id, pane_id, &client_id);
        return Err(error);
    }
    daemon.sessions_condvar.notify_all();
    publish(daemon);
    wake_drain(daemon);
    Ok(())
}

/// Whether `clients` is a single local viewer whose attach was vouched for as
/// coming from `caller`.
pub(super) fn is_sole_viewer(clients: &[SharedClient], caller: u32) -> bool {
    matches!(clients, [client] if !client.stream_only && client.peer_process_id == Some(caller))
}

/// Gives a taken pane back, at the request of the client that took it.
///
/// The other half of [`release_failed_handover`]: that one covers a handover
/// this daemon could not finish sending, and this one covers a handover the
/// client could not finish receiving — its terminal would not adopt the
/// descriptor, or the window it belonged to closed as the answer arrived.
/// Either way a live process holds a pane it does not read, which nothing else
/// here can detect: the liveness sweep only recovers a pane whose holder has
/// died.
pub(in crate::server) fn release_exclusive(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        client_process_id,
        peer_process_id,
        ..
    } = caller;
    let client_id = caller.client_id.clone();
    let sessions = daemon.sessions();
    let Some(session) = sessions.iter().find(|session| session.id == session_id) else {
        drop(sessions);
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    // Releasing a protected pane knocks its window off it, so the claim alone
    // is not enough there: the holder has to be the process the transport
    // vouched for. Elsewhere it is the inconvenience `control_process_id`
    // describes, and an unattested client keeps working.
    let caller_process_id = if session.authentication.is_some() {
        verified_process_id(client_process_id, peer_process_id)
    } else {
        Some(control_process_id(client_process_id, peer_process_id))
    };
    let Some(pane) = session.panes.iter().find(|pane| pane.id == pane_id) else {
        drop(sessions);
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    // A handover that was never answered leaves the pane granting or revoking
    // rather than exclusive, and that client is the one this request is for:
    // a revoke it could not carry out strands the pane just as surely, because
    // it has already stopped reading the pty by the time it finds out.
    let holder = match pane.attachment {
        Attachment::Exclusive(holder)
        | Attachment::Granting { holder }
        | Attachment::Revoking { holder } => holder,
        Attachment::None | Attachment::Shared(_) => {
            drop(sessions);
            return connection.send(&Response::Error {
                message: format!("session {session_id} pane {pane_id} is not held exclusively"),
            });
        }
    };
    if Some(holder) != caller_process_id {
        drop(sessions);
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} is held by another client"),
        });
    }
    if pane
        .attachment_client_id
        .as_ref()
        .is_some_and(|attachment_client_id| attachment_client_id != &client_id)
    {
        drop(sessions);
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} is held by a newer attachment"),
        });
    }
    release_failed_handover(daemon, sessions, session_id, pane_id, &client_id);
    connection.send(&Response::Ok)
}

/// Puts a pane back under the drain thread after a handover that did not
/// complete.
///
/// The pane was marked exclusive before the descriptor was sent, because that
/// is the order the send needs. If the send then fails — the client died as it
/// was being answered — the pane is left attached to a client that does not
/// have it, and `drain_reads` skips a pane whose holder is supposed to be
/// reading it. Nobody reads the pty, it fills, the shell blocks on its own
/// output and stops reading input: one keystroke lands and every viewer of
/// that pane is stuck for good. Marking it unheld is what puts it back in the
/// drain's hands.
pub(super) fn release_failed_handover(
    daemon: &Arc<Daemon>,
    mut sessions: std::sync::MutexGuard<'_, Vec<Session>>,
    session_id: u64,
    pane_id: u64,
    attachment_client_id: &ClientId,
) {
    if let Some(pane) = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
        .filter(|pane| {
            pane.attachment_client_id
                .as_ref()
                .is_none_or(|current| current == attachment_client_id)
        })
    {
        pane.attachment = Attachment::None;
        pane.attachment_client_id = None;
        #[cfg(windows)]
        pane.pty.resume_reader();
    }
    drop(sessions);
    daemon.sessions_condvar.notify_all();
    publish(daemon);
    wake_drain(daemon);
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
pub(in crate::server) fn offer_exclusive_if_alone(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane: &Pane,
) {
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
    if let Some(subscriber) = subscriber_relay(daemon, &client_id, client.process_id) {
        enqueue_subscriber_event(daemon, &subscriber, &grant);
    }
}
