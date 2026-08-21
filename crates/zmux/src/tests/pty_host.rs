use super::*;

#[test]
fn the_host_protocol_is_additive_only() {
    // The host is the part that does not get upgraded: after a daemon replaces
    // itself it finds the *old* host still running, so the daemon must be able
    // to speak to a host at least as old as this.
    const { assert!(MINIMUM_HOST_PROTOCOL_VERSION <= HOST_PROTOCOL_VERSION) };
}

#[test]
fn host_requests_are_tagged_by_name_on_the_wire() {
    // Pinned, because the two sides of this protocol are deliberately of
    // different ages.
    let encoded = serde_json::to_value(HostRequest::List).unwrap();
    assert_eq!(encoded, serde_json::json!({"request": "list"}));

    let resize = serde_json::to_value(HostRequest::Resize {
        console_id: 3,
        columns: 80,
        lines: 24,
    })
    .unwrap();
    assert_eq!(resize["request"], "resize");
    assert_eq!(resize["console_id"], 3);
}

#[test]
fn a_host_envelope_from_a_newer_daemon_still_parses() {
    // The same reason as the daemon's own envelope: a version that cannot be
    // read cannot be reported.
    let wire = r#"{"version":99,"token":"a","target_process_id":7,
                   "request":{"request":"reap"},"added_later":true}"#;
    let parsed: HostEnvelope = serde_json::from_str(wire).expect("a newer envelope must parse");

    assert_eq!(parsed.version, 99);
    assert_eq!(parsed.target_process_id, 7);
}

#[test]
fn an_exit_is_held_until_a_daemon_collects_it() {
    // An exit that happens while the daemon is being replaced has to reach its
    // successor rather than being dropped on the floor.
    let exit = ConsoleExit {
        console_id: 1,
        child_pid: 42,
        exit_code: Some(3),
    };
    let wire = serde_json::to_string(&HostResponse::Exits { exits: vec![exit] }).unwrap();
    let parsed: HostResponse = serde_json::from_str(&wire).unwrap();

    match parsed {
        HostResponse::Exits { exits } => {
            assert_eq!(exits.len(), 1);
            assert_eq!(exits[0].exit_code, Some(3));
            assert_eq!(exits[0].child_pid, 42);
        }
        other => panic!("expected exits, got {other:?}"),
    }
}

#[test]
fn windows_shell_arguments_match_the_local_terminal_rules() {
    assert!(escape_windows_shell_args(Some("powershell.exe")));
    assert!(escape_windows_shell_args(Some(
        r"C:\Program Files\PowerShell\7\pwsh.exe"
    )));
    assert!(escape_windows_shell_args(Some("wsl.exe")));
    assert!(!escape_windows_shell_args(Some("cmd.exe")));
    assert!(!escape_windows_shell_args(Some("cmd.bat")));
    assert!(escape_windows_shell_args(None));
}
