use super::*;

/// A request carrying only the endpoint token and the command name; a test
/// sets the fields its command actually uses on top of it.
///
/// Shared with the sidecars of the sibling modules, which reach it as
/// `crate::process_control::tests::request`, so the two halves of a round trip
/// build their requests the same way.
pub(super) fn request(token: &str, command: &str) -> ControlRequest {
    ControlRequest {
        token: token.to_owned(),
        command: command.to_owned(),
        ..Default::default()
    }
}

#[test]
fn pane_control_responses_round_trip_labels_and_structured_errors() {
    let response = ControlResponse {
        status: "rejected".to_owned(),
        run_id: None,
        themes: Vec::new(),
        pane_theme: None,
        pane_theme_revision: None,
        silent_mode: false,
        pane_labels: vec!["Pane 1".to_owned(), "api".to_owned()],
        error: Some(ControlError {
            code: "pane_rejected".to_owned(),
            message: "pane label \"missing\" was not found".to_owned(),
        }),
    };
    let encoded = serde_json::to_vec(&response).unwrap();
    let decoded: ControlResponse = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.status, "rejected");
    assert_eq!(decoded.pane_labels, ["Pane 1", "api"]);
    assert_eq!(decoded.error.as_ref().unwrap().code, "pane_rejected");
}

#[test]
fn silent_mode_response_round_trips_its_state() {
    let response = ControlResponse {
        status: "ok".to_owned(),
        run_id: None,
        themes: Vec::new(),
        pane_theme: None,
        pane_theme_revision: None,
        silent_mode: true,
        pane_labels: Vec::new(),
        error: None,
    };
    let encoded = serde_json::to_vec(&response).unwrap();
    let decoded: ControlResponse = serde_json::from_slice(&encoded).unwrap();
    assert!(decoded.silent_mode);
}

#[test]
fn control_request_deserialization_rejects_unknown_fields() {
    assert!(
        serde_json::from_str::<ControlRequest>(
            r#"{
            "token": "token",
            "command": "focus_tab",
            "attention_id": 42,
            "unrelated": true
        }"#
        )
        .is_err()
    );
}
