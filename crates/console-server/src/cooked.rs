// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! The cooked-read line editor: a port of OpenConsole's COOKED_READ_DATA
//! (readDataCooked.cpp), which edits a ReadConsole line with cmd.exe's keys
//! and redraws it as VT. Insert/overwrite, word and grapheme movement,
//! history (Up/Down/PgUp/PgDn/F5/F8), F1-F4 and F6 editing, the F2/F4/F7/F9
//! popups, Ctrl+Home/End deletion, Esc, the CtrlWakeupMask used for cmd's
//! Tab completion, and InitialNumBytes prefill all behave as they do there.
//!
//! This module is pure: it reads keys and screen geometry and produces VT,
//! leaving input queues, history storage and output to the caller.
use crate::history::{CommandHistory, Direction, Index};
use unicode_width::UnicodeWidthChar;

pub const NPOS: usize = usize::MAX;
const COMMAND_NUMBER_MAX: usize = 5;
pub const EXTKEY_ERASE_PREV_WORD: u16 = 0x7f;
const BACKSPACE: u16 = 0x08;
const CARRIAGE_RETURN: u16 = 0x0d;

pub mod vk {
    pub const ESCAPE: u16 = 0x1b;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const INSERT: u16 = 0x2d;
    pub const DELETE: u16 = 0x2e;
    pub const F1: u16 = 0x70;
    pub const F2: u16 = 0x71;
    pub const F3: u16 = 0x72;
    pub const F4: u16 = 0x73;
    pub const F5: u16 = 0x74;
    pub const F6: u16 = 0x75;
    pub const F7: u16 = 0x76;
    pub const F8: u16 = 0x77;
    pub const F9: u16 = 0x78;
    pub const F10: u16 = 0x79;
}
const ALT: u32 = 0x1 | 0x2;
const CTRL: u32 = 0x4 | 0x8;
const SHIFT: u32 = 0x10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Accumulating,
    DoneWithWakeupMask,
    DoneWithCarriageReturn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopupKind {
    CopyToChar,
    CopyFromChar,
    CommandNumber,
    CommandList,
}
#[derive(Clone, Debug)]
struct Popup {
    kind: PopupKind,
    number: Vec<u16>,
    height: i32,
    top: Index,
    selected: Index,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

#[derive(Default)]
struct Line {
    text: String,
    dirty_offset: usize,
    dirty_column: i32,
    columns: i32,
}

/// What the line editor needs from its console besides keys.
pub struct Context<'a> {
    pub history: Option<&'a mut CommandHistory>,
    pub processed: bool,
    /// Alt+F10 asks the caller to clear cmd.exe's aliases.
    pub clear_cmd_aliases: &'a mut bool,
}

pub struct CookedRead {
    buffer: Vec<u16>,
    dirty_beg: usize,
    cursor: usize,
    pub state: State,
    insert: bool,
    dirty: bool,
    clear_pending: bool,
    origin: Option<Point>,
    pager_prompt_end: Point,
    pager_content_top: i32,
    pager_height: i32,
    popups: Vec<Popup>,
    popup_opened: bool,
    wakeup_mask: u32,
    /// A wakeup character waiting for the erased tail to be redrawn.
    wakeup: Option<u16>,
    pub control_key_state: u32,
}

/// Width of the character starting at `units[index]` and how many units
/// its cluster spans: a base character and the zero-width characters the
/// terminal stacks onto it.
fn cluster(units: &[u16], index: usize) -> (usize, i32) {
    let mut chars = char::decode_utf16(units[index..].iter().copied());
    let Some(first) = chars.next() else {
        return (0, 0);
    };
    let first = first.unwrap_or('\u{fffd}');
    let mut length = first.len_utf16();
    for ch in chars {
        match ch {
            Ok(ch) if ch.width() == Some(0) => length += ch.len_utf16(),
            _ => break,
        }
    }
    (length, first.width().unwrap_or(1) as i32)
}
pub fn grapheme_next(units: &[u16], index: usize) -> usize {
    if index >= units.len() {
        return units.len();
    }
    index + cluster(units, index).0.max(1)
}
pub fn grapheme_prev(units: &[u16], index: usize) -> usize {
    let mut start = 0;
    let mut previous = 0;
    while start < index.min(units.len()) {
        previous = start;
        start = grapheme_next(units, start);
    }
    previous
}
/// DelimiterClass with the default (empty) word delimiter list.
fn delimiter_class(unit: u16) -> i32 {
    i32::from(unit == u16::from(b' '))
}
pub fn word_prev(units: &[u16], mut position: usize) -> usize {
    if position != 0 {
        position -= 1;
        while position != 0 && units[position] == u16::from(b' ') {
            position -= 1;
        }
        let class = delimiter_class(units[position]);
        while position != 0 && delimiter_class(units[position - 1]) == class {
            position -= 1;
        }
    }
    position
}
pub fn word_next(units: &[u16], mut position: usize) -> usize {
    if position < units.len() {
        position += 1;
        let class = delimiter_class(units[position - 1]);
        while position != units.len() && class == delimiter_class(units[position]) {
            position += 1;
        }
        while position != units.len() && units[position] == u16::from(b' ') {
            position += 1;
        }
    }
    position
}

/// FitTextIntoColumns: how many units of `text` fit into `columns`, and
/// how many columns they take. Wide characters are never split.
fn fit(text: &[u16], columns: i32) -> (usize, i32) {
    let (mut length, mut used) = (0, 0);
    while length < text.len() {
        let (units, width) = cluster(text, length);
        if used + width > columns {
            break;
        }
        length += units;
        used += width;
    }
    (length, used)
}

/// _layoutLine: appends what fits of `input[offset..]` from `column` up to
/// `limit`, showing control characters as ^X and tabs as spaces. Returns
/// the input offset reached and the column, which is `limit` when a wide
/// character had to move to the next row.
fn layout_line(
    output: &mut String,
    input: &[u16],
    offset: usize,
    column: i32,
    limit: i32,
) -> (usize, i32) {
    let mut index = offset.min(input.len());
    let mut column = column.min(limit);
    while index < input.len() && column < limit {
        let plain_end = input[index..]
            .iter()
            .position(|&unit| unit < 0x20)
            .map_or(input.len(), |end| index + end);
        if index != plain_end {
            let text = &input[index..plain_end];
            let (length, columns) = fit(text, limit - column);
            if length == 0 {
                // A wide glyph in a one-column terminal: skip it rather
                // than loop forever.
                index += grapheme_next(text, 0);
                break;
            }
            output.push_str(&String::from_utf16_lossy(&text[..length]));
            column += columns;
            index += length;
            if index != plain_end {
                return (index, limit);
            }
            if column >= limit {
                break;
            }
        }
        while index < input.len() && input[index] < 0x20 {
            let unit = input[index];
            let shown: String = if unit == u16::from(b'\t') {
                " ".repeat(((8 - (column & 7)).min(limit - column)) as usize)
            } else {
                format!("^{}", char::from(unit as u8 + b'@'))
            };
            let length = shown.len() as i32;
            if column + length > limit {
                output.push_str(&" ".repeat((limit - column) as usize));
                return (index, limit);
            }
            output.push_str(&shown);
            column += length;
            index += 1;
            if column >= limit {
                return (index, column);
            }
        }
    }
    (index, column)
}

fn cup(output: &mut String, position: Point) {
    output.push_str(&format!("\x1b[{};{}H", position.y + 1, position.x + 1));
}

impl CookedRead {
    /// `initial` is cmd's "I already wrote this" text after Tab completion;
    /// its start on screen is searched for so that it ends at `cursor`.
    pub fn new(initial: &[u16], wakeup_mask: u32, insert: bool, size: Size, cursor: Point) -> Self {
        let mut read = Self {
            buffer: Vec::new(),
            dirty_beg: NPOS,
            cursor: 0,
            state: State::Accumulating,
            insert,
            dirty: false,
            clear_pending: false,
            origin: None,
            pager_prompt_end: Point::default(),
            pager_content_top: 0,
            pager_height: 0,
            popups: Vec::new(),
            popup_opened: false,
            wakeup_mask,
            wakeup: None,
            control_key_state: 0,
        };
        if !initial.is_empty() && size.width > 0 {
            read.place_initial(initial, size, cursor);
        }
        read
    }

    fn place_initial(&mut self, initial: &[u16], size: Size, cursor: Point) {
        let remainder = (initial.len() % size.width as usize) as i32;
        let guess = (cursor.x - remainder + size.width) % size.width;
        let (mut best_distance, mut best_column, mut best_newlines) = (i32::MAX, 0, 0);
        for attempt in 0..=2 * size.width {
            // Offsets 0, 1, -1, 2, -2, ... around the guess.
            let offset = if attempt % 2 == 0 {
                -(attempt / 2)
            } else {
                attempt / 2 + 1
            };
            let begin = guess + offset;
            if begin < 0 || begin >= size.width {
                continue;
            }
            let (mut newlines, mut column, mut offset) = (0, begin, 0);
            let mut line = String::new();
            while offset < initial.len() {
                line.clear();
                (offset, column) = layout_line(&mut line, initial, offset, column, size.width);
                if column >= size.width {
                    column = 0;
                    newlines += 1;
                }
            }
            let distance = (column - cursor.x).abs();
            if distance < best_distance {
                (best_distance, best_column, best_newlines) = (distance, begin, newlines);
            }
            if distance == 0 {
                break;
            }
        }
        let mut origin = Point {
            x: best_column,
            y: cursor.y - best_newlines,
        };
        if origin.y < 0 {
            origin = Point::default();
        }
        self.buffer = initial.to_vec();
        self.dirty_beg = self.buffer.len();
        self.cursor = self.buffer.len();
        self.origin = Some(origin);
        self.pager_prompt_end = cursor;
        self.pager_height = size.height.min(best_newlines + 1);
    }

    /// After the redraw that erased the tail: the wakeup character joins the
    /// line at the cursor without being drawn.
    pub fn complete_wakeup(&mut self) {
        if let Some(ch) = self.wakeup.take() {
            let cursor = self.cursor.min(self.buffer.len());
            self.buffer.insert(cursor, ch);
            self.cursor = cursor + 1;
            self.dirty = false;
        }
    }
    pub fn has_popup(&self) -> bool {
        !self.popups.is_empty()
    }
    pub fn is_done(&self) -> bool {
        self.state != State::Accumulating
    }
    /// The edited line, as _handlePostCharInputLoop takes it.
    pub fn take_buffer(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.buffer)
    }

    fn replace(&mut self, offset: usize, remove: usize, input: &[u16]) {
        let size = self.buffer.len();
        let offset = offset.min(size);
        let remove = remove.min(size - offset);
        if remove == 0 && input.is_empty() {
            return;
        }
        self.buffer
            .splice(offset..offset + remove, input.iter().copied());
        self.cursor = offset + input.len();
        self.dirty_beg = self.dirty_beg.min(offset);
        self.dirty = true;
    }
    fn replace_all(&mut self, text: Vec<u16>) {
        self.buffer = text;
        self.cursor = self.buffer.len();
        self.dirty_beg = 0;
        self.dirty = true;
    }
    fn set_cursor(&mut self, position: usize) {
        self.cursor = position.min(self.buffer.len());
        self.dirty = true;
    }

    pub fn handle_char(&mut self, ch: u16, modifiers: u32, context: &mut Context<'_>) {
        if self.wakeup_mask != 0 && ch < 0x20 && self.wakeup_mask & (1 << ch) != 0 {
            // cmd's Tab completion: the tail is erased and redrawn first,
            // then the wakeup character goes into the result unprinted
            // (complete_wakeup, once the caller has redrawn).
            self.replace(self.cursor, NPOS, &[]);
            self.dirty = true;
            self.wakeup = Some(ch);
            self.control_key_state = modifiers;
            self.state = State::DoneWithWakeupMask;
            return;
        }
        match ch {
            CARRIAGE_RETURN => {
                self.set_cursor(NPOS);
                self.state = State::DoneWithCarriageReturn;
                return;
            }
            EXTKEY_ERASE_PREV_WORD | BACKSPACE if context.processed => {
                let cursor = self.cursor;
                let position = if ch == EXTKEY_ERASE_PREV_WORD {
                    word_prev(&self.buffer, cursor)
                } else {
                    grapheme_prev(&self.buffer, cursor)
                };
                self.replace(position, cursor - position, &[]);
                return;
            }
            _ => {}
        }
        let remove = if self.insert {
            0
        } else {
            grapheme_next(&self.buffer, self.cursor) - self.cursor
        };
        self.replace(self.cursor, remove, &[ch]);
    }

    pub fn handle_vkey(&mut self, vkey: u16, modifiers: u32, context: &mut Context<'_>) {
        let ctrl = modifiers & CTRL != 0;
        let alt = modifiers & ALT != 0;
        match vkey {
            vk::ESCAPE => {
                if !self.buffer.is_empty() {
                    self.replace(0, NPOS, &[]);
                }
            }
            vk::HOME if self.cursor > 0 => {
                if ctrl {
                    self.replace(0, self.cursor, &[]);
                }
                self.set_cursor(0);
            }
            vk::END if self.cursor < self.buffer.len() => {
                if ctrl {
                    self.replace(self.cursor, NPOS, &[]);
                }
                self.set_cursor(NPOS);
            }
            vk::LEFT if self.cursor != 0 => {
                let position = if ctrl {
                    word_prev(&self.buffer, self.cursor)
                } else {
                    grapheme_prev(&self.buffer, self.cursor)
                };
                self.set_cursor(position);
            }
            vk::F1 | vk::RIGHT => self.right(vkey, ctrl, context),
            vk::INSERT => self.insert = !self.insert,
            vk::DELETE if self.cursor < self.buffer.len() => {
                let end = grapheme_next(&self.buffer, self.cursor);
                self.replace(self.cursor, end - self.cursor, &[]);
            }
            vk::UP | vk::F5 => {
                if let Some(history) = context.history.as_deref_mut()
                    && !history.at_first()
                {
                    let text = history.retrieve(Direction::Previous);
                    self.replace_all(text);
                }
            }
            vk::DOWN => {
                if let Some(history) = context.history.as_deref_mut()
                    && !history.at_last()
                {
                    let text = history.retrieve(Direction::Next);
                    self.replace_all(text);
                }
            }
            vk::PRIOR => {
                if let Some(history) = context.history.as_deref_mut()
                    && !history.at_first()
                {
                    let text = history.retrieve_nth(0);
                    self.replace_all(text);
                }
            }
            vk::NEXT => {
                if let Some(history) = context.history.as_deref_mut()
                    && !history.at_last()
                {
                    let text = history.retrieve_nth(Index::MAX);
                    self.replace_all(text);
                }
            }
            vk::F2 if context.history.is_some() => self.popup_push(PopupKind::CopyToChar, None),
            vk::F3 => {
                if let Some(history) = context.history.as_deref() {
                    let last = history.last_command().to_vec();
                    if last.len() > self.cursor {
                        self.replace(self.cursor, NPOS, &last[self.cursor..]);
                    }
                }
            }
            vk::F4 => self.popup_push(PopupKind::CopyFromChar, None),
            vk::F6 => self.handle_char(0x1a, modifiers, context),
            vk::F7 if !ctrl && !alt => {
                if let Some(history) = context.history.as_deref()
                    && history.len() > 0
                {
                    self.popup_push(PopupKind::CommandList, Some(history.last_displayed));
                }
            }
            vk::F7 if alt => {
                if let Some(history) = context.history.as_deref_mut() {
                    history.empty();
                }
            }
            vk::F8 => {
                if let Some(history) = context.history.as_deref_mut() {
                    let position = self.cursor;
                    let prefix = self.buffer[..position].to_vec();
                    let start = history.last_displayed;
                    if let Some(index) = history.find(&prefix, start, false, false) {
                        let text = history.retrieve_nth(index);
                        self.replace_all(text);
                        self.set_cursor(position);
                    }
                }
            }
            vk::F9 => {
                if context
                    .history
                    .as_deref()
                    .is_some_and(|history| history.len() > 0)
                {
                    self.popup_push(PopupKind::CommandNumber, None);
                }
            }
            vk::F10 if alt => *context.clear_cmd_aliases = true,
            _ => {}
        }
    }

    /// Right at the end of the line copies the next character of the
    /// previous command, as F1 always does.
    fn right(&mut self, vkey: u16, ctrl: bool, context: &mut Context<'_>) {
        if self.cursor != self.buffer.len() {
            let position = if ctrl && vkey == vk::RIGHT {
                word_next(&self.buffer, self.cursor)
            } else {
                grapheme_next(&self.buffer, self.cursor)
            };
            self.set_cursor(position);
            return;
        }
        let Some(history) = context.history.as_deref() else {
            return;
        };
        let command = history.last_command().to_vec();
        let (mut buffer_index, mut command_index) = (0, 0);
        while command_index < command.len() {
            let end = grapheme_next(&command, command_index);
            if buffer_index >= self.buffer.len() {
                self.replace(NPOS, 0, &command[command_index..end]);
                break;
            }
            buffer_index = grapheme_next(&self.buffer, buffer_index);
            command_index = end;
        }
    }

    fn popup_push(&mut self, kind: PopupKind, selected: Option<Index>) {
        self.popups.push(Popup {
            kind,
            number: Vec::new(),
            height: 10,
            top: -1,
            selected: selected.unwrap_or(0),
        });
        self.dirty = true;
    }
    fn popups_done(&mut self) {
        self.popups.clear();
        self.dirty = true;
    }

    /// Keys while a popup is open: a character (`vkey == 0`) or a key.
    pub fn handle_popup(&mut self, ch: u16, vkey: u16, modifiers: u32, context: &mut Context<'_>) {
        let Some(kind) = self.popups.last().map(|popup| popup.kind) else {
            return;
        };
        match kind {
            PopupKind::CopyToChar => {
                if vkey != 0 {
                    if vkey == vk::ESCAPE {
                        self.popups_done();
                    }
                    return;
                }
                if let Some(history) = context.history.as_deref() {
                    let command = history.last_command().to_vec();
                    let cursor = self.cursor;
                    if let Some(found) = command.iter().skip(cursor).position(|&unit| unit == ch) {
                        let count = found;
                        self.replace(cursor, count, &command[cursor..cursor + count]);
                    }
                }
                self.popups_done();
            }
            PopupKind::CopyFromChar => {
                if vkey != 0 {
                    if vkey == vk::ESCAPE {
                        self.popups_done();
                    }
                    return;
                }
                let cursor = self.cursor;
                let end = self.buffer[cursor.min(self.buffer.len())..]
                    .iter()
                    .position(|&unit| unit == ch)
                    .map_or(self.buffer.len(), |found| cursor + found);
                self.replace(cursor, end - cursor, &[]);
                self.popups_done();
            }
            PopupKind::CommandNumber => self.popup_command_number(ch, vkey, context),
            PopupKind::CommandList => self.popup_command_list(ch, vkey, modifiers, context),
        }
    }

    fn popup_command_number(&mut self, ch: u16, vkey: u16, context: &mut Context<'_>) {
        if vkey != 0 {
            if vkey == vk::ESCAPE {
                self.popups_done();
            }
            return;
        }
        let popup = self.popups.last_mut().unwrap();
        if ch == CARRIAGE_RETURN {
            let number = String::from_utf16_lossy(&popup.number)
                .parse::<Index>()
                .ok();
            if let (Some(number), Some(history)) = (number, context.history.as_deref_mut()) {
                let text = history.retrieve_nth(number);
                self.replace_all(text);
            }
            self.popups_done();
        } else if (u16::from(b'0')..=u16::from(b'9')).contains(&ch) {
            if popup.number.len() < COMMAND_NUMBER_MAX {
                popup.number.push(ch);
                self.dirty = true;
            }
        } else if ch == BACKSPACE && popup.number.pop().is_some() {
            self.dirty = true;
        }
    }

    fn popup_command_list(
        &mut self,
        ch: u16,
        vkey: u16,
        modifiers: u32,
        context: &mut Context<'_>,
    ) {
        let Some(history) = context.history.as_deref_mut() else {
            self.popups_done();
            return;
        };
        let popup = self.popups.last_mut().unwrap();
        if ch == CARRIAGE_RETURN {
            let text = history.retrieve_nth(popup.selected);
            self.replace_all(text);
            self.popups_done();
            self.handle_char(CARRIAGE_RETURN, modifiers, context);
            return;
        }
        let shift = modifiers & SHIFT != 0;
        match vkey {
            vk::ESCAPE => return self.popups_done(),
            vk::F9 => return self.popup_push(PopupKind::CommandNumber, None),
            vk::DELETE => {
                history.remove(popup.selected);
                if history.len() <= 0 {
                    return self.popups_done();
                }
            }
            vk::LEFT | vk::RIGHT => {
                let text = history.retrieve_nth(popup.selected);
                self.replace_all(text);
                return self.popups_done();
            }
            vk::UP => {
                if shift {
                    history.swap(popup.selected, popup.selected - 1);
                }
                popup.selected -= 1;
            }
            vk::DOWN => {
                if shift {
                    history.swap(popup.selected, popup.selected + 1);
                }
                popup.selected += 1;
            }
            vk::HOME => popup.selected = 0,
            vk::END => popup.selected = Index::MAX,
            vk::PRIOR => popup.selected = popup.selected.saturating_sub(popup.height),
            vk::NEXT => popup.selected = popup.selected.saturating_add(popup.height),
            _ => return,
        }
        self.dirty = true;
    }

    /// The origin is taken lazily: an application that starts its read
    /// before printing its prompt has the prompt measured too.
    fn origin(&mut self, cursor: Point) -> Point {
        *self.origin.get_or_insert(cursor)
    }

    /// _redisplay: the VT that brings the screen up to date with the line,
    /// or None when nothing changed or echo is off. `cursor` is the screen
    /// cursor, used once for the origin; `popup_sgr` draws popups.
    pub fn redisplay(
        &mut self,
        size: Size,
        cursor: Point,
        echo: bool,
        popup_sgr: &str,
        history: Option<&CommandHistory>,
    ) -> Option<String> {
        if !self.dirty || !echo || size.width <= 0 || size.height <= 0 {
            return None;
        }
        let mut origin = self.origin(cursor);
        let mut origin_final = origin;
        let mut cursor_final;
        let mut prompt_end;
        let mut lines: Vec<Line>;
        loop {
            cursor_final = Point { x: origin.x, y: 0 };
            lines = vec![Line {
                dirty_column: cursor_final.x,
                columns: cursor_final.x,
                ..Default::default()
            }];
            let mut column = cursor_final.x;
            let offsets = [
                0,
                self.dirty_beg.min(self.cursor),
                self.dirty_beg.max(self.cursor),
                NPOS,
            ];
            for segment_index in 0..3 {
                let from = offsets[segment_index].min(self.buffer.len());
                let to = offsets[segment_index + 1].min(self.buffer.len());
                let segment = &self.buffer[from.min(to)..to];
                if segment.is_empty() {
                    continue;
                }
                let dirty = offsets[segment_index] >= self.dirty_beg;
                let mut begin = 0;
                while begin < segment.len() {
                    if column >= size.width {
                        lines.push(Line::default());
                    }
                    let line = lines.last_mut().unwrap();
                    let (offset, end_column) =
                        layout_line(&mut line.text, segment, begin, line.columns, size.width);
                    line.columns = end_column;
                    column = end_column;
                    if !dirty {
                        line.dirty_offset = line.text.len();
                        line.dirty_column = end_column;
                    }
                    begin = offset;
                }
                if offsets[segment_index + 1] == self.cursor {
                    cursor_final = Point {
                        x: column,
                        y: lines.len() as i32 - 1,
                    };
                }
            }
            prompt_end = Point {
                x: column,
                y: lines.len() as i32 - 1,
            };
            if prompt_end.y <= self.pager_prompt_end.y {
                let end_x = if self.pager_prompt_end.y == prompt_end.y {
                    self.pager_prompt_end.x
                } else {
                    size.width
                };
                let remaining = end_x - prompt_end.x;
                if remaining > 0 {
                    let line = lines.last_mut().unwrap();
                    if remaining <= 16 {
                        line.text.push_str(&" ".repeat(remaining as usize));
                        line.columns += remaining;
                    } else {
                        line.text.push_str("\x1b[K");
                    }
                }
            }
            if !self.popups.is_empty() {
                let last = lines.last_mut().unwrap();
                last.text.push_str("\r\n");
                last.columns = size.width;
                self.draw_popup(&mut lines, size, popup_sgr, history);
                let last = lines.last().unwrap();
                cursor_final = Point {
                    x: last.columns,
                    y: lines.len() as i32 - 1,
                };
            } else if cursor_final.x >= size.width {
                cursor_final = Point {
                    x: 0,
                    y: cursor_final.y + 1,
                };
                if self.cursor == self.buffer.len() {
                    let mut remaining = (self.pager_prompt_end.y - prompt_end.y) * size.width
                        + self.pager_prompt_end.x
                        - prompt_end.x;
                    remaining = remaining.max(1);
                    let mut line = Line::default();
                    if remaining <= 16 {
                        line.text.push_str(&" ".repeat(remaining as usize));
                        line.text.push('\r');
                    } else {
                        line.text.push_str(" \r\x1b[K");
                    }
                    lines.push(line);
                }
            }
            if lines.len() as i32 > size.height && origin_final.x != 0 {
                self.dirty_beg = 0;
                origin.x = 0;
                origin_final = Point::default();
                continue;
            }
            break;
        }
        Some(self.emit(lines, size, origin, origin_final, cursor_final, prompt_end))
    }

    /// The second half of _redisplay: scrolling the pager, writing the dirty
    /// parts of each row and parking the cursor.
    fn emit(
        &mut self,
        mut lines: Vec<Line>,
        size: Size,
        origin: Point,
        mut origin_final: Point,
        mut cursor_final: Point,
        prompt_end: Point,
    ) -> String {
        let line_count = lines.len() as i32;
        let pager_height = line_count.min(size.height);
        origin_final.y = origin_final.y.min(size.height - pager_height);
        let mut content_top = self.pager_content_top;
        content_top = content_top.min(cursor_final.y);
        content_top = content_top.max(cursor_final.y - size.height + 1);
        content_top = content_top.clamp(0, line_count - pager_height);
        cursor_final.y += origin_final.y - content_top;
        let mut output = String::new();
        if self.clear_pending {
            self.clear_pending = false;
            cup(&mut output, origin);
            output.push_str("\x1b[J");
        }
        let popup_opened = !self.popups.is_empty();
        if self.popup_opened != popup_opened {
            // DECSC and hide the cursor with a popup; DECRC and show it after.
            output.push_str(if popup_opened {
                "\x1b7\x1b[?25l\x1b[#{\x1b[m\x1b[#}"
            } else {
                "\x1b8\x1b[?25h"
            });
            self.popup_opened = popup_opened;
        }
        let delta = content_top - self.pager_content_top;
        if delta != 0 && origin == Point::default() {
            let magnitude = delta.abs();
            let (mut begin, mut end) = (0, pager_height);
            if magnitude < self.pager_height.min(pager_height) {
                begin = if delta >= 0 {
                    pager_height - magnitude
                } else {
                    0
                };
                end = if delta >= 0 { pager_height } else { magnitude };
                output.push_str(&format!(
                    "\x1b[{magnitude}{}",
                    if delta >= 0 { 'S' } else { 'T' }
                ));
            } else {
                let last = &mut lines[(pager_height - 1 + content_top) as usize];
                if last.columns < size.width {
                    last.text.push_str("\x1b[K");
                }
            }
            for row in begin..end {
                let line = &mut lines[(row + content_top) as usize];
                line.dirty_offset = 0;
                line.dirty_column = 0;
            }
        }
        let any_dirty = (0..pager_height).any(|row| {
            let line = &lines[(row + content_top) as usize];
            line.dirty_offset < line.text.len()
        });
        if any_dirty {
            let mut write = Point { x: -1, y: -1 };
            for row in 0..pager_height {
                let screen_row = (origin.y + row).min(size.height - 1);
                if write.x >= size.width {
                    write = Point {
                        x: 0,
                        y: screen_row,
                    };
                }
                let line = &lines[(row + content_top) as usize];
                if line.dirty_column >= size.width {
                    continue;
                }
                let position = Point {
                    x: line.dirty_column,
                    y: screen_row,
                };
                if write != position {
                    write = position;
                    cup(&mut output, position);
                }
                output.push_str(&line.text[line.dirty_offset.min(line.text.len())..]);
                write.x = line.columns;
            }
        }
        let previous_height = self.pager_height.min(size.height);
        if pager_height < previous_height {
            let row = (origin.y + pager_height).min(size.height - 1);
            cup(&mut output, Point { x: 0, y: row });
            output.push_str("\x1b[K");
            for _ in pager_height + 1..previous_height {
                output.push_str("\x1b[E\x1b[K");
            }
        }
        cup(&mut output, cursor_final);
        self.origin = Some(origin_final);
        self.pager_prompt_end = prompt_end;
        self.pager_content_top = content_top;
        self.pager_height = pager_height;
        self.dirty_beg = self.buffer.len();
        self.dirty = false;
        output
    }

    fn draw_prompt(
        lines: &mut Vec<Line>,
        width: i32,
        popup_sgr: &str,
        prefix: &str,
        text: &str,
        suffix: &[u16],
    ) {
        let mut units: Vec<u16> = prefix.encode_utf16().chain(text.encode_utf16()).collect();
        units.extend_from_slice(suffix);
        let mut line = String::from("\x1b[#{\x1b[K");
        line.push_str(popup_sgr);
        let (_, column) = layout_line(&mut line, &units, 0, 0, width);
        line.push_str("\x1b[m\x1b[#}");
        lines.push(Line {
            text: line,
            columns: column,
            ..Default::default()
        });
    }

    fn draw_popup(
        &mut self,
        lines: &mut Vec<Line>,
        size: Size,
        popup_sgr: &str,
        history: Option<&CommandHistory>,
    ) {
        let popup = self.popups[0].clone();
        let number_text = |popup: &Popup| {
            let mut text = popup.number.clone();
            text.resize(COMMAND_NUMBER_MAX, u16::from(b' '));
            text
        };
        match popup.kind {
            PopupKind::CopyToChar => {
                Self::draw_prompt(
                    lines,
                    size.width,
                    popup_sgr,
                    "",
                    "Enter char to copy up to: ",
                    &[],
                );
            }
            PopupKind::CopyFromChar => {
                Self::draw_prompt(
                    lines,
                    size.width,
                    popup_sgr,
                    "",
                    "Enter char to delete up to: ",
                    &[],
                );
            }
            PopupKind::CommandNumber => {
                let suffix = number_text(&popup);
                Self::draw_prompt(
                    lines,
                    size.width,
                    popup_sgr,
                    "",
                    "Enter command number: ",
                    &suffix,
                );
            }
            PopupKind::CommandList => {
                if let Some(history) = history {
                    self.draw_command_list(lines, size, popup_sgr, history);
                }
            }
        }
    }

    fn draw_command_list(
        &mut self,
        lines: &mut Vec<Line>,
        size: Size,
        popup_sgr: &str,
        history: &CommandHistory,
    ) {
        let stacked = self.popups.len() == 2 && self.popups[1].kind == PopupKind::CommandNumber;
        let stacked_number = stacked.then(|| {
            let mut text = self.popups[1].number.clone();
            text.resize(COMMAND_NUMBER_MAX, u16::from(b' '));
            text
        });
        let list = &mut self.popups[0];
        let count = history.len();
        let index_width = count.to_string().len() as i32;
        let height = count.min((size.height / 2 - 1).min(20));
        if height < 1 {
            return;
        }
        list.selected = list.selected.clamp(0, count - 1);
        if list.top < 0 {
            list.top = (list.selected - height / 2).max(0);
        }
        list.top = list.top.min(list.selected);
        list.top = list.top.max(list.selected - height + 1);
        list.top = list.top.clamp(0, count - height);
        list.height = height;
        let max = count - 1;
        let track_max = height - 3;
        let track = if max <= 0 {
            0
        } else {
            1 + (track_max * list.selected + max / 2) / max
        };
        for row in 0..height {
            let index = list.top + row;
            let command = history.nth(index);
            let selected = index == list.selected && !stacked;
            let mut line = String::from("\x1b[#{\x1b[K");
            line.push_str(popup_sgr);
            let bar = if count > height {
                if row == 0 {
                    '▴'
                } else if row == height - 1 {
                    '▾'
                } else if row == track {
                    '█'
                } else {
                    '▒'
                }
            } else {
                ' '
            };
            line.push(bar);
            if selected {
                line.push('▸');
            } else {
                line.push_str("\x1b[m\x1b[#} ");
            }
            line.push_str(&format!("{index:>width$}: ", width = index_width as usize));
            layout_line(&mut line, command, 0, index_width + 4, size.width);
            if selected {
                line.push_str("\x1b[m\x1b[#}");
            }
            line.push_str("\r\n");
            lines.push(Line {
                text: line,
                columns: size.width,
                ..Default::default()
            });
        }
        if let Some(suffix) = stacked_number {
            Self::draw_prompt(
                lines,
                size.width - 1,
                popup_sgr,
                "╰",
                "Enter command number: ",
                &suffix,
            );
        } else if let Some(last) = lines.last_mut() {
            let length = last.text.len();
            last.text.truncate(length - 2);
        }
    }
}

#[cfg(test)]
#[path = "tests/cooked.rs"]
mod tests;
