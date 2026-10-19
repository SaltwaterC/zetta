//! Standalone server options and foreground log streaming, without application configuration.
use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::format_help_table;

pub fn tftp_server_help() -> String {
    format!(
        "Serve files with TFTP\n\nUsage: ztftp server [OPTIONS]\n\nOptions:\n{}\n\nTFTP has no authentication: the server answers any host that can reach the\nport, and --writable lets those hosts create files under --root. Existing\nfiles are never overwritten, and names with a dot-prefixed component are\nrefused. Uploads are off unless --writable is given; point a writable server\nat a directory that holds only transfers.\n\nPress Ctrl-C to stop the server.",
        format_help_table([
            (
                "-r, --root PATH",
                "Directory to serve (default: current directory)",
            ),
            ("-p, --port PORT", "UDP port (default: 69)",),
            ("-w, --writable", "Accept uploads into the served directory"),
            ("-h, --help", "Print help"),
        ])
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerCommand {
    pub root: PathBuf,
    pub port: u16,
    pub writable: bool,
}

pub fn parse_server_args(args: impl IntoIterator<Item = OsString>) -> Result<ServerCommand> {
    let mut command = ServerCommand {
        root: PathBuf::from("."),
        port: 69,
        writable: false,
    };
    let mut root_set = false;
    let mut port_set = false;
    let mut args = args.into_iter();
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
            "--writable" | "-w" => {
                anyhow::ensure!(!command.writable, "--writable may only be specified once");
                command.writable = true;
            }
            "--help" | "-h" => anyhow::bail!("{}", tftp_server_help()),
            value => anyhow::bail!("unknown TFTP server argument {value:?}"),
        }
    }
    Ok(command)
}

impl ServerCommand {
    pub fn run(&self) -> Result<()> {
        let server = crate::start_server(&self.root, self.port, self.writable)?;
        eprintln!(
            "Serving {} with TFTP at {} ({}); press Ctrl-C to stop.",
            server.root.display(),
            server.address,
            if self.writable {
                "uploads enabled"
            } else {
                "read only"
            }
        );
        let mut reader = server.reader;
        let _control = server.writer;
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
                        return Err(error).context("writing TFTP server logs");
                    }
                    stdout.flush().context("flushing TFTP server logs")?;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error).context("reading TFTP server logs"),
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/cli.rs"]
mod tests;
