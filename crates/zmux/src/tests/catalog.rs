use super::*;
use crate::protocol::{
    BackgroundPaneExit, BackgroundPaneExitReason, BackgroundPaneExitSource, BackgroundPaneState,
    BackgroundPaneSummary,
};
use std::path::PathBuf;

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
        held: false,
        scoped_to: None,
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
fn protected_catalog_entries_do_not_publish_session_details_or_verifiers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(format!("zetta-{}-12.json", std::process::id()));
    let mut publisher = SessionCatalogPublisher::at_path(path.clone());
    publisher
        .publish_sessions(vec![BackgroundSessionSummary {
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
            held: false,
            scoped_to: None,
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
                held: false,
                scoped_to: None,
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
