use super::*;

fn request(token: &str, command: &str) -> ControlRequest {
    ControlRequest {
        token: token.to_owned(),
        command: command.to_owned(),
    }
}

#[test]
fn control_requests_require_the_endpoint_token() {
    assert_eq!(
        decode_control_request(&request("correct", "open_window"), "correct"),
        Some(ProcessControlCommand::OpenWindow)
    );
    assert_eq!(
        decode_control_request(&request("wrong", "open_window"), "correct"),
        None
    );
}

#[test]
fn unknown_control_commands_are_rejected() {
    assert_eq!(
        decode_control_request(&request("token", "delete_sessions"), "token"),
        None
    );
}

#[test]
fn control_server_delivers_a_token_authenticated_open_request() {
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
