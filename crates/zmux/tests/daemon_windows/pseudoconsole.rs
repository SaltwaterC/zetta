//! A pane's pseudoconsole across the daemon's life: kept through an upgrade or
//! a reconfiguration, read by one side at a time, and stopped with its host.

use super::*;

fn shell_state_request(working_directory: PathBuf) -> SpawnRequest {
    let mut request = spawn_request();
    request.args = vec![
        "/D".to_owned(),
        "/C".to_owned(),
        "echo ZETTA_ENV=%ZETTA_TEST_VALUE% & cd & echo ZETTA_CWD_END".to_owned(),
    ];
    request.env.insert(
        "ZETTA_TEST_VALUE".to_owned(),
        "from-daemon-request".to_owned(),
    );
    request.working_directory = Some(working_directory);
    request
}

fn detach_restore_request() -> SpawnRequest {
    let mut request = spawn_request();
    request.program = Some("powershell.exe".to_owned());
    request.args = vec![
        "-NoLogo".to_owned(),
        "-NoProfile".to_owned(),
        "-Command".to_owned(),
        "Write-Output before; Start-Sleep -Seconds 3; Write-Output after; Start-Sleep -Seconds 10"
            .to_owned(),
    ];
    request
}

#[test]
fn cli_upgrade_keeps_a_live_windows_pseudoconsole() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let old_process_id = client.process_id();
    let pane = client
        .spawn(spawn_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    drop(pane);

    let upgrade = Command::new(daemon_binary())
        .arg("--upgrade")
        .env("APPDATA", &daemon.config)
        .output()
        .expect("running zmux --upgrade");
    assert!(
        upgrade.status.success(),
        "zmux --upgrade failed: {}",
        String::from_utf8_lossy(&upgrade.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&upgrade.stdout).trim(),
        "Replaced the multiplexer; its sessions were kept."
    );

    let replacement = daemon.client();
    assert_ne!(replacement.process_id(), old_process_id);
    replacement
        .resize(session_id, pane_id, 100, 30)
        .expect("resizing the adopted pseudoconsole");
    let attached = replacement
        .attach(session_id, Some(pane_id), None)
        .expect("attaching the adopted pseudoconsole");
    match attached {
        AttachOutcome::Attached { pane, .. } => {
            assert_eq!(pane.session_id, session_id);
            assert_eq!(pane.pane_id, pane_id);
        }
        _ => panic!("expected an exclusive pane after upgrade"),
    }

    replacement
        .kill(session_id)
        .expect("cleaning up the upgraded pseudoconsole");
    replacement
        .shutdown()
        .expect("stopping the replacement daemon");
}

#[cfg(feature = "session-persistence")]
#[test]
fn resilient_reconfiguration_to_disk_keeps_a_windows_pseudoconsole_running() {
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let child_pid = pane.child_pid;

    let configuration = client
        .configure_with_retention_and_persistence_resilient(
            zmux::retention::Retention::Disk,
            zmux::persistence::PersistenceOptions {
                recipients: vec![identity.to_public().to_string()],
                identity: None,
            },
            zmux::retention::Retention::Memory { bytes: 4096 },
        )
        .expect("switching retention to disk");
    assert_eq!(
        configuration.effective_retention,
        zmux::retention::Retention::Disk
    );
    assert!(process_is_alive(child_pid));

    drop(pane);
    client
        .detach(
            session_id,
            session_summary(session_id, pane_id),
            serde_json::json!({"configured_after_startup": true}),
            None,
            Vec::new(),
        )
        .expect("detaching the configured pseudoconsole");
    assert!(
        daemon
            .sessions_dir()
            .join("persistence")
            .join("manifest.json")
            .is_file()
    );

    client
        .kill(session_id)
        .expect("cleaning up the pseudoconsole");
    client.shutdown().expect("stopping the daemon");
}

#[test]
fn attached_client_receives_conpty_output_without_host_competing_for_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(output_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;

    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let mut sent_device_attributes = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        let mut available = 0;
        let peek_succeeded = unsafe {
            windows::Win32::System::Pipes::PeekNamedPipe(
                windows::Win32::Foundation::HANDLE(output.as_raw_handle()),
                None,
                0,
                None,
                Some(&mut available),
                None,
            )
            .is_ok()
        };
        if !peek_succeeded {
            break;
        }
        if available > 0 {
            let start = bytes.len();
            bytes.resize(start + available as usize, 0);
            match output.read(&mut bytes[start..]) {
                Ok(length) => bytes.truncate(start + length),
                Err(_) => bytes.truncate(start),
            }
            if bytes
                .windows(b"zetta-upgrade".len())
                .any(|window| window == b"zetta-upgrade")
            {
                break;
            }
            if !sent_device_attributes && bytes.windows(3).any(|window| window == b"\x1b[c") {
                input
                    .write_all(b"\x1b[?6c")
                    .expect("answering the pseudoconsole device query");
                sent_device_attributes = true;
            }
        }

        std::thread::sleep(Duration::from_millis(10));
    }
    drop(output);
    drop(input);

    let _ = client.kill(session_id);
    let _ = client.shutdown();

    assert!(!bytes.is_empty(), "the attached client received no output");
    assert!(
        String::from_utf8_lossy(&bytes).contains("zetta-upgrade"),
        "unexpected pseudoconsole output: {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn daemon_preserves_windows_shell_arguments_environment_and_working_directory() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let working_directory = daemon.config.clone();
    let pane = client
        .spawn(shell_state_request(working_directory.clone()))
        .expect("creating a shell-state probe pane");
    let session_id = pane.session_id;
    let mut output_file = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);

    let bytes = read_until(
        &mut output_file,
        &mut input,
        b"ZETTA_CWD_END",
        Duration::from_secs(5),
    );
    let output = String::from_utf8_lossy(&bytes);
    let expected_directory = working_directory.to_string_lossy();
    assert!(
        output.contains("ZETTA_ENV=from-daemon-request"),
        "the daemon did not preserve the requested environment: {output:?}"
    );
    assert!(
        output.contains(expected_directory.as_ref()),
        "the daemon did not preserve the requested working directory: {output:?}"
    );
    assert!(
        output.contains("ZETTA_CWD_END"),
        "the daemon did not preserve the requested shell arguments: {output:?}"
    );

    drop(output_file);
    drop(input);
    let _ = client.kill(session_id);
    let _ = client.shutdown();
}

#[test]
fn detached_conpty_reader_stops_before_exclusive_reattach() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(detach_restore_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);

    let initial = read_until(&mut output, &mut input, b"before", Duration::from_secs(5));
    assert!(
        String::from_utf8_lossy(&initial).contains("before"),
        "the attached client received no initial output: {:?}",
        String::from_utf8_lossy(&initial)
    );
    drop(output);
    drop(input);

    client
        .detach_as(
            session_id,
            BackgroundSessionSummary {
                id: session_id,
                title: "test".to_owned(),
                authentication_required: false,
                active_pane: pane_id,
                layout: BackgroundPaneLayout::Pane { pane_id },
                panes: Vec::new(),
                held: false,
                scoped_to: Some(std::process::id()),
                key_envelope: None,
            },
            serde_json::Value::Null,
            None,
            std::process::id(),
        )
        .expect("detaching the pane");
    // Let the daemon enter its detached read before handing the console back;
    // otherwise the test could pass without exercising the reader that must
    // be stopped at reattach.
    std::thread::sleep(Duration::from_millis(250));

    let attached = client
        .attach(session_id, Some(pane_id), None)
        .expect("reattaching the pane");
    let AttachOutcome::Attached { pane, .. } = attached else {
        panic!("expected an exclusive pane after detaching");
    };
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    let after = read_until(&mut output, &mut input, b"after", Duration::from_secs(5));
    drop(output);
    drop(input);

    assert!(
        String::from_utf8_lossy(&after).contains("after"),
        "output produced after exclusive reattach was consumed by the daemon: {:?}",
        String::from_utf8_lossy(&after)
    );

    let _ = client.kill(session_id);
    let _ = client.shutdown();
}

#[test]
fn stopping_an_idle_daemon_stops_its_pseudoconsole_host() {
    let daemon = TestDaemon::start();
    let host_endpoint =
        zmux::transport::Endpoint::read(&zmux::pty_host::endpoint_path(&daemon.sessions_dir()))
            .expect("reading the pseudoconsole host endpoint");
    let host_process_id = host_endpoint.process_id;
    assert!(
        process_is_alive(host_process_id),
        "the pseudoconsole host must be alive before stopping the daemon"
    );

    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), false).expect("stopping the idle daemon"),
        zmux::StopOutcome::Stopped
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_alive(host_process_id) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(host_process_id),
        "stopping zmux must not leave zmux-pty process {host_process_id} running"
    );
    assert!(
        !zmux::pty_host::endpoint_path(&daemon.sessions_dir()).exists(),
        "stopping zmux must remove the stale zmux-pty endpoint"
    );
}

#[test]
fn force_stopping_a_daemon_with_a_session_stops_its_pseudoconsole_host() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request())
        .expect("creating a pseudoconsole pane");
    drop(pane);

    let host_endpoint =
        zmux::transport::Endpoint::read(&zmux::pty_host::endpoint_path(&daemon.sessions_dir()))
            .expect("reading the pseudoconsole host endpoint");
    let host_process_id = host_endpoint.process_id;
    let daemon_process_id = client.process_id();
    assert!(process_is_alive(host_process_id));

    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), true).expect("force-stopping the daemon"),
        zmux::StopOutcome::Signalled {
            process_id: daemon_process_id
        }
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_alive(host_process_id) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(host_process_id),
        "force-stopping zmux must not leave zmux-pty process {host_process_id} running"
    );
}

/// An exclusively attached Windows client reads the pseudoconsole itself, so
/// only the daemon — which owns the child through the host — can tell it how
/// that child ended.
#[test]
fn an_attached_client_learns_that_its_console_child_exited() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().expect("subscribing to pane exits");
    let exits = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();

    let mut request = spawn_request();
    request.args = vec![
        "/D".to_owned(),
        "/C".to_owned(),
        "echo ready & exit 7".to_owned(),
    ];
    let pane = client
        .spawn(request)
        .expect("creating a pane that exits by itself");
    let (reports, receiver) = async_channel::unbounded();
    exits.register_shared(pane.pane_id, reports);
    let mut output = std::fs::File::from(pane.conout);
    let mut input = std::fs::File::from(pane.conin);
    read_until(&mut output, &mut input, b"ready", Duration::from_secs(10));

    let deadline = Instant::now() + Duration::from_secs(15);
    let report = loop {
        match receiver.try_recv() {
            Ok(report) => break report,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => panic!("the pane's exit was never reported"),
        }
    };
    assert!(!report.disconnected, "the exit arrived as a disconnection");
    assert_eq!(
        report.raw_status,
        Some(7),
        "the multiplexer reported the wrong exit status"
    );
    let log = std::fs::read_to_string(daemon.sessions_dir().join("daemon.log"))
        .expect("abnormal exits must survive in the daemon's default log");
    assert!(
        log.lines().any(|line| {
            line.contains("pane exited:")
                && line.contains(&format!("session_id={} ", pane.session_id))
                && line.contains(&format!("pane_id={} ", pane.pane_id))
                && line.contains(&format!("child_pid={} ", pane.child_pid))
                && line.contains("raw_status=Some(7)")
        }),
        "the log must identify the failed pane and its actual exit status: {log}"
    );

    drop(output);
    drop(input);
    let _ = client.shutdown();
}
