use super::*;

fn buffer(width: i16, height: i16) -> ScreenBuffer {
    ScreenBuffer {
        size: (width, height),
        cells: vec![CharInfo::blank(0x07); (width * height) as usize],
        saved: Saved {
            output_mode: 0x7,
            cursor_size: 25,
            attributes: 0x1f,
            cursor_visible: true,
            cursor: (0, 0),
        },
        handles: 1,
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
