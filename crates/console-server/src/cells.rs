// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Cell-level console output: CHAR_INFO conversion, the row fills behind
//! FillConsoleOutput and WriteConsoleOutputCharacter/Attribute, and
//! VtIo::Writer::WriteInfos, which turns a row of cells into VT. Pure; the
//! server decides where cells come from and where the VT goes.
use crate::legacy::{self, LEADING_BYTE, TRAILING_BYTE};
use crate::output::{Cell, Half};
use crate::state::Palette;
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CharInfo {
    pub ch: u16,
    pub attributes: u16,
}
impl CharInfo {
    pub fn blank(attributes: u16) -> Self {
        Self {
            ch: u16::from(b' '),
            attributes,
        }
    }
    pub fn to_bytes(self) -> [u8; 4] {
        let [a, b] = self.ch.to_le_bytes();
        let [c, d] = self.attributes.to_le_bytes();
        [a, b, c, d]
    }
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            ch: u16::from_le_bytes([bytes[0], bytes[1]]),
            attributes: u16::from_le_bytes([bytes[2], bytes[3]]),
        }
    }
}

/// A grid cell as ReadConsoleOutputW reports it. Characters outside the
/// BMP cannot be one UTF-16 unit and read as U+FFFD.
pub fn char_info(cell: &Cell, palette: &Palette) -> CharInfo {
    let mut attributes = legacy::attributes(&cell.pen, palette);
    match cell.half {
        Half::Leading => attributes |= LEADING_BYTE,
        Half::Trailing => attributes |= TRAILING_BYTE,
        Half::Whole => {}
    }
    let ch = u16::try_from(u32::from(cell.ch)).unwrap_or(0xfffd);
    CharInfo { ch, attributes }
}

/// IsGlyphFullWidth, as the embedder's grid measures it.
pub fn is_wide(unit: u16) -> bool {
    unit >= 0x80 && char::from_u32(u32::from(unit)).is_some_and(|ch| ch.width() == Some(2))
}

/// What FillConsoleImpl writes into a row.
#[derive(Clone, Copy)]
pub enum Fill<'a> {
    Attributes(&'a [u16]),
    Attribute(u16),
    Characters(&'a [u16]),
    Character(u16),
}
impl Fill<'_> {
    pub fn len(&self, length: usize) -> usize {
        match self {
            Self::Attributes(input) | Self::Characters(input) => input.len(),
            Self::Attribute(_) | Self::Character(_) => length,
        }
    }
}

/// One row of FillConsoleImpl: writes from `*position` of the input into
/// `row`, returning the columns used. Attribute writes keep the lead/trail
/// markers; a wide character that would not fit the last column becomes a
/// space and ends the row.
pub fn fill_row(
    row: &mut [CharInfo],
    fill: Fill<'_>,
    position: &mut usize,
    length: usize,
) -> usize {
    const LT: u16 = LEADING_BYTE | TRAILING_BYTE;
    let available = row.len();
    let mut columns = 0;
    let mut put = |columns: &mut usize, ch: u16, wide_half: u16| {
        let cell = &mut row[*columns];
        cell.ch = ch;
        cell.attributes = (cell.attributes & !LT) | wide_half;
        *columns += 1;
    };
    match fill {
        Fill::Attributes(input) => {
            while columns < available && *position < length {
                let cell = &mut row[columns];
                cell.attributes = (input[*position] & !LT) | (cell.attributes & LT);
                columns += 1;
                *position += 1;
            }
        }
        Fill::Attribute(attributes) => {
            while columns < available && *position < length {
                let cell = &mut row[columns];
                cell.attributes = (attributes & !LT) | (cell.attributes & LT);
                columns += 1;
                *position += 1;
            }
        }
        Fill::Characters(input) => {
            while columns < available && *position < length {
                let ch = input[*position];
                if is_wide(ch) {
                    if columns + 1 >= available {
                        put(&mut columns, u16::from(b' '), 0);
                        break;
                    }
                    put(&mut columns, ch, LEADING_BYTE);
                    put(&mut columns, ch, TRAILING_BYTE);
                } else {
                    put(&mut columns, ch, 0);
                }
                *position += 1;
            }
        }
        Fill::Character(ch) => {
            while columns < available && *position < length {
                if is_wide(ch) {
                    if columns + 1 >= available {
                        put(&mut columns, u16::from(b' '), 0);
                        break;
                    }
                    put(&mut columns, ch, LEADING_BYTE);
                    put(&mut columns, ch, TRAILING_BYTE);
                } else {
                    put(&mut columns, ch, 0);
                }
                *position += 1;
            }
        }
    }
    columns
}

/// VtIo::Writer::WriteInfos: positions at `target`, then each cell with an
/// SGR whenever its attributes change. Trailing halves are covered by their
/// leading half; a half cut off by the row's edge becomes a space.
pub fn write_infos(out: &mut String, target: (i16, i16), cells: &[CharInfo], defaults: u16) {
    out.push_str(&format!("\x1b[{};{}H", target.1 + 1, target.0 + 1));
    let mut current = None;
    for (index, cell) in cells.iter().enumerate() {
        let mut ch = cell.ch;
        let mut wide = cell.attributes & (LEADING_BYTE | TRAILING_BYTE) != 0;
        if wide {
            if cell.attributes & LEADING_BYTE != 0 {
                if index == cells.len() - 1 {
                    ch = u16::from(b' ');
                    wide = false;
                }
            } else if index == 0 {
                ch = u16::from(b' ');
                wide = false;
            } else {
                continue;
            }
        }
        if current != Some(cell.attributes) {
            current = Some(cell.attributes);
            out.push_str(&legacy::sgr(cell.attributes, defaults));
        }
        let shown = legacy::sanitize(ch);
        out.push(shown);
        // A control or replacement standing in for a wide glyph is narrow.
        if wide && (legacy::is_control(ch) || (0xd800..=0xdfff).contains(&ch)) {
            out.push(shown);
        }
    }
}

/// An inclusive rectangle, as SMALL_RECT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Rect {
    pub fn from_size(width: i32, height: i32) -> Self {
        Self {
            left: 0,
            top: 0,
            right: width - 1,
            bottom: height - 1,
        }
    }
    pub fn is_valid(&self) -> bool {
        self.left <= self.right && self.top <= self.bottom
    }
    pub fn width(&self) -> i32 {
        self.right - self.left + 1
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top + 1
    }
    /// Viewport::Clamp: the intersection.
    pub fn clamp(&self, bounds: &Rect) -> Rect {
        Rect {
            left: self.left.max(bounds.left),
            top: self.top.max(bounds.top),
            right: self.right.min(bounds.right),
            bottom: self.bottom.min(bounds.bottom),
        }
    }
    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect {
            left: self.left + dx,
            top: self.top + dy,
            right: self.right + dx,
            bottom: self.bottom + dy,
        }
    }
}

#[cfg(test)]
#[path = "tests/cells.rs"]
mod tests;
