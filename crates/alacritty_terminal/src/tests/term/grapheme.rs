//! Streaming cluster regressions from the imported terminal conformance suite.
use super::*;
use crate::event::VoidListener;
use crate::term::test::TermSize;
use crate::vte::ansi::{Processor, StdSyncHandler};

fn parse(text: &str, fragmented: bool, columns: usize) -> Term<VoidListener> {
    let mut term = Term::new(Config::default(), &TermSize::new(columns, 3), VoidListener);
    let mut parser = Processor::<StdSyncHandler>::new();
    if fragmented {
        for byte in text.as_bytes() {
            parser.advance(&mut term, &[*byte]);
        }
    } else {
        parser.advance(&mut term, text.as_bytes());
    }
    term
}

#[test]
fn emoji_and_indic_clusters_remain_atomic_across_batches() {
    for text in ["👩‍💻", "👨‍👩‍👧", "👋🏿", "🇬🇧", "क्ष", "#️", "\u{06dd}A", "😀\u{200d}©", "ᄀ\u{16d63}"]
    {
        for fragmented in [false, true] {
            let term = parse(text, fragmented, 8);
            let cell = &term.grid[Line(0)][Column(0)];
            let actual: String = std::iter::once(cell.c)
                .chain(cell.zerowidth().unwrap_or_default().iter().copied())
                .collect();
            assert_eq!(actual, text);
            assert!(cell.flags.contains(Flags::WIDE_CHAR), "{text:?}");
            assert_eq!(term.grid.cursor.point.column, Column(2), "{text:?}");
        }
    }
}

#[test]
fn variation_selectors_resize_the_cluster_and_cursor() {
    let term = parse("🍋☔︎X", false, 4);
    assert_eq!(term.grid[Line(0)][Column(2)].c, '☔');
    assert!(!term.grid[Line(0)][Column(2)].flags.contains(Flags::WIDE_CHAR));
    assert_eq!(term.grid[Line(0)][Column(3)].c, 'X');
    let term = parse("ab#️X", true, 3);
    assert_eq!(term.grid[Line(0)][Column(2)].c, ' ');
    assert!(term.grid[Line(0)][Column(2)].flags.contains(Flags::LEADING_WIDE_CHAR_SPACER));
    assert_eq!(term.grid[Line(1)][Column(0)].c, '#');
    assert!(term.grid[Line(1)][Column(0)].flags.contains(Flags::WIDE_CHAR));
    assert_eq!(term.grid[Line(1)][Column(2)].c, 'X');
}

#[test]
fn combining_storage_is_bounded_and_does_not_consume_following_text() {
    let text = format!("A{}X", "\u{301}".repeat(100));
    let term = parse(&text, false, 8);
    assert_eq!(term.grid[Line(0)][Column(0)].zerowidth().unwrap().len(), 23);
    assert_eq!(term.grid[Line(0)][Column(1)].c, 'X');
}

#[test]
fn a_partial_scalar_does_not_consume_the_ascii_following_it() {
    let text = "日本語 e\u{301} 👩‍💻X";
    let expected = parse(text, true, 80);
    for split in 1..text.len() {
        let mut actual = Term::new(Config::default(), &TermSize::new(80, 3), VoidListener);
        let mut parser = Processor::<StdSyncHandler>::new();
        parser.advance(&mut actual, &text.as_bytes()[..split]);
        parser.advance(&mut actual, &text.as_bytes()[split..]);
        assert_eq!(actual.grid, expected.grid, "split {split}");
    }
}

fn one_cluster(scalars: &[char]) -> bool {
    let text = scalars.iter().collect::<String>();
    text.graphemes(true).next().is_some_and(|cluster| cluster.len() == text.len())
}

/// Everything the shortcuts take from the classes, checked against the segmenter for every
/// scalar: exactly within the table, and where above it they over-approximate, in the direction
/// that only sends a scalar on to segmentation.
#[test]
fn join_classes_agree_with_the_segmenter() {
    for c in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
        let classes = join_classes(c);
        let tabled = (c as u32) < TABLED_SCALARS;
        let previous = one_cluster(&['a', c]);
        assert!(classes.previous == previous || !tabled && classes.previous, "{c:?} previous");
        assert_eq!(classes.next, one_cluster(&[c, 'a']), "{c:?} next");
        assert_eq!(is_prepend(c), classes.next, "{c:?} prepend");
        let sequence = one_cluster(&['\u{915}', '\u{94d}', c])
            || one_cluster(&['\u{1f600}', '\u{200d}', c])
            || hangul(c).is_some()
            || regional(c);
        assert!(!sequence || classes.sequence || classes.previous, "{c:?} sequence");
        let takes = one_cluster(&[c, '\u{300}']);
        assert!(takes_marks(c) == takes || !tabled && !takes_marks(c), "{c:?} takes marks");
    }
}

/// `may_join` may only say false of a pair the segmenter splits: every tabled scalar, after
/// every kind of lone scalar the rules distinguish that can be printed (C0 controls cannot).
#[test]
fn may_join_is_false_only_of_pairs_that_split() {
    let previous = [
        'a',
        ' ',
        '\u{2028}',
        '\u{ad}',
        '\u{301}',
        '\u{600}',
        '\u{915}',
        '\u{94d}',
        '\u{1100}',
        '\u{1161}',
        '\u{11a8}',
        '\u{ac00}',
        '\u{ac01}',
        '\u{200d}',
        '\u{1f1e6}',
        '\u{1f600}',
    ];
    for c in (0..TABLED_SCALARS).filter_map(char::from_u32) {
        let classes = join_classes(c);
        for &before in &previous {
            if one_cluster(&[before, c]) {
                assert!(may_join(before, c, classes), "{before:?} {c:?}");
            }
        }
    }
}

#[test]
fn a_prepend_joins_only_what_is_printed_right_after_it() {
    let term = parse("\u{600}A", false, 8);
    assert_eq!(term.grid[Line(0)][Column(0)].zerowidth(), Some(&['A'][..]));
    // Another character printed since, the cursor brought back beside the Prepend draws into
    // the next cell rather than the Prepend's.
    let term = parse("\u{600}\x1b[6Gb\x1b[2GA", false, 8);
    assert_eq!(term.grid[Line(0)][Column(0)].zerowidth(), None);
    assert_eq!(term.grid[Line(0)][Column(1)].c, 'A');
}
