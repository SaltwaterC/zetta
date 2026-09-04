use super::*;
use crate::ThemeScope;
use crate::pane::PaneDirection;
use crate::process_control::client::request_process_tab_name;
use crate::process_control::tests::request;

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
fn fresh_window_control_requests_require_authentication_and_optional_activation_token() {
    assert_eq!(
        decode_control_request(&mut request("correct", "new_window"), "correct"),
        Some(ControlRequestCommand::OpenNewWindow {
            profile: None,
            activation_token: None,
        })
    );
    let mut with_token = request("correct", "new_window");
    with_token.config_path = Some("wayland-activation-token".to_owned());
    assert_eq!(
        decode_control_request(&mut with_token, "correct"),
        Some(ControlRequestCommand::OpenNewWindow {
            profile: None,
            activation_token: Some("wayland-activation-token".to_owned()),
        })
    );
    let mut with_profile = request("correct", "new_window");
    with_profile.profile = Some("WSL: Ubuntu".to_owned());
    assert_eq!(
        decode_control_request(&mut with_profile, "correct"),
        Some(ControlRequestCommand::OpenNewWindow {
            profile: Some("WSL: Ubuntu".to_owned()),
            activation_token: None,
        })
    );
    let mut with_profile_and_token = request("correct", "new_window");
    with_profile_and_token.profile = Some("WSL: Ubuntu".to_owned());
    with_profile_and_token.config_path = Some("wayland-activation-token".to_owned());
    assert_eq!(
        decode_control_request(&mut with_profile_and_token, "correct"),
        Some(ControlRequestCommand::OpenNewWindow {
            profile: Some("WSL: Ubuntu".to_owned()),
            activation_token: Some("wayland-activation-token".to_owned()),
        })
    );
    assert_eq!(
        decode_control_request(&mut request("wrong", "new_window"), "correct"),
        None
    );

    for mut invalid in [
        ControlRequest {
            icon: Some("terminal".to_owned()),
            ..request("correct", "new_window")
        },
        ControlRequest {
            profile: Some(String::new()),
            ..request("correct", "new_window")
        },
        ControlRequest {
            attention_id: Some(42),
            ..request("correct", "new_window")
        },
        ControlRequest {
            working_directory: Some("/tmp".to_owned()),
            ..request("correct", "new_window")
        },
        ControlRequest {
            config_path: Some("x".repeat(MAX_ACTIVATION_TOKEN_BYTES + 1)),
            ..request("correct", "new_window")
        },
    ] {
        assert_eq!(decode_control_request(&mut invalid, "correct"), None);
    }
}

#[test]
fn run_control_messages_preserve_the_two_phase_payload() {
    let mut wait = request("token", "run_wait");
    wait.attention_id = Some(9);
    wait.pane_id = Some(4);
    wait.config_path = Some(
        serde_json::to_string(&RunWaitPayload {
            dependencies: vec!["api".to_owned(), "database".to_owned()],
            allow_failure: true,
            command: vec![
                "printf".to_owned(),
                "%s\\n".to_owned(),
                "--literal".to_owned(),
            ],
        })
        .unwrap(),
    );
    assert_eq!(
        decode_control_request(&mut wait, "token"),
        Some(ControlRequestCommand::RunWait {
            request: RunWaitRequest {
                owner: RunPaneIdentity::new(9, 4),
                dependencies: vec!["api".to_owned(), "database".to_owned()],
                allow_failure: true,
                command: vec![
                    "printf".to_owned(),
                    "%s\\n".to_owned(),
                    "--literal".to_owned(),
                ],
            },
        })
    );

    let mut complete = request("token", "run_complete");
    complete.session_id = Some(7);
    complete.config_path = Some("7".to_owned());
    assert_eq!(
        decode_control_request(&mut complete, "token"),
        Some(ControlRequestCommand::RunComplete {
            id: 7,
            exit_code: Some(7),
        })
    );

    let mut malformed = request("token", "run_wait");
    malformed.attention_id = Some(9);
    malformed.pane_id = Some(4);
    malformed.config_path = Some(
        serde_json::json!({
            "dependencies": ["api"],
            "allow_failure": false,
            "command": ["echo"],
            "unexpected": true,
        })
        .to_string(),
    );
    assert_eq!(decode_control_request(&mut malformed, "token"), None);
}

#[test]
fn shell_command_control_requests_round_trip_and_reject_unsupported_fields() {
    let shell_request = ShellCommandRequest {
        command: "echo $FOO".to_owned(),
        arguments: vec!["two words".to_owned(), "--literal".to_owned()],
        environment: BTreeMap::from([("FOO".to_owned(), "bar".to_owned())]),
    };
    let mut shell_control = request("token", "run_shell_command");
    shell_control.shell_command = Some((&shell_request).into());

    let encoded = serde_json::to_value(&shell_control).unwrap();
    assert_eq!(encoded["command"], "run_shell_command");
    assert_eq!(encoded["shell_command"]["command"], "echo $FOO");
    assert_eq!(encoded["shell_command"]["arguments"][0], "two words");
    assert_eq!(encoded["shell_command"]["environment"]["FOO"], "bar");
    assert_eq!(
        decode_control_request(&mut shell_control, "token"),
        Some(ControlRequestCommand::RunShellCommand {
            request: shell_request.clone(),
        })
    );

    let mut with_pane_id = request("token", "run_shell_command");
    with_pane_id.pane_id = Some(42);
    with_pane_id.shell_command = Some((&shell_request).into());
    assert_eq!(decode_control_request(&mut with_pane_id, "token"), None);

    let mut with_invalid_environment = request("token", "run_shell_command");
    with_invalid_environment.shell_command = Some(ShellCommandControlRequest {
        command: "echo".to_owned(),
        arguments: Vec::new(),
        environment: BTreeMap::from([("ZETTA_PROCESS_ID".to_owned(), "spoof".to_owned())]),
    });
    assert_eq!(
        decode_control_request(&mut with_invalid_environment, "token"),
        None
    );

    let mut with_duplicate_environment = request("token", "run_shell_command");
    with_duplicate_environment.shell_command = Some(ShellCommandControlRequest {
        command: "echo".to_owned(),
        arguments: Vec::new(),
        environment: BTreeMap::from([
            ("FOO".to_owned(), "one".to_owned()),
            ("foo".to_owned(), "two".to_owned()),
        ]),
    });
    assert_eq!(
        decode_control_request(&mut with_duplicate_environment, "token"),
        None
    );

    let mut shell_payload_on_other_command = request("token", "open_window");
    shell_payload_on_other_command.shell_command = Some((&shell_request).into());
    assert_eq!(
        decode_control_request(&mut shell_payload_on_other_command, "token"),
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
        Some(ControlRequestCommand::ListPaneLabels { attention_id: None })
    );
    let mut targeted_list = request("token", "list_panes");
    targeted_list.attention_id = Some(42);
    assert_eq!(
        decode_control_request(&mut targeted_list, "token"),
        Some(ControlRequestCommand::ListPaneLabels {
            attention_id: Some(42),
        })
    );
    let mut list_with_payload = request("token", "list_panes");
    list_with_payload.pane_request = Some((&expected).into());
    assert_eq!(
        decode_control_request(&mut list_with_payload, "token"),
        None
    );
}

#[test]
fn open_command_control_requests_preserve_the_caller_directory() {
    let expected = PaneCommand {
        direction: None,
        label: None,
        pane: None,
        overlay: None,
        stack: false,
        list: false,
        command: vec![
            "python".to_owned(),
            "-c".to_owned(),
            "print('--help')".to_owned(),
        ],
    };
    let mut valid = request("token", "open_command");
    valid.config_path = Some("/caller/working directory".to_owned());
    valid.pane_request = Some((&expected).into());
    assert_eq!(
        decode_control_request(&mut valid, "token"),
        Some(ControlRequestCommand::OpenCommand {
            request: expected.clone(),
            working_directory: Some(PathBuf::from("/caller/working directory")),
        })
    );

    for pane_request in [
        PaneControlRequest {
            direction: Some("right".to_owned()),
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            command: expected.command.clone(),
        },
        PaneControlRequest {
            direction: None,
            label: None,
            pane: None,
            overlay: None,
            stack: true,
            command: expected.command.clone(),
        },
        PaneControlRequest {
            direction: None,
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            command: Vec::new(),
        },
    ] {
        let mut invalid = request("token", "open_command");
        invalid.pane_request = Some(pane_request);
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }

    let mut invalid_cwd = request("token", "open_command");
    invalid_cwd.config_path = Some(String::new());
    invalid_cwd.pane_request = Some((&expected).into());
    assert_eq!(
        decode_control_request(&mut invalid_cwd, "token"),
        Some(ControlRequestCommand::OpenCommand {
            request: expected,
            working_directory: None,
        })
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
            working_directory: None,
        })
    );

    let mut open_from_worktree = request("token", "open_project");
    open_from_worktree.config_path = Some("/tmp/project".to_owned());
    open_from_worktree.working_directory = Some("/tmp/project-worktrees/feature".to_owned());
    assert_eq!(
        decode_control_request(&mut open_from_worktree, "token"),
        Some(ControlRequestCommand::OpenProject {
            root: PathBuf::from("/tmp/project"),
            working_directory: Some(PathBuf::from("/tmp/project-worktrees/feature")),
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

    let mut open_with_empty_working_directory = request("token", "open_project");
    open_with_empty_working_directory.config_path = Some("/tmp/project".to_owned());
    open_with_empty_working_directory.working_directory = Some(String::new());
    assert_eq!(
        decode_control_request(&mut open_with_empty_working_directory, "token"),
        Some(ControlRequestCommand::OpenProject {
            root: PathBuf::from("/tmp/project"),
            working_directory: None,
        })
    );

    let mut unexpected_working_directory = request("token", "open_window");
    unexpected_working_directory.working_directory = Some("/tmp/project".to_owned());
    assert_eq!(
        decode_control_request(&mut unexpected_working_directory, "token"),
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
fn theme_control_requests_decode_scopes_and_allow_resetting() {
    let mut theme_request = request("token", "set_theme");
    theme_request.scope = Some("pane".to_owned());
    theme_request.theme = Some("Dracula".to_owned());
    assert_eq!(
        decode_control_request(&mut theme_request, "token"),
        Some(ControlRequestCommand::SetTheme {
            scope: ThemeScope::Pane,
            theme: Some("Dracula".to_owned())
        })
    );

    let mut reset_request = request("token", "set_theme");
    reset_request.scope = Some("tab".to_owned());
    assert_eq!(
        decode_control_request(&mut reset_request, "token"),
        Some(ControlRequestCommand::SetTheme {
            scope: ThemeScope::Tab,
            theme: None
        })
    );

    let mut missing_scope = request("token", "set_theme");
    assert_eq!(decode_control_request(&mut missing_scope, "token"), None);

    let mut invalid_scope = request("token", "set_theme");
    invalid_scope.scope = Some("window".to_owned());
    assert_eq!(decode_control_request(&mut invalid_scope, "token"), None);

    let mut empty_theme = request("token", "set_theme");
    empty_theme.scope = Some("pane".to_owned());
    empty_theme.theme = Some(String::new());
    assert_eq!(decode_control_request(&mut empty_theme, "token"), None);

    assert_eq!(
        decode_control_request(&mut request("token", "set_pane_theme"), "token"),
        None
    );
}

#[test]
fn theme_list_requests_carry_no_arguments() {
    assert_eq!(
        decode_control_request(&mut request("token", "list_themes"), "token"),
        Some(ControlRequestCommand::ListThemes)
    );

    let mut invalid_request = request("token", "list_themes");
    invalid_request.pane_theme = Some("Dracula".to_owned());
    assert_eq!(decode_control_request(&mut invalid_request, "token"), None);

    assert_eq!(
        decode_control_request(&mut request("token", "list_pane_themes"), "token"),
        None
    );
}

#[test]
fn pane_theme_query_requires_an_attention_target_and_allows_legacy_panes() {
    let mut theme_request = request("token", "get_pane_theme");
    theme_request.attention_id = Some(42);
    theme_request.pane_id = Some(9);
    assert_eq!(
        decode_control_request(&mut theme_request, "token"),
        Some(ControlRequestCommand::GetPaneTheme {
            attention_id: 42,
            pane_id: Some(9),
        })
    );

    let mut legacy_request = request("token", "get_pane_theme");
    legacy_request.attention_id = Some(42);
    assert_eq!(
        decode_control_request(&mut legacy_request, "token"),
        Some(ControlRequestCommand::GetPaneTheme {
            attention_id: 42,
            pane_id: None,
        })
    );

    assert_eq!(
        decode_control_request(&mut request("token", "get_pane_theme"), "token"),
        None
    );
    let mut zero = request("token", "get_pane_theme");
    zero.attention_id = Some(0);
    zero.pane_id = Some(9);
    assert_eq!(decode_control_request(&mut zero, "token"), None);
    let mut zero_pane = request("token", "get_pane_theme");
    zero_pane.attention_id = Some(42);
    zero_pane.pane_id = Some(0);
    assert_eq!(decode_control_request(&mut zero_pane, "token"), None);
    let mut unexpected = request("token", "get_pane_theme");
    unexpected.attention_id = Some(42);
    unexpected.pane_id = Some(9);
    unexpected.profile = Some("System".to_owned());
    assert_eq!(decode_control_request(&mut unexpected, "token"), None);
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
fn reconnect_requests_carry_a_session_target_and_optional_secret() {
    let mut request = ControlRequest {
        token: "token".to_owned(),
        command: "reconnect_session".to_owned(),
        runner_id: Some(7),
        session_id: Some(42),
        secret: Some("not-an-argument".to_owned()),
        ssh_target: None,
        ssh_port: None,
        icon: None,
        pane_theme: None,
        pane_id: None,
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
        working_directory: None,
        split: None,
        profile: None,
        theme: None,
        scope: None,
        pane_request: None,
        shell_command: None,
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
fn disk_resume_requests_decode_identity_paths_from_the_private_payload() {
    let mut resume_request = request("token", "resume_disk_session");
    resume_request.session_id = Some(42);
    resume_request.secret = Some("session-secret".to_owned());
    resume_request.config_path = Some(
        serde_json::to_string(&serde_json::json!({
            "identity_paths": ["/tmp/identity", "/tmp/second"],
            "identity_passphrases": [null, "identity-secret"]
        }))
        .unwrap(),
    );
    assert_eq!(
        decode_control_request(&mut resume_request, "token"),
        Some(ControlRequestCommand::ResumeDiskSession {
            session_id: 42,
            identity_paths: vec![PathBuf::from("/tmp/identity"), PathBuf::from("/tmp/second")],
            identity_passphrases: vec![
                None,
                Some(SessionSecret::new("identity-secret".to_owned())),
            ],
            secret: Some(SessionSecret::new("session-secret".to_owned())),
        })
    );
    assert!(resume_request.secret.is_none());

    let mut malformed = request("token", "resume_disk_session");
    malformed.session_id = Some(42);
    malformed.config_path = Some("not-json".to_owned());
    assert_eq!(decode_control_request(&mut malformed, "token"), None);

    let mut mismatched = request("token", "resume_disk_session");
    mismatched.session_id = Some(42);
    mismatched.config_path = Some(
        serde_json::to_string(&serde_json::json!({
            "identity_paths": ["/tmp/identity"],
            "identity_passphrases": []
        }))
        .unwrap(),
    );
    assert_eq!(decode_control_request(&mut mismatched, "token"), None);
}

#[test]
fn remote_session_requests_validate_the_ssh_destination_and_session_target() {
    let mut remote = request("token", "open_remote_session");
    remote.session_id = Some(42);
    remote.ssh_target = Some("build-host".to_owned());
    remote.ssh_port = Some(2222);
    remote.secret = Some("session-secret".to_owned());
    assert_eq!(
        decode_control_request(&mut remote, "token"),
        Some(ControlRequestCommand::OpenRemoteSession {
            target: "build-host".to_owned(),
            port: Some(2222),
            session_id: 42,
            secret: Some(SessionSecret::new("session-secret".to_owned())),
        })
    );
    assert!(remote.secret.is_none());

    let mut without_port = request("token", "open_remote_session");
    without_port.session_id = Some(42);
    without_port.ssh_target = Some("build-host".to_owned());
    assert_eq!(
        decode_control_request(&mut without_port, "token"),
        Some(ControlRequestCommand::OpenRemoteSession {
            target: "build-host".to_owned(),
            port: None,
            session_id: 42,
            secret: None,
        })
    );

    for invalid in [
        // No destination, no session, or a session that names nothing.
        request("token", "open_remote_session"),
        ControlRequest {
            ssh_target: Some("build-host".to_owned()),
            ..request("token", "open_remote_session")
        },
        ControlRequest {
            session_id: Some(0),
            ssh_target: Some("build-host".to_owned()),
            ..request("token", "open_remote_session")
        },
        // A destination that would be read as an ssh option, empty, or absurd.
        ControlRequest {
            session_id: Some(42),
            ssh_target: Some("-oProxyCommand=touch /tmp/pwned".to_owned()),
            ..request("token", "open_remote_session")
        },
        ControlRequest {
            session_id: Some(42),
            ssh_target: Some(String::new()),
            ..request("token", "open_remote_session")
        },
        ControlRequest {
            session_id: Some(42),
            ssh_target: Some("h".repeat(4097)),
            ..request("token", "open_remote_session")
        },
        ControlRequest {
            session_id: Some(42),
            ssh_target: Some("build-host".to_owned()),
            ssh_port: Some(0),
            ..request("token", "open_remote_session")
        },
        ControlRequest {
            session_id: Some(42),
            ssh_target: Some("build-host".to_owned()),
            secret: Some(String::new()),
            ..request("token", "open_remote_session")
        },
        // A field the command does not carry.
        ControlRequest {
            session_id: Some(42),
            ssh_target: Some("build-host".to_owned()),
            profile: Some("System".to_owned()),
            ..request("token", "open_remote_session")
        },
    ] {
        let mut invalid = invalid;
        assert_eq!(decode_control_request(&mut invalid, "token"), None);
    }

    // The destination fields belong to this command alone.
    let mut elsewhere = request("token", "open_window");
    elsewhere.ssh_target = Some("build-host".to_owned());
    assert_eq!(decode_control_request(&mut elsewhere, "token"), None);
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
