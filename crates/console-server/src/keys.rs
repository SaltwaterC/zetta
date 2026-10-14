// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Terminal input to console input events, as OpenConsole's
//! InputStateMachineEngine parses what a terminal sends ConPTY.
//!
//! win32-input-mode keys become records unchanged. Without VT input mode,
//! VT keys, SGR mouse reports, focus reports and text become the key, mouse
//! and focus records a console window would have produced. With VT input
//! mode the application asked for VT, so everything but win32 keys passes
//! through as text, as OpenConsole does when its dispatch declines.
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, MAPVK_VK_TO_CHAR, MAPVK_VK_TO_VSC, MapVirtualKeyW, VkKeyScanW,
};

pub const KEY_EVENT: u16 = 0x1;
pub const MOUSE_EVENT: u16 = 0x2;
pub const FOCUS_EVENT: u16 = 0x10;

pub const RIGHT_ALT: u32 = 0x1;
pub const LEFT_ALT: u32 = 0x2;
pub const RIGHT_CTRL: u32 = 0x4;
pub const LEFT_CTRL: u32 = 0x8;
pub const SHIFT: u32 = 0x10;
pub const ENHANCED_KEY: u32 = 0x100;

const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;
const VK_ESCAPE: u16 = 0x1b;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_INSERT: u16 = 0x2d;
const VK_DELETE: u16 = 0x2e;
const VK_F1: u16 = 0x70;

const FROM_LEFT_1ST_BUTTON_PRESSED: u32 = 0x1;
const RIGHTMOST_BUTTON_PRESSED: u32 = 0x2;
const FROM_LEFT_2ND_BUTTON_PRESSED: u32 = 0x4;
const SCROLL_DELTA_FORWARD: u32 = 0x0078_0000;
const SCROLL_DELTA_BACKWARD: u32 = 0xff88_0000;
const MOUSE_MOVED: u32 = 0x1;
const DOUBLE_CLICK: u32 = 0x2;
const MOUSE_WHEELED: u32 = 0x4;
const MOUSE_HWHEELED: u32 = 0x8;

/// An INPUT_RECORD as the driver carries it.
pub type Record = [u8; 20];

pub fn key(down: bool, repeat: u16, vk: u16, scan: u16, ch: u16, state: u32) -> Record {
    let mut record = [0; 20];
    record[..2].copy_from_slice(&KEY_EVENT.to_le_bytes());
    record[4..8].copy_from_slice(&u32::from(down).to_le_bytes());
    record[8..10].copy_from_slice(&repeat.to_le_bytes());
    record[10..12].copy_from_slice(&vk.to_le_bytes());
    record[12..14].copy_from_slice(&scan.to_le_bytes());
    record[14..16].copy_from_slice(&ch.to_le_bytes());
    record[16..20].copy_from_slice(&state.to_le_bytes());
    record
}
fn mouse(position: (i16, i16), buttons: u32, state: u32, flags: u32) -> Record {
    let mut record = [0; 20];
    record[..2].copy_from_slice(&MOUSE_EVENT.to_le_bytes());
    record[4..6].copy_from_slice(&position.0.to_le_bytes());
    record[6..8].copy_from_slice(&position.1.to_le_bytes());
    record[8..12].copy_from_slice(&buttons.to_le_bytes());
    record[12..16].copy_from_slice(&state.to_le_bytes());
    record[16..20].copy_from_slice(&flags.to_le_bytes());
    record
}
pub fn focus(focused: bool) -> Record {
    let mut record = [0; 20];
    record[..2].copy_from_slice(&FOCUS_EVENT.to_le_bytes());
    record[4..8].copy_from_slice(&u32::from(focused).to_le_bytes());
    record
}

/// What one piece of terminal input means to the console.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Records synthesized from VT, mouse and focus input.
    Records(Vec<Record>),
    /// A win32-input-mode key, which goes through Ctrl+C/Ctrl+Break
    /// handling (OpenConsole's WriteCtrlKey) before reaching the buffer.
    Key(Record),
    /// Text stored as character key-downs without virtual keys, as
    /// InputBuffer::WriteString does: VT passed through, and typed text in
    /// VT input mode.
    Raw(String),
}

#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    buttons: u32,
    last_click: Option<(Instant, (i16, i16), usize)>,
    /// A win32-input-mode key has arrived, so CSI R is a cursor report
    /// rather than F3, as OpenConsole decides.
    win32_seen: bool,
}

/// How far a parse got: a complete token and its length, or a need for
/// more bytes before deciding.
enum Token<'a> {
    Text(&'a str),
    Control(u8),
    Escape(char),
    Csi {
        private: Option<u8>,
        params: Vec<Option<u32>>,
        final_byte: u8,
        /// The sequence as received, for passing it through.
        bytes: &'a [u8],
    },
    Ss3(u8),
    /// OSC, or a CSI the parser could not use: passed through verbatim.
    Verbatim(&'a [u8]),
    /// DCS, swallowed up to its terminator.
    Swallowed,
}

impl Decoder {
    /// Decodes `bytes`, keeping an incomplete trailing sequence for the next
    /// call. A lone ESC at the end is the Escape key: a terminal sends a
    /// sequence in one write.
    pub fn decode(&mut self, bytes: &[u8], vt_input: bool) -> Vec<Event> {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut offset = 0;
        while offset < data.len() {
            let Some((token, length)) = tokenize(&data[offset..]) else {
                // Hold an incomplete sequence, but never more than a terminal
                // would plausibly split; anything longer is passed through.
                if data.len() - offset > 4096 {
                    let rest = String::from_utf8_lossy(&data[offset..]).into_owned();
                    events.push(Event::Raw(rest));
                } else {
                    self.pending = data[offset..].to_vec();
                }
                break;
            };
            self.dispatch(token, vt_input, &mut events);
            offset += length;
        }
        events
    }

    fn dispatch(&mut self, token: Token<'_>, vt: bool, events: &mut Vec<Event>) {
        match token {
            Token::Text(text) if vt => events.push(Event::Raw(text.to_owned())),
            Token::Text(text) => {
                let mut records = Vec::new();
                for unit in text.encode_utf16() {
                    char_to_keys(unit, &mut records);
                }
                events.push(Event::Records(records));
            }
            Token::Control(byte) => control(byte, false, vt, events),
            Token::Escape(ch) if vt => events.push(Event::Raw(format!("\x1b{ch}"))),
            Token::Escape('\x7f') => control(0x7f, true, vt, events),
            Token::Escape(ch) if (ch as u32) < 0x20 => control(ch as u8, true, vt, events),
            Token::Escape(ch) => {
                let mut buffer = [0; 2];
                for &unit in ch.encode_utf16(&mut buffer).iter() {
                    if let Some((vk, state)) = key_from_char(unit) {
                        events.push(Event::Records(wrapped(unit, vk, state | LEFT_ALT)));
                    }
                }
            }
            Token::Csi {
                private,
                params,
                final_byte,
                bytes,
            } => self.csi(private, &params, final_byte, bytes, vt, events),
            Token::Ss3(byte) if vt => events.push(Event::Raw(format!("\x1bO{}", byte as char))),
            Token::Ss3(byte) => {
                if let Some(vk) = cursor_key(byte) {
                    events.push(Event::Records(wrapped(vk_char(vk), vk, 0)));
                }
            }
            Token::Verbatim(bytes) => {
                events.push(Event::Raw(String::from_utf8_lossy(bytes).into_owned()));
            }
            Token::Swallowed => {}
        }
    }

    fn csi(
        &mut self,
        private: Option<u8>,
        params: &[Option<u32>],
        final_byte: u8,
        bytes: &[u8],
        vt: bool,
        events: &mut Vec<Event>,
    ) {
        let param = |index: usize| params.get(index).copied().flatten();
        let raw = || Event::Raw(String::from_utf8_lossy(bytes).into_owned());
        match (private, final_byte) {
            (None, b'_') => {
                self.win32_seen = true;
                let at = |index: usize, default: u32| param(index).unwrap_or(default);
                events.push(Event::Key(key(
                    at(3, 0) != 0,
                    at(5, 1).min(u32::from(u16::MAX)) as u16,
                    at(0, 0).min(u32::from(u16::MAX)) as u16,
                    at(1, 0).min(u32::from(u16::MAX)) as u16,
                    at(2, 0).min(u32::from(u16::MAX)) as u16,
                    at(4, 0),
                )));
            }
            (None, b'I' | b'O') if vt => {
                // Only an application that enabled focus reports itself would
                // get them from OpenConsole, and that cannot be known here.
            }
            (None, b'I' | b'O') => events.push(Event::Records(vec![focus(final_byte == b'I')])),
            (Some(b'<'), b'M' | b'm') if !vt => {
                if let Some(record) = self.mouse(params, final_byte == b'M') {
                    events.push(Event::Records(vec![record]));
                }
            }
            _ if vt => events.push(raw()),
            // CSI R is F3 only until win32-input-mode shows it is a report.
            (None, b'R') if self.win32_seen => events.push(raw()),
            (None, b'A'..=b'D' | b'H' | b'F' | b'P' | b'Q' | b'R' | b'S') => {
                let vk = cursor_key(final_byte).unwrap_or(0);
                let mut state = modifiers(param(1));
                if !matches!(final_byte, b'P'..=b'S') {
                    state |= ENHANCED_KEY;
                }
                events.push(Event::Records(wrapped(vk_char(vk), vk, state)));
            }
            (None, b'~') => {
                // Identifiers OpenConsole does not map, such as bracketed
                // paste's 200 and 201, are consumed without a key.
                if let Some(vk) = param(0).and_then(generic_key) {
                    let mut state = modifiers(param(1));
                    if param(0).is_some_and(|id| id <= 6) {
                        state |= ENHANCED_KEY;
                    }
                    events.push(Event::Records(wrapped(vk_char(vk), vk, state)));
                }
            }
            (None, b'Z') => events.push(Event::Records(wrapped(vk_char(VK_TAB), VK_TAB, SHIFT))),
            _ => events.push(raw()),
        }
    }

    /// _UpdateSGRMouseButtonState: buttons are tracked across reports, and a
    /// second press of the same button in place within the double-click time
    /// is a double click.
    fn mouse(&mut self, params: &[Option<u32>], pressed: bool) -> Option<Record> {
        let encoding = params.first().copied().flatten().unwrap_or(0) as usize;
        let x = params
            .get(1)
            .copied()
            .flatten()
            .unwrap_or(1)
            .saturating_sub(1);
        let y = params
            .get(2)
            .copied()
            .flatten()
            .unwrap_or(1)
            .saturating_sub(1);
        let position = (x.min(i16::MAX as u32) as i16, y.min(i16::MAX as u32) as i16);
        let button = (encoding & 0x3) | ((encoding & 0xc0) >> 4);
        let mut buttons = self.buttons;
        let mut flags = 0;
        let flag = match button {
            0 => FROM_LEFT_1ST_BUTTON_PRESSED,
            1 => FROM_LEFT_2ND_BUTTON_PRESSED,
            2 => RIGHTMOST_BUTTON_PRESSED,
            3 => 0,
            4..=7 => {
                // Forward is wheel up (4) and right (7).
                buttons |= if matches!(button, 4 | 7) {
                    SCROLL_DELTA_FORWARD
                } else {
                    SCROLL_DELTA_BACKWARD
                };
                flags |= if button < 6 {
                    MOUSE_WHEELED
                } else {
                    MOUSE_HWHEELED
                };
                0
            }
            _ => return None,
        };
        if pressed {
            buttons |= flag;
            let now = Instant::now();
            let window = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
            match self.last_click {
                Some((at, place, last))
                    if place == position && last == button && now - at < window =>
                {
                    flags |= DOUBLE_CLICK;
                    self.last_click = None;
                }
                _ if button <= 2 => self.last_click = Some((now, position, button)),
                _ => {}
            }
        } else {
            buttons &= !flag;
        }
        if encoding & 32 != 0 {
            flags |= MOUSE_MOVED;
        }
        self.buttons = buttons & 0xffff;
        let mut state = 0;
        if encoding & 4 != 0 {
            state |= SHIFT;
        }
        if encoding & 8 != 0 {
            state |= LEFT_ALT;
        }
        if encoding & 16 != 0 {
            state |= LEFT_CTRL;
        }
        Some(mouse(position, buttons, state, flags))
    }
}

/// _DoControlCharacter. ETX is Ctrl+C through the control-key path; the
/// rest are their Ctrl+letter keys, with BS as Ctrl+Backspace's DEL, CR and
/// TAB unmodified, and ESC as the Escape key.
fn control(byte: u8, alt: bool, vt: bool, events: &mut Vec<Event>) {
    if byte == 0x03 && !alt {
        let state = LEFT_CTRL;
        let scan = scan_code(u16::from(b'C'));
        events.push(Event::Key(key(true, 1, u16::from(b'C'), scan, 3, state)));
        events.push(Event::Key(key(false, 1, u16::from(b'C'), scan, 3, state)));
        return;
    }
    if vt {
        let mut text = String::new();
        if alt {
            text.push('\x1b');
        }
        text.push(byte as char);
        events.push(Event::Raw(text));
        return;
    }
    let alt_state = if alt { LEFT_ALT } else { 0 };
    if byte == 0x7f {
        events.push(Event::Records(wrapped(0x08, VK_BACK, alt_state)));
        return;
    }
    let (unit, keyed) = match byte {
        0x08 => (0x7f, key_from_char(0x7f).map(|(vk, _)| (vk, LEFT_CTRL))),
        b'\r' => (
            u16::from(byte),
            key_from_char(u16::from(byte)).map(|(vk, _)| (vk, 0)),
        ),
        0x1b => (u16::from(byte), Some((VK_ESCAPE, 0))),
        b'\t' => (u16::from(byte), key_from_char(u16::from(byte))),
        _ => (
            u16::from(byte),
            key_from_char(u16::from(byte)).map(|(vk, state)| (vk, state | LEFT_CTRL)),
        ),
    };
    if let Some((vk, state)) = keyed {
        events.push(Event::Records(wrapped(unit, vk, state | alt_state)));
    }
}

/// _GenerateWrappedSequence: modifier presses around the key's press and
/// release, as a keyboard produces them.
fn wrapped(ch: u16, vk: u16, state: u32) -> Vec<Record> {
    let mut records = Vec::new();
    let mut current = 0;
    let modifier_keys = [
        (SHIFT, VK_SHIFT),
        (LEFT_ALT, VK_MENU),
        (LEFT_CTRL, VK_CONTROL),
    ];
    for (flag, modifier) in modifier_keys {
        if state & flag != 0 {
            current |= flag;
            records.push(key(true, 1, modifier, scan_code(modifier), 0, current));
        }
    }
    let scan = scan_code(vk);
    records.push(key(true, 1, vk, scan, ch, state));
    records.push(key(false, 1, vk, scan, ch, state));
    for (flag, modifier) in modifier_keys.into_iter().rev() {
        if state & flag != 0 {
            current &= !flag;
            records.push(key(false, 1, modifier, scan_code(modifier), 0, current));
        }
    }
    records
}

/// CharToKeyEvents without the numpad fallback: the key that types the
/// character on the current layout, with Shift or AltGr pressed around it,
/// or a bare character where no key types it.
fn char_to_keys(unit: u16, records: &mut Vec<Record>) {
    let scan = unsafe { VkKeyScanW(unit) };
    let (vk, shift_state) = if scan == -1 {
        (0, 0)
    } else {
        ((scan & 0xff) as u16, ((scan >> 8) & 0xff) as u8)
    };
    let shift = shift_state & 1 != 0;
    let alt_gr = shift_state & 6 == 6;
    let mut state = 0;
    if shift {
        state |= SHIFT;
    }
    if shift_state & 2 != 0 {
        state |= LEFT_CTRL;
    }
    if shift_state & 4 != 0 {
        state |= RIGHT_ALT;
    }
    if alt_gr {
        records.push(key(
            true,
            1,
            VK_MENU,
            0x38,
            0,
            ENHANCED_KEY | LEFT_CTRL | RIGHT_ALT,
        ));
    } else if shift {
        records.push(key(true, 1, VK_SHIFT, 0x2a, 0, SHIFT));
    }
    let code = scan_code(vk);
    records.push(key(true, 1, vk, code, unit, state));
    records.push(key(false, 1, vk, code, unit, state));
    if alt_gr {
        records.push(key(false, 1, VK_MENU, 0x38, 0, ENHANCED_KEY));
    } else if shift {
        records.push(key(false, 1, VK_SHIFT, 0x2a, 0, 0));
    }
}

/// _GenerateKeyFromChar: VkKeyScanW's key and the modifiers it needs.
fn key_from_char(unit: u16) -> Option<(u16, u32)> {
    let scan = unsafe { VkKeyScanW(unit) };
    if scan == -1 {
        return None;
    }
    let shift_state = (scan >> 8) & 0xff;
    let mut state = 0;
    if shift_state & 1 != 0 {
        state |= SHIFT;
    }
    if shift_state & 2 != 0 {
        state |= LEFT_CTRL;
    }
    if shift_state & 4 != 0 {
        state |= LEFT_ALT;
    }
    Some(((scan & 0xff) as u16, state))
}

fn scan_code(vk: u16) -> u16 {
    unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) as u16 }
}

/// The character a key types unshifted, as _WriteSingleKey(vkey) uses.
fn vk_char(vk: u16) -> u16 {
    unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_CHAR) as u16 }
}

/// s_csiMap and s_ss3Map: the same finals in CSI and SS3 form.
fn cursor_key(final_byte: u8) -> Option<u16> {
    Some(match final_byte {
        b'A' => VK_UP,
        b'B' => VK_DOWN,
        b'C' => VK_RIGHT,
        b'D' => VK_LEFT,
        b'H' => VK_HOME,
        b'F' => VK_END,
        b'P' => VK_F1,
        b'Q' => VK_F1 + 1,
        b'R' => VK_F1 + 2,
        b'S' => VK_F1 + 3,
        _ => return None,
    })
}

/// s_genericMap: `CSI n ~` identifiers.
fn generic_key(identifier: u32) -> Option<u16> {
    Some(match identifier {
        1 => VK_HOME,
        2 => VK_INSERT,
        3 => VK_DELETE,
        4 => VK_END,
        5 => VK_PRIOR,
        6 => VK_NEXT,
        15 => VK_F1 + 4,
        17..=21 => VK_F1 + 5 + (identifier - 17) as u16,
        23 => VK_F1 + 10,
        24 => VK_F1 + 11,
        _ => return None,
    })
}

/// _GetModifier: the xterm modifier parameter, one more than its bits.
fn modifiers(parameter: Option<u32>) -> u32 {
    let bits = parameter.unwrap_or(1).saturating_sub(1);
    let mut state = 0;
    if bits & 1 != 0 {
        state |= SHIFT;
    }
    if bits & 2 != 0 {
        state |= LEFT_ALT;
    }
    if bits & 4 != 0 {
        state |= LEFT_CTRL;
    }
    state
}

/// One token from the start of `data`, or None when more bytes are needed.
fn tokenize(data: &[u8]) -> Option<(Token<'_>, usize)> {
    match data[0] {
        0x1b => escape(data),
        byte if byte < 0x20 || byte == 0x7f => Some((Token::Control(byte), 1)),
        _ => {
            let end = data
                .iter()
                .position(|&byte| byte < 0x20 || byte == 0x7f)
                .unwrap_or(data.len());
            let text = &data[..end];
            match std::str::from_utf8(text) {
                Ok(text) => Some((Token::Text(text), end)),
                Err(error) if error.valid_up_to() > 0 => {
                    let valid = error.valid_up_to();
                    Some((
                        Token::Text(std::str::from_utf8(&text[..valid]).unwrap()),
                        valid,
                    ))
                }
                // A character split across writes completes with the next.
                Err(error) if error.error_len().is_none() && end == data.len() => None,
                Err(error) => {
                    let skip = error.error_len().unwrap_or(1);
                    Some((Token::Text("\u{fffd}"), skip))
                }
            }
        }
    }
}

fn escape(data: &[u8]) -> Option<(Token<'_>, usize)> {
    let Some(&next) = data.get(1) else {
        return Some((Token::Control(0x1b), 1));
    };
    match next {
        b'[' => csi(data),
        b'O' => match data.get(2) {
            Some(&byte) if (0x40..=0x7e).contains(&byte) => Some((Token::Ss3(byte), 3)),
            Some(_) => Some((Token::Escape('O'), 2)),
            None => Some((Token::Escape('O'), 2)),
        },
        b']' => string(data, true),
        b'P' => string(data, false),
        _ => {
            // Alt with one character, which may be several bytes long.
            let length = match next {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 1,
            };
            if data.len() < 1 + length {
                return None;
            }
            match std::str::from_utf8(&data[1..1 + length])
                .ok()
                .and_then(|text| text.chars().next())
            {
                Some(ch) => Some((Token::Escape(ch), 1 + length)),
                None => Some((Token::Control(0x1b), 1)),
            }
        }
    }
}

fn csi(data: &[u8]) -> Option<(Token<'_>, usize)> {
    let mut index = 2;
    let private = match data.get(index) {
        Some(&byte @ (b'<' | b'=' | b'>' | b'?')) => {
            index += 1;
            Some(byte)
        }
        Some(_) => None,
        None => return None,
    };
    let start = index;
    while let Some(&byte) = data.get(index) {
        match byte {
            b'0'..=b'9' | b';' | b':' => index += 1,
            0x20..=0x2f => index += 1,
            0x40..=0x7e => {
                let params = std::str::from_utf8(&data[start..index])
                    .unwrap_or("")
                    .split(';')
                    .map(|value| value.split(':').next().and_then(|v| v.parse().ok()))
                    .collect();
                return Some((
                    Token::Csi {
                        private,
                        params,
                        final_byte: byte,
                        bytes: &data[..=index],
                    },
                    index + 1,
                ));
            }
            _ => return Some((Token::Verbatim(&data[..index]), index)),
        }
        if index > 256 {
            return Some((Token::Verbatim(&data[..index]), index));
        }
    }
    None
}

/// OSC (verbatim) or DCS (swallowed), ended by BEL or ST.
fn string(data: &[u8], osc: bool) -> Option<(Token<'_>, usize)> {
    let mut index = 2;
    while index < data.len() {
        let end = match data[index] {
            0x07 if osc => Some(index + 1),
            0x1b if data.get(index + 1) == Some(&b'\\') => Some(index + 2),
            0x1b if index + 1 == data.len() => return None,
            _ => None,
        };
        if let Some(end) = end {
            let token = if osc {
                Token::Verbatim(&data[..end])
            } else {
                Token::Swallowed
            };
            return Some((token, end));
        }
        index += 1;
    }
    None
}

/// TerminalInput's encoding of a win32 key for VT input mode, for keys that
/// arrive as records while the terminal has not yet left win32-input-mode.
/// Normal (not application) cursor keys, as a terminal starts.
pub fn vt_text(record: &Record) -> Option<String> {
    if u16::from_le_bytes([record[0], record[1]]) != KEY_EVENT || record[4] == 0 {
        return None;
    }
    let vk = u16::from_le_bytes([record[10], record[11]]);
    let ch = u16::from_le_bytes([record[14], record[15]]);
    let state = u32::from_le_bytes(record[16..20].try_into().unwrap());
    let repeat = u16::from_le_bytes([record[8], record[9]]).max(1);
    let shift = state & SHIFT != 0;
    let alt = state & (LEFT_ALT | RIGHT_ALT) != 0;
    let ctrl = state & (LEFT_CTRL | RIGHT_CTRL) != 0;
    let modifier = 1 + u32::from(shift) + 2 * u32::from(alt) + 4 * u32::from(ctrl);
    let csi = |final_byte: char| {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{final_byte}")
        } else {
            format!("\x1b[{final_byte}")
        }
    };
    let tilde = |id: u32| {
        if modifier > 1 {
            format!("\x1b[{id};{modifier}~")
        } else {
            format!("\x1b[{id}~")
        }
    };
    let text = match vk {
        VK_UP => csi('A'),
        VK_DOWN => csi('B'),
        VK_RIGHT => csi('C'),
        VK_LEFT => csi('D'),
        VK_HOME => csi('H'),
        VK_END => csi('F'),
        VK_INSERT => tilde(2),
        VK_DELETE => tilde(3),
        VK_PRIOR => tilde(5),
        VK_NEXT => tilde(6),
        f if (VK_F1..VK_F1 + 4).contains(&f) => {
            let final_byte = (b'P' + (f - VK_F1) as u8) as char;
            if modifier > 1 {
                format!("\x1b[1;{modifier}{final_byte}")
            } else {
                format!("\x1bO{final_byte}")
            }
        }
        f if (VK_F1 + 4..VK_F1 + 12).contains(&f) => {
            const IDS: [u32; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde(IDS[usize::from(f - VK_F1 - 4)])
        }
        VK_TAB if shift => "\x1b[Z".to_owned(),
        VK_BACK => if alt { "\x1b\x7f" } else { "\x7f" }.to_owned(),
        _ if ch != 0 => {
            let mut text = String::new();
            if alt && state & (LEFT_CTRL | RIGHT_ALT) != (LEFT_CTRL | RIGHT_ALT) {
                text.push('\x1b');
            }
            text.push_str(&String::from_utf16_lossy(&[ch]));
            text
        }
        _ => return None,
    };
    Some(text.repeat(usize::from(repeat)))
}

#[cfg(test)]
#[path = "tests/keys.rs"]
mod tests;
