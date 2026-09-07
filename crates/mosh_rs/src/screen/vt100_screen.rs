//! The default screen, on `vt100`.
//!
//! This is what the standalone client uses, and the only emulator in
//! that binary. An application embedding the protocol alongside its own
//! terminal should implement [`Screen`](super::Screen) over that one
//! instead, so its binary carries one emulator rather than two.

use super::osc_title::TitleScanner;
use super::{Cell, Color, DiffScreen, Rendition, Screen};

/// A screen backed by `vt100`.
///
/// The SCREEN is stored rather than the parser that built it, because a
/// state must be copyable. vt100's `Parser` is not clonable, so feeding
/// bytes means wrapping the stored screen in a fresh parser and taking
/// the result back; the state bytes carry everything the parser needs
/// to continue from exactly where the screen left off.
#[derive(Clone)]
pub struct Vt100Screen {
    inner: vt100::Screen,
    /// vt100 consumes the window title and exposes none, so it is read
    /// off the same bytes on the way past. Without this the title the
    /// host set never reaches the local terminal at all.
    titles: TitleScanner,
}

impl Vt100Screen {
    /// A blank screen of the given shape.
    pub fn new(rows: u16, cols: u16) -> Self {
        // No scrollback: mosh synchronizes the visible screen, and the
        // real terminal keeps whatever scrollback it wants.
        Self {
            inner: vt100::Parser::new(rows, cols, 0).screen().clone(),
            titles: TitleScanner::default(),
        }
    }

    /// The underlying screen, for a caller that wants vt100's own API.
    pub fn inner(&self) -> &vt100::Screen {
        &self.inner
    }
}

fn color_of(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

impl Screen for Vt100Screen {
    fn feed(&mut self, bytes: &[u8]) {
        self.titles.feed(bytes);
        let (rows, cols) = self.inner.size();
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(&self.inner.state_formatted());
        parser.process(bytes);
        self.inner = parser.screen().clone();
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.inner.set_size(rows, cols);
    }

    fn rows(&self) -> u16 {
        self.inner.size().0
    }

    fn cols(&self) -> u16 {
        self.inner.size().1
    }

    fn cursor(&self) -> (u16, u16) {
        self.inner.cursor_position()
    }

    fn cell(&self, row: u16, col: u16) -> Cell {
        match self.inner.cell(row, col) {
            Some(cell) => Cell {
                contents: cell.contents().to_string(),
                rendition: Rendition {
                    fg: color_of(cell.fgcolor()),
                    bg: color_of(cell.bgcolor()),
                    bold: cell.bold(),
                    dim: cell.dim(),
                    italic: cell.italic(),
                    underline: cell.underline(),
                    inverse: cell.inverse(),
                },
            },
            None => Cell::default(),
        }
    }

    fn text(&self) -> String {
        self.inner.contents()
    }

    fn title(&self) -> Option<String> {
        self.titles.title().map(str::to_string)
    }
}

impl DiffScreen for Vt100Screen {
    fn diff_from(&self, previous: &Self) -> Vec<u8> {
        self.inner.contents_diff(&previous.inner)
    }

    fn repaint(&self) -> Vec<u8> {
        self.inner.contents_formatted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeding_continues_from_where_the_screen_left_off() {
        let mut screen = Vt100Screen::new(3, 20);
        screen.feed(b"hello");
        assert_eq!(screen.text().trim(), "hello");
        screen.feed(b" world");
        assert_eq!(screen.text().trim(), "hello world");
        assert_eq!(screen.cursor(), (0, 11));
    }

    #[test]
    fn a_copy_is_independent_of_the_original() {
        let mut screen = Vt100Screen::new(3, 20);
        screen.feed(b"base");
        let mut branch = screen.clone();
        branch.feed(b"-one");
        // The original is exactly what a later diff from the same state
        // needs it to be.
        assert_eq!(screen.text().trim(), "base");
        assert_eq!(branch.text().trim(), "base-one");
    }

    #[test]
    fn a_cell_reads_back_with_its_renditions() {
        let mut screen = Vt100Screen::new(3, 20);
        screen.feed(b"\x1b[1;4;31mZ");
        let cell = screen.cell(0, 0);
        assert_eq!(cell.contents, "Z");
        assert!(cell.rendition.bold);
        assert!(cell.rendition.underline);
        assert_eq!(cell.rendition.fg, Color::Indexed(1));
        // And off the screen reads as blank rather than failing.
        assert!(screen.cell(9, 9).is_blank());
    }

    #[test]
    fn a_rendition_round_trips_through_its_own_escape() {
        let mut screen = Vt100Screen::new(3, 20);
        screen.feed(b"\x1b[1;4;31;48;5;27mZ");
        let original = screen.cell(0, 0);

        let mut replay = Vt100Screen::new(3, 20);
        replay.feed(original.rendition.sgr(false).as_bytes());
        replay.feed(b"Z");
        assert_eq!(original, replay.cell(0, 0));
    }

    #[test]
    fn the_diff_reproduces_the_screen_it_came_from() {
        let mut first = Vt100Screen::new(3, 20);
        first.feed(b"one");
        let mut second = first.clone();
        second.feed(b"\r\ntwo");

        let mut mirror = first.clone();
        mirror.feed(&second.diff_from(&first));
        assert_eq!(mirror.text(), second.text());
    }

    #[test]
    fn the_default_overlay_paints_by_escape_and_skips_the_last_column() {
        let mut screen = Vt100Screen::new(3, 6);
        screen.draw_overlay(
            &[
                super::super::OverlayCell {
                    row: 0,
                    col: 0,
                    cell: Cell {
                        contents: "a".into(),
                        rendition: Rendition::default(),
                    },
                    underline: false,
                },
                // The last column: a write there would arm the pending
                // wrap, so the default implementation declines it.
                super::super::OverlayCell {
                    row: 0,
                    col: 5,
                    cell: Cell {
                        contents: "z".into(),
                        rendition: Rendition::default(),
                    },
                    underline: false,
                },
            ],
            super::super::OverlayCursor::At(1, 2),
        );
        assert_eq!(screen.cell(0, 0).contents, "a");
        assert!(screen.cell(0, 5).is_blank());
        assert_eq!(screen.cursor(), (1, 2));
    }
}
