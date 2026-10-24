// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Screen buffers and the cell-level output APIs.
//!
//! The active buffer is the embedder's grid: cells are read back from it
//! after a flush and written as VT inside DECSC/DECRC, as OpenConsole's VtIo
//! writer does for ConPTY. CreateConsoleScreenBuffer buffers are kept as
//! cells here; activating one puts the terminal on its alternate screen and
//! redraws it there (VtIo::Writer::WriteScreenInfo), and leaving it records
//! what the grid showed so it can be redrawn later.
//!
//! The main buffer is kept the same way while another is on screen: its
//! cells are read from the grid as it leaves, so it can be read and written
//! like any other buffer, and what the terminal kept of it is redrawn when
//! it comes back only if a cell write changed it.
//!
//! The active buffer's mode, cursor size and attributes live in the server's
//! own fields, so writing to it is the plain output path; an inactive
//! buffer keeps them in its `Saved`.
use super::*;
use crate::budget::Charged;
use crate::cells::{self, CharInfo, Fill, Rect};
use crate::legacy;
use crate::output::Cell;
pub use crate::screen_buffer::{Saved, ScreenBuffer};
use crate::state::mode;

pub const MAIN: u32 = 0;
const SAVE_CURSOR: &str = "\x1b7";
const RESTORE_CURSOR: &str = "\x1b8";
const ERROR_INVALID_FUNCTION: i32 = 0xc007_0001_u32 as i32;
/// NTSTATUS_FROM_HRESULT(E_UNEXPECTED).
const E_UNEXPECTED: i32 = 0xc007_ffff_u32 as i32;
/// How much text the main buffer keeps to replay while it is off screen.
/// Past it, the main buffer is redrawn from its cells when it comes back,
/// as WriteScreenInfo always redraws it.
const REPLAY_LIMIT: usize = 16 << 20;

/// Text written to a buffer that is not on screen.
pub enum Deferred {
    Bytes(Vec<u8>),
    Wide(Vec<u16>),
}

impl Server {
    pub(super) fn output_buffer(&self, object: usize) -> Option<u32> {
        match self.objects.get(&object) {
            Some(Object::Output(buffer)) => Some(*buffer),
            _ => None,
        }
    }
    fn is_active(&self, buffer: u32) -> bool {
        buffer == self.active
    }
    /// The active buffer's dimensions: the window, or what an application
    /// set as the main buffer's size.
    fn active_size(&mut self) -> io::Result<(i16, i16)> {
        let screen = self.screen()?;
        let window = (screen.columns, screen.rows);
        Ok(if self.active == MAIN {
            self.state.buffer_size(window)
        } else {
            window
        })
    }
    pub(super) fn buffer_size(&mut self, buffer: u32) -> io::Result<(i16, i16)> {
        if self.is_active(buffer) {
            return self.active_size();
        }
        match self.buffers.get(&buffer) {
            Some(screen) => Ok(screen.size),
            None => {
                let (columns, rows) = self.sink.size();
                Ok(self.state.buffer_size((columns, rows)))
            }
        }
    }

    /// CD_IO_OBJECT_TYPE_NEW_OUTPUT: a text buffer the size of the active
    /// one, blank in the current attributes.
    pub(super) fn create_screen_buffer(&mut self, access: Access) -> io::Result<usize> {
        let size = self.active_size()?;
        let saved = Saved {
            output_mode: mode::PROCESSED_OUTPUT
                | mode::WRAP_AT_EOL_OUTPUT
                | mode::VIRTUAL_TERMINAL_PROCESSING,
            cursor_size: 25,
            attributes: self.attributes,
            cursor_visible: true,
            cursor: (0, 0),
        };
        let blank = CharInfo::blank(self.attributes);
        let screen = ScreenBuffer::new(&self.budget, size, size, blank, saved)?;
        let id = self.next_buffer;
        self.next_buffer += 1;
        self.buffers.insert(id, screen);
        Ok(self.object(Object::Output(id), access))
    }
    pub(super) fn output_opened(&mut self, buffer: u32) {
        if buffer == MAIN {
            return;
        }
        if let Some(screen) = self.buffers.get_mut(&buffer) {
            screen.handles += 1;
        }
    }
    /// The last handle to a created buffer frees it; if it was on screen the
    /// main buffer comes back first.
    pub(super) fn output_closed(&mut self, buffer: u32) -> io::Result<()> {
        if buffer == MAIN {
            return Ok(());
        }
        let Some(screen) = self.buffers.get_mut(&buffer) else {
            return Ok(());
        };
        screen.handles = screen.handles.saturating_sub(1);
        if screen.handles > 0 {
            return Ok(());
        }
        if self.active == buffer {
            self.activate(MAIN)?;
        }
        self.buffers.remove(&buffer);
        Ok(())
    }

    pub(super) fn set_active_screen_buffer(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let Some(buffer) = self.output_buffer(object) else {
            return Ok((INVALID_HANDLE, 0));
        };
        self.activate(buffer)?;
        Ok((0, 0))
    }

    /// SetConsoleActiveScreenBufferImpl through WriteScreenInfo.
    fn activate(&mut self, target: u32) -> io::Result<()> {
        if target == self.active {
            return Ok(());
        }
        let screen = self.screen()?;
        let leaving = Saved {
            output_mode: self.state.output_mode,
            cursor_size: self.state.cursor_size,
            attributes: self.attributes,
            cursor_visible: screen.cursor_visible,
            cursor: screen.cursor,
        };
        if self.active == MAIN {
            let terminal = (screen.columns, screen.rows);
            let size = self.state.buffer_size(terminal);
            let window = self.state.window_size(terminal);
            let main = self.read_screen(size, window, terminal, leaving)?;
            self.buffers.insert(MAIN, main);
        } else {
            // Record what the alternate screen shows, to redraw it later.
            let size = (screen.columns, screen.rows);
            let mut shown = self.read_screen(size, size, size, leaving)?;
            if let Some(buffer) = self.buffers.get_mut(&self.active) {
                (shown.handles, shown.dirty) = (buffer.handles, buffer.dirty);
                *buffer = shown;
            }
        }
        let previous = self.active;
        self.active = target;
        let main = if target == MAIN {
            self.buffers.remove(&MAIN)
        } else {
            None
        };
        let incoming = match &main {
            Some(main) => Some(main.saved),
            None => self.buffers.get(&target).map(|buffer| buffer.saved),
        };
        if let Some(saved) = incoming {
            self.state.output_mode = saved.output_mode;
            self.state.cursor_size = saved.cursor_size;
            self.attributes = saved.attributes;
        }
        let mut out = String::new();
        if target == MAIN {
            // The terminal restores the main screen and its cursor itself.
            out.push_str("\x1b[?1049l");
            if let Some(main) = &main {
                let terminal = (screen.columns, screen.rows);
                self.state.restore_view(terminal, main.window, main.size);
                if main.dirty {
                    main.draw(
                        &mut out,
                        terminal,
                        terminal.0,
                        self.state.palette.attributes,
                    );
                    let (x, y) = main.saved.cursor;
                    let y = y.min(terminal.1 - 1);
                    out.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
                    // The cells already hold what was written as text.
                    self.deferred_main.clear();
                    self.deferred_main_charge.shrink(usize::MAX);
                }
            }
        } else {
            if previous == MAIN {
                out.push_str("\x1b[?1049h");
            }
            if let Some(buffer) = self.buffers.get(&target) {
                let terminal = (screen.columns, screen.rows);
                let row_width = buffer.size.0.min(terminal.0);
                buffer.draw(&mut out, terminal, row_width, self.state.palette.attributes);
                let (x, y) = buffer.saved.cursor;
                out.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
            }
        }
        if let Some(saved) = incoming {
            out.push_str(&legacy::sgr(
                saved.attributes,
                self.state.palette.attributes,
            ));
            out.push_str(if saved.cursor_visible {
                "\x1b[?25h"
            } else {
                "\x1b[?25l"
            });
            out.push_str(if saved.output_mode & mode::WRAP_AT_EOL_OUTPUT != 0 {
                "\x1b[?7h"
            } else {
                "\x1b[?7l"
            });
        }
        self.write_vt(&out)?;
        if target == MAIN {
            self.deferred_main_charge.shrink(usize::MAX);
            for deferred in std::mem::take(&mut self.deferred_main) {
                match deferred {
                    Deferred::Bytes(bytes) => self.write_bytes(&bytes, 0)?,
                    Deferred::Wide(wide) => self.write_wide(&wide, 0)?,
                }
            }
        }
        Ok(())
    }

    /// Text for a buffer that is not on screen: kept for the main buffer,
    /// drawn into a created one's cells.
    /// The main buffer also keeps the text itself, to replay it exactly when
    /// nothing else changed it.
    pub(super) fn write_off_screen(&mut self, buffer: u32, deferred: Deferred) -> io::Result<()> {
        let code_page = self.state.output_cp;
        let Some(screen) = self.buffers.get_mut(&buffer) else {
            return Ok(());
        };
        let text = match &deferred {
            Deferred::Bytes(bytes) => {
                String::from_utf16_lossy(&codepage::to_utf16(code_page, bytes))
            }
            Deferred::Wide(wide) => String::from_utf16_lossy(wide),
        };
        screen.write_text(&text)?;
        // A dirty main buffer is redrawn from its cells, not replayed.
        if buffer == MAIN && !screen.dirty {
            let bytes = match &deferred {
                Deferred::Bytes(bytes) => bytes.len(),
                Deferred::Wide(wide) => wide.len() * 2,
            };
            let charge = &mut self.deferred_main_charge;
            if charge.bytes() + bytes > REPLAY_LIMIT || charge.grow(bytes).is_err() {
                screen.dirty = true;
                self.deferred_main.clear();
                charge.shrink(usize::MAX);
            } else {
                self.deferred_main.push(deferred);
            }
        }
        Ok(())
    }

    /// The grid as a buffer of `size`. Only what the terminal has is read:
    /// the grid answers a blank for the rest, and a client can size the main
    /// buffer at 32766x32766.
    fn read_screen(
        &mut self,
        size: (i16, i16),
        window: (i16, i16),
        terminal: (i16, i16),
        saved: Saved,
    ) -> io::Result<ScreenBuffer> {
        self.flush_output()?;
        let palette = self.state.palette;
        let (rows, columns) = (size.1.min(terminal.1).max(0), size.0.min(terminal.0).max(0));
        let Some(read) = self.sink.read_cells(0..rows, 0..columns) else {
            let blank = CharInfo::blank(self.attributes);
            return ScreenBuffer::new(&self.budget, size, window, blank, saved);
        };
        let blank = cells::char_info(&Cell::BLANK, &palette);
        let mut screen = ScreenBuffer::new(&self.budget, size, window, blank, saved)?;
        let mut row = Vec::with_capacity(columns as usize);
        for (y, cells) in read.chunks(columns.max(1) as usize).enumerate() {
            row.clear();
            row.extend(cells.iter().map(|cell| cells::char_info(cell, &palette)));
            screen.write_row(0, y as i32, &row)?;
        }
        Ok(screen)
    }

    /// Cells of `rect` (clamped by the caller), row by row.
    fn read_rect(&mut self, buffer: u32, rect: Rect) -> io::Result<Option<Charged<CharInfo>>> {
        if !rect.is_valid() {
            return self.budget.with_capacity(0).map(Some);
        }
        if self.is_active(buffer) {
            return self.read_terminal(rect);
        }
        let Some(screen) = self.buffers.get(&buffer) else {
            return Ok(None);
        };
        let mut out = self
            .budget
            .with_capacity((rect.width() * rect.height()) as usize)?;
        for y in rect.top..=rect.bottom {
            for x in rect.left..=rect.right {
                out.push(
                    screen
                        .cell(x, y)
                        .unwrap_or(CharInfo::blank(screen.saved.attributes)),
                );
            }
        }
        Ok(Some(out))
    }
    /// The active buffer's cells of `rect`. Only what the terminal has is
    /// read from the grid, which answers a blank for the rest, so a main
    /// buffer a client made larger than the terminal costs what is asked of
    /// it rather than sixteen bytes a cell more.
    fn read_terminal(&mut self, rect: Rect) -> io::Result<Option<Charged<CharInfo>>> {
        let palette = self.state.palette;
        let (columns, rows) = self.sink.size();
        let on_screen = rect.clamp(&Rect::from_size(i32::from(columns), i32::from(rows)));
        let Some(read) = self.sink.read_cells(
            on_screen.top as i16..(on_screen.bottom + 1) as i16,
            on_screen.left as i16..(on_screen.right + 1) as i16,
        ) else {
            return Ok(None);
        };
        let mut read = read.iter().map(|cell| cells::char_info(cell, &palette));
        let blank = cells::char_info(&Cell::BLANK, &palette);
        let mut out = self
            .budget
            .with_capacity((rect.width() * rect.height()) as usize)?;
        for y in rect.top..=rect.bottom {
            for x in rect.left..=rect.right {
                let inside = (on_screen.left..=on_screen.right).contains(&x)
                    && (on_screen.top..=on_screen.bottom).contains(&y);
                out.push(if inside { read.next() } else { None }.unwrap_or(blank));
            }
        }
        Ok(Some(out))
    }

    /// Writes `cells` (row-major, `rect`'s width) into `rect`. On screen this
    /// appends VT to `out`, for the cells the terminal has: VT for the others
    /// would land on its last row or wrap. Off screen it updates the
    /// buffer's cells.
    fn write_rect(
        &mut self,
        buffer: u32,
        rect: Rect,
        cells_in: &[CharInfo],
        out: &mut String,
    ) -> io::Result<()> {
        if !rect.is_valid() {
            return Ok(());
        }
        let width = rect.width() as usize;
        if self.is_active(buffer) {
            let defaults = self.state.palette.attributes;
            let (row_width, rows) = self.sink.size();
            let columns = (i32::from(row_width) - rect.left).max(0) as usize;
            for (row, cells) in cells_in.chunks(width).enumerate() {
                let y = rect.top + row as i32;
                if y >= i32::from(rows) || columns == 0 {
                    break;
                }
                cells::write_infos(
                    out,
                    (rect.left as i16, y as i16),
                    &cells[..cells.len().min(columns)],
                    defaults,
                    row_width,
                );
            }
            return Ok(());
        }
        if let Some(screen) = self.buffers.get_mut(&buffer) {
            screen.dirty = true;
            for (row, cells) in cells_in.chunks(width).enumerate() {
                screen.write_row(rect.left, rect.top + row as i32, cells)?;
            }
        }
        Ok(())
    }

    /// Wraps VT for an on-screen change in DECSC/DECRC and writes it.
    fn submit(&mut self, out: String) -> io::Result<()> {
        if out.is_empty() {
            return Ok(());
        }
        self.write_vt(&format!("{SAVE_CURSOR}{out}{RESTORE_CURSOR}"))
    }

    /// FillConsoleImpl over consecutive rows from `start`; returns how much
    /// input was used and how many cells changed.
    fn fill(
        &mut self,
        buffer: u32,
        start: (i32, i32),
        fill: Fill<'_>,
        length: usize,
    ) -> io::Result<Option<(usize, usize)>> {
        let (width, height) = self.buffer_size(buffer)?;
        let (width, height) = (i32::from(width), i32::from(height));
        if start.0 < 0 || start.1 < 0 || start.0 >= width || start.1 >= height || length == 0 {
            return Ok(Some((0, 0)));
        }
        if self.is_active(buffer) {
            self.flush_output()?;
        }
        let length = fill.len(length);
        let (mut position, mut modified) = (0, 0);
        let mut out = String::new();
        let mut y = start.1;
        while y < height && position < length {
            let begin = if y == start.1 { start.0 } else { 0 };
            let row_rect = Rect {
                left: begin,
                top: y,
                right: width - 1,
                bottom: y,
            };
            let Some(mut row) = self.read_rect(buffer, row_rect)? else {
                return Ok(None);
            };
            let columns = cells::fill_row(&mut row, fill, &mut position, length);
            let written = Rect {
                right: begin + columns as i32 - 1,
                ..row_rect
            };
            self.write_rect(buffer, written, &row[..columns], &mut out)?;
            modified += columns;
            y += 1;
        }
        self.submit(out)?;
        Ok(Some((position, modified)))
    }

    fn client_is(&self, process: usize, names: &[&str]) -> bool {
        self.processes.get(&process).is_some_and(|client| {
            names
                .iter()
                .any(|name| client.application.eq_ignore_ascii_case(name))
        })
    }

    /// CONSOLE_FILLCONSOLEOUTPUT_MSG: WriteCoord, ElementType, Element,
    /// Length (in: cells to fill, out: how many were).
    pub(super) fn fill_output(&mut self, d: Descriptor) -> io::Result<(i32, usize)> {
        let Some(buffer) = self.output_buffer(d.object) else {
            return Ok((INVALID_HANDLE, 0));
        };
        let start = (
            i32::from(i16_at(&self.reply, 0)),
            i32::from(i16_at(&self.reply, 2)),
        );
        let kind = u32_at(&self.reply, 4);
        let element = u16_at(&self.reply, 8);
        let length = u32_at(&self.reply, 12) as usize;
        let fill = match kind {
            3 => Fill::Attribute(element),
            1 => {
                // FillConsoleOutputCharacterAImpl takes the first character
                // the byte converts to; a byte that converts to none (a
                // stateful code page's shift) fails as E_UNEXPECTED.
                let wide = codepage::to_utf16(self.state.output_cp, &[element as u8]);
                let Some(&ch) = wide.first() else {
                    return Ok((E_UNEXPECTED, 0));
                };
                Fill::Character(ch)
            }
            2 | 4 => Fill::Character(element),
            _ => return Ok((INVALID_PARAMETER, 0)),
        };
        // PowerShell's Clear-Host fills the whole buffer with spaces from the
        // origin; ConPTY clears the terminal's scrollback too (GH#3126).
        if matches!(fill, Fill::Character(0x20))
            && start == (0, 0)
            && self.client_is(d.process, &["powershell.exe", "pwsh.exe"])
        {
            let (width, height) = self.buffer_size(buffer)?;
            if length == (i32::from(width) * i32::from(height)) as usize && self.is_active(buffer) {
                self.write_vt("\x1b[H\x1b[2J\x1b[3J")?;
                set_u32(&mut self.reply, 12, length as u32);
                return Ok((0, 0));
            }
        }
        let Some((read, _)) = self.fill(buffer, start, fill, length)? else {
            return Ok((ERROR_INVALID_FUNCTION, 0));
        };
        set_u32(&mut self.reply, 12, read as u32);
        Ok((0, 0))
    }

    /// CONSOLE_WRITECONSOLEOUTPUTSTRING_MSG: WriteCoord, StringType,
    /// NumRecords (out); the characters or attributes follow in the input.
    pub(super) fn write_output_string(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let Some(buffer) = self.output_buffer(d.object) else {
            return Ok((INVALID_HANDLE, 0));
        };
        let start = (
            i32::from(i16_at(&self.reply, 0)),
            i32::from(i16_at(&self.reply, 2)),
        );
        let kind = u32_at(&self.reply, 4);
        let input = self.api_input(handle, d, size)?;
        let units: Vec<u16> = match kind {
            1 => codepage::to_utf16(self.state.output_cp, &input),
            2..=4 => input
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect(),
            _ => return Ok((INVALID_PARAMETER, 0)),
        };
        let fill = if kind == 3 {
            Fill::Attributes(&units)
        } else {
            Fill::Characters(&units)
        };
        let Some((read, modified)) = self.fill(buffer, start, fill, units.len())? else {
            return Ok((ERROR_INVALID_FUNCTION, 0));
        };
        set_u32(
            &mut self.reply,
            8,
            if kind == 3 { modified } else { read } as u32,
        );
        Ok((0, 0))
    }

    /// CONSOLE_READCONSOLEOUTPUTSTRING_MSG: ReadCoord, StringType,
    /// NumRecords (out); the result goes after the message.
    pub(super) fn read_output_string(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let Some(buffer) = self.output_buffer(d.object) else {
            return Ok((INVALID_HANDLE, 0));
        };
        let start = (
            i32::from(i16_at(&self.reply, 0)),
            i32::from(i16_at(&self.reply, 2)),
        );
        let kind = u32_at(&self.reply, 4);
        let capacity = (d.output_size as usize).saturating_sub(size);
        let wanted = if kind == 1 { capacity } else { capacity / 2 };
        let (width, height) = self.buffer_size(buffer)?;
        let (width, height) = (i32::from(width), i32::from(height));
        set_u32(&mut self.reply, 8, 0);
        if start.0 < 0
            || start.1 < 0
            || start.0 >= width
            || start.1 >= height
            || !(1..=3).contains(&kind)
        {
            return Ok((0, 0));
        }
        if self.is_active(buffer) {
            self.flush_output()?;
        }
        let remaining = (height - start.1) as usize * width as usize - start.0 as usize;
        let mut cells = self.budget.with_capacity(wanted.min(remaining))?;
        let mut y = start.1;
        while y < height && cells.len() < wanted {
            let begin = if y == start.1 { start.0 } else { 0 };
            let rect = Rect {
                left: begin,
                top: y,
                right: width - 1,
                bottom: y,
            };
            let Some(row) = self.read_rect(buffer, rect)? else {
                return Ok((ERROR_INVALID_FUNCTION, 0));
            };
            let more = wanted - cells.len();
            cells.extend(row.iter().copied().take(more));
            y += 1;
        }
        let mut units: Vec<u16> = match kind {
            3 => cells.iter().map(|cell| cell.attributes).collect(),
            _ => characters(&cells),
        };
        let mut bytes = if kind == 1 {
            let mut bytes = codepage::from_utf16(self.state.output_cp, &units);
            bytes.truncate(wanted);
            bytes
        } else {
            units.truncate(wanted);
            units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
        };
        let count = if kind == 1 {
            bytes.len()
        } else {
            bytes.len() / 2
        };
        set_u32(&mut self.reply, 8, count as u32);
        self.reply_output(handle, d, size, &mut bytes)
    }

    /// The request rectangle of Read/WriteConsoleOutput clipped to the
    /// buffer, and the offset of its first cell in the caller's array.
    fn clip_region(&mut self, buffer: u32) -> io::Result<(Rect, Rect)> {
        let request = Rect {
            left: i32::from(i16_at(&self.reply, 0)),
            top: i32::from(i16_at(&self.reply, 2)),
            right: i32::from(i16_at(&self.reply, 4)),
            bottom: i32::from(i16_at(&self.reply, 6)),
        };
        let (width, height) = self.buffer_size(buffer)?;
        let clipped = request.clamp(&Rect::from_size(i32::from(width), i32::from(height)));
        Ok((request, clipped))
    }
    fn set_region(&mut self, rect: Rect) {
        for (offset, value) in [
            (0, rect.left),
            (2, rect.top),
            (4, rect.right),
            (6, rect.bottom),
        ] {
            set_i16(&mut self.reply, offset, value as i16);
        }
    }

    /// WriteConsoleOutput: CHAR_INFOs for the request rectangle; what fits in
    /// the buffer is written and the written rectangle returned.
    pub(super) fn write_output(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let Some(buffer) = self.output_buffer(d.object) else {
            return Ok((INVALID_HANDLE, 0));
        };
        let unicode = self.reply[8] != 0;
        let (request, clipped) = self.clip_region(buffer)?;
        let input = self.api_input(handle, d, size)?;
        let area = (request.width() * request.height()) as usize;
        if !request.is_valid() || input.len() / 4 < area {
            return Ok((INVALID_PARAMETER, 0));
        }
        let mut all = self.budget.with_capacity(area)?;
        all.extend(
            input.as_chunks::<4>().0[..area]
                .iter()
                .map(|cell| CharInfo::from_bytes(cell)),
        );
        if !unicode {
            self.cells_from_ansi(&mut all, request.width() as usize);
        }
        if clipped.is_valid() {
            let stride = request.width() as usize;
            let mut cells = self
                .budget
                .with_capacity((clipped.width() * clipped.height()) as usize)?;
            for y in clipped.top..=clipped.bottom {
                let row = (y - request.top) as usize * stride;
                let from = row + (clipped.left - request.left) as usize;
                cells.extend_from_slice(&all[from..from + clipped.width() as usize]);
            }
            let mut out = String::new();
            self.write_rect(buffer, clipped, &cells, &mut out)?;
            self.submit(out)?;
        }
        self.set_region(clipped);
        Ok((0, 0))
    }

    /// ReadConsoleOutput: the clipped rectangle's cells, placed where they
    /// belong in the caller's array for the request rectangle.
    pub(super) fn read_output(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let Some(buffer) = self.output_buffer(d.object) else {
            return Ok((INVALID_HANDLE, 0));
        };
        let unicode = self.reply[8] != 0;
        let (request, clipped) = self.clip_region(buffer)?;
        if !request.is_valid() {
            return Ok((INVALID_PARAMETER, 0));
        }
        let stride = request.width() as usize;
        let area = stride * request.height() as usize;
        if (d.output_size as usize).saturating_sub(size) < area * 4 {
            return Ok((INVALID_PARAMETER, 0));
        }
        let mut all = self.budget.vec(CharInfo::default(), area)?;
        if clipped.is_valid() {
            if self.is_active(buffer) {
                self.flush_output()?;
            }
            let Some(cells) = self.read_rect(buffer, clipped)? else {
                return Ok((ERROR_INVALID_FUNCTION, 0));
            };
            for (row, chunk) in cells.chunks(clipped.width() as usize).enumerate() {
                let at = (clipped.top - request.top) as usize * stride
                    + row * stride
                    + (clipped.left - request.left) as usize;
                all[at..at + chunk.len()].copy_from_slice(chunk);
            }
        }
        if !unicode {
            self.cells_to_ansi(&mut all);
        }
        self.set_region(clipped);
        let mut bytes = self.budget.with_capacity(area * 4)?;
        bytes.extend(all.iter().flat_map(|cell| cell.to_bytes()));
        self.reply_output(handle, d, size, &mut bytes)
    }

    /// _ConvertCellsToWInplace: the caller's lead/trail flags are ignored and
    /// worked out again from the code page. A lead byte and the cell after it
    /// in the same row become the character in both cells; a lead byte in a
    /// row's last column is a space; other bytes convert on their own.
    fn cells_from_ansi(&self, cells: &mut [CharInfo], width: usize) {
        const DBCS: u16 = legacy::LEADING_BYTE | legacy::TRAILING_BYTE;
        let code_page = self.state.output_cp;
        let convert = |bytes: &[u8]| {
            codepage::to_utf16(code_page, bytes)
                .first()
                .copied()
                .unwrap_or(u16::from(b' '))
        };
        for row in cells.chunks_mut(width.max(1)) {
            let mut index = 0;
            while index < row.len() {
                row[index].attributes &= !DBCS;
                let byte = row[index].ch as u8;
                if !codepage::is_lead_byte(code_page, byte) {
                    row[index].ch = convert(&[byte]);
                    index += 1;
                } else if index + 1 < row.len() {
                    let ch = convert(&[byte, row[index + 1].ch as u8]);
                    row[index].ch = ch;
                    row[index].attributes |= legacy::LEADING_BYTE;
                    row[index + 1].ch = ch;
                    row[index + 1].attributes =
                        row[index + 1].attributes & !DBCS | legacy::TRAILING_BYTE;
                    index += 2;
                } else {
                    row[index].ch = u16::from(b' ');
                    index += 1;
                }
            }
        }
    }
    /// _ConvertCellsToAInplace: a wide character's two cells carry its two
    /// code page bytes; anything else its single byte.
    fn cells_to_ansi(&self, cells: &mut [CharInfo]) {
        let code_page = self.state.output_cp;
        for cell in cells.iter_mut() {
            let bytes = codepage::from_utf16(code_page, &[cell.ch]);
            let byte = if cell.attributes & legacy::TRAILING_BYTE != 0 {
                bytes.get(1).or(bytes.first())
            } else {
                bytes.first()
            };
            cell.ch = u16::from(byte.copied().unwrap_or(b'?'));
        }
    }

    /// CONSOLE_SCROLLSCREENBUFFER_MSG: ScrollRectangle, ClipRectangle, Clip,
    /// Unicode, DestinationOrigin, Fill.
    pub(super) fn scroll(&mut self, d: Descriptor) -> io::Result<(i32, usize)> {
        let Some(buffer) = self.output_buffer(d.object) else {
            return Ok((INVALID_HANDLE, 0));
        };
        let rect = |offset: usize| Rect {
            left: i32::from(i16_at(&self.reply, offset)),
            top: i32::from(i16_at(&self.reply, offset + 2)),
            right: i32::from(i16_at(&self.reply, offset + 4)),
            bottom: i32::from(i16_at(&self.reply, offset + 6)),
        };
        let source = rect(0);
        let clip = (self.reply[16] != 0).then(|| rect(8));
        let unicode = self.reply[17] != 0;
        let target = (
            i32::from(i16_at(&self.reply, 18)),
            i32::from(i16_at(&self.reply, 20)),
        );
        let mut fill = CharInfo {
            ch: u16_at(&self.reply, 22),
            attributes: u16_at(&self.reply, 24),
        };
        if !unicode {
            fill.ch = codepage::to_utf16(self.state.output_cp, &[fill.ch as u8])
                .first()
                .copied()
                .unwrap_or(u16::from(b' '));
        }
        let cmd = self.client_is(d.process, &["cmd.exe"]);
        self.scroll_region(buffer, source, clip, target, fill, cmd)?;
        Ok((0, 0))
    }

    /// ScrollConsoleScreenBufferWImpl's VT strategies, without rectangular
    /// area operations: cmd's whole-buffer `cls` scroll clears the screen,
    /// a full-width vertical scroll of everything is CSI M/L, and anything
    /// else is read back, filled and rewritten.
    fn scroll_region(
        &mut self,
        buffer: u32,
        source: Rect,
        clip: Option<Rect>,
        target: (i32, i32),
        mut fill: CharInfo,
        cmd: bool,
    ) -> io::Result<()> {
        if (source.left == target.0 && source.top == target.1) || !source.is_valid() {
            return Ok(());
        }
        let (width, height) = self.buffer_size(buffer)?;
        let bounds = Rect::from_size(i32::from(width), i32::from(height));
        let clip_rect = clip.map_or(bounds, |clip| clip.clamp(&bounds));
        let fill_rect = source.clamp(&clip_rect);
        let distance = (target.0 - source.left, target.1 - source.top);
        let clip_at_source = clip_rect.offset(-distance.0, -distance.1);
        let copy_source = source.clamp(&bounds).clamp(&clip_at_source);
        let copy_target = copy_source.offset(distance.0, distance.1);
        if !fill_rect.is_valid() && !copy_target.is_valid() {
            return Ok(());
        }
        let current = if self.is_active(buffer) {
            let screen = self.screen()?;
            screen.pen.map_or(self.attributes, |pen| {
                legacy::attributes(&pen, &self.state.palette)
            })
        } else {
            self.attributes
        };
        if fill.ch == 0 && fill.attributes == 0 {
            fill.attributes = current;
        }
        fill.ch = legacy::sanitize(fill.ch) as u16;
        let active = self.is_active(buffer);
        if active
            && cmd
            && source.left <= 0
            && source.top <= 0
            && source.right >= bounds.right
            && source.bottom >= bounds.bottom
            && target.0 == 0
            && target.1 <= -i32::from(height)
            && clip.is_none()
            && fill.ch == u16::from(b' ')
            && fill.attributes == current
        {
            return self.write_vt("\x1b[H\x1b[2J\x1b[3J");
        }
        if active
            && source == bounds
            && clip_rect == bounds
            && distance.0 == 0
            && fill.ch == u16::from(b' ')
            && fill.attributes & !0xff == 0
        {
            let lines = distance.1.abs().min(i32::from(height));
            let operation = if distance.1 < 0 { 'M' } else { 'L' };
            let sgr = legacy::sgr(fill.attributes, self.state.palette.attributes);
            return self.write_vt(&format!(
                "{SAVE_CURSOR}{sgr}\x1b[H\x1b[{lines}{operation}{RESTORE_CURSOR}"
            ));
        }
        if !active {
            if let Some(screen) = self.buffers.get_mut(&buffer) {
                screen.scroll(fill_rect, copy_source, copy_target, fill)?;
            }
            return Ok(());
        }
        self.flush_output()?;
        // Only the terminal's cells are drawn, so only they are moved.
        let (columns, rows) = self.sink.size();
        let screen = Rect::from_size(i32::from(columns), i32::from(rows));
        let (fill_rect, copy_target) = (fill_rect.clamp(&screen), copy_target.clamp(&screen));
        let copy_source = copy_target.offset(-distance.0, -distance.1);
        let backup = if copy_source.is_valid() {
            self.read_rect(buffer, copy_source)?
        } else {
            None
        };
        let mut out = String::new();
        if fill_rect.is_valid() {
            let cells = self
                .budget
                .vec(fill, (fill_rect.width() * fill_rect.height()) as usize)?;
            self.write_rect(buffer, fill_rect, &cells, &mut out)?;
        }
        if let Some(backup) = &backup
            && copy_target.is_valid()
            && backup.len() == (copy_source.width() * copy_source.height()) as usize
        {
            self.write_rect(buffer, copy_target, backup, &mut out)?;
        }
        self.submit(out)
    }

    /// CONSOLE_WRITECONSOLEINPUT_MSG: NumRecords (out), Unicode, Append; the
    /// records follow. Prepending puts them before what is queued.
    pub(super) fn write_console_input(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[4] != 0;
        let append = self.reply[5] != 0;
        let input = self.api_input(handle, d, size)?;
        let records = input.as_chunks::<20>().0;
        if records.is_empty() {
            set_u32(&mut self.reply, 0, 0);
            return Ok((0, 0));
        }
        let widened;
        let records = if unicode {
            records
        } else {
            widened = self.widen_input_records(records)?;
            &widened[..]
        };
        let Some(input_queue) = &self.input else {
            return Ok((NOT_SUPPORTED, 0));
        };
        let written = input_queue.inject(records, append);
        set_u32(&mut self.reply, 0, written as u32);
        Ok((0, 0))
    }

    /// WriteConsoleInputAImpl: a key's AsciiChar is in the input code page,
    /// and a DBCS lead byte is joined with the key after it. A lead byte that
    /// ends the call waits for the next call's first record, and a pair whose
    /// trail is not a key is dropped.
    fn widen_input_records(&mut self, records: &[[u8; 20]]) -> io::Result<Charged<[u8; 20]>> {
        let code_page = self.state.input_cp;
        let is_key = |record: &[u8; 20]| record[..2] == [1, 0];
        // One record per UTF-16 unit, each a copy of the lead's.
        let push = |out: &mut Vec<[u8; 20]>, lead: &[u8; 20], narrow: &[u8]| {
            for unit in codepage::to_utf16(code_page, narrow) {
                let mut record = *lead;
                record[14..16].copy_from_slice(&unit.to_le_bytes());
                out.push(record);
            }
        };
        // A record gives one unit at most, and a lead byte left by the last
        // call gives one more with its trail.
        let mut out = self.budget.with_capacity(records.len() + 1)?;
        let mut records = records.iter();
        if let Some(lead) = self.input_lead.take()
            && let Some(trail) = records.as_slice().first().filter(|trail| is_key(trail))
        {
            push(&mut out, &lead, &[lead[14], trail[14]]);
            records.next();
        }
        while let Some(record) = records.next() {
            if !is_key(record) {
                out.push(*record);
                continue;
            }
            let byte = record[14];
            if !codepage::is_lead_byte(code_page, byte) {
                push(&mut out, record, &[byte]);
                continue;
            }
            match records.next() {
                None => self.input_lead = Some(*record),
                Some(trail) if is_key(trail) => push(&mut out, record, &[byte, trail[14]]),
                Some(_) => {}
            }
        }
        Ok(out)
    }

    // Screen-buffer state of a buffer that is not on screen.

    pub(super) fn off_screen_info(&mut self, buffer: u32) -> (i32, usize) {
        let (size, window, cursor, attributes) = match self.buffers.get(&buffer) {
            Some(screen) => (
                screen.size,
                screen.window,
                screen.saved.cursor,
                screen.saved.attributes,
            ),
            None => {
                let size = self.sink.size();
                (size, size, (0, 0), self.attributes)
            }
        };
        self.reply.fill(0);
        for (offset, (x, y)) in [(0, size), (4, cursor), (14, window), (18, size)] {
            set_i16(&mut self.reply, offset, x);
            set_i16(&mut self.reply, offset + 2, y);
        }
        set_u16(&mut self.reply, 12, attributes);
        set_u16(&mut self.reply, 22, self.state.popup_attributes);
        for (index, color) in self.state.palette.colors.into_iter().enumerate() {
            set_u32(&mut self.reply, 28 + index * 4, color);
        }
        (0, 0)
    }
    fn saved_mut(&mut self, buffer: u32) -> Option<&mut Saved> {
        self.buffers
            .get_mut(&buffer)
            .map(|screen| &mut screen.saved)
    }
    pub(super) fn off_screen_mode(&mut self, buffer: u32) -> u32 {
        self.saved_mut(buffer).map_or(0, |saved| saved.output_mode)
    }
    pub(super) fn set_off_screen_mode(&mut self, buffer: u32, value: u32) -> i32 {
        if value & !mode::OUTPUT_MODES != 0 {
            return INVALID_PARAMETER;
        }
        if let Some(saved) = self.saved_mut(buffer) {
            saved.output_mode = value;
        }
        0
    }
    pub(super) fn set_off_screen_cursor(&mut self, buffer: u32, x: i16, y: i16) -> i32 {
        match self.buffers.get_mut(&buffer) {
            Some(screen) if x >= 0 && y >= 0 && x < screen.size.0 && y < screen.size.1 => {
                screen.saved.cursor = (x, y);
                screen.dirty = true;
                0
            }
            Some(_) => INVALID_PARAMETER,
            None => self.saved_mut(buffer).map_or(0, |saved| {
                saved.cursor = (x, y);
                0
            }),
        }
    }
    /// SetConsoleWindowInfoImpl in VtIo mode on a buffer not on screen: the
    /// buffer becomes the window's size.
    pub(super) fn set_off_screen_window(
        &mut self,
        buffer: u32,
        window: Rect,
        absolute: bool,
    ) -> io::Result<i32> {
        let Some(screen) = self.buffers.get_mut(&buffer) else {
            return Ok(0);
        };
        let window = if absolute {
            window
        } else {
            Rect {
                left: window.left,
                top: window.top,
                right: window.right + i32::from(screen.window.0) - 1,
                bottom: window.bottom + i32::from(screen.window.1) - 1,
            }
        };
        if !window.is_valid() {
            return Ok(INVALID_PARAMETER);
        }
        let size = (window.width() as i16, window.height() as i16);
        screen.resize(size)?;
        screen.window = size;
        Ok(0)
    }
    /// SetConsoleScreenBufferSizeImpl on a buffer not on screen.
    /// The buffer's rows are committed as they are written, so any size
    /// that passes these checks costs only what is then written to it.
    pub(super) fn set_off_screen_size(&mut self, buffer: u32, size: (i16, i16)) -> io::Result<i32> {
        let Some(screen) = self.buffers.get_mut(&buffer) else {
            return Ok(0);
        };
        if size.0 < screen.window.0
            || size.1 < screen.window.1
            || size.0 == i16::MAX
            || size.1 == i16::MAX
        {
            return Ok(INVALID_PARAMETER);
        }
        if size != screen.size {
            screen.resize(size)?;
        }
        Ok(0)
    }
    pub(super) fn set_off_screen_attributes(&mut self, buffer: u32, attributes: u16) {
        if let Some(saved) = self.saved_mut(buffer) {
            saved.attributes = attributes;
        }
    }
    pub(super) fn set_off_screen_cursor_info(&mut self, buffer: u32, size: u32, visible: bool) {
        if let Some(saved) = self.saved_mut(buffer) {
            saved.cursor_size = size;
            saved.cursor_visible = visible;
        }
    }
    pub(super) fn off_screen_cursor_info(&mut self, buffer: u32) -> (u32, bool) {
        self.saved_mut(buffer).map_or((25, true), |saved| {
            (saved.cursor_size, saved.cursor_visible)
        })
    }
}

/// ReadConsoleOutputCharacter over `cells`: a wide glyph once when both of
/// its halves were read, a space where the read cut it in two.
fn characters(cells: &[CharInfo]) -> Vec<u16> {
    let mut out = Vec::with_capacity(cells.len());
    let mut index = 0;
    while index < cells.len() {
        let cell = cells[index];
        if cell.attributes & legacy::TRAILING_BYTE != 0 {
            if index == 0 {
                out.push(u16::from(b' '));
            }
            index += 1;
        } else if cell.attributes & legacy::LEADING_BYTE != 0 {
            let whole = cells
                .get(index + 1)
                .is_some_and(|next| next.attributes & legacy::TRAILING_BYTE != 0);
            out.push(if whole { cell.ch } else { u16::from(b' ') });
            index += if whole { 2 } else { 1 };
        } else {
            out.push(cell.ch);
            index += 1;
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/buffers.rs"]
mod tests;
