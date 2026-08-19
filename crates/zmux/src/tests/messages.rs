use super::*;

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
fn an_envelope_round_trips() {
    let envelope = Envelope {
        version: PROTOCOL_VERSION,
        token: "abcd".to_owned(),
        client_process_id: std::process::id(),
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
