//! Clipboard request state on the displaying terminal's side of the channel.

use crate::protocol::{CHUNK_SIZE, Frame, Message};
use anyhow::{Context as _, Result, ensure};
use std::{
    collections::HashMap,
    io::{Read as _, Seek as _, Write as _},
    time::{Duration, Instant},
};

const SESSION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SESSIONS: usize = 16;

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
    activity: Instant,
}

#[derive(Default)]
pub struct Host {
    sessions: HashMap<[u8; 16], Session>,
}

impl Host {
    pub fn handle(
        &mut self,
        request: Frame,
        allow_paste: bool,
        mut copy: impl FnMut(&str) -> Result<()>,
        mut paste: impl FnMut() -> Result<Option<String>>,
    ) -> Frame {
        self.sessions
            .retain(|_, session| session.activity.elapsed() < SESSION_TIMEOUT);
        let id = request.id;
        let response = self.process(id, request.message, allow_paste, &mut copy, &mut paste);
        Frame {
            id,
            message: match response {
                Ok(message) => message,
                Err(error) => {
                    self.sessions.remove(&id);
                    Message::Error(format!("{error:#}").chars().take(256).collect())
                }
            },
        }
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
                self.sessions.insert(
                    id,
                    Session {
                        transfer: Transfer::Copy { spool, next: 0 },
                        activity: Instant::now(),
                    },
                );
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
                self.sessions.insert(
                    id,
                    Session {
                        transfer: Transfer::Paste {
                            bytes,
                            offset: 0,
                            next: 0,
                        },
                        activity: Instant::now(),
                    },
                );
                self.next_paste_chunk(id)
            }
            Message::Data { sequence, bytes } => {
                let Some(Session {
                    transfer: Transfer::Copy { spool, next },
                    activity,
                }) = self.sessions.get_mut(&id)
                else {
                    anyhow::bail!("clipboard copy is not active");
                };
                ensure!(bytes.len() <= CHUNK_SIZE, "clipboard chunk is too large");
                if sequence == *next {
                    spool.write_all(&bytes).context("spooling clipboard copy")?;
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
                    ..
                }) = self.sessions.remove(&id)
                else {
                    anyhow::bail!("clipboard copy is not active");
                };
                spool.rewind().context("rewinding clipboard copy")?;
                let mut text = String::new();
                spool
                    .read_to_string(&mut text)
                    .context("clipboard copy is not UTF-8 text")?;
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
