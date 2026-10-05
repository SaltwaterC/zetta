//! The messages `wslx.exe` and the Linux relay exchange over the relay's
//! stdio.
//!
//! Every agent connection inside the distribution is carried over one byte
//! stream, so each message names its connection. A message is a nine-byte
//! header — kind, connection id and payload length, the last two big-endian —
//! followed by the payload.
//!
//! The payload of a request or a reply is exactly one SSH-agent message,
//! length prefix included, rather than whatever bytes a read returned. That is
//! what lets the Windows half talk to the agent pipe from one thread per
//! connection: a synchronous pipe handle serializes a blocked read against any
//! write, so a reader thread parked waiting for a reply would stall the
//! request it was waiting on. The agent protocol is strictly one reply per
//! request, so whole messages in turn are all it ever needs.

use std::{
    io::{self, Read, Write},
    sync::Mutex,
};

/// The largest agent message carried, matching Zosh's and zmux's limit.
pub const MAX_AGENT_FRAME: usize = 256 * 1024;

const HEADER_LEN: usize = 9;

const OPEN: u8 = 1;
const REQUEST: u8 = 2;
const REPLY: u8 = 3;
const CLOSE: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Relay to Windows: a client connected to the socket.
    Open(u32),
    /// Relay to Windows: one agent request from that client.
    Request(u32, Vec<u8>),
    /// Windows to relay: the agent's reply to the connection's request.
    Reply(u32, Vec<u8>),
    /// Either way: the sender is done with the connection. It is never
    /// answered, so whichever side closes first is the only one to say so.
    Close(u32),
}

impl Message {
    /// Writes the message and flushes it, so a reply is never left sitting in
    /// a buffer while its client waits.
    pub fn write_to(&self, output: &mut impl Write) -> io::Result<()> {
        let (kind, id, payload): (u8, u32, &[u8]) = match self {
            Self::Open(id) => (OPEN, *id, &[]),
            Self::Request(id, frame) => (REQUEST, *id, frame),
            Self::Reply(id, frame) => (REPLY, *id, frame),
            Self::Close(id) => (CLOSE, *id, &[]),
        };
        let length = u32::try_from(payload.len())
            .ok()
            .filter(|&length| length as usize <= MAX_AGENT_FRAME)
            .ok_or_else(|| invalid_input("agent message exceeds the frame limit"))?;
        let mut message = Vec::with_capacity(HEADER_LEN + payload.len());
        message.push(kind);
        message.extend_from_slice(&id.to_be_bytes());
        message.extend_from_slice(&length.to_be_bytes());
        message.extend_from_slice(payload);
        output.write_all(&message)?;
        output.flush()
    }

    /// Reads the next message, or `None` when the stream ends cleanly between
    /// messages. A stream that ends inside one is an error.
    pub fn read_from(input: &mut impl Read) -> io::Result<Option<Self>> {
        let mut header = [0_u8; HEADER_LEN];
        if !read_exact_or_eof(input, &mut header)? {
            return Ok(None);
        }
        let kind = header[0];
        let id = u32::from_be_bytes(header[1..5].try_into().expect("four bytes"));
        let length = u32::from_be_bytes(header[5..9].try_into().expect("four bytes")) as usize;
        let carries_frame = matches!(kind, REQUEST | REPLY);
        if !carries_frame && length != 0 {
            return Err(invalid_data("a connection message carried a payload"));
        }
        if length > MAX_AGENT_FRAME {
            return Err(invalid_data("agent message exceeds the frame limit"));
        }
        let mut payload = vec![0_u8; length];
        input.read_exact(&mut payload)?;
        if carries_frame && !is_agent_frame(&payload) {
            return Err(invalid_data("payload is not one agent message"));
        }
        match kind {
            OPEN => Ok(Some(Self::Open(id))),
            REQUEST => Ok(Some(Self::Request(id, payload))),
            REPLY => Ok(Some(Self::Reply(id, payload))),
            CLOSE => Ok(Some(Self::Close(id))),
            _ => Err(invalid_data("unknown message kind")),
        }
    }
}

/// Writes `message` under `output`'s lock, so messages from different
/// connections never interleave.
pub(crate) fn send(output: &Mutex<impl Write>, message: &Message) -> io::Result<()> {
    message.write_to(&mut *crate::lock(output))
}

/// Reads one SSH-agent message, length prefix included, or `None` when the
/// peer hung up before starting another.
pub fn read_agent_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0_u8; 4];
    if !read_exact_or_eof(input, &mut length)? {
        return Ok(None);
    }
    let body = u32::from_be_bytes(length) as usize;
    if body == 0 || body > MAX_AGENT_FRAME - 4 {
        return Err(invalid_data("agent message exceeds the frame limit"));
    }
    let mut frame = Vec::with_capacity(body + 4);
    frame.extend_from_slice(&length);
    frame.resize(body + 4, 0);
    input.read_exact(&mut frame[4..])?;
    Ok(Some(frame))
}

/// Whether `frame` is exactly one non-empty agent message.
pub fn is_agent_frame(frame: &[u8]) -> bool {
    frame.len() > 4
        && frame.len() <= MAX_AGENT_FRAME
        && u32::from_be_bytes(frame[..4].try_into().expect("four bytes")) as usize
            == frame.len() - 4
}

/// Fills `buffer`, returning `false` when the input ended before its first
/// byte and an error when it ended part-way.
fn read_exact_or_eof(input: &mut impl Read, buffer: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match input.read(&mut buffer[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

fn invalid_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

#[cfg(test)]
#[path = "tests/protocol.rs"]
mod tests;
