//! Defines the Row type which makes up lines in the grid.

use std::cmp::{max, min};
use std::ops::{Index, IndexMut, Range, RangeFrom, RangeFull, RangeTo, RangeToInclusive};
use std::{ptr, slice};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::grid::GridCell;
use crate::index::Column;
use crate::term::cell::{Flags, ResetDiscriminant};

/// Zetta patch: a row's DEC line size (DECSWL, DECDWL, DECDHL). It belongs to the row rather than
/// its cells: erasing within the row keeps it, the row carries it when it scrolls, and only a
/// reset of the whole row returns it to single width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum LineSize {
    #[default]
    Single,
    DoubleWidth,
    /// The top half of a double-height line; it is also double width.
    DoubleHeightTop,
    /// The bottom half of a double-height line; it is also double width.
    DoubleHeightBottom,
}

impl LineSize {
    /// The columns a row of `columns` cells shows at this size: every size but single width
    /// draws each cell two columns wide, so only the first half of the row is visible.
    #[inline]
    pub fn visible_columns(self, columns: usize) -> usize {
        match self {
            Self::Single => columns,
            _ => (columns / 2).max(1),
        }
    }
}

/// A row in the grid.
#[derive(Default, Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Row<T> {
    inner: Vec<T>,

    #[cfg_attr(feature = "serde", serde(default))]
    line_size: LineSize,

    /// Maximum number of occupied entries.
    ///
    /// This is the upper bound on the number of elements in the row, which have been modified
    /// since the last reset. All cells after this point are guaranteed to be equal.
    pub(crate) occ: usize,
    /// Every attribute still matches the last reset. All unrestricted mutable access
    /// invalidates this proof; the character-only writer preserves it.
    #[cfg_attr(feature = "serde", serde(skip))]
    characters_only: bool,
}

impl<T: PartialEq> PartialEq for Row<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner && self.line_size == other.line_size
    }
}

impl<T: Default> Row<T> {
    /// Create a new terminal row.
    ///
    /// Ideally the `template` should be `Copy` in all performance sensitive scenarios.
    pub fn new(columns: usize) -> Row<T> {
        debug_assert!(columns >= 1);

        let mut inner: Vec<T> = Vec::with_capacity(columns);

        // This is a slightly optimized version of `std::vec::Vec::resize`.
        unsafe {
            let mut ptr = inner.as_mut_ptr();

            for _ in 1..columns {
                ptr::write(ptr, T::default());
                ptr = ptr.offset(1);
            }
            ptr::write(ptr, T::default());

            inner.set_len(columns);
        }

        Row { inner, line_size: LineSize::Single, occ: 0, characters_only: false }
    }

    /// Increase the number of columns in the row.
    #[inline]
    pub fn grow(&mut self, columns: usize) {
        if self.inner.len() >= columns {
            return;
        }

        self.inner.resize_with(columns, T::default);
        self.characters_only = false;
    }

    /// Reduce the number of columns in the row.
    ///
    /// This will return all non-empty cells that were removed.
    pub fn shrink(&mut self, columns: usize) -> Option<Vec<T>>
    where
        T: GridCell,
    {
        if self.inner.len() <= columns {
            return None;
        }

        // Split off cells for a new row.
        let mut new_row = self.inner.split_off(columns);
        let index = new_row.iter().rposition(|c| !c.is_empty()).map_or(0, |i| i + 1);
        new_row.truncate(index);

        self.occ = min(self.occ, columns);

        if new_row.is_empty() { None } else { Some(new_row) }
    }

    /// Reset all cells in the row to the `template` cell.
    #[inline]
    pub fn reset<D>(&mut self, template: &T)
    where
        T: ResetDiscriminant<D> + GridCell,
        D: PartialEq,
    {
        debug_assert!(!self.inner.is_empty());

        // Mark all cells as dirty if template cell changed.
        let len = self.inner.len();
        if self.inner[len - 1].discriminant() != template.discriminant() {
            self.occ = len;
            self.characters_only = false;
        }

        // A partial reset of an imported row says nothing about its untouched suffix.
        // Establish the proof only after resetting the whole row, or retain an existing one.
        let characters_only = self.characters_only || self.occ == len;

        // Reset every dirty cell in the row.
        if self.characters_only {
            T::reset_characters(&mut self.inner[0..self.occ], template);
        } else {
            T::reset_all(&mut self.inner[0..self.occ], template);
        }

        self.occ = 0;
        self.characters_only = characters_only;
        self.line_size = LineSize::Single;
    }
}

#[allow(clippy::len_without_is_empty)]
impl<T> Row<T> {
    #[inline]
    pub fn from_vec(vec: Vec<T>, occ: usize) -> Row<T> {
        Row { inner: vec, line_size: LineSize::Single, occ, characters_only: false }
    }

    #[inline]
    pub fn line_size(&self) -> LineSize {
        self.line_size
    }

    #[inline]
    pub fn set_line_size(&mut self, line_size: LineSize) {
        self.line_size = line_size;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn last(&self) -> Option<&T> {
        self.inner.last()
    }

    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.characters_only = false;
        self.occ = self.inner.len();
        self.inner.last_mut()
    }

    #[inline]
    pub fn append(&mut self, vec: &mut Vec<T>)
    where
        T: GridCell,
    {
        self.characters_only = false;
        self.occ += vec.len();
        self.inner.append(vec);
    }

    #[inline]
    pub fn append_front(&mut self, mut vec: Vec<T>) {
        self.characters_only = false;
        self.occ += vec.len();

        vec.append(&mut self.inner);
        self.inner = vec;
    }

    /// Check if all cells in the row are empty.
    #[inline]
    pub fn is_clear(&self) -> bool
    where
        T: GridCell,
    {
        self.inner.iter().all(GridCell::is_empty)
    }

    #[inline]
    pub fn front_split_off(&mut self, at: usize) -> Vec<T> {
        self.occ = self.occ.saturating_sub(at);

        let mut split = self.inner.split_off(at);
        std::mem::swap(&mut split, &mut self.inner);
        split
    }
}

impl<'a, T> IntoIterator for &'a Row<T> {
    type IntoIter = slice::Iter<'a, T>;
    type Item = &'a T;

    #[inline]
    fn into_iter(self) -> slice::Iter<'a, T> {
        self.inner.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Row<T> {
    type IntoIter = slice::IterMut<'a, T>;
    type Item = &'a mut T;

    #[inline]
    fn into_iter(self) -> slice::IterMut<'a, T> {
        self.characters_only = false;
        self.occ = self.len();
        self.inner.iter_mut()
    }
}

impl<T> Index<Column> for Row<T> {
    type Output = T;

    #[inline]
    fn index(&self, index: Column) -> &T {
        &self.inner[index.0]
    }
}

impl Row<crate::term::cell::Cell> {
    /// Write characters without exposing attributes to a caller that could change them.
    ///
    /// The cells must be blank, as the template erased them, and hold no attribute the template
    /// does not, so their flags are set rather than read: this loop is bound by its stores.
    #[inline]
    pub(crate) fn write_ascii_characters(&mut self, start: Column, text: &[u8]) {
        let end = start.0 + text.len();
        self.occ = max(self.occ, end);
        let cells = &mut self.inner[start.0..end];
        debug_assert!(cells.iter().all(|cell| cell.flags.difference(Flags::DRAWN).is_empty()));
        if text.len() >= 32 {
            write_long_ascii(cells, text);
            return;
        }
        for (cell, &byte) in cells.iter_mut().zip(text) {
            cell.write_drawn_ascii(crate::vte::ascii_run_char(byte));
        }
    }
}

/// `Row::write_ascii_characters` for a run long enough to search first. Only undecodable output
/// puts U+FFFD in a run, and mapping it in the loop cost a screen-wide line of plain text a tenth
/// of its throughput, so a run without one is not mapped. Searching a short run costs more than
/// mapping it, and so does inlining this beside the loop that maps it.
#[inline(never)]
fn write_long_ascii(cells: &mut [crate::term::cell::Cell], text: &[u8]) {
    let replacement = crate::vte::ASCII_REPLACEMENT;
    if text.iter().fold(false, |found, &byte| found | (byte == replacement)) {
        for (cell, &byte) in cells.iter_mut().zip(text) {
            cell.write_drawn_ascii(crate::vte::ascii_run_char(byte));
        }
    } else {
        for (cell, &byte) in cells.iter_mut().zip(text) {
            cell.write_drawn_ascii(char::from(byte));
        }
    }
}

impl<T> IndexMut<Column> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: Column) -> &mut T {
        self.characters_only = false;
        self.occ = max(self.occ, *index + 1);
        &mut self.inner[index.0]
    }
}

impl<T> Index<Range<Column>> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, index: Range<Column>) -> &[T] {
        &self.inner[(index.start.0)..(index.end.0)]
    }
}

impl<T> IndexMut<Range<Column>> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: Range<Column>) -> &mut [T] {
        self.characters_only = false;
        self.occ = max(self.occ, *index.end);
        &mut self.inner[(index.start.0)..(index.end.0)]
    }
}

impl<T> Index<RangeTo<Column>> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, index: RangeTo<Column>) -> &[T] {
        &self.inner[..(index.end.0)]
    }
}

impl<T> IndexMut<RangeTo<Column>> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: RangeTo<Column>) -> &mut [T] {
        self.characters_only = false;
        self.occ = max(self.occ, *index.end);
        &mut self.inner[..(index.end.0)]
    }
}

impl<T> Index<RangeFrom<Column>> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, index: RangeFrom<Column>) -> &[T] {
        &self.inner[(index.start.0)..]
    }
}

impl<T> IndexMut<RangeFrom<Column>> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: RangeFrom<Column>) -> &mut [T] {
        self.characters_only = false;
        self.occ = self.len();
        &mut self.inner[(index.start.0)..]
    }
}

impl<T> Index<RangeFull> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, _: RangeFull) -> &[T] {
        &self.inner[..]
    }
}

impl<T> IndexMut<RangeFull> for Row<T> {
    #[inline]
    fn index_mut(&mut self, _: RangeFull) -> &mut [T] {
        self.characters_only = false;
        self.occ = self.len();
        &mut self.inner[..]
    }
}

impl<T> Index<RangeToInclusive<Column>> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, index: RangeToInclusive<Column>) -> &[T] {
        &self.inner[..=(index.end.0)]
    }
}

impl<T> IndexMut<RangeToInclusive<Column>> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: RangeToInclusive<Column>) -> &mut [T] {
        self.characters_only = false;
        self.occ = max(self.occ, *index.end + 1);
        &mut self.inner[..=(index.end.0)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::cell::{Cell, Flags};
    use crate::vte::ansi::{Color, NamedColor};

    #[test]
    fn character_writes_preserve_reset_attributes_and_occupied_bound() {
        let template = Cell { bg: Color::Named(NamedColor::Red), ..Cell::default() };
        let mut row = Row::<Cell>::new(8);
        row.reset(&template);
        row.write_ascii_characters(Column(1), b"abc\x7f");
        assert!(row.characters_only);
        assert_eq!(row.occ, 5);
        assert_eq!(row[Column(4)].c, '\u{fffd}');
        row.reset(&template);
        assert_eq!(row.occ, 0);
        assert!(row[..].iter().all(|cell| cell.c == ' ' && cell.bg == template.bg));

        // Changing the erase background must update every cell, including the blank suffix.
        row.reset(&Cell::default());
        assert!(row[..].iter().all(|cell| *cell == Cell::default()));
    }

    #[test]
    fn unrestricted_cell_access_invalidates_character_only_reset() {
        let edits: [fn(&mut Row<Cell>); 8] = [
            |row| row[Column(1)].flags = Flags::BOLD,
            |row| row[Column(1)..Column(2)][0].flags = Flags::BOLD,
            |row| row[..Column(2)][0].flags = Flags::BOLD,
            |row| row[Column(1)..][0].flags = Flags::BOLD,
            |row| row[..][0].flags = Flags::BOLD,
            |row| row[..=Column(1)][0].flags = Flags::BOLD,
            |row| row.last_mut().unwrap().flags = Flags::BOLD,
            |row| row.into_iter().next().unwrap().flags = Flags::BOLD,
        ];
        for edit in edits {
            let mut row = Row::<Cell>::new(8);
            row.write_ascii_characters(Column(0), b"abcdefgh");
            row.reset(&Cell::default());
            row.write_ascii_characters(Column(0), b"abcdefgh");
            assert!(row.characters_only);
            edit(&mut row);
            assert!(!row.characters_only);
            row.reset(&Cell::default());
            assert!(row[..].iter().all(|cell| *cell == Cell::default()));
        }
    }

    #[test]
    fn partially_reset_imported_rows_do_not_claim_untouched_attributes() {
        // Styled outside the reset discriminant and the flags plain writes assume are clear.
        let mut styled = Cell::default();
        styled.set_underline_color(Some(Color::Named(NamedColor::Red)));
        let mut row = Row::from_vec(vec![styled.clone(); 8], 4);
        row.reset(&Cell::default());
        assert!(!row.characters_only);
        assert!(row[Column(4)..].iter().all(|cell| *cell == styled));
        row.write_ascii_characters(Column(4), b"text");
        row.reset(&Cell::default());
        assert!(row.characters_only);
        assert!(row[..].iter().all(|cell| *cell == Cell::default()));
    }
}
