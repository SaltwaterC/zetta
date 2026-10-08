//! Clipboard request state on the displaying terminal's side of the channel.
//!
//! Every frame reaching this host was printed by a pane, so the host bounds
//! what pane output can make it hold: the number of sessions, the bytes one
//! transfer and all transfers together may hold, and how long a transfer may
//! live. Activity extends only the idle timeout; a transfer that keeps sending
//! still ends at its absolute lifetime.

use crate::protocol::{CHUNK_SIZE, Frame, Message};
use anyhow::{Context as _, Result, ensure};
use std::{
    collections::HashMap,
    io::{Read as _, Seek as _, Write as _},
    time::{Duration, Instant},
};

const SESSION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SESSIONS: usize = 16;
/// Bytes one copy may spool, or one paste may hold. Far beyond any selection a
/// person copies, and still a transfer a slow SSH link finishes well within
/// `TRANSFER_LIFETIME` (16 MiB is about 21 MiB of base64).
pub const MAX_TRANSFER_BYTES: usize = 16 * 1024 * 1024;
/// Bytes all of one host's transfers may hold together, so sixteen sessions
/// cannot each spool `MAX_TRANSFER_BYTES`.
const MAX_HELD_BYTES: usize = 2 * MAX_TRANSFER_BYTES;
/// Lifetime of a transfer from its first frame, which data does not refresh.
const TRANSFER_LIFETIME: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy)]
struct Limits {
    idle: Duration,
    lifetime: Duration,
    transfer_bytes: usize,
    held_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            idle: SESSION_TIMEOUT,
            lifetime: TRANSFER_LIFETIME,
            transfer_bytes: MAX_TRANSFER_BYTES,
            held_bytes: MAX_HELD_BYTES,
        }
    }
}

enum Transfer {
    Copy {
        spool: std::fs::File,
        next: u64,
    },
    Paste {
        bytes: Vec<u8>,
        offset: usize,
        next: u64,
    },
}

struct Session {
    transfer: Transfer,
    started: Instant,
    activity: Instant,
    /// Bytes spooled by a copy, or held for a paste.
    bytes: usize,
}

impl Session {
    fn new(transfer: Transfer, bytes: usize) -> Self {
        let now = Instant::now();
        Self {
            transfer,
            started: now,
            activity: now,
            bytes,
        }
    }
}

#[derive(Default)]
pub struct Host {
    sessions: HashMap<[u8; 16], Session>,
    limits: Limits,
}

impl Host {
    /// Answers one request. `None` means the request warrants no answer: a
    /// helper's error frame cancels its transfer, and the helper has stopped
    /// reading by the time it sends one.
    pub fn handle(
        &mut self,
        request: Frame,
        allow_paste: bool,
        mut copy: impl FnMut(&str) -> Result<()>,
        mut paste: impl FnMut() -> Result<Option<String>>,
    ) -> Option<Frame> {
        let limits = self.limits;
        self.sessions.retain(|_, session| {
            session.activity.elapsed() < limits.idle && session.started.elapsed() < limits.lifetime
        });
        let id = request.id;
        if let Message::Error(_) = request.message {
            self.sessions.remove(&id);
            return None;
        }
        let response = self.process(id, request.message, allow_paste, &mut copy, &mut paste);
        Some(Frame {
            id,
            message: match response {
                Ok(message) => message,
                Err(error) => {
                    self.sessions.remove(&id);
                    Message::Error(format!("{error:#}").chars().take(256).collect())
                }
            },
        })
    }

    fn held_bytes(&self) -> usize {
        self.sessions.values().map(|session| session.bytes).sum()
    }

    fn process(
        &mut self,
        id: [u8; 16],
        message: Message,
        allow_paste: bool,
        copy: &mut impl FnMut(&str) -> Result<()>,
        paste: &mut impl FnMut() -> Result<Option<String>>,
    ) -> Result<Message> {
        match message {
            Message::Probe => Ok(Message::Ready),
            Message::Copy => {
                ensure!(
                    !self.sessions.contains_key(&id),
                    "duplicate clipboard request"
                );
                ensure!(
                    self.sessions.len() < MAX_SESSIONS,
                    "too many clipboard requests"
                );
                let spool = tempfile::tempfile().context("creating private clipboard spool")?;
                self.sessions
                    .insert(id, Session::new(Transfer::Copy { spool, next: 0 }, 0));
                Ok(Message::Ack { next_sequence: 0 })
            }
            Message::Paste => {
                ensure!(
                    allow_paste,
                    "remote clipboard paste is disabled for this tab"
                );
                ensure!(
                    !self.sessions.contains_key(&id),
                    "duplicate clipboard request"
                );
                ensure!(
                    self.sessions.len() < MAX_SESSIONS,
                    "too many clipboard requests"
                );
                let bytes = paste()?.unwrap_or_default().into_bytes();
                ensure!(
                    bytes.len() <= self.limits.transfer_bytes,
                    "clipboard is too large for a remote paste"
                );
                ensure!(
                    self.held_bytes() + bytes.len() <= self.limits.held_bytes,
                    "too much clipboard data in flight"
                );
                let held = bytes.len();
                self.sessions.insert(
                    id,
                    Session::new(
                        Transfer::Paste {
                            bytes,
                            offset: 0,
                            next: 0,
                        },
                        held,
                    ),
                );
                self.next_paste_chunk(id)
            }
            Message::Data { sequence, bytes } => {
                let held = self.held_bytes();
                let limits = self.limits;
                let Some(Session {
                    transfer: Transfer::Copy { spool, next },
                    activity,
                    bytes: spooled,
                    ..
                }) = self.sessions.get_mut(&id)
                else {
                    anyhow::bail!("clipboard copy is not active");
                };
                ensure!(bytes.len() <= CHUNK_SIZE, "clipboard chunk is too large");
                if sequence == *next {
                    ensure!(
                        *spooled + bytes.len() <= limits.transfer_bytes,
                        "clipboard copy is too large"
                    );
                    ensure!(
                        held + bytes.len() <= limits.held_bytes,
                        "too much clipboard data in flight"
                    );
                    spool.write_all(&bytes).context("spooling clipboard copy")?;
                    *spooled += bytes.len();
                    *next += 1;
                } else {
                    ensure!(sequence < *next, "clipboard chunk is out of order");
                }
                *activity = Instant::now();
                Ok(Message::Ack {
                    next_sequence: *next,
                })
            }
            Message::Ack { next_sequence } => {
                let Some(Session {
                    transfer: Transfer::Paste { next, .. },
                    activity,
                    ..
                }) = self.sessions.get_mut(&id)
                else {
                    anyhow::bail!("clipboard paste is not active");
                };
                ensure!(
                    next_sequence == *next,
                    "clipboard acknowledgement is out of order"
                );
                *activity = Instant::now();
                self.next_paste_chunk(id)
            }
            Message::End => {
                let Some(Session {
                    transfer: Transfer::Copy { mut spool, .. },
                    bytes,
                    ..
                }) = self.sessions.remove(&id)
                else {
                    anyhow::bail!("clipboard copy is not active");
                };
                spool.rewind().context("rewinding clipboard copy")?;
                // The spool holds exactly what was counted; reading no further
                // keeps this allocation inside the transfer limit regardless.
                let mut text = Vec::with_capacity(bytes);
                spool
                    .take(bytes as u64)
                    .read_to_end(&mut text)
                    .context("reading clipboard copy")?;
                let text = String::from_utf8(text).context("clipboard copy is not UTF-8 text")?;
                copy(&text)?;
                Ok(Message::Done)
            }
            Message::Ready | Message::Done | Message::Error(_) => {
                anyhow::bail!("unexpected clipboard response")
            }
        }
    }

    fn next_paste_chunk(&mut self, id: [u8; 16]) -> Result<Message> {
        let Some(Session {
            transfer:
                Transfer::Paste {
                    bytes,
                    offset,
                    next,
                },
            ..
        }) = self.sessions.get_mut(&id)
        else {
            anyhow::bail!("clipboard paste is not active");
        };
        if *offset == bytes.len() {
            self.sessions.remove(&id);
            return Ok(Message::Done);
        }
        let end = (*offset + CHUNK_SIZE).min(bytes.len());
        let response = Message::Data {
            sequence: *next,
            bytes: bytes[*offset..end].to_vec(),
        };
        *offset = end;
        *next += 1;
        Ok(response)
    }
}

#[cfg(test)]
#[path = "tests/host.rs"]
mod tests;
