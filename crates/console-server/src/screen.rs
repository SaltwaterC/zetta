// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Screen-buffer state APIs: buffer info, size and window, cursor and text
//! attributes. Queries read the embedder's grid after a flush; changes reach
//! it as the VT OpenConsole's VtIo writer emits for the same call.
use super::*;
use crate::legacy;
use crate::output::Screen;

/// CONSOLE_SCREENBUFFERINFO_MSG field offsets.
const SIZE: usize = 0;
const CURSOR: usize = 4;
const SCROLL: usize = 8;
const ATTRIBUTES: usize = 12;
const WINDOW: usize = 14;
const MAXIMUM: usize = 18;
const POPUP: usize = 22;
const COLORS: usize = 28;

impl Server {
    /// The embedder's screen once every earlier write has been parsed.
    /// Waits until the embedder has parsed every earlier write, unless it
    /// has stopped taking output.
    pub(super) fn flush_output(&mut self) -> io::Result<()> {
        if self.detached {
            return Ok(());
        }
        match self.sink.flush() {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                self.detached = true;
                Ok(())
            }
            result => result,
        }
    }
    pub(super) fn screen(&mut self) -> io::Result<Screen> {
        self.flush_output()?;
        Ok(self.sink.screen())
    }
    fn window(screen: &Screen) -> (i16, i16) {
        (screen.columns, screen.rows)
    }
    /// The buffer a handle names, when it is not the one on screen.
    fn off_screen(&self, object: usize) -> Option<u32> {
        self.output_buffer(object)
            .filter(|&buffer| buffer != self.active)
    }
    pub(super) fn get_screen_info(&mut self, object: usize) -> io::Result<(i32, usize)> {
        if let Some(buffer) = self.off_screen(object) {
            return Ok(self.off_screen_info(buffer));
        }
        let screen = self.screen()?;
        let terminal = Self::window(&screen);
        let (buffer, window) = if self.active == buffers::MAIN {
            (
                self.state.buffer_size(terminal),
                self.state.window_size(terminal),
            )
        } else {
            (terminal, terminal)
        };
        let attributes = screen.pen.map_or(self.attributes, |pen| {
            legacy::attributes(&pen, &self.state.palette)
        });
        self.reply.fill(0);
        for (offset, (x, y)) in [
            (SIZE, buffer),
            (CURSOR, screen.cursor),
            (SCROLL, (0, 0)),
            (WINDOW, window),
            (MAXIMUM, buffer),
        ] {
            set_i16(&mut self.reply, offset, x);
            set_i16(&mut self.reply, offset + 2, y);
        }
        set_u16(&mut self.reply, ATTRIBUTES, attributes);
        set_u16(&mut self.reply, POPUP, self.state.popup_attributes);
        for (index, color) in self.state.palette.colors.into_iter().enumerate() {
            set_u32(&mut self.reply, COLORS + index * 4, color);
        }
        Ok((0, 0))
    }
    /// SetConsoleScreenBufferInfoExImpl as ConPTY runs it: the buffer size,
    /// color table and current attributes are kept, and none reach the
    /// terminal. The window cannot be resized from here.
    pub(super) fn set_screen_info(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let size = (i16_at(&self.reply, SIZE), i16_at(&self.reply, SIZE + 2));
        if size.0 == 0 || size.1 == 0 || size.0 == i16::MAX || size.1 == i16::MAX {
            return Ok((INVALID_PARAMETER, 0));
        }
        if let Some(buffer) = self.off_screen(object) {
            // The color table is the console's; the attributes the buffer's.
            for index in 0..16 {
                self.state.palette.colors[index] = u32_at(&self.reply, COLORS + index * 4);
            }
            let attributes = u16_at(&self.reply, ATTRIBUTES);
            self.set_off_screen_attributes(buffer, attributes);
            return Ok((0, 0));
        }
        let window = Self::window(&self.screen()?);
        // OpenConsole logs a failed resize here and carries on.
        let _ = self.state.set_buffer_size(window, size);
        for index in 0..16 {
            self.state.palette.colors[index] = u32_at(&self.reply, COLORS + index * 4);
        }
        self.attributes = u16_at(&self.reply, ATTRIBUTES);
        self.state.popup_attributes = u16_at(&self.reply, POPUP);
        if let Some(input) = &self.input {
            input.set_popup_sgr(legacy::sgr(
                self.state.popup_attributes,
                self.state.palette.attributes,
            ));
        }
        Ok((0, 0))
    }
    /// SetConsoleScreenBufferSizeImpl. The main buffer's size is reported,
    /// not given to the terminal, as ConPTY's is; a created buffer on screen
    /// is the terminal's size.
    pub(super) fn set_buffer_size(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let size = (i16_at(&self.reply, 0), i16_at(&self.reply, 2));
        if let Some(buffer) = self.off_screen(object) {
            return Ok((self.set_off_screen_size(buffer, size)?, 0));
        }
        let terminal = Self::window(&self.screen()?);
        let valid = if self.active == buffers::MAIN {
            self.state.set_buffer_size(terminal, size)
        } else {
            size.0 >= terminal.0 && size.1 >= terminal.1 && size.0 < i16::MAX && size.1 < i16::MAX
        };
        Ok(if valid {
            (0, 0)
        } else {
            (INVALID_PARAMETER, 0)
        })
    }
    /// SetConsoleWindowInfoImpl as ConPTY runs it: the buffer is resized to
    /// the window, and only an inverted rectangle is refused. A terminal pane
    /// cannot be resized by its client, so the main buffer's window and size
    /// are what is reported, as ConPTY's own buffer is.
    pub(super) fn set_window_info(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let absolute = self.reply[0] != 0;
        let [left, top, right, bottom] =
            [2, 4, 6, 8].map(|offset| i32::from(i16_at(&self.reply, offset)));
        let requested = crate::cells::Rect {
            left,
            top,
            right,
            bottom,
        };
        if let Some(buffer) = self.off_screen(object) {
            return Ok((self.set_off_screen_window(buffer, requested, absolute)?, 0));
        }
        let terminal = Self::window(&self.screen()?);
        let current = if self.active == buffers::MAIN {
            self.state.window_size(terminal)
        } else {
            terminal
        };
        let window = if absolute {
            requested
        } else {
            // Relative to the current window, whose origin is the buffer's.
            crate::cells::Rect {
                right: right + i32::from(current.0) - 1,
                bottom: bottom + i32::from(current.1) - 1,
                ..requested
            }
        };
        if !window.is_valid() {
            return Ok((INVALID_PARAMETER, 0));
        }
        if self.active == buffers::MAIN {
            let size = (window.width() as i16, window.height() as i16);
            self.state.set_window_size(terminal, size);
        }
        Ok((0, 0))
    }
    pub(super) fn get_largest_window(&mut self) -> io::Result<(i32, usize)> {
        let (columns, rows) = self.sink.size();
        set_i16(&mut self.reply, 0, columns);
        set_i16(&mut self.reply, 2, rows);
        Ok((0, 0))
    }
    pub(super) fn set_cursor_position(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let (x, y) = (i16_at(&self.reply, 0), i16_at(&self.reply, 2));
        if let Some(buffer) = self.off_screen(object) {
            return Ok((self.set_off_screen_cursor(buffer, x, y), 0));
        }
        let window = Self::window(&self.screen()?);
        let buffer = self.state.buffer_size(window);
        if x < 0 || y < 0 || x >= buffer.0 || y >= buffer.1 {
            return Ok((INVALID_PARAMETER, 0));
        }
        // The window shows the top of a taller buffer.
        let row = y.min(window.1 - 1);
        self.write_vt(&format!("\x1b[{};{}H", row + 1, x + 1))?;
        Ok((0, 0))
    }
    pub(super) fn get_cursor_info(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let (size, visible) = match self.off_screen(object) {
            Some(buffer) => self.off_screen_cursor_info(buffer),
            None => (self.state.cursor_size, self.screen()?.cursor_visible),
        };
        self.reply.fill(0);
        set_u32(&mut self.reply, 0, size);
        self.reply[4] = u8::from(visible);
        Ok((0, 0))
    }
    pub(super) fn set_cursor_info(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let size = u32_at(&self.reply, 0);
        if size == 0 || size > 100 {
            return Ok((INVALID_PARAMETER, 0));
        }
        if let Some(buffer) = self.off_screen(object) {
            let visible = self.reply[4] != 0;
            self.set_off_screen_cursor_info(buffer, size, visible);
            return Ok((0, 0));
        }
        self.state.cursor_size = size;
        self.write_vt(if self.reply[4] != 0 {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        })?;
        Ok((0, 0))
    }
    pub(super) fn set_text_attribute(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let attributes = u16_at(&self.reply, 0);
        if attributes & !legacy::VALID_ATTRIBUTES != 0 {
            return Ok((INVALID_PARAMETER, 0));
        }
        if let Some(buffer) = self.off_screen(object) {
            self.set_off_screen_attributes(buffer, attributes);
            return Ok((0, 0));
        }
        self.attributes = attributes;
        let sgr = legacy::sgr(attributes, self.state.palette.attributes);
        self.write_vt(&sgr)?;
        Ok((0, 0))
    }
}
