use super::*;
use std::{collections::HashMap, path::PathBuf};

use crate::protocol::{BackgroundPaneState, BackgroundPaneSummary};

fn shared_summary() -> BackgroundSessionSummary {
    BackgroundSessionSummary {
        id: 4,
        title: "shared".to_owned(),
        authentication_required: false,
        active_pane: 10,
        layout: BackgroundPaneLayout::Split {
            axis: "horizontal".to_owned(),
            first_ratio: crate::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
            first: Box::new(BackgroundPaneLayout::Pane { pane_id: 10 }),
            second: Box::new(BackgroundPaneLayout::Pane { pane_id: 11 }),
        },
        panes: vec![
            BackgroundPaneSummary {
                id: 10,
                label: "one".to_owned(),
                profile: "shell".to_owned(),
                configured_command: String::new(),
                application: "sh".to_owned(),
                foreground_command: None,
                terminal_title: None,
                working_directory: None,
                state: BackgroundPaneState::Running,
                exit: None,
            },
            BackgroundPaneSummary {
                id: 11,
                label: "two".to_owned(),
                profile: "shell".to_owned(),
                configured_command: String::new(),
                application: "sh".to_owned(),
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
    }
}

#[test]
fn requests_are_tagged_by_name_on_the_wire() {
    // The tag is the wire contract between a client and a daemon that may be
    // of a different build, so it is pinned rather than left to the enum order.
    let encoded = serde_json::to_value(Request::List).unwrap();
    assert_eq!(encoded, serde_json::json!({"request": "list"}));

    let attach = serde_json::to_value(Request::Attach {
        session_id: 3,
        pane_id: Some(4),
        secret: None,
        force_shared: false,
    })
    .unwrap();
    assert_eq!(attach["request"], "attach");
    assert_eq!(attach["session_id"], 3);
}

#[test]
fn ping_requests_are_tagged_by_name_on_the_wire() {
    assert_eq!(
        serde_json::to_value(Request::Ping).unwrap(),
        serde_json::json!({"request": "ping"})
    );
}

#[test]
fn image_store_messages_round_trip_with_the_raw_payload_length() {
    let request = Request::StoreImage {
        session_id: 7,
        pane_id: 8,
        length: 1234,
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["request"], "store_image");
    assert_eq!(wire["session_id"], 7);
    assert_eq!(wire["pane_id"], 8);
    assert_eq!(wire["length"], 1234);

    let parsed: Request = serde_json::from_value(wire).unwrap();
    assert!(matches!(
        parsed,
        Request::StoreImage {
            session_id: 7,
            pane_id: 8,
            length: 1234,
        }
    ));

    let response = Response::ImageStored {
        path: "/tmp/zetta-image.png".to_owned(),
    };
    let parsed: Response = serde_json::from_value(serde_json::to_value(response).unwrap()).unwrap();
    assert!(matches!(
        parsed,
        Response::ImageStored { path } if path == "/tmp/zetta-image.png"
    ));
}

#[test]
fn spawn_requests_round_trip_shell_arguments_environment_and_working_directory() {
    let mut environment = HashMap::new();
    environment.insert("PROMPT".to_owned(), "zetta-prompt".to_owned());
    environment.insert("ZETTA_TEST_VALUE".to_owned(), "from-request".to_owned());
    let request = SpawnRequest {
        session_id: Some(9),
        client_process_id: 42,
        program: Some("pwsh.exe".to_owned()),
        args: vec![
            "-NoExit".to_owned(),
            "-Command".to_owned(),
            "tracker".to_owned(),
        ],
        env: environment.clone(),
        working_directory: Some(PathBuf::from(r"C:\source\zetta")),
        size: TerminalSize {
            columns: 120,
            lines: 40,
            cell_width: 8,
            cell_height: 16,
        },
        console_palette: ConsolePalette::default(),
    };

    let wire = serde_json::to_string(&request).unwrap();
    let parsed: SpawnRequest = serde_json::from_str(&wire).unwrap();

    assert_eq!(parsed.session_id, request.session_id);
    assert_eq!(parsed.client_process_id, request.client_process_id);
    assert_eq!(parsed.program, request.program);
    assert_eq!(parsed.args, request.args);
    assert_eq!(parsed.env, environment);
    assert_eq!(parsed.working_directory, request.working_directory);
    assert_eq!(parsed.size, request.size);
}

#[test]
fn an_envelope_round_trips() {
    let envelope = Envelope {
        version: PROTOCOL_VERSION,
        token: "abcd".to_owned(),
        client_process_id: std::process::id(),
        client_id: ClientId::default(),
        stream_only: false,
        session_secret: None,
        request: Request::Kill { session_id: 9 },
    };
    let wire = serde_json::to_string(&envelope).unwrap();
    let parsed: Envelope = serde_json::from_str(&wire).unwrap();

    assert_eq!(parsed.version, PROTOCOL_VERSION);
    assert_eq!(parsed.token, "abcd");
    assert!(matches!(parsed.request, Request::Kill { session_id: 9 }));
}

#[test]
fn an_envelope_from_a_newer_client_still_parses() {
    // The version is inside the envelope, so refusing to parse one with an
    // unfamiliar field would mean never getting far enough to report the
    // mismatch — the client would see a closed connection and no reason.
    // Relative to whatever this build speaks, so the test keeps meaning the
    // same thing after the protocol is bumped.
    let newer = PROTOCOL_VERSION + 1;
    let wire =
        format!(r#"{{"version":{newer},"token":"a","request":{{"request":"list"}},"extra":true}}"#);
    let parsed: Envelope = serde_json::from_str(&wire).expect("a newer envelope must parse");

    assert_eq!(parsed.version, newer);
    assert_ne!(
        parsed.version, PROTOCOL_VERSION,
        "this test is about a version the daemon does not speak"
    );
}

#[test]
fn an_unknown_field_inside_a_request_is_still_rejected() {
    // The tolerance stops at the envelope. Silently dropping a field a newer
    // client considered essential would mean acting on a request that was not
    // the one sent.
    let wire = r#"{"session_id":1,"summary":null,"state":null,"verifier":null,
                   "snapshots":[],"extra":true}"#;
    assert!(serde_json::from_str::<DetachRequest>(wire).is_err());
}

#[test]
fn a_pane_exit_carries_the_raw_status_and_input_attribution() {
    let event = Event::PaneExited {
        session_id: 1,
        pane_id: 2,
        raw_status: Some(256),
        input_sent: true,
    };
    let wire = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&wire).unwrap();

    match parsed {
        Event::PaneExited {
            raw_status,
            input_sent,
            ..
        } => {
            assert_eq!(raw_status, Some(256));
            assert!(input_sent);
        }
        other => panic!("expected a pane exit, got {other:?}"),
    }
}

#[test]
fn shared_state_rejects_stale_layout_references_and_removes_a_pane() {
    let mut state = SharedSessionState::new(4, shared_summary(), serde_json::json!({"tab": true}));
    let operation_id = SharedOperationId::new(ClientId::new("client"), 1);
    state
        .apply_operation(
            operation_id.clone(),
            &SharedSessionOperation::ClosePane { pane_id: 11 },
        )
        .unwrap();

    assert_eq!(state.revision, SessionRevision(1));
    assert_eq!(state.last_operation_id, Some(operation_id));
    assert_eq!(state.summary.active_pane, 10);
    assert_eq!(state.summary.panes.len(), 1);
    assert!(matches!(
        state.summary.layout,
        BackgroundPaneLayout::Pane { pane_id: 10 }
    ));

    let invalid = SharedSessionOperation::SetFocus { pane_id: 99 };
    assert!(state.validate_operation(&invalid).is_err());

    let mut invalid_layout = shared_summary().layout;
    if let BackgroundPaneLayout::Split { first_ratio, .. } = &mut invalid_layout {
        *first_ratio = crate::protocol::BACKGROUND_PANE_SPLIT_RATIO_SCALE;
    }
    let state = SharedSessionState::new(4, shared_summary(), serde_json::Value::Null);
    assert!(
        state
            .validate_operation(&SharedSessionOperation::SetLayout {
                layout: invalid_layout,
            })
            .is_err()
    );
}

#[test]
fn shared_messages_round_trip_revision_and_operation_id() {
    let request = Request::ApplyShared(SharedSessionOperationRequest {
        session_id: 4,
        base_revision: SessionRevision(8),
        operation_id: SharedOperationId::new(ClientId::new("client"), 12),
        operation: SharedSessionOperation::SetFocus { pane_id: 10 },
    });
    let parsed: Request = serde_json::from_value(serde_json::to_value(request).unwrap()).unwrap();
    assert!(matches!(
        parsed,
        Request::ApplyShared(SharedSessionOperationRequest {
            base_revision: SessionRevision(8),
            operation_id: SharedOperationId { sequence: 12, .. },
            ..
        })
    ));
}

#[test]
fn version_one_shared_state_migrates_geometry_without_losing_panes() {
    let summary = shared_summary();
    let wire = serde_json::json!({
        "version": 1,
        "session_id": 4,
        "revision": 7,
        "last_operation_id": null,
        "summary": summary,
        "state": {"opaque": true}
    });

    let state: SharedSessionState = serde_json::from_value(wire).unwrap();

    assert_eq!(state.version, SHARED_SESSION_STATE_VERSION);
    assert_eq!(state.revision, SessionRevision(7));
    assert_eq!(state.presentation.layout, state.summary.layout);
    assert_eq!(state.presentation.active_pane, 10);
    assert_eq!(state.pane_ids().collect::<Vec<_>>(), vec![10, 11]);
}

#[test]
fn typed_geometry_operations_mutate_only_canonical_presentation() {
    let mut state = SharedSessionState::new(4, shared_summary(), serde_json::json!({"tab": true}));
    let opaque = state.state.clone();
    state
        .apply_operation(
            SharedOperationId::new(ClientId::new("geometry"), 1),
            &SharedSessionOperation::SetSplitRatio {
                first_pane_id: 10,
                second_pane_id: 11,
                first_ratio: 200,
            },
        )
        .unwrap();
    state
        .apply_operation(
            SharedOperationId::new(ClientId::new("geometry"), 2),
            &SharedSessionOperation::SwapPanes {
                first_pane_id: 10,
                second_pane_id: 11,
            },
        )
        .unwrap();
    state
        .apply_operation(
            SharedOperationId::new(ClientId::new("geometry"), 3),
            &SharedSessionOperation::SetMaximized { pane_id: Some(11) },
        )
        .unwrap();

    assert_eq!(state.state, opaque);
    assert_eq!(state.presentation.maximized_pane, Some(11));
    assert_eq!(state.summary.layout, state.presentation.layout);
    assert!(matches!(
        state.presentation.layout,
        BackgroundPaneLayout::Split {
            first_ratio: 200,
            first,
            second,
            ..
        } if matches!(*first, BackgroundPaneLayout::Pane { pane_id: 11 })
            && matches!(*second, BackgroundPaneLayout::Pane { pane_id: 10 })
    ));
    assert_eq!(
        state.operation_receipts.len(),
        1,
        "one receipt is retained per client"
    );
    assert_eq!(state.operation_receipts[0].operation_id.sequence, 3);
}

#[test]
fn shared_batch_wire_carries_exact_layout_and_draft_mapping() {
    let request = Request::SpawnSharedBatch(SharedSpawnBatchRequest {
        session_id: 4,
        base_revision: SessionRevision(2),
        operation_id: SharedOperationId::new(ClientId::new("batch"), 9),
        target_pane_id: Some(10),
        replacement: SharedDraftLayout::Split {
            axis: "vertical".to_owned(),
            first_ratio: 240,
            first: Box::new(SharedDraftLayout::Draft { draft_id: 1 }),
            second: Box::new(SharedDraftLayout::Existing { pane_id: 10 }),
        },
        panes: vec![SharedPaneDraft {
            draft_id: 1,
            profile: "System".to_owned(),
            command: Some(zetta_profiles::ProfileCommand::with_args(
                "sh",
                vec!["-l".to_owned()],
            )),
            env: HashMap::new(),
            working_directory: Some(PathBuf::from("/tmp")),
            size: TerminalSize {
                columns: 80,
                lines: 24,
                cell_width: 0,
                cell_height: 0,
            },
            console_palette: ConsolePalette::default(),
            metadata: shared_summary().panes[0].clone(),
        }],
        active_pane: Some(SharedPaneRef::Draft { draft_id: 1 }),
    });
    let parsed: Request = serde_json::from_value(serde_json::to_value(request).unwrap()).unwrap();

    assert!(matches!(
        parsed,
        Request::SpawnSharedBatch(SharedSpawnBatchRequest {
            target_pane_id: Some(10),
            replacement: SharedDraftLayout::Split {
                first_ratio: 240,
                ..
            },
            ..
        })
    ));
}
