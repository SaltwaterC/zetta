//! Dispatch for additional DEC control sequences and their replay ordering.
use super::*;
use crate::grid::LineSize;
use crate::vte::Params;

impl<T: EventListener> Term<T> {
    pub(in crate::term) fn report_cursor_position(&self) -> Point {
        let mut point = self.grid.cursor.point;
        if self.mode.contains(TermMode::ORIGIN) {
            point.line -= self.scroll_region.start.0;
            point.column -= self.horizontal_bounds().start.0;
        }
        point
    }

    pub(in crate::term) fn dispatch_csi_extension(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        action: char,
    ) -> bool {
        let parameter = |index: usize, default| {
            params.iter().nth(index).map(|p| p[0]).filter(|p| *p != 0).unwrap_or(default)
        };
        let handled = matches!(
            (intermediates, action),
            (b"\"", 'p' | 'q')
                | (b" ", '@' | 'A')
                | (b"*", 'x' | 'y')
                | (b"#", 'y' | '{' | '}' | 'p' | 'q')
                | (b"$", 'r' | 't' | 'v' | 'x' | 'z' | '{')
                | (b"?", 'n' | 'J' | 'K' | 's' | 'r')
                | (b"", 'x' | 'e')
                | (b"'", '}' | '~' | '|')
        );
        if !handled {
            return false;
        }
        let _recording = self.reflow.record(|| {
            let params = params.clone();
            let intermediates = intermediates.to_vec();
            move |term| {
                term.csi_extension(&params, &intermediates, action);
            }
        });
        match (intermediates, action) {
            (b"#", '{' | 'p') => self.push_sgr(params),
            (b"#", '}' | 'q') => self.pop_sgr(),
            (b"?", 's' | 'r') => {
                for param in params.iter() {
                    if action == 's' {
                        self.save_private_mode(param[0]);
                    } else {
                        self.restore_private_mode(param[0]);
                    }
                }
            },
            (b"#", 'y') => self.protocol.checksum_flags = parameter(0, 0) as u8,
            (b"*", 'y') => self.checksum_rectangle(params),
            (b" ", '@' | 'A') => self.shift_columns(parameter(0, 1) as usize, action == '@'),
            (b"?", 'J') => self.erase_screen_protected(parameter(0, 0), Flags::PROTECTED),
            (b"?", 'K') => self.erase_line_protected(parameter(0, 0), Flags::PROTECTED),
            (b"$", 'r' | 't' | 'v' | 'x' | 'z' | '{') => self.edit_rectangle(params, action),
            (b"\"", 'q') => {
                self.grid.cursor.template.flags.set(Flags::PROTECTED, parameter(0, 0) == 1)
            },
            (b"\"", 'p') => {
                let level = parameter(0, 61);
                if (61..=65).contains(&level) {
                    self.reset_state();
                    self.protocol.level = level as u8;
                    self.protocol.c1 = level > 61 && parameter(1, 0) != 1;
                }
            },
            (b"*", 'x') => {
                self.protocol.attribute_extent = match parameter(0, 0) {
                    0 | 1 => 0,
                    2 => 2,
                    _ => return true,
                }
            },
            (b"?", 'n') => self.private_device_status(parameter(0, 0), parameter(1, 0)),
            (b"", 'x') => {
                let request = parameter(0, 0);
                if request <= 1 {
                    self.write_reply(format!("\x1b[{};1;1;128;128;1;0x", request + 2));
                }
            },
            (b"", 'e') => {
                let offset = if self.mode.contains(TermMode::ORIGIN) {
                    self.scroll_region.start.0
                } else {
                    0
                };
                let line = self.grid.cursor.point.line.0 - offset;
                self.goto_line(line.saturating_add(i32::from(parameter(0, 1))));
            },
            (b"'", '|') => self.write_reply("\x1b[0&w".into()),
            (b"'", '}' | '~') => {
                let original = self.grid.cursor.point;
                if self.scroll_region.contains(&original.line) {
                    for row in self.scroll_region.start.0..self.scroll_region.end.0 {
                        self.grid.cursor.point.line = Line(row);
                        if action == '}' {
                            self.insert_blank_cells(parameter(0, 1) as usize);
                        } else {
                            self.delete_chars_cells(parameter(0, 1) as usize);
                        }
                    }
                    self.grid.cursor.point = original;
                }
            },
            _ => unreachable!(),
        }
        true
    }

    fn private_device_status(&self, request: u16, id: u16) {
        let fixed = match request {
            15 => "\x1b[?13n",
            // UDK definitions are not supported, but DECUDK's lock is kept.
            25 if self.protocol.udk_locked => "\x1b[?21n",
            25 => "\x1b[?20n",
            26 => "\x1b[?27;1;0;0n",
            55 => "\x1b[?50n",
            56 => "\x1b[?57;1n",
            62 => "\x1b[0*{",
            75 => "\x1b[?70n",
            85 => "\x1b[?83n",
            _ => "",
        };
        if !fixed.is_empty() {
            self.write_reply(fixed.into());
        } else if request == 6 {
            let point = self.report_cursor_position();
            self.write_reply(format!("\x1b[?{};{};1R", point.line + 1, point.column + 1));
        } else if request == 63 {
            self.write_reply(format!("\x1bP{id}!~0000\x1b\\"));
        }
    }

    pub(in crate::term) fn dispatch_esc_extension(
        &mut self,
        intermediates: &[u8],
        byte: u8,
    ) -> bool {
        if self.protocol.level == 52 {
            self.dispatch_vt52(byte);
            return true;
        }
        if !matches!(
            (intermediates, byte),
            (b" ", b'F' | b'G')
                | (b"", b'N' | b'O' | b'n' | b'o' | b'6' | b'9' | b'V' | b'W')
                | (b"#", b'3' | b'4' | b'5' | b'6')
        ) {
            return false;
        }
        let _recording = self.reflow.record(|| {
            let intermediates = intermediates.to_vec();
            move |term| {
                term.esc_extension(&intermediates, byte);
            }
        });
        match (intermediates, byte) {
            (b" ", b'F' | b'G') => self.protocol.c1 = byte == b'G' && self.protocol.level >= 62,
            (b"", b'N' | b'O') => {
                self.protocol.single_shift =
                    Some(if byte == b'N' { CharsetIndex::G2 } else { CharsetIndex::G3 })
            },
            (b"", b'n' | b'o') => self.set_active_charset(if byte == b'n' {
                CharsetIndex::G2
            } else {
                CharsetIndex::G3
            }),
            (b"", b'V' | b'W') => {
                self.protocol.iso_guarded = true;
                self.grid.cursor.template.flags.set(Flags::GUARDED, byte == b'V');
            },
            (b"", b'6' | b'9') => self.column_index(byte == b'9'),
            (b"#", _) => self.set_line_size(match byte {
                b'3' => LineSize::DoubleHeightTop,
                b'4' => LineSize::DoubleHeightBottom,
                b'5' => LineSize::Single,
                _ => LineSize::DoubleWidth,
            }),
            _ => unreachable!(),
        }
        true
    }

    fn shift_columns(&mut self, count: usize, forward: bool) {
        if !self.scroll_region.contains(&self.grid.cursor.point.line) {
            return;
        }
        let cursor = self.grid.cursor.clone();
        self.grid.cursor.point.column = self.horizontal_bounds().start;
        for row in self.scroll_region.start.0..self.scroll_region.end.0 {
            self.grid.cursor.point.line = Line(row);
            if forward {
                self.delete_chars_cells(count);
            } else {
                self.insert_blank_cells(count);
            }
        }
        self.grid.cursor = cursor;
    }

    fn column_index(&mut self, forward: bool) {
        let position = self.grid.cursor.point;
        let region = self.horizontal_bounds();
        let inside = region.contains(&position.column);
        let left = if inside { region.start } else { Column(0) };
        let right = if inside { region.end - 1 } else { self.last_column() };
        if forward && position.column < right {
            self.move_forward(1);
        } else if !forward && position.column > left {
            self.move_backward(1);
        } else {
            let margins = self.horizontal_margin.clone();
            if !inside {
                self.horizontal_margin = None;
            }
            self.grid.cursor.point.column = left;
            // DECBI/DECFI shift the scrolling rectangle even when the
            // cursor's row is outside its vertical boundaries.
            for row in self.scroll_region.start.0..self.scroll_region.end.0 {
                self.grid.cursor.point.line = Line(row);
                if forward {
                    self.delete_chars_cells(1);
                } else {
                    self.insert_blank_cells(1);
                }
            }
            self.horizontal_margin = margins;
            self.grid.cursor.point = position;
        }
    }
}
