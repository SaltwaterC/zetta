//! Upgrades and replacement: the daemon handing itself over to a newer binary,
//! refusing a mismatched one, and stopping when asked.

use super::*;

/// Re-exec tests share the same executable and OS process/descriptor
/// machinery. Keep upgrades from multiple test threads from overlapping; the
/// rest of the daemon tests remain parallel.
fn upgrade_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static UPGRADE_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    UPGRADE_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .recover_poison()
}

#[test]
fn an_upgrade_keeps_the_sessions_and_their_processes() {
    let _upgrade_test_guard = upgrade_test_guard();
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let verifier = zmux::auth::SessionAuthentication::create("correct horse").unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"tab": "state"}),
            Some(&verifier),
            Vec::new(),
        )
        .unwrap();

    // One wrong secret, so the backoff window is open across the upgrade.
    assert!(matches!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some("wrong".to_owned())
            )
            .unwrap(),
        AttachOutcome::AuthenticationFailed
    ));

    client.upgrade().unwrap();

    let client = daemon.wait_for_replacement();

    assert!(
        process_is_alive(pane.child_pid),
        "the session was restarted"
    );
    assert_eq!(client.list().unwrap().len(), 1);

    // Protection survives, and so does the rate limit: if `--upgrade` cleared
    // it, anyone able to trigger an upgrade could guess without penalty.
    assert!(matches!(
        client
            .attach(pane.session_id, Some(pane.pane_id), None)
            .unwrap(),
        AttachOutcome::AuthenticationRequired
    ));
    assert!(matches!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some("wrong again".to_owned())
            )
            .unwrap(),
        AttachOutcome::AuthenticationFailed
    ));

    // And the correct secret still works once the window has passed.
    std::thread::sleep(Duration::from_secs(3));
    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some("correct horse".to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached {
            pane: attached,
            state,
            ..
        } => {
            assert_eq!(attached.child_pid, pane.child_pid);
            assert_eq!(state, serde_json::json!({"tab": "state"}));
        }
        _ => panic!("the correct secret must still attach after an upgrade"),
    }
}

/// A process's descriptors above standard error, as `/proc` lists them: the
/// number, what it refers to, and whether it is close-on-exec.
#[cfg(target_os = "linux")]
fn open_descriptors(pid: u32) -> Vec<(i32, String, bool)> {
    let mut descriptors = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|entry| entry.unwrap().file_name().to_str()?.parse::<i32>().ok())
        .filter(|descriptor| *descriptor > 2)
        .filter_map(|descriptor| {
            // Gone in between: the listing's own descriptor, when `pid` is
            // this process, or one the process closed.
            let target = std::fs::read_link(format!("/proc/{pid}/fd/{descriptor}")).ok()?;
            let info = std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{descriptor}")).ok()?;
            let flags = info
                .lines()
                .find_map(|line| line.strip_prefix("flags:"))
                .map(|flags| i32::from_str_radix(flags.trim(), 8).unwrap())
                .unwrap();
            Some((
                descriptor,
                target.display().to_string(),
                flags & libc::O_CLOEXEC != 0,
            ))
        })
        .collect::<Vec<_>>();
    descriptors.sort();
    descriptors
}

/// The upgrade has to clear close-on-exec on every pane's terminal and the
/// listener for them to survive the `execv`. The replacement has to set it
/// again on each one it claims, or every shell it starts holds them all.
#[cfg(target_os = "linux")]
#[test]
fn an_upgrade_leaves_no_descriptor_inheritable() {
    let _upgrade_test_guard = upgrade_test_guard();
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    read_until(&std::fs::File::from(pane.descriptor), "ready");

    client.upgrade().unwrap();
    let _client = daemon.wait_for_replacement();

    let inheritable = open_descriptors(daemon.process_id())
        .into_iter()
        .filter(|(_, _, close_on_exec)| !close_on_exec)
        .collect::<Vec<_>>();
    assert!(
        inheritable.is_empty(),
        "the replacement holds inheritable descriptors: {inheritable:?}"
    );
}

/// What makes an inheritable descriptor matter: a shell holding another
/// session's terminal can read it and type into it without its secret, and one
/// holding the listener can take connections meant for the daemon.
#[cfg(target_os = "linux")]
#[test]
fn a_shell_started_after_an_upgrade_inherits_nothing_from_the_daemon() {
    let _upgrade_test_guard = upgrade_test_guard();
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let other = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    read_until(&std::fs::File::from(other.descriptor), "ready");

    client.upgrade().unwrap();
    let client = daemon.wait_for_replacement();
    let pane = client.spawn(spawn_request(None, "exec sleep 60")).unwrap();
    // Once it is `sleep`, the shell has made no descriptors of its own, and
    // whatever it holds came from the daemon.
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::read_to_string(format!("/proc/{}/comm", pane.child_pid))
        .is_ok_and(|name| name.trim() != "sleep")
    {
        assert!(Instant::now() < deadline, "the shell never became sleep");
        std::thread::sleep(Duration::from_millis(10));
    }

    let inherited = open_descriptors(pane.child_pid);
    assert!(
        inherited.is_empty(),
        "a shell started after the upgrade holds {inherited:?}"
    );
}

#[cfg(feature = "session-persistence")]
#[test]
fn an_upgrade_keeps_live_disk_records_unrestorable() {
    let _upgrade_test_guard = upgrade_test_guard();
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    assert!(client.list_with_restorable().unwrap().1.is_empty());

    client.upgrade().unwrap();
    let client = daemon.wait_for_replacement();

    let (sessions, records) = client.list_with_restorable().unwrap();
    assert!(
        sessions.iter().any(|session| session.id == pane.session_id),
        "the live session was lost during the upgrade: {sessions:?}"
    );
    assert!(
        records
            .iter()
            .all(|record| record.id != pane.session_id || !record.restorable),
        "the live session was also offered from disk: {records:?}"
    );
    let manifest =
        std::fs::read_to_string(daemon.sessions_dir().join("persistence/manifest.json")).unwrap();
    assert!(
        manifest.contains(r#""restorable": false"#),
        "manifest after upgrade: {manifest}"
    );

    client.kill(pane.session_id).unwrap();
}

#[test]
fn a_dead_multiplexers_endpoint_does_not_block_starting_a_new_one() {
    // An endpoint outlives the daemon that wrote it. Judging the version
    // before checking whether anything is listening let a stale file from a
    // previous build refuse every attempt to start a live multiplexer —
    // permanently, and complaining about a process that no longer existed.
    let directory = tempfile::tempdir().unwrap();
    let sessions = directory.path().join("zetta").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("zmux.json"),
        serde_json::json!({
            "version": 1,
            "protocol_version": zmux::messages::PROTOCOL_VERSION - 1,
            "process_id": 999_999,
            "socket_path": sessions.join("zmux.sock"),
            "token": "00",
        })
        .to_string(),
    )
    .unwrap();

    assert!(
        Client::connect_existing_at(&sessions).unwrap().is_none(),
        "a stale endpoint must read as no multiplexer, not as a refusal"
    );
}

#[test]
fn a_multiplexer_from_another_build_is_refused_before_it_is_used() {
    // The failure this guards against: a daemon left over from an earlier
    // build accepts the connection, cannot parse the request, and closes —
    // leaving the client with no reason and every terminal failing to open.
    let daemon = TestDaemon::start();
    let endpoint_path = daemon.sessions_dir().join("zmux.json");
    let mut endpoint: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&endpoint_path).unwrap()).unwrap();
    endpoint["protocol_version"] = serde_json::json!(999);
    std::fs::write(&endpoint_path, endpoint.to_string()).unwrap();

    let error = match Client::connect_existing_at(&daemon.sessions_dir()) {
        Ok(_) => panic!("a multiplexer speaking another protocol must be refused"),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("999"), "{error}");
    assert!(error.contains("protocol version"), "{error}");
}

#[test]
fn a_client_from_a_newer_build_is_told_why_rather_than_dropped() {
    let daemon = TestDaemon::start();
    let endpoint: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(daemon.sessions_dir().join("zmux.json")).unwrap(),
    )
    .unwrap();

    let mut stream =
        std::os::unix::net::UnixStream::connect(endpoint["socket_path"].as_str().unwrap()).unwrap();
    // A version this daemon does not speak, carrying a field it has never seen.
    let request = serde_json::json!({
        "version": 999,
        "token": endpoint["token"],
        "client_process_id": std::process::id(),
        "request": {"request": "list"},
        "something_new": true,
    });
    write_json_frame(&mut stream, &request);
    let response = read_json_frame(&mut stream);
    assert!(
        response.contains("protocol version"),
        "expected a version error, got {response:?}"
    );
    // And told in the wording a refused client recognises. This is the coupling
    // that decides what an open window does when `make install` replaces the
    // daemon under it: recognised, it stops and says so; unrecognised, it waits
    // out the resubscribe grace and then reports every attached pane's terminal
    // as ended.
    assert!(
        zmux::messages::is_protocol_mismatch_message(&response),
        "the refusal is no longer one a refused client can recognise: {response:?}"
    );
}

/// Replacing the multiplexer has to work across a protocol version boundary,
/// because crossing one is what it is for.
///
/// Rebuilding leaves a new client and an old daemon. Refusing `--upgrade` for
/// disagreeing about the version — as the general rule does, correctly, for every
/// other request — made every protocol bump a choice between running the new
/// build and keeping the sessions the old daemon holds.
#[test]
fn an_upgrade_is_accepted_from_a_client_that_disagrees_about_the_protocol() {
    let _upgrade_test_guard = upgrade_test_guard();
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    drop(descriptor);

    #[cfg(not(target_os = "macos"))]
    use std::os::unix::fs::MetadataExt as _;
    #[cfg(not(target_os = "macos"))]
    let socket_path = daemon.sessions_dir().join("zmux.sock");
    #[cfg(not(target_os = "macos"))]
    let socket_metadata = std::fs::metadata(&socket_path).unwrap();
    #[cfg(not(target_os = "macos"))]
    let before_socket_identity = (socket_metadata.dev(), socket_metadata.ino());
    let before_runner_id = zmux::catalog::read_session_catalogs(&daemon.sessions_dir())
        .unwrap()
        .first()
        .map(|catalog| catalog.runner_id)
        .unwrap();

    let endpoint: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(daemon.sessions_dir().join("zmux.json")).unwrap(),
    )
    .unwrap();
    let mut stream =
        std::os::unix::net::UnixStream::connect(endpoint["socket_path"].as_str().unwrap()).unwrap();
    let request = serde_json::json!({
        "version": 999,
        "token": endpoint["token"],
        "client_process_id": std::process::id(),
        "request": {"request": "upgrade"},
    });
    write_json_frame(&mut stream, &request);
    let response = read_json_frame(&mut stream);
    assert!(
        !response.contains("protocol version"),
        "the upgrade was refused over the version: {response:?}"
    );

    // And it really replaced itself, keeping what it held.
    let sessions = wait_for_session_list(&daemon);
    assert_eq!(
        sessions.len(),
        1,
        "the replacement did not keep the session: {sessions:?}"
    );
    assert!(
        process_is_alive(pane.child_pid),
        "the replacement lost the session's process"
    );
    #[cfg(not(target_os = "macos"))]
    {
        let socket_metadata = std::fs::metadata(&socket_path).unwrap();
        assert_eq!(
            before_socket_identity,
            (socket_metadata.dev(), socket_metadata.ino()),
            "this platform's upgrade must keep the listening socket rather than rebind it"
        );
    }
    let after_catalogs = zmux::catalog::read_session_catalogs(&daemon.sessions_dir()).unwrap();
    assert_eq!(
        after_catalogs.first().map(|catalog| catalog.runner_id),
        Some(before_runner_id)
    );
    assert_eq!(
        sessions.first().map(|session| session.id),
        Some(pane.session_id)
    );
}

#[test]
fn replacing_the_binary_out_from_under_the_daemon_does_not_lose_its_sessions() {
    let _upgrade_test_guard = upgrade_test_guard();
    // Rebuilding unlinks and recreates the executable, after which Linux reads
    // `/proc/self/exe` as "<path> (deleted)". Resolving the path at upgrade
    // time rather than at startup made `--upgrade` try to execute that, and
    // fail with a confusing error about a path nobody wrote.
    let directory = tempfile::tempdir().unwrap();
    let copied = directory.path().join("zmux");
    std::fs::copy(daemon_binary(), &copied).unwrap();

    let config = tempfile::tempdir().unwrap();
    let mut process = spawn_copied_daemon(&copied, config.path());
    // The copied binary is deliberately outside Cargo's target/debug tree, so
    // it uses the installed application's ordinary session directory.
    let sessions = config.path().join("zetta/sessions");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sessions.join("zmux.json").is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = loop {
        if let Ok(Some(client)) = Client::connect_ready_at(&sessions) {
            break client;
        }
        assert!(
            Instant::now() < deadline,
            "the copied daemon never became ready; process: {}; log:\n{}",
            process.id(),
            std::fs::read_to_string(config.path().join("zmux.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Exactly what a rebuild does to the running daemon's executable.
    std::fs::remove_file(&copied).unwrap();

    let error = client
        .upgrade()
        .expect_err("an upgrade with no executable to run must be refused");
    assert!(
        error.to_string().contains("no longer exists"),
        "expected a clear reason, got {error:#}"
    );

    // The point: refusing costs nothing, whereas attempting it would have
    // taken the sessions down.
    assert!(process_is_alive(pane.child_pid), "the session was lost");
    assert_eq!(client.list().unwrap().len(), 0);
    assert!(
        client.spawn(spawn_request(None, "sleep 1")).is_ok(),
        "the multiplexer must still be serving"
    );

    let _ = process.kill();
    let _ = process.wait();
}

#[test]
fn a_session_holding_no_panes_is_never_offered() {
    // The failure this guards against: a session with zero panes appearing in
    // the listing, with no title and nothing to attach. "Every pane is
    // unattached" is vacuously true of no panes, so the reclaim marked it as
    // available and published it.
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    // Ending the session leaves the multiplexer holding nothing for it. Give
    // the reclaim more than one pass to notice.
    client.kill(pane.session_id).unwrap();
    std::thread::sleep(Duration::from_secs(5));

    assert!(
        client.list().unwrap().is_empty(),
        "a session with no panes was offered: {:?}",
        client.list().unwrap()
    );

    // And the published catalog agrees, since that is what `zmux list`
    // and the reconnect picker read.
    let catalog = std::fs::read_dir(daemon.sessions_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("zetta-"))
        });
    if let Some(catalog) = catalog {
        let contents = std::fs::read_to_string(catalog).unwrap();
        assert!(
            !contents.contains("\"panes\": []"),
            "the catalog offered a session with no panes: {contents}"
        );
    }
}

/// A session whose process has ended is not offered, and attaching to it is
/// refused rather than handed a terminal that can never produce output again.
#[test]
fn a_session_whose_process_ended_is_never_offered() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    // The daemon keeps the shell as its child, so the test cannot wait() on
    // it. Kill it the way a process ends, and poll until the reaper has seen
    // it — the reaper only runs on SIGCHLD.
    unsafe { libc::kill(pane.child_pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "the pane's shell never exited"
    );

    // The session must be gone from the listing, not merely marked unavailable:
    // the reaper ends it as soon as its process ends.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut sessions = Vec::new();
    while Instant::now() < deadline {
        sessions = client.list().unwrap();
        if sessions.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        sessions.is_empty(),
        "a session whose process ended was still offered: {sessions:?}"
    );

    // And attaching to it must fail cleanly, not yield a dead terminal that
    // looks like a hung window.
    assert!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some(TEST_SECRET.to_owned()),
            )
            .is_err(),
        "attaching to an ended session must be refused"
    );
}

#[test]
fn upgrading_twice_does_not_duplicate_a_session() {
    let _upgrade_test_guard = upgrade_test_guard();
    // Each upgrade hands the sessions to the next image. If that image adopts
    // them without accounting for what it already has — or restores the
    // identifier counters in the wrong order — the same session comes back
    // twice, listed identically and impossible to tell apart.
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    for round in 1..=2 {
        let client = wait_for_multiplexer(&daemon);
        client.upgrade().unwrap();
        let client = wait_for_multiplexer(&daemon);
        let sessions = client.list().unwrap();
        assert_eq!(
            sessions.len(),
            1,
            "after upgrade {round} the multiplexer offered {sessions:?}"
        );
    }
}

/// Waits until a multiplexer is answering, which after an upgrade means the
/// replacement has rebound rather than merely that the socket accepts.
fn wait_for_multiplexer(daemon: &TestDaemon) -> Client {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(Some(client)) = Client::connect_ready_at(&daemon.sessions_dir())
            && client.list().is_ok()
        {
            return client;
        }
        assert!(
            Instant::now() < deadline,
            "the multiplexer never came back (process: {}); log:\n{}",
            daemon.process.id(),
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Lists sessions once the replacement can answer the list request itself.
///
/// A readiness ping can succeed in the outgoing daemon's final instant before
/// `exec`. The next connection then sees its deliberate EOF, so a test that
/// needs the listing must retry that listing rather than reuse the client that
/// answered the earlier ping.
fn wait_for_session_list(daemon: &TestDaemon) -> Vec<BackgroundSessionSummary> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(Some(client)) = Client::connect_ready_at(&daemon.sessions_dir())
            && let Ok(sessions) = client.list()
        {
            return sessions;
        }
        assert!(
            Instant::now() < deadline,
            "the multiplexer never answered a session list (process: {}); log:\n{}",
            daemon.process.id(),
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Starts a daemon from a freshly copied binary, retrying `ETXTBSY`.
///
/// Copying a file and immediately executing it races the other tests: these run
/// as threads of one process, so a `Command::spawn` on any other thread forks a
/// child that inherits this copy's still-open write descriptor, and `execve`
/// refuses to run a file anyone holds open for writing. The child closes it on
/// its own exec moments later, so retrying is the fix; the alternative is a test
/// that fails for a reason that has nothing to do with what it checks.
fn spawn_copied_daemon(binary: &Path, config: &Path) -> Child {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match Command::new(binary)
            .arg("--daemon")
            .env("XDG_CONFIG_HOME", config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(config.join("zmux.log")).unwrap())
            .spawn()
        {
            Ok(process) => return process,
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("starting the copied daemon failed: {error}"),
        }
    }
}

/// Whether a process has ended but not been reaped.
///
/// `kill(pid, 0)` succeeds for a zombie, so it cannot tell "still running" from
/// "ended and never waited for" — which is exactly the distinction that matters
/// when checking that the daemon still reaps its own children after replacing
/// itself.
fn process_is_zombie(pid: u32) -> bool {
    let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // The state is the field after the parenthesised command name, which may
    // itself contain spaces and parentheses.
    let Some(after_name) = status.rsplit_once(") ") else {
        return false;
    };
    after_name.1.starts_with('Z')
}

/// An upgrade while a client is still holding a pane's terminal.
///
/// The upgrade tests above all detach first, which meant the daemon only ever
/// re-adopted panes nobody was holding — and so never exercised the case that
/// broke: an adopted pane whose child the replacement treated as somebody
/// else's, never reaping it and reading its dead signal pipe as an exit.
#[test]
fn an_upgrade_keeps_a_pane_that_is_still_attached() {
    let _upgrade_test_guard = upgrade_test_guard();
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let child_pid = pane.child_pid;
    let (pty, events) = alacritty_terminal::tty::attach(pane.descriptor, pane.child_pid).unwrap();
    reporters.register(pane_id, events);
    // Kept, so the session is the daemon's to hold across the upgrade while
    // this client still holds the pane exclusively.
    client
        .detach(
            session_id,
            summary(session_id, pane_id),
            serde_json::json!({}),
            None,
            Vec::new(),
        )
        .unwrap();
    match client
        .attach(session_id, Some(pane_id), Some(TEST_SECRET.to_owned()))
        .unwrap()
    {
        AttachOutcome::Attached { .. } => {}
        _ => panic!("expected an exclusive attach"),
    }

    client.upgrade().unwrap();
    let client = wait_for_multiplexer(&daemon);

    assert!(process_is_alive(child_pid), "the pane's process was ended");
    // The terminal this client is holding still works. Its descriptor was never
    // the multiplexer's to touch, and an upgrade that left it hung up would be
    // indistinguishable to the user from the pane having died.
    assert_terminal_echoes(pty.file(), "still-alive-after-upgrade");
    // Asked about directly rather than through the catalog: a session a window
    // is showing is not listed there, because it is on screen rather than
    // waiting to be picked up.
    let states = client.pane_states(vec![pane_id]).unwrap();
    assert_eq!(
        states.iter().filter(|state| !state.unknown).count(),
        1,
        "the session was dropped; log:\n{}",
        daemon.log()
    );

    // The replacement must not have decided the pane ended. Reading a carried
    // signal pipe whose writer the exec had closed reported end-of-file, which
    // the reaper took for an exit — so a shell that was running perfectly well
    // was marked dead and pruned. Give the reaper something to wake on first:
    // it only re-examines panes when a child ends.
    reap(
        Command::new(system_executable("true"))
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(300));
    let states = client.pane_states(vec![pane_id]).unwrap();
    assert_eq!(states.len(), 1);
    assert!(
        !states[0].unknown && !states[0].exited,
        "the replacement reported a running pane as ended: {:?}; log:\n{}",
        states[0],
        daemon.log()
    );

    // And it must still be able to reap it, which it can only do because an
    // `execv` keeps the parent/child relationship. Treating the child as
    // foreign left it a zombie whose exit was never reported to anybody.
    unsafe { libc::kill(child_pid as i32, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let states = client.pane_states(vec![pane_id]).unwrap();
        if states[0].exited || states[0].unknown {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the replacement never reaped the pane's process (zombie: {}); log:\n{}",
            process_is_zombie(child_pid),
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !process_is_zombie(child_pid),
        "the replacement noticed the exit but never reaped the process"
    );
}

/// A client whose subscription an upgrade tore down still learns about exits.
///
/// This is the regression that mattered most: a lost subscription used to be
/// reported to every attached pane as its own process having ended — so a
/// `zmux --upgrade` showed an error in panes that were running fine — and the
/// subscription was never re-established, so no pane opened afterwards could
/// ever be told it had exited.
#[test]
fn a_subscription_survives_the_multiplexer_replacing_itself() {
    let _upgrade_test_guard = upgrade_test_guard();
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();

    let held = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let session_id = held.session_id;
    let (mut pty, events) =
        alacritty_terminal::tty::attach(held.descriptor, held.child_pid).unwrap();
    reporters.register(held.pane_id, events);
    client
        .detach(
            session_id,
            summary(session_id, held.pane_id),
            serde_json::json!({}),
            None,
            Vec::new(),
        )
        .unwrap();
    match client
        .attach(session_id, Some(held.pane_id), Some(TEST_SECRET.to_owned()))
        .unwrap()
    {
        AttachOutcome::Attached { .. } => {}
        _ => panic!("expected an exclusive attach"),
    }

    client.upgrade().unwrap();
    let client = wait_for_multiplexer(&daemon);

    // Nothing may have been reported for the held pane. Its process is running,
    // its terminal is this test's to read, and the only thing that changed is a
    // connection that carried no data for it.
    use alacritty_terminal::tty::EventedPty as _;
    assert!(
        pty.next_child_event().is_none(),
        "losing the subscription was reported as the pane's process ending"
    );
    assert_terminal_echoes(pty.file(), "held-pane-still-usable");

    // The subscription reconnects on its own, so a pane spawned after the
    // upgrade is still told when it ends.
    let deadline = Instant::now() + Duration::from_secs(30);
    let fresh = loop {
        match client.spawn(spawn_request(None, "exit 5")) {
            Ok(pane) => break pane,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("could not spawn after the upgrade: {error:#}"),
        }
    };
    let (mut fresh_pty, fresh_events) =
        alacritty_terminal::tty::attach(fresh.descriptor, fresh.child_pid).unwrap();
    reporters.register(fresh.pane_id, fresh_events);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match fresh_pty.next_child_event() {
            Some(alacritty_terminal::tty::ChildEvent::Exited(status)) => {
                assert_eq!(status.code(), Some(5));
                break;
            }
            Some(other) => panic!("expected an exit status, got {other:?}"),
            None => {}
        }
        assert!(
            Instant::now() < deadline,
            "a pane spawned after the upgrade was never told it exited; log:\n{}",
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// An exit that happens while nobody is subscribed is still reported.
///
/// Events are broadcast to whoever is listening at that instant, so an exit
/// during the gap between a daemon going away and its replacement being
/// subscribed to has already been and gone. Without reconciling on reconnect,
/// the pane showing it would wait for a notification that no longer exists.
#[test]
fn an_exit_missed_while_disconnected_is_reported_on_reconnect() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client.spawn(spawn_request(None, "sleep 60")).unwrap();
    let (_pty, events) = alacritty_terminal::tty::attach(pane.descriptor, pane.child_pid).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({}),
            None,
            Vec::new(),
        )
        .unwrap();
    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { .. } => {}
        _ => panic!("expected an exclusive attach"),
    }

    // Ends while there is deliberately no subscriber at all, so the broadcast
    // reaches nobody.
    unsafe { libc::kill(pane.child_pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let states = client.pane_states(vec![pane.pane_id]).unwrap();
        if states[0].exited || states[0].unknown {
            break;
        }
        assert!(Instant::now() < deadline, "the process never ended");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Subscribing now and registering the reporter must deliver the exit that
    // has already happened.
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();
    let missed = client.pane_states(vec![pane.pane_id]).unwrap();
    reporters.register(pane.pane_id, events);
    for report in missed {
        assert!(
            report.exited || report.unknown,
            "the multiplexer forgot that the pane had ended"
        );
    }
}

/// Stopping the multiplexer without hunting for its process.
///
/// The alternative is `pkill zmux`, which matches every multiplexer this user
/// is running and ends their sessions without asking. So the refusal is the
/// feature: a daemon holding a session says how many and stays up, and --force
/// is the caller saying they meant to end what it was holding.
#[test]
fn the_multiplexer_can_be_asked_to_stop() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    let error = format!(
        "{:#}",
        zmux::stop(&daemon.sessions_dir(), false)
            .expect_err("a multiplexer holding a session must not be stopped by accident")
    );
    assert!(error.contains("holding 1 session"), "{error}");
    assert!(error.contains("--force"), "{error}");
    assert!(
        Client::connect_ready_at(&daemon.sessions_dir())
            .unwrap()
            .is_some(),
        "a refused stop must leave the multiplexer running"
    );

    // Forced, it goes, and what it was holding goes with it.
    drop(descriptor);
    drop(client);
    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), true).expect("stopping the multiplexer"),
        zmux::StopOutcome::Signalled {
            process_id: daemon.process_id(),
        },
    );
    assert!(
        Client::connect_ready_at(&daemon.sessions_dir())
            .unwrap()
            .is_none(),
        "the multiplexer must be gone once it says it stopped: {}",
        daemon.log()
    );

    // And stopping what is not running is not a failure: the request is that it
    // not be running, and it is not.
    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), false).expect("stopping nothing"),
        zmux::StopOutcome::NotRunning,
    );
}

/// An idle multiplexer is asked, not signalled: it removes its own socket and
/// endpoint on the way out, which a signal cannot make it do.
#[test]
fn an_idle_multiplexer_stops_when_asked() {
    let daemon = TestDaemon::start();

    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), false).expect("stopping the multiplexer"),
        zmux::StopOutcome::Stopped,
    );
    assert!(
        !daemon.sessions_dir().join("zmux.sock").exists(),
        "a multiplexer that stopped when asked cleans up after itself: {}",
        daemon.log()
    );
}

/// `--upgrade` is usable across a protocol boundary, not merely accepted at one.
///
/// The daemon exempts `Request::Upgrade` from its version check, and the command
/// still failed: the *client* refused to connect to a multiplexer whose version
/// it disagreed with, so the one command that exists to cross that boundary
/// reported the boundary instead of crossing it. Rebuilding Zetta creates exactly
/// this situation, which is when the command matters.
#[test]
fn an_upgrade_connects_to_a_multiplexer_of_another_protocol_version() {
    let _upgrade_test_guard = upgrade_test_guard();
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let child_pid = pane.child_pid;
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    drop(descriptor);

    // What a rebuild leaves behind: a running multiplexer advertising a protocol
    // this build does not speak.
    let endpoint_path = daemon.sessions_dir().join("zmux.json");
    let mut endpoint: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&endpoint_path).unwrap()).unwrap();
    endpoint["protocol_version"] = serde_json::json!(zmux::messages::PROTOCOL_VERSION - 1);
    std::fs::write(&endpoint_path, serde_json::to_vec(&endpoint).unwrap()).unwrap();

    // Every other command says so, and says what to do about it.
    let error = match Client::connect_existing_at(&daemon.sessions_dir()) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("an ordinary connection must refuse the version"),
    };
    assert!(
        error.contains(&format!(
            "protocol version {}",
            zmux::messages::PROTOCOL_VERSION - 1
        )),
        "{error}"
    );
    assert!(error.contains("--upgrade"), "{error}");

    // The upgrade goes through, and the session it was holding survives it.
    let upgrading = Client::connect_for_upgrade_at(&daemon.sessions_dir())
        .expect("connecting across the version boundary")
        .expect("the multiplexer is running");
    upgrading.upgrade().expect("replacing the multiplexer");

    let client = wait_for_multiplexer(&daemon);
    assert!(
        process_is_alive(child_pid),
        "the session's process was ended by the upgrade"
    );
    assert_eq!(
        client.list().unwrap().len(),
        1,
        "the session was dropped; log:\n{}",
        daemon.log()
    );
}

/// A pane's remembered size is a stand-in on Unix: a client resizes through the
/// descriptor it holds and tells the multiplexer nothing, so only the pty knows
/// the real geometry. `--upgrade` recorded the stand-in and rebuilt the retained
/// screen from it in the next image, re-wrapping a full-screen program's screen
/// at a width it was never drawn at — htop came back as fragments of itself.
#[cfg(all(unix, feature = "scrollback-buffer"))]
#[test]
fn an_upgrade_keeps_the_retained_screen_at_the_width_it_was_drawn_at() {
    let _upgrade_test_guard = upgrade_test_guard();
    use std::os::fd::AsRawFd as _;

    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // What a window does after its first layout, and what the multiplexer never
    // hears about: the spawn asked for the stand-in geometry, this is the real
    // one.
    let size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe { libc::ioctl(descriptor.as_raw_fd(), libc::TIOCSWINSZ, &size) },
        0,
        "resizing the pty"
    );

    // Wider than the stand-in, so a grid rebuilt at the wrong width has to wrap
    // it and the damage is visible in the replay. Deliberately report the stale
    // spawn size too: snapshot ingestion must not put that metadata back after
    // it has queried the live PTY.
    let line = "W".repeat(100);
    client
        .send_snapshot(
            pane.session_id,
            pane.pane_id,
            line.clone().into_bytes(),
            80,
            24,
        )
        .unwrap();
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    client.upgrade().unwrap();
    // macOS rebinds the listener after exec so its peer-credential state is
    // rebuilt; wait for an answered request rather than only a successful
    // connect during that short gap.
    let client = wait_for_multiplexer(&daemon);

    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => {
            let replay = String::from_utf8_lossy(&pane.replay).into_owned();
            assert!(
                replay.contains(&line),
                "the retained screen was re-wrapped by the upgrade: {replay:?}"
            );
        }
        _ => panic!("attach failed"),
    }
}
