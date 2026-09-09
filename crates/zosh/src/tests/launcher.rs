use super::*;

use std::io::Cursor;

fn args(values: &[&str]) -> Vec<std::ffi::OsString> {
    values.iter().map(std::ffi::OsString::from).collect()
}

#[test]
fn launcher_parser_accepts_the_stock_option_surface() {
    let command = parse_args(args(&[
        "--client=/opt/mosh-client",
        "--server",
        "/opt/mosh-server",
        "--predict",
        "experimental",
        "-o",
        "--family=prefer-inet6",
        "-p",
        "60000:61000",
        "--bind-server",
        "203.0.113.9",
        "--ssh",
        "ssh -F 'config with spaces'",
        "--no-ssh-pty",
        "--no-init",
        "--experimental-remote-ip",
        "remote",
        "adsb",
        "--",
        "zsh",
        "-lc",
        "printf hello",
    ]))
    .unwrap();
    assert_eq!(command.client.as_deref(), Some("/opt/mosh-client"));
    assert_eq!(command.server, "/opt/mosh-server");
    assert!(command.server_explicit);
    assert_eq!(command.prediction, PredictionMode::Experimental);
    assert!(command.predict_overwrite);
    assert_eq!(command.family, AddressFamily::PreferInet6);
    assert_eq!(
        command.port.as_ref().map(PortRequest::as_argument),
        Some("60000:61000".to_owned())
    );
    assert_eq!(
        command.bind_server,
        BindServer::Address("203.0.113.9".into())
    );
    assert_eq!(command.ssh, ["ssh", "-F", "config with spaces"]);
    assert!(!command.ssh_pty);
    assert!(!command.init);
    assert_eq!(command.remote_ip, RemoteIpMode::Remote);
    assert_eq!(command.target.as_deref(), Some("adsb"));
    assert_eq!(command.remote_command, ["--", "zsh", "-lc", "printf hello"]);
}

#[test]
fn launcher_parser_rejects_invalid_and_duplicate_options() {
    assert!(parse_args(args(&["--predict", "bad", "host"])).is_err());
    assert!(parse_args(args(&["--port", "61000:60000", "host"])).is_err());
    assert!(parse_args(args(&["--family", "invalid", "host"])).is_err());
    assert!(parse_args(args(&["--predict", "adaptive", "-a", "host"])).is_err());
    assert!(parse_args(args(&["--port", "60001", "--port", "60002", "host"])).is_err());
    assert!(parse_args(args(&["-k", "--keep-alive=250", "host"])).is_err());
    assert!(parse_args(args(&["--keep-alive=0", "host"])).is_err());
}

#[test]
fn the_launcher_takes_a_keep_alive_with_or_without_an_interval() {
    let keep_alive = |values: &[&str]| parse_args(args(values)).map(|command| command.keep_alive);
    assert_eq!(keep_alive(&["host"]).unwrap(), None, "off by default");
    assert_eq!(
        keep_alive(&["-k", "host"]).unwrap(),
        Some(KEEP_ALIVE_DEFAULT_MS)
    );
    assert_eq!(
        keep_alive(&["--keep-alive", "host"]).unwrap(),
        Some(KEEP_ALIVE_DEFAULT_MS)
    );
    assert_eq!(
        keep_alive(&["--keep-alive=250", "host"]).unwrap(),
        Some(250)
    );
    assert_eq!(keep_alive(&["-k=250", "host"]).unwrap(), Some(250));
    // The value is attached-only, so a bare `-k` before the target is
    // never mistaken for an option that swallowed the host.
    assert_eq!(
        parse_args(args(&["-k", "host"])).unwrap().target.as_deref(),
        Some("host")
    );
}

#[test]
fn the_keep_alive_reaches_the_bundled_endpoint_as_a_session_setting() {
    let settings = |values: &[&str]| endpoint_settings(&parse_args(args(values)).unwrap());
    assert_eq!(
        settings(&["--keep-alive=250", "host"]).keep_alive,
        Some(250)
    );
    assert_eq!(
        settings(&["-k", "host"]).keep_alive,
        Some(KEEP_ALIVE_DEFAULT_MS)
    );
    assert_eq!(settings(&["host"]).keep_alive, None);
}

#[test]
fn zosh_defaults_to_no_terminal_initialization() {
    let command = MoshCommand::default();
    assert!(!command.init);
    assert!(!command.init_explicit);
    assert!(parse_args(args(&["--init", "host"])).unwrap().init);
}

#[test]
fn launcher_help_and_version_take_precedence_over_a_target() {
    assert!(parse_args(args(&["--help", "host"])).unwrap().help);
    assert!(parse_args(args(&["--version", "host"])).unwrap().version);
    assert!(parse_args(args(&["--help", "--version"])).unwrap().help);
}

#[test]
fn launcher_parser_handles_proxy_delimiter_and_help_modes() {
    let proxy = parse_args(args(&[
        "--fake-proxy",
        "--family=inet6",
        "--",
        "adsb",
        "2222",
    ]))
    .unwrap();
    assert_eq!(proxy.family, AddressFamily::Inet6);
    assert_eq!(
        proxy.proxy,
        Some(ProxyRequest {
            host: "adsb".into(),
            port: 2222,
        })
    );
    assert!(parse_args(args(&["--help"])).unwrap().help);
    assert!(parse_args(args(&["--version"])).unwrap().version);
    assert!(parse_args(args(&["--help", "--version"])).unwrap().help);
}

#[test]
fn bootstrap_parser_extracts_endpoint_and_diagnostics() {
    let output =
        "starting server\nMOSH IP 203.0.113.7\nMOSH CONNECT 60001 AAAAAAAAAAAAAAAAAAAAAA\n";
    let endpoint = parse_bootstrap_output(output).unwrap();
    assert_eq!(endpoint.port, 60001);
    assert_eq!(endpoint.key, "AAAAAAAAAAAAAAAAAAAAAA");
    assert_eq!(endpoint.ip.as_deref(), Some("203.0.113.7"));
    assert_eq!(endpoint.diagnostics, vec!["starting server"]);
}

#[test]
fn bootstrap_parser_accepts_ssh_connection_address() {
    let endpoint = parse_bootstrap_output(
        "MOSH SSH_CONNECTION 192.0.2.2 54321 2001:db8::4 22\nMOSH CONNECT 60001 AAAAAAAAAAAAAAAAAAAAAA",
    )
    .unwrap();
    assert_eq!(endpoint.ip.as_deref(), Some("2001:db8::4"));
}

#[test]
fn bootstrap_parser_accepts_connect_before_proxy_address() {
    let endpoint =
        parse_bootstrap_output("MOSH CONNECT 60001 AAAAAAAAAAAAAAAAAAAAAA\nMOSH IP 192.0.2.10\n")
            .unwrap();
    assert_eq!(endpoint.ip.as_deref(), Some("192.0.2.10"));
}

#[test]
fn bootstrap_parser_rejects_malformed_connect() {
    let error = parse_bootstrap_output("MOSH CONNECT 60001 too-short").unwrap_err();
    assert!(error.to_string().contains("malformed MOSH CONNECT key"));
    assert!(parse_bootstrap_output("MOSH CONNECT 60001 AAAAAAAAAAAAAAAAAAAAAAB").is_err());
    assert!(
        parse_bootstrap_output(
            "MOSH IP 192.0.2.10\nMOSH IP 192.0.2.11\nMOSH CONNECT 60001 AAAAAAAAAAAAAAAAAAAAAA"
        )
        .is_err()
    );
}

#[test]
fn fallback_only_matches_unsupported_server_output() {
    assert!(is_unsupported_server_output(
        "bash: mosh-server: command not found"
    ));
    assert!(is_unsupported_server_output(
        "mosh-server: illegal option -- z"
    ));
    assert!(!is_unsupported_server_output("Permission denied"));
    assert!(!is_unsupported_server_output("Connection timed out"));
    assert!(is_unsupported_server_output_for(
        "/opt/mosh-server: command not found",
        "/opt/mosh-server"
    ));
    assert!(is_unsupported_server_output_for(
        "/opt/mosh-server: command not found",
        "env MOSH_DEBUG=1 /opt/mosh-server"
    ));
}

#[test]
fn port_and_server_arguments_preserve_ranges_and_command() {
    let port = parse_port_request("60000:61000").unwrap();
    assert_eq!(port.as_argument(), "60000:61000");
    let command = MoshCommand {
        port: Some(port),
        remote_command: vec!["zsh".into(), "-l".into()],
        ..MoshCommand::default()
    };
    let args = server_arguments(&command);
    assert!(args.windows(2).any(|pair| pair == ["-p", "60000:61000"]));
    assert!(args.ends_with(&["--".to_owned(), "zsh".to_owned(), "-l".to_owned()]));
    assert_eq!(parse_port_request("0").unwrap().as_argument(), "0");
    assert!(parse_port_request("60000:0").is_err());
}

#[test]
fn launcher_preserves_a_literal_separator_after_the_target() {
    let command = parse_args(args(&["host", "--", "zsh"])).unwrap();
    assert_eq!(command.remote_command, ["--", "zsh"]);
}

#[test]
fn local_server_arguments_skip_the_executable_name() {
    let command = MoshCommand {
        server: "/opt/mosh-server".to_owned(),
        ..MoshCommand::default()
    };
    let arguments = local_server_arguments(&command, terminal::color_count());
    assert_eq!(arguments.first(), Some(&"new".to_owned()));
    assert!(!arguments.contains(&"/opt/mosh-server".to_owned()));
}

#[test]
fn server_option_accepts_a_shell_command_with_arguments() {
    let command = parse_args(args(&[
        "--server",
        "env MOSH_DEBUG=1 /opt/mosh-server",
        "host",
    ]))
    .unwrap();
    let arguments = server_arguments(&command);
    assert_eq!(
        &arguments[..4],
        ["env", "MOSH_DEBUG=1", "/opt/mosh-server", "new"]
    );
}

#[test]
fn external_client_receives_the_stock_wrapper_argument() {
    let command = parse_args(args(&["--predict=never", "host", "zsh", "-l"])).unwrap();
    assert_eq!(
        external_client_arguments(&command, "192.0.2.1", 60001),
        [
            OsString::from("-#"),
            OsString::from("'--predict=never' 'host' 'zsh' '-l' |"),
            OsString::from("192.0.2.1"),
            OsString::from("60001"),
        ]
    );
}

#[test]
fn prediction_and_family_values_are_stable() {
    assert_eq!(PredictionMode::Always.as_env(), "always");
    assert_eq!(parse_family("inet6").unwrap(), AddressFamily::Inet6);
    assert_eq!(parse_remote_ip("remote").unwrap(), RemoteIpMode::Remote);
}

#[test]
fn auto_family_rejects_a_dual_stack_host() {
    let addresses = [
        "192.0.2.1:22".parse().unwrap(),
        "[2001:db8::1]:22".parse().unwrap(),
    ];
    assert!(select_socket_address(&addresses, AddressFamily::Auto).is_none());
    assert_eq!(
        select_socket_address(&addresses, AddressFamily::PreferInet),
        Some("192.0.2.1:22".parse().unwrap())
    );
    assert_eq!(
        ordered_socket_addresses(&addresses, AddressFamily::PreferInet6).unwrap(),
        vec![
            "[2001:db8::1]:22".parse().unwrap(),
            "192.0.2.1:22".parse().unwrap(),
        ]
    );
}

#[test]
fn ssh_bootstrap_preserves_target_and_remote_command() {
    let command = MoshCommand {
        remote_ip: RemoteIpMode::Remote,
        ssh_pty: false,
        remote_command: vec!["zsh".into(), "-lc".into(), "printf '$HOME'".into()],
        ..MoshCommand::default()
    };
    let (program, arguments) = ssh_bootstrap_command(&command, "alice@[2001:db8::4]");

    assert_eq!(program, "ssh");
    assert!(arguments.contains(&"-T".to_owned()));
    assert!(arguments.contains(&"alice@[2001:db8::4]".to_owned()));
    let target_index = arguments
        .iter()
        .position(|argument| argument == "alice@[2001:db8::4]")
        .expect("target");
    assert_eq!(arguments.get(target_index + 1), Some(&"--".to_owned()));
    let remote = arguments.get(target_index + 2).expect("remote command");
    assert!(remote.contains("MOSH SSH_CONNECTION"));
    assert!(remote.contains("'zsh' '-lc'"));
    assert!(remote.contains("printf"));
}

#[test]
fn default_remote_server_prefers_zosh_server_then_falls_back_to_stock_mosh() {
    let command = MoshCommand::default();
    let remote = remote_server_command(&command, 256).unwrap();

    assert!(remote.contains("if command -v zosh-server >/dev/null 2>&1"));
    let zosh = remote.find("'zosh-server' 'new'").expect("zosh invocation");
    let stock = remote
        .find("'mosh-server' 'new'")
        .expect("stock invocation");
    assert!(zosh < stock, "zosh-server must be the first candidate");
}

#[test]
fn an_explicit_server_command_is_not_replaced_by_default_selection() {
    let command = parse_args(args(&["--server=/opt/mosh-server", "host"])).unwrap();
    let remote = remote_server_command(&command, 256).unwrap();

    assert!(!remote.contains("command -v zosh-server"));
    assert!(remote.starts_with("'/opt/mosh-server' 'new' '-c' '256'"));
    assert!(remote.contains("'-s'"));
}

#[test]
fn ssh_bootstrap_preserves_an_ssh_config_alias_exactly() {
    let command = MoshCommand {
        remote_ip: RemoteIpMode::Proxy,
        ..MoshCommand::default()
    };
    let (_, arguments) = ssh_bootstrap_command(&command, "adsb");
    assert!(arguments.contains(&"adsb".to_owned()));
    assert!(!arguments.iter().any(|argument| argument == "192.168.0.45"));
}

#[test]
fn proxy_address_discovery_requires_the_ssh_proxy_report() {
    let command = MoshCommand::default();
    assert!(select_endpoint_host(&command, "adsb", None).is_err());
    assert_eq!(
        select_endpoint_host(&command, "adsb", Some("192.0.2.45")).unwrap(),
        "192.0.2.45"
    );
}

#[test]
fn proxy_bootstrap_uses_zosh_as_the_proxy() {
    let command = MoshCommand::default();
    let (_, arguments) = ssh_bootstrap_command(&command, "host");
    assert!(arguments.windows(2).any(|pair| pair == ["-S", "none"]));
    assert!(arguments.iter().any(|argument| {
        argument.contains("ProxyCommand=")
            && argument.contains(" --fake-proxy --family=prefer-inet -- %h %p")
    }));
}

#[test]
fn proxy_output_flushes_binary_data_after_the_ssh_banner() {
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    let mut reader = Cursor::new(b"SSH-2.0-server\r\n\0\0\x02\xa4\x06\x14".to_vec());
    let mut writer = Writer::default();
    assert_eq!(copy_proxy_output_to(&mut reader, &mut writer).unwrap(), 22);
    assert_eq!(writer.bytes, reader.get_ref().as_slice());
    assert_eq!(writer.flushes, 1);
}

#[test]
fn launcher_help_has_the_stock_mosh_surface() {
    let help = help_text();
    for option in [
        "--client",
        "--server",
        "--predict",
        "--predict-overwrite",
        "--family",
        "--port",
        "--bind-server",
        "--ssh",
        "--no-ssh-pty",
        "--no-init",
        "--local",
        "--experimental-remote-ip",
        "--keep-alive",
        "--help",
        "--version",
    ] {
        assert!(help.contains(option), "missing {option}");
    }
    assert!(
        help.contains("--keep-alive=MS"),
        "the interval form is offered"
    );
    assert!(
        help.contains(
            "--no-init               do not send terminal initialization string [default]"
        )
    );
    assert!(!help.contains("--init                  initialize the local terminal [default]"));
}
