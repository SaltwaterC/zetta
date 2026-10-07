use super::*;

use crate::grid::archive::CompactRows;
use crate::grid::{Dimensions, Row};
use crate::term::cell::Hyperlink;
use crate::vte::ansi::{Color, NamedColor};

fn row(text: &str, columns: usize) -> Row<Cell> {
    let mut row = Row::<Cell>::new(columns);
    for (column, c) in text.chars().enumerate() {
        row[Column(column)].c = c;
    }
    row
}

/// Every character boundary of `text` mapped back to its point.
fn points(text: &RowText) -> Vec<(usize, char, Point)> {
    text.as_str().char_indices().map(|(offset, c)| (offset, c, text.point(offset))).collect()
}

/// What reading the decoded row cell by cell gives, the behaviour the compact reader mirrors.
fn decoded_text(row: &Row<Cell>, line: Line) -> RowText {
    let mut text = RowText::default();
    text.push_row(row, line);
    text
}

/// Assert that every row of `compact` reads as its decoded row does.
fn assert_reads_as_decoded(compact: &CompactRows<Cell>, label: &str) {
    for (index, decoded) in compact.rows().iter().enumerate() {
        let line = Line(-(index as i32));
        let mut text = RowText::default();
        compact.append_text(index, line, &mut text);
        let expected = decoded_text(decoded, line);
        assert_eq!(text.as_str(), expected.as_str(), "{label}: row {index}");
        assert_eq!(points(&text), points(&expected), "{label}: row {index}");
        assert_eq!(
            compact.wraps(index),
            decoded[Column(decoded.len() - 1)].flags.contains(Flags::WRAPLINE),
            "{label}: row {index}"
        );
    }
}

fn varied_rows() -> Vec<Row<Cell>> {
    let mut styled = row("ab界 x  e", 12);
    styled[Column(0)].fg = Color::Indexed(3);
    styled[Column(1)].flags = Flags::BOLD;
    styled[Column(2)].flags = Flags::WIDE_CHAR;
    styled[Column(3)].flags = Flags::WIDE_CHAR_SPACER;
    styled[Column(8)].push_zerowidth('\u{301}');
    styled[Column(9)].set_hyperlink(Some(Hyperlink::new(Some("id"), "https://example.com".into())));

    // A wide character that did not fit leaves a leading spacer in the last column.
    let mut leading = row("abcdefghi", 10);
    leading[Column(9)].flags = Flags::LEADING_WIDE_CHAR_SPACER | Flags::WRAPLINE;

    let mut wrapped = row("wrapped   ", 10);
    wrapped[Column(9)].flags = Flags::WRAPLINE;

    // A zero-width character on a space keeps the trailing run occupied.
    let mut zero_width_space = row("z", 10);
    zero_width_space[Column(4)].push_zerowidth('\u{301}');

    let mut coloured_spaces = Row::<Cell>::new(10);
    for cell in &mut coloured_spaces[..] {
        cell.bg = Color::Named(NamedColor::Red);
    }
    coloured_spaces[Column(2)].c = 'é';

    vec![
        styled,
        leading,
        wrapped,
        zero_width_space,
        coloured_spaces,
        row("", 10),
        row("==========", 10),
        row("──────────", 10),
        row("plain ascii line", 20),
        row("  leading and trailing   ", 30),
    ]
}

#[test]
fn compact_rows_read_as_their_decoded_cells() {
    assert_reads_as_decoded(&CompactRows::encode(&varied_rows()).unwrap(), "encoded");
}

#[test]
fn resized_compact_rows_read_as_their_decoded_cells() {
    let rows = varied_rows().into_iter().map(|mut row| {
        row.grow(30);
        row
    });
    let mut compact = CompactRows::encode(&rows.collect::<Vec<_>>()).unwrap();
    for columns in [40, 9, 3, 1, 12, 30] {
        compact.resize_columns(columns);
        assert_reads_as_decoded(&compact, &format!("{columns} columns"));
    }
}

#[test]
fn points_follow_wide_and_multibyte_characters() {
    let mut wide = row("a界b", 6);
    wide[Column(1)].flags = Flags::WIDE_CHAR;
    wide[Column(2)].flags = Flags::WIDE_CHAR_SPACER;
    // The spacer's own cell is skipped, so 'b' is two columns after the wide character.
    wide[Column(2)].c = ' ';
    wide[Column(3)].c = 'b';
    wide[Column(4)].c = 'é';
    wide[Column(5)].c = 'c';
    let compact = CompactRows::encode(&[wide]).unwrap();

    let mut text = RowText::default();
    compact.append_text(0, Line(-7), &mut text);

    assert_eq!(text.as_str(), "a界béc");
    let columns = points(&text)
        .into_iter()
        .map(|(_, c, point)| (c, point.line.0, point.column.0))
        .collect::<Vec<_>>();
    assert_eq!(columns, [('a', -7, 0), ('界', -7, 1), ('b', -7, 3), ('é', -7, 4), ('c', -7, 5)]);
    assert_eq!(text.last_point(text.len()), Point::new(Line(-7), Column(5)));
    // The last point of a range ending after a multibyte character is that character's cell.
    assert_eq!(text.last_point("a界".len()), Point::new(Line(-7), Column(1)));
}

#[test]
fn appended_rows_keep_their_own_lines() {
    let mut text = RowText::default();
    text.push_row(&row("first", 5), Line(-1));
    text.push_row(&row("second", 6), Line(0));

    assert_eq!(text.as_str(), "firstsecond");
    assert_eq!(text.point(4), Point::new(Line(-1), Column(4)));
    assert_eq!(text.point(5), Point::new(Line(0), Column(0)));
    assert_eq!(text.point(10), Point::new(Line(0), Column(5)));
}

#[test]
fn grid_rows_read_as_text_without_decoding_history() {
    let columns = 12;
    let mut grid = Grid::<Cell>::new(4, columns, 10_000);
    for index in 0..2_000usize {
        let line = grid.bottommost_line();
        let text = format!("line {index:04}");
        for (column, c) in text.chars().enumerate() {
            grid[line][Column(column)].c = c;
        }
        if index % 3 == 0 {
            grid[line][Column(columns - 1)].flags.insert(Flags::WRAPLINE);
        }
        grid.scroll_up(&(Line(0)..Line(4)), 1);
    }
    let lines = grid.topmost_line().0..=grid.bottommost_line().0;
    assert!(grid.history_size() > 1_500, "history must reach the compact archive");

    let mut texts = Vec::new();
    let mut wraps = Vec::new();
    for line in lines.clone() {
        let mut text = RowText::default();
        grid.row_text(Line(line), &mut text);
        texts.push(text);
        wraps.push(grid.row_wraps(Line(line)));
    }
    assert_eq!(grid.raw.decoded_len(), 0, "reading text must not decode history");

    for (line, (text, wraps)) in lines.zip(texts.iter().zip(wraps)) {
        let decoded = &grid[Line(line)];
        let expected = decoded_text(decoded, Line(line));
        assert_eq!(text.as_str(), expected.as_str(), "line {line}");
        assert_eq!(points(text), points(&expected), "line {line}");
        assert_eq!(wraps, decoded[Column(columns - 1)].flags.contains(Flags::WRAPLINE));
    }
}
