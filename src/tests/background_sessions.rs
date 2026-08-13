use super::*;

#[test]
fn reconnects_most_recently_detached_session_first() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach("build", None);
    runner.detach("server", None);

    assert_eq!(runner.len(), 2);
    assert_eq!(runner.reconnect_at(runner.len() - 1), Some("server"));
    assert_eq!(runner.reconnect_at(runner.len() - 1), Some("build"));
    assert_eq!(runner.reconnect_at(0), None);
}

#[test]
fn reconnects_a_selected_session_without_reordering_the_others() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach("build", None);
    runner.detach("server", None);
    runner.detach("editor", None);

    assert_eq!(runner.reconnect_at(1), Some("server"));
    assert_eq!(
        runner.iter().copied().collect::<Vec<_>>(),
        ["build", "editor"]
    );
    assert_eq!(runner.reconnect_at(2), None);
}

#[test]
fn session_authentication_uses_a_salted_argon2id_verifier() {
    let first = SessionAuthentication::create("sensitive session").unwrap();
    let second = SessionAuthentication::create("sensitive session").unwrap();

    assert!(first.encoded().starts_with("$argon2id$"));
    assert!(!first.encoded().contains("sensitive session"));
    assert_ne!(first.encoded(), second.encoded());
    assert!(first.verify("sensitive session").is_some());
    assert!(first.verify("changed value").is_none());

    // Authorization is scoped to the session whose secret was checked.
    let authorization = first.verify("sensitive session").unwrap();
    assert!(first.authorizes(&authorization));
    assert!(!second.authorizes(&authorization));
}

#[test]
fn only_verifying_a_secret_produces_a_reconnect_authorization() {
    let authentication = SessionAuthentication::create("secret").unwrap();

    // A clone of the verifier is not itself authorization: `authorizes` takes a
    // `VerifiedSession`, and `verify` is the only way to construct one. This is
    // the invariant `take_background_session_by_id` relies on, so if a future
    // refactor reintroduces a public constructor this stops compiling.
    assert!(authentication.verify("wrong").is_none());
    let authorization = authentication
        .verify("secret")
        .expect("the correct secret must authorize");
    assert!(authentication.clone().authorizes(&authorization));
}

#[test]
fn failed_authentication_backoff_doubles_and_saturates() {
    assert_eq!(failed_authentication_delay(0), Duration::from_secs(1));
    assert_eq!(failed_authentication_delay(1), Duration::from_secs(1));
    assert_eq!(failed_authentication_delay(2), Duration::from_secs(2));
    assert_eq!(failed_authentication_delay(3), Duration::from_secs(4));
    assert_eq!(failed_authentication_delay(4), Duration::from_secs(8));
    assert_eq!(failed_authentication_delay(5), Duration::from_secs(16));
    // Capped, and no overflow at absurd failure counts.
    assert_eq!(failed_authentication_delay(6), Duration::from_secs(30));
    assert_eq!(failed_authentication_delay(64), Duration::from_secs(30));
    assert_eq!(
        failed_authentication_delay(u32::MAX),
        Duration::from_secs(30)
    );
}

#[test]
fn a_wrong_secret_opens_a_refusal_window_scoped_to_that_session() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach(
        "privileged",
        Some(SessionAuthentication::create("secret").unwrap()),
    );
    runner.detach(
        "other",
        Some(SessionAuthentication::create("secret").unwrap()),
    );

    assert!(!runner.authentication_is_refused_at(0));
    runner.record_failed_authentication_at(0);
    assert!(runner.authentication_is_refused_at(0));
    // Backoff is per session: guessing at one must not lock the user out of
    // another, and must not be shared state an attacker can pile onto.
    assert!(!runner.authentication_is_refused_at(1));

    // A correct secret clears the penalty.
    runner.clear_failed_authentications_at(0);
    assert!(!runner.authentication_is_refused_at(0));

    // An out-of-range index is inert rather than panicking.
    runner.record_failed_authentication_at(99);
    assert!(!runner.authentication_is_refused_at(99));
}

#[test]
fn session_secrets_are_not_rendered_by_debug() {
    let secret = SessionSecret::new("hunter2".to_owned());

    assert_eq!(format!("{secret:?}"), "SessionSecret(<redacted>)");
    assert!(!format!("{secret:?}").contains("hunter2"));
    assert_eq!(secret.expose(), "hunter2");
}

#[cfg(unix)]
#[test]
fn the_session_directory_is_private_to_the_current_user() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let sessions = directory.path().join("sessions");
    create_private_dir(&sessions).unwrap();

    let mode = fs::metadata(&sessions).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "session directory must not be shared");
}

#[test]
fn authentication_is_attached_only_to_the_selected_session() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach("ordinary", None);
    runner.detach(
        "sensitive",
        Some(SessionAuthentication::create("secret").unwrap()),
    );

    assert!(runner.authentication_at(0).is_none());
    assert!(
        runner
            .authentication_at(1)
            .unwrap()
            .verify("secret")
            .is_some()
    );
    assert_eq!(
        runner.iter().copied().collect::<Vec<_>>(),
        ["ordinary", "sensitive"]
    );
}

#[test]
fn catalog_round_trips_pane_process_details() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(format!("zetta-{}-9.json", std::process::id()));
    let mut publisher = SessionCatalogPublisher::at_path(path);
    let session = BackgroundSessionSummary {
        id: 7,
        title: "build".to_owned(),
        authentication_required: false,
        active_pane: 11,
        layout: BackgroundPaneLayout::Pane { pane_id: 11 },
        panes: vec![BackgroundPaneSummary {
            id: 11,
            label: "compiler".to_owned(),
            profile: "System".to_owned(),
            configured_command: "zsh -l".to_owned(),
            application: "cargo".to_owned(),
            foreground_command: Some(vec!["cargo".to_owned(), "test".to_owned()]),
            terminal_title: Some("cargo test".to_owned()),
            working_directory: Some(PathBuf::from("/work/zetta")),
            state: BackgroundPaneState::Failed,
            exit: Some(BackgroundPaneExit {
                source: BackgroundPaneExitSource::Child,
                reason: BackgroundPaneExitReason::ForegroundCommand,
                exit_code: Some(1),
                child_pid: Some(1234),
                input_sent: true,
                foreground_is_shell: Some(false),
                foreground_command: Some("htop".to_owned()),
            }),
        }],
    };
    publisher
        .publish(&BackgroundSessionCatalog {
            version: CATALOG_VERSION,
            process_id: std::process::id(),
            runner_id: 9,
            sessions: vec![session.clone()],
        })
        .unwrap();

    let published = fs::read_to_string(&publisher.path).unwrap();
    assert!(published.contains(r#""authentication_required": false"#));
    assert!(!published.contains("argon2id"));

    let catalogs = read_session_catalogs(directory.path()).unwrap();
    assert_eq!(catalogs.len(), 1);
    assert_eq!(catalogs[0].sessions, vec![session]);
}

#[test]
fn legacy_catalogs_are_ignored_after_the_schema_bump() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(format!("zetta-{}-legacy.json", std::process::id()));
    let legacy = r#"{
                "version": 3,
                "process_id": PROCESS_ID,
                "runner_id": 9,
                "sessions": [{
                    "id": 1,
                    "title": "old",
                    "authentication_required": false,
                    "active_pane": 1,
                    "layout": {"type": "pane", "pane_id": 1},
                    "panes": [{
                        "id": 1,
                        "label": "shell",
                        "profile": "System",
                        "configured_command": "powershell",
                        "application": "powershell",
                        "foreground_command": null,
                        "terminal_title": null,
                        "working_directory": null,
                        "state": "running"
                    }]
                }]
            }"#
    .replace("PROCESS_ID", &std::process::id().to_string());
    fs::write(&path, legacy).unwrap();

    assert!(read_session_catalogs(directory.path()).unwrap().is_empty());
}

#[test]
fn unexpected_exit_metadata_is_sanitized_and_actionable() {
    let event = TerminalExited {
        exit_code: Some(1),
        source: TerminalExitSource::Child,
        child_pid: Some(77),
        input_sent: true,
        foreground_is_shell: Some(false),
        foreground_command: Some("htop".to_owned()),
    };
    let exit = BackgroundPaneExit::from_terminal(&event).unwrap();
    assert_eq!(exit.reason, BackgroundPaneExitReason::ForegroundCommand);
    assert_eq!(exit.foreground_command.as_deref(), Some("htop"));
    assert!(exit.reason_text().contains("htop"));
    assert!(exit.reason_text().contains("child PID 77"));

    let unsafe_event = TerminalExited {
        foreground_command: Some("sh -c secret=value".to_owned()),
        ..event
    };
    let sanitized = BackgroundPaneExit::from_terminal(&unsafe_event).unwrap();
    assert_eq!(sanitized.foreground_command, None);
    assert!(!sanitized.reason_text().contains("secret=value"));

    let unknown_status = TerminalExited {
        exit_code: None,
        ..unsafe_event
    };
    assert_eq!(
        BackgroundPaneExit::from_terminal(&unknown_status)
            .unwrap()
            .reason,
        BackgroundPaneExitReason::StatusUnavailable
    );
}

#[test]
fn protected_catalog_entries_do_not_publish_session_details_or_verifiers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(format!("zetta-{}-12.json", std::process::id()));
    let mut runner = BackgroundSessionRunner {
        sessions: Vec::<DetachedSession<()>>::new(),
        catalog: SessionCatalogPublisher::at_path(path.clone()),
    };
    runner
        .publish(vec![BackgroundSessionSummary {
            id: 9,
            title: "customer production database".to_owned(),
            authentication_required: true,
            active_pane: 4,
            layout: BackgroundPaneLayout::Pane { pane_id: 4 },
            panes: vec![BackgroundPaneSummary {
                id: 4,
                label: "database password reset".to_owned(),
                profile: "System".to_owned(),
                configured_command: "sensitive-command".to_owned(),
                application: "psql".to_owned(),
                foreground_command: None,
                terminal_title: None,
                working_directory: None,
                state: BackgroundPaneState::Running,
                exit: None,
            }],
        }])
        .unwrap();

    let published = fs::read_to_string(path).unwrap();
    assert!(published.contains("Protected session"));
    assert!(published.contains(r#""authentication_required": true"#));
    assert!(!published.contains("customer production database"));
    assert!(!published.contains("sensitive-command"));
    assert!(!published.contains("argon2id"));
}

#[test]
fn empty_catalog_removes_the_published_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("zetta-test-3.json");
    let mut publisher = SessionCatalogPublisher::at_path(path.clone());
    publisher
        .publish(&BackgroundSessionCatalog {
            version: CATALOG_VERSION,
            process_id: std::process::id(),
            runner_id: 3,
            sessions: vec![BackgroundSessionSummary {
                id: 1,
                title: "shell".to_owned(),
                authentication_required: false,
                active_pane: 1,
                layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
            }],
        })
        .unwrap();
    assert!(path.is_file());

    publisher
        .publish(&BackgroundSessionCatalog {
            version: CATALOG_VERSION,
            process_id: std::process::id(),
            runner_id: 3,
            sessions: Vec::new(),
        })
        .unwrap();
    assert!(!path.exists());
}

#[test]
fn human_output_escapes_terminal_control_characters() {
    assert_eq!(display_text("cargo\n\u{1b}[31m ✓"), "cargo\\n\\u{1b}[31m ✓");
}

#[test]
fn command_lines_make_argument_boundaries_visible() {
    assert_eq!(
        display_command(&["cargo".to_owned(), "test name".to_owned()]),
        "cargo \"test name\""
    );
}

#[test]
fn control_endpoint_files_are_not_parsed_as_session_catalogs() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("control-123.json"),
        r#"{"version":1,"address":"127.0.0.1:1"}"#,
    )
    .unwrap();

    assert!(read_session_catalogs(directory.path()).unwrap().is_empty());
}

#[test]
fn application_name_comes_from_the_same_argv_as_the_command_line() {
    let command = vec!["nano".to_owned(), "notes.txt".to_owned()];
    assert_eq!(
        application_from_command_line(Some(&command)),
        Some("nano".to_owned())
    );
    assert_eq!(
        application_from_command_line(Some(&["C:\\Tools\\vim.exe".to_owned()])),
        Some("vim.exe".to_owned())
    );
}
