use super::*;

#[test]
fn the_probe_tells_powershell_and_cmd_from_a_posix_shell() {
    // What each shell prints for PLATFORM_PROBE.
    assert_eq!(
        classify_probe(b"ZMUX_OS_A%OS%\r\nZMUX_OS_BWindows_NT\r\n"),
        HostPlatform::Windows
    );
    assert_eq!(
        classify_probe(b"ZMUX_OS_AWindows_NT ZMUX_OS_B$env:OS\r\n"),
        HostPlatform::Windows
    );
    assert_eq!(
        classify_probe(b"ZMUX_OS_A%OS% ZMUX_OS_B:OS\n"),
        HostPlatform::Posix
    );
    assert_eq!(classify_probe(b""), HostPlatform::Posix);
}

#[test]
fn a_learned_platform_belongs_to_the_destination_and_port() {
    let target = RemoteTarget::new("remote-host-learning-test").with_port(Some(2222));
    assert_eq!(learned(&target), None);
    learn(&target, HostPlatform::Windows);
    assert_eq!(
        learned(&target.clone().with_forward_agent(true)),
        Some(HostPlatform::Windows)
    );
    assert_eq!(learned(&target.clone().with_port(None)), None);
}

#[test]
fn windows_commands_are_encoded_powershell_that_finds_zmux_on_path() {
    for command in [program_command(), profiles_command(), bridge_command(false)] {
        assert!(
            command.starts_with("powershell.exe -NoProfile -NonInteractive -EncodedCommand "),
            "{command}"
        );
        let script = decoded(&command).unwrap();
        assert!(script.contains("Get-Command zmux.exe"), "{script}");
        assert!(!script.contains("/bin/sh"), "{script}");
    }
    assert!(
        decoded(&profiles_command())
            .unwrap()
            .ends_with("& $zmux profiles --json; exit $LASTEXITCODE")
    );
    assert!(
        decoded(&bridge_command(false))
            .unwrap()
            .ends_with("& $zmux proxy-mux; exit $LASTEXITCODE")
    );
    assert!(
        decoded(&bridge_command(true))
            .unwrap()
            .ends_with("& $zmux proxy-mux --forward-agent; exit $LASTEXITCODE")
    );
}

#[test]
fn the_windows_daemon_start_detaches_the_resolved_program() {
    let script = decoded(&start_daemon_command(Path::new(
        r"C:\Users\o'brien\AppData\Local\Programs\Zetta\zmux.exe",
    )))
    .unwrap();
    assert_eq!(
        script,
        r"& 'C:\Users\o''brien\AppData\Local\Programs\Zetta\zmux.exe' --daemon --detach; exit $LASTEXITCODE"
    );
}

#[test]
fn a_windows_program_path_must_be_absolute() {
    assert_eq!(
        parse_program_path(r"C:\Program Files\Zetta\zmux.exe").unwrap(),
        PathBuf::from(r"C:\Program Files\Zetta\zmux.exe")
    );
    assert!(parse_program_path(r"\\server\share\zmux.exe").is_ok());
    assert!(parse_program_path("zmux.exe").is_err());
    assert!(parse_program_path(r"C:zmux.exe").is_err());
    assert!(parse_program_path("/usr/bin/zmux").is_err());
}

/// Literals a remote program path could need: PowerShell closes a
/// single-quoted string at `'` and at each of U+2018–U+201B.
const POWERSHELL_LITERALS: &[&str] = &[
    "",
    "it's",
    r"C:\Users\O’Brien\AppData\Local\Programs\Zetta\zmux.exe",
    "‘’‚‛",
    "'‘’‚‛'",
    "''’’",
    "x’; Write-Output injected; ’",
    "$HOME `n \"double\"",
];

#[test]
fn every_powershell_single_quote_is_doubled() {
    assert_eq!(quote("it's"), "'it''s'");
    assert_eq!(quote("‘’‚‛"), "'‘‘’’‚‚‛‛'");
    assert_eq!(quote("‟"), "'‟'");
    let script = decoded(&start_daemon_command(Path::new(
        r"C:\Users\O’Brien\zmux.exe",
    )))
    .unwrap();
    assert!(
        script.starts_with(r"& 'C:\Users\O’’Brien\zmux.exe' "),
        "{script}"
    );
}

/// What each PowerShell engine on this host reads every literal as, decoding
/// [`encoded`]'s script as a remote host would. Windows PowerShell exists only
/// on Windows, and `pwsh` only where it is installed; a host with neither
/// returns nothing.
fn powershell_readings(literals: &[String]) -> Vec<(&'static str, Vec<String>)> {
    let script = literals
        .iter()
        .map(|literal| {
            format!(
                "[Console]::Out.WriteLine([Convert]::ToBase64String(\
                 [Text.Encoding]::UTF8.GetBytes({literal})))"
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let command = encoded(&script);
    let script = command.rsplit_once(' ').unwrap().1;
    ["powershell.exe", "pwsh"]
        .into_iter()
        .filter_map(|engine| {
            let output = std::process::Command::new(engine)
                .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", script])
                .output()
                .ok()?;
            assert!(
                output.status.success(),
                "{engine}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let readings = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| String::from_utf8(STANDARD.decode(line.trim()).unwrap()).unwrap())
                .collect();
            Some((engine, readings))
        })
        .collect()
}

#[test]
fn powershell_reads_each_quoted_literal_back_unchanged() {
    let quoted = POWERSHELL_LITERALS
        .iter()
        .map(|literal| quote(literal))
        .collect::<Vec<_>>();
    let readings = powershell_readings(&quoted);
    if readings.is_empty() {
        eprintln!("no PowerShell engine on this host; only the quoting itself was checked");
    }
    for (engine, read) in readings {
        assert_eq!(read, POWERSHELL_LITERALS, "{engine}");
    }
}
