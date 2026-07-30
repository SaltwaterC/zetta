use super::*;

#[cfg(feature = "serial-console")]
#[test]
fn serial_console_parser_uses_the_panel_defaults_and_accepts_all_settings() {
    let defaults = parse_serial_args([
        OsString::from("console"),
        OsString::from("--device"),
        OsString::from("/dev/ttyUSB0"),
    ])
    .unwrap();
    assert_eq!(
        defaults,
        CliServiceCommand::Serial(SerialCommand::Connect(SerialConnectionOptions {
            device: "/dev/ttyUSB0".to_owned(),
            baud_rate: 115_200,
            data_bits: 8,
            parity: SerialParity::None,
            stop_bits: 1,
            flow_control: SerialFlowControl::None,
        }))
    );

    let configured = parse_serial_args([
        OsString::from("console"),
        OsString::from("--device"),
        OsString::from("COM3"),
        OsString::from("--baud-rate"),
        OsString::from("9600"),
        OsString::from("--data-bits"),
        OsString::from("7"),
        OsString::from("--parity"),
        OsString::from("even"),
        OsString::from("--stop-bits"),
        OsString::from("2"),
        OsString::from("--flow-control"),
        OsString::from("hardware"),
    ])
    .unwrap();
    assert_eq!(
        configured,
        CliServiceCommand::Serial(SerialCommand::Connect(SerialConnectionOptions {
            device: "COM3".to_owned(),
            baud_rate: 9600,
            data_bits: 7,
            parity: SerialParity::Even,
            stop_bits: 2,
            flow_control: SerialFlowControl::Hardware,
        }))
    );

    let shorthand = parse_serial_args([
        OsString::from("console"),
        OsString::from("-d"),
        OsString::from("COM4"),
        OsString::from("-b"),
        OsString::from("57600"),
        OsString::from("-D"),
        OsString::from("7"),
        OsString::from("-p"),
        OsString::from("odd"),
        OsString::from("-s"),
        OsString::from("2"),
        OsString::from("-f"),
        OsString::from("software"),
    ])
    .unwrap();
    assert_eq!(
        shorthand,
        CliServiceCommand::Serial(SerialCommand::Connect(SerialConnectionOptions {
            device: "COM4".to_owned(),
            baud_rate: 57_600,
            data_bits: 7,
            parity: SerialParity::Odd,
            stop_bits: 2,
            flow_control: SerialFlowControl::Software,
        }))
    );
}

#[cfg(feature = "serial-console")]
#[test]
fn serial_list_and_invalid_console_options_are_validated() {
    assert_eq!(
        parse_serial_args([OsString::from("list")]).unwrap(),
        CliServiceCommand::Serial(SerialCommand::List)
    );
    assert!(parse_serial_args([OsString::from("console")]).is_err());
    assert!(
        parse_serial_args([
            OsString::from("console"),
            OsString::from("--device"),
            OsString::from("ttyS0"),
            OsString::from("--data-bits"),
            OsString::from("9"),
        ])
        .is_err()
    );
}

#[cfg(feature = "http-server")]
#[test]
fn http_server_parser_accepts_root_port_and_configuration_file() {
    assert_eq!(
        parse_http_args([
            OsString::from("server"),
            OsString::from("--root"),
            OsString::from("firmware"),
            OsString::from("--port"),
            OsString::from("8080"),
            OsString::from("--config"),
            OsString::from("zetta.json"),
        ])
        .unwrap(),
        CliServiceCommand::Http(HttpServerCommand {
            root: PathBuf::from("firmware"),
            port: Some(8080),
            config_path: Some(PathBuf::from("zetta.json")),
        })
    );
    assert!(
        parse_http_args([
            OsString::from("server"),
            OsString::from("--port"),
            OsString::from("0")
        ])
        .is_err()
    );
}

#[cfg(feature = "http-server")]
#[test]
fn http_server_uses_the_configured_port_unless_the_cli_overrides_it() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    std::fs::write(&config_path, r#"{"http_server_port":8081}"#).unwrap();
    let configured = HttpServerCommand {
        root: PathBuf::from("."),
        port: None,
        config_path: Some(config_path.clone()),
    };
    assert_eq!(configured.resolved_port().unwrap(), 8081);
    assert_eq!(
        HttpServerCommand {
            port: Some(8082),
            ..configured
        }
        .resolved_port()
        .unwrap(),
        8082
    );
}

#[cfg(feature = "tftp-server")]
#[test]
fn tftp_server_parser_accepts_root_and_port() {
    assert_eq!(
        parse_tftp_server_args([
            OsString::from("-r"),
            OsString::from("images"),
            OsString::from("-p"),
            OsString::from("1069"),
            OsString::from("-c"),
            OsString::from("zetta.json"),
        ])
        .unwrap(),
        CliServiceCommand::Tftp(TftpServerCommand {
            root: PathBuf::from("images"),
            port: Some(1069),
            config_path: Some(PathBuf::from("zetta.json")),
        })
    );
}

#[cfg(feature = "tftp-server")]
#[test]
fn tftp_server_uses_the_configured_port_unless_the_cli_overrides_it() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    std::fs::write(&config_path, r#"{"tftp_server_port":1069}"#).unwrap();
    let configured = TftpServerCommand {
        root: PathBuf::from("."),
        port: None,
        config_path: Some(config_path.clone()),
    };
    assert_eq!(configured.resolved_port().unwrap(), 1069);
    assert_eq!(
        TftpServerCommand {
            port: Some(1070),
            ..configured
        }
        .resolved_port()
        .unwrap(),
        1070
    );
}
