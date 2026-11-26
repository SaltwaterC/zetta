//! Handing a pane's screen between windows: returning it to exclusive use,
//! replaying a full-screen program, and abandoned or stale handovers.

use super::*;

/// A pane shared with one viewer is handed back to it, and reads its own terminal
/// from then on.
///
/// The reverse of the revoke handover, and the reason it exists: relaying to a
/// single viewer is the daemon reading a terminal that viewer could read itself,
/// at about a quarter more cost for sustained output. The ordering is the hard
/// part — everything already relayed must not be replayed, and everything still
/// queued has to arrive *before* the descriptor does.
#[test]
fn a_pane_shared_with_one_viewer_is_handed_back_to_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let revokes = subscription.revokes.clone();
    let grants = subscription.grants.clone();

    let pane = client.spawn(spawn_request(None, "exec cat")).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);

    // Into shared mode the usual way: a second client attaches and the holder
    // answers the revoke.
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
    let mut relayed = holder_pane.reader();

    // Something to see on both sides of the switch, so nothing can be lost or
    // repeated across it without showing up.
    holder_pane
        .send_input(
            b"while-shared
",
        )
        .expect("typing while shared");
    read_until_reader(&mut relayed, "while-shared");

    // The second viewer leaves, which is what makes the pane worth handing back.
    drop(second);
    reap(second_process);
    grant_rx
        .recv_blocking()
        .expect("the last viewer must be offered the terminal");

    let taken = client
        .take_exclusive(pane.session_id, pane.pane_id)
        .expect("taking the pane back");
    assert_eq!(taken.child_pid, pane.child_pid, "the same process, still");
    assert!(
        taken.replay.is_empty(),
        "a granted pane must carry no replay: the viewer already has that output"
    );
    // The relay is finished with, and its end is closed by the multiplexer — which
    // is how a client knows it has everything before it starts reading the pty.
    //
    // And it has to end *cleanly* — a plain end of stream, not an error. A
    // retirement reported as an error made the terminal's byte-stream worker print
    // one into the grid, which shifted a full-screen program's display by the lines
    // it took and left the message there for good.
    let mut buffer = [0; 4096];
    let deadline = Instant::now() + Duration::from_secs(10);
    let ended = loop {
        if Instant::now() >= deadline {
            break Err("the relay never ended".to_owned());
        }
        match relayed.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => break Err(format!("the relay ended as a failure: {error}")),
        }
    };
    assert_eq!(
        ended,
        Ok(()),
        "the relay has to end, and end cleanly: {ended:?}"
    );

    // And the descriptor is live: this is the same terminal, still running the
    // same program, now read directly.
    let descriptor = std::fs::File::from(taken.descriptor);
    assert_terminal_echoes(&descriptor, "after-the-handover");

    // Exclusive again, so a fresh attach revokes rather than joining a relay.
    drop(holder_pane);
    drop(descriptor);
}

/// The screen a window hands over is what every *other* window joins on, and is
/// not sent back to the window that is still showing it.
///
/// Both halves were one bug: the first shared attach consumed the retained
/// buffer. That attach is the sharing window's own, so it redrew the screen it
/// already had — over wherever the program had left the cursor — and every
/// window that joined afterwards got only what the program had redrawn since,
/// which for a full-screen program is whatever changed and nothing else.
#[test]
fn a_handed_over_screen_reaches_the_windows_that_do_not_have_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    // Quiet after the handover, so what a client is sent is the screen and not
    // a race with the pane's own output.
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
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
            revoke_rx.recv_blocking().expect("the revoke");
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
    let holder_pane = holder.join().expect("the holder's handover failed");

    assert!(
        !String::from_utf8_lossy(&holder_pane.replay).contains("ready-screen"),
        "the window that handed the screen over must not be sent it back: {:?}",
        String::from_utf8_lossy(&holder_pane.replay)
    );
    assert!(
        String::from_utf8_lossy(&second.replay).contains("ready-screen"),
        "a joining window must be sent the screen: {:?}",
        String::from_utf8_lossy(&second.replay)
    );

    // And the buffer survives that join too: the third window is as entitled to
    // the screen as the second.
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
        String::from_utf8_lossy(&third.replay).contains("ready-screen"),
        "every later window joins on the same screen: {:?}",
        String::from_utf8_lossy(&third.replay)
    );

    drop(holder_pane);
    drop(second);
    drop(third);
    reap(second_process);
    reap(third_process);
}

/// When the last shared client lets go, the pane is nobody's again: a fresh
/// attach takes it exclusively, exactly as though it had never been shared.
#[test]
fn a_pane_returns_to_exclusive_after_its_last_shared_client_leaves() {
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
    let (holder_pane, revoke_rx) = holder.join().expect("the holder's handover failed");

    // The holder leaves; the second client keeps the pane shared.
    drop(holder_pane);
    std::thread::sleep(Duration::from_millis(300));
    let mut second_reader = second.reader();
    read_until_reader(&mut second_reader, "tick");

    // The last shared client leaves; the pane is nobody's again.
    drop(second);
    drop(second_reader);
    std::thread::sleep(Duration::from_millis(300));

    // A fresh attach is exclusive: it takes the descriptor, no revoke, no
    // shared set. With the secret, because sharing the session set one.
    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane: taken, .. } => {
            let descriptor = std::fs::File::from(taken.descriptor);
            read_until(&descriptor, "tick");
        }
        _other => panic!("a pane nobody holds must attach exclusively"),
    }
    assert!(
        recv_timeout(&revoke_rx, Duration::from_millis(300)).is_none(),
        "an exclusive attach must not revoke a shared pane's clients — there are none"
    );
    reap(second_process);
}

/// A session a window is showing is not something to pick up.
///
/// `keep` is sticky, so reattaching leaves it set — a window that then crashes
/// must still find the session. Listing a session on that basis put the tab the
/// user was looking at into their own reconnect picker, with the reconnect button
/// beside it permanently lit.
#[test]
fn a_reattached_session_stops_being_listed() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
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
    assert_eq!(
        client.list().unwrap().len(),
        1,
        "a detached session is waiting to be picked up"
    );

    let AttachOutcome::Attached { pane: taken, .. } = client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    else {
        panic!("its own window must attach it");
    };
    assert!(
        client.list().unwrap().is_empty(),
        "a session on screen is not offered back to the window showing it"
    );
    // Still held, and still kept: detaching it again lists it again rather than
    // needing the request to be made twice.
    drop(std::fs::File::from(taken.descriptor));
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    assert_eq!(client.list().unwrap().len(), 1);

    // Sharing is the one reason to list a session a window is showing: another
    // window may then join it, which is what "in use" means.
    match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => {
            std::mem::forget(std::fs::File::from(pane.descriptor));
        }
        _ => panic!("its own window must attach it"),
    }
    share_session(&client, pane.session_id, pane.pane_id);
    let listed = client.list().unwrap();
    assert_eq!(
        listed.len(),
        1,
        "a shared session is listed while on screen"
    );
    assert!(
        listed[0].held,
        "and says a window is showing it: {listed:?}"
    );
}

/// A full-screen program comes back as its screen, however long it has run.
///
/// The multiplexer used to keep bytes: a bounded buffer of what a pane printed
/// most recently. For a program that repaints parts of a screen that is a pile of
/// fragments describing a screen the buffer has already dropped to make room for
/// them, so a reattached `htop` came back as pieces of itself over a blank
/// terminal. What is kept now is a grid — the screen those fragments were
/// painting on — so the parts nothing has repainted are still there.
#[test]
fn a_full_screen_program_comes_back_as_its_screen() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    // A program that draws a screen and then repaints one field of it, tens of
    // thousands of times: far more bytes than any buffer of recent output holds,
    // and every one of them addressed to a line rather than appended.
    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; sleep 1; \
             printf '\\033[?1049h\\033[HHEADER\\r\\nbody\\r\\nFOOTER'; \
             i=0; while [ $i -lt 20000 ]; do printf '\\033[2;1Hbody %d' \"$i\"; i=$((i+1)); done; \
             sleep 120",
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
            Vec::new(),
        )
        .unwrap();

    // Read by the multiplexer while nobody is showing it, which is when what it
    // keeps is all there is.
    std::thread::sleep(Duration::from_secs(6));
    let AttachOutcome::Attached { pane: taken, .. } = client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    else {
        panic!("its own window must attach it");
    };
    let replay = String::from_utf8_lossy(&taken.replay).into_owned();
    assert!(
        replay.contains("body 19999"),
        "the last repaint must be there: {replay:?}"
    );
    // The parts a repainting program does not repaint are exactly what a buffer
    // of recent bytes loses.
    assert!(
        replay.contains("HEADER") && replay.contains("FOOTER"),
        "the screen under the repaints must be there: {replay:?}"
    );
    // And it comes back into the screen it was drawn in.
    assert!(
        replay.starts_with("\x1b[?1049h"),
        "restored into the alternate screen: {:?}",
        &replay[..20.min(replay.len())]
    );
    drop(std::fs::File::from(taken.descriptor));
}

/// A handed-over screen is kept at the width it was drawn at.
///
/// A window spawns a pane before it has laid it out, so the size the multiplexer
/// is told is a stand-in — 80x24 — and on Unix the resize that follows goes
/// straight to the descriptor that window holds. The multiplexer therefore knows
/// nothing about the real geometry until it is asked, and a 98x51 screen seeded
/// into an 80x24 grid comes back wrapped and interleaved: the scrambled `htop` a
/// second window joined to.
#[test]
fn a_handed_over_screen_keeps_the_width_it_was_drawn_at() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // The window lays the pane out and resizes the terminal itself, which is what
    // a client on Unix does: the multiplexer is not told.
    let resized = libc::winsize {
        ws_row: 51,
        ws_col: 98,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        // SAFETY: `descriptor` is the pane's pty, open for the call, and
        // `TIOCSWINSZ` reads a `winsize` from the pointer, which `resized` is.
        unsafe {
            libc::ioctl(
                std::os::fd::AsRawFd::as_raw_fd(&descriptor),
                libc::TIOCSWINSZ,
                &raw const resized,
            )
        },
        0,
        "resizing the terminal the way a window does"
    );

    // A screen as wide as the window: one line of ninety columns, and a marker on
    // a row an 80x24 grid could not hold.
    let wide = "W".repeat(90);
    let mut screen = format!("\x1b[?1049h\x1b[H{wide}");
    for row in 2..=40 {
        screen.push_str(&format!("\x1b[{row};1Hrow {row}"));
    }
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, screen.into_bytes())],
        )
        .unwrap();

    let AttachOutcome::Attached { pane: taken, .. } = client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    else {
        panic!("its own window must attach it");
    };
    let replay = String::from_utf8_lossy(&taken.replay).into_owned();
    assert!(
        replay.contains(&wide),
        "a ninety-column line must come back unwrapped: {replay:?}"
    );
    assert!(
        replay.contains("row 40"),
        "and a row past the twenty-fourth must come back at all: {replay:?}"
    );
    drop(std::fs::File::from(taken.descriptor));
}

/// The wrapper that loads Zetta's shell integration is delivered as if typed,
/// so the pane's terminal echoes it — to every viewer of a shared pane, not
/// just whoever wrote it. Run by the daemon before the pane can be attached to,
/// it reaches nobody: what a viewer replays afterwards starts at the prompt.
#[test]
fn a_daemon_loaded_shell_integration_leaves_nothing_for_viewers_to_replay() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
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

    let mut draft = zmux::messages::SharedPaneDraft {
        draft_id: 1,
        profile: "System".to_owned(),
        // bash, not sh: only a shell Zetta has an integration for gets a
        // wrapper at all, so anything else would make this test pass by
        // never running the thing it is about.
        command: Some(zetta_profiles::ProfileCommand::with_args(
            "/bin/bash",
            vec!["-i".to_owned()],
        )),
        env: HashMap::new(),
        working_directory: None,
        inherit_working_directory_from: None,
        load_shell_integration: true,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 0,
            cell_height: 0,
        },
        console_palette: ConsolePalette::default(),
        metadata: pane_summary(0),
    };
    draft.metadata.profile = "System".to_owned();

    let zmux::client::SharedBatchResult::Applied(spawned) = client
        .spawn_shared_batch(SharedSpawnBatchRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            target_pane_id: Some(pane.pane_id),
            replacement: zmux::messages::SharedDraftLayout::Split {
                axis: zmux::protocol::SplitAxis::Vertical,
                first_ratio: 200,
                first: Box::new(zmux::messages::SharedDraftLayout::Existing {
                    pane_id: pane.pane_id,
                }),
                second: Box::new(zmux::messages::SharedDraftLayout::Draft { draft_id: 1 }),
            },
            panes: vec![draft],
            active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 1 }),
        })
        .unwrap()
    else {
        panic!("the bootstrapped spawn conflicted")
    };

    // Long enough for the shell to have echoed the wrapper and reached its
    // prompt. Without this the replay is empty because nothing has happened
    // yet, and the assertions below would hold however the bootstrap behaved.
    std::thread::sleep(Duration::from_millis(750));

    let AttachOutcome::SharedAttached { pane: shared, .. } = client
        .attach_shared_with_secret(
            pane.session_id,
            spawned.mappings[0].pane_id,
            Some(&SessionSecret::new(TEST_SECRET.to_owned())),
        )
        .unwrap()
    else {
        panic!("the bootstrapped pane did not attach as a stream")
    };

    let replay = String::from_utf8_lossy(&shared.replay).into_owned();
    assert!(
        !replay.contains("__zed_init_command_ready_"),
        "a viewer must not replay the bootstrap that ran before it attached: {replay:?}"
    );
    assert!(
        !replay.contains("zetta init"),
        "nor the payload it carried: {replay:?}"
    );
}

/// A handover that never completes must not leave a pane nobody reads.
///
/// The drain thread deliberately skips a pane that is `Exclusive`, `Revoking`
/// or `Granting`: the client holding the descriptor is the one reading it. So
/// a take that fails *after* the pane has been marked exclusive — the client
/// went away as the descriptor was being sent — leaves a pane the daemon will
/// not read and nobody else can. Its pty then fills, the shell blocks on its
/// own output, and it stops reading input: one keystroke lands, and every
/// viewer of that one pane is stuck for good.
#[test]
fn a_handover_that_is_abandoned_leaves_the_pane_readable() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; cat"))
        .unwrap();
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
        .unwrap();

    let request = spawn_request(Some(pane.session_id), "printf shared-ready; cat");
    let spawned = client
        .spawn_shared(SharedSpawnRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            program: request.program,
            args: request.args,
            env: request.env,
            working_directory: request.working_directory,
            size: request.size,
            console_palette: request.console_palette,
            console_backend: Default::default(),
        })
        .unwrap();
    let shared_pane_id = spawned.pane.pane_id();
    // The viewer that spawned it steps aside for one with a client ID of its
    // own, standing in for the window that is about to die. The process stays
    // this one, and deliberately: a pane whose holder process is gone is
    // released by the reaper, so a handover abandoned by a *dead* process
    // recovers without any of what is under test here. What wedges a pane is a
    // holder that is alive and does not have it.
    drop(spawned);

    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    // Asks for the descriptor, then refuses the response while keeping the
    // write half alive. Dropping a just-written socket races the daemon's
    // initial read: on some Unix kernels it can see EOF before the request.
    // Shutting down only reads leaves the complete request available to the
    // daemon but makes its descriptor reply fail, which is the abandoned
    // handover this test needs to exercise.
    let abandon_handover = || {
        let stream = Stream::connect(&endpoint.socket_path).unwrap();
        let mut abandoned = Connection::new(stream);
        abandoned
            .send(&Envelope {
                version: zmux::messages::PROTOCOL_VERSION,
                token: endpoint.token.clone(),
                client_process_id: std::process::id(),
                client_id: ClientId::new("abandoned-handover"),
                stream_only: false,
                session_secret: Some(SessionSecret::new(TEST_SECRET.to_owned())),
                request: Request::TakeExclusive {
                    session_id: pane.session_id,
                    pane_id: shared_pane_id,
                },
            })
            .unwrap();
        abandoned.stream().shutdown(Shutdown::Read).unwrap();
        abandoned
    };

    // Retried because a pane is only handed to its *sole* viewer, and the
    // daemon may not have noticed the spawning viewer's departure yet. The
    // daemon ending this viewer's relay is what says the take was accepted:
    // it is the step the handover takes before the descriptor goes out.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(AttachOutcome::SharedAttached { pane: viewer, .. }) = client.attach_shared_as(
            pane.session_id,
            shared_pane_id,
            std::process::id(),
            Some(TEST_SECRET.to_owned()),
        ) {
            let mut reader = viewer.reader();
            let abandoned = abandon_handover();
            // The daemon gives a handover up to five seconds to flush frames
            // already queued for this relay. Let that documented transition
            // finish before treating the take as refused.
            let taken = relay_ended(&mut reader, Duration::from_secs(6));
            drop(viewer);
            drop(abandoned);
            if taken {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the daemon never accepted the take, so the abandoned handover was never reached"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // A pane the daemon is still reading can be attached and produces output.
    // A wedged one accepts the attach and then says nothing ever again.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(AttachOutcome::SharedAttached { pane: viewer, .. }) = client
            .attach_shared_with_secret(
                pane.session_id,
                shared_pane_id,
                Some(&SessionSecret::new(TEST_SECRET.to_owned())),
            )
        {
            let (seen, ended) = collect_output_until_end(viewer.reader());
            viewer.send_input(b"echo pane-still-lives\r").unwrap();
            let answered = Instant::now() + Duration::from_secs(10);
            loop {
                if seen.lock().unwrap().contains("pane-still-lives") {
                    return;
                }
                assert!(
                    Instant::now() < answered,
                    "the pane stopped reading its input after an abandoned handover; \
                     the viewer's stream ended: {}; \
                     the daemon reports {:?}, and this viewer read {:?}",
                    ended.load(Ordering::SeqCst),
                    client.list().map_or_else(
                        |error| vec![format!("list failed: {error:#}")],
                        |sessions| sessions
                            .into_iter()
                            .map(|session| format!(
                                "session {} held={} panes={:?}",
                                session.id,
                                session.held,
                                session
                                    .panes
                                    .iter()
                                    .map(|pane| (pane.id, pane.state))
                                    .collect::<Vec<_>>()
                            ))
                            .collect::<Vec<_>>()
                    ),
                    seen.lock().unwrap().clone()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        assert!(
            Instant::now() < deadline,
            "the pane never became attachable again after an abandoned handover; \
             the daemon reports {:?}",
            client.list().map_or_else(
                |error| vec![format!("list failed: {error:#}")],
                |sessions| sessions
                    .into_iter()
                    .map(|session| format!(
                        "session {} held={} panes={}",
                        session.id,
                        session.held,
                        session.panes.len()
                    ))
                    .collect::<Vec<_>>()
            )
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A pane whose taker could not show it has to be recoverable.
///
/// The daemon stops reading a pane the moment it hands the descriptor over,
/// because from then on the client that took it is the reader. A client whose
/// terminal cannot adopt the descriptor — or whose window closed while the
/// answer was in flight — therefore holds a pane nobody drains, and nothing in
/// the daemon can notice: the liveness sweep only recovers a pane whose holder
/// has *died*, and this one is still running. Giving it back is the only way
/// out, and it has to work from the same client that took it.
#[test]
fn a_pane_the_client_could_not_show_can_be_given_back() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; cat"))
        .unwrap();
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
        .unwrap();

    let request = spawn_request(Some(pane.session_id), "printf shared-ready; cat");
    let spawned = client
        .spawn_shared(SharedSpawnRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            program: request.program,
            args: request.args,
            env: request.env,
            working_directory: request.working_directory,
            size: request.size,
            console_palette: request.console_palette,
            console_backend: Default::default(),
        })
        .unwrap();
    let shared_pane_id = spawned.pane.pane_id();

    // The sole viewer takes the pane back, the way a window does when the
    // other viewers have gone — and then drops the descriptor on the floor,
    // the way one does whose terminal would not adopt it.
    let taken = client
        .take_exclusive(pane.session_id, shared_pane_id)
        .unwrap();
    let attachment_client_id = taken.attachment_client_id().clone();
    drop(spawned);
    drop(taken);
    client
        .release_exclusive(pane.session_id, shared_pane_id, &attachment_client_id)
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(AttachOutcome::SharedAttached { pane: viewer, .. }) = client
            .attach_shared_with_secret(
                pane.session_id,
                shared_pane_id,
                Some(&SessionSecret::new(TEST_SECRET.to_owned())),
            )
        {
            let seen = collect_output(viewer.reader());
            viewer.send_input(b"echo pane-still-lives\r").unwrap();
            let answered = Instant::now() + Duration::from_secs(10);
            loop {
                // The shared reader pauses output after a size frame until its
                // owner applies that size. This test has no terminal UI, so it
                // must acknowledge the frame itself before waiting for output.
                for size in viewer.take_revisioned_sizes() {
                    viewer.finish_size_application(size);
                }
                if seen.lock().unwrap().contains("pane-still-lives") {
                    return;
                }
                assert!(
                    Instant::now() < answered,
                    "the pane read no input after it was given back; this viewer read {:?}",
                    seen.lock().unwrap().clone()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        assert!(
            Instant::now() < deadline,
            "a pane that was given back must be attachable again"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A delayed failure report belongs to one descriptor handover, not to every
/// pane attachment ever made by the same long-lived Zetta process.
///
/// Clearing the newer attachment makes the daemon and the window read the same
/// PTY. Full-screen programs expose that immediately: each reader gets arbitrary
/// pieces of their escape sequences, leaving both screens full of holes.
#[test]
fn stale_handover_cleanup_cannot_release_a_newer_attachment() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let first = client
        .spawn(spawn_request(None, "printf first-ready; sleep 60"))
        .unwrap();
    let session_id = first.session_id;
    let first_pane_id = first.pane_id;
    let descriptor = std::fs::File::from(first.descriptor);
    read_until(&descriptor, "first-ready");
    drop(descriptor);
    client
        .share(
            session_id,
            summary(session_id, first_pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .unwrap();
    let request = spawn_request(Some(session_id), "printf ready; cat");
    let shared = client
        .spawn_shared(SharedSpawnRequest {
            session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            program: request.program,
            args: request.args,
            env: request.env,
            working_directory: request.working_directory,
            size: request.size,
            console_palette: request.console_palette,
            console_backend: Default::default(),
        })
        .unwrap();
    let pane_id = shared.pane.pane_id();
    // Leave no other exclusive pane that could make the session-level `held`
    // assertion pass after this pane was wrongly released.
    client.close_pane(session_id, first_pane_id).unwrap();

    let older = client.take_exclusive(session_id, pane_id).unwrap();
    let older_attachment = older.attachment_client_id().clone();
    drop(shared);
    drop(older);

    let AttachOutcome::Attached { pane: newer, .. } =
        client.attach(session_id, Some(pane_id), None).unwrap()
    else {
        panic!("the replacement attachment was not exclusive")
    };
    assert_ne!(older_attachment, *newer.attachment_client_id());
    let mut descriptor = std::fs::File::from(newer.descriptor);

    let error = client
        .release_exclusive(session_id, pane_id, &older_attachment)
        .expect_err("an old attachment must not release a newer one");
    assert!(
        error.to_string().contains("newer attachment"),
        "unexpected stale-release error: {error:#}"
    );
    let listed = client
        .list()
        .unwrap()
        .into_iter()
        .find(|session| session.id == session_id)
        .expect("the attached session remains listed");
    assert!(listed.held, "the newer attachment was cleared");

    // Give a wrongly awakened daemon drain loop time to enter its poll before
    // producing output. With the bug it consumes the marker; with the newer
    // attachment intact, this descriptor remains the pane's only reader.
    std::thread::sleep(Duration::from_millis(100));
    descriptor
        .write_all(b"new-attachment-still-owns-output\n")
        .unwrap();
    read_until(&descriptor, "new-attachment-still-owns-output");
}

/// Reads a child's pipe on its own thread: a blocking read cannot be given a
/// deadline, and a test that hangs on one says nothing about what failed.
/// Waits for the daemon to close a shared viewer's relay, which is what a
/// granted handover does to the viewer it is taking the pane from — and the
/// only thing a client can observe about a take it is not the one making.
///
/// Returns false if the relay was still open when the wait ran out, meaning
/// the take was refused.
fn relay_ended(reader: &mut impl Read, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    let mut bytes = [0_u8; 4096];
    while Instant::now() < deadline {
        match reader.read(&mut bytes) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return true,
        }
    }
    false
}

/// [`collect_output`], plus the flag that says the stream ended rather than
/// went quiet — the difference between a viewer that was dropped and a pane
/// that stopped answering.
fn collect_output_until_end(
    mut output: impl Read + Send + 'static,
) -> (Arc<Mutex<String>>, Arc<AtomicBool>) {
    let collected = Arc::new(Mutex::new(String::new()));
    let ended = Arc::new(AtomicBool::new(false));
    let thread_collected = Arc::clone(&collected);
    let thread_ended = Arc::clone(&ended);
    std::thread::spawn(move || {
        let mut bytes = [0_u8; 4096];
        loop {
            match output.read(&mut bytes) {
                Ok(0) => break,
                Ok(read) => thread_collected
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&bytes[..read])),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        thread_ended.store(true, Ordering::SeqCst);
    });
    (collected, ended)
}
