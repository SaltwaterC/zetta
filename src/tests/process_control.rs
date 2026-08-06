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
        pane_theme: None,
        pane_overlay: None,
        pane_overlay_font_size: None,
        pane_overlay_opacity: None,
        pane_overlay_color: None,
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
fn pane_theme_control_requests_decode_names_and_allow_resetting() {
    let mut theme_request = request("token", "set_pane_theme");
    theme_request.pane_theme = Some("Dracula".to_owned());
    assert_eq!(
        decode_control_request(&mut theme_request, "token"),
        Some(ControlRequestCommand::SetPaneTheme {
            theme: Some("Dracula".to_owned())
        })
    );

    let mut reset_request = request("token", "set_pane_theme");
    assert_eq!(
        decode_control_request(&mut reset_request, "token"),
        Some(ControlRequestCommand::SetPaneTheme { theme: None })
    );
}

#[test]
fn pane_theme_list_requests_carry_no_arguments() {
    assert_eq!(
        decode_control_request(&mut request("token", "list_pane_themes"), "token"),
        Some(ControlRequestCommand::ListPaneThemes)
    );

    let mut invalid_request = request("token", "list_pane_themes");
    invalid_request.pane_theme = Some("Dracula".to_owned());
    assert_eq!(decode_control_request(&mut invalid_request, "token"), None);
}

#[test]
fn pane_overlay_control_requests_decode_text_and_allow_clearing() {
    let mut overlay_request = request("token", "set_overlay");
    overlay_request.pane_overlay = Some("Prod".to_owned());
    assert_eq!(
        decode_control_request(&mut overlay_request, "token"),
        Some(ControlRequestCommand::SetPaneOverlay {
            text: Some("Prod".to_owned()),
            font_size: None,
            opacity: None,
            color: None,
        })
    );

    let mut clear_request = request("token", "set_overlay");
    assert_eq!(
        decode_control_request(&mut clear_request, "token"),
        Some(ControlRequestCommand::SetPaneOverlay {
            text: None,
            font_size: None,
            opacity: None,
            color: None,
        })
    );
}

#[test]
fn pane_overlay_control_requests_decode_style_and_reject_invalid_values() {
    let mut styled_request = request("token", "set_overlay");
    styled_request.pane_overlay = Some("Prod".to_owned());
    styled_request.pane_overlay_font_size = Some("2xl".to_owned());
    styled_request.pane_overlay_opacity = Some(50);
    styled_request.pane_overlay_color = Some("ff8800".to_owned());
    assert_eq!(
        decode_control_request(&mut styled_request, "token"),
        Some(ControlRequestCommand::SetPaneOverlay {
            text: Some("Prod".to_owned()),
            font_size: Some(OverlayFontSize::ExtraExtraLarge),
            opacity: Some(0.5),
            color: Some("ff8800".to_owned()),
        })
    );

    let mut prefixed_color_request = request("token", "set_overlay");
    prefixed_color_request.pane_overlay_color = Some("#ff8800".to_owned());
    assert!(decode_control_request(&mut prefixed_color_request, "token").is_some());

    let mut invalid_size_request = request("token", "set_overlay");
    invalid_size_request.pane_overlay_font_size = Some("huge".to_owned());
    assert_eq!(
        decode_control_request(&mut invalid_size_request, "token"),
        None
    );

    let mut invalid_color_request = request("token", "set_overlay");
    invalid_color_request.pane_overlay_color = Some("not-a-color".to_owned());
    assert_eq!(
        decode_control_request(&mut invalid_color_request, "token"),
        None
    );
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
        pane_theme: None,
        pane_overlay: None,
        pane_overlay_font_size: None,
        pane_overlay_opacity: None,
        pane_overlay_color: None,
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
fn control_server_delivers_the_registered_theme_names() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || send_list_pane_themes_request(&endpoint).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::ListPaneThemes { completion } = command else {
        panic!("unexpected process control command");
    };
    completion
        .send(vec!["Dracula".to_owned(), "One Light".to_owned()])
        .unwrap();
    assert_eq!(
        client.join().unwrap(),
        Some(vec!["Dracula".to_owned(), "One Light".to_owned()])
    );
}

#[test]
fn control_server_delivers_a_pane_overlay_request() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let overlay_request = PaneOverlayRequest {
        text: Some("Prod".to_owned()),
        font_size: Some(OverlayFontSize::Large),
        opacity: Some(50),
        color: Some("#ff8800".to_owned()),
    };
    let client =
        thread::spawn(move || send_set_overlay_request(&endpoint, &overlay_request).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::SetPaneOverlay {
        text,
        font_size,
        opacity,
        color,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(text, Some("Prod".to_owned()));
    assert_eq!(font_size, Some(OverlayFontSize::Large));
    assert_eq!(opacity, Some(0.5));
    assert!(color.is_some());
    completion.send(true).unwrap();
    assert!(client.join().unwrap());
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
