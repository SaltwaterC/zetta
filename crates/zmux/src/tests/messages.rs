use super::*;
use std::{collections::HashMap, path::PathBuf};

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
