// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! The alias and command-history APIs doskey is built on (alias.cpp,
//! history.cpp). Strings follow each message in its input, results go after
//! it in the output, and ANSI callers use the input code page throughout.
use super::*;
use crate::editing;

/// HRESULT_FROM_WIN32 as the driver carries it (NTSTATUS_FROM_WIN32).
fn win32(code: u32) -> i32 {
    (0xc007_0000 | code) as i32
}
const ERROR_GEN_FAILURE: u32 = 31;
const ERROR_BUFFER_OVERFLOW: u32 = 111;
const BUFFER_TOO_SMALL: i32 = 0xc000_0023_u32 as i32;

impl Server {
    /// The message's input after the API header and body.
    pub(super) fn api_input(
        &self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<Vec<u8>> {
        let offset = 8 + size;
        let mut bytes = vec![0; (d.input_size as usize).saturating_sub(offset)];
        if !bytes.is_empty() {
            native::transfer(handle, READ_INPUT, d.id, offset as u32, &mut bytes)?;
        }
        Ok(bytes)
    }
    fn text(&self, bytes: &[u8], unicode: bool) -> Vec<u16> {
        if unicode {
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect()
        } else {
            codepage::to_utf16(self.state.input_cp, bytes)
        }
    }
    fn encode(&self, text: &[u16], unicode: bool) -> Vec<u8> {
        if unicode {
            text.iter().flat_map(|unit| unit.to_le_bytes()).collect()
        } else {
            codepage::from_utf16(self.state.input_cp, text)
        }
    }
    /// Writes `bytes` after the message body; the information is their size.
    pub(super) fn reply_output(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
        mut bytes: Vec<u8>,
    ) -> io::Result<(i32, usize)> {
        if bytes.is_empty() {
            return Ok((0, 0));
        }
        if bytes.len() > (d.output_size as usize).saturating_sub(size) {
            return Ok((win32(ERROR_BUFFER_OVERFLOW), 0));
        }
        native::transfer(handle, WRITE_OUTPUT, d.id, size as u32, &mut bytes)?;
        Ok((0, bytes.len()))
    }

    /// CONSOLE_ADDALIAS_MSG: SourceLength, TargetLength, ExeLength (bytes),
    /// Unicode; the input holds exe, source and target in that order.
    pub(super) fn add_alias(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let (source, target, exe) = (
            usize::from(u16_at(&self.reply, 0)),
            usize::from(u16_at(&self.reply, 2)),
            usize::from(u16_at(&self.reply, 4)),
        );
        let unicode = self.reply[6] != 0;
        let input = self.api_input(handle, d, size)?;
        if exe + source + target > input.len() || unicode && (exe | source | target) & 1 != 0 {
            return Ok((INVALID_PARAMETER, 0));
        }
        let exe_name = self.text(&input[..exe], unicode);
        let source = self.text(&input[exe..exe + source], unicode);
        let target_start = exe + usize::from(u16_at(&self.reply, 0));
        let target = self.text(&input[target_start..target_start + target], unicode);
        let added = editing::lock(&self.editing)
            .aliases
            .add(&source, &target, &exe_name);
        Ok(if added {
            (0, 0)
        } else {
            (INVALID_PARAMETER, 0)
        })
    }

    /// CONSOLE_GETALIAS_MSG: TargetLength comes back as the bytes written.
    pub(super) fn get_alias(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let source = usize::from(u16_at(&self.reply, 0));
        let exe = usize::from(u16_at(&self.reply, 4));
        let unicode = self.reply[6] != 0;
        let input = self.api_input(handle, d, size)?;
        if exe + source > input.len() || unicode && (exe | source) & 1 != 0 {
            return Ok((INVALID_PARAMETER, 0));
        }
        let exe_name = self.text(&input[..exe], unicode);
        let source = self.text(&input[exe..exe + source], unicode);
        let target = editing::lock(&self.editing)
            .aliases
            .get(&source, &exe_name)
            .map(<[u16]>::to_vec);
        let Some(mut target) = target else {
            set_u16(&mut self.reply, 2, 0);
            return Ok((win32(ERROR_GEN_FAILURE), 0));
        };
        target.push(0);
        let bytes = self.encode(&target, unicode);
        let capacity = (d.output_size as usize).saturating_sub(size);
        if bytes.len() > capacity {
            // GetConsoleAliasAImpl reports the buffer's size in wide
            // characters' bytes, a long-standing bug kept for compatibility.
            let reported = if unicode { capacity } else { capacity * 2 };
            set_u16(
                &mut self.reply,
                2,
                reported.min(usize::from(u16::MAX)) as u16,
            );
            return Ok((BUFFER_TOO_SMALL, 0));
        }
        set_u16(&mut self.reply, 2, bytes.len() as u16);
        self.reply_output(handle, d, size, bytes)
    }

    /// GetConsoleAliasesLength: bytes for "source=target\0" per alias.
    pub(super) fn get_aliases_length(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[4] != 0;
        let input = self.api_input(handle, d, size)?;
        let exe = self.text(&input, unicode);
        let list = editing::lock(&self.editing).aliases.list(&exe);
        let length = self.encode(&list, unicode).len();
        set_u32(&mut self.reply, 0, length as u32);
        Ok((0, 0))
    }
    pub(super) fn get_aliases(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[0] != 0;
        let input = self.api_input(handle, d, size)?;
        let exe = self.text(&input, unicode);
        let list = editing::lock(&self.editing).aliases.list(&exe);
        let bytes = self.encode(&list, unicode);
        let result = self.reply_output(handle, d, size, bytes)?;
        set_u32(&mut self.reply, 4, result.1 as u32);
        Ok(result)
    }
    pub(super) fn get_alias_exes_length(&mut self) -> (i32, usize) {
        let unicode = self.reply[4] != 0;
        let exes = editing::lock(&self.editing).aliases.exes();
        let length = self.encode(&exes, unicode).len();
        set_u32(&mut self.reply, 0, length as u32);
        (0, 0)
    }
    pub(super) fn get_alias_exes(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[4] != 0;
        let exes = editing::lock(&self.editing).aliases.exes();
        let bytes = self.encode(&exes, unicode);
        let result = self.reply_output(handle, d, size, bytes)?;
        set_u32(&mut self.reply, 0, result.1 as u32);
        Ok(result)
    }

    pub(super) fn expunge_history(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[0] != 0;
        let input = self.api_input(handle, d, size)?;
        let exe = self.text(&input, unicode);
        if let Some(history) = editing::lock(&self.editing).histories.by_application(&exe) {
            history.empty();
        }
        Ok((0, 0))
    }
    pub(super) fn set_number_of_commands(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let commands = u32_at(&self.reply, 0);
        let unicode = self.reply[4] != 0;
        let input = self.api_input(handle, d, size)?;
        let exe = self.text(&input, unicode);
        editing::lock(&self.editing)
            .histories
            .realloc_application(&exe, commands);
        Ok((0, 0))
    }
    /// "command\0" for each command, oldest first.
    fn history_text(&self, exe: &[u16]) -> Vec<u16> {
        let mut editing = editing::lock(&self.editing);
        let mut out = Vec::new();
        if let Some(history) = editing.histories.by_application(exe) {
            for command in history.commands() {
                out.extend_from_slice(command);
                out.push(0);
            }
        }
        out
    }
    pub(super) fn get_command_history_length(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[4] != 0;
        let input = self.api_input(handle, d, size)?;
        let exe = self.text(&input, unicode);
        let length = self.encode(&self.history_text(&exe), unicode).len();
        set_u32(&mut self.reply, 0, length as u32);
        Ok((0, 0))
    }
    pub(super) fn get_command_history(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[4] != 0;
        let input = self.api_input(handle, d, size)?;
        let exe = self.text(&input, unicode);
        let bytes = self.encode(&self.history_text(&exe), unicode);
        let result = self.reply_output(handle, d, size, bytes)?;
        set_u32(&mut self.reply, 0, result.1 as u32);
        Ok(result)
    }
}
