//! Grid rows read as text, for searching history without decoding it.
//!
//! Reading a cell of compact history through `Index` decodes its whole chunk into rows of full
//! cells (see [`super::archive`]), which is the right trade for rendering and selection but costs
//! a search over unique output about twelve times what the characters alone do: it rebuilt every
//! cell's attributes only to read its character, and freed them again a step later. These readers
//! walk the encoded runs instead, copy a run of ASCII in one go, and never touch the decode cache,
//! so a grid can be searched through a shared borrow from several threads at once.
//!
//! The text is what decoding the row and reading each cell's character up to its
//! [`LineLength`] would give, without wide-character spacers or zero-width characters. Matching
//! happens on that text, so [`RowText`] also maps a byte offset back to the cell it came from.

use super::Grid;
use super::storage::StoredRow;
use crate::index::{Column, Line, Point};
use crate::term::cell::{Cell, Flags, LineLength};

/// Cells that hold no character of their own: the right half of a wide character, and the
/// padding before a wide character that did not fit at the end of a line.
pub(super) const SPACERS: Flags = Flags::WIDE_CHAR_SPACER.union(Flags::LEADING_WIDE_CHAR_SPACER);

/// Whether `cell` counts towards [`LineLength`] whatever its character: a cell is occupied when
/// its character is not a space or it carries zero-width characters.
#[inline]
pub(super) fn has_zerowidth(cell: &Cell) -> bool {
    cell.zerowidth().is_some_and(|zerowidth| !zerowidth.is_empty())
}

/// The text of one or more grid rows, and where each of its characters came from.
#[derive(Clone, Debug, Default)]
pub struct RowText {
    text: String,
    /// Where the text and the grid fall out of step: every byte from an anchor's offset up to the
    /// next anchor is one column, counting on from the anchor's point. Plain ASCII on one line
    /// needs one anchor; a wide or non-ASCII character, a skipped spacer, or the next row of a
    /// wrapped line adds one.
    anchors: Vec<(usize, Point)>,
    /// The point the next byte maps to without a new anchor.
    next: Option<Point>,
}

impl RowText {
    #[inline]
    pub fn clear(&mut self) {
        self.text.clear();
        self.anchors.clear();
        self.next = None;
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.text.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Lowercase the ASCII letters in place. Byte offsets, and so every point, are unchanged.
    #[inline]
    pub fn make_ascii_lowercase(&mut self) {
        self.text.make_ascii_lowercase();
    }

    /// The cell holding the character that starts at byte `offset`, which must be a character
    /// boundary inside the text.
    pub fn point(&self, offset: usize) -> Point {
        debug_assert!(offset < self.text.len() && self.text.is_char_boundary(offset));
        let (at, point) = self.anchors[self.anchors.partition_point(|&(at, _)| at <= offset) - 1];
        Point::new(point.line, point.column + (offset - at))
    }

    /// The cell holding the last character before byte `end`, which must be a character boundary
    /// after the start of the text.
    pub fn last_point(&self, end: usize) -> Point {
        debug_assert!(end > 0 && self.text.is_char_boundary(end));
        let mut start = end - 1;
        while !self.text.is_char_boundary(start) {
            start -= 1;
        }
        self.point(start)
    }

    #[inline]
    fn anchor(&mut self, point: Point) {
        if self.next != Some(point) {
            self.anchors.push((self.text.len(), point));
        }
    }

    /// Append `text`, which is ASCII, as the cells starting at `point`.
    #[inline]
    pub(super) fn push_ascii(&mut self, text: &str, point: Point) {
        debug_assert!(text.is_ascii());
        if text.is_empty() {
            return;
        }
        self.anchor(point);
        self.text.push_str(text);
        self.next = Some(Point::new(point.line, point.column + text.len()));
    }

    #[inline]
    pub(super) fn push_char(&mut self, c: char, point: Point) {
        self.anchor(point);
        self.text.push(c);
        self.next = Some(Point::new(point.line, point.column + c.len_utf8()));
    }

    /// Append `c` as each of the `count` cells starting at `point`.
    pub(super) fn push_repeated(&mut self, c: char, count: usize, point: Point) {
        if c.is_ascii() && count > 0 {
            self.anchor(point);
            self.text.extend(std::iter::repeat_n(c, count));
            self.next = Some(Point::new(point.line, point.column + count));
        } else {
            for offset in 0..count {
                self.push_char(c, Point::new(point.line, point.column + offset));
            }
        }
    }

    /// Drop the text from byte `len` on.
    pub(super) fn truncate(&mut self, len: usize) {
        if len >= self.text.len() {
            return;
        }
        self.text.truncate(len);
        while self.anchors.last().is_some_and(|&(at, _)| at >= len) {
            self.anchors.pop();
        }
        self.next = None;
    }

    fn push_row(&mut self, row: &super::Row<Cell>, line: Line) {
        for column in 0..row.line_length().0 {
            let cell = &row[Column(column)];
            if !cell.flags.intersects(SPACERS) {
                self.push_char(cell.c, Point::new(line, Column(column)));
            }
        }
    }
}

impl Grid<Cell> {
    /// Append the text of `line` to `text`: each cell's character up to the row's
    /// [`LineLength`], without wide-character spacers or zero-width characters.
    ///
    /// Unlike indexing the grid, this never decodes compact history, so it needs no
    /// [`Grid::release_history_cache`] however much of the grid it reads.
    pub fn row_text(&self, line: Line, text: &mut RowText) {
        match self.raw.stored_row(line) {
            StoredRow::Row(row) => text.push_row(row, line),
            StoredRow::Compact(rows, index) => rows.append_text(index, line, text),
        }
    }

    /// Whether `line` continues on the next line, without decoding compact history.
    pub fn row_wraps(&self, line: Line) -> bool {
        match self.raw.stored_row(line) {
            StoredRow::Row(row) => row[Column(row.len() - 1)].flags.contains(Flags::WRAPLINE),
            StoredRow::Compact(rows, index) => rows.wraps(index),
        }
    }
}

#[cfg(test)]
#[path = "../tests/grid/text.rs"]
mod tests;
