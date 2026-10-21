use super::super::hyperlinks::{HyperlinkMatch, RegexSearches, find_from_grid_point};
use super::*;
use alacritty_terminal::{
    Term,
    event::VoidListener,
    index::{Column, Line, Point as AlacPoint},
    term::{Config, test::TermSize},
};
use util::paths::PathStyle;

fn readings_of(line: &str, link: &str) -> Vec<&'static str> {
    let start = line.find(link).expect("link is in the line");
    readings_at(line, start..start + link.len())
}

fn readings_at(line: &str, link: std::ops::Range<usize>) -> Vec<&'static str> {
    let readings = boundaries(line, link)
        .map(|boundaries| boundaries.readings())
        .unwrap_or_default();
    readings
        .into_iter()
        .map(|reading| &*line[reading].to_owned().leak())
        .collect()
}

fn term_with(text: &str, columns: usize) -> Term<VoidListener> {
    let mut term = Term::new(Config::default(), &TermSize::new(columns, 4), VoidListener);
    let mut parser = vte::ansi::Processor::<vte::ansi::StdSyncHandler>::new();
    parser.advance(&mut term, text.as_bytes());
    term
}

fn find(term: &Term<VoidListener>, line: i32, column: usize) -> Option<HyperlinkMatch> {
    let settings = crate::terminal_settings::TerminalSettings::default();
    let mut searches = RegexSearches::new(&settings.path_hyperlink_regexes, 1000);
    find_from_grid_point(
        term,
        AlacPoint::new(Line(line), Column(column)),
        &mut searches,
        PathStyle::local(),
    )
}

#[test]
fn every_fragment_of_a_spaced_name_offers_the_whole_name() {
    let path = "/home/user/TransFICC_FY27_SaaS Critical Hygiene - NAB - TransFICC response.xlsx";
    let line = format!("Wrote {path}");
    let mut start = "Wrote ".len();
    for fragment in path.split(' ') {
        let readings = readings_at(&line, start..start + fragment.len());
        assert!(readings.contains(&path), "{fragment}: {readings:?}");
        start += fragment.len() + 1;
    }
}

#[test]
fn readings_are_listed_longest_first() {
    let readings = readings_of("open /tmp/a b c", "b");
    assert_eq!(
        readings,
        [
            "open /tmp/a b c",
            "open /tmp/a b",
            "/tmp/a b c",
            "/tmp/a b",
            "b c"
        ]
    );
}

#[test]
fn more_than_one_space_separates_names() {
    assert!(readings_of("Documents  Music", "Music").is_empty());
    assert_eq!(readings_of("a  b c", "b"), ["b c"]);
}

#[test]
fn quotes_and_brackets_bound_a_reading() {
    assert_eq!(readings_of("cp '/tmp/a b' x", "/tmp/a"), ["/tmp/a b"]);
    assert_eq!(readings_of("cp '/tmp/a b' x", "b"), ["/tmp/a b"]);
    assert_eq!(
        readings_of("(see /tmp/a b).", "b"),
        ["see /tmp/a b", "/tmp/a b"]
    );
    assert_eq!(
        readings_of("see /tmp/a b, then", "/tmp/a"),
        ["see /tmp/a b", "see /tmp/a", "/tmp/a b"]
    );
}

#[test]
fn a_parenthesised_copy_number_stays_in_the_name() {
    assert_eq!(
        readings_of("/tmp/report (1).pdf", "/tmp/report"),
        ["/tmp/report (1).pdf"]
    );
    assert_eq!(
        readings_of("/tmp/report (1)", "/tmp/report"),
        ["/tmp/report (1)"]
    );
}

#[test]
fn a_reading_joins_at_most_max_tokens() {
    let line = vec!["w"; MAX_TOKENS + 4].join(" ");
    let readings = readings_of(&line, "w");
    assert!(!readings.is_empty());
    assert!(
        readings
            .iter()
            .all(|reading| reading.split(' ').count() <= MAX_TOKENS)
    );
}

#[test]
fn a_hovered_fragment_carries_its_readings_onto_the_grid() {
    let path = "/tmp/Q3 report.xlsx";
    let term = term_with(&format!("ls {path}"), 40);

    let found = find(&term, 0, 12).expect("the fragment is a path");
    assert_eq!(found.text, "report.xlsx");
    let reading = found
        .spaced_readings
        .iter()
        .find(|reading| reading.maybe_path == path)
        .expect("the whole name is a reading");
    assert_eq!(reading.range.start(), crate::Point::new(0, 3));
    assert_eq!(
        reading.range.end(),
        crate::Point::new(0, 3 + path.len() - 1)
    );
}

#[test]
fn readings_count_wide_characters_as_two_cells() {
    let term = term_with("/tmp/数据 表.csv", 40);

    let found = find(&term, 0, 10).expect("the fragment is a path");
    assert_eq!(found.text, "表.csv");
    let reading = &found.spaced_readings[0];
    assert_eq!(reading.maybe_path, "/tmp/数据 表.csv");
    assert_eq!(reading.range.start(), crate::Point::new(0, 0));
    // `/tmp/` is five cells, each of the three CJK characters two, `.csv` four
    // and the space one: sixteen cells.
    assert_eq!(reading.range.end(), crate::Point::new(0, 15));
}

#[test]
fn readings_follow_a_terminal_wrapped_line() {
    let term = term_with("/tmp/Q3 report.xlsx", 10);

    let found = find(&term, 1, 3).expect("the fragment is a path");
    assert_eq!(found.text, "report.xlsx");
    let reading = &found.spaced_readings[0];
    assert_eq!(reading.maybe_path, "/tmp/Q3 report.xlsx");
    assert_eq!(reading.range.start(), crate::Point::new(0, 0));
    assert_eq!(reading.range.end(), crate::Point::new(1, 8));
}

#[test]
fn a_rewritten_path_offers_no_readings() {
    let term = term_with(r#"File "/tmp/a b.py", line 3 in x"#, 60);

    let found = find(&term, 0, 8).expect("the Python pattern matches");
    assert_eq!(found.text, "/tmp/a b.py:3");
    assert!(found.spaced_readings.is_empty());
}
