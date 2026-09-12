use super::*;

fn request(secret: Option<&str>) -> PaneRequest {
    PaneRequest {
        target: zmux::remote::RemoteTarget::new("dev.example").with_port(Some(2222)),
        program: PathBuf::from("/home/user/.local/bin/zmux"),
        session_id: 7,
        keep_alive_ms: Some(500),
        secret: secret.map(str::to_owned),
    }
}

/// The command `zosh-server` runs is the contract with `zmux relay-pane`: the
/// multiplexer by absolute path, because a command started inside a Mosh
/// server has no `PATH` an SSH command would have given it, and the session
/// and pane it should attach.
#[test]
fn the_relay_command_names_the_remote_multiplexer_by_absolute_path() {
    assert_eq!(
        relay_command(&request(None), 42),
        vec![
            "/home/user/.local/bin/zmux".to_owned(),
            "relay-pane".to_owned(),
            "7".to_owned(),
            "42".to_owned(),
        ]
    );
}

/// A protected session's secret must never appear in the remote command line,
/// which every account on that host can read. All the command carries is the
/// instruction to expect one on stdin, inside the established Mosh link.
#[test]
fn a_protected_session_asks_for_its_secret_on_stdin_and_never_in_argv() {
    let command = relay_command(&request(Some("open-sesame")), 42);

    assert!(command.contains(&"--secret-stdin".to_owned()));
    assert!(
        !command.iter().any(|word| word.contains("open-sesame")),
        "the session secret must not reach the remote command line: {command:?}"
    );
}

/// The reason a fallback reports is the remote host's first word on the
/// subject, not a screen of shell startup noise.
#[test]
fn a_fallback_reason_quotes_the_first_thing_the_host_said() {
    assert_eq!(
        first_line("\n  \nbash: zosh-server: command not found\nmore\n"),
        "bash: zosh-server: command not found"
    );
    assert_eq!(first_line("   \n\n"), "the remote host said nothing");
}
