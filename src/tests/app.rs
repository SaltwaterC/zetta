use super::*;

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
fn reconnect_is_immediate_only_for_one_background_session() {
    assert_eq!(reconnect_request(0), ReconnectRequest::None);
    assert_eq!(reconnect_request(1), ReconnectRequest::Immediate(0));
    assert_eq!(reconnect_request(2), ReconnectRequest::Choose);
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

    let profile = new_tab_profile(Some(&system), &[system.clone(), alternate], 0).unwrap();

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

    let profile = new_tab_profile(None, &[system, alternate], 1).unwrap();

    assert_eq!(profile.name, "Alternate");
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
