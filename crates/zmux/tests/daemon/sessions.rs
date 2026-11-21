//! A session's life in the daemon: outliving its client, replaying output on
//! attach, what it retains of that output, ending its process, and the
//! socket's privacy.

use super::*;

fn try_read_json_frame(stream: &mut UnixStream) -> Option<serde_json::Value> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).ok()?;
    let mut bytes = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Exercises the client-side retry without needing an executable from an older
/// checkout. The first server image rejects Configure exactly as a pre-Configure
/// daemon does; its accepted Upgrade flips the listener into the replacement
/// image, which accepts the raw retry. The real daemon tests cover the PTY and
/// session state carried by an upgrade separately.
#[test]
fn configure_retries_once_after_an_unsupported_request() {
    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("zmux.sock");
    let endpoint_path = directory.path().join("zmux.json");
    let token = "test-token".to_owned();
    let listener = UnixListener::bind(&socket_path).unwrap();
    std::fs::write(
        &endpoint_path,
        serde_json::json!({
            "version": zmux::transport::ENDPOINT_VERSION,
            "protocol_version": zmux::messages::PROTOCOL_VERSION,
            "process_id": 4242,
            "socket_path": socket_path,
            "token": token,
        })
        .to_string(),
    )
    .unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let upgraded = Arc::new(AtomicBool::new(false));
    let configure_requests = Arc::new(AtomicUsize::new(0));
    let upgrade_requests = Arc::new(AtomicUsize::new(0));
    let server = std::thread::spawn({
        let stop = stop.clone();
        let upgraded = upgraded.clone();
        let configure_requests = configure_requests.clone();
        let upgrade_requests = upgrade_requests.clone();
        move || {
            while !stop.load(Ordering::Acquire) {
                let (mut stream, _) = listener.accept().unwrap();
                let Some(request) = try_read_json_frame(&mut stream) else {
                    continue;
                };
                let name = request["request"]["request"].as_str();
                let response = match name {
                    // `configure_raw` verifies that the daemon is serving
                    // requests before each attempt. A pre-Configure daemon
                    // can answer Ping even though it rejects Configure.
                    Some("ping") => serde_json::json!({"response": "ok"}),
                    Some("configure") => {
                        configure_requests.fetch_add(1, Ordering::Relaxed);
                        if upgraded.load(Ordering::Acquire) {
                            serde_json::json!({"response": "ok"})
                        } else {
                            serde_json::json!({
                                "response": "error",
                                "message": "unknown variant `configure`, expected `spawn`"
                            })
                        }
                    }
                    Some("upgrade") => {
                        upgrade_requests.fetch_add(1, Ordering::Relaxed);
                        upgraded.store(true, Ordering::Release);
                        serde_json::json!({"response": "ok"})
                    }
                    _ => serde_json::json!({
                        "response": "error",
                        "message": "unexpected request in configure retry test"
                    }),
                };
                write_json_frame(&mut stream, &response);
            }
        }
    });

    let client = Client::connect_existing_at(directory.path())
        .unwrap()
        .unwrap();
    client.configure(Retention::None, Vec::new()).unwrap();
    assert_eq!(configure_requests.load(Ordering::Acquire), 2);
    assert_eq!(upgrade_requests.load(Ordering::Acquire), 1);

    stop.store(true, Ordering::Release);
    let _ = UnixStream::connect(&socket_path);
    server.join().unwrap();
}

#[test]
fn a_session_outlives_the_client_that_started_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    // A command that keeps running, and announces itself so the test can tell
    // when the shell is actually up.
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    assert!(process_is_alive(pane.child_pid));

    // Detaching is the client dropping the terminal and telling the daemon to
    // hold the session.
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"tab": "state"}),
            None,
            Vec::new(),
        )
        .unwrap();

    // The client is gone, but the process is the daemon's child.
    drop(client);
    assert!(process_is_alive(pane.child_pid));

    let client = daemon.client();
    assert_eq!(client.list().unwrap().len(), 1);

    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
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
        _ => panic!("an unprotected session must attach without a secret"),
    }
}

/// A session's identity belongs to the daemon, not to whatever id a client
/// puts in the summary it detaches with.
///
/// Zetta used to publish its own tab id there, which diverged from the mux
/// session id: the catalog then listed a session under one id while attach,
/// kill and resize looked for it under another. The session showed up in the
/// list but "did not exist" when anything tried to address it.
#[test]
fn a_client_summary_cannot_rename_a_session() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    // A summary whose id is deliberately wrong, as a client publishing its own
    // identifier would have been.
    let mut impostor = summary(pane.session_id, pane.pane_id);
    impostor.id = pane.session_id + 1000;
    client
        .detach(
            pane.session_id,
            impostor,
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    // Listed under the mux session id, not the id the client claimed.
    let listed = client.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, pane.session_id);

    // And addressable under that id.
    let attached = match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => pane,
        _ => panic!("the session must still attach under its own id"),
    };
    drop(attached);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    client.kill(pane.session_id).unwrap();
    assert!(client.list().unwrap().is_empty());
}

/// Only meaningful when the ring is compiled in: without it there is, by
/// design, nothing to replay.
#[cfg(feature = "scrollback-buffer")]
#[test]
fn output_produced_while_detached_is_replayed_on_attach() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; sleep 0.3; printf 'while-away'; sleep 60",
        ))
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
            vec![(pane.pane_id, b"snapshot-of-screen".to_vec())],
        )
        .unwrap();

    // Give the pane time to produce output with nobody attached.
    std::thread::sleep(Duration::from_millis(800));

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
            // The screen as it was, then what happened while nobody was there.
            assert!(
                replay.starts_with("snapshot-of-screen"),
                "replay was {replay:?}"
            );
            assert!(replay.contains("while-away"), "replay was {replay:?}");
        }
        _ => panic!("attach failed"),
    }
}

#[test]
fn a_protected_session_needs_its_secret() {
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
            serde_json::Value::Null,
            Some(&verifier),
            Vec::new(),
        )
        .unwrap();

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
                Some("wrong".to_owned())
            )
            .unwrap(),
        AttachOutcome::AuthenticationFailed
    ));

    // The catalog must not describe a protected session, and must never carry
    // the verifier.
    let catalog = std::fs::read_to_string(
        std::fs::read_dir(daemon.sessions_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("zetta-"))
            })
            .expect("a published catalog"),
    )
    .unwrap();
    assert!(catalog.contains("Protected session"));
    assert!(!catalog.contains("argon2"));

    // The socket must not say more than the catalog does. The endpoint token
    // authenticates the channel, not a session: anything that could read it
    // would otherwise learn a protected session's commands and directories
    // without ever presenting its secret.
    let listed = client.list().unwrap();
    let session = listed
        .iter()
        .find(|listed| listed.id == pane.session_id)
        .expect("the session is held");
    assert!(session.authentication_required);
    assert_eq!(session.title, "Protected session");
    assert!(
        session.panes.is_empty(),
        "listing revealed a protected session's panes: {:?}",
        session.panes
    );

    // And attaching cannot be used to enumerate its panes either.
    assert!(matches!(
        client
            .attach(pane.session_id, Some(pane.pane_id), None)
            .unwrap(),
        AttachOutcome::AuthenticationRequired
    ));
}

#[test]
fn killing_a_session_ends_what_it_was_running() {
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

    client.kill(pane.session_id).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "the session kept running"
    );
    assert!(client.list().unwrap().is_empty());
}

#[test]
fn an_attached_pane_learns_that_its_process_exited() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();

    // An attached client reads the real terminal, so only the daemon — the
    // process's actual parent — can tell it how the process ended.
    let pane = client
        .spawn(spawn_request(None, "printf ready; exit 7"))
        .unwrap();
    let (mut pty, events) =
        alacritty_terminal::tty::attach(pane.descriptor, pane.child_pid).unwrap();
    // Drain the command's initial output. macOS can keep a PTY child in the
    // exiting path until the master has consumed those bytes.
    read_until(pty.file(), "ready");
    reporters.register(pane.pane_id, events);

    use alacritty_terminal::tty::EventedPty as _;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(event) = pty.next_child_event() {
            match event {
                alacritty_terminal::tty::ChildEvent::Exited(status) => {
                    assert_eq!(status.code(), Some(7));
                    return;
                }
                other => panic!("expected an exit status, got {other:?}"),
            }
        }
        assert!(Instant::now() < deadline, "the exit was never reported");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_socket_and_its_token_are_private_to_this_user() {
    use std::os::unix::fs::PermissionsExt as _;
    let daemon = TestDaemon::start();
    let sessions = daemon.sessions_dir();

    for name in ["zmux.json", "zmux.sock"] {
        let mode = std::fs::metadata(sessions.join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{name} must not be shared");
    }
    let directory = std::fs::metadata(&sessions).unwrap().permissions().mode();
    assert_eq!(directory & 0o777, 0o700);
}

#[test]
fn a_connection_with_the_wrong_token_gets_nothing() {
    let daemon = TestDaemon::start();
    let endpoint_path = daemon.sessions_dir().join("zmux.json");
    let endpoint: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&endpoint_path).unwrap()).unwrap();
    let socket = endpoint["socket_path"].as_str().unwrap();

    let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
    let request = serde_json::json!({
        "version": zmux::messages::PROTOCOL_VERSION,
        "token": "0".repeat(64),
        "request": {"request": "list"},
    });
    write_json_frame(&mut stream, &request);
    let response = read_json_frame(&mut stream);
    assert!(response.contains("invalid multiplexer token"), "{response}");
    assert!(!response.contains("\"sessions\""));
}

/// Fails early, and with a clearer message than a timeout, when the daemon
/// these tests drive was not rebuilt after a change.
#[test]
fn the_daemon_binary_is_current() {
    daemon_binary();
}

#[test]
fn retention_none_keeps_a_session_running_without_holding_its_output() {
    // The memory-constrained case: the multiplexer must still read a detached
    // pane, or its child blocks once the terminal buffer fills, but it keeps
    // none of what it reads.
    let daemon = TestDaemon::start_with(&["--retention", "none"]);
    let client = daemon.client();

    // Writing more than a terminal buffer holds only completes if something is
    // draining it. The marker file is the evidence: a child blocked mid-write
    // is still "alive", so liveness alone would prove nothing.
    let marker = daemon.config.join("finished-writing");
    let release = daemon.config.join("start-writing");
    // The readiness read must not drain the workload, and PTY buffer capacity
    // must not decide whether the child reaches its marker before detach.
    let mut request = spawn_request(
        None,
        "printf ready; while [ ! -f \"$ZMUX_TEST_RELEASE\" ]; do sleep 0.05; done; \
         i=0; while [ $i -lt 4000 ]; do \
         printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n'; \
         i=$((i+1)); done; : > \"$ZMUX_TEST_MARKER\"; sleep 60",
    );
    request.env.insert(
        "ZMUX_TEST_RELEASE".to_owned(),
        release.to_string_lossy().into_owned(),
    );
    request.env.insert(
        "ZMUX_TEST_MARKER".to_owned(),
        marker.to_string_lossy().into_owned(),
    );
    let pane = client.spawn(request).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    assert!(
        !marker.exists(),
        "the writes finished before the pane detached"
    );

    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, b"a snapshot that must not be kept".to_vec())],
        )
        .unwrap();

    std::fs::write(&release, []).expect("releasing the detached pane to write");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        marker.exists(),
        "the detached pane blocked on a terminal buffer nobody was draining"
    );

    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => assert!(
            pane.replay.is_empty(),
            "retention \"none\" retained {} bytes",
            pane.replay.len()
        ),
        _ => panic!("attach failed"),
    }
}

#[test]
fn reconfiguring_to_none_discards_a_snapshot_but_keeps_the_session_process() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    client.configure(Retention::None, Vec::new()).unwrap();
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, b"this snapshot is discarded".to_vec())],
        )
        .unwrap();

    match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane: attached, .. } => {
            assert_eq!(attached.child_pid, pane.child_pid);
            assert!(attached.replay.is_empty(), "retention none replayed output");
            drop(std::fs::File::from(attached.descriptor));
        }
        _ => panic!("the reconfigured session must attach exclusively"),
    }
    assert!(process_is_alive(pane.child_pid));
    client.kill(pane.session_id).unwrap();
}

#[cfg(feature = "scrollback-buffer")]
#[test]
fn reconfiguring_to_memory_retains_a_snapshot_and_keeps_the_session_process() {
    let daemon = TestDaemon::start_with(&["--retention", "none"]);
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    client
        .configure(Retention::Memory { bytes: 4096 }, Vec::new())
        .unwrap();
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, b"snapshot retained after reload".to_vec())],
        )
        .unwrap();

    match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane: attached, .. } => {
            assert_eq!(attached.child_pid, pane.child_pid);
            assert!(
                String::from_utf8_lossy(&attached.replay)
                    .starts_with("snapshot retained after reload"),
                "the configured memory ring did not retain the snapshot: {:?}",
                attached.replay
            );
            drop(std::fs::File::from(attached.descriptor));
        }
        _ => panic!("the reconfigured session must attach exclusively"),
    }
    assert!(process_is_alive(pane.child_pid));
    client.kill(pane.session_id).unwrap();
}

#[test]
fn closing_a_pane_that_ignores_hangup_does_not_block_the_daemon() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    // A finite fallback keeps the regression from leaving a stuck test daemon.
    let pane = client
        .spawn(spawn_request(
            None,
            "trap '' HUP; printf ready; exec sleep 5",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let started = Instant::now();
    client.close_pane(pane.session_id, pane.pane_id).unwrap();
    let next = client.spawn(spawn_request(None, "sleep 5")).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "closing a pane blocked new terminals"
    );
    client.close_pane(next.session_id, next.pane_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "closed pane's child was not reaped"
    );
}

#[test]
fn rapid_spawn_and_close_keeps_the_daemon_responsive() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let mut children = Vec::new();
    for _ in 0..50 {
        let pane = client.spawn(spawn_request(None, "exec sleep 30")).unwrap();
        children.push(pane.child_pid);
        drop(pane.descriptor);
        client.close_pane(pane.session_id, pane.pane_id).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while children.iter().any(|pid| process_is_alive(*pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(children.iter().all(|pid| !process_is_alive(*pid)));
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf still-working"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "still-working");
    client.close_pane(pane.session_id, pane.pane_id).unwrap();
}

/// No pane ever starts in the daemon's own directory.
///
/// A daemon is started detached, so its working directory is an accident —
/// `/` on macOS — and a pane that inherits it opens on the root of the
/// filesystem. This is the floor under the inherit above: when the requester
/// names no directory and none can be read from the pane being split, the new
/// pane still starts somewhere a person would want to be.
#[test]
fn a_pane_never_starts_in_the_daemons_own_directory() {
    let daemon = TestDaemon::start_in(&[], Some(Path::new("/")));
    let client = daemon.client();
    let home = std::env::var("HOME").expect("a home directory");

    // A plain spawn that names no directory.
    let mut request = spawn_request(None, "pwd; sleep 60");
    request.working_directory = None;
    let pane = client.spawn(request).expect("spawning a pane");
    let descriptor = std::fs::File::from(pane.descriptor);
    let printed = read_until(&descriptor, "\n");
    assert_eq!(
        printed.lines().next().map(str::trim),
        Some(home.as_str()),
        "a pane with no directory of its own has to start at home, not in the \
         daemon's `/`"
    );

    // And a shared draft whose inherit cannot be resolved: pane 999 does not
    // exist, which is the same dead end as a pane whose process is gone.
    let offered = summary(pane.session_id, pane.pane_id);
    client
        .share(
            pane.session_id,
            offered,
            serde_json::Value::Null,
            Some(&test_verifier()),
            true,
        )
        .expect("sharing the session");
    let zmux::client::SharedBatchResult::Applied(spawned) = client
        .spawn_shared_batch(SharedSpawnBatchRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            target_pane_id: Some(pane.pane_id),
            replacement: zmux::messages::SharedDraftLayout::Split {
                axis: "vertical".to_owned(),
                first_ratio: 500,
                first: Box::new(zmux::messages::SharedDraftLayout::Existing {
                    pane_id: pane.pane_id,
                }),
                second: Box::new(zmux::messages::SharedDraftLayout::Draft { draft_id: 1 }),
            },
            panes: vec![zmux::messages::SharedPaneDraft {
                draft_id: 1,
                profile: "System".to_owned(),
                command: Some(zetta_profiles::ProfileCommand::with_args(
                    "sh",
                    vec!["-c".to_owned(), "pwd; sleep 60".to_owned()],
                )),
                env: HashMap::new(),
                working_directory: None,
                inherit_working_directory_from: Some(999),
                load_shell_integration: false,
                size: TerminalSize {
                    columns: 80,
                    lines: 24,
                    cell_width: 0,
                    cell_height: 0,
                },
                console_palette: ConsolePalette::default(),
                metadata: pane_summary(0),
            }],
            active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 1 }),
        })
        .expect("splitting the shared pane")
    else {
        panic!("the split conflicted")
    };

    let AttachOutcome::SharedAttached { pane: split, .. } = client
        .attach_shared_with_secret(
            pane.session_id,
            spawned.mappings[0].pane_id,
            Some(&SessionSecret::new(TEST_SECRET.to_owned())),
        )
        .expect("attaching the new pane")
    else {
        panic!("the new pane did not attach as a stream")
    };
    let printed = collect_output(split.reader());
    printed
        .lock()
        .unwrap()
        .insert_str(0, &String::from_utf8_lossy(&split.replay));
    wait_for_shared_output(&printed, &home);
}

/// A pane split from one on another machine has to start where that pane is.
///
/// The viewer cannot say where that is — a pane reports its directory to the
/// window running it, and a remote window is not that one — so the draft names
/// the pane instead of a path and the daemon reads it from the process it is
/// the parent of. When that read fails the pane starts wherever the daemon
/// happens to be, which is `/`, and that is what this pins.
#[test]
fn a_draft_inherits_the_working_directory_of_the_pane_it_names() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let directory = tempfile::tempdir().expect("a directory for the source pane");
    // The daemon reads the child's own directory, and macOS hands back the
    // resolved path for a symlinked temporary directory.
    let directory = directory.path().canonicalize().expect("a real path");

    let mut source = spawn_request(None, "printf ready; sleep 60");
    source.working_directory = Some(directory.clone());
    let pane = client.spawn(source).expect("spawning the source pane");
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let offered = summary(pane.session_id, pane.pane_id);
    client
        .share(
            pane.session_id,
            offered,
            serde_json::Value::Null,
            Some(&test_verifier()),
            true,
        )
        .expect("sharing the session");

    let zmux::client::SharedBatchResult::Applied(spawned) = client
        .spawn_shared_batch(SharedSpawnBatchRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            target_pane_id: Some(pane.pane_id),
            replacement: zmux::messages::SharedDraftLayout::Split {
                axis: "vertical".to_owned(),
                first_ratio: 500,
                first: Box::new(zmux::messages::SharedDraftLayout::Existing {
                    pane_id: pane.pane_id,
                }),
                second: Box::new(zmux::messages::SharedDraftLayout::Draft { draft_id: 1 }),
            },
            panes: vec![zmux::messages::SharedPaneDraft {
                draft_id: 1,
                profile: "System".to_owned(),
                command: Some(zetta_profiles::ProfileCommand::with_args(
                    "sh",
                    vec!["-c".to_owned(), "pwd; sleep 60".to_owned()],
                )),
                env: HashMap::new(),
                // What a viewer sends when it cannot know the directory: the
                // pane to start in the same place as, rather than a path.
                working_directory: None,
                inherit_working_directory_from: Some(pane.pane_id),
                load_shell_integration: false,
                size: TerminalSize {
                    columns: 80,
                    lines: 24,
                    cell_width: 0,
                    cell_height: 0,
                },
                console_palette: ConsolePalette::default(),
                metadata: pane_summary(0),
            }],
            active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 1 }),
        })
        .expect("splitting the shared pane")
    else {
        panic!("the split conflicted")
    };

    let AttachOutcome::SharedAttached { pane: split, .. } = client
        .attach_shared_with_secret(
            pane.session_id,
            spawned.mappings[0].pane_id,
            Some(&SessionSecret::new(TEST_SECRET.to_owned())),
        )
        .expect("attaching the new pane")
    else {
        panic!("the new pane did not attach as a stream")
    };

    // `pwd` has already run by the time this attaches, so what it printed is
    // in the pane's retained output rather than on the live stream — which is
    // exactly what any viewer joining an existing pane sees.
    let printed = collect_output(split.reader());
    printed
        .lock()
        .unwrap()
        .insert_str(0, &String::from_utf8_lossy(&split.replay));
    let expected = directory.to_string_lossy().into_owned();
    wait_for_shared_output(&printed, &expected);
}

/// Reads a shared pane's stream until `expected` appears in it.
fn wait_for_shared_output(collected: &Arc<Mutex<String>>, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let seen = collected.lock().unwrap().clone();
        if seen.contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never saw {expected:?}; the pane printed {seen:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
