use super::*;
use crate::output::{Pen, PenColor};

fn row(text: &str, attributes: u16) -> Vec<CharInfo> {
    text.encode_utf16()
        .map(|ch| CharInfo { ch, attributes })
        .collect()
}

#[test]
fn write_infos_positions_once_and_changes_colors_only_when_they_change() {
    let mut cells = row("ab", 0x07);
    cells.extend(row("c", 0x1f));
    let mut out = String::new();
    write_infos(&mut out, (4, 2), &cells, 0x07, 80);
    assert_eq!(out, "\x1b[3;5H\x1b[0mab\x1b[0;97;44mc");
}

#[test]
fn wide_cells_are_written_once_and_cut_halves_become_spaces() {
    let cells = [
        CharInfo {
            ch: 0x4e2d,
            attributes: 0x07 | TRAILING_BYTE,
        },
        CharInfo {
            ch: 0x4e2d,
            attributes: 0x07 | LEADING_BYTE,
        },
        CharInfo {
            ch: 0x4e2d,
            attributes: 0x07 | TRAILING_BYTE,
        },
        CharInfo {
            ch: 0x6587,
            attributes: 0x07 | LEADING_BYTE,
        },
    ];
    let mut out = String::new();
    write_infos(&mut out, (0, 0), &cells, 0x07, 4);
    // The leading trail and the trailing lead are cut off by the row's
    // edges. The lead/trail bits count as an attribute change, as in
    // OpenConsole.
    assert_eq!(out, "\x1b[1;1H\x1b[0m \x1b[0m中 ");
    let mut out = String::new();
    write_infos(&mut out, (0, 0), &row("a\x1bb", 0x07), 0x07, 80);
    assert_eq!(out, "\x1b[1;1H\x1b[0ma←b", "controls are shown, never sent");
}

#[test]
fn a_bisected_backup_restores_whole_glyphs_as_row_write_cells_does() {
    let half = |ch, attributes| CharInfo { ch, attributes };
    // A trailing half first, a leading half last, as ReadConsoleOutputW
    // returns a region that cuts two wide glyphs.
    let cells = [
        half(0x3044, 0x07 | TRAILING_BYTE),
        half(u16::from(b'Z'), 0x07),
        half(0x306b, 0x07 | LEADING_BYTE),
    ];
    let mut out = String::new();
    write_infos(&mut out, (2, 0), &cells, 0x07, 16);
    // Starts a column early for the first glyph; the last one has room.
    assert_eq!(out, "\x1b[1;2H\x1b[0mい\x1b[0mZ\x1b[0mに");
    // A trailing half with no leading half before it leaves its column be.
    let cells = [
        half(u16::from(b'a'), 0x07),
        half(0x3044, 0x07 | TRAILING_BYTE),
        half(u16::from(b'b'), 0x07),
    ];
    let mut out = String::new();
    write_infos(&mut out, (0, 0), &cells, 0x07, 16);
    assert_eq!(out, "\x1b[1;1H\x1b[0ma\x1b[Cb");
}

#[test]
fn fills_respect_wide_glyphs_and_keep_lead_trail_markers_on_attribute_writes() {
    let mut cells = vec![CharInfo::blank(0x07); 5];
    let mut position = 0;
    let used = fill_row(&mut cells, Fill::Character(0x4e2d), &mut position, 10);
    // Two wide characters fill four columns; the fifth cannot hold a third.
    assert_eq!(used, 5);
    assert_eq!(position, 2);
    assert_eq!(cells[0].attributes & LEADING_BYTE, LEADING_BYTE);
    assert_eq!(cells[1].attributes & TRAILING_BYTE, TRAILING_BYTE);
    assert_eq!(cells[4].ch, u16::from(b' '));
    let mut position = 0;
    let used = fill_row(&mut cells, Fill::Attribute(0x1f), &mut position, 3);
    assert_eq!((used, position), (3, 3));
    assert_eq!(cells[0].attributes, 0x1f | LEADING_BYTE);
    assert_eq!(cells[3].attributes & 0xff, 0x07);
    let text: Vec<u16> = "xy".encode_utf16().collect();
    let mut position = 0;
    fill_row(
        &mut cells,
        Fill::Characters(&text),
        &mut position,
        text.len(),
    );
    assert_eq!(
        (cells[0].ch, cells[0].attributes & LEADING_BYTE),
        (u16::from(b'x'), 0)
    );
}

#[test]
fn grid_cells_become_char_info_with_wide_markers() {
    let pen = Pen {
        foreground: PenColor::Indexed(9),
        background: PenColor::Default,
        reverse: false,
        underline: false,
    };
    let palette = Palette::default();
    let lead = char_info(
        &Cell {
            ch: '中',
            pen,
            half: Half::Leading,
        },
        &palette,
    );
    assert_eq!(
        lead,
        CharInfo {
            ch: 0x4e2d,
            attributes: 0x0c | LEADING_BYTE
        }
    );
    let astral = char_info(
        &Cell {
            ch: '🙂',
            pen,
            half: Half::Whole,
        },
        &palette,
    );
    assert_eq!(astral.ch, 0xfffd);
}

#[test]
fn rectangles_clamp_and_offset_like_viewports() {
    let buffer = Rect::from_size(80, 24);
    let source = Rect {
        left: -5,
        top: 10,
        right: 100,
        bottom: 30,
    };
    assert_eq!(
        source.clamp(&buffer),
        Rect {
            left: 0,
            top: 10,
            right: 79,
            bottom: 23
        }
    );
    assert!(
        !Rect {
            left: 5,
            top: 0,
            right: 4,
            bottom: 0
        }
        .is_valid()
    );
    assert_eq!(buffer.offset(0, -3).top, -3);
}
