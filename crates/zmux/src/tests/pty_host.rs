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

#[test]
fn colorref_uses_win32_bgr_packing() {
    assert_eq!(colorref([0x12, 0x34, 0x56]).0, 0x0056_3412);
}

#[test]
fn palette_attributes_preserve_unrelated_bits() {
    let palette = ConsolePalette {
        foreground_index: 3,
        background_index: 12,
        ..Default::default()
    };
    assert_eq!(palette_attributes(0xa55a, palette), 0xa5c3);
}

#[test]
fn screen_buffer_update_changes_only_palette_color_bits_and_window_convention() {
    use windows::Win32::System::Console::{
        CONSOLE_CHARACTER_ATTRIBUTES, CONSOLE_SCREEN_BUFFER_INFOEX,
    };

    let palette = ConsolePalette {
        colors: std::array::from_fn(|index| [index as u8, index as u8 + 1, index as u8 + 2]),
        foreground_index: 14,
        background_index: 1,
    };
    let mut info = CONSOLE_SCREEN_BUFFER_INFOEX::default();
    info.wAttributes = CONSOLE_CHARACTER_ATTRIBUTES(0x5a7c);
    info.wPopupAttributes = 0xa5c3;
    info.srWindow.Right = 79;
    info.srWindow.Bottom = 23;

    update_screen_buffer_info(&mut info, palette);

    assert_eq!(info.ColorTable[7].0, 0x0009_0807);
    assert_eq!(info.wAttributes.0, 0x5a1e);
    assert_eq!(info.wPopupAttributes, 0xa51e);
    assert_eq!(info.srWindow.Right, 80);
    assert_eq!(info.srWindow.Bottom, 24);
}
