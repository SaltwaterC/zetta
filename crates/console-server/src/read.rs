// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! ReadConsole and raw reads (stream.cpp's DoReadConsole): text left over
//! from an earlier read on the same handle first, then either a cooked read
//! through the line editor or a raw read of whatever characters are queued.
use super::*;
use crate::cooked::{Context, CookedRead, Point, Size, State as Cooked};
use crate::editing;

/// Text owed to a handle: the rest of a line longer than the caller's
/// buffer, or the further lines of a multi-line alias, one per read.
#[derive(Default)]
pub(crate) struct Pending {
    text: Vec<u16>,
    multiline: bool,
}

/// What GetChar makes of a key record.
pub(crate) enum Key {
    Char(u16, u32),
    Vkey(u16, u32),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keys {
    /// Raw reads: characters only.
    Plain,
    /// Cooked reads: the line editor's keys arrive as keys.
    Editing,
    /// A popup is open: only the popup's keys do.
    Popup,
}

const VK_MENU: u16 = 0x12;
const VK_ESCAPE: u16 = 0x1b;

/// IsCommandLineEditingKey / IsCommandLinePopupKey.
fn is_editing_key(vk: u16, state: u32, keys: Keys) -> bool {
    use crate::cooked::vk as k;
    let alt = state & (LEFT_ALT | RIGHT_ALT) != 0;
    let ctrl = state & (LEFT_CTRL | RIGHT_CTRL) != 0;
    if !alt && !ctrl {
        let popup = matches!(
            vk,
            k::ESCAPE
                | k::PRIOR
                | k::NEXT
                | k::END
                | k::HOME
                | k::LEFT
                | k::UP
                | k::RIGHT
                | k::DOWN
                | k::F2
                | k::F4
                | k::F7
                | k::F9
                | k::DELETE
        );
        return match keys {
            Keys::Popup => popup,
            Keys::Editing => {
                popup || matches!(vk, k::INSERT | k::F1 | k::F3 | k::F5 | k::F6 | k::F8)
            }
            Keys::Plain => false,
        };
    }
    if keys != Keys::Editing {
        return false;
    }
    (ctrl && matches!(vk, k::END | k::HOME | k::LEFT | k::RIGHT))
        || (alt && matches!(vk, k::F7 | k::F10))
}

/// GetChar: a character from a key press (Escape and LF only in VT input
/// mode), an editing key, or nothing for releases and other events.
pub(crate) fn get_char(record: &[u8; 20], keys: Keys, vt: bool) -> Option<Key> {
    if u16::from_le_bytes([record[0], record[1]]) != 1 {
        return None;
    }
    let down = u32_at(record, 4) != 0;
    let vk = u16::from_le_bytes([record[10], record[11]]);
    let ch = u16::from_le_bytes([record[14], record[15]]);
    let state = u32_at(record, 16);
    let editing = is_editing_key(vk, state, keys);
    if ch != 0 && !editing {
        // Alt+numpad composition delivers its character on Alt's release.
        if !down && vk == VK_MENU {
            return Some(Key::Char(ch, state));
        }
        if down && (vt || (vk != VK_ESCAPE && ch != 10)) {
            return Some(Key::Char(ch, state));
        }
    }
    if down {
        if editing {
            return Some(Key::Vkey(vk, state));
        }
        // The key that types NUL (Ctrl+@ on US layouts).
        let zero = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW(0) };
        let needs = |bit: i16| zero & bit != 0;
        if (zero & 0xff) as u16 == vk
            && (state & (LEFT_ALT | RIGHT_ALT) != 0) == needs(0x400)
            && (state & (LEFT_CTRL | RIGHT_CTRL) != 0) == needs(0x200)
            && (state & 0x10 != 0) == needs(0x100)
        {
            return Some(Key::Char(ch, state));
        }
    }
    None
}

/// InputBuffer::Consume: as much of `text` as fits in `capacity` bytes,
/// UTF-16 or whole characters in the input code page. Returns the bytes and
/// how many units they used.
pub(crate) fn encode_prefix(
    text: &[u16],
    capacity: usize,
    unicode: bool,
    code_page: u32,
) -> (Vec<u8>, usize) {
    if unicode {
        let count = (capacity / 2).min(text.len());
        return (
            text[..count]
                .iter()
                .flat_map(|unit| unit.to_le_bytes())
                .collect(),
            count,
        );
    }
    let mut bytes = Vec::new();
    let mut units = 0;
    for ch in char::decode_utf16(text.iter().copied()) {
        let ch = ch.unwrap_or('\u{fffd}');
        let mut buffer = [0; 2];
        let wide = ch.encode_utf16(&mut buffer);
        let encoded = codepage::from_utf16(code_page, wide);
        if bytes.len() + encoded.len() > capacity {
            break;
        }
        bytes.extend(encoded);
        units += wide.len();
    }
    (bytes, units)
}

impl Input {
    /// A text read: Ok(Some(bytes)) completes it, Ok(None) leaves it waiting.
    pub(super) fn read_text(&self, request: &mut Request) -> io::Result<Option<Vec<u8>>> {
        let capacity = request.descriptor.output_size as usize - request.body.len();
        let object = request.descriptor.object;
        {
            let mut state = lock(&self.0.state);
            if let Some(pending) = state.pending.get_mut(&object) {
                let bytes = take_pending(pending, capacity, request.unicode, self.code_page());
                if pending.text.is_empty() {
                    state.pending.remove(&object);
                }
                return Ok(Some(bytes));
            }
        }
        let mode = self.input_mode();
        if mode & ENABLE_LINE_INPUT == 0 {
            return Ok(self.read_raw(request, capacity, mode));
        }
        self.read_cooked(request, capacity, mode)
    }

    /// ReadCharacterInput: every queued character that fits, waiting only
    /// while there is none.
    fn read_raw(&self, request: &mut Request, capacity: usize, mode: u32) -> Option<Vec<u8>> {
        let vt = mode & VIRTUAL_TERMINAL_INPUT != 0;
        let code_page = self.code_page();
        let mut text: Vec<u16> = Vec::new();
        let mut used = 0;
        let mut state = lock(&self.0.state);
        let limit = if request.waited {
            state.first_event.unwrap_or(usize::MAX)
        } else {
            usize::MAX
        };
        state.first_event = None;
        let mut taken = 0;
        while taken < limit
            && let Some(record) = state.records.pop_front()
        {
            taken += 1;
            let Some(Key::Char(ch, _)) = get_char(&record, Keys::Plain, vt) else {
                continue;
            };
            let cost = if request.unicode {
                2
            } else {
                codepage::from_utf16(code_page, &[ch]).len().max(1)
            };
            if used + cost > capacity {
                state.records.push_front(record);
                break;
            }
            used += cost;
            text.push(ch);
            let repeat = u16::from_le_bytes([record[8], record[9]]);
            if repeat > 1 {
                let mut rest = record;
                rest[8..10].copy_from_slice(&(repeat - 1).to_le_bytes());
                state.records.push_front(rest);
            }
        }
        let input_ready = state.records.len() < 65536;
        drop(state);
        self.input_ready(input_ready);
        if text.is_empty() {
            return None;
        }
        let (bytes, _) = encode_prefix(&text, capacity, request.unicode, code_page);
        Some(self.control_z(request, bytes))
    }

    /// ProcessControlZ: a read starting with ^Z returns nothing, which is
    /// end of file to the C runtime.
    fn control_z(&self, request: &Request, bytes: Vec<u8>) -> Vec<u8> {
        if request.read.control_z && bytes.first() == Some(&0x1a) {
            Vec::new()
        } else {
            bytes
        }
    }

    /// The screen as the line editor needs it, after every earlier write.
    fn geometry(&self) -> io::Result<(Size, Point)> {
        let output = lock(&self.0.output);
        output.flush()?;
        let screen = output.screen();
        Ok((
            Size {
                width: i32::from(screen.columns),
                height: i32::from(screen.rows),
            },
            Point {
                x: i32::from(screen.cursor.0).max(0),
                y: i32::from(screen.cursor.1).max(0),
            },
        ))
    }

    fn write_echo(&self, text: &str) -> io::Result<()> {
        lock(&self.0.output).write(
            text.as_bytes(),
            OutputMode::from_console_flags(self.0.output_mode.load(Ordering::Acquire)),
        )
    }

    fn read_cooked(
        &self,
        request: &mut Request,
        capacity: usize,
        mode: u32,
    ) -> io::Result<Option<Vec<u8>>> {
        let processed = mode & 0x1 != 0;
        let echo = mode & 0x4 != 0;
        let vt = mode & VIRTUAL_TERMINAL_INPUT != 0;
        if request.cooked.is_none() {
            let (size, cursor) = if request.read.initial.is_empty() {
                (
                    Size {
                        width: 0,
                        height: 0,
                    },
                    Point::default(),
                )
            } else {
                self.geometry()?
            };
            request.cooked = Some(Box::new(CookedRead::new(
                &request.read.initial,
                request.read.wakeup_mask,
                mode & INSERT_MODE != 0,
                size,
                cursor,
            )));
        }
        let records: Vec<[u8; 20]> = {
            let mut state = lock(&self.0.state);
            state.first_event = None;
            // A cooked read normally ends at Enter. Copy only through that key
            // so a queued paste does not copy and requeue every later line
            // once for each ReadConsole call.
            let count = state
                .records
                .iter()
                .position(|record| {
                    u32_at(record, 4) != 0 && u16::from_le_bytes([record[14], record[15]]) == 13
                })
                .map_or(state.records.len(), |index| index + 1);
            state.records.drain(..count).collect()
        };
        let process = request.descriptor.process;
        let cooked = request.cooked.as_mut().unwrap();
        let mut used = 0;
        let mut clear_cmd_aliases = false;
        {
            let mut editing = editing::lock(&self.0.editing);
            for record in &records {
                if cooked.is_done() {
                    break;
                }
                used += 1;
                let repeat = u16::from_le_bytes([record[8], record[9]]).max(1);
                for _ in 0..repeat {
                    if cooked.is_done() {
                        break;
                    }
                    let keys = if cooked.has_popup() {
                        Keys::Popup
                    } else {
                        Keys::Editing
                    };
                    let key = get_char(record, keys, vt);
                    let mut context = Context {
                        history: editing.histories.find(process),
                        processed,
                        clear_cmd_aliases: &mut clear_cmd_aliases,
                    };
                    match (key, keys) {
                        (Some(Key::Char(ch, state)), Keys::Popup) => {
                            cooked.handle_popup(ch, 0, state, &mut context);
                        }
                        (Some(Key::Vkey(vk, state)), Keys::Popup) => {
                            cooked.handle_popup(0, vk, state, &mut context);
                        }
                        (Some(Key::Char(ch, state)), _) => {
                            cooked.handle_char(ch, state, &mut context);
                        }
                        (Some(Key::Vkey(vk, state)), _) => {
                            cooked.handle_vkey(vk, state, &mut context);
                        }
                        (None, _) => {}
                    }
                }
            }
            if clear_cmd_aliases {
                editing.aliases.clear_cmd();
            }
        }
        if used < records.len() {
            let mut state = lock(&self.0.state);
            for record in records[used..].iter().rev() {
                state.records.push_front(*record);
            }
        }
        if echo {
            let (size, cursor) = self.geometry()?;
            let popup = lock(&self.0.popup_sgr).clone();
            let vt_text = {
                let mut editing = editing::lock(&self.0.editing);
                let history = editing.histories.find(process).map(|history| &*history);
                cooked.redisplay(size, cursor, echo, &popup, history)
            };
            if let Some(vt_text) = vt_text {
                self.write_echo(&vt_text)?;
            }
        }
        if cooked.state == Cooked::DoneWithWakeupMask {
            cooked.complete_wakeup();
        }
        if !cooked.is_done() {
            return Ok(None);
        }
        Ok(Some(
            self.finish_cooked(request, capacity, processed, echo)?,
        ))
    }

    /// _handlePostCharInputLoop: the newline is echoed, the line enters the
    /// history and alias expansion, and what does not fit the caller's buffer
    /// waits on the handle.
    fn finish_cooked(
        &self,
        request: &mut Request,
        capacity: usize,
        processed: bool,
        echo: bool,
    ) -> io::Result<Vec<u8>> {
        let cooked = request.cooked.as_mut().unwrap();
        let state_reached = cooked.state;
        request.control_key_state = cooked.control_key_state;
        let buffer = cooked.take_buffer();
        let mut text = buffer.clone();
        let mut lines = 1;
        if state_reached == Cooked::DoneWithCarriageReturn {
            let newline: &[u16] = if processed { &[13, 10] } else { &[13] };
            self.write_echo(if processed { "\r\n" } else { "\r" })?;
            let mut alias = None;
            if echo {
                let mut editing = editing::lock(&self.0.editing);
                let no_duplicates = editing.histories.flags & crate::history::NO_DUPLICATES != 0;
                if let Some(history) = editing.histories.find(request.descriptor.process) {
                    history.add(&buffer, no_duplicates);
                }
                alias = editing.aliases.expand(&buffer, &request.read.exe);
            }
            match alias {
                Some((expanded, count)) => {
                    text = expanded;
                    lines = count;
                }
                None => text.extend_from_slice(newline),
            }
        }
        let first_line = if lines > 1 {
            text.iter()
                .position(|&unit| unit == 10)
                .map_or(text.len(), |end| end + 1)
        } else {
            text.len()
        };
        let (bytes, used) = encode_prefix(
            &text[..first_line],
            capacity,
            request.unicode,
            self.code_page(),
        );
        let rest = text[used..].to_vec();
        let mut state = lock(&self.0.state);
        if !rest.is_empty() {
            state.pending.insert(
                request.descriptor.object,
                Pending {
                    text: rest,
                    multiline: lines > 1,
                },
            );
        }
        state.ignore_next_keyup = true;
        drop(state);
        Ok(self.control_z(request, bytes))
    }
}

/// _ReadPendingInput: one line at a time for a multi-line alias.
fn take_pending(pending: &mut Pending, capacity: usize, unicode: bool, code_page: u32) -> Vec<u8> {
    let limit = if pending.multiline {
        pending
            .text
            .iter()
            .position(|&unit| unit == 10)
            .map_or(pending.text.len(), |end| end + 1)
    } else {
        pending.text.len()
    };
    let (bytes, used) = encode_prefix(&pending.text[..limit], capacity, unicode, code_page);
    pending.text.drain(..used);
    bytes
}

#[cfg(test)]
#[path = "tests/read.rs"]
mod tests;
