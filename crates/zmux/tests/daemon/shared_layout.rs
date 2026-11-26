//! The layout of a shared session: published tab state, batch and headless
//! creation, splits, and repairing a summary that disagrees with the daemon.

use super::*;

/// A window publishes a shared tab in two halves: the geometry the daemon owns,
/// as a typed operation, and everything it stores without reading — the tab's
/// icon, its titles, its pane names — as an opaque blob. Only the second half
/// replaces that blob, so a viewer watching a geometry revision go past must be
/// handed back the blob it already had, and must be given the new one as soon as
/// it is published.
#[test]
fn a_published_tab_state_reaches_another_viewer_and_geometry_leaves_it_alone() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let _subscription = client.subscribe().unwrap();
    let observer = daemon.client();
    let observer_subscription = observer.subscribe().unwrap();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let offered = summary(pane.session_id, pane.pane_id);
    let shared_at_first = serde_json::json!({ "icon": "pin" });
    client
        .share(
            pane.session_id,
            offered,
            shared_at_first.clone(),
            None,
            true,
        )
        .unwrap();
    let events = observer_subscription.shared.register(pane.session_id);

    let focused = match client
        .apply_shared(
            pane.session_id,
            SessionRevision::INITIAL,
            SharedSessionOperation::SetFocus {
                pane_id: pane.pane_id,
            },
        )
        .unwrap()
    {
        zmux::client::SharedOperationResult::Applied(state) => state,
        zmux::client::SharedOperationResult::Conflict(_) => panic!("the focus change conflicted"),
    };
    match recv_timeout(&events, Duration::from_secs(10))
        .expect("a geometry change must reach the other viewer")
    {
        zmux::client::SharedSessionEvent::Updated(state) => assert_eq!(
            state.state, shared_at_first,
            "a geometry revision carries the tab state it already had"
        ),
        other => panic!("unexpected geometry event: {other:?}"),
    }

    let published = serde_json::json!({ "icon": "sparkle" });
    match client
        .apply_shared(
            pane.session_id,
            focused.revision,
            SharedSessionOperation::SetTabState {
                state: published.clone(),
            },
        )
        .unwrap()
    {
        zmux::client::SharedOperationResult::Applied(state) => {
            assert_eq!(state.state, published);
        }
        zmux::client::SharedOperationResult::Conflict(_) => {
            panic!("the tab state conflicted at the revision the focus change returned")
        }
    }
    match recv_timeout(&events, Duration::from_secs(10))
        .expect("a published tab state must reach the other viewer")
    {
        zmux::client::SharedSessionEvent::Updated(state) => {
            assert_eq!(state.state, published);
            assert!(state.revision > focused.revision);
        }
        other => panic!("unexpected tab-state event: {other:?}"),
    }

    client.kill(pane.session_id).unwrap();
    drop(descriptor);
}

#[test]
fn shared_batch_spawns_commit_exact_geometry_and_rebase_same_target_additions() {
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
    let subscription = daemon.client().subscribe().unwrap();
    let events = subscription.shared.register(pane.session_id);
    let draft = |draft_id, command: &str| zmux::messages::SharedPaneDraft {
        draft_id,
        profile: "System".to_owned(),
        command: Some(zetta_profiles::ProfileCommand::with_args(
            "sh",
            vec!["-c".to_owned(), command.to_owned()],
        )),
        env: HashMap::new(),
        working_directory: None,
        inherit_working_directory_from: None,
        load_shell_integration: false,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 0,
            cell_height: 0,
        },
        console_palette: ConsolePalette::default(),
        metadata: pane_summary(0),
    };
    let replacement = |draft_id, ratio| zmux::messages::SharedDraftLayout::Split {
        axis: zmux::protocol::SplitAxis::Vertical,
        first_ratio: ratio,
        first: Box::new(zmux::messages::SharedDraftLayout::Existing {
            pane_id: pane.pane_id,
        }),
        second: Box::new(zmux::messages::SharedDraftLayout::Draft { draft_id }),
    };
    let failed = client.spawn_shared_batch(SharedSpawnBatchRequest {
        session_id: pane.session_id,
        base_revision: SessionRevision::INITIAL,
        operation_id: client.next_shared_operation_id(),
        target_pane_id: Some(pane.pane_id),
        replacement: zmux::messages::SharedDraftLayout::Split {
            axis: zmux::protocol::SplitAxis::Horizontal,
            first_ratio: 220,
            first: Box::new(replacement(90, 210)),
            second: Box::new(zmux::messages::SharedDraftLayout::Draft { draft_id: 999 }),
        },
        panes: vec![
            draft(90, "printf provisional; sleep 60"),
            draft(91, "printf never; sleep 60"),
        ],
        active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 91 }),
    });
    assert!(failed.is_err());
    let after_failure = client.shared_snapshot(pane.session_id).unwrap();
    assert_eq!(after_failure.revision, SessionRevision::INITIAL);
    assert_eq!(after_failure.summary.panes.len(), 1);

    let first_id = client.next_shared_operation_id();
    let first_request = SharedSpawnBatchRequest {
        session_id: pane.session_id,
        base_revision: SessionRevision::INITIAL,
        operation_id: first_id.clone(),
        target_pane_id: Some(pane.pane_id),
        replacement: replacement(1, 200),
        panes: vec![draft(1, "printf one; sleep 60")],
        active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 1 }),
    };
    let zmux::client::SharedBatchResult::Applied(first) =
        client.spawn_shared_batch(first_request.clone()).unwrap()
    else {
        panic!("first batch conflicted")
    };
    assert_eq!(first.state.revision, SessionRevision(1));
    assert_eq!(first.mappings.len(), 1);
    let zmux::client::SharedBatchResult::Applied(retry) =
        client.spawn_shared_batch(first_request).unwrap()
    else {
        panic!("idempotent retry conflicted")
    };
    assert_eq!(retry.mappings, first.mappings);
    assert_eq!(retry.state.revision, SessionRevision(1));
    assert_eq!(retry.state.summary.panes.len(), 2);
    let forced = client
        .attach_shared_with_secret(
            pane.session_id,
            first.mappings[0].pane_id,
            Some(&SessionSecret::new(TEST_SECRET.to_owned())),
        )
        .unwrap();
    assert!(matches!(forced, AttachOutcome::SharedAttached { .. }));

    // The second request deliberately uses the same old base revision. A pane
    // target is rebased into the tree committed above, nesting by arrival.
    let second = client
        .spawn_shared_batch(SharedSpawnBatchRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            target_pane_id: Some(pane.pane_id),
            replacement: replacement(2, 300),
            panes: vec![draft(2, "printf two; sleep 60")],
            active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 2 }),
        })
        .unwrap();
    let zmux::client::SharedBatchResult::Applied(second) = second else {
        panic!("targeted stale batch was not rebased")
    };
    assert_eq!(second.state.revision, SessionRevision(2));
    let first_pane = first.mappings[0].pane_id;
    let second_pane = second.mappings[0].pane_id;
    let BackgroundPaneLayout::Split {
        first_ratio,
        first: first_layout,
        second: second_layout,
        ..
    } = &second.state.presentation.layout
    else {
        panic!("first addition did not produce a split")
    };
    assert_eq!(*first_ratio, 200);
    assert!(matches!(
        second_layout.as_ref(),
        BackgroundPaneLayout::Pane { pane_id: id } if *id == first_pane
    ));
    let BackgroundPaneLayout::Split {
        first_ratio,
        second: nested_second,
        ..
    } = first_layout.as_ref()
    else {
        panic!("same-target addition was not nested")
    };
    assert_eq!(*first_ratio, 300);
    assert!(matches!(
        nested_second.as_ref(),
        BackgroundPaneLayout::Pane { pane_id: id } if *id == second_pane
    ));

    let first_event = recv_timeout(&events, Duration::from_secs(10)).unwrap();
    let second_event = recv_timeout(&events, Duration::from_secs(10)).unwrap();
    assert!(matches!(
        first_event,
        zmux::client::SharedSessionEvent::PanesChanged { .. }
    ));
    assert!(matches!(
        second_event,
        zmux::client::SharedSessionEvent::PanesChanged { .. }
    ));

    // Targeted geometry ignores an intervening independent focus revision and
    // applies to the latest tree. Each operation below deliberately carries a
    // stale base revision.
    let focused = client
        .apply_shared(
            pane.session_id,
            second.state.revision,
            SharedSessionOperation::SetFocus {
                pane_id: first_pane,
            },
        )
        .unwrap();
    let zmux::client::SharedOperationResult::Applied(focused) = focused else {
        panic!("focus operation conflicted")
    };
    let resized = client
        .apply_shared(
            pane.session_id,
            second.state.revision,
            SharedSessionOperation::SetSplitRatio {
                first_pane_id: pane.pane_id,
                second_pane_id: first_pane,
                first_ratio: 650,
            },
        )
        .unwrap();
    let zmux::client::SharedOperationResult::Applied(resized) = resized else {
        panic!("stale divider operation was not rebased")
    };
    assert_eq!(resized.revision, focused.revision.next());
    assert!(matches!(
        resized.presentation.layout,
        BackgroundPaneLayout::Split {
            first_ratio: 650,
            ..
        }
    ));

    let moved = client
        .apply_shared(
            pane.session_id,
            second.state.revision,
            SharedSessionOperation::MovePane {
                pane_id: pane.pane_id,
                direction: SharedPaneDirection::Right,
            },
        )
        .unwrap();
    let zmux::client::SharedOperationResult::Applied(moved) = moved else {
        panic!("stale directional move was not rebased")
    };
    let rotated = client
        .apply_shared(
            pane.session_id,
            second.state.revision,
            SharedSessionOperation::RotateSplit {
                pane_id: pane.pane_id,
                direction: SharedRotationDirection::Clockwise,
            },
        )
        .unwrap();
    let zmux::client::SharedOperationResult::Applied(rotated) = rotated else {
        panic!("stale rotation was not rebased")
    };
    assert_eq!(rotated.revision, moved.revision.next());

    let closed = client
        .apply_shared(
            pane.session_id,
            second.state.revision,
            SharedSessionOperation::ClosePane {
                pane_id: second_pane,
            },
        )
        .unwrap();
    let zmux::client::SharedOperationResult::Applied(closed) = closed else {
        panic!("pane close conflicted")
    };
    let missing_target = client
        .apply_shared(
            pane.session_id,
            second.state.revision,
            SharedSessionOperation::SetFocus {
                pane_id: second_pane,
            },
        )
        .unwrap();
    let zmux::client::SharedOperationResult::Conflict(conflict) = missing_target else {
        panic!("a removed operation target did not return the canonical snapshot")
    };
    assert_eq!(conflict, closed);
}

/// Every pane gets a pair of forwarded-agent links in the session directory.
/// They have to go with the pane: nothing removed them, so every pane a daemon
/// had ever started left two links behind.
#[cfg(unix)]
#[test]
fn a_panes_agent_links_are_removed_with_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let created = client
        .create_shared(one_pane_shared_request(&client, "sleep 60"))
        .expect("creating a shared session");
    let pane_id = created.state.operation_receipts[0].draft_mappings[0].pane_id;
    let links = [
        daemon
            .sessions_dir()
            .join(format!("forwarded-agent-{pane_id}.sock")),
        daemon
            .sessions_dir()
            .join(format!("forwarded-agent-{pane_id}.fallback")),
    ];
    for link in &links {
        assert!(
            std::fs::symlink_metadata(link).is_ok(),
            "the pane's link {} was not made",
            link.display()
        );
    }

    client.kill(created.session_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while links
        .iter()
        .any(|link| std::fs::symlink_metadata(link).is_ok())
    {
        assert!(
            Instant::now() < deadline,
            "the pane's agent links outlived it: {links:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A window recovering a failed shared stream attaches again under the same
/// client ID, and its old stream can be torn down after the new one exists.
/// Ending the old stream removed every attachment of that client, the
/// replacement included, so the recovered pane went dead at once.
#[test]
fn ending_one_shared_stream_leaves_the_same_clients_other_attachment() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let request = one_pane_shared_request(&client, "printf ready; exec cat");
    let created = client.create_shared(request).unwrap();
    let pane_id = created.state.operation_receipts[0].draft_mappings[0].pane_id;
    let attach = || match client
        .attach_shared_with_secret(created.session_id, pane_id, None)
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a shared session's pane must attach shared"),
    };
    let failing = attach();
    let replacement = attach();
    let mut replacement_reader = replacement.reader();
    replacement.send_input(b"both-attached\n").unwrap();
    read_until_reader(&mut replacement_reader, "both-attached");

    // The old stream goes after its replacement exists, as it does when the
    // daemon notices the failure late.
    drop(failing);
    std::thread::sleep(Duration::from_millis(500));

    replacement
        .send_input(b"still-attached\n")
        .expect("the replacement must still be shared");
    read_until_reader(&mut replacement_reader, "still-attached");
    drop(replacement_reader);
    drop(replacement);
    client.kill(created.session_id).unwrap();
}

#[test]
fn headless_create_commits_a_recursive_layout_and_is_idempotent() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let draft = |draft_id: u64, label: &str| zmux::messages::SharedPaneDraft {
        draft_id,
        profile: "System".to_owned(),
        command: Some(zetta_profiles::ProfileCommand::with_args(
            "sh",
            vec!["-c".to_owned(), "sleep 60".to_owned()],
        )),
        env: HashMap::new(),
        working_directory: None,
        inherit_working_directory_from: None,
        load_shell_integration: false,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 0,
            cell_height: 0,
        },
        console_palette: ConsolePalette::default(),
        metadata: BackgroundPaneSummary {
            id: 0,
            label: label.to_owned(),
            profile: "System".to_owned(),
            configured_command: String::new(),
            application: "sh".to_owned(),
            foreground_command: None,
            terminal_title: None,
            working_directory: None,
            state: BackgroundPaneState::Starting,
            exit: None,
        },
    };
    let operation_id = client.next_shared_operation_id();
    let request = CreateSharedRequest {
        operation_id,
        title: "headless".to_owned(),
        replacement: SharedDraftLayout::Split {
            axis: zmux::protocol::SplitAxis::Horizontal,
            first_ratio: 600,
            first: Box::new(SharedDraftLayout::Draft { draft_id: 1 }),
            second: Box::new(SharedDraftLayout::Split {
                axis: zmux::protocol::SplitAxis::Vertical,
                first_ratio: 400,
                first: Box::new(SharedDraftLayout::Draft { draft_id: 2 }),
                second: Box::new(SharedDraftLayout::Draft { draft_id: 3 }),
            }),
        },
        panes: vec![draft(1, "one"), draft(2, "two"), draft(3, "three")],
        active_pane: Some(SharedPaneRef::Draft { draft_id: 3 }),
        verifier: None,
    };

    let created = client.create_shared(request.clone()).unwrap();
    assert_eq!(created.state.summary.title, "headless");
    assert_eq!(created.state.summary.panes.len(), 3);
    let active_pane = created.state.operation_receipts[0]
        .draft_mappings
        .iter()
        .find(|mapping| mapping.draft_id == 3)
        .expect("the active draft must be mapped")
        .pane_id;
    assert_eq!(created.state.summary.active_pane, active_pane);
    assert!(matches!(
        &created.state.presentation.layout,
        BackgroundPaneLayout::Split {
            axis,
            first_ratio: 600,
            second,
            ..
        } if *axis == zmux::protocol::SplitAxis::Horizontal
            && matches!(
                second.as_ref(),
                BackgroundPaneLayout::Split {
                    axis,
                    first_ratio: 400,
                    ..
                } if *axis == zmux::protocol::SplitAxis::Vertical
            )
    ));
    assert_eq!(created.state.operation_receipts.len(), 1);
    assert_eq!(created.state.operation_receipts[0].draft_mappings.len(), 3);

    let retried = client.create_shared(request).unwrap();
    assert_eq!(retried.session_id, created.session_id);
    assert_eq!(retried.state, created.state);
    assert_eq!(client.list().unwrap().len(), 1);

    let attached = client
        .attach_shared_with_secret(created.session_id, active_pane, None)
        .unwrap();
    assert!(matches!(attached, AttachOutcome::SharedAttached { .. }));
    drop(attached);
    client.kill(created.session_id).unwrap();
}

/// A client joining a shared session gets the layout as it is at that moment,
/// not as it was when sharing was switched on.
///
/// A session shared while it is on screen keeps changing: panes are split and
/// closed, tabs are renamed. The holder republishes as part of answering the
/// revoke, and the daemon re-reads the session's state once the handover
/// completes — reusing the copy read before the revoke handed the joining client
/// a layout that could be arbitrarily old.
#[test]
fn a_client_joining_a_shared_session_gets_its_layout_as_it_is_now() {
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
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"panes": 1}),
            None,
            true,
        )
        .unwrap();

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
            // The tab has been split since it was shared. Republished before
            // the snapshot, because the snapshot is what releases the waiting
            // attach.
            client
                .share(
                    session_id,
                    summary(session_id, pane_id),
                    serde_json::json!({"panes": 2}),
                    Some(&test_verifier()),
                    true,
                )
                .expect("refreshing the shared session");
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .expect("handing the screen back");
            client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .expect("the holder must re-attach");
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let state = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { state, .. } => state,
        _other => panic!("a held pane must attach as shared"),
    };
    holder.join().expect("the holder's handover failed");

    assert_eq!(
        state,
        serde_json::json!({"panes": 2}),
        "the joining client was given the layout as of when sharing was switched on"
    );
    reap(second_process);
}

/// The sequence a window actually performs when a user splits twice: the
/// second split targets the pane the first one created, and its replacement
/// names that pane by the id the commit handed back.
#[test]
fn splitting_a_pane_created_by_an_earlier_split_is_accepted() {
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

    let draft = |draft_id| zmux::messages::SharedPaneDraft {
        draft_id,
        profile: "System".to_owned(),
        command: Some(zetta_profiles::ProfileCommand::with_args(
            "sh",
            vec!["-c".to_owned(), "sleep 60".to_owned()],
        )),
        env: HashMap::new(),
        working_directory: None,
        inherit_working_directory_from: None,
        load_shell_integration: false,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 0,
            cell_height: 0,
        },
        console_palette: ConsolePalette::default(),
        metadata: pane_summary(0),
    };

    let zmux::client::SharedBatchResult::Applied(first) = client
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
            panes: vec![draft(1)],
            active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 1 }),
        })
        .unwrap()
    else {
        panic!("the first split conflicted")
    };
    let second_pane = first.mappings[0].pane_id;

    // Now split *that* pane, exactly as the window does: the target is the
    // pane the commit created, and the replacement nests it with a new draft.
    let outcome = client.spawn_shared_batch(SharedSpawnBatchRequest {
        session_id: pane.session_id,
        base_revision: first.state.revision,
        operation_id: client.next_shared_operation_id(),
        target_pane_id: Some(second_pane),
        replacement: zmux::messages::SharedDraftLayout::Split {
            axis: zmux::protocol::SplitAxis::Vertical,
            first_ratio: 200,
            first: Box::new(zmux::messages::SharedDraftLayout::Existing {
                pane_id: second_pane,
            }),
            second: Box::new(zmux::messages::SharedDraftLayout::Draft { draft_id: 2 }),
        },
        panes: vec![draft(2)],
        active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 2 }),
    });

    let zmux::client::SharedBatchResult::Applied(second) =
        outcome.expect("splitting a pane the daemon holds is not an error")
    else {
        panic!("the second split conflicted")
    };
    assert_eq!(second.state.summary.panes.len(), 3);
}

/// An exited shared pane is eventually removed once no viewer is reading it.
/// Its removal must also change the canonical layout: a later split replaces
/// only one layout node and retains every other node, including this one.
#[test]
fn pruning_an_exited_shared_pane_keeps_the_next_split_valid() {
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
            serde_json::Value::Null,
            Some(&test_verifier()),
            true,
        )
        .unwrap();

    let draft = |draft_id, command: &str| zmux::messages::SharedPaneDraft {
        draft_id,
        profile: "System".to_owned(),
        command: Some(zetta_profiles::ProfileCommand::with_args(
            "sh",
            vec!["-c".to_owned(), command.to_owned()],
        )),
        env: HashMap::new(),
        working_directory: None,
        inherit_working_directory_from: None,
        load_shell_integration: false,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 0,
            cell_height: 0,
        },
        console_palette: ConsolePalette::default(),
        metadata: pane_summary(0),
    };
    let request = spawn_request(Some(pane.session_id), "sleep 0.1; exit 0");
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
    let exited_pane = spawned.pane.pane_id();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let states = client.pane_states(vec![exited_pane]).unwrap();
        if states.first().is_some_and(|state| state.exited) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the shared pane did not exit; daemon log:\n{}",
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(spawned);
    client.leave_shared(pane.session_id).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let state = loop {
        let state = client.shared_snapshot(pane.session_id).unwrap();
        if !state.contains_pane(exited_pane) {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "the exited shared pane was never pruned; daemon log:\n{}",
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    let outcome = client.spawn_shared_batch(SharedSpawnBatchRequest {
        session_id: pane.session_id,
        base_revision: state.revision,
        operation_id: client.next_shared_operation_id(),
        target_pane_id: Some(pane.pane_id),
        replacement: zmux::messages::SharedDraftLayout::Split {
            axis: zmux::protocol::SplitAxis::Horizontal,
            first_ratio: 200,
            first: Box::new(zmux::messages::SharedDraftLayout::Existing {
                pane_id: pane.pane_id,
            }),
            second: Box::new(zmux::messages::SharedDraftLayout::Draft { draft_id: 2 }),
        },
        panes: vec![draft(2, "sleep 60")],
        active_pane: Some(zmux::messages::SharedPaneRef::Draft { draft_id: 2 }),
    });

    match outcome {
        Ok(zmux::client::SharedBatchResult::Applied(_)) => {}
        Ok(zmux::client::SharedBatchResult::Conflict(_)) => {
            panic!("splitting after an exited shared pane was pruned conflicted")
        }
        Err(error) => panic!("splitting after an exited shared pane was pruned failed: {error:#}"),
    }
}

/// An offered summary is client-supplied metadata, while the daemon's PTY
/// registry is what determines the panes a reconnect can actually attach.
/// A summary written in another id space is repaired to the latter.
///
/// A window numbers its panes from its own counter, so a summary taken
/// straight from a tab is written in *that* space. The two agree only while a
/// fresh daemon and a single window have happened to count the same number of
/// panes. The recovery must retain the daemon-issued pane rather than
/// advertising the foreign one to an attacher.
#[test]
fn an_offered_summary_describing_panes_the_daemon_lacks_is_repaired() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // A window's local pane id that this daemon has never issued.
    let foreign = pane.pane_id + 9_000;
    let mut offered = summary(pane.session_id, foreign);
    offered.layout = BackgroundPaneLayout::Pane { pane_id: foreign };

    client
        .share(
            pane.session_id,
            offered,
            serde_json::json!({"shared": true}),
            None,
            true,
        )
        .expect("a stale offered summary must be recovered in place");

    let repaired = client.shared_snapshot(pane.session_id).unwrap();
    assert_eq!(repaired.summary.active_pane, pane.pane_id);
    assert_eq!(repaired.presentation.active_pane, pane.pane_id);
    assert_eq!(
        repaired
            .summary
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect::<Vec<_>>(),
        vec![pane.pane_id]
    );
    assert_eq!(
        repaired.presentation.layout,
        BackgroundPaneLayout::Pane {
            pane_id: pane.pane_id
        }
    );
}
