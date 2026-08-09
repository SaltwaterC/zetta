use super::*;
use crate::pane::overlay_color_to_hex;
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
        attention_id: None,
        attention_summary: None,
        attention_body: None,
        config_path: None,
        split: None,
        profile: None,
        theme: None,
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
fn replace_pane_control_requests_validate_the_payload() {
    let mut replace = request("token", "replace_pane");
    replace.split = Some("quarters".to_owned());
    replace.profile = Some("System".to_owned());
    replace.theme = Some("Dracula".to_owned());
    assert_eq!(
        decode_control_request(&mut replace, "token"),
        Some(ControlRequestCommand::ReplacePane {
            split: Some("quarters".to_owned()),
            profile: Some("System".to_owned()),
            theme: Some("Dracula".to_owned()),
        })
    );

    let mut profile_only = request("token", "replace_pane");
    profile_only.profile = Some("System".to_owned());
    assert_eq!(
        decode_control_request(&mut profile_only, "token"),
        Some(ControlRequestCommand::ReplacePane {
            split: None,
            profile: Some("System".to_owned()),
            theme: None,
        })
    );

    for invalid in [
        request("token", "replace_pane"),
        ControlRequest {
            split: Some(String::new()),
            ..request("token", "replace_pane")
        },
        ControlRequest {
            profile: Some(String::new()),
            ..request("token", "replace_pane")
        },
        ControlRequest {
            theme: Some("Dracula".to_owned()),
            ..request("token", "replace_pane")
        },
    ] {
        let mut invalid = invalid;
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }

    let mut wrong_token = request("wrong", "replace_pane");
    wrong_token.profile = Some("System".to_owned());
    assert_eq!(decode_control_request(&mut wrong_token, "token"), None);
}

#[test]
fn unknown_control_commands_are_rejected() {
    assert_eq!(
        decode_control_request(&mut request("token", "delete_sessions"), "token"),
        None
    );
}

#[test]
fn tab_attention_control_requests_validate_target_and_payload() {
    let mut attention = request("token", "set_tab_attention");
    attention.attention_id = Some(42);
    attention.attention_summary = Some("Build finished".to_owned());
    attention.attention_body = Some("All tests passed".to_owned());
    assert_eq!(
        decode_control_request(&mut attention, "token"),
        Some(ControlRequestCommand::SetTabAttention {
            attention_id: 42,
            summary: "Build finished".to_owned(),
            body: Some("All tests passed".to_owned()),
        })
    );

    for invalid in [
        request("token", "set_tab_attention"),
        ControlRequest {
            attention_id: Some(0),
            attention_summary: Some("Build finished".to_owned()),
            ..request("token", "set_tab_attention")
        },
        ControlRequest {
            attention_id: Some(42),
            attention_summary: Some(String::new()),
            ..request("token", "set_tab_attention")
        },
        ControlRequest {
            attention_id: Some(42),
            attention_summary: Some("Build finished".to_owned()),
            pane_theme: Some("Dracula".to_owned()),
            ..request("token", "set_tab_attention")
        },
    ] {
        let mut invalid = invalid;
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }

    let mut wrong_token = attention;
    assert_eq!(decode_control_request(&mut wrong_token, "wrong"), None);
    assert!(
        request_process_tab_attention(
            u32::MAX,
            TabAttentionRequest {
                attention_id: 42,
                summary: "Build finished".to_owned(),
                body: None,
            }
        )
        .is_err()
    );
}

#[cfg(feature = "notifications")]
#[test]
fn focus_tab_control_requests_validate_target_and_reject_extra_fields() {
    let mut focus = request("token", "focus_tab");
    focus.attention_id = Some(42);
    assert_eq!(
        decode_control_request(&mut focus, "token"),
        Some(ControlRequestCommand::FocusTab { attention_id: 42 })
    );

    for invalid in [
        request("token", "focus_tab"),
        ControlRequest {
            attention_id: Some(0),
            ..request("token", "focus_tab")
        },
        ControlRequest {
            attention_id: Some(42),
            attention_summary: Some("unexpected".to_owned()),
            ..request("token", "focus_tab")
        },
        ControlRequest {
            attention_id: Some(42),
            pane_theme: Some("Dracula".to_owned()),
            ..request("token", "focus_tab")
        },
    ] {
        let mut invalid = invalid;
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }

    let mut wrong_token = request("wrong", "focus_tab");
    wrong_token.attention_id = Some(42);
    assert_eq!(decode_control_request(&mut wrong_token, "token"), None);
    assert!(request_process_focus_tab(0, 42).is_err());
    assert!(request_process_focus_tab(u32::MAX, 0).is_err());
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

#[test]
fn configuration_reload_requests_decode_the_normalized_path() {
    let mut reload = request("token", "reload_configuration");
    reload.config_path = Some("/tmp/zetta/config.json".to_owned());
    assert_eq!(
        decode_control_request(&mut reload, "token"),
        Some(ControlRequestCommand::ReloadConfiguration {
            config_path: "/tmp/zetta/config.json".to_owned(),
        })
    );

    let mut missing_path = request("token", "reload_configuration");
    assert_eq!(decode_control_request(&mut missing_path, "token"), None);
}

#[test]
fn config_path_identity_is_absolute_and_lexically_normalized() {
    let relative = config_path_identity(Path::new("./config/../config.json"));
    let absolute = config_path_identity(&std::env::current_dir().unwrap().join("config.json"));
    assert_eq!(relative, absolute);
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

    let mut named_color_request = request("token", "set_overlay");
    named_color_request.pane_overlay_color = Some("  ReD  ".to_owned());
    assert_eq!(
        decode_control_request(&mut named_color_request, "token"),
        Some(ControlRequestCommand::SetPaneOverlay {
            text: None,
            font_size: None,
            opacity: None,
            color: Some("  ReD  ".to_owned()),
        })
    );

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
        attention_id: None,
        attention_summary: None,
        attention_body: None,
        config_path: None,
        split: None,
        profile: None,
        theme: None,
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
    assert_eq!(endpoint.version, CONTROL_VERSION);

    let client = thread::spawn(move || send_open_window_request(&endpoint).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::OpenWindow { completion } = command else {
        panic!("unexpected process control command");
    };
    completion.send(true).unwrap();
    assert!(client.join().unwrap());
}

#[test]
fn control_server_delivers_a_replace_pane_request_and_completion_status() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    let expected = ReplacePaneRequest {
        split: Some("quarters".to_owned()),
        profile: Some("System".to_owned()),
        theme: Some("Dracula".to_owned()),
    };
    let client_request = expected.clone();

    let client = thread::spawn(move || send_replace_pane_request(&endpoint, &client_request));
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::ReplacePane {
        request,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, expected);
    completion.send(true).unwrap();
    assert!(client.join().unwrap().unwrap());
}

#[test]
fn control_server_delivers_a_configuration_reload_request() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    let config_path = config_path_identity(Path::new("config.json"));

    let client = thread::spawn({
        let config_path = config_path.clone();
        move || send_reload_configuration_request(&endpoint, &config_path).unwrap()
    });
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::ReloadConfiguration {
        config_path: received_path,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(received_path, config_path);
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
        color: Some("ReD".to_owned()),
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
    assert_eq!(overlay_color_to_hex(color.unwrap()), "#ff0000");
    completion.send(true).unwrap();
    assert!(client.join().unwrap());
}

#[test]
fn control_server_delivers_a_tab_attention_request() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    let expected = TabAttentionRequest {
        attention_id: 42,
        summary: "Build finished".to_owned(),
        body: Some("All tests passed".to_owned()),
    };
    let client_request = expected.clone();

    let client = thread::spawn(move || send_set_tab_attention_request(&endpoint, &client_request));
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::SetTabAttention {
        request,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, expected);
    completion.send(true).unwrap();
    assert!(client.join().unwrap().unwrap());
}

#[cfg(feature = "notifications")]
#[test]
fn control_server_delivers_a_focus_tab_request_and_completion_status() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || send_focus_tab_request(&endpoint, 42));
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::FocusTab {
        attention_id,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(attention_id, 42);
    completion.send(true).unwrap();
    assert!(client.join().unwrap().unwrap());
}

#[cfg(feature = "notifications")]
#[test]
fn control_server_reports_a_rejected_focus_tab_target() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || send_focus_tab_request(&endpoint, 42));
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::FocusTab {
        attention_id,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(attention_id, 42);
    completion.send(false).unwrap();
    assert!(!client.join().unwrap().unwrap());
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
