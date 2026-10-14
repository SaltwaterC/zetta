// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Console modes and code pages (GetConsoleMode/SetConsoleMode,
//! GetConsoleCP/SetConsoleCP and their output counterparts).
use super::*;
use crate::state::mode;

impl Server {
    pub(super) fn get_mode(&mut self, object: usize) -> (i32, usize) {
        let value = match self.objects.get(&object) {
            Some(Object::Input) => self.state.input_mode(),
            Some(Object::Output(buffer)) if *buffer == self.active => self.state.output_mode,
            Some(&Object::Output(buffer)) => self.off_screen_mode(buffer),
            None => return (INVALID_HANDLE, 0),
        };
        set_u32(&mut self.reply, 0, value);
        (0, 0)
    }
    pub(super) fn set_mode(&mut self, object: usize) -> io::Result<(i32, usize)> {
        let value = u32_at(&self.reply, 0);
        let status = match self.objects.get(&object) {
            Some(Object::Input) => {
                let before = self.terminal_input();
                let valid = self.state.set_input_mode(value);
                self.request_terminal_input(before)?;
                if valid { 0 } else { INVALID_PARAMETER }
            }
            Some(&Object::Output(buffer)) if buffer != self.active => {
                self.set_off_screen_mode(buffer, value)
            }
            Some(Object::Output(_)) => match self.state.set_output_mode(value) {
                // ConPTY reports a wrap change as DECAWM.
                Some(old) if (old ^ value) & mode::WRAP_AT_EOL_OUTPUT != 0 => {
                    let wrap = value & mode::WRAP_AT_EOL_OUTPUT != 0;
                    self.write_vt(if wrap { "\x1b[?7h" } else { "\x1b[?7l" })?;
                    0
                }
                Some(_) => 0,
                None => INVALID_PARAMETER,
            },
            None => return Ok((INVALID_HANDLE, 0)),
        };
        self.modes_changed();
        Ok((status, 0))
    }
    /// What the terminal is asked to send: win32 keys unless the application
    /// wants VT, and SGR mouse reports while it wants mouse records.
    fn terminal_input(&self) -> (bool, bool) {
        let vt = self.state.input_mode() & mode::VIRTUAL_TERMINAL_INPUT != 0;
        (vt, self.state.wants_mouse() && !vt)
    }
    /// OpenConsole's input buffer re-encodes keys itself in VT input mode;
    /// here the terminal already encodes them with the modes the application
    /// set through its output, so it is asked to leave win32-input-mode.
    fn request_terminal_input(&mut self, before: (bool, bool)) -> io::Result<()> {
        let (vt, mouse) = self.terminal_input();
        if vt != before.0 {
            self.write_vt(if vt { "\x1b[?9001l" } else { "\x1b[?9001h" })?;
        }
        if mouse != before.1 {
            // WriteSGR1006: any-event tracking in SGR encoding.
            self.write_vt(if mouse {
                "\x1b[?1003;1006h"
            } else {
                "\x1b[?1003;1006l"
            })?;
        }
        Ok(())
    }
    /// VtIo::StartIfNeeded's requests, without the DA1 query it uses only
    /// to tune itself.
    pub(crate) fn start_terminal_input(&mut self) -> io::Result<()> {
        self.write_vt("\x1b[?1004h\x1b[?9001h")
    }
    /// VtIo::Shutdown: the terminal gets its own input modes back.
    pub(super) fn stop_terminal_input(&mut self) {
        let (_, mouse) = self.terminal_input();
        let reset = if mouse {
            "\x1b[?1003;1006l\x1b[?1004l\x1b[?9001l"
        } else {
            "\x1b[?1004l\x1b[?9001l"
        };
        let _ = self.write_vt(reset);
    }
    /// The input worker reads modes and the input code page without locking
    /// the server.
    pub(crate) fn modes_changed(&self) {
        if let Some(input) = &self.input {
            input.set_modes(self.state.input_mode(), self.state.output_mode);
            input.set_code_page(self.state.input_cp);
        }
    }
    /// CONSOLE_GETCP_MSG: CodePage, then Output.
    pub(super) fn get_cp(&mut self) -> (i32, usize) {
        let code_page = if self.reply[4] != 0 {
            self.state.output_cp
        } else {
            self.state.input_cp
        };
        set_u32(&mut self.reply, 0, code_page);
        (0, 0)
    }
    /// DoSrvSetConsoleOutputCodePage and its input twin: any installed code
    /// page, which chcp reports as "Invalid code page" when refused.
    pub(super) fn set_cp(&mut self) -> (i32, usize) {
        let code_page = u32_at(&self.reply, 0);
        if !codepage::is_valid(code_page) {
            return (INVALID_PARAMETER, 0);
        }
        if self.reply[4] != 0 {
            self.state.output_cp = code_page;
            self.output_code_page_changed();
        } else {
            self.state.input_cp = code_page;
            self.modes_changed();
        }
        (0, 0)
    }
}
