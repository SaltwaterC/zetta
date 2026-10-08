use super::*;

use alacritty_terminal::term::{Config, Term};
use gpui::{TestAppContext, px};
use vte::ansi::Handler as _;

use crate::TerminalBounds;
use crate::alacritty::{WakeupGate, ZedListener};

/// A terminal `columns` wide holding `lines` of output, with history for all of it.
fn term_with_lines<'a>(
    columns: usize,
    lines: impl IntoIterator<Item = &'a str>,
) -> Term<ZedListener> {
    let bounds = TerminalBounds::new(
        px(10.),
        px(10.),
        gpui::bounds(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(10. * columns as f32), px(40.)),
        ),
    );
    let (events_tx, _) = futures::channel::mpsc::unbounded();
    let listener = ZedListener::new(events_tx, WakeupGate::new());
    let config = Config {
        scrolling_history: 1_000_000,
        ..Config::default()
    };
    let mut term = Term::new(config, &bounds, listener);
    for line in lines {
        for character in line.chars() {
            term.input(character);
        }
        term.newline();
        term.grid_mut().cursor.point.column = Column(0);
    }
    term
}

fn search_all(grid: &Grid<Cell>, query: Search, chunk_lines: usize) -> (SearchMatches, usize) {
    let mut search = ScrollbackSearch::new(grid, query);
    while !search.advance(grid, chunk_lines, MAX_SEARCH_MATCHES) {}
    let scanned = search.physical_rows_scanned;
    (search.finish(), scanned)
}

fn literal(query: &str) -> Search {
    Search::new_literal(query).unwrap()
}

/// Each match as `(line, start column, end column)`.
fn spans(matches: &SearchMatches) -> Vec<(i32, usize, i32, usize)> {
    matches
        .ranges
        .iter()
        .map(|range| {
            (
                range.start().line,
                range.start().column,
                range.end().line,
                range.end().column,
            )
        })
        .collect()
}

#[test]
fn scrollback_search_yields_between_chunks_and_limits_to_newest_matches() {
    let mut term = term_with_lines(4, []);
    for line in 0..4 {
        term.grid_mut()[Line(line)][Column(0)].c = 'x';
    }

    let mut search = ScrollbackSearch::new(term.grid(), Search::new("x").unwrap());
    let mut chunks = 1;
    while !search.advance(term.grid(), 1, 2) {
        chunks += 1;
    }
    let result = search.finish();

    assert!(chunks > 1);
    assert!(result.limit_reached);
    assert_eq!(result.ranges.len(), 2);
    assert_eq!(result.total_count, 4);
    assert_eq!(result.ranges.get(0).unwrap().start().line, 2);
    assert_eq!(result.ranges.get(1).unwrap().start().line, 3);
}

#[test]
fn scrollback_search_narrows_from_capped_character_matches_to_exact_word_matches() {
    let term = term_with_lines(80, std::iter::repeat_n("zzzz Zetta benchmark output", 101));

    let mut broad = ScrollbackSearch::new(term.grid(), literal("z"));
    while !broad.advance(term.grid(), 7, 256) {}
    let broad = broad.finish();
    assert!(broad.limit_reached);
    assert_eq!(broad.ranges.len(), 256);
    assert_eq!(broad.total_count, 505);

    let (narrow, physical_rows_scanned) = search_all(term.grid(), literal("zetta"), 7);
    assert!(
        physical_rows_scanned <= term.total_lines(),
        "the bounded directly mutable history prefix should be scanned only once, scanned \
         {physical_rows_scanned} physical rows for {} retained rows",
        term.total_lines()
    );
    assert!(!narrow.limit_reached);
    assert_eq!(narrow.total_count, 101);
    assert_eq!(narrow.ranges.len(), 101);
}

#[test]
fn ascii_queries_ignore_case_unless_they_have_an_uppercase_letter() {
    let term = term_with_lines(20, ["Cargo cargo CARGO"]);

    let (any_case, _) = search_all(term.grid(), literal("cargo"), SEARCH_CHUNK_LINES);
    let (exact, _) = search_all(term.grid(), literal("Cargo"), SEARCH_CHUNK_LINES);

    assert_eq!(any_case.total_count, 3);
    assert_eq!(spans(&exact), [(0, 0, 0, 4)]);
}

#[test]
fn non_ascii_queries_follow_unicode_case_rules() {
    let term = term_with_lines(20, ["café CAFÉ Café"]);

    let (any_case, _) = search_all(term.grid(), literal("café"), SEARCH_CHUNK_LINES);
    let (exact, _) = search_all(term.grid(), literal("CAFÉ"), SEARCH_CHUNK_LINES);

    assert_eq!(
        spans(&any_case),
        [(0, 0, 0, 3), (0, 5, 0, 8), (0, 10, 0, 13)]
    );
    assert_eq!(spans(&exact), [(0, 5, 0, 8)]);
}

#[test]
fn non_ascii_queries_are_literal_rather_than_patterns() {
    let term = term_with_lines(20, ["a.é aXé"]);

    let (found, _) = search_all(term.grid(), literal("a.é"), SEARCH_CHUNK_LINES);

    assert_eq!(spans(&found), [(0, 0, 0, 2)]);
}

#[test]
fn matches_after_wide_characters_point_at_their_own_cells() {
    let term = term_with_lines(30, ["界界 needle é界needle"]);

    let (ascii, _) = search_all(term.grid(), literal("needle"), SEARCH_CHUNK_LINES);
    let (wide, _) = search_all(term.grid(), literal("é界n"), SEARCH_CHUNK_LINES);

    // Each wide character takes two columns: "界界 " ends at column 4.
    assert_eq!(spans(&ascii), [(0, 5, 0, 10), (0, 15, 0, 20)]);
    assert_eq!(spans(&wide), [(0, 12, 0, 15)]);
}

#[test]
fn matches_span_the_rows_of_a_wrapped_line() {
    // Ten columns: "0123456789" fills the first row, "needle" starts at the eighth column.
    let term = term_with_lines(10, ["0123456needle", "0123456néedle"]);

    let (ascii, _) = search_all(term.grid(), literal("needle"), SEARCH_CHUNK_LINES);
    let (unicode, _) = search_all(term.grid(), literal("NÉEDLE"), SEARCH_CHUNK_LINES);
    let (unicode_any_case, _) = search_all(term.grid(), literal("néedle"), SEARCH_CHUNK_LINES);

    // Four screen lines: the second wrapped line's newline scrolls the first up by one.
    assert_eq!(spans(&ascii), [(-1, 7, 0, 2)]);
    assert!(unicode.ranges.is_empty());
    assert_eq!(spans(&unicode_any_case), [(1, 7, 2, 2)]);
}

#[test]
fn literal_matches_may_overlap() {
    let term = term_with_lines(10, ["aaaa"]);

    let (found, _) = search_all(term.grid(), literal("aa"), SEARCH_CHUNK_LINES);

    assert_eq!(spans(&found), [(0, 0, 0, 1), (0, 1, 0, 2), (0, 2, 0, 3)]);
}

#[test]
fn repeated_output_is_read_once_for_any_query() {
    let term = term_with_lines(20, std::iter::repeat_n("répété ascii", 3_000));
    assert!(term.total_lines() > 3_000);

    for query in ["ascii", "RÉPÉTÉ".to_lowercase().as_str()] {
        let (found, scanned) = search_all(term.grid(), literal(query), SEARCH_CHUNK_LINES);
        assert_eq!(found.total_count, 3_000, "{query}");
        // The live rows and the unsealed history prefix are each read once; sealed history
        // shares one row.
        assert!(scanned < 1_400, "{query}: read {scanned} rows");
    }
}

#[test]
fn empty_queries_search_nothing() {
    assert!(Search::new_literal("").is_none());
}

fn distinct_lines(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            if index % 1_000 == 0 {
                // A wrapped line every so often, for partition boundaries to avoid splitting.
                format!("needle {index:06} {}", "w".repeat(30))
            } else {
                format!("line {index:06} needle")
            }
        })
        .collect()
}

#[test]
fn partitions_cover_the_grid_and_keep_wrapped_lines_whole() {
    // Every line wraps across three rows, so a boundary placed by row count lands inside one.
    let lines = (0..MIN_PARTITION_LINES + 1)
        .map(|index| format!("{index:06} {}", "w".repeat(60)))
        .collect::<Vec<_>>();
    let term = term_with_lines(24, lines.iter().map(String::as_str));
    let grid = term.grid();

    let split = partitions(grid, 4);

    assert_eq!(split.len(), 3, "{split:?}");
    assert_eq!(split[0].0, grid.bottommost_line());
    assert_eq!(split.last().unwrap().1, grid.topmost_line());
    for pair in split.windows(2) {
        assert_eq!(pair[0].1 - 1, pair[1].0);
        assert!(!grid.row_wraps(pair[1].0), "{pair:?} splits a wrapped line");
    }
    assert_eq!(partitions(grid, 1).len(), 1);
}

fn range(line: i32) -> Range {
    Range::new(crate::Point::new(line, 0), crate::Point::new(line, 0))
}

#[test]
fn progress_reports_only_matches_known_to_be_the_newest_and_each_once() {
    let mut progress = Progress::new(2, 10);
    progress.record(Report {
        partition: 1,
        matches: vec![range(-10), range(-11)],
        counted: 2,
        finished: false,
    });
    progress.record(Report {
        partition: 0,
        matches: vec![range(5)],
        counted: 1,
        finished: false,
    });

    assert!(progress.has_newest());
    let provisional = progress.update();
    assert_eq!(provisional.older_matches, [range(5)]);
    assert_eq!(provisional.total_count, 3);
    assert!(provisional.limit_reached);
    assert!(!provisional.complete);
    assert!(!progress.changed());

    progress.record(Report {
        partition: 0,
        matches: Vec::new(),
        counted: 0,
        finished: true,
    });
    // The older partition's matches follow once the newer one has finished.
    let settled = progress.update();
    assert_eq!(settled.older_matches, [range(-10), range(-11)]);
    assert!(!settled.limit_reached);
    assert!(!settled.complete);

    progress.record(Report {
        partition: 1,
        matches: vec![range(-12)],
        counted: 1,
        finished: true,
    });
    let complete = progress.update();
    assert_eq!(complete.older_matches, [range(-12)]);
    assert_eq!(complete.total_count, 4);
    assert!(complete.complete);
    assert!(!complete.limit_reached);
}

#[test]
fn progress_stops_keeping_matches_at_the_limit() {
    let mut progress = Progress::new(2, 3);
    progress.record(Report {
        partition: 0,
        matches: vec![range(5), range(4)],
        counted: 2,
        finished: true,
    });
    progress.record(Report {
        partition: 1,
        matches: vec![range(-1), range(-2)],
        counted: 2,
        finished: false,
    });
    assert_eq!(
        progress.update().older_matches,
        [range(5), range(4), range(-1)]
    );

    progress.record(Report {
        partition: 1,
        matches: vec![range(-3)],
        counted: 1,
        finished: true,
    });
    assert!(!progress.has_newest());
    let complete = progress.update();
    assert!(complete.older_matches.is_empty());
    assert_eq!(complete.total_count, 5);
    assert!(complete.limit_reached);
    assert!(complete.complete);
}

#[test]
fn ranges_are_indexed_oldest_first_and_keep_their_place_as_older_ones_arrive() {
    let mut ranges = SearchRanges::default();
    ranges.extend_older([range(5), range(4)]);
    assert_eq!(ranges.get(1), Some(&range(5)));

    // Two older matches arrive: the newest is still the last, two places further on.
    ranges.extend_older([range(2), range(1)]);
    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges.get(0), Some(&range(1)));
    assert_eq!(ranges.get(3), Some(&range(5)));
    assert_eq!(ranges.get(4), None);
    assert_eq!(
        ranges.iter().copied().collect::<Vec<_>>(),
        [range(1), range(2), range(4), range(5)]
    );
}

#[test]
fn ranges_on_screen_are_found_including_ones_crossing_its_edges() {
    let span = |start, end| Range::new(crate::Point::new(start, 0), crate::Point::new(end, 2));
    let mut ranges = SearchRanges::default();
    ranges.extend_older([
        span(3, 3),
        span(1, 2),
        span(0, 0),
        span(-3, -2),
        span(-6, -4),
        span(-8, -7),
    ]);

    let visible = ranges.intersecting(-5..=0).copied().collect::<Vec<_>>();

    assert_eq!(visible, [span(0, 0), span(-3, -2), span(-6, -4)]);
    assert_eq!(ranges.intersecting(10..=12).count(), 0);
    assert_eq!(ranges.intersecting(-20..=-9).count(), 0);
    assert_eq!(SearchRanges::default().intersecting(0..=5).count(), 0);
}

#[gpui::test]
async fn parallel_search_matches_a_sequential_one(cx: &mut TestAppContext) {
    cx.executor().set_num_cpus(4);
    let lines = distinct_lines(2 * MIN_PARTITION_LINES + 500);
    let term = term_with_lines(24, lines.iter().map(String::as_str));
    let grid = Arc::new(term.grid().clone());
    assert!(partitions(&grid, 3).len() > 1);

    for query in ["needle", "000777", "no such text"] {
        let (expected, _) = search_all(&grid, literal(query), SEARCH_CHUNK_LINES);
        let (updates_tx, updates) = async_channel::unbounded();
        search_grid(grid.clone(), literal(query), &cx.executor(), &updates_tx).await;
        drop(updates_tx);
        let mut result = SearchMatches::default();
        let mut reports = 0;
        while let Ok(update) = updates.try_recv() {
            assert!(
                !result.complete,
                "{query}: an update after the complete one"
            );
            result.record(update);
            reports += 1;
        }
        assert!(reports > 0, "{query}: a search always reports its result");

        assert!(result.complete, "{query}");
        assert_eq!(result.total_count, expected.total_count, "{query}");
        assert_eq!(result.limit_reached, expected.limit_reached, "{query}");
        assert_eq!(result.ranges, expected.ranges, "{query}");
    }
}
