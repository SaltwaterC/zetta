use super::*;

#[test]
fn probe_classifies_both_native_windows_shells() {
    assert_eq!(
        classify_probe("ZOSH_OS_AWindows_NT ZOSH_OS_B$env:OS\n"),
        RemotePlatform::Windows
    );
    assert_eq!(
        classify_probe("ZOSH_OS_A%OS%\nZOSH_OS_BWindows_NT\n"),
        RemotePlatform::Windows
    );
    assert_eq!(
        classify_probe("ZOSH_OS_A%OS% ZOSH_OS_B:OS\n"),
        RemotePlatform::Posix
    );
}

#[test]
fn windows_command_passes_server_arguments_without_shell_interpolation() {
    let command = MoshCommand {
        server: r"C:\Program Files\Zetta\zosh-server.exe".to_owned(),
        server_explicit: true,
        forward_agent: true,
        remote_command: vec!["echo".to_owned(), "it's ready; $HOME".to_owned()],
        ..MoshCommand::default()
    };
    let script = windows_remote_script(&command, 256).unwrap();
    assert!(script.contains("'C:\\Program Files\\Zetta\\zosh-server.exe'"));
    assert!(script.contains("'--forward-agent'"));
    assert!(script.contains("'it''s ready; $HOME'"));
    let remote = windows_remote_command(&command, 256).unwrap();
    assert!(remote.starts_with("powershell.exe -NoProfile -NonInteractive -EncodedCommand "));
    let encoded = remote.rsplit_once(' ').unwrap().1;
    let bytes = STANDARD.decode(encoded).unwrap();
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    assert_eq!(String::from_utf16(&units).unwrap(), script);
}

#[test]
fn windows_bootstrap_disables_ssh_pty() {
    let command = MoshCommand::default();
    let (_, arguments) = windows_bootstrap_command(&command, "windows-host", 256).unwrap();
    assert!(arguments.contains(&"-T".to_owned()));
    assert!(!arguments.contains(&"-tt".to_owned()));
    assert!(!arguments.contains(&"-n".to_owned()));
}

#[test]
fn default_windows_server_falls_back_to_stock_without_forwarding_flag() {
    let command = MoshCommand {
        forward_agent: true,
        ..MoshCommand::default()
    };
    let script = windows_remote_script(&command, 256).unwrap();
    assert!(script.contains("Get-Command zosh-server.exe"));
    assert!(script.contains("Get-Command mosh-server.exe"));
    let stock = script.split("$arguments = ").nth(2).unwrap();
    assert!(
        !stock
            .split(" }; ")
            .next()
            .unwrap()
            .contains("--forward-agent")
    );
}

#[cfg(windows)]
#[test]
fn encoded_windows_bootstrap_runs_under_cmd_and_powershell() {
    let command = MoshCommand {
        server: "cmd.exe /d /c echo ZOSH_WINDOWS_BOOTSTRAP".to_owned(),
        server_explicit: true,
        ..MoshCommand::default()
    };
    let remote = windows_remote_command(&command, 256).unwrap();
    for (shell, options) in [
        ("cmd.exe", vec!["/d", "/s", "/c"]),
        (
            "powershell.exe",
            vec!["-NoProfile", "-NonInteractive", "-Command"],
        ),
    ] {
        let output = Command::new(shell)
            .args(options)
            .arg(&remote)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("ZOSH_WINDOWS_BOOTSTRAP new"),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
