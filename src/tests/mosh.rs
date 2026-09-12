use super::*;

fn parse_mosh(arguments: &[&str]) -> MoshCommand {
    let arguments = arguments
        .iter()
        .map(|argument| OsString::from(*argument))
        .collect::<Vec<_>>();
    parse_mosh_args(&arguments).expect("valid Mosh arguments")
}

#[test]
fn mosh_parser_preserves_target_and_remote_command() {
    let command = parse_mosh(&[
        "--client",
        "/opt/zosh",
        "--server=mosh-server-custom",
        "--predict",
        "always",
        "--predict-overwrite",
        "-6",
        "--port",
        "60000:61000",
        "--bind-server",
        "203.0.113.9",
        "--ssh",
        "ssh -F 'config with spaces'",
        "--no-ssh-pty",
        "--no-init",
        "--experimental-remote-ip",
        "remote",
        "alice@[2001:db8::4]",
        "zsh",
        "-lc",
        "printf hello",
    ]);
    assert_eq!(command.client.as_deref(), Some("/opt/zosh"));
    assert_eq!(command.server, "mosh-server-custom");
    assert_eq!(command.prediction, PredictionMode::Always);
    assert_eq!(command.family, AddressFamily::Inet6);
    assert_eq!(
        command.port.as_ref().map(PortRequest::as_argument),
        Some("60000:61000".to_owned())
    );
    assert_eq!(command.ssh, ["ssh", "-F", "config with spaces"]);
    assert!(!command.ssh_pty);
    assert!(!command.init);
    assert_eq!(command.remote_ip, RemoteIpMode::Remote);
    assert_eq!(command.target.as_deref(), Some("alice@[2001:db8::4]"));
    assert_eq!(command.remote_command, ["zsh", "-lc", "printf hello"]);
}

#[test]
fn mosh_parser_supports_delimiter_help_and_rejects_duplicates() {
    let command = parse_mosh(&["--", "-host", "--help"]);
    assert_eq!(command.target.as_deref(), Some("-host"));
    assert_eq!(command.remote_command, ["--help"]);
    let command = parse_mosh(&["host", "--", "zsh", "-l"]);
    assert_eq!(command.remote_command, ["--", "zsh", "-l"]);
    assert!(
        parse_mosh_args(&[OsString::from("--help"), OsString::from("host")])
            .unwrap()
            .help
    );
    assert!(
        parse_mosh_args(&[
            OsString::from("--port"),
            OsString::from("60001"),
            OsString::from("--port"),
            OsString::from("60002"),
            OsString::from("host"),
        ])
        .is_err()
    );
    assert!(
        parse_mosh_args(&[
            OsString::from("--predict"),
            OsString::from("bad"),
            OsString::from("host"),
        ])
        .is_err()
    );
}

#[test]
fn mosh_parser_forwards_a_keep_alive_with_or_without_an_interval() {
    assert_eq!(parse_mosh(&["host"]).keep_alive, None, "off by default");
    assert_eq!(
        parse_mosh(&["-k", "host"]).keep_alive,
        Some(KEEP_ALIVE_DEFAULT_MS)
    );
    assert_eq!(
        parse_mosh(&["--keep-alive", "host"]).keep_alive,
        Some(KEEP_ALIVE_DEFAULT_MS)
    );
    assert_eq!(
        parse_mosh(&["--keep-alive=250", "host"]).keep_alive,
        Some(250)
    );
    assert_eq!(parse_mosh(&["-k=250", "host"]).keep_alive, Some(250));
    // A bare `-k` takes no separate value, so the target after it is
    // still the target.
    assert_eq!(parse_mosh(&["-k", "host"]).target.as_deref(), Some("host"));

    for rejected in [
        vec!["--keep-alive=5", "host"],
        vec!["--keep-alive=99999", "host"],
        vec!["--keep-alive=soon", "host"],
        vec!["-k", "--keep-alive=250", "host"],
    ] {
        let arguments = rejected.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(
            parse_mosh_args(&arguments).is_err(),
            "{rejected:?} must not parse"
        );
    }
}

#[test]
fn mosh_parser_accepts_aliases_and_terminal_modes() {
    let always = parse_mosh(&["-a", "host"]);
    assert_eq!(always.prediction, PredictionMode::Always);
    let never = parse_mosh(&["-n", "host"]);
    assert_eq!(never.prediction, PredictionMode::Never);

    let family = parse_mosh(&["--family=prefer-inet6", "host"]);
    assert_eq!(family.family, AddressFamily::PreferInet6);
    let local = parse_mosh(&["--local", "host"]);
    assert!(local.local);
    let initialized = parse_mosh(&["--ssh-pty", "--init", "host"]);
    assert!(initialized.ssh_pty);
    assert!(initialized.init);
    assert!(initialized.init_explicit);

    let help = parse_mosh_args(&[OsString::from("--help")]).unwrap();
    assert!(help.help);
    let version = parse_mosh_args(&[OsString::from("--version")]).unwrap();
    assert!(version.version);
}

#[test]
fn proxy_forwards_the_original_arguments_without_rewriting_them() {
    let command = MoshCommand {
        raw_arguments: vec![
            "--family=prefer-inet6".into(),
            "pi@adsb".into(),
            "--".into(),
            "zsh".into(),
            "-l".into(),
        ],
        ..MoshCommand::default()
    };
    assert_eq!(
        forwarded_arguments(&command),
        command.raw_arguments,
        "zetta mosh must not reinterpret arguments before zosh sees them"
    );
}

#[test]
fn proxy_reconstructs_help_and_version_for_directly_built_commands() {
    let help = MoshCommand {
        help: true,
        ..MoshCommand::default()
    };
    assert_eq!(forwarded_arguments(&help), vec![OsString::from("--help")]);

    let version = MoshCommand {
        version: true,
        ..MoshCommand::default()
    };
    assert_eq!(
        forwarded_arguments(&version),
        vec![OsString::from("--version")]
    );
}
