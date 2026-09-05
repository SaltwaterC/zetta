use super::*;
use crate::pane::overlay_color_to_hex;
use futures::StreamExt as _;

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
        attention_id,
        ..Default::default()
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
fn control_server_delivers_a_token_authenticated_fresh_window_request() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    assert_eq!(endpoint.version, CONTROL_VERSION);

    let client = thread::spawn(move || {
        send_open_new_window_request_with_profile_and_token(&endpoint, "WSL: Ubuntu", "dock-token")
            .unwrap()
    });
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::OpenNewWindow {
        profile,
        activation_token,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(profile.as_deref(), Some("WSL: Ubuntu"));
    assert_eq!(activation_token.as_deref(), Some("dock-token"));
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
fn control_server_delivers_a_shell_command_and_reports_structured_rejection() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    let expected = ShellCommandRequest {
        command: "echo $FOO".to_owned(),
        arguments: vec!["two words".to_owned()],
        environment: BTreeMap::from([("FOO".to_owned(), "bar".to_owned())]),
    };
    let client_request = expected.clone();
    let client = thread::spawn(move || send_run_shell_command_request(&endpoint, &client_request));
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::RunShellCommand {
        request,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, expected);
    completion
        .send(Err("the active pane has no running base shell".to_owned()))
        .unwrap();
    let error = client.join().unwrap().unwrap_err().to_string();
    assert!(error.contains("shell_command_rejected"));
    assert!(error.contains("no running base shell"));
}

#[test]
fn control_server_delivers_an_open_command_with_the_callers_directory() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    let expected = PaneCommand {
        direction: None,
        label: None,
        pane: None,
        overlay: None,
        stack: false,
        list: false,
        command: vec!["pwsh".to_owned(), "-NoLogo".to_owned()],
    };
    let client_request = expected.clone();
    let working_directory = PathBuf::from("/caller/working directory");
    let client_working_directory = working_directory.clone();
    let client = thread::spawn(move || {
        send_open_command_request(
            &endpoint,
            &client_request,
            Some(client_working_directory.as_path()),
        )
    });
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::OpenCommand {
        request,
        working_directory: received_working_directory,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(request, expected);
    assert_eq!(received_working_directory, Some(working_directory));
    completion.send(true).unwrap();
    assert!(client.join().unwrap().unwrap());
}

#[test]
fn control_server_delivers_pane_label_listing() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();
    let client = thread::spawn(move || send_list_pane_labels_request(&endpoint, None));
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::ListPaneLabels {
        attention_id,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(attention_id, None);
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
fn control_client_continues_startup_when_fresh_window_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || send_open_new_window_request(&endpoint).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::OpenNewWindow { completion, .. } = command else {
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

    let client = thread::spawn(move || send_list_themes_request(&endpoint).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::ListThemes { completion } = command else {
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
#[cfg(feature = "syntax-highlighting")]
fn control_server_delivers_the_originating_pane_theme() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let client_endpoint = endpoint.clone();
    let client = thread::spawn(move || {
        send_get_pane_theme_request(&client_endpoint, 42, Some(9), None).unwrap()
    });
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::GetPaneTheme {
        attention_id,
        pane_id,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(attention_id, 42);
    assert_eq!(pane_id, Some(9));
    completion.send(Ok("One Dark".to_owned())).unwrap();
    let PaneThemeAnswer::Resolved { name, .. } = client.join().unwrap() else {
        panic!("a query with no known revision must be answered outright");
    };
    assert_eq!(name.as_deref(), Some("One Dark"));

    let legacy_client =
        thread::spawn(move || send_get_pane_theme_request(&endpoint, 42, None, None).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::GetPaneTheme {
        attention_id,
        pane_id,
        completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    assert_eq!(attention_id, 42);
    assert_eq!(pane_id, None);
    completion.send(Ok("One Dark".to_owned())).unwrap();
    let PaneThemeAnswer::Resolved { name, .. } = legacy_client.join().unwrap() else {
        panic!("a query with no known revision must be answered outright");
    };
    assert_eq!(name.as_deref(), Some("One Dark"));
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

/// The revision gate is what keeps an idle `zetta vi` from waking the thread
/// that draws twice a second, so the property worth pinning is that an
/// up-to-date client is answered *without any command reaching the main loop*.
///
/// The revision is process-wide and other tests run in parallel, so one of them
/// can bump between reading it here and the server reading it. That shows up as
/// a dispatched query rather than a wrong answer; the attempt is serviced and
/// retried rather than being allowed to fail the run intermittently.
#[test]
#[cfg(feature = "syntax-highlighting")]
fn a_current_revision_is_answered_without_reaching_the_main_loop() {
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    for _ in 0..16 {
        let current = crate::process_control::pane_theme_revision();
        let (finished, answers) = std::sync::mpsc::channel();
        let query_endpoint = endpoint.clone();
        thread::spawn(move || {
            let answer =
                send_get_pane_theme_request(&query_endpoint, 42, Some(9), Some(current)).unwrap();
            let _ = finished.send(answer);
        });

        // Either the connection thread answered on its own, or a concurrent
        // bump made this query stale and it dispatched. Waiting on both avoids
        // joining a client that is still blocked on a completion.
        let answer = loop {
            if let Ok(answer) = answers.try_recv() {
                break answer;
            }
            if let Ok(ProcessControlCommand::GetPaneTheme { completion, .. }) = received.try_recv()
            {
                completion.send(Ok("One Dark".to_owned())).unwrap();
            }
            thread::sleep(std::time::Duration::from_millis(1));
        };

        if matches!(answer, PaneThemeAnswer::Unchanged) {
            assert!(
                received.try_recv().is_err(),
                "an unchanged answer must not dispatch to the main loop"
            );
            return;
        }
    }
    panic!("a query at the current revision was never answered as unchanged");
}

/// The gate must not swallow a real change: once the revision moves, the same
/// client gets a full answer again.
#[test]
#[cfg(feature = "syntax-highlighting")]
fn a_stale_revision_is_dispatched_and_answered() {
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let stale = crate::process_control::pane_theme_revision();
    crate::process_control::bump_pane_theme_revision();
    assert_ne!(crate::process_control::pane_theme_revision(), stale);

    let client = thread::spawn(move || {
        send_get_pane_theme_request(&endpoint, 42, Some(9), Some(stale)).unwrap()
    });
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::GetPaneTheme { completion, .. } = command else {
        panic!("a stale revision must dispatch a pane theme query");
    };
    completion.send(Ok("One Dark".to_owned())).unwrap();

    let PaneThemeAnswer::Resolved { name, revision } = client.join().unwrap() else {
        panic!("a stale revision must be answered with the theme");
    };
    assert_eq!(name.as_deref(), Some("One Dark"));
    assert!(
        revision.is_some_and(|revision| revision > stale),
        "the answer must carry the revision it was resolved at"
    );
}
