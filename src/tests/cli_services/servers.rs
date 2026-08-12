use std::ffi::OsString;
use std::path::PathBuf;

use super::*;
use crate::cli_services::CliServiceCommand;

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
            writable: false,
        })
    );
}

#[cfg(feature = "tftp-server")]
#[test]
fn tftp_server_uploads_require_an_explicit_opt_in() {
    // Anonymous uploads are the whole exposure, so the default must be off and
    // both spellings of the opt-in must reach the server.
    let read_only =
        parse_tftp_server_args([OsString::from("-r"), OsString::from("images")]).unwrap();
    assert_eq!(
        read_only,
        CliServiceCommand::Tftp(TftpServerCommand {
            root: PathBuf::from("images"),
            port: None,
            config_path: None,
            writable: false,
        })
    );

    for flag in ["--writable", "-w"] {
        assert_eq!(
            parse_tftp_server_args([
                OsString::from(flag),
                OsString::from("-r"),
                OsString::from("images"),
            ])
            .unwrap(),
            CliServiceCommand::Tftp(TftpServerCommand {
                root: PathBuf::from("images"),
                port: None,
                config_path: None,
                writable: true,
            })
        );
    }

    let repeated =
        parse_tftp_server_args([OsString::from("--writable"), OsString::from("-w")]).unwrap_err();
    assert!(
        repeated
            .to_string()
            .contains("--writable may only be specified once")
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
        writable: false,
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
