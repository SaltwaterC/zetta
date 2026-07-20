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
