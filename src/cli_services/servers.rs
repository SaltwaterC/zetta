use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context as _, Result};

use super::CliServiceCommand;

#[cfg(feature = "http-server")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HttpServerCommand {
    root: PathBuf,
    port: Option<u16>,
    config_path: Option<PathBuf>,
}

#[cfg(feature = "http-server")]
pub(crate) fn http_server_help() -> &'static str {
    "Serve static files over HTTP\n\nUsage: zetta http server [OPTIONS]\n\nOptions:\n  -r, --root PATH                   Directory to serve (default: current directory)\n  -p, --port PORT                   TCP port (default: http_server_port from configuration)\n  -c, --config PATH                 Read the HTTP port default from this configuration file\n  -h, --help                        Print help\n\nPress Ctrl-C to stop the server."
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
        anyhow::bail!(http_server_help());
    }
    anyhow::ensure!(
        args.first().is_some_and(|argument| argument == "server"),
        "usage: zetta http server [OPTIONS]; run `zetta http server --help` for details"
    );
    let (root, port, config_path) = parse_server_options(&args[1..], "HTTP")?;
    Ok(CliServiceCommand::Http(HttpServerCommand {
        root,
        port,
        config_path,
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

    pub(super) fn run(&self) -> Result<()> {
        let port = self.resolved_port()?;
        let server = crate::start_http_server(&self.root, port)?;
        eprintln!(
            "Serving {} at http://{}; press Ctrl-C to stop.",
            server.root.display(),
            server.address
        );
        stream_server_logs(server.reader, server.writer)
    }
}

#[cfg(feature = "tftp-server")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TftpServerCommand {
    root: PathBuf,
    port: Option<u16>,
    config_path: Option<PathBuf>,
}

#[cfg(feature = "tftp-server")]
pub(crate) fn tftp_server_help() -> &'static str {
    "Serve files with TFTP\n\nUsage: zetta tftp server [OPTIONS]\n\nOptions:\n  -r, --root PATH                   Directory to serve (default: current directory)\n  -p, --port PORT                   UDP port (default: tftp_server_port from configuration)\n  -c, --config PATH                 Read the TFTP port default from this configuration file\n  -h, --help                        Print help\n\nPress Ctrl-C to stop the server."
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
        anyhow::bail!(tftp_server_help());
    }
    let (root, port, config_path) = parse_server_options(&args, "TFTP")?;
    Ok(CliServiceCommand::Tftp(TftpServerCommand {
        root,
        port,
        config_path,
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

    pub(super) fn run(&self) -> Result<()> {
        let port = self.resolved_port()?;
        let server = crate::start_server(&self.root, port)?;
        eprintln!(
            "Serving {} with TFTP at {}; press Ctrl-C to stop.",
            server.root.display(),
            server.address
        );
        stream_server_logs(server.reader, server.writer)
    }
}

fn stream_server_logs(
    mut reader: Box<dyn io::Read + Send>,
    _control: Box<dyn io::Write + Send>,
) -> Result<()> {
    let mut stdout = io::stdout().lock();
    let mut buffer = [0; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if let Err(error) = stdout.write_all(&buffer[..count]) {
                    if error.kind() == io::ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                    return Err(error).context("writing server log output");
                }
                stdout.flush().context("flushing server log output")?;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error).context("reading server log output"),
        }
    }
}

#[cfg(test)]
#[path = "../tests/cli_services/servers.rs"]
mod tests;
