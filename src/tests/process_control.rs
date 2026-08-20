use super::*;
use crate::pane::{PaneDirection, overlay_color_to_hex};
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
        tab_name: None,
        worktree_name: None,
        config_path: None,
        split: None,
        profile: None,
        theme: None,
        pane_request: None,
    }
}

fn send_reconnect_session_request(
    endpoint: &ControlEndpoint,
    runner_id: u64,
    session_id: u64,
    attention_id: Option<u64>,
    secret: Option<SessionSecret>,
) -> Result<ReconnectSessionResult> {
    use zeroize::Zeroize as _;

    let mut stream = UnixStream::connect(&endpoint.socket_path)?;
    stream.set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_CLIENT_TIMEOUT))?;
    let mut request = ControlRequest {
        token: endpoint.token.clone(),
        command: "reconnect_session".to_owned(),
        runner_id: Some(runner_id),
        session_id: Some(session_id),
        secret: secret.as_ref().map(|secret| secret.expose().to_owned()),
        icon: None,
        pane_theme: None,
        pane_overlay: None,
        pane_overlay_font_size: None,
        pane_overlay_opacity: None,
        pane_overlay_color: None,
        attention_id,
        attention_summary: None,
        attention_body: None,
        tab_name: None,
        worktree_name: None,
        config_path: None,
        split: None,
        profile: None,
        theme: None,
        pane_request: None,
    };
    let result = write_message(&mut stream, &request).and_then(|()| {
        let response = read_message::<ControlResponse>(&mut stream)?;
        Ok(match response.status.as_str() {
            "ok" => ReconnectSessionResult::Reconnected,
            "authentication_failed" => ReconnectSessionResult::AuthenticationFailed,
            "session_not_found" => ReconnectSessionResult::SessionNotFound,
            "session_starting" => ReconnectSessionResult::StillStarting,
            _ => ReconnectSessionResult::Rejected,
        })
    });
    if let Some(secret) = request.secret.as_mut() {
        secret.zeroize();
    }
    result
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
fn pane_control_requests_validate_authentication_and_payloads() {
    let expected = PaneCommand {
        direction: Some(PaneDirection::Right),
        label: Some("api".to_owned()),
        pane: None,
        overlay: Some(PaneOverlayRequest {
            text: Some("API".to_owned()),
            font_size: Some(OverlayFontSize::Large),
            opacity: Some(70),
            color: Some("cyan".to_owned()),
        }),
        stack: false,
        list: false,
        command: vec![
            "npm".to_owned(),
            "run dev".to_owned(),
            "--host=127.0.0.1".to_owned(),
        ],
    };
    let mut valid = request("token", "run_pane");
    valid.pane_request = Some((&expected).into());
    assert_eq!(
        decode_control_request(&mut valid, "token"),
        Some(ControlRequestCommand::RunPane {
            request: expected.clone(),
        })
    );

    let mut wrong_token = request("wrong", "run_pane");
    wrong_token.pane_request = Some((&expected).into());
    assert_eq!(decode_control_request(&mut wrong_token, "token"), None);

    let invalid_requests = [
        PaneControlRequest {
            direction: None,
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            command: Vec::new(),
        },
        PaneControlRequest {
            direction: Some("sideways".to_owned()),
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            command: vec!["echo".to_owned()],
        },
        PaneControlRequest {
            direction: None,
            label: Some("api".to_owned()),
            pane: None,
            overlay: None,
            stack: false,
            command: vec!["echo".to_owned()],
        },
        PaneControlRequest {
            direction: Some("right".to_owned()),
            label: None,
            pane: Some("api".to_owned()),
            overlay: None,
            stack: false,
            command: vec!["echo".to_owned()],
        },
        PaneControlRequest {
            direction: Some("right".to_owned()),
            label: None,
            pane: None,
            overlay: None,
            stack: true,
            command: vec!["echo".to_owned()],
        },
        PaneControlRequest {
            direction: None,
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            command: vec!["x".repeat(MAX_PANE_COMMAND_BYTES + 1)],
        },
        PaneControlRequest {
            direction: None,
            label: None,
            pane: None,
            overlay: Some(PaneControlOverlayRequest {
                text: Some("API".to_owned()),
                font_size: None,
                opacity: None,
                color: None,
            }),
            stack: false,
            command: vec!["echo".to_owned()],
        },
        PaneControlRequest {
            direction: Some("right".to_owned()),
            label: None,
            pane: None,
            overlay: Some(PaneControlOverlayRequest {
                text: None,
                font_size: Some("xl".to_owned()),
                opacity: None,
                color: None,
            }),
            stack: false,
            command: vec!["echo".to_owned()],
        },
        PaneControlRequest {
            direction: Some("right".to_owned()),
            label: None,
            pane: None,
            overlay: Some(PaneControlOverlayRequest {
                text: Some("API".to_owned()),
                font_size: Some("huge".to_owned()),
                opacity: None,
                color: None,
            }),
            stack: false,
            command: vec!["echo".to_owned()],
        },
        PaneControlRequest {
            direction: Some("right".to_owned()),
            label: None,
            pane: None,
            overlay: Some(PaneControlOverlayRequest {
                text: Some("API".to_owned()),
                font_size: None,
                opacity: Some(101),
                color: None,
            }),
            stack: false,
            command: vec!["echo".to_owned()],
        },
        PaneControlRequest {
            direction: Some("right".to_owned()),
            label: None,
            pane: None,
            overlay: Some(PaneControlOverlayRequest {
                text: Some("API".to_owned()),
                font_size: None,
                opacity: None,
                color: Some("nope".to_owned()),
            }),
            stack: false,
            command: vec!["echo".to_owned()],
        },
    ];
    for pane_request in invalid_requests {
        let mut invalid = request("token", "run_pane");
        invalid.pane_request = Some(pane_request);
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }

    let mut list = request("token", "list_panes");
    assert_eq!(
        decode_control_request(&mut list, "token"),
        Some(ControlRequestCommand::ListPaneLabels)
    );
    let mut list_with_payload = request("token", "list_panes");
    list_with_payload.pane_request = Some((&expected).into());
    assert_eq!(
        decode_control_request(&mut list_with_payload, "token"),
        None
    );
}

#[test]
fn pane_control_responses_round_trip_labels_and_structured_errors() {
    let response = ControlResponse {
        status: "rejected".to_owned(),
        themes: Vec::new(),
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
fn unknown_control_commands_are_rejected() {
    assert_eq!(
        decode_control_request(&mut request("token", "delete_sessions"), "token"),
        None
    );
}

#[test]
fn silent_mode_control_requests_decode_with_optional_attention_target() {
    assert_eq!(
        decode_control_request(&mut request("token", "get_silent_mode"), "token"),
        Some(ControlRequestCommand::GetSilentMode { attention_id: None })
    );

    let mut targeted = request("token", "get_silent_mode");
    targeted.attention_id = Some(42);
    assert_eq!(
        decode_control_request(&mut targeted, "token"),
        Some(ControlRequestCommand::GetSilentMode {
            attention_id: Some(42)
        })
    );

    let mut invalid = request("token", "get_silent_mode");
    invalid.profile = Some("unexpected".to_owned());
    assert_eq!(decode_control_request(&mut invalid, "token"), None);

    for mut invalid in [
        ControlRequest {
            attention_id: Some(0),
            ..request("token", "get_silent_mode")
        },
        ControlRequest {
            attention_summary: Some("unexpected".to_owned()),
            ..request("token", "get_silent_mode")
        },
    ] {
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }
}

#[test]
fn silent_mode_response_round_trips_its_state() {
    let response = ControlResponse {
        status: "ok".to_owned(),
        themes: Vec::new(),
        silent_mode: true,
        pane_labels: Vec::new(),
        error: None,
    };
    let encoded = serde_json::to_vec(&response).unwrap();
    let decoded: ControlResponse = serde_json::from_slice(&encoded).unwrap();
    assert!(decoded.silent_mode);
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
fn tab_name_control_requests_validate_target_and_support_clearing() {
    let mut set_name = request("token", "set_tab_name");
    set_name.attention_id = Some(42);
    set_name.tab_name = Some("feature/api".to_owned());
    assert_eq!(
        decode_control_request(&mut set_name, "token"),
        Some(ControlRequestCommand::SetTabName {
            attention_id: 42,
            name: Some("feature/api".to_owned()),
        })
    );

    let mut clear_name = request("token", "set_tab_name");
    clear_name.attention_id = Some(42);
    assert_eq!(
        decode_control_request(&mut clear_name, "token"),
        Some(ControlRequestCommand::SetTabName {
            attention_id: 42,
            name: None,
        })
    );

    for invalid in [
        ControlRequest {
            attention_id: Some(0),
            tab_name: Some("feature/api".to_owned()),
            ..request("token", "set_tab_name")
        },
        ControlRequest {
            attention_id: Some(42),
            tab_name: Some(String::new()),
            ..request("token", "set_tab_name")
        },
        ControlRequest {
            attention_id: Some(42),
            attention_summary: Some("unexpected".to_owned()),
            ..request("token", "set_tab_name")
        },
        ControlRequest {
            attention_id: Some(42),
            tab_name: Some("unexpected".to_owned()),
            ..request("token", "set_tab_attention")
        },
    ] {
        let mut invalid = invalid;
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }

    let mut wrong_token = set_name;
    assert_eq!(decode_control_request(&mut wrong_token, "wrong"), None);
    assert!(
        request_process_tab_name(
            u32::MAX,
            TabNameRequest {
                attention_id: 42,
                name: Some("feature/api".to_owned()),
            },
        )
        .is_err()
    );
}

#[test]
fn worktree_name_control_requests_validate_target_and_support_clearing() {
    let mut set_name = request("token", "set_worktree_name");
    set_name.attention_id = Some(42);
    set_name.worktree_name = Some("feature/api".to_owned());
    assert_eq!(
        decode_control_request(&mut set_name, "token"),
        Some(ControlRequestCommand::SetWorktreeName {
            attention_id: 42,
            name: Some("feature/api".to_owned()),
        })
    );

    let mut clear_name = request("token", "set_worktree_name");
    clear_name.attention_id = Some(42);
    assert_eq!(
        decode_control_request(&mut clear_name, "token"),
        Some(ControlRequestCommand::SetWorktreeName {
            attention_id: 42,
            name: None,
        })
    );

    for invalid in [
        ControlRequest {
            attention_id: Some(0),
            worktree_name: Some("feature/api".to_owned()),
            ..request("token", "set_worktree_name")
        },
        ControlRequest {
            attention_id: Some(42),
            worktree_name: Some(String::new()),
            ..request("token", "set_worktree_name")
        },
        ControlRequest {
            attention_id: Some(42),
            tab_name: Some("unexpected".to_owned()),
            ..request("token", "set_worktree_name")
        },
        ControlRequest {
            attention_id: Some(42),
            worktree_name: Some("unexpected".to_owned()),
            ..request("token", "set_tab_name")
        },
    ] {
        let mut invalid = invalid;
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }
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
fn project_control_requests_decode_and_reject_unexpected_payloads() {
    let mut open = request("token", "open_project");
    open.config_path = Some("/tmp/project".to_owned());
    assert_eq!(
        decode_control_request(&mut open, "token"),
        Some(ControlRequestCommand::OpenProject {
            root: PathBuf::from("/tmp/project"),
        })
    );

    assert_eq!(
        decode_control_request(&mut request("token", "reload_projects"), "token"),
        Some(ControlRequestCommand::ReloadProjects)
    );
    let mut reload_with_payload = request("token", "reload_projects");
    reload_with_payload.config_path = Some("/tmp/project".to_owned());
    assert_eq!(
        decode_control_request(&mut reload_with_payload, "token"),
        None
    );
    assert_eq!(
        decode_control_request(&mut request("token", "open_project"), "token"),
        None
    );
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
        tab_name: None,
        worktree_name: None,
        config_path: None,
        split: None,
        profile: None,
        theme: None,
        pane_request: None,
    };
    assert_eq!(
        decode_control_request(&mut request, "token"),
        Some(ControlRequestCommand::ReconnectSession {
            runner_id: 7,
            session_id: 42,
            attention_id: None,
            secret: Some(SessionSecret::new("not-an-argument".to_owned())),
        })
    );
    assert!(request.secret.is_none());
}

#[test]
fn reconnect_requests_can_target_the_originating_tab() {
    let mut reconnect_request = request("token", "reconnect_session");
    reconnect_request.runner_id = Some(7);
    reconnect_request.session_id = Some(42);
    reconnect_request.attention_id = Some(99);
    assert_eq!(
        decode_control_request(&mut reconnect_request, "token"),
        Some(ControlRequestCommand::ReconnectSession {
            runner_id: 7,
            session_id: 42,
            attention_id: Some(99),
            secret: None,
        })
    );

    let mut invalid = request("token", "reconnect_session");
    invalid.runner_id = Some(7);
    invalid.session_id = Some(42);
    invalid.attention_id = Some(0);
    assert_eq!(decode_control_request(&mut invalid, "token"), None);
}

#[test]
fn a_token_differing_only_in_its_last_byte_is_rejected() {
    // The token compare is constant time, which means it must not short-circuit
    // on the first differing byte. A mismatch confined to the final byte is the
    // case a short-circuiting compare would still get right, so this guards the
    // rest of the property indirectly: it fails loudly if the comparison is
    // ever swapped for something that only inspects a prefix.
    let token = "0123456789abcdef";
    for wrong in [
        "0123456789abcdee",
        "0123456789abcdefa",
        "0123456789abcde",
        "",
        "f123456789abcdef",
    ] {
        assert!(
            !token_matches(wrong, token),
            "{wrong:?} must not authenticate against {token:?}"
        );
    }
    assert!(token_matches(token, token));
}

#[test]
fn an_authentication_failure_still_fits_the_completion_budget() {
    // A wrong secret costs one Argon2 verification. Guess-rate limiting refuses
    // early attempts rather than sleeping on them, precisely so it stays out of
    // this budget: a delay long enough to matter would exceed the timeout, and
    // `zmux reconnect` would report that Zetta refused the request
    // rather than that the secret was wrong. If verification alone ever
    // approaches the budget, raise the budget rather than the Argon2 cost.
    let authentication =
        crate::background_sessions::SessionAuthentication::create("secret").unwrap();
    let started = Instant::now();
    assert!(authentication.verify("wrong").is_none());
    let verification = started.elapsed();

    assert!(
        verification * 4 < RECONNECT_COMPLETION_TIMEOUT,
        "verification takes {verification:?}, too close to the \
         {RECONNECT_COMPLETION_TIMEOUT:?} reconnect budget"
    );
}

#[cfg(unix)]
#[test]
fn the_control_socket_is_not_reachable_by_other_users() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("sessions").join("control.json");
    let (commands, _received) = futures::channel::mpsc::unbounded();
    let server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();

    // Connecting to a Unix socket requires write permission on it, so the mode
    // is what keeps another local user off the control channel regardless of
    // the umask Zetta happened to inherit.
    let socket = fs::metadata(&server.socket_path).unwrap().permissions();
    assert_eq!(socket.mode() & 0o777, 0o600);

    let endpoint = fs::metadata(&endpoint_path).unwrap().permissions();
    assert_eq!(endpoint.mode() & 0o777, 0o600);

    let parent = fs::metadata(endpoint_path.parent().unwrap())
        .unwrap()
        .permissions();
    assert_eq!(parent.mode() & 0o777, 0o700);
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

#[cfg(feature = "notifications")]
#[test]
fn control_server_delivers_targeted_and_untargeted_silent_mode_queries() {
    for attention_id in [None, Some(42)] {
        let directory = tempfile::tempdir().unwrap();
        let endpoint_path = directory.path().join("control.json");
        let (commands, mut received) = futures::channel::mpsc::unbounded();
        let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
        let endpoint: ControlEndpoint =
            serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

        let client =
            thread::spawn(move || send_get_silent_mode_request(&endpoint, attention_id).unwrap());
        let command = futures::executor::block_on(received.next()).unwrap();
        let ProcessControlCommand::GetSilentMode {
            attention_id: delivered_attention_id,
            completion,
        } = command
        else {
            panic!("unexpected process control command");
        };
        assert_eq!(delivered_attention_id, attention_id);
        completion.send(attention_id.is_some()).unwrap();
        assert_eq!(client.join().unwrap(), attention_id.is_some());
    }
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
fn control_server_delivers_a_pane_command_and_reports_structured_rejection() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    let expected = PaneCommand {
        direction: None,
        label: None,
        pane: Some("api".to_owned()),
        overlay: None,
        stack: true,
        list: false,
        command: vec!["tail".to_owned(), "server log".to_owned()],
    };
    let client_request = expected.clone();
    let client = thread::spawn(move || send_run_pane_request(&endpoint, &client_request));
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::RunPane {
        request,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, expected);
    completion
        .send(Err("no pane named \"api\"".to_owned()))
        .unwrap();
    let error = client.join().unwrap().unwrap_err().to_string();
    assert!(error.contains("pane_rejected"));
    assert!(error.contains("no pane named"));
}

#[test]
fn control_server_delivers_pane_label_listing() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    let client = thread::spawn(move || send_list_pane_labels_request(&endpoint));
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::ListPaneLabels { completion } = command else {
        panic!("unexpected process control command");
    };
    completion
        .send(Ok(vec!["Pane 1".to_owned(), "api".to_owned()]))
        .unwrap();
    assert_eq!(
        client.join().unwrap().unwrap(),
        Some(vec!["Pane 1".to_owned(), "api".to_owned()])
    );
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

#[test]
fn control_server_delivers_a_reconnect_origin() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || {
        send_reconnect_session_request(&endpoint, 7, 42, Some(99), None).unwrap()
    });
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::ReconnectSession {
        runner_id,
        session_id,
        attention_id,
        secret,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(runner_id, 7);
    assert_eq!(session_id, 42);
    assert_eq!(attention_id, Some(99));
    assert!(secret.is_none());
    completion
        .send(ReconnectSessionResult::Reconnected)
        .unwrap();
    assert_eq!(client.join().unwrap(), ReconnectSessionResult::Reconnected);
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
fn control_server_delivers_authenticated_tab_name_set_and_clear_requests() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let expected = TabNameRequest {
        attention_id: 42,
        name: Some("feature/api".to_owned()),
    };
    let client = thread::spawn({
        let endpoint = endpoint.clone();
        let expected = expected.clone();
        move || send_set_tab_name_request(&endpoint, &expected).unwrap()
    });
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::SetTabName {
        request,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, expected);
    completion.send(true).unwrap();
    assert!(client.join().unwrap());

    let clear = TabNameRequest {
        attention_id: 42,
        name: None,
    };
    let client_request = clear.clone();
    let client =
        thread::spawn(move || send_set_tab_name_request(&endpoint, &client_request).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::SetTabName {
        request,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, clear);
    completion.send(true).unwrap();
    assert!(client.join().unwrap());
}

#[test]
fn control_server_delivers_authenticated_worktree_name_set_and_clear_requests() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let expected = WorktreeNameRequest {
        attention_id: 42,
        name: Some("feature/api".to_owned()),
    };
    let client = thread::spawn({
        let endpoint = endpoint.clone();
        let expected = expected.clone();
        move || send_set_worktree_name_request(&endpoint, &expected).unwrap()
    });
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::SetWorktreeName {
        request,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, expected);
    completion.send(true).unwrap();
    assert!(client.join().unwrap());

    let clear = WorktreeNameRequest {
        attention_id: 42,
        name: None,
    };
    let client_request = clear.clone();
    let client =
        thread::spawn(move || send_set_worktree_name_request(&endpoint, &client_request).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::SetWorktreeName {
        request,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, clear);
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
