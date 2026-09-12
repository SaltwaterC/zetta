use super::*;

fn pane_summary(id: u64) -> BackgroundPaneSummary {
    BackgroundPaneSummary {
        id,
        label: format!("Pane {id}"),
        profile: "System".to_owned(),
        configured_command: String::new(),
        application: "sh".to_owned(),
        foreground_command: None,
        terminal_title: None,
        working_directory: None,
        state: BackgroundPaneState::Running,
        exit: None,
    }
}

fn shared_state(session_id: u64, revision: u64) -> SharedSessionState {
    let mut state = SharedSessionState::new(
        session_id,
        BackgroundSessionSummary {
            id: session_id,
            title: "shared".to_owned(),
            authentication_required: false,
            active_pane: 41,
            layout: BackgroundPaneLayout::Pane { pane_id: 41 },
            panes: vec![pane_summary(41)],
            held: false,
            scoped_to: None,
            key_envelope: None,
        },
        serde_json::Value::Null,
    );
    state.revision = zmux::messages::SessionRevision(revision);
    state
}

fn local_tab(tab_id: u64, pane_id: u64) -> Tab {
    Tab {
        id: tab_id,
        attention_id: tab_id,
        attention: None,
        panes: vec![TerminalPane::new(
            pane_id,
            Profile {
                name: "System".to_owned(),
                command: task::Shell::System,
                theme: None,
                dark_theme: None,
                icon: ProfileIcon::default(),
            },
        )],
        pane_indices: HashMap::from([(pane_id, 0)]),
        next_pane_label: 2,
        theme_override: None,
        layout: PaneLayout::Pane(pane_id),
        active_pane: pane_id,
        focus_history: vec![pane_id],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        shared: true,
        custom_title: None,
        worktree_seed_title: None,
        process_title: None,
        icon: None,
        icon_override: TabIconOverride::None,
        pinned: false,
        renaming_pane: None,
        rename_buffer: None,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_style_picker: None,
    }
}

fn canonical_state(session_id: u64, revision: u64) -> SharedSessionState {
    let mut state = shared_state(session_id, revision);
    state.summary.title = format!("canonical-{revision}");
    let mut tab = local_tab(session_id, 41);
    tab.custom_title = Some(format!("canonical-{revision}"));
    state.state =
        serde_json::to_value(TabState::from_tab(&tab, &HashMap::from([(41, 41)]))).unwrap();
    state
}

#[test]
fn stable_mux_ids_are_mapped_to_local_pane_ids() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, shared_state(9, 1), [(41, 7)])
        .unwrap();

    assert_eq!(coordinator.local_pane_id(9, 41), Some(7));
    assert_eq!(coordinator.mux_pane_id(9, 7), Some(41));

    coordinator.record_pane(9, 42, 8);
    assert_eq!(coordinator.local_pane_id(9, 42), Some(8));
    coordinator.record_pane(9, 42, 9);
    assert_eq!(coordinator.local_pane_id(9, 42), Some(9));
    assert_eq!(coordinator.mux_pane_id(9, 8), None);
    assert_eq!(coordinator.remove_pane(9, 42), Some(9));
    assert_eq!(coordinator.local_pane_id(9, 42), None);
}

#[test]
fn canonical_snapshots_apply_with_local_ids_and_reject_stale_revisions() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, canonical_state(9, 4), [(41, 7)])
        .unwrap();

    let mut tab = local_tab(100, 7);
    assert_eq!(
        coordinator
            .apply_snapshot_to_tab(9, canonical_state(9, 5), &mut tab)
            .unwrap(),
        SharedSnapshotDisposition::Applied
    );
    assert_eq!(tab.custom_title.as_deref(), Some("canonical-5"));
    assert_eq!(tab.active_pane, 7);
    assert_eq!(tab.layout, PaneLayout::Pane(7));

    assert_eq!(
        coordinator
            .apply_snapshot_to_tab(9, canonical_state(9, 4), &mut tab)
            .unwrap(),
        SharedSnapshotDisposition::Stale
    );
    assert_eq!(tab.custom_title.as_deref(), Some("canonical-5"));
    assert_eq!(coordinator.state(9).unwrap().revision.0, 5);
}

/// Only `ReplaceTab` and `SetTabState` replace the daemon's copy of the opaque
/// tab blob, so every geometry revision hands back whichever one was published
/// last. Applying that again rewrote the icon, the titles and the pane names
/// from it — undoing a local change still waiting for its own publication, which
/// is how an icon set in one window snapped back and never reached the other.
#[test]
fn a_geometry_only_snapshot_does_not_roll_back_unpublished_tab_state() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, canonical_state(9, 4), [(41, 7)])
        .unwrap();
    let mut tab = local_tab(100, 7);
    coordinator
        .apply_snapshot_to_tab(9, canonical_state(9, 5), &mut tab)
        .unwrap();

    // A durable change this window has made and not published yet.
    tab.custom_title = Some("chosen here".to_owned());
    tab.icon = Some(IconName::Sparkle);
    tab.icon_override = TabIconOverride::Icon(IconName::Sparkle);

    // A revision that moved the geometry and nothing else: same blob, because
    // no viewer published one.
    let mut geometry = canonical_state(9, 5);
    geometry.revision = zmux::messages::SessionRevision(6);
    geometry.presentation.maximized_pane = Some(41);
    assert_eq!(
        coordinator
            .apply_snapshot_to_tab(9, geometry, &mut tab)
            .unwrap(),
        SharedSnapshotDisposition::Applied
    );
    assert_eq!(
        tab.maximized_pane,
        Some(7),
        "geometry is canonical on every snapshot"
    );
    assert_eq!(tab.custom_title.as_deref(), Some("chosen here"));
    assert_eq!(tab.icon, Some(IconName::Sparkle));

    // The control: a snapshot that does carry a new blob still wins, so the
    // assertions above are about the blob being unchanged and not about the
    // durable half having stopped applying altogether.
    assert_eq!(
        coordinator
            .apply_snapshot_to_tab(9, canonical_state(9, 7), &mut tab)
            .unwrap(),
        SharedSnapshotDisposition::Applied
    );
    assert_eq!(tab.custom_title.as_deref(), Some("canonical-7"));
    assert_eq!(tab.icon, None);
}

/// The publication chain: geometry converges one dimension per request, and the
/// durable half is what runs once there is no geometry left to send. It used to
/// be the last arm of the same `if`/`else`, so any difference in the layout, the
/// focus, the maximized pane or the minimized ones sent a geometry operation
/// *instead of* the tab state, and nothing ever retried it.
#[test]
fn geometry_converges_a_dimension_at_a_time_and_then_yields_to_the_tab_state() {
    let canonical = canonical_state(9, 3);
    let mut summary = canonical.summary.clone();

    assert_eq!(
        shared_geometry_operation(&canonical.presentation, &summary, None, &[]),
        None,
        "an agreed geometry leaves the publication free to send the tab state"
    );

    summary.active_pane = 42;
    assert_eq!(
        shared_geometry_operation(&canonical.presentation, &summary, None, &[]),
        Some(zmux::messages::SharedSessionOperation::SetFocus { pane_id: 42 })
    );

    summary.active_pane = canonical.presentation.active_pane;
    assert_eq!(
        shared_geometry_operation(&canonical.presentation, &summary, Some(41), &[]),
        Some(zmux::messages::SharedSessionOperation::SetMaximized { pane_id: Some(41) })
    );
    assert_eq!(
        shared_geometry_operation(&canonical.presentation, &summary, None, &[41]),
        Some(zmux::messages::SharedSessionOperation::SetMinimized {
            pane_id: 41,
            minimized: true
        })
    );
}

/// A pane the daemon has not committed yet — the draft of a split in flight —
/// makes the summary undescribable in the multiplexer's ids, which used to
/// abandon the whole publication. The blob needs no such mapping.
#[test]
fn a_tab_the_summary_cannot_describe_still_publishes_its_durable_state() {
    let state = serde_json::json!({ "icon": "sparkle" });
    assert!(matches!(
        durable_state_operation(state.clone(), None),
        zmux::messages::SharedSessionOperation::SetTabState { .. }
    ));
    assert!(
        matches!(
            durable_state_operation(state, Some(shared_state(9, 1).summary)),
            zmux::messages::SharedSessionOperation::ReplaceTab { .. }
        ),
        "a summary that does map is published with the blob, so the catalog stays fresh"
    );
}

/// Geometry churn republishing identical bytes would bump the canonical revision
/// for every viewer and give them a blob they already have.
#[test]
fn the_same_tab_state_is_not_published_twice() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, canonical_state(9, 4), [(41, 7)])
        .unwrap();
    let state = serde_json::json!({ "icon": "sparkle" });

    assert!(
        coordinator.durable_state_is_unpublished(9, &state),
        "a window that has published nothing owes its first blob"
    );
    coordinator.record_published_state(9, state.clone());
    assert!(!coordinator.durable_state_is_unpublished(9, &state));
    assert!(coordinator.durable_state_is_unpublished(9, &serde_json::json!({ "icon": "pin" })));
}

/// A publication re-queues itself between its halves. Geometry the daemon will
/// not accept would otherwise propose the same change for ever.
#[test]
fn a_publication_stops_re_queuing_at_its_attempt_limit() {
    assert_eq!(
        publication_retry(0),
        Some(SharedOperation::PublishState { attempts: 1 })
    );
    assert_eq!(
        publication_retry(SHARED_PUBLISH_ATTEMPTS - 2),
        Some(SharedOperation::PublishState {
            attempts: SHARED_PUBLISH_ATTEMPTS - 1
        })
    );
    assert_eq!(publication_retry(SHARED_PUBLISH_ATTEMPTS - 1), None);
}

/// `Tab::remove_pane` leaves the layout alone, and the shared-session removers
/// have no reshape of their own — they expect a canonical snapshot to supply
/// one. When that snapshot turns out to be stale or unapplicable, the pane's
/// region stays reserved in a layout nothing can draw into, which is the grey
/// rectangle that outlived every close.
#[test]
fn detaching_a_pane_collapses_the_split_that_held_it() {
    let mut tab = local_tab(100, 7);
    tab.panes
        .push(TerminalPane::new(8, tab.panes[0].profile.clone()));
    tab.pane_indices.insert(8, 1);
    tab.layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: crate::pane::DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(7)),
        second: Box::new(PaneLayout::Pane(8)),
    };
    tab.active_pane = 8;

    detach_pane_from_tab(&mut tab, 8);

    assert_eq!(
        tab.layout,
        PaneLayout::Pane(7),
        "the survivor takes the whole region back"
    );
    assert!(tab.pane(8).is_none());
    assert_eq!(
        tab.active_pane, 7,
        "focus moves off the pane that went away"
    );
}

/// A canonical layout that places a pane the tab does not hold is exactly what
/// left grey rectangles on screen: `render_pane_leaf` draws an empty region for
/// it, and the split node goes on reserving the space, so the surviving panes
/// never grow back into it.
#[test]
fn a_snapshot_placing_a_pane_the_tab_does_not_hold_is_refused() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, canonical_state(9, 4), [(41, 7), (42, 8)])
        .unwrap();

    // The tab holds only pane 7 — pane 8 was closed here a moment ago — while
    // the canonical state still places both.
    let mut tab = local_tab(100, 7);
    let mut two_panes = canonical_state(9, 5);
    two_panes.summary.panes.push(pane_summary(42));
    two_panes.presentation.layout = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: crate::pane::DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 41 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 42 }),
    };

    let error = coordinator
        .apply_snapshot_to_tab(9, two_panes, &mut tab)
        .expect_err("a layout naming an absent pane is not applicable");
    assert!(
        format!("{error:#}").contains("does not hold"),
        "unexpected error: {error:#}"
    );
    assert_eq!(
        tab.layout,
        PaneLayout::Pane(7),
        "a refused snapshot leaves the tab's own layout alone"
    );
    assert_eq!(
        coordinator.state(9).unwrap().revision.0,
        4,
        "a refused snapshot is not recorded as the canonical state, so the next \
         one is not mistaken for stale"
    );
}

#[test]
fn pane_events_update_mappings_and_watcher_lifecycle_is_generation_safe() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, shared_state(9, 1), [(41, 7)])
        .unwrap();

    let first_watch = coordinator.begin_watch(9).unwrap();
    assert!(coordinator.watch_is_current(9, first_watch));
    assert!(coordinator.begin_watch(9).is_none());
    coordinator.end_watch(9, first_watch);

    let second_watch = coordinator.begin_watch(9).unwrap();
    coordinator.end_watch(9, first_watch);
    assert!(coordinator.watch_is_current(9, second_watch));

    let mut added = shared_state(9, 2);
    added.summary.panes.push(pane_summary(42));
    assert_eq!(
        coordinator.accept_pane_added(9, added, 42, 8).unwrap(),
        SharedSnapshotDisposition::Applied
    );
    assert_eq!(coordinator.local_pane_id(9, 42), Some(8));
    assert_eq!(coordinator.mux_pane_id(9, 8), Some(42));

    assert_eq!(
        coordinator
            .accept_pane_removed(9, shared_state(9, 1), 42)
            .unwrap(),
        None
    );
    assert_eq!(coordinator.local_pane_id(9, 42), Some(8));
    assert_eq!(
        coordinator
            .accept_pane_removed(9, shared_state(9, 3), 42)
            .unwrap(),
        Some(8)
    );
    assert_eq!(coordinator.local_pane_id(9, 42), None);
    coordinator.end_watch(9, second_watch);
    assert!(!coordinator.watch_is_current(9, second_watch));
}

#[test]
fn local_layout_diffs_choose_targeted_ratio_and_swap_operations() {
    let current = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 200,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 10 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 11 }),
    };
    let resized = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 300,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 10 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 11 }),
    };
    assert!(matches!(
        shared_layout_operation(&current, &resized, 10),
        Some(zmux::messages::SharedSessionOperation::SetSplitRatio {
            first_pane_id: 10,
            second_pane_id: 11,
            first_ratio: 300,
        })
    ));

    let swapped = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 200,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 11 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 10 }),
    };
    assert!(matches!(
        shared_layout_operation(&current, &swapped, 10),
        Some(zmux::messages::SharedSessionOperation::SwapPanes {
            first_pane_id: 10,
            second_pane_id: 11,
        })
    ));
}

#[test]
fn local_layout_diffs_choose_directional_move_and_rotation_operations() {
    let current = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 300,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 10 }),
        second: Box::new(BackgroundPaneLayout::Split {
            axis: "horizontal".to_owned(),
            first_ratio: 400,
            first: Box::new(BackgroundPaneLayout::Pane { pane_id: 11 }),
            second: Box::new(BackgroundPaneLayout::Pane { pane_id: 12 }),
        }),
    };
    let moved = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 700,
        first: Box::new(BackgroundPaneLayout::Split {
            axis: "horizontal".to_owned(),
            first_ratio: 400,
            first: Box::new(BackgroundPaneLayout::Pane { pane_id: 11 }),
            second: Box::new(BackgroundPaneLayout::Pane { pane_id: 12 }),
        }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 10 }),
    };
    assert!(matches!(
        shared_layout_operation(&current, &moved, 11),
        Some(zmux::messages::SharedSessionOperation::MovePane {
            pane_id: 11,
            direction: zmux::messages::SharedPaneDirection::Left,
        })
    ));

    let pair = BackgroundPaneLayout::Split {
        axis: "horizontal".to_owned(),
        first_ratio: 300,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 10 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 11 }),
    };
    let rotated = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 700,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 11 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 10 }),
    };
    assert!(matches!(
        shared_layout_operation(&pair, &rotated, 10),
        Some(zmux::messages::SharedSessionOperation::RotateSplit {
            pane_id: 10,
            direction: zmux::messages::SharedRotationDirection::Clockwise,
        })
    ));
}

#[test]
fn shared_geometry_queue_is_single_flight_and_remembers_pending_work() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, shared_state(9, 1), [(41, 7)])
        .unwrap();

    coordinator.enqueue(9, SharedOperation::PublishState { attempts: 0 });
    coordinator.enqueue(
        9,
        SharedOperation::ClosePane {
            local_pane_id: 7,
            mux_pane_id: 41,
            attempts: 0,
        },
    );

    assert_eq!(
        coordinator.take_next_operation(9),
        Some(SharedOperation::PublishState { attempts: 0 }),
        "operations run in the order they were asked for"
    );
    assert!(!coordinator.may_report_size(9));
    assert_eq!(
        coordinator.take_next_operation(9),
        None,
        "a second operation waits for the one in flight"
    );

    coordinator.finish_operation(9);
    assert!(coordinator.may_report_size(9));
    assert_eq!(
        coordinator.take_next_operation(9),
        Some(SharedOperation::ClosePane {
            local_pane_id: 7,
            mux_pane_id: 41,
            attempts: 0,
        })
    );
    coordinator.finish_operation(9);
    assert_eq!(coordinator.take_next_operation(9), None);
}

#[test]
fn queued_shared_operations_collapse_onto_equivalent_work() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, shared_state(9, 1), [(41, 7)])
        .unwrap();

    coordinator.enqueue(9, SharedOperation::PublishState { attempts: 0 });
    // A publication re-queues itself between its geometry and durable halves,
    // and must not leave a second one behind when a local change queues one at
    // the same time: both would publish the same tab.
    coordinator.enqueue(9, SharedOperation::PublishState { attempts: 1 });
    let close = SharedOperation::ClosePane {
        local_pane_id: 7,
        mux_pane_id: 41,
        attempts: 0,
    };
    coordinator.enqueue(9, close);
    // A retry differs only in its attempt count and must not queue a second
    // close of the same pane: the daemon refuses one naming a pane it no
    // longer holds, and that refusal would abandon the real close.
    coordinator.enqueue(
        9,
        SharedOperation::ClosePane {
            local_pane_id: 7,
            mux_pane_id: 41,
            attempts: 1,
        },
    );

    assert_eq!(
        coordinator.take_next_operation(9),
        Some(SharedOperation::PublishState { attempts: 0 })
    );
    coordinator.finish_operation(9);
    assert_eq!(coordinator.take_next_operation(9), Some(close));
    coordinator.finish_operation(9);
    assert_eq!(coordinator.take_next_operation(9), None);
}

#[test]
fn a_snapshot_older_than_the_bound_state_is_stale() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, shared_state(9, 4), [(41, 7)])
        .unwrap();

    assert!(coordinator.snapshot_is_stale(9, &shared_state(9, 3)));
    assert!(
        !coordinator.snapshot_is_stale(9, &shared_state(9, 4)),
        "the revision the window already holds is applied again, not discarded: \
         it is the answer to an operation this window asked for"
    );
    assert!(!coordinator.snapshot_is_stale(9, &shared_state(9, 5)));
}

#[test]
fn a_pane_is_only_attached_once_across_concurrent_snapshots() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, shared_state(9, 1), [(41, 7)])
        .unwrap();

    assert!(coordinator.begin_attach(9, 42));
    assert!(
        !coordinator.begin_attach(9, 42),
        "a second snapshot seeing the same pane missing must not attach it again"
    );
    coordinator.end_attach(9, 42);
    assert!(
        coordinator.begin_attach(9, 42),
        "a failed attachment is retried by the next snapshot"
    );
}

/// A split puts its new pane in the tab before the session has heard of it, so
/// a tab being reconciled mid-split holds a pane with no translation. Reading
/// that as "the session dropped this pane" deletes the very pane the split is
/// about to propose — which is how every split in a shared tab came to fail
/// with "shared draft pane is not in the tab layout".
#[test]
fn a_pane_the_session_has_not_heard_of_yet_is_not_one_it_lost() {
    let mut tab = local_tab(100, 7);
    tab.panes
        .push(TerminalPane::new(8, tab.panes[0].profile.clone()));
    tab.pane_indices.insert(8, 1);
    // Pane 7 is the session's pane 41; pane 8 is a draft with no translation.
    let local_to_mux = HashMap::from([(7, 41)]);

    let lost = panes_the_session_lost(&tab, &local_to_mux, &shared_state(9, 1));

    assert!(
        lost.is_empty(),
        "an untranslated pane is a draft, not a loss: {lost:?}"
    );
}

#[test]
fn a_pane_whose_session_pane_is_gone_is_dropped() {
    let mut tab = local_tab(100, 7);
    tab.panes
        .push(TerminalPane::new(8, tab.panes[0].profile.clone()));
    tab.pane_indices.insert(8, 1);
    // Pane 8 translates to a session pane the snapshot does not contain.
    let local_to_mux = HashMap::from([(7, 41), (8, 99)]);

    assert_eq!(
        panes_the_session_lost(&tab, &local_to_mux, &shared_state(9, 1)),
        vec![8]
    );
}

/// A snapshot arriving while a split is in flight describes a tab without the
/// pane that split just created. Installing it verbatim dropped that pane out
/// of the layout, and the proposal built from the layout then had nothing to
/// describe — every split in a shared tab failed once any event raced it.
#[test]
fn a_snapshot_keeps_a_pane_the_session_has_not_accepted_yet() {
    // The tab as a split leaves it: pane 8 has just been created beside pane 7.
    let mut previous = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: crate::pane::DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(7)),
        second: Box::new(PaneLayout::Pane(8)),
    };
    // The session's layout, which knows only pane 7.
    let mut canonical = PaneLayout::Pane(7);

    assert!(reinsert_unplaced_pane(&previous, &mut canonical, 8));
    assert_eq!(
        canonical, previous,
        "the pane goes back beside what it was split from, with that split's axis and ratio"
    );

    // And the other way round: a pane split off to the left comes back left.
    previous = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first_ratio: 300,
        first: Box::new(PaneLayout::Pane(8)),
        second: Box::new(PaneLayout::Pane(7)),
    };
    let mut canonical = PaneLayout::Pane(7);
    assert!(reinsert_unplaced_pane(&previous, &mut canonical, 8));
    assert_eq!(canonical, previous);
}

#[test]
fn a_pane_with_nothing_left_to_sit_beside_is_not_reinserted() {
    let previous = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: crate::pane::DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(7)),
        second: Box::new(PaneLayout::Pane(8)),
    };
    // Pane 7 is gone from the session too, so there is no anchor for pane 8.
    let mut canonical = PaneLayout::Pane(9);

    assert!(!reinsert_unplaced_pane(&previous, &mut canonical, 8));
    assert_eq!(canonical, PaneLayout::Pane(9), "the layout is left alone");
}

/// The same property through the path that actually installs a layout.
#[test]
fn applying_a_snapshot_mid_split_leaves_the_new_pane_in_the_layout() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, canonical_state(9, 4), [(41, 7)])
        .unwrap();

    // The tab as a split leaves it: pane 8 exists beside pane 7 and the
    // session has not been told about it yet.
    let mut tab = local_tab(100, 7);
    tab.panes
        .push(TerminalPane::new(8, tab.panes[0].profile.clone()));
    tab.pane_indices.insert(8, 1);
    tab.layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: crate::pane::DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(7)),
        second: Box::new(PaneLayout::Pane(8)),
    };

    assert_eq!(
        coordinator
            .apply_snapshot_to_tab(9, canonical_state(9, 5), &mut tab)
            .unwrap(),
        SharedSnapshotDisposition::Applied
    );
    assert!(
        tab.layout.contains_pane(8),
        "the pane a split is still proposing has to survive the snapshot that races it: {:?}",
        tab.layout
    );
    assert!(tab.layout.contains_pane(7));
}
