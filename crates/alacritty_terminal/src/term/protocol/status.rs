//! DECRQSS and XTGETTCAP report actual current state and implemented capabilities.
use super::*;
use crate::vte::Params;

impl<T: EventListener> Term<T> {
    pub(in crate::term) fn handle_dcs(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        action: char,
        data: &[u8],
    ) {
        if self.protocol.level < 62 {
            return;
        }
        match (intermediates, action) {
            (b"$", 'q') => self.report_setting(data),
            (b"+", 'q') => self.report_capabilities(data),
            // DECUDK: a lock parameter of 0 (the default) locks the keys.
            (b"", '|') => {
                let lock = params.iter().nth(1).map_or(0, |param| param[0]);
                self.protocol.udk_locked |= lock == 0;
            },
            _ => (),
        }
    }

    fn report_setting(&mut self, request: &[u8]) {
        let setting = match request {
            b"m" => Some(self.sgr_status()),
            b"r" => Some(format!("{};{}r", self.scroll_region.start + 1, self.scroll_region.end)),
            b"s" => {
                let bounds = self.horizontal_bounds();
                Some(format!("{};{}s", bounds.start + 1, bounds.end))
            },
            b"\"q" => Some(format!(
                "{}\"q",
                u8::from(self.grid.cursor.template.flags.contains(Flags::PROTECTED))
            )),
            b"\"p" => Some(format!("{};{}\"p", self.protocol.level, u8::from(!self.protocol.c1))),
            b"*x" => Some(format!("{}*x", self.protocol.attribute_extent)),
            b" q" => {
                let style = self.cursor_style();
                let shape = match style.shape {
                    CursorShape::Underline => 3,
                    CursorShape::Beam => 5,
                    _ => 1,
                };
                Some(format!("{} q", shape + u8::from(!style.blinking)))
            },
            _ => None,
        };
        self.write_reply(match setting {
            Some(setting) => format!("\x1bP1$r{setting}\x1b\\"),
            None => "\x1bP0$r\x1b\\".into(),
        });
    }

    fn sgr_status(&self) -> String {
        let cell = &self.grid.cursor.template;
        let mut parts = vec!["0".to_owned()];
        for (flag, code) in [(Flags::BOLD, "1"), (Flags::DIM, "2"), (Flags::ITALIC, "3")] {
            if cell.flags.contains(flag) {
                parts.push(code.into());
            }
        }
        for (flag, code) in [
            (Flags::UNDERLINE, "4"),
            (Flags::DOUBLE_UNDERLINE, "4:2"),
            (Flags::UNDERCURL, "4:3"),
            (Flags::DOTTED_UNDERLINE, "4:4"),
            (Flags::DASHED_UNDERLINE, "4:5"),
        ] {
            if cell.flags.contains(flag) {
                parts.push(code.into());
            }
        }
        for (flag, code) in [
            (Flags::BLINK, "5"),
            (Flags::INVERSE, "7"),
            (Flags::HIDDEN, "8"),
            (Flags::STRIKEOUT, "9"),
            (Flags::OVERLINE, "53"),
        ] {
            if cell.flags.contains(flag) {
                parts.push(code.into());
            }
        }
        append_color(&mut parts, cell.fg, 38);
        append_color(&mut parts, cell.bg, 48);
        if let Some(color) = cell.underline_color() {
            append_color(&mut parts, color, 58);
        }
        parts.join(";") + "m"
    }

    fn report_capabilities(&self, request: &[u8]) {
        for encoded in request.split(|byte| *byte == b';') {
            let name = decode_hex(encoded);
            let value = match name.as_deref() {
                Some(b"TN") => Some("xterm-256color"),
                Some(b"Co" | b"colors") => Some("256"),
                Some(b"RGB") => Some("8"),
                _ => None,
            };
            let encoded = String::from_utf8_lossy(encoded);
            let reply = match value {
                Some(value) => {
                    let hex: String = value.bytes().map(|byte| format!("{byte:02x}")).collect();
                    format!("\x1bP1+r{encoded}={hex}\x1b\\")
                },
                None => format!("\x1bP0+r{encoded}\x1b\\"),
            };
            self.write_reply(reply);
        }
    }
}

fn append_color(parts: &mut Vec<String>, color: Color, channel: u8) {
    let index = match color {
        Color::Named(name) if name as usize >= 256 => return,
        Color::Named(name) => name as u8,
        Color::Indexed(index) => index,
        Color::Spec(rgb) => {
            parts.push(format!("{channel}:2::{}:{}:{}", rgb.r, rgb.g, rgb.b));
            return;
        },
    };
    parts.push(if channel != 58 && index < 16 {
        (u16::from(channel - 8) + u16::from(index % 8) + if index >= 8 { 60 } else { 0 })
            .to_string()
    } else {
        format!("{channel}:5:{index}")
    });
}

fn decode_hex(data: &[u8]) -> Option<Vec<u8>> {
    if !data.len().is_multiple_of(2) {
        return None;
    }
    data.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}
