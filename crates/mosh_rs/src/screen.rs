//! What a state number resolves to: a screen.
//!
//! The protocol needs surprisingly little from a terminal emulator,
//! and naming exactly what it needs is what lets an application bring
//! its own. A host diff is escape bytes to be fed somewhere; a state
//! is a snapshot that a later diff may be computed from, so it must be
//! copyable; and the prediction engine has to read back cells and the
//! cursor to judge its own guesses. That is the whole of [`Screen`].
//!
//! Producing BYTES for a real terminal is a separate concern, and
//! deliberately a separate trait. A client writing to a tty needs it;
//! an application that owns the grid it draws from does not, because
//! showing the newest state is just drawing that state.

/// A colour, in the three forms a terminal understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// Whatever the terminal's default is.
    #[default]
    Default,
    /// One of the 256 indexed colours.
    Indexed(u8),
    /// Direct colour.
    Rgb(u8, u8, u8),
}

/// How a cell should be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rendition {
    /// Foreground colour.
    pub fg: Color,
    /// Background colour.
    pub bg: Color,
    /// SGR 1.
    pub bold: bool,
    /// SGR 2.
    pub dim: bool,
    /// SGR 3.
    pub italic: bool,
    /// SGR 4.
    pub underline: bool,
    /// SGR 7.
    pub inverse: bool,
}

impl Rendition {
    /// The escape that sets a terminal to these attributes from any
    /// starting state. It always begins with a reset, because the pen
    /// left behind by whatever was drawn last is not ours to assume.
    ///
    /// `underline` forces the attribute on regardless: that is how a
    /// prediction is marked as a guess.
    pub fn sgr(&self, underline: bool) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("\x1b[0");
        if self.bold {
            out.push_str(";1");
        }
        if self.dim {
            out.push_str(";2");
        }
        if self.italic {
            out.push_str(";3");
        }
        if self.underline || underline {
            out.push_str(";4");
        }
        if self.inverse {
            out.push_str(";7");
        }
        for (color, base) in [(self.fg, 30u16), (self.bg, 40u16)] {
            match color {
                Color::Default => {}
                // The eight named colours, then their bright forms,
                // then the 256-colour and direct-colour escapes.
                Color::Indexed(i) if i < 8 => {
                    let _ = write!(out, ";{}", base + u16::from(i));
                }
                Color::Indexed(i) if i < 16 => {
                    let _ = write!(out, ";{}", base + 60 + u16::from(i - 8));
                }
                Color::Indexed(i) => {
                    let _ = write!(out, ";{};5;{i}", base + 8);
                }
                Color::Rgb(r, g, b) => {
                    let _ = write!(out, ";{};2;{r};{g};{b}", base + 8);
                }
            }
        }
        out.push('m');
        out
    }
}

/// One cell's worth of screen: the grapheme it shows and how.
///
/// Equality covers both, because that is what decides whether anything
/// needs painting. Whether a PREDICTION came true is a different
/// question, answered by [`Self::contents_match`]: the server is free
/// to colour our character differently and still have echoed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cell {
    /// The grapheme shown, empty for a cell nothing has written.
    pub contents: String,
    /// How it is drawn.
    pub rendition: Rendition,
}

impl Cell {
    /// Nothing a user would see. An empty cell and one holding a space
    /// look identical, so both count, as does the no-break space.
    pub fn is_blank(&self) -> bool {
        self.contents.is_empty() || self.contents == " " || self.contents == "\u{a0}"
    }

    /// Same character, renditions aside. Two blanks match however they
    /// are spelled.
    pub fn contents_match(&self, other: &Self) -> bool {
        (self.is_blank() && other.is_blank()) || self.contents == other.contents
    }

    /// What a terminal must be sent to make a cell show this. An empty
    /// cell is written as a space: a screen blanks a cell by holding an
    /// empty one, and a terminal blanks it by being given something
    /// blank to draw.
    pub fn glyph(&self) -> &str {
        if self.contents.is_empty() {
            " "
        } else {
            &self.contents
        }
    }
}

/// A screen the client can hold, copy, feed and read back.
///
/// `Clone` is not an ergonomic nicety: a host diff names the state it
/// was computed FROM, and that state has to survive being diffed
/// against, because the server may compute another diff from the same
/// place while an acknowledgement is in flight.
pub trait Screen: Clone {
    /// Feed host output. These are ECMA-48 escape bytes rendered by the
    /// SERVER's own emulator, so this is exactly what feeding a PTY
    /// would be.
    fn feed(&mut self, bytes: &[u8]);

    /// Change the screen's shape.
    fn resize(&mut self, rows: u16, cols: u16);

    // Deliberately two accessors rather than a tuple: every emulator
    // orders that pair differently and a silent transposition is the
    // kind of bug that only shows on a non-square terminal.
    /// How many rows the screen has.
    fn rows(&self) -> u16;
    /// How many columns the screen has.
    fn cols(&self) -> u16;

    /// Where the cursor is, as `(row, col)`.
    fn cursor(&self) -> (u16, u16);

    /// What one cell holds. Off-screen positions read as blank rather
    /// than failing: every caller here already bounds-checks, and a
    /// second way to say "nothing there" would only be a second thing
    /// to get wrong.
    fn cell(&self, row: u16, col: u16) -> Cell;

    /// The screen as plain text, for tests and for anything reporting
    /// what the user should be seeing.
    fn text(&self) -> String;

    /// The window title the host has asked for, if this screen tracks
    /// one.
    ///
    /// `None` by default, and that is a real answer rather than a
    /// placeholder: an emulator that does not model the title has
    /// nothing to report, and a caller must not invent one. It is also
    /// why mosh's `[mosh] ` title prefix is left to the caller. Whoever
    /// owns the window owns its title, and an application embedding
    /// this already puts its own text in a tab.
    fn title(&self) -> Option<String> {
        None
    }

    /// Draw a predicted cell, and put the cursor somewhere.
    ///
    /// The default renders both as escape bytes through [`Self::feed`],
    /// which any implementation can do. One that OWNS its grid should
    /// override this and write the cells directly, which is what mosh
    /// itself does: it is cheaper, and it avoids the two places where
    /// painting by escape has to hold back (the last column, where a
    /// write would arm the terminal's pending wrap, and column zero,
    /// where there is no pen to inherit from).
    fn draw_overlay(&mut self, cells: &[OverlayCell], cursor: OverlayCursor) {
        let mut out = Vec::new();
        let cols = self.cols();
        for painted in cells {
            if painted.col + 1 >= cols {
                continue;
            }
            push_cup(&mut out, painted.row, painted.col);
            out.extend_from_slice(painted.cell.rendition.sgr(painted.underline).as_bytes());
            out.extend_from_slice(painted.cell.glyph().as_bytes());
        }
        if !out.is_empty() {
            // Leave the pen where the confirmed screen would have it
            // rather than in the last prediction's colours.
            out.extend_from_slice(b"\x1b[m");
        }
        match cursor {
            OverlayCursor::Unchanged => {}
            OverlayCursor::At(row, col) => push_cup(&mut out, row, col),
            // Feeding the escape is enough: the emulator tracks cursor
            // visibility, so the difference against what is displayed
            // carries both the hide and, when the overlay goes away,
            // the show.
            OverlayCursor::Hidden => out.extend_from_slice(b"\x1b[?25l"),
        }
        if !out.is_empty() {
            self.feed(&out);
        }
    }
}

/// What should become of the cursor while an overlay is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayCursor {
    /// Leave it where the screen already has it.
    #[default]
    Unchanged,
    /// Put it here, as `(row, col)`.
    At(u16, u16),
    /// Do not draw it at all: something is painted over the cell it
    /// would sit in, and a cursor blinking inside a status message
    /// reads as a glitch.
    Hidden,
}

/// One cell an overlay wants painted.
#[derive(Debug, Clone)]
pub struct OverlayCell {
    /// Row to paint on.
    pub row: u16,
    /// Column to paint on.
    pub col: u16,
    /// What to paint there.
    pub cell: Cell,
    /// Mark it visibly as a guess.
    pub underline: bool,
}

/// Turning a screen into bytes for a real terminal.
///
/// Separate from [`Screen`] because it is a separate job. A client
/// writing to a tty needs it; an application that owns the grid it
/// draws from never calls it, because showing the newest state is just
/// drawing that state.
pub trait DiffScreen: Screen {
    /// The bytes that turn a terminal showing `previous` into one
    /// showing `self`, and nothing more.
    fn diff_from(&self, previous: &Self) -> Vec<u8>;

    /// The whole screen, for a terminal whose state cannot be known.
    fn repaint(&self) -> Vec<u8>;
}

fn push_cup(out: &mut Vec<u8>, row: u16, col: u16) {
    out.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());
}

#[cfg(feature = "vt100-screen")]
mod osc_title;
#[cfg(feature = "vt100-screen")]
mod vt100_screen;
#[cfg(feature = "vt100-screen")]
pub use vt100_screen::Vt100Screen;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanks_match_however_they_are_spelled() {
        let empty = Cell::default();
        let space = Cell {
            contents: " ".into(),
            rendition: Rendition::default(),
        };
        assert!(empty.contents_match(&space));
        assert_eq!(empty.glyph(), " ");
        assert_eq!(space.glyph(), " ");
    }

    #[test]
    fn flagging_underlines_whatever_the_rendition_says() {
        let plain = Rendition::default();
        assert!(!plain.sgr(false).contains(";4"));
        assert!(plain.sgr(true).contains(";4"));
    }

    #[test]
    fn colours_render_to_the_escapes_a_terminal_expects() {
        let named = Rendition {
            fg: Color::Indexed(1),
            ..Rendition::default()
        };
        assert!(named.sgr(false).contains(";31"));
        let bright = Rendition {
            fg: Color::Indexed(10),
            ..Rendition::default()
        };
        assert!(bright.sgr(false).contains(";92"));
        let indexed = Rendition {
            fg: Color::Indexed(27),
            ..Rendition::default()
        };
        assert!(indexed.sgr(false).contains(";38;5;27"));
        let direct = Rendition {
            bg: Color::Rgb(10, 20, 30),
            ..Rendition::default()
        };
        assert!(direct.sgr(false).contains(";48;2;10;20;30"));
        // The default colour says nothing at all, which is what lets a
        // reset carry it.
        assert_eq!(Rendition::default().sgr(false), "\x1b[0m");
    }
}
