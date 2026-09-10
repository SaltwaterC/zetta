use super::*;

fn shared_state(session_id: u64, revision: u64) -> SharedSessionState {
    SharedSessionState {
        version: zmux::messages::SHARED_SESSION_STATE_VERSION,
        session_id,
        revision: zmux::messages::SessionRevision(revision),
        last_operation_id: None,
        summary: BackgroundSessionSummary {
            id: session_id,
            title: "shared".to_owned(),
            authentication_required: false,
            active_pane: 41,
            layout: BackgroundPaneLayout::Pane { pane_id: 41 },
            panes: vec![BackgroundPaneSummary {
                id: 41,
                label: "Pane 1".to_owned(),
                profile: String::new(),
                configured_command: String::new(),
                application: "sh".to_owned(),
                foreground_command: None,
                terminal_title: None,
                working_directory: None,
                state: BackgroundPaneState::Running,
                exit: None,
            }],
            held: false,
            scoped_to: None,
            key_envelope: None,
        },
        state: serde_json::Value::Null,
    }
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
    assert_eq!(coordinator.remove_pane(9, 42), Some(8));
    assert_eq!(coordinator.local_pane_id(9, 42), None);
}

#[test]
fn an_older_shared_snapshot_cannot_replace_newer_revision() {
    let mut coordinator = SharedSessionCoordinator::default();
    coordinator
        .bind(9, 100, shared_state(9, 4), [(41, 7)])
        .unwrap();
    assert_eq!(coordinator.state(9).unwrap().revision.0, 4);
}
