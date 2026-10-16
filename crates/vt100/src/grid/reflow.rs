//! Normal-screen resize: rebuild logical lines, then retain overflow as history.
//! Kept separate from the fixed-coordinate resize used by protocol replicas.

use super::{Grid, Pos, Size};
use crate::{row::Row, Cell};
use std::sync::Arc;
use unicode_width::UnicodeWidthChar as _;

impl Grid {
    pub(crate) fn set_size_reflow(&mut self, size: Size) {
        if self.size == size {
            return;
        }
        if self.scroll_region_active() || self.rows.is_empty() {
            self.set_size(size);
            return;
        }
        let (rows, pos, saved_pos) = self.reflow_rows(size.cols);
        self.rows = rows;
        // Empty space below the cursor must not push text into history when
        // only the height changes. Meaningful rows always survive, even below
        // the cursor (for example after a cursor-up).
        while self.rows.len() > usize::from(size.rows)
            && self.rows.len() > pos.row + 1
            && self.rows.last().is_some_and(|row| row_is_blank(row))
        {
            self.rows.pop();
        }
        let overflow = self.rows.len().saturating_sub(usize::from(size.rows));
        let mut rows = std::mem::take(&mut self.rows);
        for row in rows.drain(..overflow) {
            self.remember_row(Arc::unwrap_or_clone(row));
        }
        self.rows = rows;
        self.pos = pos.clamped(overflow);
        self.saved_pos = saved_pos.clamped(overflow);
        // Geometry and margin clamping are shared with fixed-grid resizing.
        self.size.cols = size.cols;
        self.set_size(size);
        self.pos.col = pos.col.min(size.cols);
    }

    fn reflow_rows(
        &self,
        cols: u16,
    ) -> (Vec<Arc<Row>>, ReflowPosition, ReflowPosition) {
        let mut result = Vec::new();
        let mut row = Row::new(cols);
        let mut column = 0;
        let mut pos = ReflowPosition::default();
        let mut saved_pos = ReflowPosition::default();
        for (y, source) in self.rows.iter().enumerate() {
            let end = if source.wrapped() {
                // A wide glyph wrapped before the last column leaves a
                // padding cell there; it is not part of the logical line.
                self.size.cols
                    - u16::from(
                        source.get(self.size.cols - 1) == Some(&Cell::new())
                            && self.rows.get(y + 1).is_some_and(|next| {
                                next.get(0).is_some_and(Cell::is_wide)
                            }),
                    )
            } else {
                (0..self.size.cols)
                    .rfind(|&x| {
                        source.get(x).is_some_and(|c| c != &Cell::new())
                    })
                    .map_or(0, |x| x + 1)
            }
            .max(if y == usize::from(self.pos.row) {
                self.pos.col
            } else {
                0
            })
            .max(if y == usize::from(self.saved_pos.row) {
                self.saved_pos.col
            } else {
                0
            });
            for x in 0..=end {
                let cell = source.get(x);
                let width = cell.map_or(1, |c| {
                    if c.is_wide()
                        || c.contents()
                            .chars()
                            .next()
                            .is_some_and(|ch| ch.width() == Some(2))
                    {
                        2
                    } else {
                        1
                    }
                });
                let continuation =
                    cell.is_some_and(Cell::is_wide_continuation);
                if x < end
                    && !continuation
                    && column > 0
                    && width > cols.saturating_sub(column)
                {
                    row.wrap(true);
                    result.push(Arc::new(row));
                    row = Row::new(cols);
                    column = 0;
                }
                let mapped = ReflowPosition {
                    row: result.len(),
                    col: column,
                };
                if y == usize::from(self.pos.row) && x == self.pos.col {
                    pos = mapped;
                }
                if y == usize::from(self.saved_pos.row)
                    && x == self.saved_pos.col
                {
                    saved_pos = mapped;
                }
                if x == end || continuation {
                    continue;
                }
                if let Some(cell) = cell {
                    let mut cell = cell.clone();
                    cell.set_wide(width == 2 && cols > 1);
                    *row.get_mut(column).unwrap() = cell;
                    if width == 2 && cols > 1 {
                        let mut continuation = Cell::new();
                        continuation.clear(*source.get(x).unwrap().attrs());
                        continuation.set_wide_continuation(true);
                        *row.get_mut(column + 1).unwrap() = continuation;
                    }
                }
                column += width.min(cols);
            }
            if !source.wrapped() {
                result.push(Arc::new(row));
                row = Row::new(cols);
                column = 0;
            }
        }
        if self.rows.last().is_some_and(|row| row.wrapped()) {
            result.push(Arc::new(row));
        }
        (result, pos, saved_pos)
    }
}

fn row_is_blank(row: &Row) -> bool {
    !row.wrapped()
        && (0..row.cols()).all(|x| row.get(x) == Some(&Cell::new()))
}

#[derive(Clone, Copy, Default)]
struct ReflowPosition {
    row: usize,
    col: u16,
}

impl ReflowPosition {
    fn clamped(self, overflow: usize) -> Pos {
        Pos {
            row: u16::try_from(self.row.saturating_sub(overflow))
                .unwrap_or(u16::MAX),
            col: self.col,
        }
    }
}

#[cfg(test)]
mod tests {
    fn resize(parser: &mut crate::Parser, rows: u16, cols: u16) {
        parser.screen_mut().set_size_reflow(rows, cols);
    }

    #[test]
    fn narrowing_and_widening_preserve_logical_lines_and_cursor() {
        let mut parser = crate::Parser::new(8, 12, 0);
        parser.process(b"abcdefghijklmnop\r\nsecond");
        resize(&mut parser, 8, 5);
        assert_eq!(parser.screen().contents(), "abcdefghijklmnop\nsecond");
        assert_eq!(parser.screen().cursor_position(), (5, 1));
        resize(&mut parser, 8, 20);
        assert_eq!(parser.screen().contents(), "abcdefghijklmnop\nsecond");
        assert_eq!(parser.screen().cursor_position(), (1, 6));
    }

    #[test]
    fn shortening_preserves_the_bottom_and_evicts_the_top() {
        let mut parser = crate::Parser::new(4, 12, 0);
        parser.screen_mut().set_capture_evicted_rows(true);
        parser.process(b"first\r\nsecond\r\nthird\r\nfourth");
        resize(&mut parser, 2, 12);
        assert_eq!(parser.screen().contents(), "third\nfourth");
        let (first, rows) = parser.screen_mut().take_evicted_rows();
        assert_eq!(first, 0);
        assert_eq!(
            rows.iter()
                .map(|r| r.contents.as_slice())
                .collect::<Vec<_>>(),
            [b"first".as_slice(), b"second"]
        );
        assert_eq!(parser.screen().cursor_position(), (1, 6));
    }

    #[test]
    fn shrinking_blank_space_does_not_scroll_and_height_keeps_wrapping() {
        let mut parser = crate::Parser::new(8, 5, 0);
        parser.process(b"abcdef");
        resize(&mut parser, 3, 5);
        assert_eq!(parser.screen().contents(), "abcdef");
        assert_eq!(parser.screen().evicted_rows_total(), 0);
        resize(&mut parser, 3, 10);
        assert_eq!(parser.screen().contents(), "abcdef");
    }

    #[test]
    fn pending_wrap_survives_resize_and_the_next_print() {
        let mut parser = crate::Parser::new(4, 8, 0);
        parser.process(b"abcdefgh");
        resize(&mut parser, 4, 4);
        parser.process(b"i");
        assert_eq!(parser.screen().contents(), "abcdefghi");
    }

    #[test]
    fn a_wide_glyph_can_be_parsed_at_one_column_and_reexpanded() {
        let mut parser = crate::Parser::new(4, 1, 0);
        parser.process("界x".as_bytes());
        assert_eq!(parser.screen().contents(), "界x");
        resize(&mut parser, 4, 4);
        assert_eq!(parser.screen().contents(), "界x");
        assert!(parser.screen().cell(0, 0).unwrap().is_wide());
        assert!(parser.screen().cell(0, 1).unwrap().is_wide_continuation());
    }

    #[test]
    fn wide_combining_and_colored_cells_survive_a_new_wrap_boundary() {
        let mut parser = crate::Parser::new(8, 10, 0);
        parser.process("ab\x1b[31m界e\u{301}XYZ".as_bytes());
        resize(&mut parser, 8, 3);
        assert!(parser.screen().cell(1, 0).unwrap().is_wide());
        assert!(parser.screen().cell(1, 1).unwrap().is_wide_continuation());
        assert_eq!(
            parser.screen().cell(1, 2).unwrap().contents(),
            "e\u{301}"
        );
        assert_eq!(
            parser.screen().cell(1, 0).unwrap().fgcolor(),
            crate::Color::Idx(1)
        );
        resize(&mut parser, 8, 1);
        assert!(parser.screen().contents().contains('界'));
        resize(&mut parser, 8, 10);
        assert_eq!(parser.screen().contents(), "ab界e\u{301}XYZ");
    }

    #[test]
    fn alternate_screen_and_scroll_regions_remain_fixed_grids() {
        for prefix in [b"\x1b[?1049h".as_slice(), b"\x1b[2;4r"] {
            let mut parser = crate::Parser::new(4, 8, 0);
            parser.screen_mut().set_capture_evicted_rows(true);
            parser.process(prefix);
            parser.process(b"abcdefgh");
            resize(&mut parser, 4, 4);
            assert_eq!(parser.screen().contents().trim(), "abcd");
            assert_eq!(parser.screen().evicted_rows_total(), 0);
        }
    }
}
