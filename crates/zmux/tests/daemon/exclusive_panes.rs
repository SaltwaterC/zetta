//! Explicit pane closure must remove the PTY rather than leave it available
//! for a later background-session restore.

use super::*;

#[test]
fn closed_panes_do_not_return_when_the_remaining_session_is_restored() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let mut panes = Vec::new();
    let mut session_id = None;
    for _ in 0..4 {
        let pane = client
            .spawn(spawn_request(session_id, "printf ready; exec sleep 120"))
            .unwrap();
        session_id = Some(pane.session_id);
        let descriptor = std::fs::File::from(pane.descriptor);
        read_until(&descriptor, "ready");
        panes.push((pane.pane_id, pane.child_pid, descriptor));
    }
    let session_id = session_id.unwrap();
    // Only the two side-by-side panes remain in the window. The closed panes
    // used to stay daemon-owned and be appended as extra horizontal splits.
    for (pane_id, _, _) in &panes[2..] {
        client.discard_pane(session_id, *pane_id).unwrap();
    }
    let mut saved = summary(session_id, panes[0].0);
    saved.panes.push(pane_summary(panes[1].0));
    saved.active_pane = panes[1].0;
    saved.layout = BackgroundPaneLayout::Split {
        axis: zmux::protocol::SplitAxis::Vertical,
        first_ratio: 630,
        first: Box::new(BackgroundPaneLayout::Pane {
            pane_id: panes[0].0,
        }),
        second: Box::new(BackgroundPaneLayout::Pane {
            pane_id: panes[1].0,
        }),
    };
    client
        .detach(
            session_id,
            saved.clone(),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    let AttachOutcome::Attached { summary, .. } = client.attach(session_id, None, None).unwrap()
    else {
        panic!("a detached exclusive session must attach exclusively")
    };
    assert_eq!(summary.panes, saved.panes);
    assert_eq!(summary.layout, saved.layout);
    assert_eq!(summary.active_pane, saved.active_pane);
    for (pane_id, _, _) in &panes[2..] {
        assert!(client.attach(session_id, Some(*pane_id), None).is_err());
    }
}

#[test]
fn discarding_the_last_pane_removes_a_previously_kept_session() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client.spawn(spawn_request(None, "exec sleep 120")).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    let AttachOutcome::Attached { pane: attached, .. } =
        client.attach(pane.session_id, None, None).unwrap()
    else {
        panic!("the kept pane must attach exclusively")
    };
    client.discard_pane(pane.session_id, pane.pane_id).unwrap();
    assert!(client.list().unwrap().is_empty());
    assert!(client.attach(pane.session_id, None, None).is_err());
    drop(attached);
    drop(pane);
}

#[test]
fn releasing_a_kept_pane_preserves_it_for_reattachment() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client.spawn(spawn_request(None, "exec sleep 120")).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    let AttachOutcome::Attached { pane: attached, .. } =
        client.attach(pane.session_id, None, None).unwrap()
    else {
        panic!("the kept pane must attach exclusively")
    };
    client.close_pane(pane.session_id, pane.pane_id).unwrap();
    let AttachOutcome::Attached { pane: restored, .. } =
        client.attach(pane.session_id, None, None).unwrap()
    else {
        panic!("releasing an attachment must preserve its kept pane")
    };
    assert_eq!(restored.pane_id, pane.pane_id);
    assert_eq!(restored.child_pid, pane.child_pid);
    drop(attached);
}

#[test]
fn discarding_requires_the_panes_exclusive_holder() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let holder = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let pane = client
        .spawn(spawn_request_as(None, "exec sleep 120", holder.id()))
        .unwrap();
    let refused = client.discard_pane(pane.session_id, pane.pane_id);
    reap(holder);
    assert!(refused.unwrap_err().to_string().contains("exclusively"));
}

#[test]
fn a_detached_session_is_drained_again_when_its_client_dies() {
    // Detaching is what asks the multiplexer to keep a session. If the window
    // showing it then dies without letting go, the pane has to be read again:
    // a pane nobody reads blocks its program the moment the terminal's buffer
    // fills, so the session would look alive while being frozen.
    let daemon = TestDaemon::start();
    let client = daemon.client();

    // The pane waits to be told before producing more than a terminal buffer
    // holds, so the writing happens after the client has died, not before.
    let trigger = daemon.config.join("go");
    let marker = daemon.config.join("kept-writing");
    let pane = client
        .spawn(spawn_request(
            None,
            &format!(
                "printf ready; while [ ! -f {} ]; do sleep 0.1; done; \
                 i=0; while [ $i -lt 400 ]; do \
                 printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n'; \
                 i=$((i+1)); done; : > {}; sleep 60",
                trigger.display(),
                marker.display()
            ),
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    // Shared while it is still held, so the window that picks it up below may
    // be a different process; a backgrounded session is otherwise its own
    // window's alone.
    share_session(&client, pane.session_id, pane.pane_id);
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

    // Taken by a window that is already gone, and dropped without detaching.
    let departed = reaped_process();
    match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            departed,
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("the held session should have attached"),
    }

    std::fs::write(&trigger, b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        marker.exists(),
        "the pane was never drained again, so its program stayed blocked"
    );
    assert!(
        client
            .list()
            .unwrap()
            .iter()
            .any(|session| session.id == pane.session_id),
        "a held session must stay available after its window dies"
    );
}

/// A liveness sweep must never turn a live exclusive attachment into a second
/// PTY reader.
///
/// On macOS, asking `sysinfo` for one PID still enumerated every process. If
/// the process count changed between its sizing and fill calls, that complete
/// enumeration was discarded and the live holder was reported missing. The
/// daemon then reclaimed the pane and raced the window for output, leaving
/// full-screen applications with arbitrary holes in their escape sequences.
#[test]
fn a_live_exclusive_attachment_survives_liveness_sweeps() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; cat"))
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

    let AttachOutcome::Attached { pane, .. } = client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    else {
        panic!("the detached pane was not attached exclusively")
    };
    let descriptor = std::fs::File::from(pane.descriptor);

    // Cross two sweeps before producing output. If either one falsely
    // reclaims this process, the daemon's poll wakes on the marker and drains
    // it during the deliberate pause before this descriptor reads.
    std::thread::sleep(Duration::from_millis(4_250));
    let mut writer = &descriptor;
    writer.write_all(b"exclusive-reader-marker\n").unwrap();
    std::thread::sleep(Duration::from_millis(250));
    read_until(&descriptor, "exclusive-reader-marker");

    assert!(
        client.list().unwrap().is_empty(),
        "a private session held by this live process must not be listed as detached"
    );
}

#[test]
fn every_pane_in_a_restored_layout_keeps_its_exclusive_reader() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let first = client
        .spawn(spawn_request(None, "printf first-ready; cat"))
        .unwrap();
    let second = client
        .spawn(spawn_request(
            Some(first.session_id),
            "printf second-ready; cat",
        ))
        .unwrap();
    let first_descriptor = std::fs::File::from(first.descriptor);
    let second_descriptor = std::fs::File::from(second.descriptor);
    read_until(&first_descriptor, "first-ready");
    read_until(&second_descriptor, "second-ready");

    let mut published = summary(first.session_id, first.pane_id);
    let mut second_summary = published.panes[0].clone();
    second_summary.id = second.pane_id;
    second_summary.label = "Pane 2".to_owned();
    published.panes.push(second_summary);
    published.active_pane = second.pane_id;
    published.layout = BackgroundPaneLayout::Split {
        axis: zmux::protocol::SplitAxis::Vertical,
        first_ratio: zmux::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
        first: Box::new(BackgroundPaneLayout::Pane {
            pane_id: first.pane_id,
        }),
        second: Box::new(BackgroundPaneLayout::Pane {
            pane_id: second.pane_id,
        }),
    };
    client
        .detach(
            first.session_id,
            published,
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    drop(first_descriptor);
    drop(second_descriptor);

    let AttachOutcome::Attached {
        pane: first_attached,
        ..
    } = client
        .attach(first.session_id, Some(first.pane_id), None)
        .unwrap()
    else {
        panic!("the first pane did not attach exclusively")
    };
    let AttachOutcome::Attached {
        pane: second_attached,
        ..
    } = client
        .attach(first.session_id, Some(second.pane_id), None)
        .unwrap()
    else {
        panic!("the second pane did not attach exclusively")
    };
    let first_descriptor = std::fs::File::from(first_attached.descriptor);
    let second_descriptor = std::fs::File::from(second_attached.descriptor);

    // Cross more than two liveness sweeps. A pane reclaimed behind the
    // window's back gains a second reader in the daemon; the pause after each
    // write then lets that reader steal the marker before this descriptor can
    // consume it.
    std::thread::sleep(Duration::from_millis(4_250));
    let mut first_writer = &first_descriptor;
    first_writer.write_all(b"first-exclusive-marker\n").unwrap();
    let mut second_writer = &second_descriptor;
    second_writer
        .write_all(b"second-exclusive-marker\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(250));
    read_until(&first_descriptor, "first-exclusive-marker");
    read_until(&second_descriptor, "second-exclusive-marker");

    client.kill(first.session_id).unwrap();
}

/// One pane may exit while its viewer still owns the descriptor. That delay
/// before release must not leave the public catalog claiming it is attachable,
/// and must not make the daemon start reading from a live sibling behind its
/// exclusive viewer.
#[test]
fn an_exited_held_pane_is_unpublished_without_stalling_its_live_sibling() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let first = client
        .spawn(spawn_request(None, "printf first-ready; read line"))
        .unwrap();
    let second = client
        .spawn(spawn_request(
            Some(first.session_id),
            "printf second-ready; cat",
        ))
        .unwrap();
    let first_descriptor = std::fs::File::from(first.descriptor);
    let second_descriptor = std::fs::File::from(second.descriptor);
    read_until(&first_descriptor, "first-ready");
    read_until(&second_descriptor, "second-ready");

    let mut published = summary(first.session_id, first.pane_id);
    let mut second_summary = published.panes[0].clone();
    second_summary.id = second.pane_id;
    second_summary.label = "Pane 2".to_owned();
    published.panes.push(second_summary);
    published.layout = BackgroundPaneLayout::Split {
        axis: zmux::protocol::SplitAxis::Vertical,
        first_ratio: zmux::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
        first: Box::new(BackgroundPaneLayout::Pane {
            pane_id: first.pane_id,
        }),
        second: Box::new(BackgroundPaneLayout::Pane {
            pane_id: second.pane_id,
        }),
    };
    client
        .share(
            first.session_id,
            published,
            serde_json::Value::Null,
            None,
            true,
        )
        .unwrap();

    // End it while the exclusive viewer still owns its descriptor. This is
    // the interval in which pruning deliberately waits for that viewer, while
    // the catalog still has to stop offering the pane immediately.
    // SAFETY: `kill` takes no pointers; the pid is a pane shell the daemon has
    // not reaped yet, so it cannot have been reused.
    unsafe { libc::kill(first.child_pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let states = client.pane_states(vec![first.pane_id]).unwrap();
        if states.first().is_some_and(|state| state.exited) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the first pane did not exit; daemon log:\n{}",
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let catalogs = zmux::catalog::read_session_catalogs(&daemon.sessions_dir()).unwrap();
        let panes = catalogs
            .iter()
            .flat_map(|catalog| &catalog.sessions)
            .find(|session| session.id == first.session_id)
            .map(|session| session.panes.iter().map(|pane| pane.id).collect::<Vec<_>>());
        if panes.as_deref() == Some(&[second.pane_id]) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the catalog kept advertising the exited pane: {panes:?}; daemon log:\n{}",
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // The surviving descriptor remains its one reader. If the exit path made
    // the daemon consume from it too, the pause gives that reader time to steal
    // the marker and this read cannot find it.
    let mut second_writer = &second_descriptor;
    second_writer.write_all(b"survivor-still-live\n").unwrap();
    std::thread::sleep(Duration::from_millis(250));
    read_until(&second_descriptor, "survivor-still-live");

    client.kill(first.session_id).unwrap();
}

#[test]
fn a_session_nobody_detached_ends_with_the_window_that_had_it() {
    // Detaching is explicit. A window that dies without detaching leaves
    // sessions nobody asked to keep: promoting them turned every Zetta that
    // was killed into a pile of held sessions holding stray shells, listed
    // with no title and nothing the user could recognise or use.
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let departed = reaped_process();
    let mut request = spawn_request(None, "printf ready; sleep 60");
    request.client_process_id = departed;
    let pane = client.spawn(request).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let deadline = Instant::now() + Duration::from_secs(30);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "a session nobody detached kept running after its window died"
    );
    assert!(
        client.list().unwrap().is_empty(),
        "a session nobody detached was offered as a background session"
    );
}

/// A process that has exited *and been reaped*, standing in for a client that
/// died. Waiting matters: an unreaped child is a zombie, and a zombie still
/// reports as an existing process.
fn reaped_process() -> u32 {
    let mut child = Command::new(system_executable("true")).spawn().unwrap();
    let id = child.id();
    child.wait().unwrap();
    id
}
