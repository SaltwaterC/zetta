//! Selective DEC erasure and ISO guarded areas share cell-boundary repair.
use super::*;

impl<T: EventListener> Term<T> {
    fn erase_area(&mut self, row: i32, columns: Range<usize>, protection: Flags) {
        let blank = self.grid.cursor.template.erased();
        let mut column = columns.start;
        while column < columns.end {
            let flags = self.grid[Line(row)][Column(column)].flags;
            if flags.intersects(protection) {
                column += 1;
                continue;
            }
            self.clear_edit_boundary(Line(row), column);
            self.clear_edit_boundary(Line(row), column + 1);
            self.grid[Line(row)][Column(column)] = blank.clone();
            column += 1;
        }
        self.damage.damage_line(row as usize, columns.start, columns.end - 1);
        self.selection = None;
    }

    pub(super) fn erase_line_protected(&mut self, mode: u16, protection: Flags) {
        let point = self.grid.cursor.point;
        let columns = match mode {
            0 => point.column.0..self.columns(),
            1 => 0..point.column.0 + 1,
            2 => 0..self.columns(),
            _ => return,
        };
        self.erase_area(point.line.0, columns, protection);
    }

    pub(super) fn erase_screen_protected(&mut self, mode: u16, protection: Flags) {
        let point = self.grid.cursor.point;
        let rows = match mode {
            0 => point.line.0..self.screen_lines() as i32,
            1 => 0..point.line.0 + 1,
            2 => 0..self.screen_lines() as i32,
            _ => return,
        };
        for row in rows {
            let start = if mode == 0 && row == point.line.0 { point.column.0 } else { 0 };
            let end =
                if mode == 1 && row == point.line.0 { point.column.0 + 1 } else { self.columns() };
            self.erase_area(row, start..end, protection);
        }
    }

    pub(in crate::term) fn erase_guarded_chars(&mut self, count: usize) -> bool {
        if !self.protocol.iso_guarded || self.protocol.erase_guarded {
            return false;
        }
        let point = self.grid.cursor.point;
        self.erase_area(
            point.line.0,
            point.column.0..(point.column.0 + count.min(self.columns() - point.column.0)),
            Flags::GUARDED,
        );
        true
    }

    pub(in crate::term) fn erase_guarded_line(&mut self, mode: ansi::LineClearMode) -> bool {
        if !self.protocol.iso_guarded || self.protocol.erase_guarded {
            return false;
        }
        let mode = match mode {
            ansi::LineClearMode::Right => 0,
            ansi::LineClearMode::Left => 1,
            ansi::LineClearMode::All => 2,
        };
        self.erase_line_protected(mode, Flags::GUARDED);
        true
    }

    pub(in crate::term) fn erase_guarded_screen(&mut self, mode: ansi::ClearMode) -> bool {
        if !self.protocol.iso_guarded || self.protocol.erase_guarded {
            return false;
        }
        let mode = match mode {
            ansi::ClearMode::Below => 0,
            ansi::ClearMode::Above => 1,
            ansi::ClearMode::All => 2,
            ansi::ClearMode::Saved => return false,
        };
        self.erase_screen_protected(mode, Flags::GUARDED);
        true
    }
}
