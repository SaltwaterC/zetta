//! VT52 compatibility commands, kept distinct from ANSI escape meanings.
use super::*;

impl<T: EventListener> Term<T> {
    pub(super) fn dispatch_vt52(&mut self, byte: u8) {
        let _recording = self.reflow.record(|| {
            move |term| {
                term.esc_extension(&[], byte);
            }
        });
        match byte {
            b'A' => self.move_up(1),
            b'B' => self.move_down(1),
            b'C' => self.move_forward(1),
            b'D' => self.move_backward(1),
            b'H' => self.goto(0, 0),
            b'I' => self.reverse_index(),
            b'J' => self.clear_screen(ansi::ClearMode::Below),
            b'K' => self.clear_line(ansi::LineClearMode::Right),
            b'Z' => self.write_reply("\x1b/Z".into()),
            b'F' | b'G' => {
                self.active_charset = CharsetIndex::G0;
                self.configure_charset(
                    CharsetIndex::G0,
                    if byte == b'F' {
                        StandardCharset::SpecialCharacterAndLineDrawing
                    } else {
                        StandardCharset::Ascii
                    },
                );
            },
            b'=' => self.set_keypad_application_mode(),
            b'>' => self.unset_keypad_application_mode(),
            b'<' => {
                self.protocol.level = 61;
                self.protocol.c1 = false;
                self.grid.cursor.charsets = Default::default();
                self.active_charset = CharsetIndex::G0;
            },
            // RIS is an extension accepted here to make a frontend hard reset
            // leave compatibility mode, including after an incomplete command.
            b'c' => self.reset_state(),
            _ => (),
        }
    }
}
