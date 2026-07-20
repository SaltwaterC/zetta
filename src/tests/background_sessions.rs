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
    assert!(first.is_same_verifier(&first.clone()));
    assert!(!first.is_same_verifier(&second));
    assert!(first.verify("sensitive session"));
    assert!(!first.verify("changed value"));
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
    assert!(runner.authentication_at(1).unwrap().verify("secret"));
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
            state: BackgroundPaneState::Running,
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
