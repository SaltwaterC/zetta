//! Character insertion, deletion and erasure, including wide-cell boundaries.
//!
//! An edit may split a two-cell glyph at either end of its moved or erased
//! range. Repair those boundaries before moving cells, without scanning the row.

use super::*;

impl<T: EventListener> Term<T> {
    pub(super) fn insert_blank_cells(&mut self, count: usize) {
        let point = self.grid.cursor.point;
        let bounds = self.horizontal_bounds();
        if self.horizontal_margin.is_some() && !bounds.contains(&point.column) {
            return;
        }
        let columns = bounds.end.0;
        let start = point.column.0;
        let count = count.min(columns - start);
        if count == 0 {
            return;
        }
        self.grid.cursor.input_needs_wrap = false;
        self.clear_edit_boundary(point.line, columns);
        self.clear_edit_boundary(point.line, start);
        self.clear_edit_boundary(point.line, columns - count);
        let blank = self.grid.cursor.template.erased();
        self.damage.damage_line(point.line.0 as usize, 0, columns - 1);
        let row = &mut self.grid[point.line][..];
        row[columns - 1].flags.remove(Flags::WRAPLINE | Flags::LEADING_WIDE_CHAR_SPACER);
        for column in (start..columns - count).rev() {
            row.swap(column, column + count);
        }
        row[start..start + count].fill_with(|| blank.clone());
    }

    pub(super) fn erase_chars_cells(&mut self, count: usize) {
        let columns = self.columns();
        let point = self.grid.cursor.point;
        let start = point.column.0;
        let end = start + count.min(columns - start);
        if start == end {
            return;
        }
        self.grid.cursor.input_needs_wrap = false;
        self.clear_edit_boundary(point.line, columns);
        self.clear_edit_boundary(point.line, start);
        self.clear_edit_boundary(point.line, end);
        let blank = self.grid.cursor.template.erased();
        self.damage.damage_line(point.line.0 as usize, start, end.min(columns - 1));
        self.grid[point.line][Column(start)..Column(end)].fill_with(|| blank.clone());
    }

    pub(super) fn delete_chars_cells(&mut self, count: usize) {
        let point = self.grid.cursor.point;
        let bounds = self.horizontal_bounds();
        if self.horizontal_margin.is_some() && !bounds.contains(&point.column) {
            return;
        }
        let columns = bounds.end.0;
        let start = point.column.0;
        let count = count.min(columns - start);
        if count == 0 {
            return;
        }
        self.grid.cursor.input_needs_wrap = false;
        self.clear_edit_boundary(point.line, columns);
        self.clear_edit_boundary(point.line, start);
        self.clear_edit_boundary(point.line, start + count);
        let blank = self.grid.cursor.template.erased();
        self.damage.damage_line(point.line.0 as usize, 0, columns - 1);
        let row = &mut self.grid[point.line][..];
        // The old last cell moves into the interior; its wrapping metadata must
        // not move with it or make a later resize join unrelated rows.
        row[columns - 1].flags.remove(Flags::WRAPLINE | Flags::LEADING_WIDE_CHAR_SPACER);
        for column in start..columns - count {
            row.swap(column, column + count);
        }
        row[columns - count..columns].fill_with(|| blank.clone());
    }

    pub(super) fn clear_edit_boundary(&mut self, line: Line, column: usize) {
        if column <= 1 && line > self.topmost_line() {
            let last_column = self.last_column();
            let previous = &mut self.grid[line - 1i32][last_column];
            if previous.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                previous.flags.remove(Flags::LEADING_WIDE_CHAR_SPACER);
                // Retained history has no screen damage entry.
                if line > Line(0) {
                    self.damage.damage_line((line.0 - 1) as usize, last_column.0, last_column.0);
                }
            }
        }
        if column == 0
            || column >= self.columns()
            || !self.grid[line][Column(column)].flags.contains(Flags::WIDE_CHAR_SPACER)
        {
            return;
        }
        let row = &mut self.grid[line];
        row[Column(column - 1)].clear_wide();
        row[Column(column)].flags.remove(Flags::WIDE_CHAR_SPACER);
        row[Column(column)].clear_wide();
        self.damage.damage_line(line.0 as usize, column - 1, column);
    }
}

#[cfg(test)]
#[path = "../tests/term/char_edit.rs"]
mod tests;
