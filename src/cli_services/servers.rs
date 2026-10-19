use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::startup::format_help_table;

use super::CliServiceCommand;

#[cfg(feature = "http-server")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HttpServerCommand {
    root: PathBuf,
    port: Option<u16>,
    config_path: Option<PathBuf>,
    options: crate::HttpServerOptions,
}

#[cfg(feature = "http-server")]
pub(crate) fn http_server_help() -> String {
    format!(
        "Serve static files over HTTP\n\nUsage: zetta http server [OPTIONS]\n\nOptions:\n{}\n\nThe server has no authentication: it answers any host that can reach the\nport, which is how it hands files to devices on the LAN. It refuses names\nwith a dot-prefixed component (.git, .env) unless --serve-dotfiles is given.\nIt answers only requests whose Host header is absent or names this machine:\nthe address the request arrived on, a loopback address, localhost, this\nmachine's host name or <hostname>.local, or an --allow-host name. {limits}\n\nPress Ctrl-C to stop the server.",
        format_help_table([
            (
                "-r, --root PATH",
                "Directory to serve (default: current directory)",
            ),
            (
                "-p, --port PORT",
                "TCP port (default: http_server_port from configuration)",
            ),
            (
                "-c, --config PATH",
                "Read the HTTP port default from this configuration file",
            ),
            (
                "-d, --serve-dotfiles",
                "Serve names with a dot-prefixed component, such as .git",
            ),
            (
                "-a, --allow-host NAME",
                "Also answer this host name or address (repeatable)",
            ),
            (
                "-A, --allow-any-host",
                "Answer requests whatever their Host header names",
            ),
            ("-h, --help", "Print help"),
        ]),
        limits = crate::http_server_limits_help(),
    )
}

#[cfg(feature = "http-server")]
pub(crate) fn parse_http_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        anyhow::bail!("{}", http_server_help());
    }
    anyhow::ensure!(
        args.first().is_some_and(|argument| argument == "server"),
        "usage: zetta http server [OPTIONS]; run `zetta http server --help` for details"
    );
    let (options, remaining) = zhttp::cli::parse_http_server_flags(&args[1..])?;
    let (root, port, config_path) = parse_server_options(&remaining, "HTTP")?;
    Ok(CliServiceCommand::Http(HttpServerCommand {
        root,
        port,
        config_path,
        options,
    }))
}

#[cfg(feature = "http-server")]
impl HttpServerCommand {
    fn resolved_port(&self) -> Result<u16> {
        Ok(match self.port {
            Some(port) => port,
            None => {
                crate::Config::load(self.config_path.as_deref(), None)
                    .context("loading configuration for the HTTP server")?
                    .http_server_port
            }
        })
    }

    fn proxy_args(&self) -> Result<Vec<OsString>> {
        let mut args = vec![
            OsString::from("server"),
            OsString::from("--root"),
            self.root.as_os_str().to_owned(),
            OsString::from("--port"),
            OsString::from(self.resolved_port()?.to_string()),
        ];
        if self.options.serve_dotfiles {
            args.push(OsString::from("--serve-dotfiles"));
        }
        for host in &self.options.allowed_hosts {
            args.extend([OsString::from("--allow-host"), OsString::from(host)]);
        }
        if self.options.allow_any_host {
            args.push(OsString::from("--allow-any-host"));
        }
        Ok(args)
    }

    pub(super) fn run(&self) -> Result<()> {
        crate::http_server::run_http_proxy(&self.proxy_args()?)
    }
}

#[cfg(feature = "tftp-server")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TftpServerCommand {
    root: PathBuf,
    port: Option<u16>,
    config_path: Option<PathBuf>,
    writable: bool,
}

#[cfg(feature = "tftp-server")]
pub(crate) fn tftp_server_help() -> String {
    format!(
        "Serve files with TFTP\n\nUsage: zetta tftp server [OPTIONS]\n\nOptions:\n{}\n\nTFTP has no authentication: the server answers any host that can reach the\nport, and --writable lets those hosts create files under --root. Existing\nfiles are never overwritten, and names with a dot-prefixed component are\nrefused. Uploads are off unless --writable is given; point a writable server\nat a directory that holds only transfers.\n\nPress Ctrl-C to stop the server.",
        format_help_table([
            (
                "-r, --root PATH",
                "Directory to serve (default: current directory)",
            ),
            (
                "-p, --port PORT",
                "UDP port (default: tftp_server_port from configuration)",
            ),
            (
                "-c, --config PATH",
                "Read the TFTP port default from this configuration file",
            ),
            ("-w, --writable", "Accept uploads into the served directory"),
            ("-h, --help", "Print help"),
        ])
    )
}

#[cfg(feature = "tftp-server")]
pub(crate) fn parse_tftp_server_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        anyhow::bail!("{}", tftp_server_help());
    }
    let mut writable = false;
    let mut remaining = Vec::with_capacity(args.len());
    for argument in args {
        match argument.to_string_lossy().as_ref() {
            "--writable" | "-w" => {
                anyhow::ensure!(!writable, "--writable may only be specified once");
                writable = true;
            }
            _ => remaining.push(argument),
        }
    }
    let (root, port, config_path) = parse_server_options(&remaining, "TFTP")?;
    Ok(CliServiceCommand::Tftp(TftpServerCommand {
        root,
        port,
        config_path,
        writable,
    }))
}

fn parse_server_options(
    args: &[OsString],
    service: &str,
) -> Result<(PathBuf, Option<u16>, Option<PathBuf>)> {
    let mut root = PathBuf::from(".");
    let mut root_set = false;
    let mut port = None;
    let mut config_path = None;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--root" | "-r" => {
                anyhow::ensure!(!root_set, "--root may only be specified once");
                root = arguments
                    .next()
                    .context("--root requires a directory")?
                    .into();
                root_set = true;
            }
            "--port" | "-p" => {
                anyhow::ensure!(port.is_none(), "--port may only be specified once");
                port = Some(parse_port(
                    arguments.next().context("--port requires a port number")?,
                )?);
            }
            "--config" | "-c" => {
                anyhow::ensure!(config_path.is_none(), "--config may only be specified once");
                config_path = Some(arguments.next().context("--config requires a path")?.into());
            }
            option if option.starts_with('-') => {
                anyhow::bail!("unknown {service} server option {option:?}")
            }
            value => anyhow::bail!("unexpected {service} server argument {value:?}"),
        }
    }
    Ok((root, port, config_path))
}

fn parse_port(argument: &OsString) -> Result<u16> {
    let port = argument
        .to_string_lossy()
        .parse::<u16>()
        .context("--port must be a number from 1 to 65535")?;
    anyhow::ensure!(port != 0, "--port must be a number from 1 to 65535");
    Ok(port)
}

#[cfg(feature = "tftp-server")]
impl TftpServerCommand {
    fn resolved_port(&self) -> Result<u16> {
        Ok(match self.port {
            Some(port) => port,
            None => {
                crate::Config::load(self.config_path.as_deref(), None)
                    .context("loading configuration for the TFTP server")?
                    .tftp_server_port
            }
        })
    }

    fn proxy_args(&self) -> Result<Vec<OsString>> {
        let port = self.resolved_port()?;
        let mut args = vec![
            OsString::from("server"),
            OsString::from("--root"),
            self.root.as_os_str().to_owned(),
            OsString::from("--port"),
            OsString::from(port.to_string()),
        ];
        if self.writable {
            args.push(OsString::from("--writable"));
        }
        Ok(args)
    }

    pub(super) fn run(&self) -> Result<()> {
        crate::tftp::run_tftp_proxy(&self.proxy_args()?)
    }
}

#[cfg(test)]
#[path = "../tests/cli_services/servers.rs"]
mod tests;
