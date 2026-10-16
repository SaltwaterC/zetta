//! PowerShell's provider location must be visible to remote pane spawning.

use super::*;
use zmux::messages::{SessionRevision, SharedSpawnBatchRequest};

fn powershell_draft() -> SharedPaneDraft {
    SharedPaneDraft {
        draft_id: 1,
        profile: "Windows PowerShell".into(),
        command: Some(zetta_profiles::ProfileCommand::with_args(
            "powershell.exe",
            vec!["-NoLogo".into(), "-NoProfile".into()],
        )),
        env: HashMap::new(),
        working_directory: None,
        inherit_working_directory_from: None,
        load_shell_integration: true,
        size: TerminalSize {
            columns: 240,
            ..spawn_request().size
        },
        console_palette: ConsolePalette::default(),
        metadata: BackgroundPaneSummary {
            id: 0,
            label: "pane".into(),
            profile: "Windows PowerShell".into(),
            configured_command: String::new(),
            application: "powershell.exe".into(),
            foreground_command: None,
            terminal_title: None,
            working_directory: None,
            state: BackgroundPaneState::Starting,
            exit: None,
        },
    }
}

fn wait_for_output(pane: &zmux::client::SharedPane, expected: &str) {
    let mut output = pane.replay.clone();
    let mut reader = pane.reader();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if String::from_utf8_lossy(&output).contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "missing {expected:?}: {:?}",
            String::from_utf8_lossy(&output)
        );
        let mut buffer = [0; 4096];
        match reader.read(&mut buffer) {
            Ok(0) => panic!("pane closed: {:?}", String::from_utf8_lossy(&output)),
            Ok(length) => {
                output.extend_from_slice(&buffer[..length]);
                if buffer[..length].windows(3).any(|bytes| bytes == b"\x1b[c") {
                    pane.send_input(b"\x1b[?6c").unwrap();
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => panic!("reading the pane: {error}"),
        }
    }
}

#[test]
fn a_remote_powershell_split_inherits_the_directory_after_set_location() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let directory = tempfile::Builder::new()
        .prefix("zetta cwd ")
        .tempdir()
        .unwrap();
    let directory = directory.path().to_string_lossy().into_owned();
    let created = client
        .create_shared(CreateSharedRequest {
            operation_id: client.next_shared_operation_id(),
            title: "cwd".into(),
            replacement: SharedDraftLayout::Draft { draft_id: 1 },
            panes: vec![powershell_draft()],
            active_pane: Some(SharedPaneRef::Draft { draft_id: 1 }),
            verifier: None,
        })
        .unwrap();
    let session_id = created.session_id;
    let pane_id = created.state.operation_receipts[0].draft_mappings[0].pane_id;
    let AttachOutcome::SharedAttached { pane, .. } = client
        .attach_shared_with_secret(session_id, pane_id, None)
        .unwrap()
    else {
        panic!("source pane must attach shared")
    };
    wait_for_output(&pane, "\x1b]2;zetta-cwd:");
    pane.send_input(
        format!(
            "Set-Location -LiteralPath '{}'\r",
            directory.replace('\'', "''")
        )
        .as_bytes(),
    )
    .unwrap();
    // Wait for the prompt marker, not the echo of the Set-Location command.
    wait_for_output(&pane, &format!("\x1b]2;zetta-cwd:{directory}\x1b\\"));

    let mut split = powershell_draft();
    split.inherit_working_directory_from = Some(pane_id);
    let zmux::client::SharedBatchResult::Applied(spawned) = client
        .spawn_shared_batch(SharedSpawnBatchRequest {
            session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            target_pane_id: Some(pane_id),
            replacement: SharedDraftLayout::Split {
                axis: "vertical".into(),
                first_ratio: 500,
                first: Box::new(SharedDraftLayout::Existing { pane_id }),
                second: Box::new(SharedDraftLayout::Draft { draft_id: 1 }),
            },
            panes: vec![split],
            active_pane: Some(SharedPaneRef::Draft { draft_id: 1 }),
        })
        .unwrap()
    else {
        panic!("split conflicted")
    };
    let AttachOutcome::SharedAttached { pane: split, .. } = client
        .attach_shared_with_secret(session_id, spawned.mappings[0].pane_id, None)
        .unwrap()
    else {
        panic!("split must attach shared")
    };
    wait_for_output(&split, &format!("\x1b]2;zetta-cwd:{directory}\x1b\\"));
    client.kill(session_id).unwrap();
}
