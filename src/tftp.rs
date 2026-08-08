use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

#[cfg(feature = "tftp-client")]
use anyhow::Result;

#[cfg(feature = "tftp-server")]
mod server;
#[cfg(feature = "tftp-server")]
pub(crate) use server::{OpenTftpServer, start_server};

#[cfg(feature = "tftp-client")]
mod client;
#[cfg(feature = "tftp-client")]
pub(crate) use client::{TftpCommand, parse_tftp_args, tftp_help};

#[cfg(feature = "tftp-client")]
pub(crate) const DEFAULT_TFTP_PORT: u16 = 69;
const DEFAULT_BLOCK_SIZE: usize = 512;
#[cfg(feature = "tftp-client")]
const REQUESTED_BLOCK_SIZE: usize = 1428;
const MIN_BLOCK_SIZE: usize = 8;
const MAX_BLOCK_SIZE: usize = 65_464;
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RETRIES: usize = 5;

const OP_RRQ: u16 = 1;
const OP_WRQ: u16 = 2;
const OP_DATA: u16 = 3;
const OP_ACK: u16 = 4;
const OP_ERROR: u16 = 5;
const OP_OACK: u16 = 6;

#[cfg(tftp_enabled)]
fn transfer_socket(peer_ip: IpAddr) -> io::Result<UdpSocket> {
    match peer_ip {
        IpAddr::V4(_) => UdpSocket::bind(("0.0.0.0", 0)),
        IpAddr::V6(_) => UdpSocket::bind(("::", 0)),
    }
}

fn zero_terminated_fields(mut bytes: &[u8]) -> std::result::Result<Vec<&[u8]>, String> {
    let mut fields = Vec::new();
    while !bytes.is_empty() {
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| "request field is not terminated".to_owned())?;
        if end == 0 {
            return Err("request contains an empty field".to_owned());
        }
        fields.push(&bytes[..end]);
        bytes = &bytes[end + 1..];
    }
    Ok(fields)
}

fn read_block(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buffer.len() {
        match reader.read(&mut buffer[total..])? {
            0 => break,
            count => total += count,
        }
    }
    Ok(total)
}

#[cfg(feature = "tftp-client")]
fn request_packet(opcode: u16, filename: &str, size: Option<u64>) -> Vec<u8> {
    let mut packet = Vec::with_capacity(filename.len() + 48);
    packet.extend_from_slice(&opcode.to_be_bytes());
    push_field(&mut packet, filename);
    push_field(&mut packet, "octet");
    push_field(&mut packet, "blksize");
    push_field(&mut packet, &REQUESTED_BLOCK_SIZE.to_string());
    push_field(&mut packet, "tsize");
    push_field(&mut packet, &size.unwrap_or(0).to_string());
    packet
}

#[cfg(feature = "tftp-server")]
fn option_ack_packet(options: &[(String, String)]) -> Vec<u8> {
    let mut packet = OP_OACK.to_be_bytes().to_vec();
    for (name, value) in options {
        push_field(&mut packet, name);
        push_field(&mut packet, value);
    }
    packet
}

fn push_field(packet: &mut Vec<u8>, value: &str) {
    packet.extend_from_slice(value.as_bytes());
    packet.push(0);
}

fn set_data_packet(packet: &mut Vec<u8>, block: u16, data: &[u8]) {
    packet.clear();
    packet.extend_from_slice(&OP_DATA.to_be_bytes());
    packet.extend_from_slice(&block.to_be_bytes());
    packet.extend_from_slice(data);
}

fn ack_packet(block: u16) -> [u8; 4] {
    let [high, low] = block.to_be_bytes();
    [0, OP_ACK as u8, high, low]
}

#[cfg(feature = "tftp-server")]
fn send_error(socket: &UdpSocket, peer: SocketAddr, code: u16, message: &str) {
    let mut packet = Vec::with_capacity(message.len() + 5);
    packet.extend_from_slice(&OP_ERROR.to_be_bytes());
    packet.extend_from_slice(&code.to_be_bytes());
    push_field(&mut packet, message);
    let _ = send_packet(socket, &packet, peer);
}

fn send_packet(socket: &UdpSocket, packet: &[u8], peer: SocketAddr) -> io::Result<()> {
    loop {
        match socket.send_to(packet, peer) {
            Ok(size) if size == packet.len() => return Ok(()),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "UDP socket sent a partial datagram",
                ));
            }
            Err(error) if socket_operation_was_interrupted(&error) => continue,
            Err(error) => return Err(error),
        }
    }
}

fn socket_operation_was_interrupted(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Interrupted
}

fn packet_opcode(packet: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes([*packet.first()?, *packet.get(1)?]))
}

fn packet_block(packet: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes([*packet.get(2)?, *packet.get(3)?]))
}

fn error_message(packet: &[u8]) -> String {
    let message = packet.get(4..).unwrap_or_default();
    let end = message
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(message.len());
    String::from_utf8_lossy(&message[..end]).into_owned()
}

#[cfg(feature = "tftp-client")]
fn check_error_packet(packet: &[u8]) -> Result<()> {
    if packet_opcode(packet) == Some(OP_ERROR) {
        anyhow::bail!("TFTP server error: {}", error_message(packet));
    }
    Ok(())
}

#[cfg(feature = "tftp-client")]
fn parsed_block_size(packet: &[u8]) -> Option<usize> {
    if packet_opcode(packet) != Some(OP_OACK) {
        return None;
    }
    let fields = zero_terminated_fields(packet.get(2..)?).ok()?;
    for pair in fields.chunks_exact(2) {
        if pair[0].eq_ignore_ascii_case(b"blksize") {
            let value = std::str::from_utf8(pair[1]).ok()?.parse().ok()?;
            return (MIN_BLOCK_SIZE..=MAX_BLOCK_SIZE)
                .contains(&value)
                .then_some(value);
        }
    }
    None
}

#[cfg(all(test, tftp_enabled))]
#[path = "tests/tftp.rs"]
mod tests;
