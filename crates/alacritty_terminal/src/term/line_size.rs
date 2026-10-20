//! DEC double-width and double-height lines (DECSWL, DECDWL, DECDHL).
//!
//! The size is a property of the row (see [`LineSize`]); what the terminal adds
//! is that a doubled row shows only the first half of its cells, so writing
//! stops at that half. Cells past it keep their contents, as in xterm, and come
//! back into view if the row returns to single width.

use super::*;
use crate::grid::LineSize;

impl<T> Term<T> {
    /// Clears `doubled_rows` once no row of the screen is doubled: scrolling
    /// a doubled row away resets it, and so does erasing it whole. Run after
    /// anything that replaces the screen's rows wholesale, since that can also
    /// bring a doubled row in, and after scrolling while one is known.
    pub(super) fn recheck_doubled_rows(&mut self) {
        self.doubled_rows = (0..self.screen_lines() as i32)
            .any(|line| self.grid[Line(line)].line_size() != LineSize::Single);
    }
}

impl<T: EventListener> Term<T> {
    pub(super) fn set_line_size(&mut self, line_size: LineSize) {
        self.doubled_rows |= line_size != LineSize::Single;
        let line = self.grid.cursor.point.line;
        self.grid[line].set_line_size(line_size);
        self.clamp_cursor_to_line_size();
        self.mark_fully_damaged();
    }

    /// Moves a cursor left in a doubled row's hidden half onto its last
    /// visible column, where the next character lands.
    #[inline]
    pub(super) fn clamp_cursor_to_line_size(&mut self) {
        if !self.doubled_rows {
            return;
        }
        let point = self.grid.cursor.point;
        let visible = self.grid[point.line].line_size().visible_columns(self.columns());
        if point.column.0 >= visible {
            self.grid.cursor.point.column = Column(visible - 1);
        }
    }

    /// Erasing a whole row also returns it to single width; erasing part of
    /// one keeps its size.
    pub(super) fn reset_line_size(&mut self, line: Line) {
        self.grid[line].set_line_size(LineSize::Single);
    }
}

#[cfg(test)]
#[path = "../tests/term/line_size.rs"]
mod tests;
