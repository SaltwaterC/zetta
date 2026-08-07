use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Write as _};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use super::{
    DEFAULT_BLOCK_SIZE, DEFAULT_TFTP_PORT, MAX_BLOCK_SIZE, MAX_RETRIES, OP_ACK, OP_DATA, OP_ERROR,
    OP_OACK, OP_RRQ, OP_WRQ, SOCKET_TIMEOUT, ack_packet, check_error_packet, packet_block,
    packet_opcode, parsed_block_size, read_block, request_packet, send_packet, set_data_packet,
    socket_operation_was_interrupted, transfer_socket,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TftpCommand {
    Get {
        host: String,
        remote: String,
        local: PathBuf,
        port: u16,
    },
    Put {
        host: String,
        local: PathBuf,
        remote: String,
        port: u16,
    },
}

impl TftpCommand {
    pub(crate) fn run(&self) -> Result<()> {
        match self {
            Self::Get {
                host,
                remote,
                local,
                port,
            } => {
                download(host, *port, remote, local)?;
                println!("Downloaded {remote} to {}", local.display());
            }
            Self::Put {
                host,
                local,
                remote,
                port,
            } => {
                upload(host, *port, local, remote)?;
                println!("Uploaded {} as {remote}", local.display());
            }
        }
        Ok(())
    }
}

pub(crate) fn tftp_help() -> &'static str {
    #[cfg(feature = "tftp-server")]
    {
        "Zetta TFTP tools\n\nUsage:\n  zetta tftp get [--port PORT] HOST REMOTE [LOCAL]\n  zetta tftp put [--port PORT] HOST LOCAL [REMOTE]\n  zetta tftp server [OPTIONS]\n\nCommands:\n  get       Download REMOTE, optionally naming the LOCAL output file\n  put       Upload LOCAL, optionally naming the REMOTE file\n  server    Serve files with TFTP\n\nClient options:\n  -p, --port PORT    Server port (default: 69)\n  -h, --help         Print help\n\nRun `zetta tftp server --help` for server options."
    }
    #[cfg(not(feature = "tftp-server"))]
    {
        "Zetta TFTP client\n\nUsage:\n  zetta tftp get [--port PORT] HOST REMOTE [LOCAL]\n  zetta tftp put [--port PORT] HOST LOCAL [REMOTE]\n\nCommands:\n  get    Download REMOTE, optionally naming the LOCAL output file\n  put    Upload LOCAL, optionally naming the REMOTE file\n\nOptions:\n  -p, --port PORT    Server port (default: 69)\n  -h, --help         Print help"
    }
}

pub(crate) fn parse_tftp_args(args: impl IntoIterator<Item = OsString>) -> Result<TftpCommand> {
    let mut args = args.into_iter();
    let operation = args
        .next()
        .context("missing TFTP command; expected get or put")?
        .to_string_lossy()
        .into_owned();
    let mut port = DEFAULT_TFTP_PORT;
    let mut positional = Vec::new();
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--port" | "-p" => {
                port = args
                    .next()
                    .context("--port requires a port number")?
                    .to_string_lossy()
                    .parse::<u16>()
                    .context("--port must be a number from 1 to 65535")?;
                anyhow::ensure!(port != 0, "--port must be a number from 1 to 65535");
            }
            "--help" | "-h" => anyhow::bail!("{}", tftp_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown TFTP option {option:?}"),
            _ => positional.push(argument),
        }
    }

    match operation.as_str() {
        "get" => {
            anyhow::ensure!(
                (2..=3).contains(&positional.len()),
                "usage: zetta tftp get [--port PORT] HOST REMOTE [LOCAL]"
            );
            let host = utf8_argument(&positional[0], "HOST")?;
            let remote = utf8_argument(&positional[1], "REMOTE")?;
            anyhow::ensure!(!remote.contains('\0'), "REMOTE must not contain a NUL byte");
            let local = positional.get(2).map(PathBuf::from).unwrap_or_else(|| {
                Path::new(&remote)
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(&remote))
            });
            anyhow::ensure!(!local.as_os_str().is_empty(), "LOCAL must not be empty");
            Ok(TftpCommand::Get {
                host,
                remote,
                local,
                port,
            })
        }
        "put" => {
            anyhow::ensure!(
                (2..=3).contains(&positional.len()),
                "usage: zetta tftp put [--port PORT] HOST LOCAL [REMOTE]"
            );
            let host = utf8_argument(&positional[0], "HOST")?;
            let local = PathBuf::from(&positional[1]);
            let remote = positional
                .get(2)
                .map(|value| utf8_argument(value, "REMOTE"))
                .transpose()?
                .or_else(|| {
                    local
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(ToOwned::to_owned)
                })
                .context("REMOTE is required when LOCAL has no file name")?;
            anyhow::ensure!(!remote.contains('\0'), "REMOTE must not contain a NUL byte");
            Ok(TftpCommand::Put {
                host,
                local,
                remote,
                port,
            })
        }
        _ => anyhow::bail!("unknown TFTP command {operation:?}; expected get or put"),
    }
}

fn utf8_argument(argument: &OsStr, name: &str) -> Result<String> {
    argument
        .to_str()
        .map(ToOwned::to_owned)
        .with_context(|| format!("{name} must be valid UTF-8"))
}

fn download(host: &str, port: u16, remote: &str, local: &Path) -> Result<()> {
    let server = resolve_server(host, port)?;
    let socket = transfer_socket(server.ip())?;
    socket.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    let request = request_packet(OP_RRQ, remote, None);
    let mut response = vec![0; MAX_BLOCK_SIZE + 4];
    let (mut packet_size, peer) = initial_response(&socket, server, &request, &mut response)?;
    let mut block_size = DEFAULT_BLOCK_SIZE;
    if packet_opcode(&response[..packet_size]) == Some(OP_OACK) {
        block_size = parsed_block_size(&response[..packet_size]).unwrap_or(DEFAULT_BLOCK_SIZE);
        packet_size = receive_from_peer(&socket, peer, &ack_packet(0), OP_DATA, 1, &mut response)?;
    }
    check_error_packet(&response[..packet_size])?;
    let mut output =
        File::create(local).with_context(|| format!("creating local file {}", local.display()))?;
    let mut expected = 1_u16;
    loop {
        let packet = &response[..packet_size];
        if packet_opcode(packet) != Some(OP_DATA) || packet_block(packet) != Some(expected) {
            anyhow::bail!("expected DATA block {expected}");
        }
        let data = packet.get(4..).context("malformed DATA packet")?;
        output.write_all(data)?;
        let ack = ack_packet(expected);
        if data.len() < block_size {
            send_packet(&socket, &ack, peer)?;
            output.flush()?;
            return Ok(());
        }
        expected = expected.wrapping_add(1);
        packet_size = receive_from_peer(&socket, peer, &ack, OP_DATA, expected, &mut response)?;
    }
}

fn upload(host: &str, port: u16, local: &Path, remote: &str) -> Result<()> {
    let mut input =
        File::open(local).with_context(|| format!("opening local file {}", local.display()))?;
    let size = input.metadata()?.len();
    let server = resolve_server(host, port)?;
    let socket = transfer_socket(server.ip())?;
    socket.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    let request = request_packet(OP_WRQ, remote, Some(size));
    let mut response = vec![0; MAX_BLOCK_SIZE + 4];
    let (packet_size, peer) = initial_response(&socket, server, &request, &mut response)?;
    let packet = &response[..packet_size];
    check_error_packet(packet)?;
    let block_size = if packet_opcode(packet) == Some(OP_OACK) {
        parsed_block_size(packet).unwrap_or(DEFAULT_BLOCK_SIZE)
    } else {
        anyhow::ensure!(
            packet_opcode(packet) == Some(OP_ACK) && packet_block(packet) == Some(0),
            "expected ACK 0 or OACK"
        );
        DEFAULT_BLOCK_SIZE
    };
    let mut block = 1_u16;
    let mut data = vec![0; block_size];
    let mut outgoing = Vec::with_capacity(block_size + 4);
    loop {
        let count = read_block(&mut input, &mut data)?;
        set_data_packet(&mut outgoing, block, &data[..count]);
        let incoming_size =
            receive_from_peer(&socket, peer, &outgoing, OP_ACK, block, &mut response)?;
        let incoming = &response[..incoming_size];
        check_error_packet(incoming)?;
        anyhow::ensure!(
            packet_opcode(incoming) == Some(OP_ACK) && packet_block(incoming) == Some(block),
            "expected ACK {block}"
        );
        if count < block_size {
            return Ok(());
        }
        block = block.wrapping_add(1);
    }
}

fn resolve_server(host: &str, port: u16) -> Result<SocketAddr> {
    (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolving TFTP server {host}"))?
        .next()
        .with_context(|| format!("no address found for TFTP server {host}"))
}

fn initial_response(
    socket: &UdpSocket,
    server: SocketAddr,
    request: &[u8],
    response: &mut [u8],
) -> Result<(usize, SocketAddr)> {
    for _ in 0..MAX_RETRIES {
        send_packet(socket, request, server)?;
        loop {
            match socket.recv_from(response) {
                Ok((size, peer)) if peer.ip() == server.ip() => {
                    return Ok((size, peer));
                }
                Ok(_) => continue,
                Err(error) if socket_operation_was_interrupted(&error) => continue,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    anyhow::bail!("TFTP server did not respond")
}

fn receive_from_peer(
    socket: &UdpSocket,
    peer: SocketAddr,
    retry_packet: &[u8],
    expected_opcode: u16,
    expected_block: u16,
    response: &mut [u8],
) -> Result<usize> {
    for _ in 0..MAX_RETRIES {
        send_packet(socket, retry_packet, peer)?;
        loop {
            match socket.recv_from(response) {
                Ok((size, source)) if source == peer => {
                    let packet = &response[..size];
                    if packet_opcode(packet) == Some(OP_ERROR)
                        || (packet_opcode(packet) == Some(expected_opcode)
                            && packet_block(packet) == Some(expected_block))
                    {
                        return Ok(size);
                    }
                    send_packet(socket, retry_packet, peer)?;
                }
                Ok(_) => continue,
                Err(error) if socket_operation_was_interrupted(&error) => continue,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    anyhow::bail!("TFTP transfer timed out")
}

#[cfg(test)]
#[path = "../tests/tftp/client.rs"]
mod tests;
