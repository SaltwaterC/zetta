//! Translate observable core grid state into the imported harness's cell model.
//!
//! Colors use the harness's explicitly pinned VGA palette, rather than a user's
//! theme. Fields with no equivalent in this engine are omitted, never fabricated.

use alacritty_terminal::grid::{Dimensions, LineSize};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::{Term, cell::Flags, color::Colors};
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use serde_json::{Value, json};
use std::collections::HashMap;

pub(super) fn snapshot<T: alacritty_terminal::event::EventListener>(terminal: &Term<T>) -> Value {
    let grid = terminal.grid();
    let colors = terminal.renderable_content().colors;
    let mut indices = HashMap::new();
    let mut symbols = Vec::new();
    let mut cells = Vec::new();
    let mut lines = Vec::new();
    let line = |row: usize| &grid[Line(row as i32 - grid.display_offset() as i32)];
    let moved_wraps: Vec<_> = (0..grid.screen_lines())
        .map(|row| wrap_before_leading_spacer(line(row), grid.columns()))
        .collect();
    for (row, moved_wrap) in moved_wraps.iter().enumerate() {
        let line = line(row);
        let line_attribute = line_attribute(line.line_size());
        let mut text = String::new();
        for column in 0..grid.columns() {
            let cell = match moved_wrap {
                Some((padding, _)) if column + 1 == grid.columns() => padding,
                Some((_, written)) if column + 2 == grid.columns() => written,
                _ => &line[Column(column)],
            };
            text.push(cell.c);
            let index = *indices
                .entry(SymbolKey::new(cell, line_attribute))
                .or_insert_with(|| {
                    let index = symbols.len();
                    symbols.push(cell_value(cell, colors, line_attribute));
                    index
                });
            cells.push(index);
        }
        lines.push(text);
    }
    json!({
        "columns": grid.columns(), "rows": grid.screen_lines(),
        "cursor_x": grid.cursor.point.column.0, "cursor_y": grid.cursor.point.line.0,
        "view_offset": grid.display_offset(), "cells": cells, "symbols": symbols, "lines": lines,
    })
}

/// A wide character that did not fit leaves a blank leading spacer in the last
/// column, and the engine marks the wrap there. The harness marks the wrap on
/// the last written cell, so move it onto the cell before the padding.
fn wrap_before_leading_spacer(
    line: &alacritty_terminal::grid::Row<Cell>,
    columns: usize,
) -> Option<(Cell, Cell)> {
    let last = &line[Column(columns.checked_sub(2)? + 1)];
    if !last
        .flags
        .contains(Flags::LEADING_WIDE_CHAR_SPACER | Flags::WRAPLINE)
    {
        return None;
    }
    let mut padding = last.clone();
    padding.flags.remove(Flags::WRAPLINE);
    let mut written = line[Column(columns - 2)].clone();
    written.flags.insert(Flags::WRAPLINE);
    Some((padding, written))
}

/// The harness's encoding of a row's DEC line size, which it reports per cell.
fn line_attribute(line_size: LineSize) -> u8 {
    match line_size {
        LineSize::Single => 0,
        LineSize::DoubleHeightTop => 1,
        LineSize::DoubleHeightBottom => 2,
        LineSize::DoubleWidth => 3,
    }
}

// Repeated blank cells and styles dominate large snapshots. Interning complete
// observations keeps the control channel proportional to the grid's content.
#[derive(Eq, PartialEq, Hash)]
struct SymbolKey<'a> {
    character: char,
    flags: u32,
    colors: [u32; 3],
    cluster: &'a [char],
    uri: Option<String>,
    line_attribute: u8,
}

impl<'a> SymbolKey<'a> {
    fn new(cell: &'a Cell, line_attribute: u8) -> Self {
        let encode = |color| match color {
            Color::Spec(rgb) => u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b),
            Color::Named(name) => 0x1000000 | name as u32,
            Color::Indexed(index) => 0x2000000 | u32::from(index),
        };
        Self {
            character: cell.c,
            flags: cell.flags.bits(),
            colors: [
                encode(cell.fg),
                encode(cell.bg),
                cell.underline_color().map_or(u32::MAX, encode),
            ],
            cluster: cell.zerowidth().unwrap_or_default(),
            uri: cell.hyperlink().map(|link| link.uri().to_owned()),
            line_attribute,
        }
    }
}

fn cell_value(cell: &Cell, colors: &Colors, line_attribute: u8) -> Value {
    let flags = cell.flags;
    let (foreground, foreground_index) = color(cell.fg, colors);
    let (background, background_index) = color(cell.bg, colors);
    let (underline_color, underline_index) = color(
        cell.underline_color()
            .unwrap_or(if flags.contains(Flags::INVERSE) {
                cell.bg
            } else {
                cell.fg
            }),
        colors,
    );
    let underline_style = [
        Flags::UNDERLINE,
        Flags::DOUBLE_UNDERLINE,
        Flags::UNDERCURL,
        Flags::DOTTED_UNDERLINE,
        Flags::DASHED_UNDERLINE,
    ]
    .iter()
    .position(|flag| flags.contains(*flag))
    .map_or(0, |index| index + 1);
    json!({
                "char": cell.c.to_string(),
                "double_width": flags.contains(Flags::WIDE_CHAR),
                "double_width_continuation": flags.contains(Flags::WIDE_CHAR_SPACER),
                "bold": flags.contains(Flags::BOLD), "italic": flags.contains(Flags::ITALIC),
                "underline": flags.intersects(Flags::ALL_UNDERLINES),
                "underline_style": underline_style,
                "blink": flags.contains(Flags::BLINK), "overline": flags.contains(Flags::OVERLINE),
                "protected": flags.intersects(Flags::PROTECTED | Flags::GUARDED),
                "faint": flags.contains(Flags::DIM), "conceal": flags.contains(Flags::HIDDEN),
                "strike": flags.contains(Flags::STRIKEOUT), "inverse": flags.contains(Flags::INVERSE),
                "wrapped": flags.contains(Flags::WRAPLINE), "drawn": flags.contains(Flags::DRAWN),
                "foreground": foreground, "foreground_index": foreground_index,
                "background": background, "background_index": background_index,
                "underline_color": underline_color, "underline_index": underline_index,
                "uri": cell.hyperlink().map(|link| link.uri().to_owned()),
                "line_attribute": line_attribute,
                "grapheme": cell.zerowidth().filter(|chars| !chars.is_empty()).map(|chars| {
                    std::iter::once(cell.c as u32).chain(chars.iter().map(|c| *c as u32)).collect::<Vec<_>>()
                }).unwrap_or_default(),
    })
}

fn color(color: Color, colors: &Colors) -> ([u8; 3], i32) {
    let (slot, index) = match color {
        Color::Spec(rgb) => return ([rgb.r, rgb.g, rgb.b], -1),
        Color::Indexed(index) => (index as usize, index as i32),
        Color::Named(named) => (
            named as usize,
            if (named as usize) < 256 {
                named as i32
            } else {
                -2
            },
        ),
    };
    if let Some(rgb) = colors[slot] {
        return ([rgb.r, rgb.g, rgb.b], index);
    }
    const VGA: [u32; 16] = [
        0x000000, 0xaa0000, 0x00aa00, 0xaa5500, 0x0000aa, 0xaa00aa, 0x00aaaa, 0xaaaaaa, 0x555555,
        0xff5555, 0x55ff55, 0xffff55, 0x5555ff, 0xff55ff, 0x55ffff, 0xffffff,
    ];
    let rgb = match slot {
        0..16 => VGA[slot],
        16..232 => {
            let component = |part| if part == 0 { 0 } else { 55 + 40 * part };
            let value = slot as u32 - 16;
            component(value / 36) << 16 | component(value / 6 % 6) << 8 | component(value % 6)
        }
        232..256 => (8 + 10 * (slot as u32 - 232)) * 0x010101,
        value if value == NamedColor::Background as usize => 0,
        _ => 0xffffff,
    };
    ([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8], index)
}
