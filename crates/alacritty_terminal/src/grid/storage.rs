use std::collections::VecDeque;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use super::Row;
use crate::index::Line;

// Keep copy-on-write mutations and deferred destruction bounded tightly enough
// that one shared chunk cannot create a noticeable allocator or lock-time spike.
const ROW_CHUNK_SIZE: usize = 256;

/// A chunked double-ended row buffer.
///
/// Only the first and last chunk can be partially populated, so indexing stays
/// constant-time without one contiguous allocation for every retained row.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct RowBuffer<T> {
    chunks: VecDeque<Arc<VecDeque<Arc<Row<T>>>>>,
    len: usize,
}

impl<T> Default for RowBuffer<T> {
    fn default() -> Self {
        Self { chunks: VecDeque::new(), len: 0 }
    }
}

impl<T: PartialEq> PartialEq for RowBuffer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self
                .chunks
                .iter()
                .flat_map(|chunk| chunk.iter())
                .eq(other.chunks.iter().flat_map(|chunk| chunk.iter()))
    }
}

impl<T> RowBuffer<T> {
    fn position(&self, index: usize) -> (usize, usize) {
        debug_assert!(index < self.len);
        let first_len = self.chunks.front().map_or(0, |chunk| chunk.len());
        if index < first_len {
            (0, index)
        } else {
            let index = index - first_len;
            (1 + index / ROW_CHUNK_SIZE, index % ROW_CHUNK_SIZE)
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn row_storage_id(&self, index: usize) -> usize {
        let (chunk, row) = self.position(index);
        Arc::as_ptr(&self.chunks[chunk][row]) as usize
    }
}

impl<T: Clone> RowBuffer<T> {
    /// Apply a mutation to every row while retaining consecutive row sharing.
    ///
    /// Large terminal workloads frequently contain millions of references to the same
    /// consecutive row. Materializing those rows before a non-reflowing resize both destroys
    /// that compression and can stall the UI thread for seconds.
    fn map_rows_preserving_sharing(&mut self, mut map: impl FnMut(&mut Row<T>)) {
        let mut previous_source = None;
        let mut previous_replacement: Option<Arc<Row<T>>> = None;
        let mut previous_uniform_chunk: Option<(usize, usize, Arc<VecDeque<Arc<Row<T>>>>)> = None;
        let mut previous_source_chunk: Option<(
            usize,
            usize,
            Arc<Row<T>>,
            Arc<VecDeque<Arc<Row<T>>>>,
        )> = None;

        for chunk in &mut self.chunks {
            let source_chunk = Arc::as_ptr(chunk) as usize;
            if let Some((source, last_row_source, last_row_replacement, replacement)) =
                &previous_source_chunk
                && *source == source_chunk
            {
                *chunk = replacement.clone();
                previous_source = Some(*last_row_source);
                previous_replacement = Some(last_row_replacement.clone());
                continue;
            }

            let first = chunk.front().unwrap();
            let first_source = Arc::as_ptr(first) as usize;
            let uniform = chunk.iter().all(|row| Arc::ptr_eq(first, row));
            if uniform {
                if let Some((source, len, replacement)) = &previous_uniform_chunk
                    && *source == first_source
                    && *len == chunk.len()
                {
                    *chunk = replacement.clone();
                    previous_source = Some(first_source);
                    previous_replacement = replacement.front().cloned();
                    previous_source_chunk = Some((
                        source_chunk,
                        first_source,
                        replacement.back().unwrap().clone(),
                        replacement.clone(),
                    ));
                    continue;
                }

                let replacement = if previous_source == Some(first_source) {
                    previous_replacement.as_ref().unwrap().clone()
                } else {
                    let mut replacement = (**first).clone();
                    map(&mut replacement);
                    Arc::new(replacement)
                };
                let replacement_chunk = Arc::new(
                    std::iter::repeat_n(replacement.clone(), chunk.len()).collect::<VecDeque<_>>(),
                );
                *chunk = replacement_chunk.clone();
                previous_source = Some(first_source);
                previous_replacement = Some(replacement);
                previous_uniform_chunk = Some((first_source, chunk.len(), replacement_chunk));
                previous_source_chunk = Some((
                    source_chunk,
                    first_source,
                    chunk.back().unwrap().clone(),
                    chunk.clone(),
                ));
                continue;
            }

            previous_uniform_chunk = None;
            for row in Arc::make_mut(chunk) {
                let source = Arc::as_ptr(row) as usize;
                if previous_source == Some(source) {
                    *row = previous_replacement.as_ref().unwrap().clone();
                    continue;
                }

                let mut replacement = (**row).clone();
                map(&mut replacement);
                let replacement = Arc::new(replacement);
                *row = replacement.clone();
                previous_source = Some(source);
                previous_replacement = Some(replacement);
            }
            previous_source_chunk = Some((
                source_chunk,
                previous_source.unwrap(),
                previous_replacement.as_ref().unwrap().clone(),
                chunk.clone(),
            ));
        }
    }

    fn push_back(&mut self, row: Arc<Row<T>>) {
        if self.chunks.back().is_none_or(|chunk| chunk.len() == ROW_CHUNK_SIZE) {
            self.chunks.push_back(Arc::new(VecDeque::with_capacity(ROW_CHUNK_SIZE)));
        }
        Arc::make_mut(self.chunks.back_mut().unwrap()).push_back(row);
        self.len += 1;
    }

    fn push_front(&mut self, row: Arc<Row<T>>) {
        if self.chunks.front().is_none_or(|chunk| chunk.len() == ROW_CHUNK_SIZE) {
            self.chunks.push_front(Arc::new(VecDeque::with_capacity(ROW_CHUNK_SIZE)));
        }
        Arc::make_mut(self.chunks.front_mut().unwrap()).push_front(row);
        self.len += 1;
    }

    fn pop_back(&mut self) -> Option<Arc<Row<T>>> {
        let row = Arc::make_mut(self.chunks.back_mut()?).pop_back()?;
        if self.chunks.back().is_some_and(|chunk| chunk.is_empty()) {
            self.chunks.pop_back();
        }
        self.len -= 1;
        Some(row)
    }

    fn pop_front(&mut self) -> Option<Arc<Row<T>>> {
        let row = Arc::make_mut(self.chunks.front_mut()?).pop_front()?;
        if self.chunks.front().is_some_and(|chunk| chunk.is_empty()) {
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

    fn split_off(&mut self, at: usize) -> Self {
        assert!(at <= self.len);
        if at == self.len {
            return Self::default();
        }
        if at == 0 {
            return std::mem::take(self);
        }

        let tail_len = self.len - at;
        let (chunk_index, row_index) = self.position(at);
        let mut tail_chunks = if row_index == 0 {
            self.chunks.split_off(chunk_index)
        } else {
            let mut tail_chunks = self.chunks.split_off(chunk_index + 1);
            let boundary_tail = Arc::make_mut(self.chunks.back_mut().unwrap()).split_off(row_index);
            tail_chunks.push_front(Arc::new(boundary_tail));
            tail_chunks
        };
        tail_chunks.retain(|chunk| !chunk.is_empty());
        self.len = at;

        Self { chunks: tail_chunks, len: tail_len }
    }

    fn pop_back_chunk(&mut self) -> Option<Arc<VecDeque<Arc<Row<T>>>>> {
        let chunk = self.chunks.pop_back()?;
        self.len -= chunk.len();
        Some(chunk)
    }

    fn from_rows(rows: Vec<Row<T>>) -> Self {
        let mut buffer = Self::default();
        for row in rows {
            buffer.push_back(Arc::new(row));
        }
        buffer
    }

    fn into_rows(self) -> Vec<Row<T>> {
        let mut rows = Vec::with_capacity(self.len);
        for chunk in self.chunks {
            match Arc::try_unwrap(chunk) {
                Ok(chunk) => rows.extend(
                    chunk
                        .into_iter()
                        .map(|row| Arc::try_unwrap(row).unwrap_or_else(|row| (*row).clone())),
                ),
                Err(chunk) => rows.extend(chunk.iter().map(|row| (**row).clone())),
            }
        }
        rows
    }

    fn shared_row(&self, index: usize) -> Arc<Row<T>> {
        let (chunk, row) = self.position(index);
        self.chunks[chunk][row].clone()
    }

    fn replace_shared_row(&mut self, index: usize, replacement: Arc<Row<T>>) {
        let (chunk, row) = self.position(index);
        Arc::make_mut(&mut self.chunks[chunk])[row] = replacement;
    }
}

impl<T> Index<usize> for RowBuffer<T> {
    type Output = Row<T>;

    fn index(&self, index: usize) -> &Self::Output {
        let (chunk, row) = self.position(index);
        &self.chunks[chunk][row]
    }
}

impl<T: Clone> IndexMut<usize> for RowBuffer<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        let (chunk, row) = self.position(index);
        Arc::make_mut(&mut Arc::make_mut(&mut self.chunks[chunk])[row])
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

    /// Most recently retained history row, used to share identical consecutive output.
    #[cfg_attr(feature = "serde", serde(skip))]
    history_row_candidate: Option<Arc<Row<T>>>,

    /// Shared default row used when growing the scrollback ring.
    #[cfg_attr(feature = "serde", serde(skip))]
    blank_row: Option<Arc<Row<T>>>,
}

impl<T: PartialEq> PartialEq for Storage<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T> Storage<T> {
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Compute the row index for an Alacritty line coordinate.
    #[inline]
    fn compute_index(&self, requested: Line) -> usize {
        debug_assert!(requested.0 < self.visible_lines as i32);

        let index = -(requested - self.visible_lines).0 as usize - 1;
        debug_assert!(index < self.inner.len());
        index
    }

    pub(crate) fn row_storage_id(&self, requested: Line) -> usize {
        self.inner.row_storage_id(self.compute_index(requested))
    }
}

impl<T: Clone> Storage<T> {
    #[inline]
    pub fn with_capacity(visible_lines: usize, columns: usize) -> Storage<T>
    where
        T: Default,
    {
        let mut inner = RowBuffer::default();
        for _ in 0..visible_lines {
            inner.push_back(Arc::new(Row::new(columns)));
        }

        Storage { inner, visible_lines, history_row_candidate: None, blank_row: None }
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

    /// Detach all history rows without destroying their cell allocations.
    #[inline]
    pub fn take_history(&mut self) -> Self {
        let history = self.inner.split_off(self.visible_lines);
        Self {
            inner: history,
            visible_lines: 0,
            history_row_candidate: self.history_row_candidate.take(),
            blank_row: None,
        }
    }

    /// Resize every retained row without expanding shared scrollback rows.
    pub(crate) fn resize_columns_without_reflow(&mut self, columns: usize)
    where
        T: Default + crate::grid::GridCell,
    {
        self.inner.map_rows_preserving_sharing(|row| {
            if row.len() < columns {
                row.grow(columns);
            } else {
                row.shrink(columns);
            }
        });
        self.history_row_candidate = None;
        self.blank_row = None;
    }

    /// Destroy at most one allocation chunk, returning whether work was performed.
    pub fn reclaim_next_chunk(&mut self) -> bool {
        let Some(chunk) = self.inner.pop_back_chunk() else {
            return false;
        };
        drop(chunk);
        true
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
        let blank_row = self.blank_row.get_or_insert_with(|| Arc::new(Row::new(columns))).clone();
        for _ in 0..additional_rows {
            self.inner.push_back(blank_row.clone());
        }
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
    pub fn rotate(&mut self, count: isize)
    where
        T: PartialEq,
    {
        debug_assert!(count.unsigned_abs() <= self.inner.len());

        if count >= 0 {
            self.inner.rotate_left(count as usize);
        } else {
            let count = count.unsigned_abs();
            self.inner.rotate_right(count);
            self.deduplicate_new_history_rows(count);
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
        self.history_row_candidate = None;
        self.blank_row = None;
    }

    /// Remove and return all rows in bottom-to-top order.
    #[inline]
    pub fn take_all(&mut self) -> Vec<Row<T>> {
        self.history_row_candidate = None;
        self.blank_row = None;
        std::mem::take(&mut self.inner).into_rows()
    }

    fn deduplicate_new_history_rows(&mut self, count: usize)
    where
        T: PartialEq,
    {
        let end = (self.visible_lines + count).min(self.inner.len());
        for index in self.visible_lines..end {
            let row = self.inner.shared_row(index);
            if self
                .history_row_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.as_ref() == row.as_ref())
            {
                self.inner.replace_shared_row(
                    index,
                    self.history_row_candidate.as_ref().unwrap().clone(),
                );
            } else {
                self.history_row_candidate = Some(row);
            }
        }
    }
}

impl<T> Index<Line> for Storage<T> {
    type Output = Row<T>;

    #[inline]
    fn index(&self, index: Line) -> &Self::Output {
        &self.inner[self.compute_index(index)]
    }
}

impl<T: Clone> IndexMut<Line> for Storage<T> {
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
    fn cloned_storage_preserves_an_immutable_snapshot_after_live_mutation() {
        let mut storage = Storage::<char>::with_capacity(3, 1);
        storage[Line(0)][Column(0)] = 'a';
        let snapshot = storage.clone();

        storage[Line(0)][Column(0)] = 'b';

        assert_eq!(storage[Line(0)][Column(0)], 'b');
        assert_eq!(snapshot[Line(0)][Column(0)], 'a');
    }

    #[test]
    fn consecutive_identical_history_rows_share_cell_storage() {
        let mut storage = Storage::<char>::with_capacity(2, 2);
        storage[Line(0)][Column(0)] = 'x';
        storage.initialize(1, 2);
        storage.rotate(-1);

        storage[Line(0)][Column(0)] = 'x';
        storage.initialize(1, 2);
        storage.rotate(-1);

        let newest_history = storage.inner.shared_row(storage.visible_lines);
        let older_history = storage.inner.shared_row(storage.visible_lines + 1);
        assert!(Arc::ptr_eq(&newest_history, &older_history));

        storage[Line(-1)][Column(0)] = 'y';
        assert_eq!(storage[Line(-1)][Column(0)], 'y');
        assert_eq!(storage[Line(-2)][Column(0)], 'x');
    }

    #[test]
    fn repeated_output_retains_one_row_allocation_per_run() {
        let mut storage = Storage::<char>::with_capacity(1, 2);
        for _ in 0..10_000 {
            storage[Line(0)][Column(0)] = 'x';
            storage.initialize(1, 2);
            storage.rotate(-1);
        }

        let first = storage.inner.shared_row(storage.visible_lines);
        assert!(
            (storage.visible_lines..storage.len())
                .all(|index| Arc::ptr_eq(&first, &storage.inner.shared_row(index)))
        );
    }

    #[test]
    fn bulk_row_mapping_preserves_sharing_across_chunks() {
        let shared = Arc::new(filled_row('x'));
        let mut rows = RowBuffer::default();
        for _ in 0..(ROW_CHUNK_SIZE * 3 + 1) {
            rows.push_back(shared.clone());
        }

        let mut mapped_rows = 0;
        rows.map_rows_preserving_sharing(|row| {
            mapped_rows += 1;
            row.grow(3);
        });
        assert_eq!(mapped_rows, 1);

        let first = rows.shared_row(0);
        assert_eq!(first.len(), 3);
        assert!(
            (1..rows.len()).all(|index| Arc::ptr_eq(&first, &rows.shared_row(index))),
            "mapping expanded shared rows into separate allocations"
        );
        assert!(Arc::ptr_eq(&rows.chunks[0], &rows.chunks[1]));
        assert!(Arc::ptr_eq(&rows.chunks[1], &rows.chunks[2]));

        let mut remapped_rows = 0;
        rows.map_rows_preserving_sharing(|row| {
            remapped_rows += 1;
            row.grow(4);
        });
        assert_eq!(remapped_rows, 1);
        assert_eq!(rows.shared_row(0).len(), 4);
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
    fn taking_history_detaches_rows_and_preserves_the_viewport() {
        let mut storage = Storage::<char>::with_capacity(3, 1);
        storage[Line(0)] = filled_row('0');
        storage[Line(1)] = filled_row('1');
        storage[Line(2)] = filled_row('2');
        storage.initialize(2, 1);
        storage[Line(-1)] = filled_row('a');
        storage[Line(-2)] = filled_row('b');

        let history = storage.take_history();

        assert_eq!(storage.len(), 3);
        assert_eq!(storage[Line(0)], filled_row('0'));
        assert_eq!(storage[Line(1)], filled_row('1'));
        assert_eq!(storage[Line(2)], filled_row('2'));
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn reclaiming_history_is_incremental_by_storage_chunk() {
        let mut storage = Storage::<char>::with_capacity(1, 1);
        storage.initialize(ROW_CHUNK_SIZE + 1, 1);
        let mut history = storage.take_history();

        assert!(history.reclaim_next_chunk());
        assert!(history.len() > 0);
        assert!(history.reclaim_next_chunk());
        assert_eq!(history.len(), 0);
        assert!(!history.reclaim_next_chunk());
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
