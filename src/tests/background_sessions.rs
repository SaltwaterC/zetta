use super::*;

#[test]
fn reconnects_most_recently_detached_session_first() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach("build");
    runner.detach("server");

    assert_eq!(runner.len(), 2);
    assert_eq!(runner.reconnect_at(runner.len() - 1), Some("server"));
    assert_eq!(runner.reconnect_at(runner.len() - 1), Some("build"));
    assert_eq!(runner.reconnect_at(0), None);
}

#[test]
fn reconnects_a_selected_session_without_reordering_the_others() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach("build");
    runner.detach("server");
    runner.detach("editor");

    assert_eq!(runner.reconnect_at(1), Some("server"));
    assert_eq!(
        runner.iter().copied().collect::<Vec<_>>(),
        ["build", "editor"]
    );
    assert_eq!(runner.reconnect_at(2), None);
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

    let catalogs = read_session_catalogs(directory.path()).unwrap();
    assert_eq!(catalogs.len(), 1);
    assert_eq!(catalogs[0].sessions, vec![session]);
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
