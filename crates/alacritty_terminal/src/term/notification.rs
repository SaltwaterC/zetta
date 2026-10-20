//! OSC 99 desktop notifications (kitty's protocol): metadata parsing, chunk
//! assembly, and the capability query.
//!
//! The core only assembles a notification; showing it is the frontend's job,
//! which is why the whole protocol is off unless [`Config::desktop_notifications`]
//! says a frontend can deliver one. Chunks are bounded per sequence and per
//! notification so an application cannot make the terminal buffer without
//! limit, and a notification that broke any rule is dropped whole rather than
//! shown in part.

use std::fmt::Write as _;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as Base64;

use super::*;

/// Largest plain payload one sequence may carry.
const MAX_PLAIN_CHUNK: usize = 2048;
/// Largest base64 payload one sequence may carry: 3072 decoded bytes.
const MAX_ENCODED_CHUNK: usize = 4096;
/// Largest title and body together, after decoding.
const MAX_NOTIFICATION: usize = 8192;
/// Identities assembled at once; the oldest is dropped to admit another.
const MAX_PENDING: usize = 16;

/// The reply to `p=?`. Clicking a notification focuses its pane; there is no
/// `report` action, no close or alive tracking, and no sound or urgency.
const CAPABILITIES: &str = "a=focus:o=always,unfocused,invisible:p=title,body,?:w=1";

/// A complete notification, ready for the frontend to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopNotification {
    /// The application's identifier, when it gave one.
    pub id: Option<String>,
    pub title: String,
    pub body: String,
    /// The application name from `f`.
    pub app_name: Option<String>,
    pub occasion: NotificationOccasion,
    pub expiry: NotificationExpiry,
}

/// When the application wants the notification shown (`o`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotificationOccasion {
    #[default]
    Always,
    /// Only while the pane does not have keyboard focus.
    Unfocused,
    /// Only while the pane cannot be seen at all.
    Invisible,
}

/// How long the notification stays up (`w`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotificationExpiry {
    /// Whatever the desktop's policy is.
    #[default]
    Default,
    Never,
    Milliseconds(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Payload {
    Title,
    Body,
    Close,
    Query,
    /// A payload type this terminal does not implement, such as `icon`.
    Unsupported,
}

/// One sequence's metadata. `None` fields were not given.
#[derive(Debug, PartialEq, Eq)]
struct Chunk {
    id: Option<String>,
    payload: Payload,
    done: bool,
    encoded: bool,
    app_name: Option<String>,
    occasion: Option<NotificationOccasion>,
    expiry: Option<NotificationExpiry>,
}

impl Chunk {
    /// Parses `key=value:key=value`. A known key with an invalid value rejects
    /// the whole sequence; unknown keys are skipped, as the protocol requires
    /// for extensibility.
    fn parse(metadata: &[u8]) -> Option<Self> {
        let mut chunk = Self {
            id: None,
            payload: Payload::Title,
            done: true,
            encoded: false,
            app_name: None,
            occasion: None,
            expiry: None,
        };
        for field in metadata.split(|&byte| byte == b':').filter(|field| !field.is_empty()) {
            let Some(split) = field.iter().position(|&byte| byte == b'=') else {
                continue;
            };
            let (key, value) = (&field[..split], &field[split + 1..]);
            match key {
                b"i" => chunk.id = Some(identifier(value)?).filter(|id| !id.is_empty()),
                b"d" => chunk.done = flag(value)?,
                b"e" => chunk.encoded = flag(value)?,
                b"p" => {
                    chunk.payload = match value {
                        b"title" => Payload::Title,
                        b"body" => Payload::Body,
                        b"close" => Payload::Close,
                        b"?" => Payload::Query,
                        _ => Payload::Unsupported,
                    }
                },
                b"o" => {
                    chunk.occasion = Some(match value {
                        b"always" => NotificationOccasion::Always,
                        b"unfocused" => NotificationOccasion::Unfocused,
                        b"invisible" => NotificationOccasion::Invisible,
                        _ => return None,
                    })
                },
                b"w" => {
                    let value: i64 = std::str::from_utf8(value).ok()?.parse().ok()?;
                    chunk.expiry = Some(match value {
                        -1 => NotificationExpiry::Default,
                        0 => NotificationExpiry::Never,
                        1.. => NotificationExpiry::Milliseconds(
                            u32::try_from(value).unwrap_or(u32::MAX),
                        ),
                        _ => return None,
                    })
                },
                b"f" => {
                    let name = String::from_utf8(Base64.decode(value).ok()?).ok()?;
                    chunk.app_name = Some(name).filter(|name| is_escape_safe(name));
                },
                _ => (),
            }
        }
        Some(chunk)
    }
}

/// An identifier is echoed back in replies, so only the protocol's safe set
/// is accepted rather than sanitized into something the application never sent.
fn identifier(value: &[u8]) -> Option<String> {
    value
        .iter()
        .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.'))
        .then(|| String::from_utf8_lossy(value).into_owned())
}

fn flag(value: &[u8]) -> Option<bool> {
    match value {
        b"0" => Some(false),
        b"1" => Some(true),
        _ => None,
    }
}

/// Text a desktop notification may show: no control characters beyond line
/// breaks and tabs, so nothing in it can act as an escape sequence elsewhere.
fn is_escape_safe(text: &str) -> bool {
    text.chars().all(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
}

/// A title or body as it arrives: decoded bytes, plus any base64 characters
/// that do not yet make a whole quartet.
#[derive(Clone, Debug, Default)]
struct Part {
    text: Vec<u8>,
    encoded: Vec<u8>,
}

impl Part {
    fn append(&mut self, payload: &[u8], encoded: bool) -> bool {
        if !encoded {
            // Plain text after an unfinished quartet would split it.
            if payload.len() > MAX_PLAIN_CHUNK || !self.encoded.is_empty() {
                return false;
            }
            self.text.extend_from_slice(payload);
            return true;
        }
        if payload.len() > MAX_ENCODED_CHUNK {
            return false;
        }
        self.encoded.extend_from_slice(payload);
        let whole = self.encoded.len() / 4 * 4;
        if Base64.decode_vec(&self.encoded[..whole], &mut self.text).is_err() {
            return false;
        }
        self.encoded.drain(..whole);
        true
    }

    fn finish(self) -> Option<String> {
        if !self.encoded.is_empty() {
            return None;
        }
        String::from_utf8(self.text).ok().filter(|text| is_escape_safe(text))
    }
}

#[derive(Clone, Debug, Default)]
struct Pending {
    title: Part,
    body: Part,
    app_name: Option<String>,
    occasion: NotificationOccasion,
    expiry: NotificationExpiry,
    /// A chunk broke a rule: the rest is consumed, and nothing is shown.
    failed: bool,
}

impl Pending {
    fn append(&mut self, chunk: &Chunk, payload: &[u8]) {
        if let Some(app_name) = &chunk.app_name {
            self.app_name = Some(app_name.clone());
        }
        self.occasion = chunk.occasion.unwrap_or(self.occasion);
        self.expiry = chunk.expiry.unwrap_or(self.expiry);
        if self.failed {
            return;
        }
        let part = if chunk.payload == Payload::Body { &mut self.body } else { &mut self.title };
        let size = |part: &Part| part.text.len() + part.encoded.len();
        self.failed = !part.append(payload, chunk.encoded)
            || size(&self.title) + size(&self.body) > MAX_NOTIFICATION;
    }

    fn finish(self, id: Option<String>) -> Option<DesktopNotification> {
        if self.failed {
            return None;
        }
        let (title, body) = (self.title.finish()?, self.body.finish()?);
        // A body alone is shown as the title, which every desktop displays.
        let (title, body) = if title.is_empty() { (body, String::new()) } else { (title, body) };
        if title.is_empty() {
            return None;
        }
        Some(DesktopNotification {
            id,
            title,
            body,
            app_name: self.app_name,
            occasion: self.occasion,
            expiry: self.expiry,
        })
    }
}

/// Notifications whose final chunk has not arrived, keyed by identifier; one
/// without an identifier is keyed by the empty string.
#[derive(Clone, Debug, Default)]
pub(super) struct Assembler {
    pending: Vec<(String, Pending)>,
}

impl Assembler {
    fn push(&mut self, chunk: &Chunk, payload: &[u8]) -> Option<DesktopNotification> {
        let key = chunk.id.as_deref().unwrap_or_default();
        let index = match self.pending.iter().position(|(id, _)| id == key) {
            Some(index) => index,
            None => {
                if self.pending.len() == MAX_PENDING {
                    self.pending.remove(0);
                }
                self.pending.push((key.to_owned(), Pending::default()));
                self.pending.len() - 1
            },
        };
        self.pending[index].1.append(chunk, payload);
        if !chunk.done {
            return None;
        }
        self.pending.remove(index).1.finish(chunk.id.clone())
    }

    fn close(&mut self, id: &str) {
        self.pending.retain(|(pending, _)| pending != id);
    }
}

impl<T: EventListener> Term<T> {
    pub(super) fn handle_desktop_notification(
        &mut self,
        metadata: &[u8],
        payload: &[u8],
        terminator: &str,
    ) {
        if !self.config.desktop_notifications {
            return;
        }
        let Some(chunk) = Chunk::parse(metadata) else {
            debug!("Ignoring OSC 99 with invalid metadata");
            return;
        };
        match chunk.payload {
            Payload::Title | Payload::Body => {
                if let Some(notification) = self.protocol.notifications.push(&chunk, payload) {
                    self.event_proxy.send_event(Event::DesktopNotification(notification));
                }
            },
            // A shown notification cannot be withdrawn, but one still being
            // assembled can be abandoned.
            Payload::Close => {
                if let Some(id) = &chunk.id {
                    self.protocol.notifications.close(id);
                }
            },
            Payload::Query => {
                let mut reply = String::from("\x1b]99;");
                if let Some(id) = &chunk.id {
                    let _ = write!(reply, "i={id}:");
                }
                let _ = write!(reply, "p=?;{CAPABILITIES}{terminator}");
                self.write_reply(reply);
            },
            Payload::Unsupported => (),
        }
    }
}

#[cfg(test)]
#[path = "../tests/term/notification.rs"]
mod tests;
