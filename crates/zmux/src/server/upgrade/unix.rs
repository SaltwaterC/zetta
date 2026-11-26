//! The Unix upgrade: the daemon re-executes itself, carrying its listener
//! and every pane's descriptors across `exec`, and the new image adopts them.

use super::*;

/// Replaces this daemon with a fresh image of itself.
///
/// Returns only on failure, in which case this daemon carries on holding
/// everything it had: an upgrade that cannot be completed must not be a way to
/// lose the sessions it was meant to preserve.
pub(in crate::server) fn upgrade_daemon(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    prepare_and_exec(daemon, connection)
}

pub(in crate::server) fn prepare_and_exec(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    let prepared = prepare_upgrade(daemon).map_err(UpgradeRefused::Before)?;

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
    Err(UpgradeRefused::AfterAnswering(exec_carrying(
        daemon, &prepared,
    )))
}

/// What [`prepare_upgrade`] settled on, for [`exec_carrying`] to carry out.
pub(in crate::server) struct PreparedUpgrade {
    pub(super) executable: PathBuf,
    /// The handover, still close-on-exec.
    pub(super) handover: std::fs::File,
    /// Every pane the handover names, by id and by the descriptor it names.
    /// A pane that has closed since is not carried, and neither is one that
    /// started since, even if it was given a closed pane's descriptor number.
    pub(super) panes: HashSet<(u64, RawFd)>,
}

/// Makes everything the handover names inheritable and replaces this image,
/// returning only if that could not be done — with every descriptor
/// close-on-exec again.
///
/// No pane starts from clearing the first flag to the `execv`: it would
/// inherit every pane's terminal, the listener and the handover.
pub(super) fn exec_carrying(daemon: &Daemon, prepared: &PreparedUpgrade) -> anyhow::Error {
    let _no_spawns = daemon.spawn_gate.write().recover_poison();
    let sessions = daemon.sessions();
    let lifetime_lock = daemon.lifetime_lock.lock().recover_poison();
    let panes = sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .filter(|pane| {
            prepared
                .panes
                .contains(&(pane.id, pane.pty.file().as_raw_fd()))
        })
        .map(|pane| pane.pty.file());
    // Keep the listener together with the PTY masters where the platform
    // preserves its local peer-credential state across exec. macOS has to
    // rebind: inheriting a Darwin local listener makes every new connection
    // fail the credential lookup in the replacement.
    #[cfg(target_os = "macos")]
    let listener = None;
    // SAFETY: the daemon's listener stays open for the daemon's lifetime, and
    // this borrow ends with this function.
    #[cfg(not(target_os = "macos"))]
    let listener = Some(unsafe { BorrowedFd::borrow_raw(daemon.listener_fd) });
    // If the lock cannot be carried, the replacement takes it itself.
    let lock = lifetime_lock.as_ref().map(LifetimeLock::as_fd);
    let carried = panes
        .map(std::os::fd::AsFd::as_fd)
        .chain(listener)
        .chain(lock)
        .chain(std::iter::once(prepared.handover.as_fd()));
    let _inherited = match crate::upgrade::InheritedAcrossExec::clear(carried) {
        Ok(inherited) => inherited,
        Err(error) => return error.context("keeping descriptors open for the replacement"),
    };
    match crate::upgrade::exec_replacement(
        &prepared.executable,
        prepared.handover.as_raw_fd(),
        listener.map(|listener| listener.as_raw_fd()),
    ) {
        Ok(never) => match never {},
        Err(error) => error,
    }
}

/// Everything that can still be refused: resolving the image, checking it can
/// take over, and writing the handover.
pub(in crate::server) fn prepare_upgrade(daemon: &Arc<Daemon>) -> Result<PreparedUpgrade> {
    let executable = prepare_replacement(daemon)?;
    let mut sessions = daemon.sessions();
    let mut handover = handover_of(daemon, &mut sessions);
    handover.lifetime_lock = held_lock(daemon, |lock| lock.raw_fd());

    drop(sessions);
    // Only the terminals. The `SIGCHLD` pipes are deliberately left
    // close-on-exec: the replacement registers its own, and a carried one is
    // worse than useless — see `PaneHandover::descriptor`.
    let panes = handover
        .sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .map(|pane| (pane.id, pane.descriptor))
        .collect();
    let handover = crate::upgrade::write_handover(&handover)?;
    Ok(PreparedUpgrade {
        executable,
        handover,
        panes,
    })
}

/// Takes over the sessions the previous image left behind.
///
/// The descriptors are the same ones it held: the `execv` kept them open, so a
/// pane is rebuilt around its existing terminal rather than being restarted.
pub(in crate::server) fn adopt_handover(
    daemon: &Arc<Daemon>,
    handover: crate::upgrade::Handover,
) -> Result<usize> {
    adopt_sessions(
        daemon,
        handover,
        |pane| {
            // SAFETY: it was kept across the exec by the previous image, and is
            // claimed here exactly once.
            let descriptor = unsafe { crate::upgrade::claim_inherited_fd(pane.descriptor) }
                .with_context(|| {
                    format!("pane {}'s terminal did not survive the upgrade", pane.id)
                })?;
            // Reclaimed regardless of who was holding it. An `execv` does not
            // change parentage, so every one of these shells is still this
            // process's own child and is reaped here with `waitpid` — which is
            // the whole reason the upgrade is a re-exec rather than a fresh
            // process. Treating a held pane as though its child belonged to
            // somebody else meant the daemon never reaped it (a zombie, and no
            // exit ever reported) and read a dead pipe as though the process had
            // ended (a false exit for a shell that was running fine).
            let pty = tty::reclaim(descriptor, pane.child_pid)?;
            Ok(AdoptedTerminal { pty })
        },
        // A skipped session's descriptors are left open, as they always were:
        // claiming one is what takes ownership of it.
        |_| {},
    )
}

pub(in crate::server) fn handover_handles(
    _daemon: &Daemon,
    pane: &Pane,
    _client_process_id: u32,
) -> Result<Handover> {
    // A duplicate, close-on-exec like the original, taken while the caller
    // still holds the sessions lock that keeps the pane alive.
    let descriptor = pane
        .pty
        .file()
        .try_clone()
        .context("duplicating the pane's terminal to hand over")?;
    Ok(Handover {
        attachments: Attachments(vec![descriptor.into()]),
        values: Vec::new(),
    })
}

pub(in crate::server) fn pause_pane_reader(_pane: &mut Pane) -> Result<()> {
    Ok(())
}

/// Reads whatever a detached pane has produced.
///
/// The two platforms expose different things to read: a Unix PTY is one
/// descriptor, while a pseudoconsole is an unblocked pipe with its own buffer.
pub(in crate::server) fn read_pane(
    pty: &mut tty::Pty,
    buffer: &mut [u8],
) -> std::io::Result<usize> {
    pty.file().read(buffer)
}
