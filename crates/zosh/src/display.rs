//! The terminal-facing part of the Mosh screen diff.
//!
//! `mosh-rs` keeps the screen state and can diff its cells, but a cell diff
//! alone cannot preserve a terminal's scrollback. Stock Mosh recognizes a
//! screen that moved upward, scrolls the real terminal, and only repaints the
//! rows that are new. This wrapper keeps that display optimization while still
//! using `mosh-rs` for the protocol screen and prediction state. It also
//! carries the patched server's clear-scrollback generation in each screen,
//! so a remote `CSI 3 J` clears the real terminal once when that state is shown.

use std::io;

use mosh_rs::{DiffScreen, Screen};

const DISPLAY_OPEN: &[u8] = b"\x1b[?1h\x1b[?5l\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[?25l";
const DISPLAY_RESIZE: &[u8] = b"\x1b[r\x1b[0m\x1b[H\x1b[2J";
const CLEAR_SCROLLBACK: &[u8] = b"\x1b[3J";

type Vt100Screen = mosh_rs::screen::Vt100Screen;

/// A protocol screen whose display diff also preserves physical scrollback.
#[derive(Clone)]
pub(crate) struct DisplayScreen {
    inner: Vt100Screen,
    received_output: bool,
    scrollback: crate::scrollback::ScrollbackState,
}

impl DisplayScreen {
    pub(crate) fn new(rows: u16, columns: u16) -> Self {
        Self {
            inner: Vt100Screen::new(rows, columns),
            received_output: false,
            scrollback: crate::scrollback::ScrollbackState::default(),
        }
    }
}

impl Screen for DisplayScreen {
    fn feed(&mut self, bytes: &[u8]) {
        self.scrollback.feed(bytes);
        self.inner.feed(bytes);
        self.received_output = true;
    }

    fn resize(&mut self, rows: u16, columns: u16) {
        self.inner.resize(rows, columns);
    }

    fn rows(&self) -> u16 {
        self.inner.rows()
    }

    fn cols(&self) -> u16 {
        self.inner.cols()
    }

    fn cursor(&self) -> (u16, u16) {
        self.inner.cursor()
    }

    fn cell(&self, row: u16, column: u16) -> mosh_rs::Cell {
        self.inner.cell(row, column)
    }

    fn text(&self) -> String {
        self.inner.text()
    }

    fn title(&self) -> Option<String> {
        self.inner.title()
    }
}

impl DiffScreen for DisplayScreen {
    fn diff_from(&self, previous: &Self) -> Vec<u8> {
        let mut output = Vec::new();
        let resized = previous.inner.inner().size() != self.inner.inner().size();
        if self.clears_scrollback_since(previous) {
            // Repaint from the resulting frame. A cell diff may leave old
            // terminal contents outside the protocol's current screen.
            output.extend_from_slice(DISPLAY_RESIZE);
            output.extend_from_slice(CLEAR_SCROLLBACK);
            output.extend_from_slice(&self.repaint());
            output.extend_from_slice(&self.input_modes_diff(previous));
            return output;
        }
        if !previous.received_output {
            // `open` hides the real cursor before the first frame. The
            // protocol screen starts from vt100's default (visible) cursor,
            // so its ordinary state diff cannot see that physical change.
            // Restore the server's cursor state explicitly on the first
            // frame, just as stock Mosh does after its initial paint.
            output.extend_from_slice(if self.inner.inner().hide_cursor() {
                b"\x1b[?25l"
            } else {
                b"\x1b[?25h"
            });
        }
        if resized {
            // The real terminal has already changed geometry, but its
            // contents are implementation-defined: some terminals reflow
            // them and others clip them. Stock Mosh treats a size change as
            // a new frame, clears the display, and repaints every row. A
            // normal state diff compares mismatched grids and leaves a TUI
            // such as htop shredded after a maximize/restore cycle.
            output.extend_from_slice(DISPLAY_RESIZE);
            let blank = Vt100Screen::new(self.rows(), self.cols());
            output.extend_from_slice(&self.inner.inner().contents_diff(blank.inner()));
            output.extend_from_slice(&self.inner.inner().input_mode_diff(previous.inner.inner()));
            return output;
        }
        let mut physical_previous = previous.inner.clone();
        if let Some(plan) = scroll_plan(previous, self) {
            let scroll = plan.sequence(previous.inner.cursor().0, self.rows());
            physical_previous.feed(&scroll);
            output.extend_from_slice(&scroll);
        }
        output.extend_from_slice(&self.inner.inner().state_diff(physical_previous.inner()));
        output
    }

    fn repaint(&self) -> Vec<u8> {
        // A resize does not reset the terminal's input modes. Repainting
        // them here would turn off application cursor/keypad mode (and
        // bracketed paste and mouse reporting) even though the preceding
        // frame may have enabled them. Stock Mosh repaints the contents
        // only; ordinary diffs still carry input-mode changes.
        self.inner.inner().contents_formatted()
    }
}

impl DisplayScreen {
    pub(crate) fn clears_scrollback_since(&self, previous: &Self) -> bool {
        self.scrollback.generation != previous.scrollback.generation
    }

    pub(crate) fn input_modes(&self) -> Vec<u8> {
        self.inner.inner().input_mode_formatted()
    }

    pub(crate) fn input_modes_diff(&self, previous: &Self) -> Vec<u8> {
        self.inner.inner().input_mode_diff(previous.inner.inner())
    }
}

pub(crate) fn clear_scrollback_sequence() -> &'static [u8] {
    CLEAR_SCROLLBACK
}

/// Initialize the Mosh display without choosing whether the terminal uses an
/// alternate screen. `--no-init` omits only the alternate-screen transition;
/// stock Mosh still homes and clears the visible page before its first frame.
pub(crate) fn open(output: &mut impl io::Write) -> io::Result<()> {
    output.write_all(DISPLAY_OPEN)?;
    output.flush()
}

/// Clear the real terminal after a server-reported geometry change, then
/// paint the complete protocol screen into the new shape.
pub(crate) fn repaint_after_resize(
    output: &mut impl io::Write,
    repaint: &[u8],
    input_modes: &[u8],
    scrollback_clear: &[u8],
) -> io::Result<()> {
    output.write_all(DISPLAY_RESIZE)?;
    output.write_all(repaint)?;
    output.write_all(input_modes)?;
    output.write_all(scrollback_clear)?;
    output.flush()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScrollPlan {
    lines_scrolled: u16,
    scroll_height: u16,
}

fn scroll_plan(previous: &DisplayScreen, current: &DisplayScreen) -> Option<ScrollPlan> {
    if !previous.received_output || previous.inner.inner().size() != current.inner.inner().size() {
        return None;
    }
    let height = current.rows();
    if height < 2 {
        return None;
    }
    // A matching first row is the common non-scroll case. Stock Mosh checks
    // row zero before looking for a shifted match; without this guard a
    // screen that is merely growing through otherwise blank rows looks like
    // it scrolled and gets needlessly repainted.
    if rows_equal(&current.inner, 0, &previous.inner, 0) {
        return None;
    }
    for lines_scrolled in 1..height {
        if !rows_equal(&current.inner, 0, &previous.inner, lines_scrolled) {
            continue;
        }
        let mut scroll_height = 1;
        while lines_scrolled + scroll_height < height
            && rows_equal(
                &current.inner,
                scroll_height,
                &previous.inner,
                lines_scrolled + scroll_height,
            )
        {
            scroll_height += 1;
        }
        return Some(ScrollPlan {
            lines_scrolled,
            scroll_height,
        });
    }
    None
}

fn rows_equal(
    current: &Vt100Screen,
    current_row: u16,
    previous: &Vt100Screen,
    previous_row: u16,
) -> bool {
    let current_inner = current.inner();
    let previous_inner = previous.inner();
    if current_inner.row_wrapped(current_row) != previous_inner.row_wrapped(previous_row) {
        return false;
    }
    (0..current.cols()).all(|column| {
        current_inner.cell(current_row, column) == previous_inner.cell(previous_row, column)
    })
}

impl ScrollPlan {
    fn sequence(self, previous_cursor_row: u16, height: u16) -> Vec<u8> {
        let bottom = self.lines_scrolled + self.scroll_height - 1;
        let mut output = Vec::new();
        if self.lines_scrolled + self.scroll_height == height && previous_cursor_row == height - 1 {
            output.push(b'\r');
            output.extend(std::iter::repeat_n(b'\n', usize::from(self.lines_scrolled)));
        } else {
            output.extend_from_slice(
                format!("\x1b[1;{}r\x1b[{};1H", bottom + 1, bottom + 1).as_bytes(),
            );
            output.extend(std::iter::repeat_n(b'\n', usize::from(self.lines_scrolled)));
            output.extend_from_slice(b"\x1b[r");
        }
        output
    }
}

#[cfg(test)]
#[path = "tests/display.rs"]
mod tests;
