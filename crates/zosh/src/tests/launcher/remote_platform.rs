use super::*;

#[cfg(unix)]
#[test]
fn shell_detection_does_not_require_the_address_discovery_proxy() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/launcher/remote_platform_probe.sh");
    for (name, expected) in [
        ("windows", RemotePlatform::Windows),
        ("posix", RemotePlatform::Posix),
    ] {
        let command = MoshCommand {
            ssh: vec![
                "/bin/sh".to_owned(),
                fixture.to_string_lossy().into_owned(),
                name.to_owned(),
            ],
            ..MoshCommand::default()
        };
        assert_eq!(command.remote_ip, RemoteIpMode::Proxy);
        assert_eq!(probe(&command, "host").unwrap(), expected);
    }
}

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
        .as_chunks::<2>()
        .0
        .iter()
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

/// A Windows host's panes: the embedder says the host is Windows, so the pane
/// is bootstrapped through PowerShell with no probe of its own, and over the
/// embedder's login when it has one — as a POSIX host's panes are.
#[test]
fn an_embedded_windows_pane_is_bootstrapped_over_the_shared_login() {
    let request = PaneBootstrapRequest {
        target: "thinkpad".to_owned(),
        remote_command: vec![
            r"C:\Users\dev\AppData\Local\Programs\Zetta\zmux.exe".to_owned(),
            "relay-pane".to_owned(),
            "1".to_owned(),
            "2".to_owned(),
            "--viewer-stdin".to_owned(),
        ],
        control_path: Some(PathBuf::from("/tmp/zetta-zmux-x/ctl")),
        windows_host: true,
        ..PaneBootstrapRequest::default()
    };
    let mut command = embedded_command(&request).unwrap();
    assert!(command.embedded && command.windows_host);
    command.control_path = request.control_path.clone();
    command.remote_ip = RemoteIpMode::Local;

    let (_, arguments) = windows_bootstrap_command(&command, "thinkpad", 256).unwrap();
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["-S", "/tmp/zetta-zmux-x/ctl"]),
        "{arguments:?}"
    );
    assert!(arguments.contains(&"ControlMaster=no".to_owned()));
    let script = windows_remote_script(&command, 256).unwrap();
    assert!(script.contains("Get-Command zosh-server.exe"), "{script}");
    assert!(script.contains("'relay-pane'"), "{script}");
    assert!(
        script.contains(r"'C:\Users\dev\AppData\Local\Programs\Zetta\zmux.exe'"),
        "{script}"
    );
}

/// Literals a server path or remote command could need: PowerShell closes a
/// single-quoted string at `'` and at each of U+2018–U+201B.
const POWERSHELL_LITERALS: &[&str] = &[
    "",
    "it's",
    r"C:\Users\O’Brien\AppData\Local\Programs\Zetta\zosh-server.exe",
    "‘’‚‛",
    "'‘’‚‛'",
    "''’’",
    "x’; Write-Output injected; ’",
    "$HOME `n \"double\"",
];

#[test]
fn every_powershell_single_quote_is_doubled() {
    assert_eq!(powershell_quote("it's"), "'it''s'");
    assert_eq!(powershell_quote("‘’‚‛"), "'‘‘’’‚‚‛‛'");
    assert_eq!(powershell_quote("‟"), "'‟'");
    let command = MoshCommand {
        server: r"C:\Users\O’Brien\zosh-server.exe".to_owned(),
        server_explicit: true,
        remote_command: vec!["echo".to_owned(), "it’s".to_owned()],
        ..MoshCommand::default()
    };
    let script = windows_remote_script(&command, 256).unwrap();
    assert!(
        script.contains(r"'C:\Users\O’’Brien\zosh-server.exe'"),
        "{script}"
    );
    assert!(script.contains("'it’’s'"), "{script}");
}

/// Runs `script` on each PowerShell engine on this host, encoded as
/// [`windows_remote_command`] encodes it, and returns what each printed.
/// Windows PowerShell exists only on Windows, and `pwsh` only where it is
/// installed; a host with neither returns nothing.
fn powershell_outputs(script: &str) -> Vec<(&'static str, String)> {
    let encoded = STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    ["powershell.exe", "pwsh"]
        .into_iter()
        .filter_map(|engine| {
            let output = Command::new(engine)
                .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
                .output()
                .ok()?;
            assert!(
                output.status.success(),
                "{engine}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Some((engine, String::from_utf8(output.stdout).unwrap()))
        })
        .collect()
}

#[test]
fn powershell_reads_each_quoted_argument_back_unchanged() {
    let values = POWERSHELL_LITERALS
        .iter()
        .map(|literal| (*literal).to_owned())
        .collect::<Vec<_>>();
    // Each value is printed as base64, which no console code page can mangle.
    let script = format!(
        "foreach ($value in {}) {{ [Console]::Out.WriteLine(\
         [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($value))) }}",
        powershell_array(&values)
    );
    let outputs = powershell_outputs(&script);
    if outputs.is_empty() {
        eprintln!("no PowerShell engine on this host; only the quoting itself was checked");
    }
    for (engine, output) in outputs {
        let read = output
            .lines()
            .map(|line| String::from_utf8(STANDARD.decode(line.trim()).unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(read, POWERSHELL_LITERALS, "{engine}");
    }
}
