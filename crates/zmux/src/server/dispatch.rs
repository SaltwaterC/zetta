//! Reading a request off a connection and answering it.
//!
//! `serve` is the daemon's whole request surface: one arm per [`Request`],
//! each delegating to the module that owns that operation. A request is
//! authenticated before it is decoded, so an unauthenticated peer cannot reach
//! any of them.

use super::*;

pub(super) fn serve(daemon: &Arc<Daemon>, stream: Stream, token: &str) -> Result<()> {
    // A terminal must never cross a user boundary. The socket's permissions
    // already restrict this; checking the peer as well means a mistake in the
    // directory's access control is not on its own enough.
    anyhow::ensure!(
        peer_is_this_user(&stream)?,
        "refusing a connection from another user"
    );
    let peer_process_id = crate::transport::peer_process_id(&stream)?;

    let mut connection = Connection::new(stream);
    let mut envelope = read_request(&mut connection, token)?;

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
        envelope = read_request(&mut connection, token)?;
        attested
    } else {
        peer_process_id
    };
    // Nothing to establish where the kernel already answered the question, but
    // the exchange still has to be answered so a portable client can ask.
    #[cfg(unix)]
    if matches!(envelope.request, Request::Attest) {
        connection.send(&Response::Ok)?;
        envelope = read_request(&mut connection, token)?;
    }

    // Where the platform reports no peer credentials, a request whose answer
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

    let client_id = envelope.client_id.clone();
    let stream_only = envelope.stream_only;
    let session_secret = envelope.session_secret;

    match envelope.request {
        Request::Ping => connection.send(&Response::Ok),
        Request::Subscribe => {
            // Keyed by process so a revoke can be sent to the one client that
            // holds a pane rather than broadcast to every subscriber. A client
            // that subscribes twice is the same process, so the later
            // connection replaces the earlier one.
            daemon.subscribers.lock().unwrap().insert(
                client_id,
                Subscriber {
                    process_id: envelope.client_process_id,
                    connection,
                },
            );
            Ok(())
        }
        Request::Spawn(request) => {
            if stream_only {
                return connection.send(&Response::Error {
                    message: "stream-only clients cannot spawn panes".to_owned(),
                });
            }
            let response = spawn(
                daemon,
                request,
                envelope.client_process_id,
                peer_process_id,
                &mut connection,
            );
            match response {
                Ok(()) => Ok(()),
                Err(error) => {
                    let _ = connection.send(&Response::Error {
                        message: format!("{error:#}"),
                    });
                    Err(error)
                }
            }
        }
        Request::Attach {
            session_id,
            pane_id,
            secret,
        } => attach(
            daemon,
            session_id,
            pane_id,
            secret,
            envelope.client_process_id,
            client_id.clone(),
            stream_only,
            &mut connection,
        ),
        // A screen checkpoint from the client showing the pane. Its own
        // connection is distinct from the shared connection; a revoke answer
        // also wakes the attach that started the handover.
        Request::Snapshot {
            session_id,
            pane_id,
            length,
            columns,
            lines,
        } => {
            if stream_only {
                return connection.send(&Response::Error {
                    message: "stream-only clients cannot perform exclusive handover".to_owned(),
                });
            }
            snapshot(
                daemon,
                session_id,
                pane_id,
                length,
                columns,
                lines,
                envelope.client_process_id,
                peer_process_id,
                &mut connection,
            )
        }
        Request::StoreImage {
            session_id,
            pane_id,
            length,
        } => {
            match store_image(
                daemon,
                session_id,
                pane_id,
                length,
                envelope.client_process_id,
                peer_process_id,
                client_id,
                stream_only,
                session_secret.as_deref(),
                &mut connection,
            ) {
                Ok(()) => Ok(()),
                Err(error) => connection.send(&Response::Error {
                    message: format!("{error:#}"),
                }),
            }
        }
        Request::Input { .. } => connection.send(&Response::Error {
            message: "input belongs on a shared connection".to_owned(),
        }),
        Request::Attested { .. } | Request::Attest => connection.send(&Response::Error {
            message: "an attestation precedes a request, it does not replace one".to_owned(),
        }),
        Request::Detach(request) => {
            if stream_only {
                return connection.send(&Response::Error {
                    message: "stream-only clients cannot detach sessions".to_owned(),
                });
            }
            match detach(
                daemon,
                request,
                envelope.client_process_id,
                peer_process_id,
                &mut connection,
            ) {
                Ok(()) => Ok(()),
                Err(error) => connection.send(&Response::Error {
                    message: format!("{error:#}"),
                }),
            }
        }
        Request::Resume(request) => {
            if stream_only {
                return connection.send(&Response::Error {
                    message: "stream-only clients cannot resume disk sessions".to_owned(),
                });
            }
            match resume(
                daemon,
                request,
                envelope.client_process_id,
                peer_process_id,
                &mut connection,
            ) {
                Ok(()) => Ok(()),
                Err(error) => connection.send(&Response::Error {
                    message: format!("{error:#}"),
                }),
            }
        }
        Request::TakeExclusive {
            session_id,
            pane_id,
        } => take_exclusive(
            daemon,
            session_id,
            pane_id,
            envelope.client_process_id,
            client_id,
            stream_only,
            &mut connection,
        ),
        Request::Share(request) => share(
            daemon,
            request,
            envelope.client_process_id,
            peer_process_id,
            session_secret.as_deref(),
            &mut connection,
        ),
        Request::Resize {
            session_id,
            pane_id,
            columns,
            lines,
        } => match resize_pane(
            daemon,
            session_id,
            pane_id,
            columns,
            lines,
            peer_process_id,
            session_secret.as_deref(),
        ) {
            Ok(()) => connection.send(&Response::Ok),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::SetConsolePalette {
            session_id,
            pane_id,
            palette,
        } => match set_console_palette(
            daemon,
            session_id,
            pane_id,
            palette,
            envelope.client_process_id,
            peer_process_id,
            session_secret.as_deref(),
        ) {
            Ok(()) => connection.send(&Response::Ok),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::List => {
            // Sanitized exactly as the published catalog is. The endpoint token
            // authenticates the channel, not a session: listing must not reveal
            // the commands, titles or directories of a session whose whole
            // point is that they stay private until its secret is presented.
            let sessions = daemon
                .sessions
                .lock()
                .unwrap()
                .iter()
                .filter(|session| session.is_available() && (!stream_only || session.offered))
                .map(|session| catalog_summary(session).for_public_catalog())
                .collect();
            #[cfg(feature = "session-persistence")]
            let mut restorable: Vec<crate::protocol::RestorableSessionRecord> = daemon
                .persistence
                .lock()
                .unwrap()
                .as_ref()
                .map(|persistence| {
                    persistence
                        .records()
                        .iter()
                        .filter(|record| record.restorable)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            #[cfg(feature = "session-persistence")]
            for restored in daemon.restored.lock().unwrap().iter() {
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
        Request::PaneStates { pane_ids } => {
            let panes = pane_states(
                daemon,
                &pane_ids,
                peer_process_id,
                session_secret.as_deref(),
            );
            connection.send(&Response::PaneStates { panes })
        }
        Request::ClosePane {
            session_id,
            pane_id,
        } => {
            if stream_only {
                return connection.send(&Response::Error {
                    message: "stream-only clients release panes by closing the shared connection"
                        .to_owned(),
                });
            }
            match close_pane(
                daemon,
                session_id,
                pane_id,
                envelope.client_process_id,
                peer_process_id,
                session_secret.as_deref(),
            ) {
                Ok(()) => connection.send(&Response::Ok),
                Err(error) => connection.send(&Response::Error {
                    message: format!("{error:#}"),
                }),
            }
        }
        Request::Kill { session_id } => match kill(
            daemon,
            session_id,
            peer_process_id,
            session_secret.as_deref(),
        ) {
            Ok(()) => {
                publish(daemon);
                connection.send(&Response::Ok)
            }
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::SetSessionScope {
            session_id,
            shared,
            verifier,
            key_envelope,
        } => match set_session_scope(
            daemon,
            session_id,
            shared,
            verifier,
            key_envelope,
            peer_process_id,
            session_secret.as_deref(),
            stream_only,
            &mut connection,
        ) {
            Ok(()) => Ok(()),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::Forget { session_id } => match forget(
            daemon,
            session_id,
            peer_process_id,
            session_secret.as_deref(),
        ) {
            Ok(()) => {
                publish(daemon);
                connection.send(&Response::Ok)
            }
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::Configure {
            retention,
            persistence_recipients,
        } if !stream_only => match configure_daemon(daemon, retention, persistence_recipients) {
            Ok(()) => connection.send(&Response::Ok),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::Upgrade if !stream_only => {
            #[cfg(any(unix, windows))]
            {
                // Only returns when the replacement was refused: a successful
                // upgrade never comes back, because this process becomes it.
                match upgrade_daemon(daemon, &mut connection) {
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
        Request::Shutdown if !stream_only => {
            let held = daemon.sessions.lock().unwrap().len();
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
        Request::Configure { .. } | Request::Upgrade | Request::Shutdown => {
            connection.send(&Response::Error {
                message: "this request is local-only".to_owned(),
            })
        }
    }
}

/// Applies settings from a client that may have been started after this
/// daemon. A daemon deliberately outlives its clients, so treating its startup
/// arguments as permanent makes editing the configuration appear to work while
/// leaving all subsequent sessions under the old retention policy.
pub(super) fn configure_daemon(
    daemon: &Arc<Daemon>,
    retention: Retention,
    persistence_recipients: Vec<String>,
) -> Result<()> {
    retention.validate()?;
    #[cfg(not(feature = "session-persistence"))]
    let _ = persistence_recipients;

    let old_retention = *daemon.retention.lock().unwrap();
    let mut sessions = daemon.sessions.lock().unwrap();
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
    #[cfg(feature = "session-persistence")]
    let persisted_sessions = if matches!(retention, Retention::Disk) {
        sessions
            .iter()
            .filter(|session| session.keep || session.offered)
            .map(persisted_live_session)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    drop(sessions);

    #[cfg(feature = "session-persistence")]
    let mut next_persistence = {
        let mut persistence = daemon.persistence.lock().unwrap();
        if let Some(persistence) = persistence.as_mut() {
            persistence
                .flush_segments()
                .context("flushing encrypted scrollback before changing retention")?;
        }
        if matches!(retention, Retention::Disk) && !persistence_recipients.is_empty() {
            // Configure is the normal path used when a client brings an
            // already-running daemon onto disk persistence. It is not an
            // in-process executable handoff: any records left by an earlier
            // daemon must become restorable even when the boot stamp is the
            // same. Live sessions are written below as non-restorable again.
            PersistenceStore::open_with_recovery_state(
                &session_catalog_dir(),
                Some(&persistence_recipients),
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

    #[cfg(feature = "session-persistence")]
    {
        if matches!(retention, Retention::Disk)
            && let Some(persistence) = next_persistence.as_mut()
        {
            for session in &persisted_sessions {
                persistence.save_session(session)?;
            }
        }
        let mut persistence = daemon.persistence.lock().unwrap();
        let persistence_enabled = matches!(retention, Retention::Disk)
            && !persistence_recipients.is_empty()
            && next_persistence.is_some();
        *persistence = next_persistence;
        daemon
            .persistence_enabled
            .store(persistence_enabled, Ordering::Release);
    }
    *daemon.retention.lock().unwrap() = retention;
    wake_drain(daemon);
    publish(daemon);
    Ok(())
}
