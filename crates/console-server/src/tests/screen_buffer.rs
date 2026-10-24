use super::*;

fn buffer(width: i16, height: i16) -> ScreenBuffer {
    let saved = Saved {
        output_mode: 0x7,
        cursor_size: 25,
        attributes: 0x1f,
        cursor_visible: true,
        cursor: (0, 0),
    };
    let blank = CharInfo::blank(0x07);
    let budget = Budget::session();
    let mut screen =
        ScreenBuffer::new(&budget, (width, height), (width, height), blank, saved).unwrap();
    screen.handles = 1;
    screen
}
fn text(screen: &ScreenBuffer) -> String {
    (0..i32::from(screen.size.1))
        .map(|y| {
            let row: Vec<u16> = (0..i32::from(screen.size.0))
                .map(|x| screen.cell(x, y).unwrap().ch)
                .collect();
            String::from_utf16_lossy(&row)
        })
        .collect::<Vec<_>>()
        .join("|")
}
/// Rows that hold cells of their own.
fn written(screen: &ScreenBuffer) -> usize {
    screen
        .rows
        .iter()
        .filter(|row| matches!(row, Row::Cells(_)))
        .count()
}
/// What a buffer of `height` rows is charged with `written` of them
/// holding `width` cells.
fn charged(width: usize, height: usize, written: usize) -> usize {
    height * size_of::<Row>() + written * width * size_of::<CharInfo>()
}
fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    Rect {
        left,
        top,
        right,
        bottom,
    }
}

#[test]
fn off_screen_text_wraps_scrolls_and_skips_escape_sequences() {
    let mut screen = buffer(4, 2);
    screen.write_text("ab\x1b[31mcdef\r\nxy").unwrap();
    // "abcd" fills the first row, "ef" wraps, then CRLF scrolls it up.
    assert_eq!(text(&screen), "ef  |xy  ");
    assert_eq!(screen.saved.cursor, (2, 1));
    assert_eq!(screen.cell(0, 0).unwrap().attributes, 0x1f);
}

#[test]
fn off_screen_control_characters_move_the_cursor() {
    let mut screen = buffer(10, 1);
    screen.write_text("abc\x08X\tY\rZ").unwrap();
    assert_eq!(text(&screen), "ZbX     Y ");
}

#[test]
fn legacy_off_screen_text_wraps_at_once_or_overwrites_the_last_column() {
    let mut screen = buffer(3, 2);
    screen.saved.output_mode = mode::WRAP_AT_EOL_OUTPUT;
    screen.write_text("abc").unwrap();
    assert_eq!(screen.saved.cursor, (0, 1));
    let mut screen = buffer(3, 2);
    screen.saved.output_mode = 0;
    screen.write_text("abcd").unwrap();
    assert_eq!(text(&screen), "abd|   ");
    assert_eq!(screen.saved.cursor, (2, 0));
}

#[test]
fn resizing_keeps_what_fits_where_it_was_and_clamps_the_cursor() {
    let mut screen = buffer(3, 2);
    screen.write_text("abcdef").unwrap();
    assert_eq!(text(&screen), "abc|def");
    screen.resize((2, 3)).unwrap();
    assert_eq!(text(&screen), "ab|de|  ");
    assert_eq!(screen.saved.cursor, (1, 1));
    assert!(screen.dirty);
    screen.window = (2, 3);
    screen.resize((4, 1)).unwrap();
    assert_eq!(text(&screen), "ab  ");
    assert_eq!(screen.window, (2, 1));
}

#[test]
fn resizing_a_partly_written_buffer_keeps_the_blanks_it_had() {
    let mut screen = buffer(2, 2);
    screen.write_text("ab").unwrap();
    // The new blank is 0x1f, so the unwritten row keeps reading as 0x07.
    screen.resize((3, 3)).unwrap();
    assert_eq!(screen.cell(0, 1).unwrap().attributes, 0x07);
    assert_eq!(screen.cell(2, 0).unwrap().attributes, 0x1f);
    assert_eq!(screen.cell(0, 2).unwrap().attributes, 0x1f);
    assert_eq!(written(&screen), 2);
    // Rows that do not widen keep what they had; the new ones are blank.
    screen.resize((3, 30000)).unwrap();
    assert_eq!(written(&screen), 2);
    assert_eq!(screen.charge.bytes(), charged(3, 30000, 2));
}

#[test]
fn a_buffer_written_on_its_last_row_pays_for_that_row_alone() {
    let mut screen = buffer(32766, 32766);
    screen.saved.cursor = (32764, 32765);
    screen.write_text("ab").unwrap();
    assert_eq!(screen.cell(32765, 32765).unwrap().ch, u16::from(b'b'));
    assert_eq!(written(&screen), 1);
    assert_eq!(screen.charge.bytes(), charged(32766, 32766, 1));
    assert_eq!(screen.charge.budget().used(), screen.charge.bytes());
}

#[test]
fn a_newline_at_the_bottom_rotates_the_rows_and_refunds_the_one_that_left() {
    let mut screen = buffer(3, 3);
    screen.saved.attributes = 0x07;
    screen.write_text("abc\r\ndef\r\nghi").unwrap();
    assert_eq!(written(&screen), 3);
    screen.write_text("\r\n").unwrap();
    assert_eq!(text(&screen), "def|ghi|   ");
    assert_eq!(written(&screen), 2);
    assert_eq!(screen.charge.bytes(), charged(3, 3, 2));
    // The row that came in is the pen's blank, held as one cell.
    screen.saved.attributes = 0x1f;
    screen.write_text("\r\n").unwrap();
    assert_eq!(text(&screen), "ghi|   |   ");
    assert_eq!(screen.cell(2, 2).unwrap().attributes, 0x1f);
    assert_eq!(written(&screen), 1);
}

#[test]
fn rows_written_or_filled_all_the_same_are_held_as_one_cell() {
    let mut screen = buffer(4, 3);
    let x = CharInfo {
        ch: u16::from(b'x'),
        attributes: 0x07,
    };
    // Part of a row, the same as the rest of it.
    screen.write_row(1, 0, &[CharInfo::blank(0x07); 2]).unwrap();
    assert_eq!(written(&screen), 0);
    screen.write_row(1, 0, &[x; 2]).unwrap();
    assert_eq!(text(&screen), " xx |    |    ");
    assert_eq!(written(&screen), 1);
    // Writing the whole row with one cell gives its cells back.
    screen.write_row(0, 0, &[x; 9]).unwrap();
    assert_eq!(text(&screen), "xxxx|    |    ");
    assert_eq!(written(&screen), 0);
    screen.write_text("ab\r\ncd").unwrap();
    assert_eq!(written(&screen), 2);
    // So does filling the whole buffer.
    let all = rect(0, 0, 3, 2);
    screen
        .scroll(all, rect(0, 0, -1, -1), rect(0, 0, -1, -1), x)
        .unwrap();
    assert_eq!(text(&screen), "xxxx|xxxx|xxxx");
    assert_eq!(written(&screen), 0);
    assert_eq!(screen.charge.bytes(), charged(4, 3, 0));
}

#[test]
fn scrolling_off_screen_moves_each_row_before_it_is_written_over() {
    let fill = CharInfo {
        ch: u16::from(b'.'),
        attributes: 0x07,
    };
    let scrolled = |source: Rect, target: Rect, fill_rect: Rect| {
        let mut screen = buffer(4, 4);
        screen.write_text("abcdefghijklmnop").unwrap();
        assert_eq!(text(&screen), "abcd|efgh|ijkl|mnop");
        screen.scroll(fill_rect, source, target, fill).unwrap();
        assert!(screen.dirty);
        text(&screen)
    };
    let all = rect(0, 0, 3, 3);
    assert_eq!(
        scrolled(rect(0, 0, 3, 2), rect(0, 1, 3, 3), all),
        "....|abcd|efgh|ijkl"
    );
    assert_eq!(
        scrolled(rect(0, 1, 3, 3), rect(0, 0, 3, 2), all),
        "efgh|ijkl|mnop|...."
    );
    assert_eq!(
        scrolled(rect(0, 1, 2, 2), rect(1, 1, 3, 2), rect(0, 1, 3, 2)),
        "abcd|.efg|.ijk|mnop"
    );
    assert_eq!(
        scrolled(rect(1, 0, 3, 1), rect(0, 1, 2, 2), rect(1, 0, 3, 1)),
        "a...|bcd.|fghl|mnop"
    );
}
