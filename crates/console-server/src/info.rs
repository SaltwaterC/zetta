// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Queries about a console window this host does not have: fonts, display
//! mode, selection and mouse buttons. The answers are the ones a ConPTY
//! console gives, measured on Windows 11, so that programs which ask behave
//! as they do under ConPTY. Also history settings and the process list.
use super::*;

/// ConPTY's font: Consolas at 16 rows, unscaled, with TMPF_TRUETYPE.
const FONT_HEIGHT: i16 = 16;
const FONT_FAMILY: u32 = 0x4;
const FONT_FACE: &str = "Consolas";

impl Server {
    /// CONSOLE_CURRENTFONT_MSG: MaximumWindow, FontIndex, FontSize,
    /// FontFamily, FontWeight, FaceName[32].
    pub(super) fn get_current_font(&mut self) -> (i32, usize) {
        self.reply.fill(0);
        set_i16(&mut self.reply, 10, FONT_HEIGHT);
        set_u32(&mut self.reply, 12, FONT_FAMILY);
        for (index, unit) in FONT_FACE.encode_utf16().enumerate() {
            set_u16(&mut self.reply, 20 + index * 2, unit);
        }
        (0, 0)
    }
    /// SetCurrentConsoleFontEx changes nothing a terminal would draw with.
    pub(super) fn set_current_font(&mut self) -> (i32, usize) {
        (0, 0)
    }
    /// Only font index 0 exists.
    pub(super) fn get_font_size(&mut self) -> (i32, usize) {
        let index = u32_at(&self.reply, 0);
        self.reply[4..8].fill(0);
        if index != 0 {
            return (INVALID_PARAMETER, 0);
        }
        set_i16(&mut self.reply, 6, FONT_HEIGHT);
        (0, 0)
    }
    pub(super) fn get_mouse_info(&mut self) -> (i32, usize) {
        set_u32(&mut self.reply, 0, 0);
        (0, 0)
    }
    pub(super) fn get_display_mode(&mut self) -> (i32, usize) {
        set_u32(&mut self.reply, 0, 0);
        (0, 0)
    }
    /// SetConsoleDisplayModeImpl: fullscreen (1) or windowed (2) is required;
    /// the reply is the buffer size, which does not change.
    pub(super) fn set_display_mode(&mut self) -> io::Result<(i32, usize)> {
        let flags = u32_at(&self.reply, 0);
        let screen = self.screen()?;
        let buffer = self.state.buffer_size((screen.columns, screen.rows));
        set_i16(&mut self.reply, 4, buffer.0);
        set_i16(&mut self.reply, 6, buffer.1);
        Ok(if flags & 0x3 == 0 {
            (INVALID_PARAMETER, 0)
        } else {
            (0, 0)
        })
    }
    /// The selection belongs to the terminal, so the console has none.
    pub(super) fn get_selection_info(&mut self) -> (i32, usize) {
        self.reply.fill(0);
        (0, 0)
    }
    /// CONSOLE_HISTORY_MSG: HistoryBufferSize, NumberOfHistoryBuffers, dwFlags.
    pub(super) fn get_history(&mut self) -> (i32, usize) {
        let editing = crate::editing::lock(&self.editing);
        let (size, buffers, flags) = (
            editing.histories.size,
            editing.histories.buffers,
            editing.histories.flags,
        );
        drop(editing);
        set_u32(&mut self.reply, 0, size);
        set_u32(&mut self.reply, 4, buffers);
        set_u32(&mut self.reply, 8, flags);
        (0, 0)
    }
    /// SetConsoleHistoryInfoImpl: sizes up to SHORT_MAX, and HISTORY_NO_DUP
    /// is the only flag. Every list is resized at once.
    pub(super) fn set_history(&mut self) -> (i32, usize) {
        let size = u32_at(&self.reply, 0);
        let buffers = u32_at(&self.reply, 4);
        let flags = u32_at(&self.reply, 8);
        let limit = i16::MAX as u32;
        if size > limit || buffers > limit || flags & !crate::history::NO_DUPLICATES != 0 {
            return (INVALID_PARAMETER, 0);
        }
        let mut editing = crate::editing::lock(&self.editing);
        editing.histories.resize_all(size);
        editing.histories.buffers = buffers;
        editing.histories.flags = flags;
        (0, 0)
    }
    pub(super) fn get_lang_id(&mut self) -> (i32, usize) {
        set_u16(&mut self.reply, 0, 0x0409);
        (0, 0)
    }
    /// The count always; the pids only when they fit after the header.
    pub(super) fn process_list(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        offset: u32,
    ) -> io::Result<(i32, usize)> {
        let count = self.processes.len();
        set_u32(&mut self.reply, 0, count as u32);
        if (d.output_size - offset) as usize / 4 < count {
            return Ok((0, 0));
        }
        let mut pids: Vec<u8> = self
            .processes
            .values()
            .flat_map(|client| client.pid.to_le_bytes())
            .collect();
        native::transfer(handle, WRITE_OUTPUT, d.id, offset, &mut pids)?;
        Ok((0, pids.len()))
    }
}
