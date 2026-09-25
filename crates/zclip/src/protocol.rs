//! Versioned clipboard messages carried by private OSC 777 sequences.
//!
//! The scanner removes only well-formed clipboard sequences. In particular,
//! an ordinary OSC 52 sequence is passed through unchanged.

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use std::borrow::Cow;

pub const CHUNK_SIZE: usize = 32 * 1024;
pub const WINDOW_SIZE: usize = 16;
const PREFIX: &[u8] = b"\x1b]777;zclip;1;";
const MAX_SEQUENCE: usize = 46 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Probe,
    Ready,
    Copy,
    Paste,
    Data { sequence: u64, bytes: Vec<u8> },
    Ack { next_sequence: u64 },
    End,
    Done,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: [u8; 16],
    pub message: Message,
}

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut output = PREFIX.to_vec();
        for byte in self.id {
            output.extend_from_slice(format!("{byte:02x}").as_bytes());
        }
        match &self.message {
            Message::Probe => output.extend_from_slice(b";probe"),
            Message::Ready => output.extend_from_slice(b";ready"),
            Message::Copy => output.extend_from_slice(b";copy"),
            Message::Paste => output.extend_from_slice(b";paste"),
            Message::Data { sequence, bytes } => {
                output.extend_from_slice(format!(";data;{sequence};").as_bytes());
                output.extend_from_slice(STANDARD_NO_PAD.encode(bytes).as_bytes());
            }
            Message::Ack { next_sequence } => {
                output.extend_from_slice(format!(";ack;{next_sequence}").as_bytes());
            }
            Message::End => output.extend_from_slice(b";end"),
            Message::Done => output.extend_from_slice(b";done"),
            Message::Error(error) => {
                output.extend_from_slice(b";error;");
                output.extend_from_slice(STANDARD_NO_PAD.encode(error).as_bytes());
            }
        }
        output.push(7);
        output
    }

    pub fn parse(sequence: &[u8]) -> Option<Self> {
        let body = sequence.strip_prefix(PREFIX)?;
        let body = body
            .strip_suffix(b"\x07")
            .or_else(|| body.strip_suffix(b"\x1b\\"))?;
        let body = std::str::from_utf8(body).ok()?;
        let mut fields = body.split(';');
        let id_text = fields.next()?;
        if id_text.len() != 32 {
            return None;
        }
        let mut id = [0; 16];
        for (index, byte) in id.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&id_text[index * 2..index * 2 + 2], 16).ok()?;
        }
        let kind = fields.next()?;
        let message = match kind {
            "probe" => Message::Probe,
            "ready" => Message::Ready,
            "copy" => Message::Copy,
            "paste" => Message::Paste,
            "end" => Message::End,
            "done" => Message::Done,
            "ack" => Message::Ack {
                next_sequence: fields.next()?.parse().ok()?,
            },
            "data" => {
                let sequence = fields.next()?.parse().ok()?;
                let bytes = STANDARD_NO_PAD.decode(fields.next()?).ok()?;
                if bytes.len() > CHUNK_SIZE {
                    return None;
                }
                Message::Data { sequence, bytes }
            }
            "error" => {
                let bytes = STANDARD_NO_PAD.decode(fields.next()?).ok()?;
                if bytes.len() > 1024 {
                    return None;
                }
                Message::Error(String::from_utf8(bytes).ok()?)
            }
            _ => return None,
        };
        if fields.next().is_some() {
            return None;
        }
        Some(Self { id, message })
    }
}

/// Removes complete clipboard frames while preserving all other terminal data.
/// A partial OSC is held until its terminator arrives in a later read.
#[derive(Default)]
pub struct Scanner {
    pending: Vec<u8>,
    osc: bool,
}

impl Scanner {
    /// Avoid a copy for normal text and CSI-heavy terminal output. Only OSC
    /// candidates need the byte-by-byte state machine.
    pub fn filter_cow<'a>(
        &mut self,
        input: &'a [u8],
        on_frame: impl FnMut(Frame),
    ) -> Cow<'a, [u8]> {
        if self.pending.is_empty()
            && !input.ends_with(b"\x1b")
            && !input.windows(2).any(|pair| pair == b"\x1b]")
        {
            return Cow::Borrowed(input);
        }
        Cow::Owned(self.filter(input, on_frame))
    }

    /// Report frames without copying ordinary PTY output.
    pub fn observe(&mut self, input: &[u8], on_frame: impl FnMut(Frame)) {
        let _ = self.filter_cow(input, on_frame);
    }

    pub fn filter(&mut self, input: &[u8], mut on_frame: impl FnMut(Frame)) -> Vec<u8> {
        let mut output = Vec::with_capacity(input.len());
        for &byte in input {
            if self.pending.is_empty() {
                if byte == 0x1b {
                    self.pending.push(byte);
                } else {
                    output.push(byte);
                }
                continue;
            }
            if self.pending.len() == 1 && !self.osc {
                if byte == b']' {
                    self.pending.push(byte);
                    self.osc = true;
                    continue;
                }
                output.push(0x1b);
                self.pending.clear();
                if byte == 0x1b {
                    self.pending.push(byte);
                } else {
                    output.push(byte);
                }
                continue;
            }
            if !self.osc {
                output.append(&mut self.pending);
                output.push(byte);
                continue;
            }
            self.pending.push(byte);
            let terminated =
                byte == 7 || self.pending.ends_with(b"\x1b\\") || matches!(byte, 0x18 | 0x1a);
            if terminated || self.pending.len() > MAX_SEQUENCE {
                if let Some(frame) = Frame::parse(&self.pending) {
                    on_frame(frame);
                } else {
                    output.extend_from_slice(&self.pending);
                }
                self.pending.clear();
                self.osc = false;
            }
        }
        output
    }

    pub fn finish(&mut self) -> Vec<u8> {
        self.osc = false;
        std::mem::take(&mut self.pending)
    }
}

pub fn new_request_id() -> std::io::Result<[u8; 16]> {
    let mut id = [0; 16];
    getrandom::fill(&mut id).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(id)
}

#[cfg(test)]
#[path = "tests/protocol.rs"]
mod tests;
