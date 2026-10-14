// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Legacy console semantics expressed as VT for the embedder's parser:
//! attribute words as SGR, and text written with VT processing disabled.
//! These follow OpenConsole's VtIo writer, which does the same translation
//! for ConPTY, so the terminal sees what it would have seen from ConPTY.
use crate::output::{Pen, PenColor};
use crate::state::Palette;
use unicode_width::UnicodeWidthChar;

pub const FOREGROUND_MASK: u16 = 0x000f;
pub const BACKGROUND_MASK: u16 = 0x00f0;
pub const LEADING_BYTE: u16 = 0x0100;
pub const TRAILING_BYTE: u16 = 0x0200;
pub const REVERSE_VIDEO: u16 = 0x4000;
pub const UNDERSCORE: u16 = 0x8000;
/// Every attribute bit SetConsoleTextAttribute accepts; 0x2000 is reserved.
pub const VALID_ATTRIBUTES: u16 = !0x2000;

/// Legacy attribute colors are BGR-ordered; ANSI indices are RGB-ordered.
pub fn ansi_index(legacy: u16) -> u8 {
    let legacy = legacy & 0xf;
    ((legacy & 0x8) | ((legacy & 0x1) << 2) | (legacy & 0x2) | ((legacy & 0x4) >> 2)) as u8
}

/// formatAttributes: SGR 0 first, because SetConsoleTextAttribute replaces
/// every attribute, including VT-only ones. Colors equal to the console's
/// default attributes stay the terminal's default colors, as
/// TextAttribute::SetLegacyDefaultAttributes arranges.
pub fn sgr(attributes: u16, defaults: u16) -> String {
    const CODES: [u8; 16] = [
        30, 31, 32, 33, 34, 35, 36, 37, 90, 91, 92, 93, 94, 95, 96, 97,
    ];
    let mut sgr = String::from("\x1b[0");
    if attributes & REVERSE_VIDEO != 0 {
        sgr.push_str(";7");
    }
    let foreground = attributes & FOREGROUND_MASK;
    if foreground != defaults & FOREGROUND_MASK {
        sgr.push_str(&format!(";{}", CODES[usize::from(ansi_index(foreground))]));
    }
    let background = (attributes & BACKGROUND_MASK) >> 4;
    if background != (defaults & BACKGROUND_MASK) >> 4 {
        sgr.push_str(&format!(
            ";{}",
            CODES[usize::from(ansi_index(background))] + 10
        ));
    }
    sgr.push('m');
    sgr
}

/// IsControlCharacter: C0, DEL and C1.
pub fn is_control(unit: u16) -> bool {
    unit <= 0x1f || (0x7f..=0x9f).contains(&unit)
}

/// SanitizeUCS2: controls become the DOS 437 glyphs a raster-font console
/// showed, C1 becomes '?', and an unpaired surrogate becomes U+FFFD.
pub fn sanitize(unit: u16) -> char {
    const C0: [char; 32] = [
        ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼', '►', '◄',
        '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
    ];
    match unit {
        0..=0x1f => C0[usize::from(unit)],
        0x7f => '⌂',
        0x80..=0x9f => '?',
        _ => char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}'),
    }
}

/// The console's own cursor column, for the two things a legacy write does
/// that a terminal cannot work out alone: how many spaces a TAB writes, and
/// wrapping as soon as a character lands in the last column (a terminal waits
/// for the next one). `width` 0 means the column is unknown, and neither
/// happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    /// Where the next character goes; `width` once the last column is full.
    pub at: usize,
    pub width: usize,
    /// ENABLE_WRAP_AT_EOL_OUTPUT.
    pub wrap: bool,
}

impl Column {
    pub const UNKNOWN: Self = Self {
        at: 0,
        width: 0,
        wrap: false,
    };
    fn known(&self) -> bool {
        self.width > 0
    }
    fn advance(&mut self, out: &mut String, ch: char) {
        out.push(ch);
        let cells = ch.width().unwrap_or(0);
        if self.wrap && self.at + cells > self.width {
            self.at = 0;
        }
        self.at = (self.at + cells).min(self.width);
    }
    /// _writeCharsLegacyUnprocessed wraps a run of text that ends in the last
    /// column at once, which ConPTY's writer spells as CRLF.
    fn end_run(&mut self, out: &mut String) {
        if self.known() && self.wrap && self.at >= self.width {
            out.push_str("\r\n");
            self.at = 0;
        }
    }
}

/// WriteUTF16StripControlChars, for ENABLE_PROCESSED_OUTPUT clear: every
/// control is shown as its glyph so the terminal never acts on it.
pub fn unprocessed(text: &[u16], column: &mut Column) -> String {
    let mut out = String::with_capacity(text.len());
    for unit in char::decode_utf16(text.iter().copied()) {
        let ch = match unit {
            Ok(ch) if ch.is_ascii() && !is_control(ch as u16) => ch,
            Ok(ch) if (ch as u32) <= 0xffff => sanitize(ch as u16),
            Ok(ch) => ch,
            Err(_) => '\u{fffd}',
        };
        column.advance(&mut out, ch);
    }
    column.end_run(&mut out);
    out
}

/// WriteCharsLegacy with ENABLE_PROCESSED_OUTPUT set and VT processing off.
/// BEL, BS and CR reach the terminal unchanged, NUL is a space, LF gains a
/// CR unless DISABLE_NEWLINE_AUTO_RETURN is set, and any other control is
/// the output code page's glyph for it (`glyph` returns None when the code
/// page has none, as UTF-8 does, and the character is dropped).
///
/// A TAB writes spaces to the next multiple of 8, never past the right edge,
/// so it overwrites what it passes as the console's buffer does; with the
/// column unknown it is the terminal's own tab.
pub fn processed(
    text: &[u16],
    auto_return: bool,
    column: &mut Column,
    glyph: impl Fn(u8) -> Option<char>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous = None;
    for unit in char::decode_utf16(text.iter().copied()) {
        let ch = unit.unwrap_or('\u{fffd}');
        let control = (ch as u32) < 0x20 || ch == '\x7f';
        if control {
            // conhost writes the text between controls as one run.
            column.end_run(&mut out);
        }
        match ch {
            '\0' => column.advance(&mut out, ' '),
            '\x07' => out.push(ch),
            '\x08' => {
                out.push(ch);
                column.at = column.at.saturating_sub(1);
            }
            '\t' if column.known() => {
                let at = column.at.min(column.width - 1);
                let count = (column.width - at).min(8 - (at & 7));
                out.extend(std::iter::repeat_n(' ', count));
                column.at = at + count;
            }
            '\t' => out.push(ch),
            '\r' => {
                out.push(ch);
                column.at = 0;
            }
            '\n' if auto_return && previous != Some('\r') => {
                out.push_str("\r\n");
                column.at = 0;
            }
            '\n' => {
                out.push('\n');
                if auto_return {
                    column.at = 0;
                }
            }
            c if control => {
                if let Some(glyph) = glyph(c as u8) {
                    column.advance(&mut out, glyph);
                }
            }
            c => column.advance(&mut out, c),
        }
        if control {
            column.end_run(&mut out);
        }
        previous = Some(ch);
    }
    column.end_run(&mut out);
    out
}

/// The attribute word GetConsoleScreenBufferInfo reports for a pen, as
/// TextAttribute::GetLegacyAttributes: default colors are the console's
/// default attributes, and other colors go through conhost's fixed tables,
/// whatever the palette holds.
pub fn attributes(pen: &Pen, palette: &Palette) -> u16 {
    let foreground = nibble(pen.foreground, palette.attributes & FOREGROUND_MASK);
    let background = nibble(pen.background, (palette.attributes & BACKGROUND_MASK) >> 4);
    let mut value = foreground | (background << 4);
    if pen.reverse {
        value |= REVERSE_VIDEO;
    }
    if pen.underline {
        value |= UNDERSCORE;
    }
    value
}

/// TextColor::GetLegacyIndex.
fn nibble(color: PenColor, default: u16) -> u16 {
    let index = match color {
        PenColor::Default => return default,
        PenColor::Indexed(index) => INDEX256_TO_INDEX16[usize::from(index)],
        PenColor::Rgb(r, g, b) => {
            let compressed = (r & 0b1110_0000) + ((g >> 3) & 0b0001_1100) + ((b >> 6) & 0b11);
            COMPRESSED_RGB_TO_INDEX16[usize::from(compressed)]
        }
    };
    u16::from(index)
}

/// 8-bit RGB colors, in the form RRRGGGBB, to one of the 16 legacy colors
/// (TextColor.cpp's CompressedRgbToIndex16).
#[rustfmt::skip]
const COMPRESSED_RGB_TO_INDEX16: [u8; 256] = [
     0,  1,  1,  9,  0,  0,  1,  1,  2,  1,  1,  1,  2,  8,  1,  9,
     2,  2,  3,  3,  2,  2, 11,  3, 10, 10, 11, 11, 10, 10, 10, 11,
     0,  5,  1,  1,  0,  0,  1,  1,  8,  1,  1,  1,  2,  8,  1,  9,
     2,  2,  3,  3,  2,  2, 11,  3, 10, 10, 10, 11, 10, 10, 10, 11,
     5,  5,  5,  1,  4,  5,  1,  1,  8,  8,  1,  9,  2,  8,  9,  9,
     2,  2,  3,  3,  2,  2, 11,  3, 10, 10, 11, 11, 10, 10, 10, 11,
     4,  5,  5,  1,  4,  5,  5,  1,  8,  5,  5,  1,  8,  8,  9,  9,
     2,  2,  8,  9, 10,  2, 11,  3, 10, 10, 11, 11, 10, 10, 10, 11,
     4, 13,  5,  5,  4, 13,  5,  5,  4, 13, 13, 13,  6,  8, 13,  9,
     6,  8,  8,  9, 10, 10, 11,  3, 10, 10, 11, 11, 10, 10, 10, 11,
     4, 13, 13, 13,  4, 13, 13, 13,  4, 12, 13, 13,  6, 12, 13, 13,
     6,  6,  8,  9,  6,  6,  7,  7, 10, 14, 14,  7, 10, 10, 14, 11,
     4, 12, 13, 13,  4, 12, 13, 13,  4, 12, 13, 13,  6, 12, 12, 13,
     6,  6, 12,  7,  6,  6,  7,  7,  6, 14, 14,  7, 14, 14, 14, 15,
    12, 12, 13, 13, 12, 12, 13, 13, 12, 12, 12, 13, 12, 12, 12, 13,
     6, 12, 12,  7,  6,  6,  7,  7,  6, 14, 14,  7, 14, 14, 14, 15,
];

/// The 256-color palette to one of the 16 legacy colors (TextColor.cpp's
/// Index256ToIndex16); the first 16 are the ANSI-to-legacy swap.
#[rustfmt::skip]
const INDEX256_TO_INDEX16: [u8; 256] = [
     0,  4,  2,  6,  1,  5,  3,  7,  8, 12, 10, 14,  9, 13, 11, 15,
     0,  1,  1,  1,  9,  9,  2,  1,  1,  1,  1,  1,  2,  2,  3,  3,
     3,  3,  2,  2, 11, 11,  3,  3, 10, 10, 11, 11, 11, 11, 10, 10,
    10, 10, 11, 11,  5,  5,  5,  5,  1,  1,  8,  8,  1,  1,  9,  9,
     2,  2,  3,  3,  3,  3,  2,  2, 11, 11,  3,  3, 10, 10, 11, 11,
    11, 11, 10, 10, 10, 10, 11, 11,  4, 13,  5,  5,  5,  5,  4, 13,
    13, 13, 13, 13,  6,  8,  8,  8,  9,  9, 10, 10, 11, 11,  3,  3,
    10, 10, 11, 11, 11, 11, 10, 10, 10, 10, 11, 11,  4, 13, 13, 13,
    13, 13,  4, 12, 13, 13, 13, 13,  6,  6,  8,  8,  9,  9,  6,  6,
     7,  7,  7,  7, 10, 14, 14, 14,  7,  7, 10, 10, 14, 14, 11, 11,
     4, 12, 13, 13, 13, 13,  4, 12, 13, 13, 13, 13,  6,  6, 12, 12,
     7,  7,  6,  6,  7,  7,  7,  7,  6, 14, 14, 14,  7,  7, 14, 14,
    14, 14, 15, 15, 12, 12, 13, 13, 13, 13, 12, 12, 12, 12, 13, 13,
     6, 12, 12, 12,  7,  7,  6,  6,  7,  7,  7,  7,  6, 14, 14, 14,
     7,  7, 14, 14, 14, 14, 15, 15,  0,  0,  0,  0,  0,  0,  8,  8,
     8,  8,  8,  8,  8,  8,  8,  8,  7,  7,  7,  7,  7,  7, 15, 15,
];

#[cfg(test)]
#[path = "tests/legacy.rs"]
mod tests;
