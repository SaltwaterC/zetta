use std::collections::VecDeque;
use std::ops::{Index, IndexMut};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use super::Row;
use crate::index::Line;

const ROW_CHUNK_SIZE: usize = 1_024;

/// A chunked double-ended row buffer.
///
/// Only the first and last chunk can be partially populated, so indexing stays
/// constant-time without one contiguous allocation for every retained row.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct RowBuffer<T> {
    chunks: VecDeque<VecDeque<Row<T>>>,
    len: usize,
}

impl<T> Default for RowBuffer<T> {
    fn default() -> Self {
        Self { chunks: VecDeque::new(), len: 0 }
    }
}

impl<T: PartialEq> PartialEq for RowBuffer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.chunks.iter().flatten().eq(other.chunks.iter().flatten())
    }
}

impl<T> RowBuffer<T> {
    fn push_back(&mut self, row: Row<T>) {
        if self.chunks.back().is_none_or(|chunk| chunk.len() == ROW_CHUNK_SIZE) {
            self.chunks.push_back(VecDeque::with_capacity(ROW_CHUNK_SIZE));
        }
        self.chunks.back_mut().unwrap().push_back(row);
        self.len += 1;
    }

    fn push_front(&mut self, row: Row<T>) {
        if self.chunks.front().is_none_or(|chunk| chunk.len() == ROW_CHUNK_SIZE) {
            self.chunks.push_front(VecDeque::with_capacity(ROW_CHUNK_SIZE));
        }
        self.chunks.front_mut().unwrap().push_front(row);
        self.len += 1;
    }

    fn pop_back(&mut self) -> Option<Row<T>> {
        let row = self.chunks.back_mut()?.pop_back()?;
        if self.chunks.back().is_some_and(VecDeque::is_empty) {
            self.chunks.pop_back();
        }
        self.len -= 1;
        Some(row)
    }

    fn pop_front(&mut self) -> Option<Row<T>> {
        let row = self.chunks.front_mut()?.pop_front()?;
        if self.chunks.front().is_some_and(VecDeque::is_empty) {
            self.chunks.pop_front();
        }
        self.len -= 1;
        Some(row)
    }

    fn rotate_left(&mut self, count: usize) {
        for _ in 0..count {
            let row = self.pop_front().unwrap();
            self.push_back(row);
        }
    }

    fn rotate_right(&mut self, count: usize) {
        for _ in 0..count {
            let row = self.pop_back().unwrap();
            self.push_front(row);
        }
    }

    fn truncate(&mut self, len: usize) {
        while self.len > len {
            self.pop_back();
        }
    }

    fn position(&self, index: usize) -> (usize, usize) {
        debug_assert!(index < self.len);
        let first_len = self.chunks.front().map_or(0, VecDeque::len);
        if index < first_len {
            (0, index)
        } else {
            let index = index - first_len;
            (1 + index / ROW_CHUNK_SIZE, index % ROW_CHUNK_SIZE)
        }
    }

    fn from_rows(rows: Vec<Row<T>>) -> Self {
        let mut buffer = Self::default();
        for row in rows {
            buffer.push_back(row);
        }
        buffer
    }

    fn into_rows(self) -> Vec<Row<T>> {
        let mut rows = Vec::with_capacity(self.len);
        for chunk in self.chunks {
            rows.extend(chunk);
        }
        rows
    }

    fn len(&self) -> usize {
        self.len
    }
}

impl<T> Index<usize> for RowBuffer<T> {
    type Output = Row<T>;

    fn index(&self, index: usize) -> &Self::Output {
        let (chunk, row) = self.position(index);
        &self.chunks[chunk][row]
    }
}

impl<T> IndexMut<usize> for RowBuffer<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        let (chunk, row) = self.position(index);
        &mut self.chunks[chunk][row]
    }
}

/// A chunked row deque optimized for incremental scrollback growth.
///
/// Rows are ordered from the bottom of the visible grid toward the oldest
/// retained history. Moving one line into scrollback rotates one row from the
/// back to the front. Growing history appends only the rows immediately needed,
/// avoiding a full-buffer rezero, contiguous-buffer relocation, or bulk
/// initialization of future rows.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Storage<T> {
    inner: RowBuffer<T>,

    /// Number of visible lines.
    visible_lines: usize,
}

impl<T: PartialEq> PartialEq for Storage<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T> Storage<T> {
    #[inline]
    pub fn with_capacity(visible_lines: usize, columns: usize) -> Storage<T>
    where
        T: Default,
    {
        let mut inner = RowBuffer::default();
        for _ in 0..visible_lines {
            inner.push_back(Row::new(columns));
        }

        Storage { inner, visible_lines }
    }

    /// Increase the number of visible lines in the buffer.
    #[inline]
    pub fn grow_visible_lines(&mut self, next: usize)
    where
        T: Default,
    {
        let additional_lines = next - self.visible_lines;
        let columns = self[Line(0)].len();
        self.initialize(additional_lines, columns);
        self.visible_lines = next;
    }

    /// Decrease the number of visible lines in the buffer.
    #[inline]
    pub fn shrink_visible_lines(&mut self, next: usize) {
        let shrinkage = self.visible_lines - next;
        self.shrink_lines(shrinkage);
        self.visible_lines = next;
    }

    /// Remove the oldest lines from the buffer.
    #[inline]
    pub fn shrink_lines(&mut self, shrinkage: usize) {
        self.inner.truncate(self.inner.len() - shrinkage);
    }

    /// Release capacity which is no longer used by retained rows.
    #[inline]
    pub fn truncate(&mut self) {
        // Chunked storage retains at most two partially filled chunks, so
        // there is no large invisible row cache to release.
    }

    /// Add newly retained rows without preinitializing future scrollback.
    #[inline]
    pub fn initialize(&mut self, additional_rows: usize, columns: usize)
    where
        T: Default,
    {
        for _ in 0..additional_rows {
            self.inner.push_back(Row::new(columns));
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn swap(&mut self, a: Line, b: Line) {
        let a = self.compute_index(a);
        let b = self.compute_index(b);
        if a == b {
            return;
        }

        // No chunk mutation occurs while these pointers are live, and distinct
        // indices guarantee that the rows do not alias.
        unsafe {
            let a = &mut self.inner[a] as *mut Row<T>;
            let b = &mut self.inner[b] as *mut Row<T>;
            std::ptr::swap(a, b);
        }
    }

    /// Rotate the grid, moving all lines up/down in history.
    #[inline]
    pub fn rotate(&mut self, count: isize) {
        debug_assert!(count.unsigned_abs() <= self.inner.len());

        if count >= 0 {
            self.inner.rotate_left(count as usize);
        } else {
            self.inner.rotate_right(count.unsigned_abs());
        }
    }

    /// Rotate all existing lines down in history.
    #[inline]
    pub fn rotate_down(&mut self, count: usize) {
        self.inner.rotate_left(count);
    }

    /// Replace all raw rows.
    #[inline]
    pub fn replace_inner(&mut self, rows: Vec<Row<T>>) {
        self.inner = RowBuffer::from_rows(rows);
    }

    /// Remove and return all rows in bottom-to-top order.
    #[inline]
    pub fn take_all(&mut self) -> Vec<Row<T>> {
        std::mem::take(&mut self.inner).into_rows()
    }

    /// Compute the row index for an Alacritty line coordinate.
    #[inline]
    fn compute_index(&self, requested: Line) -> usize {
        debug_assert!(requested.0 < self.visible_lines as i32);

        let index = -(requested - self.visible_lines).0 as usize - 1;
        debug_assert!(index < self.inner.len());
        index
    }
}

impl<T> Index<Line> for Storage<T> {
    type Output = Row<T>;

    #[inline]
    fn index(&self, index: Line) -> &Self::Output {
        &self.inner[self.compute_index(index)]
    }
}

impl<T> IndexMut<Line> for Storage<T> {
    #[inline]
    fn index_mut(&mut self, index: Line) -> &mut Self::Output {
        let index = self.compute_index(index);
        &mut self.inner[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::GridCell;
    use crate::index::Column;
    use crate::term::cell::Flags;

    impl GridCell for char {
        fn is_empty(&self) -> bool {
            *self == ' ' || *self == '\t'
        }

        fn reset(&mut self, template: &Self) {
            *self = *template;
        }

        fn flags(&self) -> &Flags {
            unimplemented!();
        }

        fn flags_mut(&mut self) -> &mut Flags {
            unimplemented!();
        }
    }

    #[test]
    fn with_capacity_initializes_only_visible_rows() {
        let storage = Storage::<char>::with_capacity(3, 1);

        assert_eq!(storage.inner.len(), 3);
        assert_eq!(storage.visible_lines, 3);
    }

    #[test]
    fn indexing_maps_visible_and_history_lines() {
        let mut storage = Storage::<char>::with_capacity(3, 1);
        storage[Line(0)] = filled_row('0');
        storage[Line(1)] = filled_row('1');
        storage[Line(2)] = filled_row('2');
        storage.initialize(1, 1);
        storage[Line(-1)] = filled_row('h');

        assert_eq!(storage[Line(2)], filled_row('2'));
        assert_eq!(storage[Line(1)], filled_row('1'));
        assert_eq!(storage[Line(0)], filled_row('0'));
        assert_eq!(storage[Line(-1)], filled_row('h'));
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn indexing_above_inner_len() {
        let storage = Storage::<char>::with_capacity(1, 1);
        let _ = &storage[Line(-1)];
    }

    #[test]
    fn rotating_up_moves_the_top_row_into_history() {
        let mut storage = labeled_storage();
        storage.initialize(1, 1);
        storage.rotate(-1);

        assert_eq!(storage[Line(2)], filled_row('\0'));
        assert_eq!(storage[Line(1)], filled_row('2'));
        assert_eq!(storage[Line(0)], filled_row('1'));
        assert_eq!(storage[Line(-1)], filled_row('0'));
    }

    #[test]
    fn opposite_rotations_restore_row_order() {
        let mut storage = labeled_storage();
        let original = storage.clone();

        storage.rotate(2);
        storage.rotate(-2);
        storage.rotate_down(1);
        storage.rotate(-1);

        assert_eq!(storage, original);
    }

    #[test]
    fn growing_visible_lines_appends_only_requested_rows() {
        let mut storage = labeled_storage();
        storage.grow_visible_lines(4);

        assert_eq!(storage.len(), 4);
        assert_eq!(storage.visible_lines, 4);
        assert_eq!(storage[Line(3)], filled_row('2'));
        assert_eq!(storage[Line(0)], filled_row('\0'));
    }

    #[test]
    fn shrinking_drops_oldest_rows_immediately() {
        let mut storage = labeled_storage();
        storage.initialize(2, 1);
        storage[Line(-1)] = filled_row('a');
        storage[Line(-2)] = filled_row('b');

        storage.shrink_lines(2);

        assert_eq!(storage.len(), 3);
        assert_eq!(storage[Line(0)], filled_row('0'));
    }

    #[test]
    fn shrinking_and_growing_visible_lines_preserves_remaining_rows() {
        let mut storage = labeled_storage();
        storage.shrink_visible_lines(2);
        assert_eq!(storage.len(), 2);
        assert_eq!(storage[Line(1)], filled_row('2'));
        assert_eq!(storage[Line(0)], filled_row('1'));

        storage.grow_visible_lines(3);
        assert_eq!(storage.len(), 3);
        assert_eq!(storage[Line(2)], filled_row('2'));
        assert_eq!(storage[Line(1)], filled_row('1'));
        assert_eq!(storage[Line(0)], filled_row('\0'));
    }

    #[test]
    fn initialize_does_not_preallocate_rows() {
        let mut storage = Storage::<char>::with_capacity(24, 1);

        for expected in 25..=100_000 {
            storage.initialize(1, 1);
            assert_eq!(storage.len(), expected);
            assert_eq!(storage.inner.len(), expected);
        }

        assert_eq!(storage.inner.chunks.len(), 100_000_usize.div_ceil(ROW_CHUNK_SIZE));
        assert!(storage.inner.chunks.iter().all(|chunk| chunk.len() <= ROW_CHUNK_SIZE));
    }

    #[test]
    fn rotations_across_chunk_boundaries_preserve_every_row() {
        let mut storage = Storage::<char>::with_capacity(24, 1);
        storage.initialize(3 * ROW_CHUNK_SIZE, 1);
        for index in 0..storage.len() {
            storage.inner[index][Column(0)] = char::from_u32((index % 95 + 32) as u32).unwrap();
        }
        let original = storage.clone();

        storage.rotate(-(ROW_CHUNK_SIZE as isize + 17));
        storage.rotate(ROW_CHUNK_SIZE as isize + 17);

        assert_eq!(storage, original);
    }

    #[test]
    fn take_and_replace_inner_preserve_bottom_to_top_order() {
        let mut storage = labeled_storage();
        storage.rotate(-1);

        let rows = storage.take_all();
        assert_eq!(storage.inner.len(), 0);
        assert_eq!(rows, vec![filled_row('0'), filled_row('2'), filled_row('1')]);

        storage.replace_inner(rows);
        assert_eq!(storage.len(), 3);
        assert_eq!(storage[Line(2)], filled_row('0'));
        assert_eq!(storage[Line(0)], filled_row('1'));
    }

    fn labeled_storage() -> Storage<char> {
        let mut storage = Storage::with_capacity(3, 1);
        storage[Line(0)] = filled_row('0');
        storage[Line(1)] = filled_row('1');
        storage[Line(2)] = filled_row('2');
        storage
    }

    fn filled_row(content: char) -> Row<char> {
        let mut row = Row::new(1);
        row[Column(0)] = content;
        row
    }
}
