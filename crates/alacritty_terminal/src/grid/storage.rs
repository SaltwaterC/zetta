use std::collections::VecDeque;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use super::archive::{CompactRows, DecodedChunks};
use super::{GridCell, Row};
use crate::index::Line;

/// Recent history retained with the visible grid in directly mutable storage.
///
/// Search snapshots copy this bounded prefix, while older sealed chunks are shared. Keeping this
/// reasonably small bounds search-start latency without putting copy-on-write checks in the
/// per-character input path.
const LIVE_HISTORY_ROWS: usize = 1_024;
const ARCHIVE_CHUNK_ROWS: usize = 256;
/// Sealing a uniform chunk releases at most `ARCHIVE_CHUNK_ROWS` rows (all of them when it shares
/// the adjacent chunk's row), which is what the next chunk's worth of scrolling consumes.
const RECYCLED_ROWS_LIMIT: usize = ARCHIVE_CHUNK_ROWS;

/// An immutable block of older history.
///
/// Uniform chunks preserve the memory benefit of the previous per-row deduplication without
/// comparing rows or touching reference counts for every character written to the live grid.
/// Any other chunk is compact when its cells allow it, and only rows when they do not; see
/// [`super::archive`].
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Deserialize))]
enum ArchivedRows<T> {
    Uniform {
        row: Arc<Row<T>>,
        len: usize,
    },
    Dense(Vec<Row<T>>),
    /// Serialized as `Dense`, which keeps the format free of the encoding; last, so that the
    /// other variants keep their indices.
    #[cfg_attr(feature = "serde", serde(skip))]
    Compact(CompactRows<T>),
}

#[cfg(feature = "serde")]
impl<T: Serialize> Serialize for ArchivedRows<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename = "ArchivedRows")]
        enum Rows<'a, T> {
            Uniform { row: &'a Arc<Row<T>>, len: usize },
            Dense(&'a [Row<T>]),
        }

        match self {
            Self::Uniform { row, len } => Rows::Uniform { row, len: *len }.serialize(serializer),
            Self::Dense(rows) => Rows::Dense(rows).serialize(serializer),
            Self::Compact(rows) => Rows::Dense(&rows.rows()).serialize(serializer),
        }
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct ArchivedChunk<T> {
    rows: ArchivedRows<T>,
}

impl<T> ArchivedChunk<T> {
    fn uniform_row(&self) -> Option<&Arc<Row<T>>> {
        match &self.rows {
            ArchivedRows::Uniform { row, .. } => Some(row),
            ArchivedRows::Compact(_) | ArchivedRows::Dense(_) => None,
        }
    }

    /// Whether every row is its own allocation, so that removing one yields a reusable row.
    fn owns_rows(&self) -> bool {
        matches!(self.rows, ArchivedRows::Dense(_))
    }

    fn len(&self) -> usize {
        match &self.rows {
            ArchivedRows::Uniform { len, .. } => *len,
            ArchivedRows::Compact(rows) => rows.len(),
            ArchivedRows::Dense(rows) => rows.len(),
        }
    }
}

impl<T: Clone> ArchivedChunk<T> {
    fn row_mut(&mut self, index: usize) -> &mut Row<T> {
        match &self.rows {
            ArchivedRows::Uniform { row, len } => {
                self.rows = ArchivedRows::Dense(vec![(**row).clone(); *len]);
            },
            ArchivedRows::Compact(rows) => self.rows = ArchivedRows::Dense(rows.rows()),
            ArchivedRows::Dense(_) => (),
        }

        match &mut self.rows {
            ArchivedRows::Dense(rows) => &mut rows[index],
            ArchivedRows::Uniform { .. } | ArchivedRows::Compact(_) => unreachable!(),
        }
    }

    fn truncate_oldest(&mut self, count: usize) {
        debug_assert!(count <= self.len());
        match &mut self.rows {
            ArchivedRows::Uniform { len, .. } => *len -= count,
            ArchivedRows::Compact(rows) => rows.truncate_oldest(count),
            ArchivedRows::Dense(rows) => rows.truncate(rows.len() - count),
        }
    }

    fn pop_oldest(&mut self) -> Row<T> {
        match &mut self.rows {
            ArchivedRows::Uniform { row, len } => {
                *len -= 1;
                (**row).clone()
            },
            ArchivedRows::Compact(rows) => {
                let row = rows.row(rows.len() - 1);
                rows.truncate_oldest(1);
                row
            },
            ArchivedRows::Dense(rows) => rows.pop().unwrap(),
        }
    }

    /// Resize every row with `map`, which resizes to `columns`. Compact rows apply the resize
    /// as they are decoded instead.
    fn map_rows(
        &mut self,
        columns: usize,
        map: &mut impl FnMut(&mut Row<T>),
        previous_uniform: &mut Option<(usize, Arc<Row<T>>)>,
    ) {
        match &mut self.rows {
            ArchivedRows::Compact(rows) => {
                *previous_uniform = None;
                rows.resize_columns(columns);
            },
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
            ArchivedRows::Compact(rows) => rows.rows(),
            ArchivedRows::Dense(rows) => rows,
        }
    }
}

impl<T: GridCell + Default + PartialEq> ArchivedChunk<T> {
    /// Seal `rows`, handing the rows a uniform or compact chunk no longer needs to `recycled`.
    fn seal(
        rows: Vec<Row<T>>,
        adjacent_uniform_row: Option<&Arc<Row<T>>>,
        recycled: &mut RecycledRows<T>,
    ) -> Self {
        debug_assert!(!rows.is_empty());
        let is_uniform = rows[1..].iter().all(|row| row == &rows[0]);
        let rows = if is_uniform {
            let len = rows.len();
            let mut rows = rows.into_iter();
            let row = rows.next().unwrap();
            recycled.extend(rows);
            let row = match adjacent_uniform_row.filter(|candidate| candidate.as_ref() == &row) {
                Some(shared) => {
                    recycled.extend(std::iter::once(row));
                    shared.clone()
                },
                None => Arc::new(row),
            };
            ArchivedRows::Uniform { row, len }
        } else if let Some(compact) = CompactRows::encode(&rows) {
            recycled.extend(rows.into_iter());
            ArchivedRows::Compact(compact)
        } else {
            ArchivedRows::Dense(rows)
        };
        Self { rows }
    }
}

/// Rows a sealed uniform or compact chunk no longer needs, kept as the next lines scrolled into
/// the grid.
///
/// Output seals a chunk every `ARCHIVE_CHUNK_ROWS` lines. Without reuse, every scrolled line
/// allocated and initialized a row only for sealing to free it. The grid resets a row as it
/// scrolls in, so these are spare allocations rather than content: snapshots do not copy them.
#[derive(Debug)]
struct RecycledRows<T>(Vec<Row<T>>);

impl<T> Default for RecycledRows<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> Clone for RecycledRows<T> {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl<T> RecycledRows<T> {
    /// A spare row `columns` wide.
    #[inline]
    fn take(&mut self, columns: usize) -> Option<Row<T>> {
        // A resize leaves rows of the previous width behind; those are dropped, not resized.
        while let Some(row) = self.0.pop() {
            if row.len() == columns {
                return Some(row);
            }
        }
        None
    }

    fn extend(&mut self, rows: impl Iterator<Item = Row<T>>) {
        let room = RECYCLED_ROWS_LIMIT.saturating_sub(self.0.len());
        self.0.extend(rows.take(room));
    }

    fn clear(&mut self) {
        self.0 = Vec::new();
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
    #[cfg_attr(feature = "serde", serde(skip))]
    recycled: RecycledRows<T>,
    #[cfg_attr(feature = "serde", serde(skip))]
    decoded: DecodedChunks<ArchivedChunk<T>, T>,
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

    /// Every cell the terminal reads or writes resolves its row here, and nearly all of them are
    /// live rows, so only that test is inlined.
    #[inline]
    fn row_at(&self, index: usize) -> &Row<T> {
        match self.live.get(index) {
            Some(row) => row,
            None => self.history_row_at(index - self.live.len()),
        }
    }

    #[inline(never)]
    fn history_row_at(&self, mut index: usize) -> &Row<T> {
        if index < self.archive_head.len() {
            return &self.archive_head[index];
        }
        index -= self.archive_head.len();

        if index < self.archived_lines {
            let chunk = &self.archive_chunks[index / ARCHIVE_CHUNK_ROWS];
            let row_index = index % ARCHIVE_CHUNK_ROWS;
            return match &chunk.rows {
                ArchivedRows::Uniform { row, len } => {
                    debug_assert!(row_index < *len);
                    row
                },
                ArchivedRows::Compact(rows) => self.decoded.row(chunk, row_index, || rows.rows()),
                ArchivedRows::Dense(rows) => &rows[row_index],
            };
        }
        index -= self.archived_lines;

        &self.pending[index]
    }

    /// A value equal for two lines exactly when they share storage, stable while the row is.
    pub(crate) fn row_storage_id(&self, requested: Line) -> usize {
        let index = self.compute_index(requested);
        // A decoded compact row lives only until the decoded chunk is released, which would
        // let its address be reused for another row.
        if let Some(archived) = index
            .checked_sub(self.live.len() + self.archive_head.len())
            .filter(|&archived| archived < self.archived_lines)
            && let ArchivedRows::Compact(rows) =
                &self.archive_chunks[archived / ARCHIVE_CHUNK_ROWS].rows
        {
            return rows.row_id(archived % ARCHIVE_CHUNK_ROWS);
        }
        self.row_at(index) as *const Row<T> as usize
    }

    /// Release decoded compact chunks. Reading them again decodes them again.
    #[inline]
    pub(crate) fn release_decoded(&mut self) {
        self.decoded.clear();
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
            recycled: RecycledRows::default(),
            decoded: DecodedChunks::default(),
        }
    }

    /// See [`Self::row_at`].
    #[inline]
    fn row_at_mut(&mut self, index: usize) -> &mut Row<T> {
        let live = self.live.len();
        if index < live {
            return &mut self.live[index];
        }
        self.history_row_at_mut(index - live)
    }

    #[inline(never)]
    fn history_row_at_mut(&mut self, mut index: usize) -> &mut Row<T> {
        self.release_decoded();
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
        self.release_decoded();
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
        self.release_decoded();
        let history_live = self.live.split_off(self.visible_lines);
        Self {
            live: history_live,
            archive_head: std::mem::take(&mut self.archive_head),
            archive_chunks: std::mem::take(&mut self.archive_chunks),
            archived_lines: std::mem::take(&mut self.archived_lines),
            pending: std::mem::take(&mut self.pending),
            visible_lines: 0,
            recycled: RecycledRows::default(),
            decoded: DecodedChunks::default(),
        }
    }

    /// Resize every retained row, cloning only archive chunks held by an active snapshot.
    pub(crate) fn resize_columns_without_reflow(&mut self, columns: usize)
    where
        T: Default + GridCell,
    {
        self.release_decoded();
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
            Arc::make_mut(chunk).map_rows(columns, &mut resize, &mut previous_uniform);
        }
        for row in &mut self.pending {
            resize(row);
        }
        self.recycled.clear();
    }

    /// Destroy at most one bounded allocation group.
    pub fn reclaim_next_chunk(&mut self) -> bool {
        self.release_decoded();
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
        self.recycled.clear();
        self.release_decoded();
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
        T: GridCell + Default + PartialEq,
    {
        debug_assert!(growth <= positions);
        self.release_decoded();
        // The grid resets every row scrolled in, so only the allocation is reused here.
        for index in 0..positions {
            let row = if index < growth {
                self.recycled.take(columns)
            } else {
                self.pop_oldest_allocation(columns)
            }
            .unwrap_or_else(|| Row::new(columns));
            self.live.push_front(row);
        }
        self.archive_live_excess();
    }

    /// Rotate the complete logical buffer. This is not used by the normal output scroll path.
    pub fn rotate(&mut self, count: isize)
    where
        T: GridCell + Default + PartialEq,
    {
        self.release_decoded();
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
        self.release_decoded();
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
        T: GridCell + Default + PartialEq,
    {
        self.release_decoded();
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
        self.release_decoded();
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

    /// Remove the oldest row for its allocation alone. Unlike [`Self::pop_oldest`], a row a
    /// uniform or compact chunk does not hold as its own allocation is not built to be thrown
    /// away.
    fn pop_oldest_allocation(&mut self, columns: usize) -> Option<Row<T>> {
        if self.pending.is_empty()
            && let Some(chunk) = self.archive_chunks.back_mut()
            && !chunk.owns_rows()
        {
            Arc::make_mut(chunk).truncate_oldest(1);
            self.archived_lines -= 1;
            if chunk.len() == 0 {
                self.archive_chunks.pop_back();
            }
            return self.recycled.take(columns);
        }

        self.pop_oldest()
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
        T: GridCell + Default + PartialEq,
    {
        let live_limit = self.visible_lines.saturating_add(LIVE_HISTORY_ROWS);
        while self.live.len() > live_limit {
            self.archive_head.push_front(self.live.pop_back().unwrap());
        }

        while self.archive_head.len() >= ARCHIVE_CHUNK_ROWS {
            let keep = self.archive_head.len() - ARCHIVE_CHUNK_ROWS;
            let rows = self.archive_head.split_off(keep).into();
            let adjacent = self.archive_chunks.front().and_then(|chunk| chunk.uniform_row());
            let chunk = ArchivedChunk::seal(rows, adjacent, &mut self.recycled);
            self.archive_chunks.push_front(Arc::new(chunk));
            self.archived_lines += ARCHIVE_CHUNK_ROWS;
        }
    }

    fn rebuild_archive(&mut self, rows: Vec<Row<T>>)
    where
        T: GridCell + Default + PartialEq,
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
            let chunk = ArchivedChunk::seal(chunk, adjacent, &mut self.recycled);
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

        fn archive_char(&self) -> Option<char> {
            Some(*self)
        }

        fn same_archive_attributes(&self, _other: &Self) -> bool {
            true
        }

        fn with_archive_char(&self, c: char) -> Self {
            c
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

        // The newest visible row is a reused allocation, which the grid resets and storage does
        // not.
        assert_eq!(storage.len(), 3 + LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS + 4);
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
    fn detached_history_reclaims_every_retained_row() {
        let mut storage = Storage::<char>::with_capacity(1, 1);
        for _ in 0..(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS + 1) {
            storage.scroll_up(1, 1, 1);
        }
        let mut history = storage.take_history();

        while history.reclaim_next_chunk() {}

        assert_eq!(history.len(), 0);
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

    /// Scroll `lines` rows of 'x' into history, `columns` wide, growing it as the grid does.
    fn uniform_history(lines: usize, columns: usize) -> Storage<char> {
        let mut storage = Storage::<char>::with_capacity(1, columns);
        for _ in 0..lines {
            storage[Line(0)][Column(0)] = 'x';
            storage.scroll_up(1, 1, columns);
        }
        storage
    }

    #[test]
    fn a_sealed_uniform_chunk_hands_its_rows_to_the_next_scrolled_lines() {
        let mut storage = uniform_history(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS, 2);
        assert!(matches!(storage.archive_chunks[0].rows, ArchivedRows::Uniform { .. }));
        // The first uniform chunk keeps one of its rows.
        let recycled = storage.recycled.0.len();
        assert_eq!(recycled, ARCHIVE_CHUNK_ROWS - 1);

        storage.scroll_up(1, 1, 2);

        assert_eq!(storage.recycled.0.len(), recycled - 1);
        assert_eq!(storage.len(), 2 + LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS);
        let archived_line = Line(-((LIVE_HISTORY_ROWS + 2) as i32));
        assert_eq!(storage[archived_line][Column(0)], 'x');
    }

    /// Scroll `lines` distinct rows into history, `columns` wide.
    fn distinct_history(lines: usize, columns: usize) -> Storage<char> {
        let mut storage = Storage::<char>::with_capacity(1, columns);
        for index in 0..lines {
            for column in 0..columns {
                storage[Line(0)][Column(column)] = letter(index + column);
            }
            storage.scroll_up(1, 1, columns);
        }
        storage
    }

    fn letter(index: usize) -> char {
        char::from_u32((index % 26) as u32 + u32::from(b'a')).unwrap()
    }

    /// The row scrolled off `back` lines ago in [`distinct_history`] of `lines` rows.
    fn distinct_row(lines: usize, back: usize, columns: usize) -> String {
        (0..columns).map(|column| letter(lines - back + column)).collect()
    }

    fn text(row: &Row<char>) -> String {
        row[..].iter().collect()
    }

    #[test]
    fn distinct_rows_seal_compactly_and_hand_their_rows_on() {
        let lines = LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS;
        let storage = distinct_history(lines, 3);

        assert!(matches!(storage.archive_chunks[0].rows, ArchivedRows::Compact(_)));
        assert_eq!(storage.recycled.0.len(), ARCHIVE_CHUNK_ROWS);
        for back in [1, LIVE_HISTORY_ROWS, LIVE_HISTORY_ROWS + 1, lines] {
            let line = Line(-(back as i32));
            assert_eq!(text(&storage[line]), distinct_row(lines, back, 3), "{line:?}");
        }
    }

    #[test]
    fn cells_without_an_archive_character_stay_rows() {
        let mut storage = Storage::<usize>::with_capacity(1, 1);
        for index in 0..(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS) {
            storage[Line(0)][Column(0)] = index + 1;
            storage.scroll_up(1, 1, 1);
        }

        assert!(matches!(storage.archive_chunks[0].rows, ArchivedRows::Dense(_)));
        assert!(storage.recycled.0.is_empty());
    }

    #[test]
    fn compact_rows_decode_once_until_storage_is_changed() {
        let lines = LIVE_HISTORY_ROWS + 2 * ARCHIVE_CHUNK_ROWS;
        let mut storage = distinct_history(lines, 2);
        let oldest = Line(-(lines as i32));
        let newest_archived = Line(-(LIVE_HISTORY_ROWS as i32 + 1));

        let first = &storage[oldest] as *const Row<char>;
        assert_eq!(&storage[oldest] as *const Row<char>, first);
        let _ = &storage[newest_archived];
        assert_eq!(storage.decoded.len(), 2);

        // Ids name rows, not where a decoded row happens to live.
        for line in [Line(0), Line(-1), Line(-(LIVE_HISTORY_ROWS as i32))] {
            let address = &storage[line] as *const Row<char> as usize;
            assert_eq!(storage.row_storage_id(line), address, "{line:?}");
        }
        let id = storage.row_storage_id(oldest);
        assert_ne!(id, storage.row_storage_id(Line(oldest.0 + 1)));
        storage.release_decoded();
        assert_eq!(storage.decoded.len(), 0);
        assert_eq!(storage.row_storage_id(oldest), id);
        assert_eq!(text(&storage[oldest]), distinct_row(lines, lines, 2));

        storage.scroll_up(1, 1, 2);
        assert_eq!(storage.decoded.len(), 0);
    }

    #[test]
    fn snapshots_decode_compact_rows_for_themselves() {
        let lines = LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS;
        let storage = distinct_history(lines, 2);
        let snapshot = storage.clone();
        let oldest = Line(-(lines as i32));

        assert_eq!(text(&snapshot[oldest]), distinct_row(lines, lines, 2));
        assert_eq!(snapshot.decoded.len(), 1);
        assert_eq!(storage.decoded.len(), 0);
        assert!(snapshot == storage);
    }

    #[test]
    fn compact_rows_round_trip_through_every_way_out_of_the_archive() {
        let lines = LIVE_HISTORY_ROWS + 2 * ARCHIVE_CHUNK_ROWS;
        let storage = distinct_history(lines, 3);
        let expected = (1..=lines).map(|back| distinct_row(lines, back, 3)).collect::<Vec<_>>();
        let history = |rows: Vec<Row<char>>| rows[1..].iter().map(text).collect::<Vec<_>>();

        assert_eq!(history(storage.clone().take_all()), expected);

        // Making an archived row mutable expands its chunk.
        let mut mutable = storage.clone();
        let oldest = Line(-(lines as i32));
        mutable[oldest][Column(0)] = '!';
        assert!(mutable.archive_chunks.iter().any(|chunk| chunk.owns_rows()));
        assert_eq!(text(&mutable[oldest]), format!("!{}", &expected[lines - 1][1..]));

        // Pulling archived rows back into the live region, as shrinking the window does.
        let mut rotated = storage.clone();
        rotated.rotate_down(LIVE_HISTORY_ROWS + 1);
        assert_eq!(rotated.len(), storage.len());

        // Dropping the oldest rows of a full history.
        let mut full = storage.clone();
        full.scroll_up(1, 0, 3);
        assert_eq!(full.len(), storage.len());
        assert_eq!(text(&full[oldest]), expected[lines - 2]);
    }

    #[test]
    fn compact_rows_resize_like_rows_without_being_decoded() {
        let lines = LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS;
        let mut storage = distinct_history(lines, 4);
        let oldest = Line(-(lines as i32));
        let original = distinct_row(lines, lines, 4);

        storage.resize_columns_without_reflow(6);
        assert_eq!(storage.decoded.len(), 0);
        assert_eq!(text(&storage[oldest]), format!("{original}\0\0"));

        // Shrinking drops cells, which growing again does not bring back.
        storage.resize_columns_without_reflow(2);
        assert_eq!(text(&storage[oldest]), original[..2]);
        storage.resize_columns_without_reflow(5);
        assert_eq!(text(&storage[oldest]), format!("{}\0\0\0", &original[..2]));
    }

    #[test]
    fn snapshots_do_not_copy_recycled_rows() {
        let storage = uniform_history(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS, 2);
        assert!(!storage.recycled.0.is_empty());

        let snapshot = storage.clone();

        assert!(snapshot.recycled.0.is_empty());
        assert!(snapshot == storage);
    }

    #[test]
    fn recycled_rows_are_only_reused_at_their_own_width() {
        let mut storage = uniform_history(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS, 2);
        storage.recycled.0.push(Row::new(3));

        assert_eq!(storage.recycled.take(2).map(|row| row.len()), Some(2));
        assert_eq!(storage.recycled.take(3).map(|row| row.len()), None);
        assert!(storage.recycled.0.is_empty(), "rows of another width are dropped");

        let mut storage = uniform_history(LIVE_HISTORY_ROWS + ARCHIVE_CHUNK_ROWS, 2);
        storage.resize_columns_without_reflow(3);
        assert!(storage.recycled.0.is_empty());
    }

    #[test]
    fn a_full_history_drops_its_oldest_uniform_row_without_copying_it() {
        let mut storage = uniform_history(LIVE_HISTORY_ROWS + 2 * ARCHIVE_CHUNK_ROWS, 2);
        let len = storage.len();
        let oldest_chunk = storage.archive_chunks.len() - 1;
        let shared = storage.archive_chunks[oldest_chunk].uniform_row().unwrap().clone();
        let recycled = storage.recycled.0.len();

        storage.scroll_up(1, 0, 2);

        assert_eq!(storage.len(), len);
        assert_eq!(storage.archive_chunks[oldest_chunk].len(), ARCHIVE_CHUNK_ROWS - 1);
        assert_eq!(storage.recycled.0.len(), recycled - 1);
        assert!(Arc::ptr_eq(storage.archive_chunks[oldest_chunk].uniform_row().unwrap(), &shared));
        let oldest_line = Line(1 - len as i32);
        assert_eq!(storage[oldest_line][Column(0)], 'x');
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
