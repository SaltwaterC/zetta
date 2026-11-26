//! Starting the daemon: binding its socket, adopting the panes a daemon it
//! replaces hands over, and accepting connections until it shuts down.

use super::*;

pub fn endpoint_path(directory: &Path) -> PathBuf {
    directory.join("zmux.json")
}

pub(super) fn socket_path(directory: &Path) -> PathBuf {
    directory.join("zmux.sock")
}

/// Runs the daemon until it is holding nothing and every client has gone.
///
/// `resume_from` is the handover descriptor left by a previous image that
/// replaced itself; the sessions it describes are adopted rather than started.
pub fn run(
    retention: Retention,
    persistence_recipients: Option<Vec<String>>,
    #[cfg(unix)] resume_from: Option<i32>,
    #[cfg(windows)] resume_from: Option<crate::upgrade::HandoverSource>,
    #[cfg(unix)] resume_listener: Option<i32>,
    #[cfg(windows)] resume_ready: Option<PathBuf>,
) -> Result<()> {
    // First, before anything is claimed: whatever is not claimed must not reach
    // the shells this daemon starts.
    #[cfg(unix)]
    crate::upgrade::mark_inherited_close_on_exec();
    retention.validate()?;
    #[cfg(not(feature = "session-persistence"))]
    if persistence_recipients.is_some() {
        anyhow::bail!(
            "disk persistence needs the session-persistence feature, which this multiplexer \
             was built without"
        );
    }
    #[cfg(unix)]
    let mut resumed_handover = resume_from
        .map(crate::upgrade::read_handover)
        .transpose()
        .context("reading the multiplexer upgrade handover")?;
    #[cfg(windows)]
    let mut resumed_handover = resume_from
        .as_ref()
        .map(crate::upgrade::receive_handover)
        .transpose()
        .context("reading the multiplexer upgrade handover")?;
    let generation = resumed_handover
        .as_ref()
        .map_or(new_generation()?, |handover| handover.generation);
    let retention = resumed_handover
        .as_ref()
        .map_or(retention, |handover| handover.retention);
    let directory = session_catalog_dir();
    create_private_dir(&directory)?;
    let socket = socket_path(&directory);
    let endpoint = endpoint_path(&directory);

    // Before anything here touches the socket or the pseudoconsole host, and
    // held for the rest of this process's life: see `lifetime_lock`. A
    // replacement carries its predecessor's.
    let carried_lock = resumed_handover
        .as_mut()
        .and_then(|handover| handover.lifetime_lock.take());
    let lifetime_lock = if resumed_handover.is_some() {
        lock_for_replacement(&directory, carried_lock)
    } else {
        Some(claim_directory(&directory, &socket)?)
    };

    // Replacing itself keeps the endpoint: a client holding the old token has
    // to keep working, and reissuing one would lock out every window that is
    // attached to a session right now. A first start gets a fresh token.
    #[cfg(unix)]
    let inherited = resume_from
        .is_some()
        .then(|| Endpoint::read(&endpoint).ok())
        .flatten();
    #[cfg(windows)]
    let inherited = if resumed_handover.is_some() {
        Some(Endpoint::read(&endpoint).context("reading the daemon endpoint for replacement")?)
    } else {
        None
    };

    #[cfg(windows)]
    let pty_host = crate::pty_host::ensure_running(&directory)?;

    #[cfg(windows)]
    if let Some(handover) = resumed_handover.as_ref() {
        let ready = resume_ready
            .as_deref()
            .context("a Windows handover requires --resume-ready")?;
        validate_handover_with_host(&pty_host, handover)?;
        crate::upgrade::mark_ready(ready)?;
        wait_for_old_daemon(
            inherited
                .as_ref()
                .context("the Windows replacement has no old endpoint")?,
        )?;
    }

    let listener = bind_listener(
        &socket,
        resumed_handover.is_some(),
        #[cfg(unix)]
        resume_listener,
    )?;
    let token = match inherited {
        Some(endpoint) => endpoint.token,
        None => random_hex(32)?,
    };
    #[cfg(feature = "session-persistence")]
    let persistence = if matches!(retention, Retention::Disk) {
        PersistenceStore::open_with_recovery_state(
            &directory,
            persistence_recipients.as_deref(),
            resume_from.is_some(),
        )?
    } else {
        // A temporary memory fallback must not make records from an earlier
        // disk daemon disappear.  Reopen the store for listing, resume, and
        // explicit cleanup, while `PersistenceQueue::enabled` keeps this
        // recovery handle from persisting new memory-mode sessions.
        PersistenceStore::open_with_recovery_state(&directory, None, false)?
    };
    // Only recipients the starting client passed are trusted as startup, never
    // the ones a recovery open read back from whoever configured last.
    #[cfg(feature = "session-persistence")]
    let startup_recipients = persistence_recipients
        .is_some()
        .then(|| {
            persistence
                .as_ref()
                .map(|store| store.recipient_values().to_vec())
        })
        .flatten();
    #[cfg(not(feature = "session-persistence"))]
    let startup_recipients = None;
    #[cfg(feature = "session-persistence")]
    let next_session_id = crate::persistence::next_record_id(&directory)?;
    #[cfg(not(feature = "session-persistence"))]
    let next_session_id = 1;
    let daemon = Arc::new(Daemon::new(
        &directory,
        retention,
        directory.join("clipboard"),
        #[cfg(feature = "session-persistence")]
        persistence,
        next_session_id,
        generation,
        #[cfg(all(unix, not(target_os = "macos")))]
        listener.as_raw_fd(),
        #[cfg(windows)]
        pty_host,
    ));
    *lock_grants(&daemon) = RecipientGrants::starting_with(startup_recipients);
    *daemon.lifetime_lock.lock().recover_poison() = lifetime_lock;
    if let Some(handover) = resumed_handover {
        adopt_resumed_handover(
            &daemon,
            handover,
            #[cfg(windows)]
            resume_ready.as_deref(),
        )?;
    }
    sweep_image_staging(&daemon)?;
    #[cfg(feature = "session-persistence")]
    {
        let waker = Arc::downgrade(&daemon);
        daemon.persistence.start(move || {
            if let Some(daemon) = waker.upgrade() {
                wake_drain(&daemon);
            }
        });
    }
    start_reaper(daemon.clone())?;
    start_drain(daemon.clone())?;

    // Publish only after recovery, daemon construction, handover adoption, and
    // worker setup have completed. A bound socket can accept a connection
    // before any of those steps are done, so the endpoint is not itself a
    // readiness signal; clients use `Request::Ping` as the final probe.
    Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: std::process::id(),
        socket_path: socket.clone(),
        token: token.clone(),
    }
    .write(&endpoint)?;

    log::info!("zmux listening on {}", socket.display());
    accept_connections(&daemon, &listener, &token);
    shut_down(&daemon, &socket, &endpoint);
    Ok(())
}

/// Takes over the sessions a previous image handed over, once the daemon that
/// will serve them exists.
pub(super) fn adopt_resumed_handover(
    daemon: &Arc<Daemon>,
    mut handover: crate::upgrade::Handover,
    #[cfg(windows)] resume_ready: Option<&Path>,
) -> Result<()> {
    let grants = handover.recipient_grants.take();
    // The previous image left its sessions behind in this same process: the
    // descriptors are still open and the shells are still this process's
    // children, so they are adopted rather than restarted.
    #[cfg(unix)]
    match adopt_handover(daemon, handover) {
        Ok(count) => log::info!("zmux resumed {count} session(s) across an upgrade"),
        Err(error) => log::error!("could not resume sessions across the upgrade: {error:#}"),
    }
    #[cfg(windows)]
    {
        let count = adopt_handover(daemon, handover)
            .context("adopting sessions across the Windows upgrade")?;
        log::info!("zmux resumed {count} session(s) across an upgrade");
    }
    adopt_seals(daemon, grants);
    // An older image could hand over panes whose process ended before the
    // exec: the reaper never re-examines a pane already marked exited, so
    // prune what an upgraded daemon should not offer.
    prune_exited_panes(daemon);
    publish(daemon);
    #[cfg(windows)]
    if let Some(ready) = resume_ready {
        crate::upgrade::remove_ready(ready);
    }
    Ok(())
}

/// Serves every connection the listener accepts until the daemon stops.
pub(super) fn accept_connections(daemon: &Arc<Daemon>, listener: &Listener, token: &str) {
    for stream in listener.incoming() {
        if !daemon.running.load(Ordering::SeqCst) {
            break;
        }
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("multiplexer accept failed: {error:#}");
                continue;
            }
        };
        let accepted = Instant::now();
        // Refused here rather than left in the kernel's backlog, where it would
        // hold the user's own next request behind whatever filled the daemon.
        let Some(permit) = ConnectionPermit::claim(&daemon.connections, MAX_CONNECTIONS) else {
            log::warn!("refusing a multiplexer connection: {MAX_CONNECTIONS} are being served");
            refuse_connection(stream);
            continue;
        };
        spawn_connection(
            daemon.clone(),
            stream,
            token.to_owned(),
            Accepted { accepted, permit },
        );
    }
}

/// What a stopping daemon does once it has stopped accepting connections.
pub(super) fn shut_down(daemon: &Daemon, socket: &Path, endpoint: &Path) {
    // A Windows daemon that handed over to a replacement owns none of what it
    // would tidy up here: the replacement may already have bound and published
    // the socket and endpoint, and is writing the persistence directory. Its
    // scrollback was flushed before the replacement started
    // (`prepare_replacement`); flushing again now would rewrite the manifest
    // the replacement keeps with this daemon's older copy.
    if daemon.handed_over.load(Ordering::SeqCst) {
        return;
    }
    #[cfg(feature = "session-persistence")]
    {
        let mut persistence = daemon.persistence.lock();
        if let Some(persistence) = persistence.as_mut()
            && let Err(error) = persistence.flush_segments()
        {
            log::warn!("could not flush encrypted scrollback: {error:#}");
        }
    }
    let _ = std::fs::remove_file(socket);
    let _ = std::fs::remove_file(endpoint);
}

/// The listener: inherited from the image this one replaced where the
/// platform carries it across `execv`, otherwise bound afresh.
#[cfg_attr(
    windows,
    expect(
        unused_variables,
        reason = "Windows never carries a listener across an upgrade"
    )
)]
pub(super) fn bind_listener(
    socket: &Path,
    resumed: bool,
    #[cfg(unix)] resume_listener: Option<i32>,
) -> Result<Listener> {
    #[cfg(target_os = "macos")]
    let resume_listener = {
        if let Some(descriptor) = resume_listener {
            // An older image may still have passed its listener through. It is
            // unusable after a Darwin exec, so close it before binding the
            // replacement's fresh listener.
            // SAFETY: the descriptor was inherited from the previous image.
            unsafe { libc::close(descriptor) };
        }
        None::<i32>
    };
    #[cfg(unix)]
    if let Some(descriptor) = resume_listener {
        anyhow::ensure!(resumed, "--resume-listener requires --resume-from");
        // SAFETY: the descriptor was inherited from the previous image and is
        // claimed by this listener exactly once.
        let descriptor = unsafe { crate::upgrade::claim_inherited_fd(descriptor) }
            .context("claiming the inherited listener")?;
        return Ok(Listener::from(descriptor));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    anyhow::ensure!(!resumed, "an upgrade did not carry its listening socket");
    // Only the lock's holder gets here. A fresh start found nothing answering
    // on the socket, and a replacement's predecessor has stopped listening, so
    // whatever is left there is a dead listener's.
    remove_stale_socket(socket)?;
    let listener = Listener::bind(socket)
        .with_context(|| format!("binding the multiplexer socket {}", socket.display()))?;
    restrict_socket(socket)?;
    Ok(listener)
}

pub(super) fn new_generation() -> Result<u64> {
    u64::from_str_radix(&random_hex(8)?, 16).context("creating a daemon generation")
}

#[cfg(windows)]
pub(super) fn validate_handover_with_host(
    host: &crate::pty_host::HostClient,
    handover: &crate::upgrade::Handover,
) -> Result<()> {
    let consoles = host.list()?;
    let expected = handover
        .sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .map(|pane| pane.console_id)
        .collect::<std::collections::HashSet<_>>();
    anyhow::ensure!(
        expected.len()
            == handover
                .sessions
                .iter()
                .map(|session| session.panes.len())
                .sum::<usize>(),
        "the Windows handover contains a duplicate pseudoconsole"
    );
    let known = consoles
        .iter()
        .map(|console| console.console_id)
        .collect::<std::collections::HashSet<_>>();
    for console_id in &expected {
        anyhow::ensure!(
            known.contains(console_id),
            "the pseudoconsole host no longer holds console {console_id}"
        );
        let (_, handles) = host.handles(*console_id, std::process::id())?;
        let handles = crate::transport::claim_duplicated(&handles);
        anyhow::ensure!(
            handles.len() == 2,
            "pseudoconsole {console_id} did not return two pipe handles"
        );
    }
    // A failed daemon start must not strand a console in the host forever.
    // This is safe because the host directory identifies one daemon instance.
    for console in consoles {
        if !expected.contains(&console.console_id) {
            let _ = host.close(console.console_id);
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn wait_for_old_daemon(endpoint: &Endpoint) -> Result<()> {
    const TIMEOUT: Duration = Duration::from_secs(10);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if Stream::connect(&endpoint.socket_path).is_err() {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "the previous multiplexer did not release its endpoint within {TIMEOUT:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
pub(super) fn restrict_socket(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting the multiplexer socket {}", path.display()))
}

/// Windows carries no mode bits to set here. The socket inherits the access
/// control of the per-user configuration directory it lives in, which is where
/// the endpoint token already relies on being private.
#[cfg(windows)]
pub(super) fn restrict_socket(_path: &Path) -> Result<()> {
    Ok(())
}
