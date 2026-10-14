// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Console-wide settings OpenConsole keeps in CONSOLE_INFORMATION and on its
//! screen buffer: modes, code pages, cursor size, titles, the color table and
//! the requested buffer size. Validation follows getset.cpp; turning a change
//! into VT for the embedder is the caller's job.

pub mod mode {
    pub const PROCESSED_INPUT: u32 = 0x0001;
    pub const LINE_INPUT: u32 = 0x0002;
    pub const ECHO_INPUT: u32 = 0x0004;
    pub const WINDOW_INPUT: u32 = 0x0008;
    pub const MOUSE_INPUT: u32 = 0x0010;
    pub const INSERT_MODE: u32 = 0x0020;
    pub const QUICK_EDIT_MODE: u32 = 0x0040;
    pub const EXTENDED_FLAGS: u32 = 0x0080;
    pub const AUTO_POSITION: u32 = 0x0100;
    pub const VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    pub const INPUT_MODES: u32 = PROCESSED_INPUT
        | LINE_INPUT
        | ECHO_INPUT
        | WINDOW_INPUT
        | MOUSE_INPUT
        | VIRTUAL_TERMINAL_INPUT;
    pub const PRIVATE_MODES: u32 = INSERT_MODE | QUICK_EDIT_MODE | AUTO_POSITION | EXTENDED_FLAGS;

    pub const PROCESSED_OUTPUT: u32 = 0x0001;
    pub const WRAP_AT_EOL_OUTPUT: u32 = 0x0002;
    pub const VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    pub const DISABLE_NEWLINE_AUTO_RETURN: u32 = 0x0008;
    pub const LVB_GRID_WORLDWIDE: u32 = 0x0010;
    pub const OUTPUT_MODES: u32 = PROCESSED_OUTPUT
        | WRAP_AT_EOL_OUTPUT
        | VIRTUAL_TERMINAL_PROCESSING
        | DISABLE_NEWLINE_AUTO_RETURN
        | LVB_GRID_WORLDWIDE;
}

/// The Campbell scheme, which a ConPTY console reports until told otherwise.
/// COLORREF (0x00bbggrr) in console attribute order.
pub const CAMPBELL: [u32; 16] = [
    0x0c0c0c, 0xda3700, 0x0ea113, 0xdd963a, 0x1f0fc5, 0x981788, 0x009cc1, 0xcccccc, 0x767676,
    0xff783b, 0x0cc616, 0xd6d661, 0x5648e7, 0x9e00b4, 0xa5f1f9, 0xf2f2f2,
];

/// What the embedder hands a new console in place of OpenConsole's defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// COLORREF values in console attribute order.
    pub colors: [u32; 16],
    /// Legacy attribute word whose colors are the terminal's defaults.
    pub attributes: u16,
}
impl Default for Palette {
    fn default() -> Self {
        Self {
            colors: CAMPBELL,
            attributes: 0x07,
        }
    }
}

pub struct ConsoleState {
    input_mode: u32,
    insert: bool,
    quick_edit: bool,
    auto_position: bool,
    private_flags: bool,
    pub output_mode: u32,
    pub input_cp: u32,
    pub output_cp: u32,
    pub cursor_size: u32,
    pub title: String,
    pub original_title: String,
    titled: bool,
    pub palette: Palette,
    pub popup_attributes: u16,
    /// The main buffer's window and size as an application set them.
    view: Option<View>,
}

/// An application's window and buffer size for the main buffer, valid for
/// the terminal size they were set against: a resize replaces both, as
/// ResizePseudoConsole does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    against: (i16, i16),
    window: (i16, i16),
    size: (i16, i16),
}
impl ConsoleState {
    pub fn new(code_page: u32, palette: Palette) -> Self {
        Self {
            // OpenConsole's defaults: 0x1f7 as GetConsoleMode reports it.
            input_mode: mode::PROCESSED_INPUT
                | mode::LINE_INPUT
                | mode::ECHO_INPUT
                | mode::MOUSE_INPUT,
            insert: true,
            quick_edit: true,
            auto_position: true,
            private_flags: true,
            output_mode: mode::PROCESSED_OUTPUT
                | mode::WRAP_AT_EOL_OUTPUT
                | mode::VIRTUAL_TERMINAL_PROCESSING,
            input_cp: code_page,
            output_cp: code_page,
            cursor_size: 25,
            title: String::new(),
            original_title: String::new(),
            titled: false,
            palette,
            popup_attributes: 0xf5,
            view: None,
        }
    }

    /// GetConsoleInputModeImpl: the stored mode plus the private flags,
    /// which are only reported once ENABLE_EXTENDED_FLAGS has been used.
    pub fn input_mode(&self) -> u32 {
        let mut value = self.input_mode;
        if self.private_flags {
            value |= mode::EXTENDED_FLAGS;
            if self.insert {
                value |= mode::INSERT_MODE;
            }
            if self.quick_edit {
                value |= mode::QUICK_EDIT_MODE;
            }
            if self.auto_position {
                value |= mode::AUTO_POSITION;
            }
        }
        value
    }

    /// Whether the terminal should report mouse events: quick edit owns the
    /// mouse while it is on.
    pub fn wants_mouse(&self) -> bool {
        self.input_mode & mode::MOUSE_INPUT != 0 && !(self.private_flags && self.quick_edit)
    }

    /// SetConsoleInputModeImpl stores the mode before validating it, so an
    /// invalid request still takes effect and then reports E_INVALIDARG.
    pub fn set_input_mode(&mut self, value: u32) -> bool {
        if value & mode::PRIVATE_MODES != 0 {
            self.private_flags = true;
            self.quick_edit = value & mode::QUICK_EDIT_MODE != 0;
            self.auto_position = value & mode::AUTO_POSITION != 0;
            self.insert = value & mode::INSERT_MODE != 0;
        } else {
            self.private_flags = false;
        }
        self.input_mode = value & !mode::PRIVATE_MODES;
        value & !(mode::INPUT_MODES | mode::PRIVATE_MODES) == 0
            && !(value & mode::ECHO_INPUT != 0 && value & mode::LINE_INPUT == 0)
    }

    /// Insert mode as the cooked line editor uses it.
    pub fn insert_mode(&self) -> bool {
        self.insert
    }

    /// SetConsoleOutputModeImpl rejects unknown bits before changing anything.
    pub fn set_output_mode(&mut self, value: u32) -> Option<u32> {
        if value & !mode::OUTPUT_MODES != 0 {
            return None;
        }
        Some(std::mem::replace(&mut self.output_mode, value))
    }

    pub fn vt_output(&self) -> bool {
        self.output_mode & mode::VIRTUAL_TERMINAL_PROCESSING != 0
    }

    /// The first client's startup title is the console's title until changed.
    pub fn connected(&mut self, title: &str) {
        if !self.titled {
            self.titled = true;
            title.clone_into(&mut self.title);
            title.clone_into(&mut self.original_title);
        }
    }

    fn view(&self, terminal: (i16, i16)) -> View {
        match self.view {
            Some(view) if view.against == terminal => view,
            _ => View {
                against: terminal,
                window: terminal,
                size: terminal,
            },
        }
    }
    fn store(&mut self, view: View) {
        self.view = (view.window != view.against || view.size != view.against).then_some(view);
    }

    /// The main buffer's reported size on a terminal of `terminal` cells.
    pub fn buffer_size(&self, terminal: (i16, i16)) -> (i16, i16) {
        self.view(terminal).size
    }

    /// The main buffer's reported window on a terminal of `terminal` cells.
    pub fn window_size(&self, terminal: (i16, i16)) -> (i16, i16) {
        self.view(terminal).window
    }

    /// SetConsoleWindowInfoImpl in VtIo mode: the buffer is resized to the
    /// window, which then shows all of it. The terminal is not resized.
    pub fn set_window_size(&mut self, terminal: (i16, i16), window: (i16, i16)) {
        self.store(View {
            against: terminal,
            window,
            size: window,
        });
    }

    /// SetConsoleScreenBufferSizeImpl: never smaller than the window, and
    /// SHORT_MAX is reserved.
    pub fn set_buffer_size(&mut self, terminal: (i16, i16), size: (i16, i16)) -> bool {
        let view = self.view(terminal);
        if size.0 < view.window.0
            || size.1 < view.window.1
            || size.0 == i16::MAX
            || size.1 == i16::MAX
        {
            return false;
        }
        self.store(View { size, ..view });
        true
    }

    /// Puts back what the main buffer had while another one was on screen.
    pub fn restore_view(&mut self, terminal: (i16, i16), window: (i16, i16), size: (i16, i16)) {
        self.store(View {
            against: terminal,
            window,
            size,
        });
    }
}

#[cfg(test)]
#[path = "tests/state.rs"]
mod tests;
