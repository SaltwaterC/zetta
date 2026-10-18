use super::*;

#[test]
fn mapped_view_matches_ordinary_reflow_for_wide_styled_and_combining_cells() {
    let mut grid = Grid::<Cell>::new(4, 12, 4_000);
    for index in 0..2_000 {
        let mut row = Row::<Cell>::new(12);
        row[Column(0)].c = char::from(b'a' + (index % 26) as u8);
        row[Column(1)].c = '界';
        row[Column(1)].flags = Flags::WIDE_CHAR;
        row[Column(2)].flags = Flags::WIDE_CHAR_SPACER;
        row[Column(3)].c = 'é';
        row[Column(4)].c = 'e';
        row[Column(4)].push_zerowidth('\u{301}');
        row[Column(5)].c = 'x';
        row[Column(5)].bg = Color::Indexed(2);
        if index % 3 != 0 {
            row[Column(11)].flags.insert(Flags::WRAPLINE);
        }
        grid[Line(3)] = row;
        grid.scroll_up(&(Line(0)..Line(4)), 1);
    }
    for columns in [2, 7, 12, 31, 97] {
        for offset in [0, 1, 500, grid.history_size()] {
            let expected = grid.reflow_window(offset, 4, columns);
            let (actual, mapping) = grid.mapped_reflow_window(offset, 4, columns, None);
            assert_eq!(grid.raw.decoded_len(), 0);
            for line in 0..4 {
                assert_eq!(
                    &actual[Line(line)][..],
                    &expected[Line(line - expected.display_offset() as i32)][..],
                    "{columns} columns, offset {offset}, line {line}"
                );
            }
            for (target, source) in mapping {
                let original = grid.raw.owned_row(source.line);
                assert_eq!(actual[target].c, original[source.column].c);
                assert_eq!(actual[target].zerowidth(), original[source.column].zerowidth());
            }
        }
    }
}
