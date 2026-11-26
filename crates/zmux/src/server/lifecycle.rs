//! What a session is made of over its life: spawning it, resuming it,
//! detaching it, sharing it, and ending it.
//!
//! Every operation here decides authorization first — who may hold, take, or
//! end a session — because a session outlives the client that created it and
//! may be reached by any client that can open the socket.

use super::*;

mod authorization;
mod shared_create;
mod sharing;
mod spawn;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub(super) use authorization::*;
pub(super) use shared_create::*;
pub(super) use sharing::*;
pub(super) use spawn::*;
#[cfg(unix)]
pub(super) use unix::*;
#[cfg(windows)]
pub(super) use windows::*;

pub(super) enum SpawnOutcome {
    Complete,
    SharedConflict(Box<crate::messages::SharedSessionState>),
}

/// The directory a pane is started in, given what the requester asked for.
///
/// Never the daemon's own. A daemon is started detached — by a window that has
/// since closed, by a login item, by `zmux --daemon` from anywhere — so its
/// working directory is an accident, and on macOS it is `/`. A pane that
/// inherits it opens on the root of the filesystem, which is no use to
/// anybody; the account's home directory is the shell's own answer to "where
/// should I start", and is what every login shell would have used.
fn pane_start_directory(requested: Option<PathBuf>) -> Option<PathBuf> {
    requested
        .filter(|directory| !directory.as_os_str().is_empty())
        .or_else(home_directory)
}

/// The account's home directory, when it has a usable one.
fn home_directory() -> Option<PathBuf> {
    crate::paths::home_dir().filter(|home| !home.as_os_str().is_empty() && home.is_dir())
}

/// The directory a pane is running in, read from the process the daemon
/// started for it. Only the daemon can answer this for a pane whose viewers
/// are on another machine: it is the process's parent, and they are not.
fn pane_working_directory(daemon: &Arc<Daemon>, pane_id: u64) -> Option<PathBuf> {
    let child_pid = {
        let sessions = daemon.sessions.lock().recover_poison();
        sessions
            .iter()
            .flat_map(|session| &session.panes)
            .find(|pane| pane.id == pane_id)
            .map(|pane| pane.pty.child_pid())?
    };
    if child_pid == 0 {
        return None;
    }
    let child_pid = sysinfo::Pid::from_u32(child_pid);
    let mut system = sysinfo::System::new();
    // Ask for the working directory by name. `refresh_processes` refreshes
    // memory, CPU, disk usage, the executable and the task list — everything
    // except the one field this function exists to read — so asking with it
    // returned `None` on every platform, and a pane split from a remote one
    // started wherever the daemon happened to be, which is `/`. Naming the
    // field is also less work than the default set, not more.
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[child_pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_cwd(sysinfo::UpdateKind::Always),
    );
    system
        .process(child_pid)
        .and_then(|process| process.cwd().map(Path::to_path_buf))
        .filter(|directory| !directory.as_os_str().is_empty())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn remove_pane_agent_links(_pane_id: u64) {}

/// Checks a protected resume's secret, returning the response that refuses it
/// or `None` when the resume may go ahead (including when nothing protects the
/// record).
///
/// The verifier comes from the client, which decrypted the record, so it is
/// held to the import policy before anything is hashed; and the check goes
/// through the daemon's [`VerificationGate`] like every other, under the
/// daemon's own refusal window — see [`check_resume_secret`]. A failure is
/// written back to the record's authentication sidecar, so a daemon started
/// later begins from it.
#[cfg(feature = "session-persistence")]
fn authenticate_resume(
    daemon: &Daemon,
    request: &mut crate::messages::ResumeRequest,
    peer_process_id: Option<u32>,
    fallback_persistence: &mut Option<PersistenceStore>,
) -> Result<Option<Response>> {
    let Some(verifier) = request.verifier.as_ref() else {
        return Ok(None);
    };
    anyhow::ensure!(
        peer_process_id.is_some(),
        "protected restores require a verified peer process"
    );
    let authentication = SessionAuthentication::from_verifier(verifier.clone())?;
    let reported = ReportedResumeBackoff {
        failed_authentications: request.failed_authentications,
        refusing: request.backoff_seconds > unix_now().saturating_sub(request.updated_at),
    };
    if resume_refused(daemon, request.record_id, reported) {
        return Ok(Some(Response::AuthenticationFailed));
    }
    let Some(secret) = request.secret.as_deref() else {
        return Ok(Some(Response::AuthenticationRequired));
    };
    let (failed_authentications, backoff_seconds) =
        match check_resume_secret(daemon, request.record_id, &authentication, secret, reported) {
            ResumeCheck::Verified => return Ok(None),
            ResumeCheck::Refused => return Ok(Some(Response::AuthenticationFailed)),
            ResumeCheck::Failed {
                failed_authentications,
                backoff_seconds,
            } => (failed_authentications, backoff_seconds),
        };
    request.secret = None;
    request.failed_authentications = failed_authentications;
    request.backoff_seconds = backoff_seconds;
    request.updated_at = unix_now();
    let mut persistence = daemon.persistence.lock();
    if let Some(persistence) = persistence.as_mut().or(fallback_persistence.as_mut()) {
        persistence.update_authentication(
            request.record_id,
            request.updated_at,
            request.failed_authentications,
            request.backoff_seconds,
        )?;
    }
    Ok(Some(Response::AuthenticationFailed))
}

#[cfg_attr(
    not(feature = "session-persistence"),
    expect(
        unused_variables,
        reason = "without persistence there is no record to resume"
    )
)]
pub(super) fn resume(
    daemon: &Arc<Daemon>,
    request: crate::messages::ResumeRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        peer_process_id, ..
    } = caller;
    #[cfg(not(feature = "session-persistence"))]
    {
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
        let lengths = request
            .snapshots
            .iter()
            .map(|snapshot| (snapshot.pane_id, snapshot.length))
            .collect::<Vec<_>>();
        let mut received = read_snapshots(&daemon.snapshot_budget, connection, &lengths)?;
        let snapshots = std::mem::take(&mut received.bytes);
        // A daemon that was started before the memory-fallback recovery path
        // was available may still be running without a persistence handle.
        // Open a temporary recovery handle in that case so a disk record can
        // still be resumed; it is deliberately not installed on the daemon,
        // and therefore cannot make a memory-mode session durable.
        let mut fallback_persistence = if daemon.persistence.lock().is_none() {
            PersistenceStore::open_with_recovery_state(&daemon.directory, None, false)?
        } else {
            None
        };
        let daemon_has_record = daemon
            .persistence
            .lock()
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
                .sessions()
                .iter()
                .any(|session| session.id == request.record_id),
            "session {} is already live",
            request.record_id
        );
        anyhow::ensure!(
            !daemon
                .restored
                .lock()
                .recover_poison()
                .iter()
                .any(|restored| restored.request.record_id == request.record_id),
            "session {} is already being resumed",
            request.record_id
        );
        if let Some(refusal) = authenticate_resume(
            daemon,
            &mut request,
            peer_process_id,
            &mut fallback_persistence,
        )? {
            return connection.send(&refusal);
        }
        // The secret is only a request credential. It must not survive in the
        // daemon's restored-session memory after the verifier has been checked.
        request.secret = None;
        // The decrypted record is now represented by the authenticated lease.
        // Removing the disk copy keeps it from being offered a second time or
        // mistaken for a separate restore while the first handoff is in
        // flight. A failed Spawn leaves the lease itself intact.
        let mut persistence = daemon.persistence.lock();
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
        daemon
            .restored
            .lock()
            .recover_poison()
            .push(RestoredSession {
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

pub(super) fn detach(
    daemon: &Arc<Daemon>,
    request: crate::messages::DetachRequest,
    caller: Caller<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let Caller {
        client_process_id,
        peer_process_id,
        ..
    } = caller;
    // Read the snapshots before taking the lock: they arrive as raw bytes on
    // this connection and can be large.
    let lengths = request
        .snapshots
        .iter()
        .map(|snapshot| (snapshot.pane_id, snapshot.length))
        .collect::<Vec<_>>();
    let mut received = read_snapshots(&daemon.snapshot_budget, connection, &lengths)?;
    let mut snapshots = std::mem::take(&mut received.bytes);
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

    let mut sessions = daemon.sessions();
    let Some(session) = sessions
        .iter_mut()
        .find(|session| session.id == request.session_id)
    else {
        return connection.send(&Response::Error {
            message: format!("session {} does not exist", request.session_id),
        });
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, &SecretProof::NONE),
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
        session.owner = Some(ProcessIdentity::of(client_process_id));
    }
    if let Some(verifier) = request.verifier {
        session.authentication = Some(SessionAuthentication::from_verifier(verifier)?);
        // Paired with the verifier it arrived beside, so a session reprotected
        // with a typed secret loses the stale envelope rather than keeping a way
        // in that no longer opens anything.
        session.key_envelope = request.key_envelope;
        session.failed_authentications = 0;
        session.refuse_until = None;
        protected_by(daemon, session, peer_process_id);
    }
    session.summary.authentication_required = session.authentication.is_some();
    if session.offered {
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
    }
    #[cfg(feature = "session-persistence")]
    let persisted = daemon.persistence.enabled().then(|| PersistedSession {
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
        persist_session(daemon, &persisted, &seal_for(session))?;
    }
    // Released only once nothing can refuse the detach any more. The client
    // has stopped reading its panes and takes them back when it is refused,
    // and a pane already released here would have been drained in between:
    // its output read into the daemon's grid instead of the window's, and
    // lost from the screen the window carries on with.
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
                pane.attachment_client_id = None;
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
    drop(sessions);

    prune_exited_panes(daemon);
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::Detached)
}

/// Applies a client's new size to a pane's terminal, recording it as the
/// pane's size so a later shared attach starts from it.
pub(super) struct ResizePaneContext<'a> {
    pub(super) session_id: u64,
    pub(super) pane_id: u64,
    pub(super) revision: Option<crate::messages::SessionRevision>,
    pub(super) columns: u16,
    pub(super) lines: u16,
    pub(super) peer_process_id: Option<u32>,
    pub(super) session_secret: Option<&'a str>,
}

pub(super) fn resize_pane(daemon: &Arc<Daemon>, context: ResizePaneContext<'_>) -> Result<()> {
    use alacritty_terminal::event::OnResize as _;
    let ResizePaneContext {
        session_id,
        pane_id,
        revision,
        columns,
        lines,
        peer_process_id,
        session_secret,
    } = context;
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == session_id
    });
    let mut sessions = daemon.sessions();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    if let Some(revision) = revision {
        let current = session
            .shared_state
            .as_ref()
            .map_or(crate::messages::SessionRevision::INITIAL, |state| {
                state.revision
            });
        anyhow::ensure!(
            revision == current,
            "stale shared size report for revision {}; current revision is {}",
            revision.0,
            current.0
        );
    }
    if session.authentication.is_some()
        && !protected_holder_authorized(session, peer_process_id)
        && !session_control_authorized(session, peer_process_id, &proof)
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

#[cfg_attr(
    not(windows),
    expect(
        unused_variables,
        reason = "only a Windows pseudoconsole has a palette to set"
    )
)]
pub(super) fn set_console_palette(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    palette: ConsolePalette,
    caller: Caller<'_>,
) -> Result<()> {
    let Caller {
        client_process_id,
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
    let protected = session.authentication.is_some();
    let holder_authorized = if protected {
        protected_holder_authorized(session, peer_process_id)
    } else {
        Some(control_process_id(client_process_id, peer_process_id))
            .is_some_and(|process_id| session_is_held_by(session, process_id))
    };
    let secret_authorized = protected
        && !holder_authorized
        && session_control_authorized(session, peer_process_id, &proof);
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
    Ok(())
}

pub(super) fn kill(
    daemon: &Arc<Daemon>,
    session_id: u64,
    peer_process_id: Option<u32>,
    session_secret: Option<&str>,
) -> Result<()> {
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == session_id
    });
    let mut sessions = daemon.sessions();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        drop(sessions);
        #[cfg(feature = "session-persistence")]
        {
            let mut persistence = daemon.persistence.lock();
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
                remove_image_session(daemon, session_id);
                return Ok(());
            }
        }
        anyhow::bail!("session {session_id} does not exist");
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, &proof),
        "session {session_id} is protected and can only be ended by its owner or current holder"
    );
    // Dropping the panes drops their PTYs, which hangs up the children.
    #[cfg(windows)]
    let consoles = session
        .panes
        .iter()
        .map(|pane| pane.console_id)
        .collect::<Vec<_>>();
    let discarded = sessions
        .extract_if(.., |session| session.id == session_id)
        .collect::<Vec<_>>();
    drop(sessions);
    let discarded_panes = discarded
        .iter()
        .flat_map(|session| session.panes.iter().map(|pane| pane.id))
        .collect::<Vec<_>>();
    drop(discarded);
    for pane_id in discarded_panes {
        remove_pane_agent_links(pane_id);
    }
    #[cfg(feature = "session-persistence")]
    if let Some(persistence) = daemon.persistence.lock().as_mut() {
        persistence.forget(session_id)?;
    }
    remove_image_session(daemon, session_id);
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
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.id == session_id
    });
    let mut sessions = daemon.sessions();
    if let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) {
        anyhow::ensure!(
            session_control_authorized(session, peer_process_id, &proof),
            "session {session_id} is protected and can only be forgotten by its owner or current holder"
        );
        #[cfg(windows)]
        let consoles = session
            .panes
            .iter()
            .map(|pane| pane.console_id)
            .collect::<Vec<_>>();
        let discarded = sessions
            .extract_if(.., |session| session.id == session_id)
            .collect::<Vec<_>>();
        drop(sessions);
        drop(discarded);
        #[cfg(feature = "session-persistence")]
        if let Some(persistence) = daemon.persistence.lock().as_mut() {
            persistence.forget(session_id)?;
        }
        remove_image_session(daemon, session_id);
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
            .as_ref()
            .is_some_and(|persistence| {
                persistence
                    .records()
                    .iter()
                    .any(|record| record.id == session_id)
            });
        if has_record {
            if let Some(persistence) = daemon.persistence.lock().as_mut() {
                persistence.forget(session_id)?;
            }
            remove_image_session(daemon, session_id);
            return Ok(());
        }
        let mut restored = daemon.restored.lock().recover_poison();
        if restored
            .iter()
            .any(|session| session.request.record_id == session_id)
        {
            restored.retain(|session| session.request.record_id != session_id);
            remove_image_session(daemon, session_id);
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
    // One check per protected session the panes belong to, not one per pane:
    // a poll naming every pane of a session used to run Argon2, and charge a
    // wrong secret, once for each of them.
    let proof = prove_session_secret(daemon, peer_process_id, session_secret, |session| {
        session.panes.iter().any(|pane| pane_ids.contains(&pane.id))
    });
    let sessions = daemon.sessions();
    pane_ids
        .iter()
        .map(|&pane_id| {
            let session_index = sessions
                .iter()
                .position(|session| session.panes.iter().any(|pane| pane.id == pane_id));
            if let Some(index) = session_index {
                let protected =
                    !session_control_authorized(&sessions[index], peer_process_id, &proof);
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
    let mut sessions = daemon.sessions();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id, &proof),
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
            pane.attachment_client_id = None;
        }
        // A shared client leaves by closing its own data-plane connection,
        // which is what `remove_shared_attachment` acts on; releasing the whole
        // pane here would evict the other viewers too.
        _ => anyhow::bail!("client {client_process_id} does not hold pane {pane_id}"),
    }
    let ended_sessions = take_abandoned_sessions(&mut sessions);
    let released = !ended_sessions.is_empty();
    drop(sessions);
    dispose_sessions(daemon, ended_sessions);
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
