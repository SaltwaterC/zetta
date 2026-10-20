//! Horizontal scrolling regions and relative cursor movement.
//!
//! Full-width regions keep the grid's row-rotation and scrollback fast path.
//! Rectangular regions move only their columns and never create history rows.

use super::*;

impl<T> Term<T> {
    pub(super) fn horizontal_bounds(&self) -> Range<Column> {
        self.horizontal_margin.clone().unwrap_or(Column(0)..Column(self.columns()))
    }

    /// The column printing stops before at the cursor: the right margin, the
    /// screen's edge, or the end of a doubled row's visible half.
    #[inline]
    pub(super) fn input_right_edge(&self) -> usize {
        // Every printed character asks; nearly always there is only the screen.
        if self.horizontal_margin.is_none() && !self.doubled_rows {
            return self.columns();
        }
        let region = self.horizontal_bounds();
        let point = self.grid.cursor.point;
        let edge = if point.column < region.end { region.end.0 } else { self.columns() };
        if !self.doubled_rows {
            return edge;
        }
        edge.min(self.grid[point.line].line_size().visible_columns(self.columns()))
    }
}

impl<T: EventListener> Term<T> {
    pub(super) fn horizontal_margins(&mut self, left: usize, right: usize) -> bool {
        if self.horizontal_margin.is_none() {
            return false;
        }
        let right = if right == 0 { self.columns() } else { right.min(self.columns()) };
        let left = left.max(1) - 1;
        if left < right.saturating_sub(1) {
            self.horizontal_margin = Some(Column(left)..Column(right));
            self.goto(0, 0);
        }
        true
    }

    pub(super) fn move_vertical(&mut self, delta: i32) {
        let current = self.grid.cursor.point.line;
        let top =
            if current >= self.scroll_region.start { self.scroll_region.start } else { Line(0) };
        let bottom = if current < self.scroll_region.end {
            self.scroll_region.end - 1
        } else {
            self.bottommost_line()
        };
        self.damage_cursor();
        self.grid.cursor.point.line = Line(current.0.saturating_add(delta)).clamp(top, bottom);
        self.grid.cursor.input_needs_wrap = false;
        self.damage_cursor();
    }

    pub(super) fn move_backward_wrapping(&mut self, mut count: usize) {
        let bounds = self.horizontal_bounds();
        let mut point = self.grid.cursor.point;
        let inside = bounds.contains(&point.column);
        let left = if inside { bounds.start.0 } else { 0 };
        let right = if inside { bounds.end - 1 } else { self.last_column() };
        let top = if self.scroll_region.contains(&point.line) {
            self.scroll_region.start
        } else {
            Line(0)
        };
        self.damage_cursor();
        while count > point.column.0 - left && point.line > top {
            if !self.mode.contains(TermMode::REVERSE_WRAP_ALL)
                && !self.grid[point.line - 1i32][right].flags.contains(Flags::WRAPLINE)
            {
                break;
            }
            count -= point.column.0 - left + 1;
            point.line -= 1;
            point.column = right;
        }
        point.column = Column(point.column.0.saturating_sub(count).max(left));
        self.grid.cursor.point = point;
        self.grid.cursor.input_needs_wrap = false;
        self.damage_cursor();
    }

    /// Scrolls only the band between the left and right margins, when they
    /// narrow the screen; false leaves the scroll to the whole rows. Every
    /// scrolled line asks, so the common answer is kept inline.
    #[inline]
    pub(super) fn scroll_rectangle(&mut self, origin: Line, count: usize, down: bool) -> bool {
        if self.horizontal_margin.is_none() {
            return false;
        }
        self.scroll_margin_rectangle(origin, count, down)
    }

    fn scroll_margin_rectangle(&mut self, origin: Line, count: usize, down: bool) -> bool {
        let columns = self.horizontal_bounds();
        if columns.start == 0 && columns.end == self.columns() {
            return false;
        }
        let end = self.scroll_region.end;
        let count = count.min((end - origin).0.max(0) as usize);
        if count == 0 {
            return true;
        }
        let last = self.last_column();
        let blank = self.grid.cursor.template.erased();
        // Repair glyphs crossing either vertical edge before moving the band.
        for row in origin.0..end.0 {
            self.clear_edit_boundary(Line(row), columns.start.0);
            self.clear_edit_boundary(Line(row), columns.end.0);
        }
        for index in 0..(end - origin).0 {
            let row = if down { end - 1 - index } else { origin + index };
            let source = if down { row - count } else { row + count };
            let wrapped = self.grid[row][last].flags & Flags::WRAPLINE;
            for column in columns.start.0..columns.end.0 {
                self.grid[row][Column(column)] = if source >= origin && source < end {
                    self.grid[source][Column(column)].clone()
                } else {
                    blank.clone()
                };
            }
            if columns.end > last {
                self.grid[row][last].flags.remove(Flags::WRAPLINE);
                self.grid[row][last].flags.insert(wrapped);
            }
            self.damage.damage_line(row.0 as usize, columns.start.0, columns.end.0 - 1);
        }
        self.selection = None;
        true
    }
}

#[cfg(test)]
#[path = "../tests/term/margins.rs"]
mod tests;
