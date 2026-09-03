use super::*;
use crate::background_sessions::{BackgroundPaneExitReason, BackgroundPaneExitSource};

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
            exit: None,
        }],
        held: false,
        scoped_to: None,
        key_envelope: None,
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
fn failed_sessions_show_the_exit_reason_in_the_reconnect_picker() {
    let entries = Zetta::picker_entries_from_summaries(&[BackgroundSessionSummary {
        id: 8,
        title: "shell".to_owned(),
        authentication_required: false,
        active_pane: 1,
        layout: BackgroundPaneLayout::Split {
            axis: "horizontal".to_owned(),
            first: Box::new(BackgroundPaneLayout::Pane { pane_id: 1 }),
            second: Box::new(BackgroundPaneLayout::Pane { pane_id: 2 }),
        },
        panes: vec![
            BackgroundPaneSummary {
                id: 1,
                label: "failed shell".to_owned(),
                profile: "System".to_owned(),
                configured_command: "pwsh".to_owned(),
                application: "htop".to_owned(),
                foreground_command: None,
                terminal_title: None,
                working_directory: None,
                state: BackgroundPaneState::Failed,
                exit: Some(BackgroundPaneExit {
                    source: BackgroundPaneExitSource::Child,
                    reason: BackgroundPaneExitReason::ForegroundCommand,
                    exit_code: Some(1),
                    child_pid: Some(42),
                    input_sent: true,
                    foreground_is_shell: Some(false),
                    foreground_command: Some("htop".to_owned()),
                }),
            },
            BackgroundPaneSummary {
                id: 2,
                label: "running shell".to_owned(),
                profile: "System".to_owned(),
                configured_command: "pwsh".to_owned(),
                application: "pwsh".to_owned(),
                foreground_command: None,
                terminal_title: None,
                working_directory: None,
                state: BackgroundPaneState::Running,
                exit: None,
            },
        ],
        held: false,
        scoped_to: None,
        key_envelope: None,
    }]);

    assert_eq!(entries.len(), 1);
    assert!(
        entries[0]
            .2
            .contains("failed: the shell exited while \"htop\" was foreground")
    );
}
