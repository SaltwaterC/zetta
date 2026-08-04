use super::*;
use futures::StreamExt as _;

fn request(token: &str, command: &str) -> ControlRequest {
    ControlRequest {
        token: token.to_owned(),
        command: command.to_owned(),
        runner_id: None,
        session_id: None,
        secret: None,
        icon: None,
    }
}

#[test]
fn control_requests_require_the_endpoint_token() {
    assert_eq!(
        decode_control_request(&mut request("correct", "open_window"), "correct"),
        Some(ControlRequestCommand::OpenWindow)
    );
    assert_eq!(
        decode_control_request(&mut request("wrong", "open_window"), "correct"),
        None
    );
}

#[test]
fn unknown_control_commands_are_rejected() {
    assert_eq!(
        decode_control_request(&mut request("token", "delete_sessions"), "token"),
        None
    );
}

#[test]
fn tab_icon_control_requests_decode_names_and_allow_clearing() {
    let mut icon_request = request("token", "set_tab_icon");
    icon_request.icon = Some("terminal".to_owned());
    assert_eq!(
        decode_control_request(&mut icon_request, "token"),
        Some(ControlRequestCommand::SetTabIcon {
            icon: Some(ui::IconName::Terminal)
        })
    );

    let mut clear_request = request("token", "set_tab_icon");
    assert_eq!(
        decode_control_request(&mut clear_request, "token"),
        Some(ControlRequestCommand::SetTabIcon { icon: None })
    );

    let mut invalid_request = request("token", "set_tab_icon");
    invalid_request.icon = Some("not-an-icon".to_owned());
    assert_eq!(decode_control_request(&mut invalid_request, "token"), None);
}

#[test]
fn reconnect_results_use_distinct_control_statuses() {
    assert_eq!(
        reconnect_session_status(ReconnectSessionResult::AuthenticationFailed),
        "authentication_failed"
    );
    assert_eq!(
        reconnect_session_status(ReconnectSessionResult::SessionNotFound),
        "session_not_found"
    );
    assert_eq!(
        reconnect_session_status(ReconnectSessionResult::StillStarting),
        "session_starting"
    );
}

#[test]
fn reconnect_requests_carry_a_session_target_and_optional_secret() {
    let mut request = ControlRequest {
        token: "token".to_owned(),
        command: "reconnect_session".to_owned(),
        runner_id: Some(7),
        session_id: Some(42),
        secret: Some("not-an-argument".to_owned()),
        icon: None,
    };
    assert_eq!(
        decode_control_request(&mut request, "token"),
        Some(ControlRequestCommand::ReconnectSession {
            runner_id: 7,
            session_id: 42,
            secret: Some("not-an-argument".to_owned()),
        })
    );
    assert!(request.secret.is_none());
}

#[test]
fn control_server_delivers_a_token_authenticated_open_request() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || send_open_window_request(&endpoint).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::OpenWindow { completion } = command else {
        panic!("unexpected process control command");
    };
    completion.send(true).unwrap();
    assert!(client.join().unwrap());
}

#[test]
fn control_client_continues_startup_when_window_open_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || send_open_window_request(&endpoint).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::OpenWindow { completion } = command else {
        panic!("unexpected process control command");
    };
    completion.send(false).unwrap();
    assert!(!client.join().unwrap());
}

#[test]
fn shutdown_rejects_an_in_flight_window_handoff() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(&endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || send_open_window_request(&endpoint).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::OpenWindow {
        completion: _completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    server.begin_shutdown();

    assert!(!client.join().unwrap());
    assert!(!endpoint_path.exists());
    assert!(!server.is_accepting());
}
