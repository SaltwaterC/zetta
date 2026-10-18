use super::super::hyperlinks::{HyperlinkMatch, RegexSearches, find_from_grid_point};
use super::*;
use alacritty_terminal::{
    event::VoidListener,
    index::{Line, Point as AlacPoint},
    term::{Config, test::TermSize},
};
use util::paths::PathStyle;

fn drawn_rows(rows: &[&str], columns: usize) -> Term<VoidListener> {
    let mut term = Term::new(
        Config::default(),
        &TermSize::new(columns, rows.len() + 2),
        VoidListener,
    );
    let mut parser = vte::ansi::Processor::<vte::ansi::StdSyncHandler>::new();
    for (row, text) in rows.iter().enumerate() {
        parser.advance(&mut term, format!("\x1b[{};1H{text}", row + 1).as_bytes());
    }
    term
}

fn find(term: &Term<VoidListener>, row: usize, column: usize) -> Option<HyperlinkMatch> {
    let settings = crate::terminal_settings::TerminalSettings::default();
    let mut searches = RegexSearches::new(&settings.path_hyperlink_regexes, 1000);
    find_from_grid_point(
        term,
        AlacPoint::new(Line(row as i32), Column(column)),
        &mut searches,
        PathStyle::local(),
    )
}

#[test]
fn application_wrapped_paths_resolve_from_every_fragment() {
    for path in [
        "C:/Users/saltw/source/repos/zetta/crates/terminal/src/alacritty/hyperlinks.rs:124",
        r"C:\Users\saltw\source\repos\zetta\crates\terminal\src\alacritty\hyperlinks.rs:124",
        "/home/user/source/zetta/crates/terminal/src/alacritty/hyperlinks.rs:124:9",
        "crates/terminal/src/alacritty/hyperlinks.rs",
    ] {
        for columns in [20, 32, 40] {
            let chunks: Vec<_> = path
                .as_bytes()
                .chunks(columns - 3)
                .map(|s| std::str::from_utf8(s).unwrap())
                .collect();
            let rows: Vec<_> = chunks.iter().map(|chunk| format!("  {chunk}")).collect();
            let rows: Vec<_> = rows.iter().map(String::as_str).collect();
            let term = drawn_rows(&rows, columns);
            for (row, chunk) in chunks.iter().enumerate() {
                for column in 2..chunk.len() + 2 {
                    let found = find(&term, row, column).expect("every fragment is clickable");
                    assert_eq!(
                        found.text, path,
                        "row {row}, column {column}, width {columns}"
                    );
                    assert_eq!(found.range.start(), crate::Point::new(0, 2));
                    assert_eq!(
                        found.range.end(),
                        crate::Point::new(
                            (rows.len() - 1) as i32,
                            chunks.last().unwrap().len() + 1
                        )
                    );
                }
            }
        }
    }
}

#[test]
fn separate_lines_do_not_become_one_path() {
    for (rows, columns, expected) in [
        (["  src/first.rs", "  src/second.rs"], 40, "src/first.rs"),
        (["  src/first.rs", "  C:/second.rs"], 15, "src/first.rs"),
        (["  src/first.rs", "  ~/second.rs"], 15, "src/first.rs"),
        (["  src/first.rs", "Second paragraph"], 20, "src/first.rs"),
        (["  src/first.rs)", "  src/second.rs"], 16, "src/first.rs"),
        (
            ["  src/first.rs", "  \x1b[31msecond.rs"],
            15,
            "src/first.rs",
        ),
    ] {
        let term = drawn_rows(&rows, columns);
        assert_eq!(find(&term, 0, 3).unwrap().text, expected, "{rows:?}");
    }
}

#[test]
fn unicode_cells_and_continuation_padding_map_to_grid_coordinates() {
    let term = drawn_rows(&["  src/目录/long", "  filename.rs:9"], 16);
    for (row, columns) in [(0, 2..15), (1, 2..15)] {
        for column in columns {
            let found = find(&term, row, column).unwrap();
            assert_eq!(found.text, "src/目录/longfilename.rs:9");
            assert_eq!(found.range.start(), crate::Point::new(0, 2));
            assert_eq!(found.range.end(), crate::Point::new(1, 14));
        }
    }
    assert!(find(&term, 1, 0).is_none());
    assert!(find(&term, 1, 1).is_none());
}

#[test]
fn inference_is_bounded_and_requires_a_path() {
    let rows = vec!["  abcdefghijklmno"; 2];
    let term = drawn_rows(&rows, 18);
    assert_eq!(find(&term, 0, 2).unwrap().text, "abcdefghijklmno");

    let rows = vec!["  src/abcdefghijk"; MAX_ROWS + 1];
    let term = drawn_rows(&rows, 18);
    assert_eq!(find(&term, 0, 2).unwrap().text, "src/abcdefghijk");
}
