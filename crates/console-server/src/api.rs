// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! User-defined console API dispatch (CONSOLE_IO_USER_DEFINED). Each API's
//! message is validated against its size and copied into the reply, then
//! handed to the module that owns it; anything else fails explicitly.
use super::*;
use crate::protocol::api;

/// sizeof the API's message struct, from conmsgl1.h-conmsgl3.h, or None for
/// an API this server does not implement.
fn message_size(number: u32) -> Option<usize> {
    Some(match number {
        api::GET_CP
        | api::GET_CONSOLE_INPUT
        | api::WRITE_CONSOLE
        | api::SET_CP
        | api::GET_CURSOR_INFO
        | api::SET_CURSOR_INFO
        | api::GET_FONT_SIZE
        | api::SET_DISPLAY_MODE
        | api::GET_TITLE => 8,
        api::GET_MODE
        | api::SET_MODE
        | api::GET_NUMBER_OF_INPUT_EVENTS
        | api::SET_SCREEN_BUFFER_SIZE
        | api::SET_CURSOR_POSITION
        | api::GET_LARGEST_WINDOW_SIZE
        | api::GET_MOUSE_INFO
        | api::GET_DISPLAY_MODE
        | api::GET_PROCESS_LIST => 4,
        api::READ_CONSOLE => 20,
        api::GET_LANG_ID | api::SET_TEXT_ATTRIBUTE => 2,
        api::GET_SCREEN_BUFFER_INFO | api::SET_SCREEN_BUFFER_INFO => 92,
        api::SET_WINDOW_INFO => 10,
        api::SET_TITLE => 1,
        api::FLUSH_INPUT_BUFFER => 0,
        api::GET_CURRENT_FONT | api::SET_CURRENT_FONT => 84,
        api::GET_SELECTION_INFO => 16,
        api::GET_HISTORY | api::SET_HISTORY => 12,
        api::ADD_ALIAS
        | api::GET_ALIAS
        | api::GET_ALIASES_LENGTH
        | api::GET_ALIAS_EXES_LENGTH
        | api::GET_ALIASES
        | api::GET_ALIAS_EXES
        | api::SET_NUMBER_OF_COMMANDS
        | api::GET_COMMAND_HISTORY_LENGTH
        | api::GET_COMMAND_HISTORY => 8,
        api::EXPUNGE_COMMAND_HISTORY => 1,
        api::FILL_CONSOLE_OUTPUT => 16,
        api::GENERATE_CTRL_EVENT => 8,
        api::SET_ACTIVE_SCREEN_BUFFER => 0,
        api::SCROLL_SCREEN_BUFFER => 26,
        api::READ_OUTPUT_STRING | api::WRITE_OUTPUT_STRING => 12,
        api::WRITE_CONSOLE_INPUT => 8,
        api::READ_CONSOLE_OUTPUT | api::WRITE_CONSOLE_OUTPUT => 10,
        api::GET_CONSOLE_WINDOW => size_of::<usize>(),
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Input,
    Output,
}
/// The object an API operates on: input-buffer APIs refuse output handles
/// and the reverse, as OpenConsole's GetInputBuffer/GetScreenBuffer do.
fn object_kind(number: u32) -> Option<Kind> {
    match number {
        api::GET_NUMBER_OF_INPUT_EVENTS
        | api::GET_CONSOLE_INPUT
        | api::READ_CONSOLE
        | api::FLUSH_INPUT_BUFFER
        | api::WRITE_CONSOLE_INPUT => Some(Kind::Input),
        api::WRITE_CONSOLE
        | api::GET_CURSOR_INFO
        | api::SET_CURSOR_INFO
        | api::GET_SCREEN_BUFFER_INFO
        | api::SET_SCREEN_BUFFER_INFO
        | api::SET_SCREEN_BUFFER_SIZE
        | api::SET_CURSOR_POSITION
        | api::GET_LARGEST_WINDOW_SIZE
        | api::SET_TEXT_ATTRIBUTE
        | api::SET_WINDOW_INFO
        | api::GET_FONT_SIZE
        | api::GET_CURRENT_FONT
        | api::SET_CURRENT_FONT
        | api::SET_DISPLAY_MODE
        | api::FILL_CONSOLE_OUTPUT
        | api::SET_ACTIVE_SCREEN_BUFFER
        | api::SCROLL_SCREEN_BUFFER
        | api::READ_OUTPUT_STRING
        | api::WRITE_OUTPUT_STRING
        | api::READ_CONSOLE_OUTPUT
        | api::WRITE_CONSOLE_OUTPUT => Some(Kind::Output),
        _ => None,
    }
}

impl Server {
    pub(super) fn api(
        &mut self,
        handle: &Handle,
        packet: &Packet,
        available: usize,
    ) -> io::Result<(i32, usize)> {
        let d = packet.descriptor;
        if available < 8 || d.input_size < 8 {
            return Ok((INVALID_PARAMETER, 0));
        }
        let number = u32_at(&packet.payload, 0);
        let size = u32_at(&packet.payload, 4) as usize;
        let Some(required) = message_size(number) else {
            return Ok(self.unsupported_api(number));
        };
        if size < required
            || size > packet.payload.len() - 8
            || size + 8 > available
            || size + 8 > d.input_size as usize
            || size > d.output_size as usize
        {
            return Ok((INVALID_PARAMETER, 0));
        }
        let actual = match self.objects.get(&d.object) {
            Some(Object::Input) => Some(Kind::Input),
            Some(Object::Output(_)) => Some(Kind::Output),
            None => None,
        };
        if let Some(kind) = object_kind(number)
            && actual != Some(kind)
        {
            return Ok((INVALID_HANDLE, 0));
        }
        self.reply.extend_from_slice(&packet.payload[8..8 + size]);
        match number {
            api::GET_CP => Ok(self.get_cp()),
            api::GET_MODE => Ok(self.get_mode(d.object)),
            api::SET_MODE => self.set_mode(d.object),
            api::GET_NUMBER_OF_INPUT_EVENTS => Ok(self.input_events()),
            api::GET_CONSOLE_INPUT => {
                self.read_input(d, size, self.reply[6] != 0, true, Default::default())
            }
            api::READ_CONSOLE => self.read_console(handle, d, size),
            api::WRITE_CONSOLE => self.write_console(handle, d, size),
            api::GET_LANG_ID => Ok(self.get_lang_id()),
            api::FLUSH_INPUT_BUFFER => Ok(self.flush_input(d.object)),
            api::SET_CP => Ok(self.set_cp()),
            api::GET_CURSOR_INFO => self.get_cursor_info(d.object),
            api::SET_CURSOR_INFO => self.set_cursor_info(d.object),
            api::GET_SCREEN_BUFFER_INFO => self.get_screen_info(d.object),
            api::SET_SCREEN_BUFFER_INFO => self.set_screen_info(d.object),
            api::SET_SCREEN_BUFFER_SIZE => self.set_buffer_size(d.object),
            api::SET_CURSOR_POSITION => self.set_cursor_position(d.object),
            api::GET_LARGEST_WINDOW_SIZE => self.get_largest_window(),
            api::SET_TEXT_ATTRIBUTE => self.set_text_attribute(d.object),
            api::SET_WINDOW_INFO => self.set_window_info(),
            api::GET_TITLE => self.get_title(handle, d, size),
            api::SET_TITLE => self.set_title(handle, d, size),
            api::GET_MOUSE_INFO => Ok(self.get_mouse_info()),
            api::GET_FONT_SIZE => Ok(self.get_font_size()),
            api::GET_CURRENT_FONT => Ok(self.get_current_font()),
            api::SET_CURRENT_FONT => Ok(self.set_current_font()),
            api::SET_DISPLAY_MODE => self.set_display_mode(),
            api::GET_DISPLAY_MODE => Ok(self.get_display_mode()),
            api::GET_CONSOLE_WINDOW => Ok(self.get_console_window()),
            api::GET_SELECTION_INFO => Ok(self.get_selection_info()),
            api::GET_PROCESS_LIST => self.process_list(handle, d, size as u32),
            api::GET_HISTORY => Ok(self.get_history()),
            api::SET_HISTORY => Ok(self.set_history()),
            api::ADD_ALIAS => self.add_alias(handle, d, size),
            api::GET_ALIAS => self.get_alias(handle, d, size),
            api::GET_ALIASES_LENGTH => self.get_aliases_length(handle, d, size),
            api::GET_ALIAS_EXES_LENGTH => Ok(self.get_alias_exes_length()),
            api::GET_ALIASES => self.get_aliases(handle, d, size),
            api::GET_ALIAS_EXES => self.get_alias_exes(handle, d, size),
            api::EXPUNGE_COMMAND_HISTORY => self.expunge_history(handle, d, size),
            api::SET_NUMBER_OF_COMMANDS => self.set_number_of_commands(handle, d, size),
            api::GET_COMMAND_HISTORY_LENGTH => self.get_command_history_length(handle, d, size),
            api::GET_COMMAND_HISTORY => self.get_command_history(handle, d, size),
            api::FILL_CONSOLE_OUTPUT => self.fill_output(d),
            api::GENERATE_CTRL_EVENT => Ok(self.generate_ctrl_event()),
            api::SET_ACTIVE_SCREEN_BUFFER => self.set_active_screen_buffer(d.object),
            api::SCROLL_SCREEN_BUFFER => self.scroll(d),
            api::READ_OUTPUT_STRING => self.read_output_string(handle, d, size),
            api::WRITE_OUTPUT_STRING => self.write_output_string(handle, d, size),
            api::WRITE_CONSOLE_INPUT => self.write_console_input(handle, d, size),
            api::READ_CONSOLE_OUTPUT => self.read_output(handle, d, size),
            api::WRITE_CONSOLE_OUTPUT => self.write_output(handle, d, size),
            _ => unreachable!("message_size admits only dispatched APIs"),
        }
    }
    fn input_events(&mut self) -> (i32, usize) {
        let ready = self.input.as_ref().map_or(0, |input| input.ready() as u32);
        set_u32(&mut self.reply, 0, ready);
        (0, 0)
    }
    /// CONSOLE_WRITECONSOLE_MSG: NumBytes, then Unicode; the text follows.
    fn write_console(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[4] != 0;
        let (status, bytes) = self.write_request(handle, d, (8 + size) as u32, unicode)?;
        set_u32(&mut self.reply, 0, bytes as u32);
        Ok((status, bytes))
    }
    /// CONSOLE_READCONSOLE_MSG: Unicode, ProcessControlZ, ExeNameLength (in
    /// characters), InitialNumBytes, CtrlWakeupMask. The input holds the exe
    /// name, then the initial text, which only Unicode reads may pass.
    fn read_console(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[0] != 0;
        let exe_bytes = usize::from(u16_at(&self.reply, 2)) * 2;
        let initial_bytes = u32_at(&self.reply, 4) as usize;
        let input_offset = 8 + size;
        let available = (d.input_size as usize).saturating_sub(input_offset);
        if exe_bytes + initial_bytes > available
            || initial_bytes > (d.output_size as usize).saturating_sub(size)
            || initial_bytes > 0 && !unicode
        {
            return Ok((INVALID_PARAMETER, 0));
        }
        let mut bytes = vec![0; exe_bytes + initial_bytes];
        if !bytes.is_empty() {
            native::transfer(handle, READ_INPUT, d.id, input_offset as u32, &mut bytes)?;
        }
        let wide = |range: &[u8]| -> Vec<u16> {
            range
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect()
        };
        let read = ReadParams {
            exe: wide(&bytes[..exe_bytes]),
            initial: wide(&bytes[exe_bytes..]),
            wakeup_mask: u32_at(&self.reply, 8),
            control_z: self.reply[1] != 0
                && self.state.input_mode() & crate::state::mode::PROCESSED_INPUT != 0,
        };
        self.read_input(d, size, unicode, false, read)
    }
}
