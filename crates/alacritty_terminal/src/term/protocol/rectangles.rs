//! Rectangular DEC editing, with origin-relative coordinates and overlap-safe copies.
use super::*;
use crate::vte::Params;

#[derive(Clone)]
struct Rectangle {
    rows: Range<i32>,
    columns: Range<usize>,
}

fn parameter(params: &Params, index: usize, default: usize) -> usize {
    params.iter().nth(index).map(|p| usize::from(p[0])).filter(|p| *p != 0).unwrap_or(default)
}

#[derive(Default)]
struct Attributes(Vec<Attr>);
impl Handler for Attributes {
    fn terminal_attribute(&mut self, attr: Attr) {
        self.0.push(attr);
    }
}

impl<T: EventListener> Term<T> {
    fn rectangle(&self, params: &Params, offset: usize) -> Option<Rectangle> {
        let (rows, columns) = if self.mode.contains(TermMode::ORIGIN) {
            (self.scroll_region.start.0..self.scroll_region.end.0, self.horizontal_bounds())
        } else {
            (0..self.screen_lines() as i32, Column(0)..Column(self.columns()))
        };
        let height = (rows.end - rows.start) as usize;
        let width = columns.end.0 - columns.start.0;
        let top = parameter(params, offset, 1) - 1;
        let left = parameter(params, offset + 1, 1) - 1;
        let bottom = parameter(params, offset + 2, height).min(height);
        let right = parameter(params, offset + 3, width).min(width);
        if top >= bottom || left >= right {
            return None;
        }
        Some(Rectangle {
            rows: rows.start + top as i32..rows.start + bottom as i32,
            columns: columns.start.0 + left..columns.start.0 + right,
        })
    }

    pub(super) fn edit_rectangle(&mut self, params: &Params, action: char) {
        if action == 'v' {
            self.copy_rectangle(params);
            return;
        }
        let offset = usize::from(action == 'x');
        let Some(rectangle) = self.rectangle(params, offset) else { return };
        let mut attributes = Attributes::default();
        if matches!(action, 'r' | 't') {
            let mut iter = params.iter();
            for _ in 0..4 {
                iter.next();
            }
            ansi::attrs_from_sgr_parameters(&mut attributes, &mut iter);
            if attributes.0.is_empty() {
                attributes.0.push(Attr::Reset);
            }
        }
        let template = self.grid.cursor.template.clone();
        let blank = template.erased();
        let fill = parameter(params, 0, 32) as u32;
        if action == 'x' && !(32..=126).contains(&fill) && !(160..=255).contains(&fill) {
            return;
        }
        for row in rectangle.rows.clone() {
            let mut columns = rectangle.columns.clone();
            if matches!(action, 'r' | 't') && self.protocol.attribute_extent != 2 {
                if row != rectangle.rows.start {
                    columns.start = 0;
                }
                if row != rectangle.rows.end - 1 {
                    columns.end = self.columns();
                }
            }
            if !matches!(action, 'r' | 't') {
                self.clear_edit_boundary(Line(row), columns.start);
                self.clear_edit_boundary(Line(row), columns.end);
            }
            for column in columns.clone() {
                let cell = &mut self.grid[Line(row)][Column(column)];
                match action {
                    'x' => {
                        *cell = template.clone();
                        cell.c = char::from_u32(fill).unwrap();
                        cell.flags.insert(Flags::DRAWN);
                    },
                    'z' => *cell = blank.clone(),
                    '{' if !cell.flags.contains(Flags::PROTECTED) => *cell = blank.clone(),
                    'r' => {
                        let layout = cell.flags - crate::snapshot::RENDERED_FLAGS;
                        for attribute in &attributes.0 {
                            cell.apply_attribute(*attribute);
                        }
                        cell.flags |= layout;
                    },
                    't' => {
                        for attribute in &attributes.0 {
                            let flags = match attribute {
                                Attr::Reset => {
                                    Flags::BOLD | Flags::UNDERLINE | Flags::BLINK | Flags::INVERSE
                                },
                                Attr::Bold => Flags::BOLD,
                                Attr::Underline => Flags::UNDERLINE,
                                Attr::BlinkSlow | Attr::BlinkFast => Flags::BLINK,
                                Attr::Reverse => Flags::INVERSE,
                                _ => Flags::empty(),
                            };
                            cell.flags.toggle(flags);
                        }
                    },
                    _ => (),
                }
            }
            self.damage.damage_line(row as usize, columns.start, columns.end - 1);
        }
        self.selection = None;
    }

    /// DECRQCRA uses the DEC internal byte value, rather than UTF-8 bytes.
    /// XTCHECKSUM selects raw Unicode values and inclusion of undrawn blanks.
    pub(super) fn checksum_rectangle(&self, params: &Params) {
        let Some(rectangle) = self.rectangle(params, 2) else {
            return;
        };
        let mut sum = 0u16;
        let flags = self.protocol.checksum_flags;
        let mut first = true;
        {
            for row in rectangle.rows {
                for column in rectangle.columns.clone() {
                    let cell = &self.grid[Line(row)][Column(column)];
                    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        continue;
                    }
                    if !cell.flags.contains(Flags::DRAWN)
                        && flags & 4 == 0
                        && (flags & 8 == 0 || !first)
                    {
                        continue;
                    }
                    first = false;
                    let mut value = if flags & 16 != 0 {
                        cell.c as u16
                    } else {
                        match cell.c as u32 {
                            0..=31 | 256.. => 27,
                            160 => 32,
                            127 | 255 => 0,
                            value => (value & 0x7f) as u16,
                        }
                    };
                    if flags & 2 == 0 {
                        for (attribute, weight) in [
                            (Flags::PROTECTED | Flags::GUARDED, 4),
                            (Flags::HIDDEN, 8),
                            (Flags::ALL_UNDERLINES, 16),
                            (Flags::INVERSE, 32),
                            (Flags::BLINK, 64),
                            (Flags::BOLD, 128),
                        ] {
                            if cell.flags.intersects(attribute) {
                                value = value.wrapping_add(weight);
                            }
                        }
                    }
                    sum = sum.wrapping_add(value);
                    if flags & 16 == 0 {
                        for &scalar in cell.zerowidth().unwrap_or_default() {
                            sum = sum.wrapping_add(scalar as u16);
                        }
                    }
                }
            }
        }
        if flags & 1 == 0 {
            sum = sum.wrapping_neg();
        }
        let id = params.iter().next().map_or(0, |p| p[0]);
        self.write_reply(format!("\x1bP{id}!~{sum:04X}\x1b\\"));
    }

    fn copy_rectangle(&mut self, params: &Params) {
        // Like xterm, this terminal has one page, so page numbers select
        // the visible page without allocating hidden screens.
        let Some(source) = self.rectangle(params, 0) else { return };
        let (top, left, bottom, right) = if self.mode.contains(TermMode::ORIGIN) {
            let columns = self.horizontal_bounds();
            (self.scroll_region.start.0, columns.start.0, self.scroll_region.end.0, columns.end.0)
        } else {
            (0, 0, self.screen_lines() as i32, self.columns())
        };
        let row = top + parameter(params, 5, 1) as i32 - 1;
        let column = left + parameter(params, 6, 1) - 1;
        if row >= bottom || column >= right {
            return;
        }
        let height = (source.rows.end - source.rows.start).min(bottom - row);
        let width = (source.columns.end - source.columns.start).min(right - column);
        let mut cells = Vec::with_capacity(height as usize * width);
        for y in 0..height {
            for x in 0..width {
                cells.push(
                    self.grid[Line(source.rows.start + y)][Column(source.columns.start + x)]
                        .clone(),
                );
            }
        }
        for y in 0..height {
            self.clear_edit_boundary(Line(row + y), column);
            self.clear_edit_boundary(Line(row + y), column + width);
            for x in 0..width {
                self.grid[Line(row + y)][Column(column + x)] =
                    cells[y as usize * width + x].clone();
            }
            // A copied fragment must not leave half a wide glyph at either edge.
            let first = &mut self.grid[Line(row + y)][Column(column)];
            if first.flags.contains(Flags::WIDE_CHAR_SPACER) {
                first.clear_wide();
            }
            let last = &mut self.grid[Line(row + y)][Column(column + width - 1)];
            if last.flags.contains(Flags::WIDE_CHAR) {
                last.clear_wide();
            }
            self.damage.damage_line((row + y) as usize, column, column + width - 1);
        }
        self.selection = None;
    }
}
