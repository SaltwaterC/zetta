//! The small status bar the stock Mosh client uses when a link goes quiet.

use mosh_rs::LinkHealth;
use mosh_rs::screen::{Cell, Color, OverlayCell, Rendition};

const SERVER_LATE_MS: u64 = 6_500;
const REPLY_LATE_MS: u64 = 10_000;
const MESSAGE_MS: u64 = 1_000;

pub(crate) struct Notifier {
    message: Option<String>,
    expires_at: Option<u64>,
    quit_hint: String,
}

impl Notifier {
    pub(crate) fn new(escape_name: Option<&str>) -> Self {
        Self {
            message: None,
            expires_at: None,
            quit_hint: escape_name
                .map(|name| format!(" [To quit: {name} .]"))
                .unwrap_or_default(),
        }
    }

    pub(crate) fn say(&mut self, message: impl Into<String>, now: u64) {
        self.message = Some(message.into());
        self.expires_at = Some(now + MESSAGE_MS);
    }

    pub(crate) fn bar(&mut self, health: LinkHealth, now: u64, columns: u16) -> Vec<OverlayCell> {
        self.expire(now);
        let no_contact = health.since_heard_ms > SERVER_LATE_MS;
        let no_reply = health.since_ack_ms > REPLY_LATE_MS;
        let line = match (&self.message, no_contact || no_reply) {
            (None, false) => return Vec::new(),
            (None, true) => {
                let (elapsed, what) = late_kind(health, no_contact);
                format!(
                    "zosh: Last {what} {} ago.{}",
                    format_duration(elapsed / 1_000),
                    self.quit_hint
                )
            }
            (Some(message), false) => format!("zosh: {message}{}", self.quit_hint),
            (Some(message), true) => {
                let (elapsed, what) = late_kind(health, no_contact);
                format!(
                    "zosh: {message} ({} without {what}.){}",
                    format_duration(elapsed / 1_000),
                    self.quit_hint
                )
            }
        };
        let style = Rendition {
            fg: Color::Indexed(7),
            bg: Color::Indexed(4),
            bold: true,
            ..Rendition::default()
        };
        (0..columns)
            .map(|column| OverlayCell {
                row: 0,
                col: column,
                cell: Cell {
                    contents: line
                        .chars()
                        .nth(usize::from(column))
                        .map_or_else(|| " ".to_owned(), |character| character.to_string()),
                    rendition: style,
                },
                underline: false,
            })
            .collect()
    }

    fn expire(&mut self, now: u64) {
        if self.expires_at.is_some_and(|expires_at| now >= expires_at) {
            self.message = None;
            self.expires_at = None;
        }
    }
}

fn late_kind(health: LinkHealth, no_contact: bool) -> (u64, &'static str) {
    if !no_contact && health.since_ack_ms > REPLY_LATE_MS {
        (health.since_ack_ms, "reply")
    } else {
        (health.since_heard_ms, "contact")
    }
}

fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} seconds")
    } else if seconds < 3_600 {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3_600,
            (seconds / 60) % 60,
            seconds % 60
        )
    }
}

#[cfg(test)]
#[path = "tests/notification.rs"]
mod tests;
