//! Replacing a running daemon with a newer build without dropping its panes.
//!
//! The outgoing daemon hands over its pty descriptors and the state needed to
//! rebuild its sessions; the incoming one adopts them and continues. Pane
//! readers are paused across the exchange so no output is read by a process
//! that is about to exit.

use super::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(super) use unix::*;
#[cfg(windows)]
mod windows_handover;
#[cfg(windows)]
pub(super) use windows_handover::*;

/// Why an upgrade did not happen, and whether the client has been told.
///
/// The answer is sent before the exec, because an exec never returns to send
/// one. Everything that fails after that point therefore must not be reported on
/// the connection again.
#[cfg(any(unix, windows))]
pub(super) enum UpgradeRefused {
    Before(anyhow::Error),
    #[cfg_attr(
        windows,
        expect(dead_code, reason = "only the Unix exec can fail after answering")
    )]
    AfterAnswering(anyhow::Error),
}

/// A pane's attachment in the form that crosses an exec.
#[cfg(any(unix, windows))]
fn attachment_handover(attachment: &Attachment) -> crate::upgrade::AttachmentHandover {
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
                    columns: client.size.map(|(columns, _)| columns),
                    lines: client.size.map(|(_, lines)| lines),
                    input_sent: client.input_sent,
                })
                .collect(),
        },
    }
}

/// The image that takes over: this daemon's own executable, refused while it
/// is missing or cannot adopt a handover, so a refused upgrade has touched
/// nothing.
#[cfg(any(unix, windows))]
fn replacement_executable(daemon: &Daemon) -> Result<PathBuf> {
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
    Ok(executable)
}

/// What every upgrade settles before taking its handover: the image that
/// takes over, and the scrollback still buffered in memory, written out.
///
/// The flush has to come before the replacement exists. From then on the
/// persistence directory is the replacement's to write, and a flush from this
/// daemon would also rewrite the manifest under it — which a Windows daemon,
/// still running beside its replacement, used to do on its way out (see
/// `shut_down`). Output buffered after this point is not written to disk,
/// which is also what a Unix `exec` does with it. What this daemon reads
/// before the handover snapshots each pane's screen still reaches the
/// replacement in that snapshot, and what it has not read yet waits in the
/// terminal for the replacement; only what it reads after the snapshot misses
/// the replacement's screen, as it always has on both platforms.
#[cfg(any(unix, windows))]
pub(super) fn prepare_replacement(daemon: &Daemon) -> Result<PathBuf> {
    let executable = replacement_executable(daemon)?;
    #[cfg(feature = "session-persistence")]
    if let Some(persistence) = daemon.persistence.lock().as_mut() {
        persistence
            .flush_segments()
            .context("flushing encrypted scrollback before replacing the daemon")?;
    }
    Ok(executable)
}

/// The handover of `sessions`, with the counters and policy the replacement
/// continues from. The lifetime lock is left for the caller to attach, and on
/// Windows each pane's agent pipe, which needs the replacement to exist.
#[cfg(any(unix, windows))]
pub(super) fn handover_of(daemon: &Daemon, sessions: &mut [Session]) -> crate::upgrade::Handover {
    let now = std::time::Instant::now();
    crate::upgrade::Handover {
        version: crate::upgrade::HANDOVER_VERSION,
        generation: daemon.catalog.lock().recover_poison().runner_id(),
        next_session_id: daemon.next_session_id.load(Ordering::SeqCst),
        next_pane_id: daemon.next_pane_id.load(Ordering::SeqCst),
        retention: *daemon.retention.lock().recover_poison(),
        recipient_grants: Some(lock_grants(daemon).clone()),
        lifetime_lock: None,
        sessions: sessions
            .iter_mut()
            .map(|session| crate::upgrade::SessionHandover {
                id: session.id,
                summary: session.summary.clone(),
                state: session.state.clone(),
                shared_state: session.shared_state.clone(),
                keep: session.keep,
                offered: session.offered,
                owner: session.owner.map(|owner| owner.pid),
                owner_started: session.owner.and_then(|owner| owner.started),
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
                sealed_to: session.sealed_to.clone(),
                panes: session.panes.iter_mut().map(pane_handover).collect(),
            })
            .collect(),
    }
}

/// One pane's part of the handover: its terminal as the platform carries it,
/// and everything the replacement rebuilds the pane around.
#[cfg(any(unix, windows))]
fn pane_handover(pane: &mut Pane) -> crate::upgrade::PaneHandover {
    let (columns, lines, retained) = snapshot_for_upgrade(pane);
    crate::upgrade::PaneHandover {
        id: pane.id,
        #[cfg(unix)]
        descriptor: std::os::fd::AsRawFd::as_raw_fd(pane.pty.file()),
        #[cfg(windows)]
        console_id: pane.console_id,
        child_pid: pane.pty.child_pid(),
        attachment: attachment_handover(&pane.attachment),
        attachment_client_id: pane.attachment_client_id.clone(),
        columns,
        lines,
        exited: pane.exited,
        exit_status: pane.exit_status,
        // Copied, not taken. Nothing before the replacement takes over is
        // irreversible, and emptying the live ring here meant a refused
        // upgrade had already destroyed the output it was protecting.
        retained,
        // Filled in once the replacement exists to hold an instance:
        // `hand_over_agent_pipes`.
        #[cfg(windows)]
        agent_pipe: None,
        #[cfg(windows)]
        agent_pipe_instance: None,
    }
}

/// A pane's terminal as the replacement reclaimed it, which is the part of
/// adopting a pane that differs by platform.
#[cfg(any(unix, windows))]
pub(super) struct AdoptedTerminal {
    pub(super) pty: tty::Pty,
    #[cfg(windows)]
    pub(super) child_events: tty::AttachedChildEvents,
}

/// Takes over the sessions the previous daemon handed over.
///
/// `claim` reclaims one pane's terminal; a failure abandons the adoption, as
/// the replacement cannot serve a session it holds only part of. `release`
/// lets go of what a session carried that is skipped because one with its id
/// is already held: adopting it twice would leave the multiplexer holding two
/// entries that no client could tell apart.
#[cfg(any(unix, windows))]
pub(super) fn adopt_sessions(
    daemon: &Daemon,
    handover: crate::upgrade::Handover,
    mut claim: impl FnMut(&crate::upgrade::PaneHandover) -> Result<AdoptedTerminal>,
    mut release: impl FnMut(&crate::upgrade::SessionHandover),
) -> Result<usize> {
    let retention = *daemon.retention.lock().recover_poison();
    let mut sessions = daemon.sessions();
    let now = std::time::Instant::now();
    for mut session in handover.sessions {
        if sessions.iter().any(|existing| existing.id == session.id) {
            log::warn!("ignoring session {}, which is already held", session.id);
            release(&session);
            continue;
        }
        let panes = std::mem::take(&mut session.panes)
            .into_iter()
            .map(|pane| {
                let terminal = claim(&pane)?;
                Ok(adopted_pane(retention, pane, terminal))
            })
            .collect::<Result<Vec<_>>>()?;
        sessions.push(adopted_session(session, panes, now)?);
    }
    continue_ids_after(
        daemon,
        &sessions,
        handover.next_session_id,
        handover.next_pane_id,
    );
    Ok(sessions.len())
}

/// A pane rebuilt from its handover around the terminal the platform reclaimed
/// for it.
#[cfg(any(unix, windows))]
fn adopted_pane(
    retention: crate::retention::Retention,
    pane: crate::upgrade::PaneHandover,
    terminal: AdoptedTerminal,
) -> Pane {
    // Rebuilt at the size the pane is running at, then given the screen the
    // previous image serialized for it.
    let mut retained = retention.new_retained(pane.columns, pane.lines);
    retained.seed(pane.retained);
    Pane {
        id: pane.id,
        pty: terminal.pty,
        #[cfg(windows)]
        console_id: pane.console_id,
        #[cfg(windows)]
        child_events: terminal.child_events,
        attachment: adopt_attachment(pane.attachment),
        attachment_client_id: pane.attachment_client_id,
        // The size the pane is actually running at, carried across so
        // arbitration continues from what its viewers are showing. Restarting
        // from a default silently resized every adopted pane.
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
        clipboard_scanner: zclip::protocol::Scanner::default(),
    }
}

/// A session rebuilt from its handover around `panes`, the ones adopted from
/// it. `now` is when adoption started, which the rate-limit window resumes
/// from.
#[cfg(any(unix, windows))]
fn adopted_session(
    session: crate::upgrade::SessionHandover,
    panes: Vec<Pane>,
    now: std::time::Instant,
) -> Result<Session> {
    // The session's own id is authoritative. A previous image could have been
    // handed a summary whose id disagreed with it (see `detach`), and letting
    // that survive the upgrade would keep the session listed under one id
    // while being looked up under another. Normalizing here is what lets an
    // upgrade repair a session that had been corrupted.
    let mut summary = session.summary;
    summary.id = session.id;
    Ok(Session {
        id: session.id,
        summary,
        state: session.state,
        shared_state: session.shared_state,
        authentication: session
            .verifier
            .map(SessionAuthentication::adopt_verifier)
            .transpose()?,
        key_envelope: session.key_envelope,
        failed_authentications: session.failed_authentications,
        // Restored as a deadline again, from what was left of the window.
        refuse_until: session.refuse_for.and_then(|left| now.checked_add(left)),
        panes,
        keep: session.keep,
        offered: session.offered,
        owner: adopt_owner(session.owner, session.owner_started),
        sealed_to: session.sealed_to,
    })
}

/// Moves the identifier counters past every session and pane now held.
///
/// Derived from what was actually adopted, not merely taken from the handover.
/// A counter that lands at or below an existing identifier makes the next
/// spawn reuse it, and two sessions sharing an id are indistinguishable to
/// every client: they list identically, and attaching one is a coin toss.
/// Trusting the handover alone let exactly that happen.
#[cfg(any(unix, windows))]
fn continue_ids_after(
    daemon: &Daemon,
    sessions: &[Session],
    handover_session: u64,
    handover_pane: u64,
) {
    let session_ids = sessions
        .iter()
        .map(|session| session.id)
        .collect::<Vec<_>>();
    let pane_ids = sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    let (next_session_id, next_pane_id) =
        next_ids_after(&session_ids, &pane_ids, handover_session, handover_pane);
    daemon
        .next_session_id
        .store(next_session_id, Ordering::SeqCst);
    daemon.next_pane_id.store(next_pane_id, Ordering::SeqCst);
}

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
fn adopt_attachment(attachment: crate::upgrade::AttachmentHandover) -> Attachment {
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

/// Preserves the owner's recorded identity, including an unavailable start time.
#[cfg(any(unix, windows))]
pub(super) fn adopt_owner(owner: Option<u32>, started: Option<u64>) -> Option<ProcessIdentity> {
    Some(ProcessIdentity {
        pid: owner?,
        started,
    })
}

/// How a terminal reaches the client, which differs by platform.
///
/// Unix attaches the descriptor to the message itself. Windows has no such
/// mechanism, so the handles are duplicated into the client's process first
/// and travel in the message as plain values.
pub(super) struct Handover {
    pub(super) attachments: Attachments,
    pub(super) values: Vec<i64>,
}

/// What travels attached to the message: on Unix the descriptors, owned
/// rather than borrowed from the pane, because the message is sent after the
/// sessions lock is dropped, when nothing stops the pane from going.
#[derive(Default)]
pub(super) struct Attachments(#[cfg(unix)] pub(super) Vec<std::os::fd::OwnedFd>);

impl Attachments {
    /// What [`Connection::send_with`](crate::transport::Connection::send_with)
    /// takes.
    #[cfg(unix)]
    pub(super) fn borrowed(&self) -> Vec<std::os::fd::BorrowedFd<'_>> {
        self.0.iter().map(std::os::fd::AsFd::as_fd).collect()
    }

    #[cfg(windows)]
    pub(super) fn borrowed(&self) -> [(); 0] {
        []
    }
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
