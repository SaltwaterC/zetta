//! Replacing a running daemon with a newer build without dropping its panes.
//!
//! The outgoing daemon hands over its pty descriptors and the state needed to
//! rebuild its sessions; the incoming one adopts them and continues. Pane
//! readers are paused across the exchange so no output is read by a process
//! that is about to exit.

use super::*;

/// Why an upgrade did not happen, and whether the client has been told.
///
/// The answer is sent before the exec, because an exec never returns to send
/// one. Everything that fails after that point therefore must not be reported on
/// the connection again.
#[cfg(any(unix, windows))]
pub(super) enum UpgradeRefused {
    Before(anyhow::Error),
    #[cfg_attr(windows, allow(dead_code))]
    AfterAnswering(anyhow::Error),
}

/// Replaces this daemon with a fresh image of itself.
///
/// Returns only on failure, in which case this daemon carries on holding
/// everything it had: an upgrade that cannot be completed must not be a way to
/// lose the sessions it was meant to preserve.
#[cfg(unix)]
pub(super) fn upgrade_daemon(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    prepare_and_exec(daemon, connection)
}

#[cfg(unix)]
pub(super) fn prepare_and_exec(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    let prepared = prepare_upgrade(daemon).map_err(UpgradeRefused::Before)?;
    let (executable, file) = prepared;

    // Answered before the exec, because the exec never returns to answer. The
    // pre-flight has already established that the replacement can take over, so
    // this reports what was accepted rather than what completed.
    connection
        .send(&Response::Ok)
        .map_err(UpgradeRefused::Before)?;

    // Tell the clients this is deliberate, so the disconnect the exec is about
    // to cause reads as a replacement rather than a failure. Their terminals are
    // unaffected either way — in exclusive mode they hold the descriptors
    // themselves — but a client that knows can reconnect at once instead of
    // waiting out a backoff, and it never logs a lost multiplexer as a fault.
    // Best effort by nature: whether or not this lands, no client may treat a
    // lost subscription as its panes' processes ending.
    broadcast(daemon, &Event::Replacing);

    // Past this point the process becomes the replacement, so anything that
    // was going to fail had to have failed already.
    #[cfg(target_os = "macos")]
    let listener = None;
    #[cfg(not(target_os = "macos"))]
    let listener = Some(daemon.listener_fd);
    match crate::upgrade::exec_replacement(&executable, file.as_raw_fd(), listener) {
        Ok(_) => Ok(()),
        Err(error) => Err(UpgradeRefused::AfterAnswering(error)),
    }
}

/// Everything that can still be refused: resolving the image, checking it can
/// take over, and writing the handover.
#[cfg(unix)]
pub(super) fn prepare_upgrade(daemon: &Arc<Daemon>) -> Result<(PathBuf, std::fs::File)> {
    let executable = daemon
        .executable
        .clone()
        .context("this multiplexer does not know its own path, so it cannot be replaced")?;
    anyhow::ensure!(
        executable.is_file(),
        "{} no longer exists, so this multiplexer cannot replace itself with it. Its sessions \
         are untouched; rebuild or reinstall in place and try again.",
        executable.display()
    );
    anyhow::ensure!(
        crate::upgrade::replacement_accepts_handover(&executable)?,
        "{} cannot take over this multiplexer's sessions, so it was not started",
        executable.display()
    );

    #[cfg(feature = "session-persistence")]
    if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
        persistence
            .flush_segments()
            .context("flushing encrypted scrollback before replacing the daemon")?;
    }

    let sessions = daemon.sessions.lock().unwrap();
    let now = std::time::Instant::now();
    let handover = crate::upgrade::Handover {
        version: crate::upgrade::HANDOVER_VERSION,
        generation: daemon.catalog.lock().unwrap().runner_id(),
        next_session_id: daemon.next_session_id.load(Ordering::SeqCst),
        next_pane_id: daemon.next_pane_id.load(Ordering::SeqCst),
        sessions: sessions
            .iter()
            .map(|session| {
                crate::upgrade::SessionHandover {
                    id: session.id,
                    summary: session.summary.clone(),
                    state: session.state.clone(),
                    keep: session.keep,
                    offered: session.offered,
                    owner: session.owner,
                    verifier: session
                        .authentication
                        .as_ref()
                        .map(|authentication| authentication.verifier().to_owned()),
                    key_envelope: session.key_envelope.clone(),
                    failed_authentications: session.failed_authentications,
                    // A monotonic deadline means nothing in another image, so
                    // what is carried is how much of the window is left.
                    refuse_for: session
                        .refuse_until
                        .and_then(|until| until.checked_duration_since(now)),
                    panes: session
                        .panes
                        .iter()
                        .map(|pane| crate::upgrade::PaneHandover {
                            id: pane.id,
                            descriptor: pane.pty.file().as_raw_fd(),
                            child_pid: pane.pty.child_pid(),
                            attachment: attachment_handover(&pane.attachment),
                            columns: pane.size.columns,
                            lines: pane.size.lines,
                            exited: pane.exited,
                            exit_status: pane.exit_status,
                            // Copied, not taken. The exec is irreversible but
                            // everything before it is not, and emptying the live
                            // ring here meant a refused upgrade had already
                            // destroyed the output it was protecting.
                            retained: pane.retained.snapshot(),
                        })
                        .collect(),
                }
            })
            .collect(),
    };

    // Only the terminals. The `SIGCHLD` pipes are deliberately left
    // close-on-exec: the replacement registers its own, and a carried one is
    // worse than useless — see `PaneHandover::descriptor`.
    for session in sessions.iter() {
        for pane in &session.panes {
            crate::upgrade::keep_across_exec(pane.pty.file())?;
        }
    }
    // Keep the listener together with the PTY masters where the platform
    // preserves its local peer-credential state across exec. macOS has to
    // rebind: inheriting a Darwin local listener makes every new connection
    // fail the credential lookup in the replacement.
    #[cfg(not(target_os = "macos"))]
    {
        let listener = unsafe { BorrowedFd::borrow_raw(daemon.listener_fd) };
        crate::upgrade::keep_across_exec(&listener)?;
    }
    let file = crate::upgrade::write_handover(&handover)?;
    drop(sessions);
    Ok((executable, file))
}

#[cfg(windows)]
pub(super) fn upgrade_daemon(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    let (executable, handover, ready) = prepare_upgrade(daemon).map_err(UpgradeRefused::Before)?;
    let mut replacement = match crate::upgrade::spawn_replacement(&executable, &handover, &ready) {
        Ok(child) => child,
        Err(error) => {
            crate::upgrade::remove_handover(&handover, &ready);
            return Err(UpgradeRefused::Before(error));
        }
    };
    if let Err(error) = crate::upgrade::wait_for_ready(&mut replacement, &ready) {
        let _ = replacement.kill();
        crate::upgrade::remove_handover(&handover, &ready);
        return Err(UpgradeRefused::Before(error));
    }
    if let Err(error) = connection.send(&Response::Ok) {
        let _ = replacement.kill();
        crate::upgrade::remove_handover(&handover, &ready);
        return Err(UpgradeRefused::Before(error));
    }
    broadcast(daemon, &Event::Replacing);
    daemon.running.store(false, Ordering::SeqCst);
    let _ = Stream::connect(socket_path(&session_catalog_dir()));
    Ok(())
}

#[cfg(windows)]
pub(super) fn prepare_upgrade(daemon: &Arc<Daemon>) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let executable = daemon
        .executable
        .clone()
        .context("this multiplexer does not know its own path, so it cannot be replaced")?;
    anyhow::ensure!(
        executable.is_file(),
        "{} no longer exists, so this multiplexer cannot replace itself with it. Its sessions \
         are untouched; rebuild or reinstall in place and try again.",
        executable.display()
    );
    anyhow::ensure!(
        crate::upgrade::replacement_accepts_handover(&executable)?,
        "{} cannot take over this multiplexer's sessions, so it was not started",
        executable.display()
    );

    let sessions = daemon.sessions.lock().unwrap();
    let now = Instant::now();
    let handover = crate::upgrade::Handover {
        version: crate::upgrade::HANDOVER_VERSION,
        generation: daemon.catalog.lock().unwrap().runner_id(),
        next_session_id: daemon.next_session_id.load(Ordering::SeqCst),
        next_pane_id: daemon.next_pane_id.load(Ordering::SeqCst),
        retention: *daemon.retention.lock().unwrap(),
        sessions: sessions
            .iter()
            .map(|session| crate::upgrade::SessionHandover {
                id: session.id,
                summary: session.summary.clone(),
                state: session.state.clone(),
                keep: session.keep,
                offered: session.offered,
                owner: session.owner,
                verifier: session
                    .authentication
                    .as_ref()
                    .map(|authentication| authentication.verifier().to_owned()),
                key_envelope: session.key_envelope.clone(),
                failed_authentications: session.failed_authentications,
                refuse_for: session
                    .refuse_until
                    .and_then(|until| until.checked_duration_since(now)),
                panes: session
                    .panes
                    .iter()
                    .map(|pane| crate::upgrade::PaneHandover {
                        id: pane.id,
                        console_id: pane.console_id,
                        child_pid: pane.pty.child_pid(),
                        attachment: attachment_handover(&pane.attachment),
                        columns: pane.size.columns,
                        lines: pane.size.lines,
                        exited: pane.exited,
                        exit_status: pane.exit_status,
                        retained: pane.retained.snapshot(),
                    })
                    .collect(),
            })
            .collect(),
    };
    drop(sessions);
    validate_handover_with_host(&daemon.pty_host, &handover)?;
    let (handover, ready) = crate::upgrade::write_handover(&session_catalog_dir(), &handover)?;
    Ok((executable, handover, ready))
}

/// A pane's attachment in the form that crosses an exec.
#[cfg(any(unix, windows))]
pub(super) fn attachment_handover(attachment: &Attachment) -> crate::upgrade::AttachmentHandover {
    use crate::upgrade::{AttachmentHandover, SharedClientHandover};
    match attachment {
        Attachment::None => AttachmentHandover::None,
        Attachment::Exclusive(holder) => AttachmentHandover::Exclusive { holder: *holder },
        Attachment::Revoking { holder } => AttachmentHandover::Revoking { holder: *holder },
        // A grant in flight cannot survive: the descriptor was going out on a
        // connection the exec closes. It comes back as shared with nobody, which
        // is what every shared pane comes back as, and the client re-attaches —
        // whereupon it is alone again and offered the grant afresh.
        Attachment::Granting { .. } => AttachmentHandover::Shared {
            clients: Vec::new(),
        },
        Attachment::Shared(clients) => AttachmentHandover::Shared {
            clients: clients
                .iter()
                .map(|client| SharedClientHandover {
                    process_id: client.process_id,
                    client_id: client.client_id.clone(),
                    stream_only: client.stream_only,
                    columns: client.columns,
                    lines: client.lines,
                    input_sent: client.input_sent,
                })
                .collect(),
        },
    }
}

/// Takes over the sessions the previous image left behind.
///
/// The descriptors are the same ones it held: the `execv` kept them open, so a
/// pane is rebuilt around its existing terminal rather than being restarted.
/// The identifiers a resumed multiplexer must continue from.
///
/// Kept separate so the rule is testable without a live handover: never at or
/// below anything already held, whatever the handover claims.
#[cfg(any(unix, windows))]
pub(super) fn next_ids_after(
    session_ids: &[u64],
    pane_ids: &[u64],
    handover_session: u64,
    handover_pane: u64,
) -> (u64, u64) {
    let next_session = session_ids
        .iter()
        .map(|id| id + 1)
        .chain(std::iter::once(handover_session))
        .max()
        .unwrap_or(1);
    let next_pane = pane_ids
        .iter()
        .map(|id| id + 1)
        .chain(std::iter::once(handover_pane))
        .max()
        .unwrap_or(1);
    (next_session, next_pane)
}

/// Restores a pane's attachment from what crossed the exec.
///
/// Shared clients come back with their sizes and attribution but no
/// connections — those cannot be carried — so the pane stays shared and each
/// viewer rejoins the relay through a fresh attach. It must stay shared: coming
/// back exclusive-capable would hand the descriptor to whichever viewer
/// reconnected first, and the rest would be reading a terminal nobody was
/// relaying.
#[cfg(any(unix, windows))]
pub(super) fn adopt_attachment(attachment: crate::upgrade::AttachmentHandover) -> Attachment {
    use crate::upgrade::AttachmentHandover;
    match attachment {
        AttachmentHandover::None => Attachment::None,
        AttachmentHandover::Exclusive { holder } => Attachment::Exclusive(holder),
        // Still mid-handover: the holder has the descriptor and its snapshot is
        // still expected, so nothing here may read the pane. Collapsing this to
        // "no holder" let the drain thread start reading a terminal the holder
        // was still reading, and made the holder's snapshot arrive at a pane
        // that was no longer being handed over — so the handover failed and left
        // that pane inert.
        AttachmentHandover::Revoking { holder } => Attachment::Revoking { holder },
        AttachmentHandover::Shared { clients: _ } => Attachment::Shared(Vec::new()),
    }
}

#[cfg(unix)]
pub(super) fn adopt_handover(
    daemon: &Arc<Daemon>,
    handover: crate::upgrade::Handover,
) -> Result<usize> {
    use std::os::fd::FromRawFd as _;

    let retention = *daemon.retention.lock().unwrap();
    let mut sessions = daemon.sessions.lock().unwrap();
    let now = std::time::Instant::now();
    for session in handover.sessions {
        // Adopting one twice would leave the multiplexer holding two entries
        // that no client could tell apart.
        if sessions.iter().any(|existing| existing.id == session.id) {
            log::warn!("ignoring session {}, which is already held", session.id);
            continue;
        }
        // The session's own id is authoritative. A previous image could have
        // been handed a summary whose id disagreed with it (see `detach`), and
        // letting that survive the upgrade would keep the session listed under
        // one id while being looked up under another. Normalizing here is what
        // lets an upgrade repair a session that had been corrupted.
        let mut summary = session.summary;
        summary.id = session.id;
        let mut panes = Vec::new();
        for pane in session.panes {
            anyhow::ensure!(
                crate::upgrade::descriptor_is_open(pane.descriptor),
                "pane {}'s terminal did not survive the upgrade",
                pane.id
            );
            // SAFETY: checked open just above; it was kept across the exec by
            // the previous image, and is claimed here exactly once.
            let descriptor = unsafe { std::os::fd::OwnedFd::from_raw_fd(pane.descriptor) };
            // Reclaimed regardless of who was holding it. An `execv` does not
            // change parentage, so every one of these shells is still this
            // process's own child and is reaped here with `waitpid` — which is
            // the whole reason the upgrade is a re-exec rather than a fresh
            // process. Treating a held pane as though its child belonged to
            // somebody else meant the daemon never reaped it (a zombie, and no
            // exit ever reported) and read a dead pipe as though the process had
            // ended (a false exit for a shell that was running fine).
            let pty = tty::reclaim(descriptor, pane.child_pid)?;
            // Rebuilt at the size the pane is running at, then given the screen
            // the previous image serialized for it.
            let mut retained = retention.new_retained(pane.columns, pane.lines);
            retained.seed(pane.retained);
            panes.push(Pane {
                id: pane.id,
                pty,
                attachment: adopt_attachment(pane.attachment),
                // The size the pane is actually running at, carried across so
                // arbitration continues from what its viewers are showing.
                // Restarting from a default silently resized every adopted pane.
                size: TerminalSize {
                    columns: pane.columns,
                    lines: pane.lines,
                    cell_width: 0,
                    cell_height: 0,
                },
                retained,
                // An upgrade cannot land mid-handover: the client that would be
                // waiting for one is the client that asked for the upgrade.
                handed_over: None,
                handover_waiters: 0,
                exited: pane.exited,
                exit_status: pane.exit_status,
                pending_input: Vec::new(),
            });
        }
        sessions.push(Session {
            id: session.id,
            summary,
            state: session.state,
            authentication: session
                .verifier
                .map(SessionAuthentication::from_verifier)
                .transpose()?,
            key_envelope: session.key_envelope,
            failed_authentications: session.failed_authentications,
            // Restored as a deadline again, from what was left of the window.
            refuse_until: session.refuse_for.and_then(|left| now.checked_add(left)),
            panes,
            keep: session.keep,
            offered: session.offered,
            owner: session.owner,
        });
    }
    // Derived from what was actually adopted, not merely taken from the
    // handover. A counter that lands at or below an existing identifier makes
    // the next spawn reuse it, and two sessions sharing an id are
    // indistinguishable to every client: they list identically, and attaching
    // one is a coin toss. Trusting the handover alone let exactly that happen.
    let session_ids = sessions
        .iter()
        .map(|session| session.id)
        .collect::<Vec<_>>();
    let pane_ids = sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    let (next_session_id, next_pane_id) = next_ids_after(
        &session_ids,
        &pane_ids,
        handover.next_session_id,
        handover.next_pane_id,
    );
    daemon
        .next_session_id
        .store(next_session_id, Ordering::SeqCst);
    daemon.next_pane_id.store(next_pane_id, Ordering::SeqCst);
    Ok(sessions.len())
}

#[cfg(windows)]
pub(super) fn adopt_handover(
    daemon: &Arc<Daemon>,
    handover: crate::upgrade::Handover,
) -> Result<usize> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let now = Instant::now();
    for session in handover.sessions {
        if sessions.iter().any(|existing| existing.id == session.id) {
            log::warn!("ignoring session {}, which is already held", session.id);
            continue;
        }
        let mut summary = session.summary;
        summary.id = session.id;
        let mut panes = Vec::new();
        for pane in session.panes {
            let (child_pid, handles) = daemon
                .pty_host
                .handles(pane.console_id, std::process::id())?;
            anyhow::ensure!(
                pane.child_pid == 0 || child_pid == 0 || pane.child_pid == child_pid,
                "pseudoconsole {} changed children during the upgrade",
                pane.console_id
            );
            let mut handles = crate::transport::claim_duplicated(&handles);
            anyhow::ensure!(
                handles.len() == 2,
                "pseudoconsole {} did not return two pipe handles",
                pane.console_id
            );
            let conin = handles.remove(1);
            let conout = handles.remove(0);
            let (pty, child_events) = tty::attach(conout, conin, child_pid)
                .context("attaching an adopted pseudoconsole to the daemon")?;
            let retention = *daemon.retention.lock().unwrap();
            let mut retained = retention.new_retained(pane.columns, pane.lines);
            retained.seed(pane.retained);
            panes.push(Pane {
                id: pane.id,
                pty,
                console_id: pane.console_id,
                child_events,
                attachment: adopt_attachment(pane.attachment),
                size: TerminalSize {
                    columns: pane.columns,
                    lines: pane.lines,
                    cell_width: 0,
                    cell_height: 0,
                },
                retained,
                handed_over: None,
                handover_waiters: 0,
                exited: pane.exited,
                exit_status: pane.exit_status,
                pending_input: Vec::new(),
            });
        }
        sessions.push(Session {
            id: session.id,
            summary,
            state: session.state,
            authentication: session
                .verifier
                .map(SessionAuthentication::from_verifier)
                .transpose()?,
            key_envelope: session.key_envelope,
            failed_authentications: session.failed_authentications,
            refuse_until: session.refuse_for.and_then(|left| now.checked_add(left)),
            panes,
            keep: session.keep,
            offered: session.offered,
            owner: session.owner,
        });
    }
    let session_ids = sessions
        .iter()
        .map(|session| session.id)
        .collect::<Vec<_>>();
    let pane_ids = sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    let (next_session_id, next_pane_id) = next_ids_after(
        &session_ids,
        &pane_ids,
        handover.next_session_id,
        handover.next_pane_id,
    );
    daemon
        .next_session_id
        .store(next_session_id, Ordering::SeqCst);
    daemon.next_pane_id.store(next_pane_id, Ordering::SeqCst);
    Ok(sessions.len())
}

/// Reaps children and tells whoever is holding their terminals.
///
/// Windows has no `SIGCHLD`: each PTY carries its own exit watcher, which
/// signals when its process ends, so this polls those watchers rather than
/// waiting on a signal.
#[cfg(windows)]
pub(super) fn start_reaper(daemon: Arc<Daemon>) -> Result<()> {
    spawn_worker("zmux reaper", move || {
        let daemon = daemon.clone();
        Box::new(move || windows_reaper_loop(daemon))
    });
    Ok(())
}

#[cfg(windows)]
pub(super) fn windows_reaper_loop(daemon: Arc<Daemon>) {
    loop {
        match daemon.pty_host.reap() {
            Ok(exits) if !exits.is_empty() => {
                let mut sessions = daemon.sessions.lock().unwrap();
                for exit in exits {
                    let Some(pane) = sessions
                        .iter_mut()
                        .flat_map(|session| session.panes.iter_mut())
                        .find(|pane| pane.console_id == exit.console_id)
                    else {
                        continue;
                    };
                    let result = match exit.exit_code {
                        Some(code) => pane.child_events.report_exit(code),
                        None => pane.child_events.report_status_unavailable(),
                    };
                    if let Err(error) = result {
                        log::debug!(
                            "could not report exit for pseudoconsole {}: {error}",
                            exit.console_id
                        );
                    }
                }
            }
            Ok(_) => {}
            Err(error) => log::warn!("could not collect pseudoconsole exits: {error:#}"),
        }
        let mut exits = Vec::new();
        {
            let mut sessions = daemon.sessions.lock().unwrap();
            for session in sessions.iter_mut() {
                for pane in session.panes.iter_mut() {
                    if pane.exited {
                        continue;
                    }
                    if let Some(exit) = observe_pane_exit(session.id, pane) {
                        exits.push(exit);
                    }
                }
            }
        }
        for (session_id, pane_id, raw_status, input_sent) in exits {
            broadcast(
                &daemon,
                &Event::PaneExited {
                    session_id,
                    pane_id,
                    raw_status,
                    input_sent,
                },
            );
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// How a terminal reaches the client, which differs by platform.
///
/// Unix attaches the descriptor to the message itself. Windows has no such
/// mechanism, so the handles are duplicated into the client's process first
/// and travel in the message as plain values.
pub(super) struct Handover {
    #[cfg(unix)]
    pub(super) attachments: Vec<std::os::fd::BorrowedFd<'static>>,
    #[cfg(windows)]
    pub(super) attachments: Vec<()>,
    pub(super) values: Vec<i64>,
}

#[cfg(unix)]
pub(super) fn handover_handles(
    _daemon: &Daemon,
    pane: &Pane,
    _client_process_id: u32,
) -> Result<Handover> {
    // SAFETY: the borrow lives only until the message has been sent, well
    // inside the lifetime of the PTY the daemon is holding.
    let descriptor = unsafe {
        std::os::fd::BorrowedFd::borrow_raw(std::os::fd::AsRawFd::as_raw_fd(pane.pty.file()))
    };
    Ok(Handover {
        attachments: vec![descriptor],
        values: Vec::new(),
    })
}

#[cfg(windows)]
pub(super) fn handover_handles(
    daemon: &Daemon,
    pane: &Pane,
    client_process_id: u32,
) -> Result<Handover> {
    anyhow::ensure!(
        client_process_id != 0,
        "the client did not say which process to hand its terminal to"
    );
    let (_, values) = daemon
        .pty_host
        .handles(pane.console_id, client_process_id)?;
    Ok(Handover {
        attachments: Vec::new(),
        values,
    })
}

/// Stops the Windows daemon reader at the same boundary at which the console
/// handles are handed to an exclusive client. The reader's internal pipe may
/// already contain bytes after its last drain pass, so retain and relay those
/// bytes before the descriptor changes owner.
#[cfg(windows)]
pub(super) fn pause_pane_reader(pane: &mut Pane) -> Result<()> {
    pane.pty
        .pause_reader()
        .context("pausing the Windows pseudoconsole reader")?;
    let mut buffer = [0; 16 * 1024];
    loop {
        let read = pane.pty.read_buffered(&mut buffer);
        if read == 0 {
            break;
        }
        pane.retained.push(&buffer[..read]);
        record_handover_output(pane, &buffer[..read]);
        relay_output(pane, &buffer[..read]);
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn pause_pane_reader(_pane: &mut Pane) -> Result<()> {
    Ok(())
}

/// Reads whatever a detached pane has produced.
///
/// The two platforms expose different things to read: a Unix PTY is one
/// descriptor, while a pseudoconsole is an unblocked pipe with its own buffer.
#[cfg(unix)]
pub(super) fn read_pane(pty: &mut tty::Pty, buffer: &mut [u8]) -> std::io::Result<usize> {
    pty.file().read(buffer)
}

#[cfg(windows)]
pub(super) fn read_pane(pty: &mut tty::Pty, buffer: &mut [u8]) -> std::io::Result<usize> {
    use alacritty_terminal::tty::EventedReadWrite as _;
    Ok(pty.reader().try_read(buffer))
}

/// This process's own executable, if it can be established.
///
/// Called once, at startup, so the answer predates any rebuild. A path Linux
/// has marked `(deleted)` is rejected rather than repaired: the file it named
/// is gone, and guessing at which file replaced it is how a multiplexer ends
/// up executing something the user did not install.
pub(super) fn resolve_own_executable() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    #[cfg(unix)]
    let path = current;
    #[cfg(windows)]
    let path = current
        .parent()
        .map(|directory| directory.join("zmux.exe"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or(current);
    if path.as_os_str().as_encoded_bytes().ends_with(b" (deleted)") {
        log::warn!(
            "this multiplexer's executable was already replaced at startup, so it cannot be \
             upgraded in place"
        );
        return None;
    }
    path.is_file().then_some(path)
}

#[cfg(all(test, unix))]
#[path = "../tests/server/upgrade.rs"]
mod tests;
