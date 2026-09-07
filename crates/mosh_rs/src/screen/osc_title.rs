//! Picking the window title out of the host's byte stream.
//!
//! The server sends the title inside the ordinary host diff: when it
//! changes, `Display::new_frame` writes `ESC ] 0 ; <title> BEL` in
//! among the cell updates. So the bytes arrive; the question is whether
//! anything is listening.
//!
//! `vt100` is not. It consumes the sequence and exposes no title, so a
//! screen built on it would drop the title on the floor and the local
//! terminal would keep whatever it had before the session started. This
//! is the smallest thing that listens: one sequence family, tracked
//! across feeds because a diff could in principle end mid-sequence.

/// Where the scanner is in a sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    /// Ordinary output.
    #[default]
    Ground,
    /// Just saw `ESC`.
    Escape,
    /// Inside `ESC ]`, reading the number before the `;`.
    Command,
    /// Reading the title itself.
    Text,
    /// Inside an OSC we do not care about; skip to the terminator.
    Skip,
    /// Saw `ESC` while reading text or skipping: `ESC \` ends it.
    Terminator,
}

/// Reads window titles out of a byte stream and ignores everything else.
#[derive(Debug, Clone, Default)]
pub struct TitleScanner {
    state: State,
    /// The OSC number, as digits arrive.
    command: u16,
    pending: String,
    title: Option<String>,
}

/// The OSC numbers that carry a window title. `0` sets the icon name
/// and the title together, `2` sets the title alone; `1` is the icon
/// name only, which is not a window title and is deliberately ignored.
const OSC_ICON_AND_TITLE: u16 = 0;
const OSC_TITLE: u16 = 2;

impl TitleScanner {
    /// The most recent title the host asked for.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Feed the same bytes the screen is fed.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.step(byte);
        }
    }

    fn step(&mut self, byte: u8) {
        match self.state {
            State::Ground => {
                if byte == 0x1b {
                    self.state = State::Escape;
                }
            }
            State::Escape => {
                if byte == b']' {
                    self.state = State::Command;
                    self.command = 0;
                    self.pending.clear();
                } else {
                    // Any other escape sequence is somebody else's.
                    self.state = State::Ground;
                }
            }
            State::Command => match byte {
                b'0'..=b'9' => {
                    self.command = self.command.saturating_mul(10) + u16::from(byte - b'0');
                }
                b';' => {
                    self.state = if matches!(self.command, OSC_ICON_AND_TITLE | OSC_TITLE) {
                        State::Text
                    } else {
                        State::Skip
                    };
                }
                // A malformed OSC: give up rather than swallow the rest
                // of the stream looking for a terminator.
                _ => self.state = State::Ground,
            },
            State::Text | State::Skip => match byte {
                // BEL ends it. mosh's own comment says ST is more
                // correct and BEL more widely supported, and it sends
                // BEL; both are accepted here.
                0x07 => self.finish(),
                0x1b => self.state = State::Terminator,
                _ if self.state == State::Text => {
                    // A title is text; anything that is not is a sign
                    // the sequence was never a title at all.
                    if byte < 0x20 {
                        self.state = State::Ground;
                    } else {
                        self.pending.push(char::from(byte));
                    }
                }
                _ => {}
            },
            State::Terminator => {
                if byte == b'\\' {
                    self.finish();
                } else {
                    // `ESC` inside a title that was not the start of ST.
                    // Not a title we can trust.
                    self.state = State::Ground;
                }
            }
        }
    }

    fn finish(&mut self) {
        if self.state != State::Skip && self.state != State::Ground {
            self.title = Some(std::mem::take(&mut self.pending));
        }
        self.pending.clear();
        self.state = State::Ground;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned(bytes: &[u8]) -> Option<String> {
        let mut scanner = TitleScanner::default();
        scanner.feed(bytes);
        scanner.title().map(str::to_string)
    }

    #[test]
    fn the_sequence_mosh_actually_sends() {
        // `Display::new_frame` writes exactly this when the title
        // changes: OSC 0, the title, BEL.
        assert_eq!(
            scanned(b"\x1b]0;wilson@host: ~\x07").as_deref(),
            Some("wilson@host: ~")
        );
    }

    #[test]
    fn osc_two_is_the_window_title_too() {
        assert_eq!(
            scanned(b"\x1b]2;just the title\x07").as_deref(),
            Some("just the title")
        );
    }

    #[test]
    fn the_icon_name_alone_is_not_a_window_title() {
        // OSC 1 sets the icon name, which is a different thing and not
        // what goes in a title bar.
        assert_eq!(scanned(b"\x1b]1;icon only\x07"), None);
    }

    #[test]
    fn the_string_terminator_ends_it_too() {
        // ST is the more correct ending; mosh sends BEL because it is
        // more widely supported, but a server is free to send either.
        assert_eq!(scanned(b"\x1b]0;via ST\x1b\\").as_deref(), Some("via ST"));
    }

    #[test]
    fn a_title_split_across_feeds_still_arrives() {
        // A host diff could in principle end mid-sequence, and a
        // scanner that reset per call would lose the title silently.
        let mut scanner = TitleScanner::default();
        scanner.feed(b"\x1b]0;dep");
        assert_eq!(scanner.title(), None, "not finished yet");
        scanner.feed(b"loy@prod\x07");
        assert_eq!(scanner.title(), Some("deploy@prod"));
    }

    #[test]
    fn the_newest_title_wins() {
        let mut scanner = TitleScanner::default();
        scanner.feed(b"\x1b]0;first\x07");
        scanner.feed(b"\x1b]0;second\x07");
        assert_eq!(scanner.title(), Some("second"));
    }

    #[test]
    fn ordinary_output_is_left_alone() {
        assert_eq!(scanned(b"hello\r\nworld\x1b[1;1H\x1b[0m"), None);
    }

    #[test]
    fn a_title_surrounded_by_ordinary_output_is_still_found() {
        assert_eq!(
            scanned(b"before\x1b]0;middle\x07after").as_deref(),
            Some("middle")
        );
    }

    #[test]
    fn an_empty_title_is_a_title() {
        // Clearing the title is a thing a shell does, and reporting
        // `None` would mean "never set" rather than "set to nothing".
        assert_eq!(scanned(b"\x1b]0;\x07").as_deref(), Some(""));
    }

    #[test]
    fn a_malformed_sequence_does_not_swallow_the_stream() {
        // The danger with a terminator-seeking parser: one bad byte and
        // it eats everything after it, including the next real title.
        let mut scanner = TitleScanner::default();
        scanner.feed(b"\x1b]9x;junk");
        scanner.feed(b"\x1b]0;recovered\x07");
        assert_eq!(scanner.title(), Some("recovered"));
    }

    #[test]
    fn a_control_byte_inside_a_title_abandons_it() {
        let mut scanner = TitleScanner::default();
        scanner.feed(b"\x1b]0;bro\nken\x07");
        assert_eq!(scanner.title(), None);
        // And the scanner is usable again afterwards.
        scanner.feed(b"\x1b]0;fine\x07");
        assert_eq!(scanner.title(), Some("fine"));
    }
}
