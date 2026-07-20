use super::*;

#[test]
fn control_requests_require_the_endpoint_token() {
    let accepted = serde_json::to_vec(&ControlRequest {
        token: "correct".to_owned(),
        command: "open_window".to_owned(),
    })
    .unwrap();
    let rejected = serde_json::to_vec(&ControlRequest {
        token: "wrong".to_owned(),
        command: "open_window".to_owned(),
    })
    .unwrap();

    assert_eq!(
        decode_control_request(&accepted, "correct"),
        Some(ProcessControlCommand::OpenWindow)
    );
    assert_eq!(decode_control_request(&rejected, "correct"), None);
}

#[test]
fn unknown_control_commands_are_rejected() {
    let request = serde_json::to_vec(&ControlRequest {
        token: "token".to_owned(),
        command: "delete_sessions".to_owned(),
    })
    .unwrap();

    assert_eq!(decode_control_request(&request, "token"), None);
}

#[test]
fn control_server_delivers_an_authenticated_open_request() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    assert!(send_open_window_request(&endpoint).unwrap());
    assert_eq!(
        received.try_recv().unwrap(),
        ProcessControlCommand::OpenWindow
    );
}
