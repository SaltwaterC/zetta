//! Screen-sized reflow with source coordinates retained for searches made
//! against an immutable history while its full reflow is still running.

use super::*;
use crate::term::cell::Cell;
use crate::vte::ansi::Color;

#[derive(Clone, Default, PartialEq)]
struct MappedCell {
    cell: Cell,
    source: Option<Point>,
}

#[cfg(test)]
#[path = "../tests/grid/reflow_view.rs"]
mod tests;

impl GridCell for MappedCell {
    fn is_empty(&self) -> bool {
        self.cell.is_empty()
    }
    fn reset(&mut self, template: &Self) {
        self.cell.reset(&template.cell);
        self.source = None;
    }
    fn flags(&self) -> &Flags {
        &self.cell.flags
    }
    fn flags_mut(&mut self) -> &mut Flags {
        &mut self.cell.flags
    }
}

impl ResetDiscriminant<(Color, Color)> for MappedCell {
    fn discriminant(&self) -> (Color, Color) {
        (self.cell.fg, self.cell.bg)
    }
}

impl Grid<Cell> {
    /// Reflow only the requested viewport, retaining each displayed cell's
    /// original position. Wide spacers, styles and combining marks follow the
    /// ordinary grid reflow implementation, rather than a second text wrapper.
    pub(crate) fn mapped_reflow_window(
        &self,
        offset: usize,
        lines: usize,
        columns: usize,
        focus: Option<Point>,
    ) -> (Self, Vec<(Point, Point)>) {
        let start = Line(-(offset.min(self.history_size()) as i32));
        let source_lines = lines.saturating_mul(columns.div_ceil(self.columns) + 1);
        let end = (start.0 as i64 + source_lines as i64).min(self.lines as i64) as i32;
        let count = ((end - start.0) as usize).max(lines);
        let mut raw = Storage::with_capacity(0, self.columns);
        raw.set_visible_lines(count);
        for line in start.0..end {
            let row = self.raw.owned_row(Line(line));
            let cells = row[..]
                .iter()
                .cloned()
                .enumerate()
                .map(|(column, cell)| MappedCell {
                    cell,
                    source: Some(Point::new(Line(line), Column(column))),
                })
                .collect();
            raw.push_newest(Row::from_vec(cells, self.columns));
        }
        for _ in (end - start.0) as usize..count {
            raw.push_newest(Row::new(self.columns));
        }
        let mut window = Grid {
            raw,
            columns: self.columns,
            lines: count,
            cursor: Default::default(),
            saved_cursor: Default::default(),
            display_offset: 0,
            max_scroll_limit: usize::MAX - count,
        };
        window.resize::<(Color, Color)>(true, count, columns);
        window.lines = lines;
        window.raw.set_visible_lines(lines);
        window.display_offset = window.history_size();
        if let Some(focus) = focus {
            'rows: for line in window.topmost_line().0..=window.bottommost_line().0 {
                let row = window.raw.owned_row(Line(line));
                if row[..].iter().any(|cell| cell.source == Some(focus)) {
                    let top = -(window.display_offset as i32);
                    if line < top || line >= top + lines as i32 {
                        window.display_offset =
                            (line.saturating_neg().max(0) as usize).min(window.history_size());
                    }
                    break 'rows;
                }
            }
        }
        let mut mapping = Vec::new();
        let mut raw = Storage::with_capacity(0, columns);
        raw.set_visible_lines(lines);
        let first = -(window.display_offset as i32);
        // Keep only the displayed rows: the temporary viewport's extra rows
        // are never retained by the search or the live terminal.
        for line in first..first + lines as i32 {
            let row = window.raw.owned_row(Line(line));
            let cells = row[..]
                .iter()
                .cloned()
                .enumerate()
                .map(|(column, cell)| {
                    if let Some(source) = cell.source {
                        mapping.push((Point::new(Line(line - first), Column(column)), source));
                    }
                    cell.cell
                })
                .collect();
            raw.push_newest(Row::from_vec(cells, columns));
        }
        (
            Self {
                raw,
                columns,
                lines,
                cursor: Default::default(),
                saved_cursor: Default::default(),
                display_offset: 0,
                max_scroll_limit: 0,
            },
            mapping,
        )
    }
}
