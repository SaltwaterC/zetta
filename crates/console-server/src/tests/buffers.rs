use super::*;

fn buffer(width: i16, height: i16) -> ScreenBuffer {
    ScreenBuffer {
        size: (width, height),
        window: (width, height),
        cells: vec![CharInfo::blank(0x07); (width * height) as usize],
        saved: Saved {
            output_mode: 0x7,
            cursor_size: 25,
            attributes: 0x1f,
            cursor_visible: true,
            cursor: (0, 0),
        },
        handles: 1,
        dirty: false,
    }
}
fn text(screen: &ScreenBuffer) -> String {
    screen
        .cells
        .chunks(screen.size.0 as usize)
        .map(|row| String::from_utf16_lossy(&row.iter().map(|cell| cell.ch).collect::<Vec<_>>()))
        .collect::<Vec<_>>()
        .join("|")
}

#[test]
fn off_screen_text_wraps_scrolls_and_skips_escape_sequences() {
    let mut screen = buffer(4, 2);
    screen.write_text("ab\x1b[31mcdef\r\nxy");
    // "abcd" fills the first row, "ef" wraps, then CRLF scrolls it up.
    assert_eq!(text(&screen), "ef  |xy  ");
    assert_eq!(screen.saved.cursor, (2, 1));
    assert_eq!(screen.cells[0].attributes, 0x1f);
}

#[test]
fn off_screen_control_characters_move_the_cursor() {
    let mut screen = buffer(10, 1);
    screen.write_text("abc\x08X\tY\rZ");
    assert_eq!(text(&screen), "ZbX     Y ");
}

#[test]
fn legacy_off_screen_text_wraps_at_once_or_overwrites_the_last_column() {
    let mut screen = buffer(3, 2);
    screen.saved.output_mode = mode::WRAP_AT_EOL_OUTPUT;
    screen.write_text("abc");
    assert_eq!(screen.saved.cursor, (0, 1));
    let mut screen = buffer(3, 2);
    screen.saved.output_mode = 0;
    screen.write_text("abcd");
    assert_eq!(text(&screen), "abd|   ");
    assert_eq!(screen.saved.cursor, (2, 0));
}

#[test]
fn reading_characters_returns_wide_glyphs_once_or_a_space_when_cut() {
    let lead = CharInfo {
        ch: 0x4e2d,
        attributes: 0x07 | legacy::LEADING_BYTE,
    };
    let trail = CharInfo {
        ch: 0x4e2d,
        attributes: 0x07 | legacy::TRAILING_BYTE,
    };
    let a = CharInfo {
        ch: u16::from(b'a'),
        attributes: 0x07,
    };
    assert_eq!(characters(&[a, lead, trail, a]), [97, 0x4e2d, 97]);
    assert_eq!(characters(&[a, lead]), [97, 0x20]);
    assert_eq!(characters(&[trail, a]), [0x20, 97]);
}

#[test]
fn resizing_keeps_what_fits_where_it_was_and_clamps_the_cursor() {
    let mut screen = buffer(3, 2);
    screen.write_text("abcdef");
    assert_eq!(text(&screen), "abc|def");
    screen.resize((2, 3));
    assert_eq!(text(&screen), "ab|de|  ");
    assert_eq!(screen.saved.cursor, (1, 1));
    assert!(screen.dirty);
    screen.window = (2, 3);
    screen.resize((4, 1));
    assert_eq!(text(&screen), "ab  ");
    assert_eq!(screen.window, (2, 1));
}

fn server() -> Server {
    Server::new(crate::sink::Sink::new(false, None, 500).unwrap(), false)
}
/// A key-down INPUT_RECORD whose AsciiChar is `byte`.
fn key(byte: u8) -> [u8; 20] {
    let mut record = [0; 20];
    record[0] = 1;
    record[4] = 1;
    record[8] = 1;
    record[14] = byte;
    record
}
fn chars(records: &[[u8; 20]]) -> Vec<u16> {
    records
        .iter()
        .map(|record| u16::from_le_bytes([record[14], record[15]]))
        .collect()
}

#[test]
fn filling_with_a_byte_the_code_page_cannot_convert_fails_as_e_unexpected() {
    let mut server = server();
    let object = server.object(Object::Output(0), Access::SHARED);
    // CONSOLE_FILLCONSOLEOUTPUT_MSG for FillConsoleOutputCharacterA, one cell.
    server.state.output_cp = 50220;
    let mut fill = |byte: u8| {
        server.reply.clear();
        server.reply.resize(16, 0);
        set_u32(&mut server.reply, 4, 1);
        set_u16(&mut server.reply, 8, u16::from(byte));
        set_u32(&mut server.reply, 12, 1);
        let descriptor = Descriptor {
            object,
            ..Default::default()
        };
        server.fill_output(descriptor).unwrap().0
    };
    // GH#4258: in ISO-2022-JP, 14 is a shift with no character of its own.
    assert_eq!(fill(14), E_UNEXPECTED);
    // A byte that is a character gets past the conversion to the fill itself.
    assert_ne!(fill(b'a'), E_UNEXPECTED);
}

#[test]
fn a_lead_byte_ending_one_ansi_input_write_joins_the_next_writes_first_key() {
    let mut server = server();
    // Shift-JIS "あ" is 0x82 0xa0.
    server.state.input_cp = 932;
    assert_eq!(
        chars(&server.widen_input_records(&[key(b'a'), key(0x82)])),
        [97]
    );
    assert_eq!(
        chars(&server.widen_input_records(&[key(0xa0), key(b'b')])),
        [0x3042, 98]
    );
    // A lead byte whose trail is not a key is dropped with it.
    let mut focus = [0; 20];
    focus[0] = 0x10;
    assert_eq!(
        chars(&server.widen_input_records(&[key(0x82), focus, key(b'c')])),
        [99]
    );
}

#[test]
fn ansi_cells_take_lead_and_trail_from_the_code_page_one_row_at_a_time() {
    let mut server = server();
    server.state.output_cp = 932;
    let cell = |byte: u8, attributes: u16| CharInfo {
        ch: u16::from(byte),
        attributes,
    };
    // Two rows of three. The first row's lead byte has no trail in its row;
    // the 'c' claims to be a trail byte, which the caller cannot decide.
    let mut cells = [
        cell(b'a', 0x07),
        cell(b'b', 0x07),
        cell(0x82, 0x07),
        cell(0x82, 0x07),
        cell(0xa0, 0x07),
        cell(b'c', 0x07 | legacy::TRAILING_BYTE),
    ];
    server.cells_from_ansi(&mut cells, 3);
    let lead = 0x07 | legacy::LEADING_BYTE;
    let trail = 0x07 | legacy::TRAILING_BYTE;
    assert_eq!(
        cells.map(|cell| (cell.ch, cell.attributes)),
        [
            (97, 0x07),
            (98, 0x07),
            (0x20, 0x07),
            (0x3042, lead),
            (0x3042, trail),
            (99, 0x07),
        ]
    );
}
