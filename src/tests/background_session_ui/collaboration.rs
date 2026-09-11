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

    let generation = coordinator.schedule_sync(9).unwrap();
    assert!(!coordinator.may_report_size(9));
    assert_eq!(coordinator.schedule_sync(9), None);
    assert!(coordinator.finish_sync(9, generation));
    assert!(coordinator.may_report_size(9));
    assert!(coordinator.schedule_sync(9).is_some());
}
