//! Scrollback search: matching a query against a grid snapshot, newest output first.
//!
//! A search reads each row as text through [`Grid::row_text`], which walks compact history in
//! its encoded form rather than decoding it into cells: that decoding was three quarters of what a
//! search over distinct output cost. An ASCII query is matched with `memmem`; anything else with a
//! Unicode-aware regex, which keeps the case rules Alacritty's own regex search had for it. Rows
//! sharing storage (repeated output) are matched once.
//!
//! [`search_grid`] splits a large snapshot into partitions searched in parallel, each newest first,
//! and reports results while it runs. Only matches that are known to be the newest are reported:
//! those of the newest partition so far, and of an older one once every newer one has finished.
//! So the newest matches arrive first and every later report only adds older ones, which is what
//! lets a report carry just the matches found since the previous one, and [`SearchRanges`] append
//! them without moving the ones already shown. The count keeps growing until the whole snapshot
//! has been read, so the exact total still arrives, just no longer gating the first results.

use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::{
    grid::{Dimensions as _, Grid, RowText},
    index::{Column, Line, Point as AlacPoint},
    term::cell::Cell,
};
use futures_lite::future::{self, yield_now};
use gpui::{BackgroundExecutor, Priority, Task};
use memchr::memmem;

#[cfg(test)]
use crate::SearchMatches;
use crate::{Range, Search, SearchUpdate};

/// Physical rows one scan step reads before yielding, so a replaced query cancels promptly.
pub(crate) const SEARCH_CHUNK_LINES: usize = 2_048;
/// The newest matches a search keeps for highlighting and navigation; it still counts the rest.
///
/// This bounds memory, not drawing: a frame looks up only the matches it shows (see
/// [`SearchRanges::intersecting`]) and an update carries only new matches, so neither costs more
/// as the matches grow. A match is 32 bytes, and until every newer partition has finished, each of
/// a search's workers may hold up to this many of its own.
pub(crate) const MAX_SEARCH_MATCHES: usize = 100_000;
/// A snapshot smaller than this is searched by one worker; splitting it costs more than it saves.
const MIN_PARTITION_LINES: usize = 65_536;
/// Workers a single search may use. Tab search starts one search per pane.
const MAX_SEARCH_WORKERS: usize = 8;
/// How long a search runs before its first provisional results, and how often after that it
/// reports a growing count. A search finishing sooner reports only its result.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// A search's matches, indexed oldest first, the order navigation visits them in.
///
/// They are stored newest first, the order a running search finds them in, so its older results
/// append rather than shift every match already shown. Both the starts and the ends of the matches
/// are ordered (matches do not nest), which is what lets [`Self::intersecting`] find the matches
/// on screen by binary search instead of walking them all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchRanges {
    newest_first: Vec<Range>,
}

impl SearchRanges {
    #[inline]
    pub fn len(&self) -> usize {
        self.newest_first.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.newest_first.is_empty()
    }

    pub fn clear(&mut self) {
        self.newest_first.clear();
    }

    /// Match `index`, counting from the oldest.
    pub fn get(&self, index: usize) -> Option<&Range> {
        let from_newest = self.newest_first.len().checked_sub(index)?.checked_sub(1)?;
        self.newest_first.get(from_newest)
    }

    /// Oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Range> + ExactSizeIterator {
        self.newest_first.iter().rev()
    }

    /// Add `older`, newest first, each older than every match already held. The matches held
    /// keep their distance from the newest, so their indices move up by `older`'s length.
    pub fn extend_older(&mut self, older: impl IntoIterator<Item = Range>) {
        self.newest_first.extend(older);
    }

    /// The matches on any of `lines`, newest first.
    pub fn intersecting(&self, lines: RangeInclusive<i32>) -> impl Iterator<Item = &Range> {
        let (top, bottom) = lines.into_inner();
        let below = self
            .newest_first
            .partition_point(|range| range.start().line > bottom);
        self.newest_first[below..]
            .iter()
            .take_while(move |range| range.end().line >= top)
    }
}

/// How a [`Search`] matches text.
#[derive(Clone, Debug)]
pub(crate) enum SearchMatcher {
    /// An ASCII query, ignoring ASCII case unless it has an uppercase letter. Matches may overlap.
    Literal {
        /// Boxed because a finder is several times the size of a regex handle.
        finder: Box<memmem::Finder<'static>>,
        case_insensitive: bool,
    },
    /// A regex, ignoring case unless it has an uppercase letter, as Alacritty's regex search does.
    Regex(regex::Regex),
}

impl Search {
    /// A regex search, or `None` when `pattern` is not a valid regex.
    pub fn new(pattern: &str) -> Option<Self> {
        let case_insensitive = !pattern.chars().any(char::is_uppercase);
        let regex = regex::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .ok()?;
        Some(Self {
            matcher: SearchMatcher::Regex(regex),
        })
    }

    /// A search for `query` as written, or `None` when it is empty.
    ///
    /// A query with a non-ASCII character is matched as an escaped regex, so that it follows
    /// Unicode case rules (`é` finds `É`) rather than only ASCII ones.
    pub fn new_literal(query: &str) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        if !query.is_ascii() {
            return Self::new(&regex::escape(query));
        }
        let case_insensitive = !query.bytes().any(|byte| byte.is_ascii_uppercase());
        let needle = if case_insensitive {
            query.to_ascii_lowercase()
        } else {
            query.to_owned()
        };
        Some(Self {
            matcher: SearchMatcher::Literal {
                finder: Box::new(memmem::Finder::new(needle.as_bytes()).into_owned()),
                case_insensitive,
            },
        })
    }
}

impl SearchMatcher {
    /// Append the byte ranges of `text` that match, in ascending order. May lowercase `text`.
    fn find(&self, text: &mut RowText, found: &mut Vec<(usize, usize)>) {
        match self {
            Self::Literal {
                finder,
                case_insensitive,
            } => {
                if *case_insensitive {
                    text.make_ascii_lowercase();
                }
                let haystack = text.as_str().as_bytes();
                let len = finder.needle().len();
                let mut from = 0;
                while let Some(at) = finder.find(&haystack[from..]) {
                    let start = from + at;
                    found.push((start, start + len));
                    // An ASCII needle never matches inside a multibyte character, so stepping one
                    // byte finds overlapping matches without landing on a false one.
                    from = start + 1;
                }
            }
            Self::Regex(regex) => found.extend(
                regex
                    .find_iter(text.as_str())
                    .filter(|found| !found.is_empty())
                    .map(|found| (found.start(), found.end())),
            ),
        }
    }
}

/// A match within a logical line: row offset from its first row, and column.
type LinePoint = (i32, Column);

/// One partition of a search, read newest line first a step at a time.
pub(crate) struct ScrollbackSearch {
    matcher: SearchMatcher,
    next_line: Line,
    oldest_line: Line,
    /// Newest first, at most the limit passed to [`Self::advance`].
    matches: Vec<Range>,
    /// Matches kept so far, including any [`Self::take_matches`] handed on.
    kept: usize,
    total_count: usize,
    text: RowText,
    found: Vec<(usize, usize)>,
    /// The storage of the last logical line read and the matches in it, which repeated output
    /// reuses instead of reading the same rows again.
    cached_rows: Vec<usize>,
    cached_matches: Vec<(LinePoint, LinePoint)>,
    rows: Vec<usize>,
    #[cfg(test)]
    pub(crate) physical_rows_scanned: usize,
}

impl ScrollbackSearch {
    /// A search of all of `grid`.
    #[cfg(test)]
    pub(crate) fn new(grid: &Grid<Cell>, search: Search) -> Self {
        Self::lines(search, grid.bottommost_line(), grid.topmost_line())
    }

    /// A search of the lines from `newest` back to `oldest`.
    fn lines(search: Search, newest: Line, oldest: Line) -> Self {
        Self {
            matcher: search.matcher,
            next_line: newest,
            oldest_line: oldest,
            matches: Vec::new(),
            kept: 0,
            total_count: 0,
            text: RowText::default(),
            found: Vec::new(),
            cached_rows: Vec::new(),
            cached_matches: Vec::new(),
            rows: Vec::new(),
            #[cfg(test)]
            physical_rows_scanned: 0,
        }
    }

    /// Search about `chunk_lines` more physical rows of `grid`, which must not have changed since
    /// the search began. Returns whether the search has finished.
    ///
    /// A step ends at the start of a logical line when one is near, so a wrapped line is matched
    /// whole; an extremely long one is split after one more step's worth of rows.
    pub(crate) fn advance(
        &mut self,
        grid: &Grid<Cell>,
        chunk_lines: usize,
        match_limit: usize,
    ) -> bool {
        if self.next_line < self.oldest_line {
            return true;
        }
        let candidate = Line(
            self.next_line
                .0
                .saturating_sub(chunk_lines.saturating_sub(1) as i32)
                .max(self.oldest_line.0),
        );
        let extension_limit = Line(
            candidate
                .0
                .saturating_sub(chunk_lines as i32)
                .max(self.oldest_line.0),
        );
        let mut chunk_end = candidate;
        while chunk_end > extension_limit && grid.row_wraps(chunk_end - 1) {
            chunk_end -= 1;
        }

        let mut logical_newest = self.next_line;
        while logical_newest >= chunk_end {
            let mut logical_oldest = logical_newest;
            while logical_oldest > chunk_end && grid.row_wraps(logical_oldest - 1) {
                logical_oldest -= 1;
            }
            self.search_logical_line(grid, logical_oldest, logical_newest, match_limit);
            logical_newest = logical_oldest - 1;
        }
        self.next_line = chunk_end - 1;
        self.next_line < self.oldest_line
    }

    fn search_logical_line(
        &mut self,
        grid: &Grid<Cell>,
        oldest: Line,
        newest: Line,
        match_limit: usize,
    ) {
        self.rows.clear();
        self.rows
            .extend((oldest.0..=newest.0).map(|line| grid.row_storage_id(Line(line))));
        if self.rows != self.cached_rows {
            std::mem::swap(&mut self.rows, &mut self.cached_rows);
            #[cfg(test)]
            {
                self.physical_rows_scanned += self.cached_rows.len();
            }
            self.text.clear();
            for line in oldest.0..=newest.0 {
                grid.row_text(Line(line), &mut self.text);
            }
            self.found.clear();
            self.matcher.find(&mut self.text, &mut self.found);
            let relative = |point: AlacPoint| (point.line.0 - oldest.0, point.column);
            self.cached_matches.clear();
            self.cached_matches
                .extend(self.found.iter().map(|&(start, end)| {
                    (
                        relative(self.text.point(start)),
                        relative(self.text.last_point(end)),
                    )
                }));
        }

        // Rightmost first, so the matches kept under the limit are the newest.
        for &(start, end) in self.cached_matches.iter().rev() {
            self.total_count = self.total_count.saturating_add(1);
            if self.kept < match_limit {
                self.kept += 1;
                let point = |(line, column): LinePoint| AlacPoint::new(oldest + line, column);
                self.matches
                    .push(Range::from_alacritty(point(start)..=point(end)));
            }
        }
    }

    /// The matches found since the last call, newest first.
    fn take_matches(&mut self) -> Vec<Range> {
        std::mem::take(&mut self.matches)
    }

    /// All the matches found, for a search driven to its end in one place.
    #[cfg(test)]
    pub(crate) fn finish(self) -> SearchMatches {
        SearchMatches {
            limit_reached: self.total_count > self.matches.len(),
            total_count: self.total_count,
            ranges: SearchRanges {
                newest_first: self.matches,
            },
            complete: true,
        }
    }
}

/// Split `grid` into at most `workers` ranges of lines, newest first, as `(newest, oldest)`.
/// A boundary moves to the start of a logical line when one is near, as a search step's does.
fn partitions(grid: &Grid<Cell>, workers: usize) -> Vec<(Line, Line)> {
    let (top, bottom) = (grid.topmost_line(), grid.bottommost_line());
    let total = (bottom.0 - top.0 + 1) as usize;
    let count = workers.min(total / MIN_PARTITION_LINES).max(1);
    let mut partitions = Vec::with_capacity(count);
    let mut newest = bottom;
    for index in 1..count {
        let mut boundary = Line(bottom.0 + 1 - (total * index / count) as i32);
        let limit = Line((boundary.0 - SEARCH_CHUNK_LINES as i32).max(top.0 + 1));
        while boundary > limit && grid.row_wraps(boundary - 1) {
            boundary -= 1;
        }
        if boundary > newest || boundary <= top {
            continue;
        }
        partitions.push((newest, boundary));
        newest = boundary - 1;
    }
    partitions.push((newest, top));
    partitions
}

/// What one partition found in one step.
struct Report {
    partition: usize,
    /// Newest first.
    matches: Vec<Range>,
    counted: usize,
    finished: bool,
}

#[derive(Default)]
struct PartitionProgress {
    /// Not yet reported, newest first.
    matches: Vec<Range>,
    total_count: usize,
    finished: bool,
}

/// Every partition's results so far, newest partition first.
struct Progress {
    partitions: Vec<PartitionProgress>,
    match_limit: usize,
    /// Matches handed on in updates so far.
    reported: usize,
    /// The count the last update carried.
    reported_count: usize,
}

impl Progress {
    fn new(partitions: usize, match_limit: usize) -> Self {
        Self {
            partitions: (0..partitions)
                .map(|_| PartitionProgress::default())
                .collect(),
            match_limit,
            reported: 0,
            reported_count: 0,
        }
    }

    fn record(&mut self, report: Report) {
        let partition = &mut self.partitions[report.partition];
        // Once the limit has been reported, every match still to come is too old to keep.
        if self.reported < self.match_limit {
            partition.matches.extend(report.matches);
        }
        partition.total_count = partition.total_count.saturating_add(report.counted);
        partition.finished |= report.finished;
    }

    fn complete(&self) -> bool {
        self.partitions.iter().all(|partition| partition.finished)
    }

    /// The partitions whose matches are known to be the newest: every newer one has finished.
    fn settled(&mut self) -> &mut [PartitionProgress] {
        let unfinished = self
            .partitions
            .iter()
            .position(|partition| !partition.finished)
            .map_or(self.partitions.len(), |index| index + 1);
        &mut self.partitions[..unfinished]
    }

    /// Whether an update would carry matches.
    fn has_newest(&mut self) -> bool {
        self.reported < self.match_limit
            && self
                .settled()
                .iter()
                .any(|partition| !partition.matches.is_empty())
    }

    /// Whether an update would show anything new.
    fn changed(&mut self) -> bool {
        self.total_count() != self.reported_count || self.has_newest()
    }

    fn total_count(&self) -> usize {
        self.partitions.iter().fold(0, |total, partition| {
            total.saturating_add(partition.total_count)
        })
    }

    /// The newest matches not yet reported, up to the limit, and the count so far.
    fn update(&mut self) -> SearchUpdate {
        let mut older_matches = Vec::new();
        let mut room = self.match_limit - self.reported;
        for partition in self.settled() {
            partition.matches.truncate(room);
            room -= partition.matches.len();
            if older_matches.is_empty() {
                older_matches = std::mem::take(&mut partition.matches);
            } else {
                older_matches.append(&mut partition.matches);
            }
        }
        self.reported += older_matches.len();
        self.reported_count = self.total_count();
        SearchUpdate {
            older_matches,
            total_count: self.reported_count,
            limit_reached: self.reported_count > self.reported,
            complete: self.complete(),
        }
    }
}

async fn search_partition(
    grid: Arc<Grid<Cell>>,
    mut search: ScrollbackSearch,
    partition: usize,
    reports: async_channel::Sender<Report>,
) {
    loop {
        let counted_before = search.total_count;
        let finished = search.advance(&grid, SEARCH_CHUNK_LINES, MAX_SEARCH_MATCHES);
        let report = Report {
            partition,
            matches: search.take_matches(),
            counted: search.total_count - counted_before,
            finished,
        };
        // A step that found nothing has nothing to report; the coordinator's timer covers time.
        if (finished || report.counted > 0) && reports.send(report).await.is_err() {
            return;
        }
        if finished {
            return;
        }
        yield_now().await;
    }
}

/// Search `grid`, sending provisional results to `updates` while it runs and the complete ones
/// last. Stops early when `updates` is closed; dropping the future cancels its workers.
pub(crate) async fn search_grid(
    grid: Arc<Grid<Cell>>,
    search: Search,
    executor: &BackgroundExecutor,
    updates: &async_channel::Sender<SearchUpdate>,
) {
    let workers = executor
        .num_cpus()
        .saturating_sub(1)
        .clamp(1, MAX_SEARCH_WORKERS);
    let partitions = partitions(&grid, workers);
    let (reports_tx, reports) = async_channel::unbounded();
    let _workers = partitions
        .iter()
        .enumerate()
        .map(|(partition, &(newest, oldest))| {
            let search = ScrollbackSearch::lines(search.clone(), newest, oldest);
            executor.spawn_with_priority(
                Priority::Low,
                search_partition(grid.clone(), search, partition, reports_tx.clone()),
            )
        })
        .collect::<Vec<Task<()>>>();
    drop(reports_tx);

    let mut progress = Progress::new(partitions.len(), MAX_SEARCH_MATCHES);
    let started = Instant::now();
    // When provisional results were last due, whether or not there were any to send.
    let mut last_due: Option<Instant> = None;
    loop {
        let next_report_at = last_due.unwrap_or(started) + PROGRESS_INTERVAL;
        let wait = next_report_at.saturating_duration_since(Instant::now());
        let event = future::or(async { reports.recv().await.ok() }, async {
            executor.timer(wait).await;
            None
        })
        .await;
        if let Some(report) = event {
            progress.record(report);
        }
        // Workers report before they finish, so a closed channel means they were dropped.
        if progress.complete() || (reports.is_closed() && reports.is_empty()) {
            break;
        }

        let now = Instant::now();
        if now < started + PROGRESS_INTERVAL {
            continue;
        }
        // The first matches are reported at once; more matches and a growing count once an
        // interval, which bounds how often the window redraws for a long search.
        let urgent = progress.reported == 0 && progress.has_newest();
        let due = last_due.is_none_or(|at| now >= at + PROGRESS_INTERVAL);
        if !urgent && !due {
            continue;
        }
        // Restart the interval even when nothing changed, or the timer would fire at once again.
        last_due = Some(now);
        if progress.changed() && updates.send(progress.update()).await.is_err() {
            return;
        }
    }
    updates.send(progress.update()).await.ok();
}

#[cfg(test)]
#[path = "../tests/alacritty/search.rs"]
mod tests;
