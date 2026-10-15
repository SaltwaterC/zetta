//! Clipboard request state on the displaying terminal's side of the channel.
//!
//! Every frame reaching this host was printed by a pane, so the host bounds
//! what pane output can make it hold: the number of sessions, the bytes one
//! transfer and all transfers together may hold, and how long a transfer may
//! live. Activity extends only the idle timeout; a transfer that keeps sending
//! still ends at its absolute lifetime.
//!
//! The tab grants copy and paste separately ([`Permissions`]), and both are
//! checked on every frame, not only on the one that starts a transfer: a frame
//! handled while one is withdrawn, or [`Host::revoke_paste`] /
//! [`Host::revoke_copy`], ends every transfer of that kind in flight. A copy
//! ended so never reaches the clipboard; its spool is discarded.
//!
//! A paste holds the host clipboard it captured until the helper has fetched
//! all of it. However a paste ends — finished, cancelled, refused, revoked,
//! expired, or with the host dropped — its captured clipboard is overwritten
//! before it is freed.

use crate::protocol::{CHUNK_SIZE, Frame, Message};
use anyhow::{Context as _, Result, ensure};
use std::{
    collections::HashMap,
    io::{Read as _, Seek as _, Write as _},
    sync::atomic::{Ordering, compiler_fence},
    time::{Duration, Instant},
};

/// The answer to a paste, or to its continuation, the tab does not allow.
pub const PASTE_DISABLED: &str = "remote clipboard paste is disabled for this tab";
/// The answer to a copy, or to its continuation, the tab does not allow.
pub const COPY_DISABLED: &str = "remote clipboard copy is disabled for this tab";

/// What the tab lets its panes do with the host clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permissions {
    /// Write the host clipboard: the zcopy direction.
    pub copy: bool,
    /// Read the host clipboard: the zpaste direction.
    pub paste: bool,
}

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
        bytes: PasteBuffer,
        offset: usize,
        next: u64,
    },
}

/// Host clipboard captured for a paste, erased when it is dropped.
///
/// The erasure is volatile writes followed by a compiler fence rather than the
/// `zeroize` crate, which this crate's lockfile and three of the four that
/// build it as a dependency do not carry; these few lines are all of what it
/// would be used for here.
struct PasteBuffer(Vec<u8>);

impl Drop for PasteBuffer {
    fn drop(&mut self) {
        erase(&mut self.0);
    }
}

/// Overwrites `bytes` with zeros in a way the optimiser may not drop as a dead
/// store: the whole allocation, spare capacity included, since the reader that
/// filled it may have left clipboard bytes past `len`.
fn erase(bytes: &mut Vec<u8>) {
    let start = bytes.as_mut_ptr();
    for index in 0..bytes.capacity() {
        // SAFETY: `index` is within the allocation, and a write reads nothing,
        // so the spare capacity need not be initialised.
        unsafe { start.add(index).write_volatile(0) };
    }
    compiler_fence(Ordering::SeqCst);
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
        permissions: Permissions,
        mut copy: impl FnMut(&str) -> Result<()>,
        mut paste: impl FnMut() -> Result<Option<String>>,
    ) -> Option<Frame> {
        self.expire();
        if !permissions.paste {
            self.revoke_paste();
        }
        if !permissions.copy {
            self.revoke_copy();
        }
        let id = request.id;
        if let Message::Error(_) = request.message {
            self.sessions.remove(&id);
            return None;
        }
        let response = self.process(id, request.message, permissions, &mut copy, &mut paste);
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

    /// Ends every paste in flight, erasing what each captured. Called when the
    /// tab's paste permission is withdrawn, so revocation does not wait for
    /// the helper's next acknowledgement; that acknowledgement is then refused.
    /// Copies are unaffected; [`Host::revoke_copy`] ends those.
    pub fn revoke_paste(&mut self) {
        self.sessions
            .retain(|_, session| !matches!(session.transfer, Transfer::Paste { .. }));
    }

    /// Ends every copy in flight, discarding what each spooled, so none of them
    /// reaches the clipboard. Called when the tab's copy permission is
    /// withdrawn; the copy's next chunk or its end is then refused. Pastes are
    /// unaffected.
    pub fn revoke_copy(&mut self) {
        self.sessions
            .retain(|_, session| !matches!(session.transfer, Transfer::Copy { .. }));
    }

    /// Ends every transfer past its idle timeout or its lifetime.
    pub fn expire(&mut self) {
        let limits = self.limits;
        self.sessions.retain(|_, session| {
            session.activity.elapsed() < limits.idle && session.started.elapsed() < limits.lifetime
        });
    }

    /// When the next transfer expires, if any is held. A caller that waits for
    /// requests should call [`Host::expire`] by then, so an abandoned paste is
    /// erased on time rather than whenever the next request happens to arrive.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.sessions
            .values()
            .map(|session| {
                (session.activity + self.limits.idle).min(session.started + self.limits.lifetime)
            })
            .min()
    }

    fn held_bytes(&self) -> usize {
        self.sessions.values().map(|session| session.bytes).sum()
    }

    fn process(
        &mut self,
        id: [u8; 16],
        message: Message,
        permissions: Permissions,
        copy: &mut impl FnMut(&str) -> Result<()>,
        paste: &mut impl FnMut() -> Result<Option<String>>,
    ) -> Result<Message> {
        match message {
            Message::Probe => Ok(Message::Ready),
            Message::Copy => {
                ensure!(permissions.copy, COPY_DISABLED);
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
                ensure!(permissions.paste, PASTE_DISABLED);
                ensure!(
                    !self.sessions.contains_key(&id),
                    "duplicate clipboard request"
                );
                ensure!(
                    self.sessions.len() < MAX_SESSIONS,
                    "too many clipboard requests"
                );
                // Wrapped at once, so a paste refused below is erased too.
                let bytes = PasteBuffer(paste()?.unwrap_or_default().into_bytes());
                ensure!(
                    bytes.0.len() <= self.limits.transfer_bytes,
                    "clipboard is too large for a remote paste"
                );
                ensure!(
                    self.held_bytes() + bytes.0.len() <= self.limits.held_bytes,
                    "too much clipboard data in flight"
                );
                let held = bytes.0.len();
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
                // `handle` has already ended the copy; this names why.
                ensure!(permissions.copy, COPY_DISABLED);
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
                // `handle` has already ended the paste; this names why.
                ensure!(permissions.paste, PASTE_DISABLED);
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
                ensure!(permissions.copy, COPY_DISABLED);
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
        if *offset == bytes.0.len() {
            self.sessions.remove(&id);
            return Ok(Message::Done);
        }
        let end = (*offset + CHUNK_SIZE).min(bytes.0.len());
        let response = Message::Data {
            sequence: *next,
            bytes: bytes.0[*offset..end].to_vec(),
        };
        *offset = end;
        *next += 1;
        Ok(response)
    }
}

#[cfg(test)]
#[path = "tests/host.rs"]
mod tests;
