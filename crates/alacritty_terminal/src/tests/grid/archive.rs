use super::*;

use crate::index::Column;
use crate::term::cell::{Cell, Flags, Hyperlink};
use crate::vte::ansi::{Color, NamedColor, Rgb};

fn row(text: &str, columns: usize) -> Row<Cell> {
    let mut row = Row::<Cell>::new(columns);
    for (column, c) in text.chars().enumerate() {
        row[Column(column)].c = c;
    }
    row
}

fn round_trip(rows: &[Row<Cell>]) -> Vec<Row<Cell>> {
    CompactRows::encode(rows).expect("cells are archivable").rows()
}

#[test]
fn plain_rows_round_trip_and_are_far_smaller() {
    let rows = (0..256)
        .map(|index| row(&format!("line {index:016x} some text after it"), 94))
        .collect::<Vec<_>>();

    let compact = CompactRows::encode(&rows).unwrap();

    assert_eq!(compact.rows(), rows);
    assert_eq!(compact.len(), 256);
    let row_bytes = 94 * std::mem::size_of::<Cell>();
    assert!(compact.bytes.len() * 20 < rows.len() * row_bytes, "{} bytes", compact.bytes.len());
}

#[test]
fn attributes_wide_and_zero_width_characters_round_trip() {
    let mut styled = row("ab界 x", 10);
    styled[Column(0)].fg = Color::Spec(Rgb { r: 1, g: 2, b: 3 });
    styled[Column(1)].bg = Color::Indexed(200);
    styled[Column(1)].flags = Flags::BOLD | Flags::UNDERLINE;
    styled[Column(2)].flags = Flags::WIDE_CHAR;
    styled[Column(3)].flags = Flags::WIDE_CHAR_SPACER;
    styled[Column(5)].push_zerowidth('\u{301}');
    styled[Column(6)].set_hyperlink(Some(Hyperlink::new(Some("id"), "https://example.com".into())));
    styled[Column(9)].flags = Flags::WRAPLINE;

    let mut background = Row::<Cell>::new(10);
    for cell in &mut background[..] {
        cell.bg = Color::Named(NamedColor::Red);
    }
    background[Column(0)].c = 'é';

    let rows = vec![styled, background, row("", 10)];
    assert_eq!(round_trip(&rows), rows);
}

#[test]
fn shared_extra_storage_stays_shared() {
    let mut rows = vec![row("abc", 4)];
    rows[0][Column(0)].set_underline_color(Some(Color::Indexed(1)));
    let extra = rows[0][Column(0)].extra.clone();
    rows[0][Column(1)].extra = extra.clone();

    let decoded = round_trip(&rows);

    assert_eq!(decoded, rows);
    let decoded_extra = decoded[0][Column(1)].extra.as_ref().unwrap();
    assert!(Arc::ptr_eq(decoded_extra, extra.as_ref().unwrap()));
}

#[test]
fn too_many_distinct_attributes_are_left_as_rows() {
    let mut colourful = Row::<Cell>::new(MAX_ATTRIBUTES + 1);
    for (index, cell) in colourful[..].iter_mut().enumerate() {
        cell.fg = Color::Indexed(index as u8);
    }
    assert!(CompactRows::encode(&[colourful]).is_none());

    let mut enough = Row::<Cell>::new(MAX_ATTRIBUTES);
    for (index, cell) in enough[..].iter_mut().enumerate() {
        cell.fg = Color::Indexed(index as u8);
    }
    assert!(CompactRows::encode(&[enough]).is_some());
}

#[test]
fn rows_resize_as_growing_and_shrinking_them_would() {
    let mut rows = vec![row("abcdef", 8), row("x", 8)];
    rows[0][Column(1)].flags = Flags::BOLD;
    let mut compact = CompactRows::encode(&rows).unwrap();

    for columns in [12, 3, 10] {
        compact.resize_columns(columns);
        for row in &mut rows {
            if row.len() < columns {
                row.grow(columns);
            } else {
                row.shrink(columns);
            }
        }
        assert_eq!(compact.rows(), rows, "{columns} columns");
    }
}

#[test]
fn dropping_the_oldest_rows_keeps_the_rest_and_their_ids() {
    let rows = (0..4).map(|index| row(&index.to_string(), 2)).collect::<Vec<_>>();
    let mut compact = CompactRows::encode(&rows).unwrap();
    let ids = (0..4).map(|index| compact.row_id(index)).collect::<Vec<_>>();

    compact.truncate_oldest(2);

    assert_eq!(compact.rows(), rows[..2]);
    assert_eq!(compact.row_id(1), ids[1]);
    assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 4);
}
