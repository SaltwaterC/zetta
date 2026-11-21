//! The Windows upgrade: there is no `exec`, so a new daemon is started and
//! the panes, their console handles and the agent pipes are handed to it, with
//! a reaper standing in for the parent the children no longer have.

use super::*;

pub(in crate::server) fn upgrade_daemon(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    let (executable, mut handover) = prepare_upgrade(daemon).map_err(UpgradeRefused::Before)?;
    let ready =
        crate::upgrade::ready_path(&session_catalog_dir()).map_err(UpgradeRefused::Before)?;
    let mut replacement =
        crate::upgrade::spawn_replacement(&executable, &ready).map_err(UpgradeRefused::Before)?;
    // Everything the replacement was given — the handover pipe, the agent pipe
    // instances — goes with it if it is stopped here.
    let refuse = |replacement: &mut std::process::Child, error: anyhow::Error| {
        let _ = replacement.kill();
        crate::upgrade::remove_ready(&ready);
        Err(UpgradeRefused::Before(error))
    };
    hand_over_agent_pipes(&mut handover, &replacement);
    let sent = held_lock(daemon, |lock| {
        crate::upgrade::send_handover(&mut replacement, &handover, Some(lock.file()))
    })
    .unwrap_or_else(|| crate::upgrade::send_handover(&mut replacement, &handover, None));
    if let Err(error) = sent {
        return refuse(&mut replacement, error);
    }
    drop(handover);
    if let Err(error) = crate::upgrade::wait_for_ready(&mut replacement, &ready) {
        return refuse(&mut replacement, error);
    }
    if let Err(error) = connection.send(&Response::Ok) {
        return refuse(&mut replacement, error);
    }
    broadcast(daemon, &Event::Replacing);
    // The replacement binds and publishes as soon as this daemon's listener
    // closes, so neither is this daemon's to remove on the way out.
    daemon.handed_over.store(true, Ordering::SeqCst);
    daemon.running.store(false, Ordering::SeqCst);
    let _ = Stream::connect(socket_path(&session_catalog_dir()));
    Ok(())
}

/// Gives the replacement an instance of every pane's agent pipe, so each name
/// stays held while this daemon exits; see `agent_pipe/listener.rs`.
pub(super) fn hand_over_agent_pipes(
    handover: &mut crate::upgrade::Handover,
    replacement: &std::process::Child,
) {
    use std::os::windows::io::AsRawHandle as _;

    let process = windows::Win32::Foundation::HANDLE(replacement.as_raw_handle());
    for pane in handover
        .sessions
        .iter_mut()
        .flat_map(|session| session.panes.iter_mut())
    {
        if let Some((name, instance)) = super::agent_pipe::hand_over(pane.id, process) {
            pane.agent_pipe = Some(name.to_string_lossy().into_owned());
            pane.agent_pipe_instance = instance;
        }
    }
}

pub(in crate::server) fn prepare_upgrade(
    daemon: &Arc<Daemon>,
) -> Result<(PathBuf, crate::upgrade::Handover)> {
    let executable = prepare_replacement(daemon)?;
    let mut sessions = daemon.sessions();
    let handover = handover_of(daemon, &mut sessions);
    drop(sessions);
    validate_handover_with_host(&daemon.pty_host, &handover)?;
    Ok((executable, handover))
}

/// Takes over the sessions the previous daemon handed over. Their consoles
/// never left the zmux-pty host, which hands this process its own handles to
/// each, and every agent pipe name stays held by the instance passed with it.
pub(in crate::server) fn adopt_handover(
    daemon: &Arc<Daemon>,
    handover: crate::upgrade::Handover,
) -> Result<usize> {
    adopt_sessions(
        daemon,
        handover,
        |pane| {
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
            // The previous daemon's listener dies with it; the shell still has
            // this pane's pipe name, which the instance it passed keeps held.
            // What the pane fell back to did not survive the handover, so this
            // daemon's own agent stands in.
            if let Some(name) = &pane.agent_pipe {
                super::agent_pipe::adopt(
                    pane.id,
                    PathBuf::from(name),
                    pane.agent_pipe_instance,
                    std::env::var_os("SSH_AUTH_SOCK")
                        .filter(|path| !path.is_empty())
                        .map(PathBuf::from),
                );
            }
            Ok(AdoptedTerminal { pty, child_events })
        },
        |session| {
            for pane in &session.panes {
                super::agent_pipe::discard(pane.agent_pipe_instance);
            }
        },
    )
}

/// Reaps children and tells whoever is holding their terminals.
///
/// Windows has no `SIGCHLD`: each PTY carries its own exit watcher, which
/// signals when its process ends, so this polls those watchers rather than
/// waiting on a signal.
pub(in crate::server) fn start_reaper(daemon: Arc<Daemon>) -> Result<()> {
    spawn_worker("zmux reaper", move || {
        let daemon = daemon.clone();
        Box::new(move || windows_reaper_loop(daemon))
    });
    Ok(())
}

pub(in crate::server) fn windows_reaper_loop(daemon: Arc<Daemon>) {
    loop {
        match daemon.pty_host.reap() {
            Ok(exits) if !exits.is_empty() => {
                let mut sessions = daemon.sessions();
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
            let mut sessions = daemon.sessions();
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
        if !exits.is_empty() {
            wake_drain(&daemon);
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

pub(in crate::server) fn handover_handles(
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
pub(in crate::server) fn pause_pane_reader(pane: &mut Pane) -> Result<()> {
    pane.pty
        .pause_reader()
        .context("pausing the Windows pseudoconsole reader")?;
    let mut buffer = [0; 16 * 1024];
    loop {
        let read = pane.pty.read_buffered(&mut buffer);
        if read == 0 {
            break;
        }
        let visible = filter_clipboard_output(pane, &buffer[..read]);
        pane.retained.push(&visible);
        record_handover_output(pane, &visible);
        relay_output(pane, &visible);
    }
    Ok(())
}

pub(in crate::server) fn read_pane(
    pty: &mut tty::Pty,
    buffer: &mut [u8],
) -> std::io::Result<usize> {
    use alacritty_terminal::tty::EventedReadWrite as _;
    Ok(pty.reader().try_read(buffer))
}
