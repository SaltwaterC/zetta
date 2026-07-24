use std::collections::VecDeque;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use super::Row;
use crate::index::Line;

/// Recent history retained with the visible grid in directly mutable storage.
///
/// Search snapshots copy this bounded prefix, while older sealed chunks are shared. Keeping this
/// reasonably small bounds search-start latency without putting copy-on-write checks in the
/// per-character input path.
const LIVE_HISTORY_ROWS: usize = 1_024;
const ARCHIVE_CHUNK_ROWS: usize = 256;

/// An immutable block of older history.
///
/// Uniform chunks preserve the memory benefit of the previous per-row deduplication without
/// comparing rows or touching reference counts for every character written to the live grid.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
enum ArchivedRows<T> {
    Uniform { row: Arc<Row<T>>, len: usize },
    Dense(Vec<Row<T>>),
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct ArchivedChunk<T> {
    rows: ArchivedRows<T>,
}

impl<T> ArchivedChunk<T> {
    fn len(&self) -> usize {
        match &self.rows {
            ArchivedRows::Uniform { len, .. } => *len,
            ArchivedRows::Dense(rows) => rows.len(),
        }
    }

    fn row(&self, index: usize) -> &Row<T> {
        match &self.rows {
            ArchivedRows::Uniform { row, len } => {
                debug_assert!(index < *len);
                row
            },
            ArchivedRows::Dense(rows) => &rows[index],
        }
    }
}

impl<T: Clone> ArchivedChunk<T> {
    fn row_mut(&mut self, index: usize) -> &mut Row<T> {
        if let ArchivedRows::Uniform { row, len } = &self.rows {
            let rows = vec![(**row).clone(); *len];
            self.rows = ArchivedRows::Dense(rows);
        }

        match &mut self.rows {
            ArchivedRows::Dense(rows) => &mut rows[index],
            ArchivedRows::Uniform { .. } => unreachable!(),
        }
    }

    fn truncate_oldest(&mut self, count: usize) {
        debug_assert!(count <= self.len());
        match &mut self.rows {
            ArchivedRows::Uniform { len, .. } => *len -= count,
            ArchivedRows::Dense(rows) => rows.truncate(rows.len() - count),
        }
    }

    fn pop_oldest(&mut self) -> Row<T> {
        match &mut self.rows {
            ArchivedRows::Uniform { row, len } => {
                *len -= 1;
                (**row).clone()
            },
            ArchivedRows::Dense(rows) => rows.pop().unwrap(),
        }
    }

    fn map_rows(
        &mut self,
        map: &mut impl FnMut(&mut Row<T>),
        previous_uniform: &mut Option<(usize, Arc<Row<T>>)>,
    ) {
        match &mut self.rows {
            ArchivedRows::Uniform { row, .. } => {
                let source = Arc::as_ptr(row) as usize;
                if let Some((previous_source, replacement)) = previous_uniform
                    && *previous_source == source
                {
                    *row = replacement.clone();
                    return;
                }

                let mut replacement = (**row).clone();
                map(&mut replacement);
                let replacement = Arc::new(replacement);
                *row = replacement.clone();
                *previous_uniform = Some((source, replacement));
            },
            ArchivedRows::Dense(rows) => {
                *previous_uniform = None;
                for row in rows {
                    map(row);
                }
            },
        }
    }

    fn into_rows(self) -> Vec<Row<T>> {
        match self.rows {
            ArchivedRows::Uniform { row, len } => vec![(*row).clone(); len],
            ArchivedRows::Dense(rows) => rows,
        }
    }
}

impl<T: Clone + PartialEq> ArchivedChunk<T> {
    fn seal(rows: Vec<Row<T>>, adjacent_uniform_row: Option<&Arc<Row<T>>>) -> Self {
        debug_assert!(!rows.is_empty());
        let is_uniform = rows[1..].iter().all(|row| row == &rows[0]);
        let rows = if is_uniform {
            let len = rows.len();
            let row = rows.into_iter().next().unwrap();
            let row = adjacent_uniform_row
                .filter(|candidate| candidate.as_ref() == &row)
                .cloned()
                .unwrap_or_else(|| Arc::new(row));
            ArchivedRows::Uniform { row, len }
        } else {
            ArchivedRows::Dense(rows)
        };
        Self { rows }
    }

    fn uniform_row(&self) -> Option<&Arc<Row<T>>> {
        match &self.rows {
            ArchivedRows::Uniform { row, .. } => Some(row),
            ArchivedRows::Dense(_) => None,
        }
    }
}

/// Tiered terminal row storage.
///
/// The visible grid and a bounded recent-history prefix are ordinary owned rows. Older completed
/// rows are sealed into immutable chunks, which makes complete search snapshots cheap while
/// preserving direct mutable access for terminal output.
///
/// Rows use Alacritty's bottom-to-top order:
///
/// 1. `live` contains the viewport followed by recent history.
/// 2. `archive_head` contains the not-yet-sealed history prefix.
/// 3. `archive_chunks` contains sealed history from newest to oldest.
/// 4. `pending` contains rows appended at the oldest end by resize/ref-test operations.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Storage<T> {
    live: VecDeque<Row<T>>,
    archive_head: VecDeque<Row<T>>,
    archive_chunks: VecDeque<Arc<ArchivedChunk<T>>>,
    archived_lines: usize,
    pending: VecDeque<Row<T>>,
    visible_lines: usize,
}

impl<T: PartialEq> PartialEq for Storage<T> {
    fn eq(&self, other: &Self) -> bool {
        self.visible_lines == other.visible_lines
            && self.len() == other.len()
            && (0..self.len()).all(|index| self.row_at(index) == other.row_at(index))
    }
}

impl<T> Storage<T> {
    #[inline]
    pub fn len(&self) -> usize {
        self.live.len() + self.archive_head.len() + self.archived_lines + self.pending.len()
    }

    #[inline]
    fn compute_index(&self, requested: Line) -> usize {
        debug_assert!(requested.0 < self.visible_lines as i32);
        let index = -(requested - self.visible_lines).0 as usize - 1;
        debug_assert!(index < self.len());
        index
    }

    fn row_at(&self, mut index: usize) -> &Row<T> {
        if index < self.live.len() {
            return &self.live[index];
        }
        index -= self.live.len();

        if index < self.archive_head.len() {
            return &self.archive_head[index];
        }
        index -= self.archive_head.len();

        if index < self.archived_lines {
            let chunk_index = index / ARCHIVE_CHUNK_ROWS;
            let row_index = index % ARCHIVE_CHUNK_ROWS;
            return self.archive_chunks[chunk_index].row(row_index);
        }
        index -= self.archived_lines;

        &self.pending[index]
    }

    pub(crate) fn row_storage_id(&self, requested: Line) -> usize {
        self.row_at(self.compute_index(requested)) as *const Row<T> as usize
    }
}

impl<T: Clone> Storage<T> {
    #[inline]
    pub fn with_capacity(visible_lines: usize, columns: usize) -> Storage<T>
    where
        T: Default,
    {
        let live = (0..visible_lines).map(|_| Row::new(columns)).collect();
        Storage {
            live,
            archive_head: VecDeque::new(),
            archive_chunks: VecDeque::new(),
            archived_lines: 0,
            pending: VecDeque::new(),
            visible_lines,
        }
    }

    fn row_at_mut(&mut self, mut index: usize) -> &mut Row<T> {
        if index < self.live.len() {
            return &mut self.live[index];
        }
        index -= self.live.len();

        if index < self.archive_head.len() {
            return &mut self.archive_head[index];
        }
        index -= self.archive_head.len();

        if index < self.archived_lines {
            let chunk_index = index / ARCHIVE_CHUNK_ROWS;
            let row_index = index % ARCHIVE_CHUNK_ROWS;
            return Arc::make_mut(&mut self.archive_chunks[chunk_index]).row_mut(row_index);
        }
        index -= self.archived_lines;

        &mut self.pending[index]
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
    pub fn shrink_lines(&mut self, mut shrinkage: usize) {
        let pending = shrinkage.min(self.pending.len());
        self.pending.truncate(self.pending.len() - pending);
        shrinkage -= pending;

        while shrinkage != 0 {
            let Some(chunk) = self.archive_chunks.back_mut() else {
                break;
            };
            let count = shrinkage.min(chunk.len());
            if count == chunk.len() {
                self.archive_chunks.pop_back();
            } else {
                Arc::make_mut(chunk).truncate_oldest(count);
            }
            self.archived_lines -= count;
            shrinkage -= count;
        }

        let head = shrinkage.min(self.archive_head.len());
        self.archive_head.truncate(self.archive_head.len() - head);
        shrinkage -= head;

        self.live.truncate(self.live.len() - shrinkage);
    }

    /// Detach all history rows without destroying their cell allocations.
    #[inline]
    pub fn take_history(&mut self) -> Self {
        let history_live = self.live.split_off(self.visible_lines);
        let history = Self {
            live: history_live,
            archive_head: std::mem::take(&mut self.archive_head),
            archive_chunks: std::mem::take(&mut self.archive_chunks),
            archived_lines: std::mem::take(&mut self.archived_lines),
            pending: std::mem::take(&mut self.pending),
            visible_lines: 0,
        };
        history
    }

    /// Resize every retained row, cloning only archive chunks held by an active snapshot.
    pub(crate) fn resize_columns_without_reflow(&mut self, columns: usize)
    where
        T: Default + crate::grid::GridCell,
    {
        let mut resize = |row: &mut Row<T>| {
            if row.len() < columns {
                row.grow(columns);
            } else {
                row.shrink(columns);
            }
        };

        for row in &mut self.live {
            resize(row);
        }
        for row in &mut self.archive_head {
            resize(row);
        }
        let mut previous_uniform = None;
        for chunk in &mut self.archive_chunks {
            Arc::make_mut(chunk).map_rows(&mut resize, &mut previous_uniform);
        }
        for row in &mut self.pending {
            resize(row);
        }
    }

    /// Destroy at most one bounded allocation group.
    pub fn reclaim_next_chunk(&mut self) -> bool {
        if !self.pending.is_empty() {
            let keep = self.pending.len().saturating_sub(ARCHIVE_CHUNK_ROWS);
            self.pending.truncate(keep);
            return true;
        }
        if let Some(chunk) = self.archive_chunks.pop_back() {
            self.archived_lines -= chunk.len();
            return true;
        }
        if !self.archive_head.is_empty() {
            let keep = self.archive_head.len().saturating_sub(ARCHIVE_CHUNK_ROWS);
            self.archive_head.truncate(keep);
            return true;
        }
        if !self.live.is_empty() {
            let keep = self.live.len().saturating_sub(ARCHIVE_CHUNK_ROWS);
            self.live.truncate(keep);
            return true;
        }
        false
    }

    /// Release capacity which is no longer used by retained rows.
    #[inline]
    pub fn truncate(&mut self) {
        self.live.shrink_to_fit();
        self.archive_head.shrink_to_fit();
        self.pending.shrink_to_fit();
    }

    /// Append rows at the oldest end. Normal terminal scrolling uses [`Self::scroll_up`].
    #[inline]
    pub fn initialize(&mut self, additional_rows: usize, columns: usize)
    where
        T: Default,
    {
        self.pending.extend((0..additional_rows).map(|_| Row::new(columns)));
    }

    #[inline]
    pub fn swap(&mut self, a: Line, b: Line) {
        let a = self.compute_index(a);
        let b = self.compute_index(b);
        if a == b {
            return;
        }

        // Distinct logical indices cannot alias. Archived uniform chunks are expanded by
        // `row_at_mut` before either pointer is returned.
        unsafe {
            let a = self.row_at_mut(a) as *mut Row<T>;
            let b = self.row_at_mut(b) as *mut Row<T>;
            std::ptr::swap(a, b);
        }
    }

    /// Fast path for moving complete viewport rows into history.
    pub fn scroll_up(&mut self, positions: usize, growth: usize, columns: usize)
    where
        T: Default + PartialEq,
    {
        debug_assert!(growth <= positions);
        for index in 0..positions {
            let row = if index < growth {
                Row::new(columns)
            } else {
                self.pop_oldest().unwrap_or_else(|| Row::new(columns))
            };
            self.live.push_front(row);
        }
        self.archive_live_excess();
    }

    /// Rotate the complete logical buffer. This is not used by the normal output scroll path.
    pub fn rotate(&mut self, count: isize)
    where
        T: PartialEq,
    {
        debug_assert!(count.unsigned_abs() <= self.len());
        if count > 0 {
            self.rotate_down(count as usize);
        } else {
            for _ in 0..count.unsigned_abs() {
                let row = self.pop_oldest().unwrap();
                self.live.push_front(row);
                self.archive_live_excess();
            }
        }
    }

    /// Rotate all existing lines down in history.
    pub fn rotate_down(&mut self, count: usize)
    where
        T: PartialEq,
    {
        debug_assert!(count <= self.len());
        for _ in 0..count {
            let row = self.live.pop_front().unwrap();
            self.pending.push_back(row);
            let live_target = self.len().min(self.visible_lines.saturating_add(LIVE_HISTORY_ROWS));
            while self.live.len() < live_target {
                let row = self.pop_newest_after_live().unwrap();
                self.live.push_back(row);
            }
        }
    }

    /// Replace all raw rows.
    pub fn replace_inner(&mut self, rows: Vec<Row<T>>)
    where
        T: PartialEq,
    {
        self.live.clear();
        self.archive_head.clear();
        self.archive_chunks.clear();
        self.archived_lines = 0;
        self.pending.clear();

        let live_len = rows.len().min(self.visible_lines.saturating_add(LIVE_HISTORY_ROWS));
        let mut rows = rows.into_iter();
        self.live.extend(rows.by_ref().take(live_len));
        let archived = rows.collect::<Vec<_>>();
        self.rebuild_archive(archived);
    }

    /// Remove and return all rows in bottom-to-top order.
    pub fn take_all(&mut self) -> Vec<Row<T>> {
        let mut rows = Vec::with_capacity(self.len());
        rows.extend(std::mem::take(&mut self.live));
        rows.extend(std::mem::take(&mut self.archive_head));
        for chunk in std::mem::take(&mut self.archive_chunks) {
            let chunk = Arc::try_unwrap(chunk).unwrap_or_else(|chunk| (*chunk).clone()).into_rows();
            rows.extend(chunk);
        }
        self.archived_lines = 0;
        rows.extend(std::mem::take(&mut self.pending));
        rows
    }

    fn pop_oldest(&mut self) -> Option<Row<T>> {
        if let Some(row) = self.pending.pop_back() {
            return Some(row);
        }

        if let Some(chunk) = self.archive_chunks.back_mut() {
            let row = Arc::make_mut(chunk).pop_oldest();
            self.archived_lines -= 1;
            if chunk.len() == 0 {
                self.archive_chunks.pop_back();
            }
            return Some(row);
        }

        self.archive_head.pop_back().or_else(|| self.live.pop_back())
    }

    fn pop_newest_after_live(&mut self) -> Option<Row<T>> {
        if let Some(row) = self.archive_head.pop_front() {
            return Some(row);
        }

        if let Some(chunk) = self.archive_chunks.pop_front() {
            self.archived_lines -= chunk.len();
            let chunk = Arc::try_unwrap(chunk).unwrap_or_else(|chunk| (*chunk).clone());
            let mut rows = VecDeque::from(chunk.into_rows());
            let row = rows.pop_front().unwrap();
            debug_assert!(self.archive_head.is_empty());
            self.archive_head = rows;
            return Some(row);
        }

        self.pending.pop_front()
    }

    fn archive_live_excess(&mut self)
    where
        T: PartialEq,
    {
        let live_limit = self.visible_lines.saturating_add(LIVE_HISTORY_ROWS);
        while self.live.len() > live_limit {
            self.archive_head.push_front(self.live.pop_back().unwrap());
        }

        while self.archive_head.len() >= ARCHIVE_CHUNK_ROWS {
            let keep = self.archive_head.len() - ARCHIVE_CHUNK_ROWS;
            let rows = self.archive_head.split_off(keep).into();
            let adjacent = self.archive_chunks.front().and_then(|chunk| chunk.uniform_row());
            let chunk = ArchivedChunk::seal(rows, adjacent);
            self.archive_chunks.push_front(Arc::new(chunk));
            self.archived_lines += ARCHIVE_CHUNK_ROWS;
        }
    }

    fn rebuild_archive(&mut self, rows: Vec<Row<T>>)
    where
        T: PartialEq,
    {
        if rows.is_empty() {
            return;
        }

        let head_len = rows.len() % ARCHIVE_CHUNK_ROWS;
        let mut rows = rows.into_iter();
        self.archive_head.extend(rows.by_ref().take(head_len));
        loop {
            let chunk = rows.by_ref().take(ARCHIVE_CHUNK_ROWS).collect::<Vec<_>>();
            if chunk.is_empty() {
                break;
            }
            debug_assert_eq!(chunk.len(), ARCHIVE_CHUNK_ROWS);
            let adjacent = self.archive_chunks.back().and_then(|chunk| chunk.uniform_row());
            let chunk = ArchivedChunk::seal(chunk, adjacent);
            self.archive_chunks.push_back(Arc::new(chunk));
            self.archived_lines += ARCHIVE_CHUNK_ROWS;
        }
    }
}

impl<T> Index<Line> for Storage<T> {
    type Output = Row<T>;

    #[inline]
    fn index(&self, index: Line) -> &Self::Output {
        self.row_at(self.compute_index(index))
    }
}

impl<T: Clone> IndexMut<Line> for Storage<T> {
    #[inline]
    fn index_mut(&mut self, index: Line) -> &mut Self::Output {
        let index = self.compute_index(index);
        self.row_at_mut(index)
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
    fn live_rows_are_directly_mutable_and_snapshots_copy_only_the_live_prefix() {
        let mut storage = Storage::<char>::with_capacity(3, 1);
        storage[Line(0)][Column(0)] = 'a';
        let snapshot = storage.clone();
        let snapshot_id = snapshot.row_storage_id(Line(0));

        storage[Line(0)][Column(0)] = 'b';

        assert_eq!(storage[Line(0)][Column(0)], 'b');
        assert_eq!(snapshot[Line(0)][Column(0)], 'a');
        assert_ne!(storage.row_storage_id(Line(0)), snapshot_id);
    }

    #[test]
    fn old_history_is_sealed_and_shared_by_snapshots() {
        let mut storage = Storage::<char>::with_capacity(1, 2);
        for _ in 0..(LIVE_HISTORY_ROWS + 2 * ARCHIVE_CHUNK_ROWS) {
            storage[Line(0)][Column(0)] = 'x';
            storage.scroll_up(1, 1, 2);
        }
        let archived_line = Line(-((LIVE_HISTORY_ROWS + 1) as i32));
        let older_archived_line = Line(-((LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS + 1) as i32));
        let snapshot = storage.clone();

        assert_eq!(storage.row_storage_id(archived_line), snapshot.row_storage_id(archived_line));
        assert_eq!(
            storage.row_storage_id(archived_line),
            storage.row_storage_id(older_archived_line),
            "adjacent uniform chunks should share one archived row"
        );
        assert_eq!(storage.archive_chunks.len(), 2);
        assert!(matches!(storage.archive_chunks[0].rows, ArchivedRows::Uniform { .. }));
    }

    #[test]
    fn resizing_archived_uniform_chunks_preserves_sharing_and_snapshots() {
        let mut storage = Storage::<char>::with_capacity(1, 2);
        for _ in 0..(LIVE_HISTORY_ROWS + 2 * ARCHIVE_CHUNK_ROWS) {
            storage[Line(0)][Column(0)] = 'x';
            storage.scroll_up(1, 1, 2);
        }
        let snapshot = storage.clone();
        let archived_line = Line(-((LIVE_HISTORY_ROWS + 1) as i32));
        let older_archived_line = Line(-((LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS + 1) as i32));

        storage.resize_columns_without_reflow(3);

        assert_eq!(storage[archived_line].len(), 3);
        assert_eq!(snapshot[archived_line].len(), 2);
        assert_eq!(
            storage.row_storage_id(archived_line),
            storage.row_storage_id(older_archived_line)
        );
    }

    #[test]
    fn indexing_maps_visible_live_and_archived_lines() {
        let mut storage = Storage::<char>::with_capacity(3, 1);
        for index in 0..(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS + 4) {
            storage[Line(0)][Column(0)] =
                char::from_u32((index % 26) as u32 + u32::from(b'a')).unwrap();
            storage.scroll_up(1, 1, 1);
        }

        assert_eq!(storage.len(), 3 + LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS + 4);
        assert_eq!(storage[Line(2)][Column(0)], '\0');
        assert_eq!(storage[Line(-1)][Column(0)], 'j');
        assert_eq!(
            storage[Line(-((LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS) as i32))][Column(0)],
            'e'
        );
    }

    #[test]
    fn taking_history_detaches_rows_and_preserves_the_viewport() {
        let mut storage = Storage::<char>::with_capacity(3, 1);
        storage[Line(0)] = filled_row('0');
        storage[Line(1)] = filled_row('1');
        storage[Line(2)] = filled_row('2');
        for _ in 0..(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS) {
            storage.scroll_up(1, 1, 1);
        }

        let history = storage.take_history();

        assert_eq!(storage.len(), 3);
        assert_eq!(history.len(), LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS);
    }

    #[test]
    fn reclaiming_history_is_incremental_by_bounded_chunk() {
        let mut storage = Storage::<char>::with_capacity(1, 1);
        for _ in 0..(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS + 1) {
            storage.scroll_up(1, 1, 1);
        }
        let mut history = storage.take_history();
        let before = history.len();

        assert!(history.reclaim_next_chunk());
        assert!(before - history.len() <= ARCHIVE_CHUNK_ROWS);
    }

    #[test]
    fn shrinking_drops_oldest_rows_across_archive_boundaries() {
        let mut storage = Storage::<char>::with_capacity(1, 1);
        for index in 0..(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS + 10) {
            storage[Line(0)][Column(0)] =
                char::from_u32((index % 26) as u32 + u32::from(b'a')).unwrap();
            storage.scroll_up(1, 1, 1);
        }

        storage.shrink_lines(ARCHIVE_CHUNK_ROWS + 5);

        assert_eq!(storage.len(), 1 + LIVE_HISTORY_ROWS + 5);
        assert_eq!(storage[Line(-((LIVE_HISTORY_ROWS + 5) as i32))][Column(0)], 'b');
    }

    #[test]
    fn rotating_down_pulls_rows_from_the_archive_without_materializing_all_history() {
        let mut storage = Storage::<char>::with_capacity(1, 1);
        for index in 0..(LIVE_HISTORY_ROWS + 2 * ARCHIVE_CHUNK_ROWS) {
            storage[Line(0)][Column(0)] =
                char::from_u32((index % 26) as u32 + u32::from(b'a')).unwrap();
            storage.scroll_up(1, 1, 1);
        }
        let mut expected = storage.clone().take_all();
        expected.rotate_left(1);

        storage.rotate_down(1);

        assert_eq!(storage.take_all(), expected);
    }

    #[test]
    fn take_and_replace_inner_preserve_bottom_to_top_order() {
        let mut storage = labeled_storage();
        storage.rotate(-1);

        let rows = storage.take_all();
        assert_eq!(rows, vec![filled_row('0'), filled_row('2'), filled_row('1')]);

        storage.replace_inner(rows.clone());
        assert_eq!(storage.take_all(), rows);
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
