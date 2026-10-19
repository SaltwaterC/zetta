//! Standalone HTTP command parsing and foreground log streaming.
use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;

use crate::format_help_table;
use anyhow::{Context as _, Result};

pub fn http_server_help() -> String {
    format!(
        "Serve static files over HTTP\n\nUsage: zhttp server [OPTIONS]\n\nOptions:\n{}\n\nThe server has no authentication: it answers any host that can reach the\nport, which is how it hands files to devices on the LAN. It refuses names\nwith a dot-prefixed component (.git, .env) unless --serve-dotfiles is given.\nIt answers only requests whose Host header is absent or names this machine:\nthe address the request arrived on, a loopback address, localhost, this\nmachine's host name or <hostname>.local, or an --allow-host name. {limits}\n\nPress Ctrl-C to stop the server.",
        format_help_table([
            (
                "-r, --root PATH",
                "Directory to serve (default: current directory)",
            ),
            ("-p, --port PORT", "TCP port (default: 8000)",),
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

/// Separates HTTP access-policy flags from root, port and configuration options.
pub fn parse_http_server_flags(
    args: &[OsString],
) -> Result<(crate::HttpServerOptions, Vec<OsString>)> {
    let mut options = crate::HttpServerOptions::default();
    let mut remaining = Vec::with_capacity(args.len());
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--serve-dotfiles" | "-d" => {
                anyhow::ensure!(
                    !options.serve_dotfiles,
                    "--serve-dotfiles may only be specified once"
                );
                options.serve_dotfiles = true;
            }
            "--allow-any-host" | "-A" => {
                anyhow::ensure!(
                    !options.allow_any_host,
                    "--allow-any-host may only be specified once"
                );
                options.allow_any_host = true;
            }
            "--allow-host" | "-a" => {
                let name = arguments
                    .next()
                    .context("--allow-host requires a host name or address")?;
                let name = crate::normalize_allowed_host(&name.to_string_lossy())
                    .map_err(anyhow::Error::msg)?;
                if !options.allowed_hosts.contains(&name) {
                    options.allowed_hosts.push(name);
                }
            }
            "--root" | "-r" | "--port" | "-p" | "--config" | "-c" => {
                // Keep a shared option's value with it, so a value that looks
                // like one of the flags above is not taken for it.
                remaining.push(argument.clone());
                remaining.extend(arguments.next().cloned());
            }
            _ => remaining.push(argument.clone()),
        }
    }
    Ok((options, remaining))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerCommand {
    pub root: PathBuf,
    pub port: u16,
    pub options: crate::HttpServerOptions,
}

pub fn parse_http_args(args: impl IntoIterator<Item = OsString>) -> Result<ServerCommand> {
    let args: Vec<_> = args.into_iter().collect();
    anyhow::ensure!(
        args.first().is_some_and(|argument| argument == "server"),
        "usage: zhttp server [OPTIONS]; run `zhttp server --help` for details"
    );
    let (options, remaining) = parse_http_server_flags(&args[1..])?;
    let mut command = ServerCommand {
        root: PathBuf::from("."),
        port: 8000,
        options,
    };
    let mut root_set = false;
    let mut port_set = false;
    let mut args = remaining.into_iter();
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--root" | "-r" => {
                anyhow::ensure!(!root_set, "--root may only be specified once");
                command.root = args.next().context("--root requires a directory")?.into();
                root_set = true;
            }
            "--port" | "-p" => {
                anyhow::ensure!(!port_set, "--port may only be specified once");
                command.port = args
                    .next()
                    .context("--port requires a port number")?
                    .to_string_lossy()
                    .parse()
                    .context("--port must be a number from 1 to 65535")?;
                anyhow::ensure!(command.port != 0, "--port must be a number from 1 to 65535");
                port_set = true;
            }
            "--help" | "-h" => anyhow::bail!("{}", http_server_help()),
            value => anyhow::bail!("unknown HTTP server argument {value:?}"),
        }
    }
    Ok(command)
}

impl ServerCommand {
    pub fn run(&self) -> Result<()> {
        let server = crate::start_http_server(&self.root, self.port, self.options.clone())?;
        eprintln!(
            "Serving {} at http://{}; press Ctrl-C to stop.",
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
#[path = "tests/cli.rs"]
mod tests;
