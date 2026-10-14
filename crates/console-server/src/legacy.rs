// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Legacy console semantics expressed as VT for the embedder's parser:
//! attribute words as SGR, and text written with VT processing disabled.
//! These follow OpenConsole's VtIo writer, which does the same translation
//! for ConPTY, so the terminal sees what it would have seen from ConPTY.
use crate::output::{Pen, PenColor};
use crate::state::Palette;

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

/// The inverse of [`ansi_index`]; the swap is its own inverse.
pub fn legacy_index(ansi: u8) -> u16 {
    u16::from(ansi_index(u16::from(ansi)))
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

/// WriteUTF16StripControlChars, for ENABLE_PROCESSED_OUTPUT clear: every
/// control is shown as its glyph so the terminal never acts on it.
pub fn unprocessed(text: &[u16]) -> String {
    let mut out = String::with_capacity(text.len());
    for unit in char::decode_utf16(text.iter().copied()) {
        match unit {
            Ok(ch) if ch.is_ascii() && !is_control(ch as u16) => out.push(ch),
            Ok(ch) if (ch as u32) <= 0xffff => out.push(sanitize(ch as u16)),
            Ok(ch) => out.push(ch),
            Err(_) => out.push('\u{fffd}'),
        }
    }
    out
}

/// WriteCharsLegacy with ENABLE_PROCESSED_OUTPUT set and VT processing off.
/// BEL, BS, TAB and CR reach the terminal unchanged, NUL is a space, LF gains
/// a CR unless DISABLE_NEWLINE_AUTO_RETURN is set, and any other control is
/// the output code page's glyph for it (`glyph` returns None when the code
/// page has none, as UTF-8 does, and the character is dropped).
pub fn processed(text: &[u16], auto_return: bool, glyph: impl Fn(u8) -> Option<char>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous = None;
    for unit in char::decode_utf16(text.iter().copied()) {
        let ch = unit.unwrap_or('\u{fffd}');
        match ch {
            '\0' => out.push(' '),
            '\x07' | '\x08' | '\t' | '\r' => out.push(ch),
            '\n' if auto_return && previous != Some('\r') => out.push_str("\r\n"),
            '\n' => out.push('\n'),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                if let Some(glyph) = glyph(c as u8) {
                    out.push(glyph);
                }
            }
            c => out.push(c),
        }
        previous = Some(ch);
    }
    out
}

/// The attribute word GetConsoleScreenBufferInfo reports for a pen, as
/// TextAttribute::GetLegacyAttributes: default colors are the console's
/// default attributes, and colors outside the 16 take the nearest entry.
pub fn attributes(pen: &Pen, palette: &Palette) -> u16 {
    let foreground = nibble(
        pen.foreground,
        palette.attributes & FOREGROUND_MASK,
        palette,
    );
    let background = nibble(
        pen.background,
        (palette.attributes & BACKGROUND_MASK) >> 4,
        palette,
    );
    let mut value = foreground | (background << 4);
    if pen.reverse {
        value |= REVERSE_VIDEO;
    }
    if pen.underline {
        value |= UNDERSCORE;
    }
    value
}

fn nibble(color: PenColor, default: u16, palette: &Palette) -> u16 {
    match color {
        PenColor::Default => default,
        PenColor::Indexed(index) if index < 16 => legacy_index(index),
        PenColor::Indexed(index) => nearest(xterm_rgb(index), palette),
        PenColor::Rgb(r, g, b) => nearest((r, g, b), palette),
    }
}

/// The xterm 256-color cube and gray ramp.
fn xterm_rgb(index: u8) -> (u8, u8, u8) {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    if index >= 232 {
        let level = 8 + 10 * (index - 232);
        return (level, level, level);
    }
    let cube = index - 16;
    (
        LEVELS[usize::from(cube / 36)],
        LEVELS[usize::from(cube / 6 % 6)],
        LEVELS[usize::from(cube % 6)],
    )
}

/// The legacy index of the closest palette color (COLORREF is 0x00bbggrr).
fn nearest((r, g, b): (u8, u8, u8), palette: &Palette) -> u16 {
    let distance = |color: u32| {
        let channel = |shift: u32, value: u8| {
            let delta = i32::from(((color >> shift) & 0xff) as u8) - i32::from(value);
            delta * delta
        };
        channel(0, r) + channel(8, g) + channel(16, b)
    };
    (0..16u16)
        .min_by_key(|&index| distance(palette.colors[usize::from(index)]))
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/legacy.rs"]
mod tests;
