//! Panes shared between windows: handing an exclusive pane over, sizing to the
//! smallest viewer, and when a session is offered, shared or unshared.

use super::*;

/// A second client attaching to a held pane takes it over: the daemon asks
/// the holder to hand its screen back, and from then on every client reads the
/// pane together — shared mode.
#[test]
fn attaching_to_a_held_pane_handsover_and_makes_it_shared() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; while true; do sleep 0.2; printf 'tick\\n'; done",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // The holder answers the revoke: it stops reading, hands its screen back
    // as a snapshot, and re-attaches, which joins the pane's shared set.
    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask the holder to hand the pane over");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .expect("handing the screen back");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => Some(pane),
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second_pid = second_process.id();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_pid,
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder
        .join()
        .expect("the holder.s handover failed")
        .expect("the holder never re-attached");

    // Both clients are on the same data plane: each sees what the pane
    // produces, including what the other one typed.
    let mut holder_reader = holder_pane.reader();
    let mut second_reader = second.reader();
    read_until_reader(&mut holder_reader, "tick");
    read_until_reader(&mut second_reader, "tick");

    holder_pane
        .send_input(b"echo from-holder\n")
        .expect("sending input as the holder");
    read_until_reader(&mut second_reader, "from-holder");
    read_until_reader(&mut holder_reader, "from-holder");

    // A pane turning shared is logged with the process that asked for it:
    // nothing else records who that was, and a pane nobody meant to share was
    // found relaying with no way to tell why.
    let log = std::fs::read_to_string(daemon.sessions_dir().join("daemon.log"))
        .expect("the daemon keeps a log in its session directory");
    let handover = log
        .lines()
        .find(|line| line.contains("so the pane becomes shared"))
        .unwrap_or_else(|| panic!("the handover was not logged: {log}"));
    assert!(
        handover.contains(&format!("process {second_pid}")),
        "the log must name the requesting process: {handover}"
    );
    if cfg!(target_os = "linux") {
        assert!(
            handover.contains("/bin/sleep 120"),
            "the log must carry the requester's command line: {handover}"
        );
    }
    reap(second_process);
}

/// A third client attaches to a shared pane without disturbing anyone: there
/// is nothing to take over, so no revoke is sent.
#[test]
fn a_shared_pane_welcomes_further_clients_without_a_revoke() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; while true; do sleep 0.2; printf 'tick\\n'; done",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            let pane = match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            };
            (pane, revoke_rx)
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second_pid = second_process.id();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_pid,
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };

    // The holder has rejoined; the third client joins without any revoke
    // being sent — nobody has anything left to hand over.
    let (holder_pane, revoke_rx) = holder.join().expect("the holder's handover failed");
    let third_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let third = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            third_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a shared pane must welcome a third client"),
    };
    assert!(
        recv_timeout(&revoke_rx, Duration::from_millis(300)).is_none(),
        "joining a shared pane must not revoke anyone"
    );

    // All three read the same pane.
    let mut holder_reader = holder_pane.reader();
    let mut second_reader = second.reader();
    let mut third_reader = third.reader();
    read_until_reader(&mut second_reader, "tick");
    read_until_reader(&mut third_reader, "tick");
    read_until_reader(&mut holder_reader, "tick");

    // A client leaving the relay drops only itself; the others keep reading.
    drop(third);
    drop(third_reader);
    second
        .send_input(b"echo still-sharing\n")
        .expect("sending input through the remaining client");
    read_until_reader(&mut holder_reader, "still-sharing");
    read_until_reader(&mut second_reader, "still-sharing");
    reap(second_process);
    reap(third_process);
}

/// Shared clients are sized to the smallest of them, and a size report that
/// stops being the smallest lets the pane grow again.
#[test]
fn shared_clients_are_sized_to_the_smallest_of_them() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    // `stty size` prints the terminal's size, which is the pane's truth.
    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; stty size; while read -r line; do printf 'input:%s\\n' \"$line\"; done",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    // One read can contain both markers; wait for the size in the same
    // accumulated read so the first assertion cannot consume it.
    read_until(&descriptor, "24 80");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder.s handover failed");
    let mut holder_reader = holder_pane.reader();
    let mut second_reader = second.reader();

    // Advance the canonical session before either viewer reports its actual
    // layout. A delayed six-row report from the old revision must not remain
    // in arbitration and constrain the new geometry.
    let revision = match client
        .apply_shared(
            pane.session_id,
            SessionRevision::INITIAL,
            SharedSessionOperation::SetTabState {
                state: serde_json::json!({"sizing-revision": 1}),
            },
        )
        .unwrap()
    {
        zmux::client::SharedOperationResult::Applied(state) => state.revision,
        zmux::client::SharedOperationResult::Conflict(_) => {
            panic!("tab-state operation conflicted")
        }
    };
    second
        .send_resize(40, 6)
        .expect("sending a deliberately stale size report");
    holder_pane
        .send_resize_for_revision(revision, 80, 24)
        .expect("reporting the holder's initialized size");

    // A remote client reaches the same shared data path with a stream-only
    // envelope. It has no descriptor or exclusive handover, but its input must
    // still be attributed and delivered over the full-duplex shared socket.
    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    let stream = Stream::connect(&endpoint.socket_path).unwrap();
    let mut remote = Connection::new(stream);
    remote
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    remote
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id: std::process::id(),
            client_id: ClientId::new("remote-style-test"),
            stream_only: true,
            session_secret: None,
            request: Request::Attach(zmux::messages::AttachRequest {
                session_id: pane.session_id,
                pane_id: Some(pane.pane_id),
                secret: Some(SessionSecret::new(TEST_SECRET.to_owned())),
                force_shared: false,
                relaying_for: None,
            }),
        })
        .unwrap();
    let (response, _) = remote.receive::<Response>().unwrap();
    let replay_length = match response {
        Response::SharedAttached { replay_length, .. } => replay_length,
        other => panic!("the stream-only client must attach as shared: {other:?}"),
    };
    remote.read_exact(replay_length).unwrap();
    let remote_input = b"remote-style\n";
    remote
        .send(&Request::Input {
            length: remote_input.len(),
        })
        .unwrap();
    remote.write_all(remote_input).unwrap();
    let mut remote_output = Vec::new();
    while !remote_output
        .windows(b"input:remote-style".len())
        .any(|bytes| bytes == b"input:remote-style")
    {
        let (event, _) = remote.receive::<Event>().unwrap();
        if let Event::Output { length, .. } = event {
            remote_output.extend(remote.read_exact(length).unwrap());
        }
    }

    // The smaller client's size wins, and is announced to everyone.
    second
        .send_resize_for_revision(revision, 40, 10)
        .expect("reporting a smaller size");
    assert_eq!(
        wait_for_shared_size(&mut holder_reader, &holder_pane, (40, 10)).last(),
        Some(&(40, 10))
    );
    assert_eq!(
        wait_for_shared_size(&mut second_reader, &second, (40, 10)).last(),
        Some(&(40, 10))
    );

    // Once the holder has observed that correction, a larger report from the
    // second client does not change the effective size. It still gets the
    // existing size broadcast back, rather than being left divergent.
    holder_pane
        .send_resize_for_revision(revision, 40, 10)
        .expect("recording the holder's corrected size");
    second
        .send_resize_for_revision(revision, 120, 40)
        .expect("reporting a larger size that needs correction");
    assert_eq!(
        wait_for_shared_size(&mut holder_reader, &holder_pane, (40, 10)).last(),
        Some(&(40, 10))
    );
    assert_eq!(
        wait_for_shared_size(&mut second_reader, &second, (40, 10)).last(),
        Some(&(40, 10))
    );

    // The largest client no longer sets the smallest size; after the holder
    // reports its real size again, the pane grows back to it.
    holder_pane
        .send_resize_for_revision(revision, 80, 24)
        .expect("reporting the holder's larger size");
    assert_eq!(
        wait_for_shared_size(&mut holder_reader, &holder_pane, (80, 24)).last(),
        Some(&(80, 24))
    );
    assert_eq!(
        wait_for_shared_size(&mut second_reader, &second, (80, 24)).last(),
        Some(&(80, 24))
    );

    // Input must remain usable after the shared grid has been shrunk and grown
    // several times; the size reports and their broadcasts share the same
    // full-duplex connection as the input.
    second
        .send_input(b"after-resizes\n")
        .expect("sending input after multiple size transitions");
    read_until_reader(&mut holder_reader, "input:after-resizes");
    read_until_reader(&mut second_reader, "input:after-resizes");
    reap(second_process);
}

/// Input a shared client sends reaches the pane, and the pane's exit reports
/// that it was typed into — which is the multiplexer's own attribution.
/// Sends one request as a named client on a fresh connection, the way a remote
/// window does, so the daemon attributes it to that client and nobody else.
fn connect_as(daemon: &TestDaemon, client_id: &str, request: Request) -> Connection {
    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    let mut connection = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
    connection
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    connection
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id: std::process::id(),
            client_id: ClientId::new(client_id),
            stream_only: true,
            session_secret: None,
            request,
        })
        .unwrap();
    connection
}

/// A window that closes a shared tab says so before its data streams drop, and
/// the leave is then what takes it out of the set. The pane must grow back to
/// the viewers that remain at that point, not wait for a stream drop that will
/// find nothing left to remove.
#[test]
fn leaving_a_shared_session_lets_the_pane_grow_back_to_the_remaining_viewer() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            match client
                .attach_as(
                    session_id,
                    pane_id,
                    std::process::id(),
                    Some(TEST_SECRET.to_owned()),
                )
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let mut small = connect_as(
        &daemon,
        "small-viewer",
        Request::Attach(zmux::messages::AttachRequest {
            session_id: pane.session_id,
            pane_id: Some(pane.pane_id),
            secret: Some(SessionSecret::new(TEST_SECRET.to_owned())),
            force_shared: false,
            relaying_for: None,
        }),
    );
    let (response, _) = small.receive::<Response>().unwrap();
    let Response::SharedAttached { replay_length, .. } = response else {
        panic!("the small viewer must attach as shared: {response:?}");
    };
    small.read_exact(replay_length).unwrap();
    let holder_pane = holder.join().expect("the holder's handover failed");
    let mut holder_reader = holder_pane.reader();

    let revision = client.shared_snapshot(pane.session_id).unwrap().revision;
    holder_pane
        .send_resize_for_revision(revision, 120, 40)
        .expect("reporting the holder's size");
    small
        .send(&Request::Resize {
            session_id: pane.session_id,
            pane_id: pane.pane_id,
            revision: Some(revision),
            columns: 60,
            lines: 20,
        })
        .unwrap();
    wait_for_shared_size(&mut holder_reader, &holder_pane, (60, 20));

    // The small viewer's data stream stays open: the leave alone has to do it.
    let mut leave = connect_as(
        &daemon,
        "small-viewer",
        Request::LeaveShared {
            session_id: pane.session_id,
        },
    );
    let (response, _) = leave.receive::<Response>().unwrap();
    assert!(
        matches!(response, Response::Ok),
        "leaving was refused: {response:?}"
    );
    wait_for_shared_size(&mut holder_reader, &holder_pane, (120, 40));
    drop(small);
}

#[test]
fn shared_input_is_attributed_when_the_pane_exits() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; read line; printf 'got:%s\\n' \"$line\"; exit 0",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder.s handover failed");
    let mut holder_reader = holder_pane.reader();

    let (exit_tx, exit_rx) = async_channel::unbounded();
    reporters.register_shared(pane.pane_id, exit_tx);

    // The second client types; the pane answers, then exits.
    second.send_input(b"hello shared\n").unwrap();
    read_until_reader(&mut holder_reader, "got:hello shared");

    match recv_timeout(&exit_rx, Duration::from_secs(15)) {
        Some(report) => {
            assert_eq!(report.raw_status, Some(0));
            assert!(report.input_sent, "the shared input must be attributed");
            assert!(!report.disconnected);
        }
        None => panic!("the pane's exit was never reported"),
    }
    reap(second_process);
}

/// A pane that exits without any shared client having typed into it is
/// reported as such.
#[test]
fn a_shared_pane_that_received_no_input_reports_so() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    // Do not make the handover race a one-second sleep. Under a loaded or
    // virtualized scheduler the child can finish that sleep before the second
    // attach has made it through the daemon, which tests an already-ended pane
    // rather than shared exit reporting.
    let release_path = daemon.config.join("release-shared-exit");
    let mut request = spawn_request(
        None,
        "printf ready; while [ ! -e \"$ZMUX_TEST_RELEASE\" ]; do sleep 0.05; done; exit 3",
    );
    request.env.insert(
        "ZMUX_TEST_RELEASE".to_owned(),
        release_path.to_string_lossy().into_owned(),
    );
    let pane = client.spawn(request).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let _second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder.s handover failed");

    let (exit_tx, exit_rx) = async_channel::unbounded();
    reporters.register_shared(pane.pane_id, exit_tx);
    std::fs::write(&release_path, []).expect("releasing the pane to exit");

    match recv_timeout(&exit_rx, Duration::from_secs(15)) {
        Some(report) => {
            // The raw wait status: exit code 3 lives in the upper byte.
            assert_eq!(report.raw_status, Some(768));
            assert!(!report.input_sent, "nobody typed into the pane");
            assert!(!report.disconnected);
        }
        None => panic!("the pane's exit was never reported"),
    }
    drop(holder_pane);
    reap(second_process);
}

/// Sharing is a workflow of its own: a session on screen becomes joinable
/// without being detached first.
///
/// The daemon has always *allowed* attaching a live pane — that is what the
/// revoke handover is for — but it would not *offer* one, because availability
/// required `keep`, and only detaching sets that. So the sole route to a shared
/// session was to dismiss the tab and immediately take it back.
#[test]
fn a_live_session_is_only_offered_once_its_window_shares_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    assert!(
        client.list().unwrap().is_empty(),
        "a session nobody shared or detached must not be offered"
    );

    let mut offered = summary(pane.session_id, pane.pane_id);
    offered.title = "shared live".to_owned();
    client
        .share(
            pane.session_id,
            offered,
            serde_json::json!({"tab": 1}),
            None,
            true,
        )
        .unwrap();

    let sessions = client.list().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "sharing must offer the session: {sessions:?}"
    );
    assert_eq!(sessions[0].id, pane.session_id);
    assert_eq!(sessions[0].title, "shared live");
    // Still on screen, so joining it means a revoke handover rather than an
    // ordinary reconnect. This flag is what lets a picker say so.
    assert!(
        sessions[0].held,
        "a session its window is still showing must be listed as held"
    );

    // Withdrawing takes it off offer and touches nothing else: the pane is
    // still this client's, and its process is still running.
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            false,
        )
        .unwrap();
    assert!(
        client.list().unwrap().is_empty(),
        "a withdrawn session must stop being offered"
    );
    assert!(
        process_is_alive(pane.child_pid),
        "withdrawing an offer must not end the session"
    );
    assert_terminal_echoes(&descriptor, "still-here");
}

/// A window can republish a tab state that names a pane the daemon no longer
/// owns. The daemon's PTY registry must repair the catalog and presentation
/// together, so reconnect never tries to attach the advertised missing pane.
#[test]
fn republishing_an_unattachable_pane_set_repairs_shared_reconnect_state() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    // Consume two IDs so the stale presentation and the replacement panes have
    // the same shape as the real handoff that exposed this bug.
    let first = client.spawn(spawn_request(None, "sleep 60")).unwrap();
    assert_eq!(first.pane_id, 1);
    client.kill(first.session_id).unwrap();
    drop(first);
    let second = client.spawn(spawn_request(None, "sleep 60")).unwrap();
    assert_eq!(second.pane_id, 2);
    client.kill(second.session_id).unwrap();
    drop(second);

    let initial = client
        .spawn(spawn_request(None, "sleep 60"))
        .expect("spawning the initial shared pane");
    assert_eq!(initial.pane_id, 3);
    client
        .share(
            initial.session_id,
            summary(initial.session_id, initial.pane_id),
            serde_json::json!({"before": true}),
            None,
            true,
        )
        .unwrap();

    let spawn_shared = |base_revision, operation_id| {
        let request = spawn_request(Some(initial.session_id), "sleep 60");
        SharedSpawnRequest {
            session_id: initial.session_id,
            base_revision,
            operation_id,
            program: request.program,
            args: request.args,
            env: request.env,
            working_directory: request.working_directory,
            size: request.size,
            console_palette: request.console_palette,
            console_backend: Default::default(),
        }
    };
    let fourth = client
        .spawn_shared(spawn_shared(
            SessionRevision::INITIAL,
            client.next_shared_operation_id(),
        ))
        .unwrap();
    assert_eq!(fourth.pane.pane_id(), 4);
    let vertical_layout = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: zmux::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
        first: Box::new(BackgroundPaneLayout::Pane {
            pane_id: initial.pane_id,
        }),
        second: Box::new(BackgroundPaneLayout::Pane {
            pane_id: fourth.pane.pane_id(),
        }),
    };
    let vertical = match client
        .apply_shared(
            initial.session_id,
            fourth.state.revision,
            SharedSessionOperation::SetLayout {
                layout: vertical_layout.clone(),
            },
        )
        .unwrap()
    {
        zmux::client::SharedOperationResult::Applied(state) => state,
        zmux::client::SharedOperationResult::Conflict(_) => {
            panic!("setting the original shared layout conflicted")
        }
    };
    let focused = match client
        .apply_shared(
            initial.session_id,
            vertical.revision,
            SharedSessionOperation::SetFocus {
                pane_id: initial.pane_id,
            },
        )
        .unwrap()
    {
        zmux::client::SharedOperationResult::Applied(state) => state,
        zmux::client::SharedOperationResult::Conflict(_) => {
            panic!("focusing the original shared pane conflicted")
        }
    };
    // Leave the old pane in the canonical presentation and make its visibility
    // references point at it as well. The handoff below also advertises pane 5,
    // which was never created by this daemon. Repairing against only the
    // catalog used to leave that phantom pane in the presentation, producing
    // the reconnect failure this test covers.
    let maximized = match client
        .apply_shared(
            initial.session_id,
            focused.revision,
            SharedSessionOperation::SetMaximized {
                pane_id: Some(initial.pane_id),
            },
        )
        .unwrap()
    {
        zmux::client::SharedOperationResult::Applied(state) => state,
        zmux::client::SharedOperationResult::Conflict(_) => {
            panic!("maximizing the existing pane conflicted")
        }
    };
    let stale = match client
        .apply_shared(
            initial.session_id,
            maximized.revision,
            SharedSessionOperation::SetMinimized {
                pane_id: initial.pane_id,
                minimized: true,
            },
        )
        .unwrap()
    {
        zmux::client::SharedOperationResult::Applied(state) => state,
        zmux::client::SharedOperationResult::Conflict(_) => {
            panic!("minimizing the existing pane conflicted")
        }
    };
    assert_eq!(stale.presentation.maximized_pane, Some(initial.pane_id));
    assert_eq!(stale.presentation.minimized_panes, vec![initial.pane_id]);

    let phantom_pane_id = 5;
    let mut republished = summary(initial.session_id, phantom_pane_id);
    republished.title = "recovered".to_owned();
    republished.layout = BackgroundPaneLayout::Split {
        axis: "horizontal".to_owned(),
        first_ratio: zmux::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
        first: Box::new(BackgroundPaneLayout::Pane {
            pane_id: fourth.pane.pane_id(),
        }),
        second: Box::new(BackgroundPaneLayout::Pane {
            pane_id: phantom_pane_id,
        }),
    };
    republished.panes = vec![
        pane_summary(fourth.pane.pane_id()),
        pane_summary(phantom_pane_id),
    ];
    let observer = daemon.client();
    let subscription = observer.subscribe().unwrap();
    let events = subscription.shared.register(initial.session_id);
    client
        .share(
            initial.session_id,
            republished.clone(),
            serde_json::json!({"after": true}),
            None,
            true,
        )
        .unwrap();
    match recv_timeout(&events, Duration::from_secs(10)).expect("the repair must be broadcast") {
        zmux::client::SharedSessionEvent::Updated(state) => {
            assert_eq!(state.summary.title, republished.title);
            assert_eq!(state.summary.active_pane, initial.pane_id);
            assert_eq!(
                state.summary.layout,
                BackgroundPaneLayout::Split {
                    axis: "vertical".to_owned(),
                    first_ratio: zmux::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
                    first: Box::new(BackgroundPaneLayout::Pane {
                        pane_id: initial.pane_id,
                    }),
                    second: Box::new(BackgroundPaneLayout::Pane {
                        pane_id: fourth.pane.pane_id(),
                    }),
                }
            );
            assert_eq!(
                state
                    .summary
                    .panes
                    .iter()
                    .map(|pane| pane.id)
                    .collect::<Vec<_>>(),
                vec![fourth.pane.pane_id(), initial.pane_id]
            );
            assert_eq!(state.presentation.layout, state.summary.layout);
            assert_eq!(state.presentation.active_pane, initial.pane_id);
            assert_eq!(state.presentation.maximized_pane, Some(initial.pane_id));
            assert_eq!(state.presentation.minimized_panes, vec![initial.pane_id]);
        }
        other => panic!("unexpected repair event: {other:?}"),
    }

    let listed = client.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].active_pane, initial.pane_id);
    assert_eq!(
        listed[0]
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect::<Vec<_>>(),
        vec![fourth.pane.pane_id(), initial.pane_id]
    );

    let repaired = client.shared_snapshot(initial.session_id).unwrap();
    assert!(repaired.revision > stale.revision);
    assert_eq!(repaired.summary.active_pane, listed[0].active_pane);
    assert_eq!(repaired.summary.layout, listed[0].layout);
    assert_eq!(
        repaired
            .summary
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect::<Vec<_>>(),
        listed[0]
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(repaired.presentation.layout, repaired.summary.layout);
    assert_eq!(repaired.presentation.active_pane, initial.pane_id);
    assert_eq!(repaired.presentation.maximized_pane, Some(initial.pane_id));
    assert_eq!(repaired.presentation.minimized_panes, vec![initial.pane_id]);
    assert_eq!(repaired.state, serde_json::json!({"after": true}));

    let repeated = client.shared_snapshot(initial.session_id).unwrap();
    assert_eq!(repeated, repaired, "a second repair must be idempotent");

    for pane_id in [fourth.pane.pane_id(), initial.pane_id] {
        let attached = observer
            .attach(initial.session_id, Some(pane_id), None)
            .expect("every pane advertised by a repaired snapshot must attach");
        assert!(matches!(
            attached,
            AttachOutcome::Attached { .. } | AttachOutcome::SharedAttached { .. }
        ));
        drop(attached);
    }

    let session_id = initial.session_id;
    drop(fourth);
    drop(initial);
    client.kill(session_id).unwrap();
}

/// Shared spawning uses the same canonical operation path for a local viewer
/// and a stream-only viewer. The local stream must retain its grant capability,
/// while both requests must produce an event and an exact revisioned snapshot.
#[test]
fn shared_spawns_publish_events_and_preserve_local_grants() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let grants = subscription.grants.clone();
    let observer = daemon.client();
    let observer_subscription = observer.subscribe().unwrap();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let offered = summary(pane.session_id, pane.pane_id);
    client
        .share(
            pane.session_id,
            offered,
            serde_json::json!({"shared": true}),
            Some(&test_verifier()),
            true,
        )
        .unwrap();
    let events = observer_subscription.shared.register(pane.session_id);

    let local_request = spawn_request(Some(pane.session_id), "printf local; sleep 60");
    let local = client
        .spawn_shared(SharedSpawnRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            program: local_request.program,
            args: local_request.args,
            env: local_request.env,
            working_directory: local_request.working_directory,
            size: local_request.size,
            console_palette: local_request.console_palette,
            console_backend: Default::default(),
        })
        .unwrap();
    let (grant_tx, grant_rx) = async_channel::unbounded();
    grants.register(local.pane.pane_id(), grant_tx);
    assert_eq!(local.state.revision, SessionRevision(1));
    let first_event = recv_timeout(&events, Duration::from_secs(10))
        .expect("the local shared spawn must be broadcast");
    let first_pane_id = match first_event {
        zmux::client::SharedSessionEvent::PaneAdded { pane_id, state, .. } => {
            assert_eq!(state.revision, SessionRevision(1));
            pane_id
        }
        other => panic!("unexpected first shared spawn event: {other:?}"),
    };
    assert_eq!(first_pane_id, local.pane.pane_id());

    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    let stream = Stream::connect(&endpoint.socket_path).unwrap();
    let mut stream_only = Connection::new(stream);
    stream_only
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let stream_request = spawn_request(Some(pane.session_id), "printf remote; sleep 60");
    stream_only
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token.clone(),
            client_process_id: std::process::id(),
            client_id: ClientId::new("stream-only-spawn"),
            stream_only: true,
            session_secret: Some(SessionSecret::new(TEST_SECRET.to_owned())),
            request: Request::SpawnShared(SharedSpawnRequest {
                session_id: pane.session_id,
                base_revision: local.state.revision,
                operation_id: zmux::messages::SharedOperationId::new(
                    ClientId::new("stream-only-spawn"),
                    1,
                ),
                program: stream_request.program,
                args: stream_request.args,
                env: stream_request.env,
                working_directory: stream_request.working_directory,
                size: stream_request.size,
                console_palette: stream_request.console_palette,
                console_backend: Default::default(),
            }),
        })
        .unwrap();
    let (response, _) = stream_only.receive::<Response>().unwrap_or_else(|error| {
        panic!(
            "receiving the stream-only spawn response: {error:#}\ndaemon log:\n{}",
            daemon.log()
        )
    });
    let (stream_pane_id, stream_revision, replay_length) = match response {
        Response::SharedSpawned {
            pane_id,
            replay_length,
            shared_state,
            ..
        } => (pane_id, shared_state.revision, replay_length),
        other => panic!("unexpected stream-only shared spawn response: {other:?}"),
    };
    stream_only.read_exact(replay_length).unwrap();
    assert_eq!(stream_revision, SessionRevision(2));
    let second_event = recv_timeout(&events, Duration::from_secs(10))
        .expect("the stream-only shared spawn must be broadcast");
    match second_event {
        zmux::client::SharedSessionEvent::PaneAdded { pane_id, state, .. } => {
            assert_eq!(pane_id, stream_pane_id);
            assert_eq!(state.revision, SessionRevision(2));
        }
        other => panic!("unexpected second shared spawn event: {other:?}"),
    }
    let canonical = client.shared_snapshot(pane.session_id).unwrap();
    assert_eq!(canonical.revision, SessionRevision(2));
    assert_eq!(canonical.summary.panes.len(), 3);

    // Put a stream-only viewer on the locally spawned pane as well. Its
    // departure leaves the local viewer alone, which is the handover that
    // proves the local SpawnShared request kept stream_only=false.
    let attach_stream = Stream::connect(&endpoint.socket_path).unwrap();
    let mut stream_attach = Connection::new(attach_stream);
    stream_attach
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream_attach
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id: std::process::id(),
            client_id: ClientId::new("stream-only-attach"),
            stream_only: true,
            session_secret: Some(SessionSecret::new(TEST_SECRET.to_owned())),
            request: Request::Attach(zmux::messages::AttachRequest {
                session_id: pane.session_id,
                pane_id: Some(local.pane.pane_id()),
                secret: Some(SessionSecret::new(TEST_SECRET.to_owned())),
                force_shared: false,
                relaying_for: None,
            }),
        })
        .unwrap();
    let (response, _) = stream_attach.receive::<Response>().unwrap();
    let replay_length = match response {
        Response::SharedAttached {
            pane_id,
            replay_length,
            ..
        } => {
            assert_eq!(pane_id, local.pane.pane_id());
            replay_length
        }
        other => panic!("unexpected stream-only attach response: {other:?}"),
    };
    stream_attach.read_exact(replay_length).unwrap();

    // With the stream-only viewer gone, the local shared client must be offered
    // the descriptor. A hard-coded stream-only spawn flag would suppress this.
    drop(stream_only);
    drop(stream_attach);
    let grant = recv_timeout(&grant_rx, Duration::from_secs(10));
    assert!(
        grant.is_some(),
        "the local shared client must receive a grant"
    );
    let attached = client
        .take_exclusive(pane.session_id, local.pane.pane_id())
        .unwrap();
    drop(std::fs::File::from(attached.descriptor));
    client.kill(pane.session_id).unwrap();
    drop(local);
    drop(descriptor);
}

/// A session that is both kept and shared stays shared when its window hands
/// the terminal back. This is the daemon-side contract for Zetta's
/// `Ctrl-Shift-B`: closing that window must leave a session another process can
/// reconnect to, rather than narrowing it back to the old process.
#[test]
fn a_shared_session_stays_shared_when_it_is_detached() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            true,
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

    let listed = client.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].scoped_to, None);

    let stranger = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    match client
        .attach_as(pane.session_id, pane.pane_id, stranger.id(), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("a shared kept session must attach from another process"),
    }
    reap(stranger);
}

/// Unsharing scopes a session back to one window, so it is only accepted while one
/// window has it.
///
/// There is no way to take a pane away from a viewer that is still relaying it — a
/// grant goes to the last viewer, not to a chosen one — so withdrawing the offer
/// while others are attached would leave the session listed as private while other
/// windows carried on driving it. Refusing says which, and leaves the tab shared.
#[test]
fn a_session_is_only_unshared_while_one_window_has_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let revokes = subscription.revokes.clone();
    let grants = subscription.grants.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let offer = |offered: bool| {
        client.share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"panes": 1}),
            None,
            offered,
        )
    };
    offer(true).expect("sharing a live session");

    // One window has it, so it can be scoped straight back.
    offer(false).expect("unsharing while this window alone has it");
    assert!(
        client.list().unwrap().is_empty(),
        "an unshared session must stop being offered"
    );
    offer(true).expect("sharing it again");

    // A second window joins, and now it cannot.
    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let (grant_tx, grant_rx) = async_channel::unbounded();
    grants.register(pane.pane_id, grant_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx.recv_blocking().expect("the revoke must arrive");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, Vec::new(), 80, 24)
                .expect("handing the screen back");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });
    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder's handover failed");

    let refused = offer(false).expect_err("two windows cannot be scoped back to one");
    assert!(
        refused.to_string().contains("still open in 2 windows"),
        "the refusal has to say why: {refused:#}"
    );
    assert_eq!(
        client.list().unwrap().len(),
        1,
        "a refused unshare must leave the session shared"
    );

    // Once the other window goes, it is accepted — and the pane is offered back,
    // so the session stops being relayed as well as stopping being listed.
    drop(second);
    reap(second_process);
    grant_rx
        .recv_blocking()
        .expect("the last viewer is offered the terminal");
    offer(false).expect("unsharing once this window alone has it again");
    drop(holder_pane);
}

/// Sharing says who may see the session now. It does not ask for the session to
/// outlive the window showing it — that is what detaching is for, and conflating
/// the two would turn every shared tab into a session left behind on a crash.
#[test]
fn sharing_a_session_does_not_ask_for_it_to_outlive_its_window() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .unwrap();

    // Closing the shared tab. Nobody asked for this session to be kept, so it
    // ends with the pane rather than being left behind holding a stray shell.
    drop(descriptor);
    client.close_pane(pane.session_id, pane.pane_id).unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "a shared session nobody detached kept running after its tab was closed"
    );
    assert!(
        client.list().unwrap().is_empty(),
        "a shared session nobody detached was left behind as a background session"
    );
}

/// Only a client that is showing a session may offer it.
///
/// Not merely tidiness: the request republishes the session's verifier, so
/// accepting it from anybody would be a way to reprotect — or unprotect — a
/// session without presenting the secret it already has.
#[test]
fn only_a_client_showing_a_session_may_share_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let other_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let mut request = spawn_request(None, "printf ready; sleep 60");
    request.client_process_id = other_process.id();
    let pane = client.spawn(request).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    let error = client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .expect_err("a client that is not showing the session must be refused");
    assert!(
        error.to_string().contains("not showing it"),
        "the refusal must say why: {error:#}"
    );
    assert!(
        client.list().unwrap().is_empty(),
        "a refused share must not offer the session"
    );
    reap(other_process);
}
