use super::*;

#[test]
fn reconnect_is_immediate_only_for_one_background_session() {
    assert_eq!(reconnect_request(0), ReconnectRequest::None);
    assert_eq!(reconnect_request(1), ReconnectRequest::Immediate(0));
    assert_eq!(reconnect_request(2), ReconnectRequest::Choose);
}

#[test]
fn background_session_is_reaped_after_its_final_pane_exits() {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        icon: ProfileIcon::Zetta,
    };
    let tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
        panes: vec![TerminalPane {
            id: 3,
            label_number: 1,
            generated_label: None,
            custom_label: None,
            overlay_text: None,
            overlay_font_size: None,
            overlay_opacity: None,
            overlay_color: None,
            profile,
            environment_overrides: HashMap::new(),
            terminal: None,
            view: None,
            error: None,
            base_exited: false,
            wsl_cwd_file: None,
            pending_command: None,
            detected_worktree_title: None,
            worktree_detection_directory: None,
            worktree_detection_generation: 0,
            stack: PaneStack::default(),
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
        silent_mode: true,
        close_policy: TabClosePolicy::Close,
        custom_title: None,
        pinned_worktree_title: None,
        process_title: None,
        icon: Some(IconName::Terminal),
        pinned: false,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
    };
    let mut sessions = BackgroundSessionRunner::default();
    sessions.detach(tab, None);

    let reconnected = sessions.reconnect_at(0).unwrap();
    assert!(reconnected.silent_mode);
    sessions.detach(reconnected, None);

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
