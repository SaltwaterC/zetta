use super::*;

#[test]
fn launch_theme_override_applies_case_insensitively_by_name_only() {
    let mut profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
    };
    apply_launch_theme_override(
        &mut profile,
        Some(&("system".to_owned(), "Override Theme".to_owned())),
    );
    assert_eq!(profile.theme, Some("Override Theme".to_owned()));

    let mut other_profile = Profile {
        name: "Other".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
    };
    apply_launch_theme_override(
        &mut other_profile,
        Some(&("system".to_owned(), "Override Theme".to_owned())),
    );
    assert_eq!(other_profile.theme, Some("Configured Theme".to_owned()));

    let mut unaffected_profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
    };
    apply_launch_theme_override(&mut unaffected_profile, None);
    assert_eq!(
        unaffected_profile.theme,
        Some("Configured Theme".to_owned())
    );
}

#[test]
fn pane_controls_idle_delay_resets_and_expires() {
    let start = Instant::now();

    assert_eq!(
        pane_controls_hide_delay(start, start + Duration::from_millis(200)),
        Some(Duration::from_millis(1000))
    );
    assert_eq!(
        pane_controls_hide_delay(start, start + PANE_CONTROLS_IDLE_DELAY),
        None
    );
    assert_eq!(
        pane_controls_hide_delay(start, start + Duration::from_secs(5)),
        None
    );
}

#[test]
fn pane_resize_combines_window_dimension_changes() {
    let mut resize = WindowResize::default();
    resize.add(SplitAxis::Vertical, 120.);
    resize.add(SplitAxis::Horizontal, 48.);
    resize.add(SplitAxis::Vertical, -8.);

    assert_eq!(resize.width_delta, 112.);
    assert_eq!(resize.height_delta, 48.);
}

#[test]
fn pane_resize_keeps_the_window_large_enough_for_client_controls() {
    assert_eq!(minimum_resized_window_extent(720., 100., px(520.)), 520.);
    assert_eq!(minimum_resized_window_extent(400., 100., px(520.)), 400.);
}

#[test]
fn mouse_window_resize_clamps_each_dimension_to_the_minimum() {
    assert_eq!(
        clamp_window_size_to_minimum(size(px(400.), px(500.))),
        size(px(520.), px(500.))
    );
    assert_eq!(
        clamp_window_size_to_minimum(size(px(600.), px(200.))),
        size(px(600.), px(320.))
    );
}

#[test]
fn pane_resize_mode_uses_a_twenty_pixel_mouse_gutter() {
    assert_eq!(PANE_RESIZE_GUTTER_SIZE, px(20.));
}

#[test]
fn pane_resize_mode_pauses_terminal_input() {
    assert!(pane_input_enabled(false));
    assert!(!pane_input_enabled(true));
}

#[test]
fn pane_resize_menu_entry_requires_at_least_two_panes() {
    assert!(!pane_resize_menu_entry_available(0));
    assert!(!pane_resize_menu_entry_available(1));
    assert!(pane_resize_menu_entry_available(2));
    assert!(pane_resize_menu_entry_available(3));
}

#[test]
fn pane_move_menu_entry_requires_at_least_two_panes() {
    assert!(!pane_move_menu_entry_available(0));
    assert!(!pane_move_menu_entry_available(1));
    assert!(pane_move_menu_entry_available(2));
    assert!(pane_move_menu_entry_available(3));
}

#[test]
fn pane_resize_arrows_follow_the_active_pane_edge() {
    let vertical = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    assert_eq!(
        pane_resize_cell_delta(&vertical, 1, SplitAxis::Vertical, -1),
        -1
    );
    assert_eq!(
        pane_resize_cell_delta(&vertical, 2, SplitAxis::Vertical, -1),
        1
    );

    let horizontal = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    assert_eq!(
        pane_resize_cell_delta(&horizontal, 1, SplitAxis::Horizontal, -1),
        -1
    );
    assert_eq!(
        pane_resize_cell_delta(&horizontal, 2, SplitAxis::Horizontal, -1),
        1
    );
}

#[test]
fn held_pane_resize_arrows_repeat_on_both_axes() {
    let mut keys = PaneResizeKeys::default();

    assert!(keys.press(PaneResizeDirection::Down));
    assert_eq!(keys.len(), 1);
    assert_eq!(keys.delta(), (0, 1));
    assert!(keys.press(PaneResizeDirection::Right));
    assert_eq!(keys.len(), 2);
    assert_eq!(keys.delta(), (1, 1));

    keys.release(PaneResizeDirection::Right);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys.delta(), (0, 1));
    assert!(!keys.press(PaneResizeDirection::Down));
}

#[test]
fn opposite_held_pane_resize_arrows_cancel_each_other() {
    let mut keys = PaneResizeKeys::default();

    assert!(keys.press(PaneResizeDirection::Left));
    assert!(keys.press(PaneResizeDirection::Right));
    assert!(keys.press(PaneResizeDirection::Down));
    assert_eq!(keys.delta(), (0, 1));
}

#[test]
fn pane_controls_toggle_hides_and_restores_each_requested_pane() {
    let mut hidden_panes = HashSet::from([1]);

    assert!(toggle_hidden_pane_controls(&mut hidden_panes, &[1, 2]));
    assert_eq!(hidden_panes, HashSet::from([1, 2]));

    assert!(!toggle_hidden_pane_controls(&mut hidden_panes, &[1, 2]));
    assert!(hidden_panes.is_empty());
}

#[test]
fn pane_controls_toggle_keeps_other_tabs_unchanged() {
    let mut hidden_panes = HashSet::from([1, 2, 3]);

    assert!(!toggle_hidden_pane_controls(&mut hidden_panes, &[1, 2]));
    assert_eq!(hidden_panes, HashSet::from([3]));
}

#[test]
fn pane_controls_can_start_hidden_without_disabling_toggles() {
    let mut hidden_panes = default_hidden_pane_controls(true, [1, 2]);

    assert_eq!(hidden_panes, HashSet::from([1, 2]));
    assert!(!toggle_hidden_pane_controls(&mut hidden_panes, &[1, 2]));
    assert!(hidden_panes.is_empty());
    assert!(default_hidden_pane_controls(false, [3]).is_empty());
}

#[test]
fn reloading_changed_pane_controls_default_resets_open_panes() {
    let mut hidden_panes = HashSet::from([1, 3]);

    reset_pane_controls_visibility(&mut hidden_panes, true, [1, 2]);
    assert_eq!(hidden_panes, HashSet::from([1, 2, 3]));

    reset_pane_controls_visibility(&mut hidden_panes, false, [1, 2]);
    assert_eq!(hidden_panes, HashSet::from([3]));
}

#[test]
fn reconnect_is_immediate_only_for_one_background_session() {
    assert_eq!(reconnect_request(0), ReconnectRequest::None);
    assert_eq!(reconnect_request(1), ReconnectRequest::Immediate(0));
    assert_eq!(reconnect_request(2), ReconnectRequest::Choose);
}

#[test]
fn application_menu_navigation_wraps_in_both_directions() {
    assert_eq!(
        adjacent_application_menu_index(2, 0, ApplicationMenuDirection::Left),
        1
    );
    assert_eq!(
        adjacent_application_menu_index(2, 1, ApplicationMenuDirection::Right),
        0
    );
    assert_eq!(
        adjacent_application_menu_index(3, 1, ApplicationMenuDirection::Left),
        0
    );
    assert_eq!(
        adjacent_application_menu_index(3, 1, ApplicationMenuDirection::Right),
        2
    );
}

#[test]
fn exited_terminal_is_not_backgrounded_by_the_tab_pin() {
    let pinned = TabClosePolicy::Background {
        authentication: None,
    };

    assert!(background_authentication_for_close(&pinned, true).is_some());
    assert!(background_authentication_for_close(&pinned, false).is_none());
}

#[test]
fn new_tab_inherits_the_active_profile_after_an_explicit_profile_tab_closes() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
    };

    let profile = new_tab_profile(
        Some(&system),
        &[system.clone(), alternate],
        0,
        NewTabProfile::Inherit,
    )
    .unwrap();

    assert_eq!(profile.name, "System");
}

#[test]
fn first_tab_uses_the_configured_default_profile() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
    };

    let profile = new_tab_profile(None, &[system, alternate], 1, NewTabProfile::Default).unwrap();

    assert_eq!(profile.name, "Alternate");
}

#[test]
fn default_new_tabs_ignore_the_active_profile() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
    };

    let profile = new_tab_profile(
        Some(&alternate),
        &[system, alternate.clone()],
        0,
        NewTabProfile::Default,
    )
    .unwrap();

    assert_eq!(profile.name, "System");
}

#[test]
fn background_session_is_reaped_after_its_final_pane_exits() {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
    };
    let tab = Tab {
        id: 1,
        panes: vec![TerminalPane {
            id: 3,
            label_number: 1,
            generated_label: None,
            custom_label: None,
            profile,
            terminal: None,
            view: None,
            error: None,
            wsl_cwd_file: None,
            pending_command: None,
        }],
        pane_indices: HashMap::from([(3, 0)]),
        next_pane_label: 2,
        layout: PaneLayout::Pane(3),
        active_pane: 3,
        focus_history: vec![3],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        close_policy: TabClosePolicy::Close,
        custom_title: None,
        icon: Some(IconName::Terminal),
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
    };
    let mut sessions = BackgroundSessionRunner::default();
    sessions.detach(tab, None);

    assert_eq!(
        remove_exited_background_pane(&mut sessions, 3),
        Some(vec![3])
    );
    assert!(sessions.is_empty());
}

#[test]
fn protected_sessions_are_redacted_in_the_reconnect_picker() {
    let entries = Zetta::picker_entries_from_summaries(&[BackgroundSessionSummary {
        id: 42,
        title: "production database".to_owned(),
        authentication_required: true,
        active_pane: 7,
        layout: BackgroundPaneLayout::Pane { pane_id: 7 },
        panes: vec![BackgroundPaneSummary {
            id: 7,
            label: "secret work".to_owned(),
            profile: "System".to_owned(),
            configured_command: "sensitive-command".to_owned(),
            application: "psql".to_owned(),
            foreground_command: None,
            terminal_title: None,
            working_directory: None,
            state: BackgroundPaneState::Running,
        }],
    }]);

    assert_eq!(
        entries,
        vec![(
            42,
            "Protected session".to_owned(),
            "Session 42 · protected".to_owned()
        )]
    );
}

#[test]
fn pane_window_edges_follow_split_direction() {
    let edges = PaneWindowEdges::all();
    assert!(!edges.with_bottom(false).bottom);

    let top = edges.first(SplitAxis::Horizontal);
    let bottom = edges.second(SplitAxis::Horizontal);
    assert!(top.left && top.right && !top.bottom);
    assert!(bottom.left && bottom.right && bottom.bottom);

    let left = edges.first(SplitAxis::Vertical);
    let right = edges.second(SplitAxis::Vertical);
    assert!(left.left && left.bottom && !left.right);
    assert!(!right.left && right.bottom && right.right);
}
