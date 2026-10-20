//! Bounded xterm rendition stack; selective pops preserve unselected live state.
use super::*;
use crate::vte::Params;

#[derive(Clone)]
pub(super) struct SavedSgr {
    template: Cell,
    attributes: Flags,
    foreground: bool,
    background: bool,
    underline_color: bool,
}

impl<T: EventListener> Term<T> {
    pub(super) fn push_sgr(&mut self, params: &Params) {
        let all = params.is_empty() || params.iter().all(|p| p == [0]);
        let mut saved = SavedSgr {
            template: self.grid.cursor.template.clone(),
            attributes: if all { crate::snapshot::RENDERED_FLAGS } else { Flags::empty() },
            foreground: all,
            background: all,
            underline_color: all,
        };
        for parameter in params.iter() {
            saved.attributes |= match parameter[0] {
                1 => Flags::BOLD,
                2 => Flags::DIM,
                3 => Flags::ITALIC,
                4 => Flags::ALL_UNDERLINES,
                5 | 6 => Flags::BLINK,
                7 => Flags::INVERSE,
                8 => Flags::HIDDEN,
                9 => Flags::STRIKEOUT,
                21 => Flags::DOUBLE_UNDERLINE,
                53 => Flags::OVERLINE,
                30 => {
                    saved.foreground = true;
                    Flags::empty()
                },
                31 => {
                    saved.background = true;
                    Flags::empty()
                },
                _ => Flags::empty(),
            };
        }
        if self.protocol.sgr_stack.len() == 10 {
            self.protocol.sgr_stack.remove(0);
        }
        self.protocol.sgr_stack.push(saved);
    }

    pub(super) fn pop_sgr(&mut self) {
        let Some(saved) = self.protocol.sgr_stack.pop() else { return };
        let template = &mut self.grid.cursor.template;
        template.flags =
            (template.flags - saved.attributes) | (saved.template.flags & saved.attributes);
        if saved.foreground {
            template.fg = saved.template.fg;
        }
        if saved.background {
            template.bg = saved.template.bg;
        }
        if saved.underline_color {
            template.set_underline_color(saved.template.underline_color());
        }
    }
}
