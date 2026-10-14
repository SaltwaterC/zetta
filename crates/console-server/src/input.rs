//! Pending console reads and the keyboard/paste input queue. Requests are
//! owned until completed or cancelled; servicing input never holds the
//! parser/grid lock. Text reads, raw and cooked, are in read.rs.
use crate::codepage;
use crate::editing::SharedEditing;
use crate::keys;
use crate::native::{self, Handle};
use crate::output::{ConsoleOutput, OutputMode};
use crate::protocol::*;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub type SharedOutput = Arc<Mutex<Box<dyn ConsoleOutput>>>;
pub(crate) struct Request {
    pub descriptor: Descriptor,
    pub body: Vec<u8>,
    pub records: bool,
    pub unicode: bool,
    pub peek: bool,
    pub nowait: bool,
    pub read: ReadParams,
    /// The line being edited, kept while the read waits for more keys.
    pub cooked: Option<Box<crate::cooked::CookedRead>>,
    pub control_key_state: u32,
    /// The read found nothing and waited for input.
    pub waited: bool,
}
/// What a ReadConsole call asks for besides its buffer.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReadParams {
    /// The executable whose aliases apply to the line.
    pub exe: Vec<u16>,
    /// Text the caller already echoed (cmd after Tab completion).
    pub initial: Vec<u16>,
    pub wakeup_mask: u32,
    pub control_z: bool,
}
struct State {
    records: VecDeque<[u8; 20]>,
    pending: std::collections::HashMap<usize, read::Pending>,
    /// CONSOLE_IGNORE_NEXT_KEYUP: the Enter release after a cooked read.
    ignore_next_keyup: bool,
    /// Records of the first input event to reach an empty queue: a read
    /// already waiting completes with those alone, as OpenConsole wakes
    /// waiting reads after each event it writes.
    first_event: Option<usize>,
    requests: VecDeque<Request>,
    keys: keys::Decoder,
    closed: bool,
    clients: std::collections::BTreeSet<usize>,
    handles: std::collections::BTreeSet<usize>,
    /// Control keys the input thread has yet to apply to pending reads.
    alert: Controls,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    pub mode: AtomicU32,
    pub output_mode: AtomicU32,
    /// Input code page, for ANSI reads.
    code_page: AtomicU32,
    server: Arc<Handle>,
    event: Handle,
    output: SharedOutput,
    readiness: Option<Arc<dyn Fn(bool) + Send + Sync>>,
    /// Attached clients by connection token, so newest is last.
    processes: Mutex<std::collections::BTreeMap<usize, u32>>,
    editing: SharedEditing,
    /// SGR for the line editor's popups, from the popup attributes.
    popup_sgr: Mutex<String>,
}
#[derive(Clone)]
pub(crate) struct Input(Arc<Shared>);
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
impl Input {
    pub fn new(
        server: Arc<Handle>,
        event: Handle,
        output: SharedOutput,
        readiness: Option<Arc<dyn Fn(bool) + Send + Sync>>,
        editing: SharedEditing,
    ) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                records: VecDeque::new(),
                pending: Default::default(),
                ignore_next_keyup: false,
                first_event: None,
                requests: VecDeque::new(),
                keys: keys::Decoder::default(),
                closed: false,
                clients: Default::default(),
                handles: Default::default(),
                alert: Controls::default(),
            }),
            changed: Condvar::new(),
            mode: AtomicU32::new(0x1f7),
            output_mode: AtomicU32::new(7),
            code_page: AtomicU32::new(codepage::UTF8),
            server,
            event,
            output,
            readiness,
            processes: Mutex::new(Default::default()),
            editing,
            popup_sgr: Mutex::new(crate::legacy::sgr(0xf5, 0x07)),
        }))
    }
    pub fn set_popup_sgr(&self, sgr: String) {
        *lock(&self.0.popup_sgr) = sgr;
    }
    fn input_ready(&self, ready: bool) {
        if let Some(callback) = &self.0.readiness {
            callback(ready);
        }
    }
    pub fn connected(&self, process: usize, pid: u32) {
        lock(&self.0.state).clients.insert(process);
        lock(&self.0.processes).insert(process, pid);
    }
    /// Attached clients, newest first, the order control events go in.
    pub fn client_pids(&self) -> Vec<u32> {
        lock(&self.0.processes).values().rev().copied().collect()
    }
    pub fn disconnected(&self, process: usize, _pid: u32) {
        lock(&self.0.processes).remove(&process);
        let mut state = lock(&self.0.state);
        state.clients.remove(&process);
        state
            .requests
            .retain(|request| request.descriptor.process != process);
        self.0.changed.notify_all();
    }
    pub fn object_opened(&self, object: usize) {
        lock(&self.0.state).handles.insert(object);
    }
    /// A closing handle ends its reads (HandleClosing: STATUS_ALERTED) and
    /// forgets the text it was owed.
    pub fn object_closed(&self, object: usize) {
        let mut state = lock(&self.0.state);
        state.handles.remove(&object);
        state.pending.remove(&object);
        let (closed, kept): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut state.requests)
            .into_iter()
            .partition(|request| request.descriptor.object == object);
        state.requests = kept;
        drop(state);
        for mut request in closed {
            self.finish_empty(&mut request, STATUS_ALERTED);
        }
        self.0.changed.notify_all();
    }
    pub fn input_mode(&self) -> u32 {
        self.0.mode.load(Ordering::Acquire)
    }
    pub fn set_modes(&self, input: u32, output: u32) {
        self.0.mode.store(input, Ordering::Release);
        self.0.output_mode.store(output, Ordering::Release);
    }
    pub fn set_code_page(&self, code_page: u32) {
        self.0.code_page.store(code_page, Ordering::Release);
    }
    fn code_page(&self) -> u32 {
        self.0.code_page.load(Ordering::Acquire)
    }
    pub fn ready(&self) -> usize {
        lock(&self.0.state).records.len()
    }
    pub fn request(&self, request: Request) -> bool {
        let mut state = lock(&self.0.state);
        if state.closed || state.requests.len() >= 256 {
            return false;
        }
        state.requests.push_back(request);
        self.0.changed.notify_all();
        true
    }
    pub fn resize(&self, columns: u16, rows: u16) {
        let mut record = [0; 20];
        record[..2].copy_from_slice(&4u16.to_le_bytes());
        record[4..6].copy_from_slice(&columns.to_le_bytes());
        record[6..8].copy_from_slice(&rows.to_le_bytes());
        let mut state = lock(&self.0.state);
        if self.input_mode() & 8 == 0 || state.closed {
            return;
        }
        if let Some(last) = state
            .records
            .back_mut()
            .filter(|last| last[..2] == 4u16.to_le_bytes())
        {
            *last = record;
        } else if state.records.len() < 65536 {
            state.records.push_back(record);
        }
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.0.event.raw());
        }
        self.0.changed.notify_all();
    }
    /// WriteConsoleInput: records an application queues itself, after or
    /// before what is waiting. In VT input mode a key press becomes its VT
    /// text, as InputBuffer::_WriteBuffer encodes it; Ctrl+C is not special.
    pub fn inject(&self, records: &[[u8; 20]], append: bool) -> usize {
        let vt = self.input_mode() & VIRTUAL_TERMINAL_INPUT != 0;
        let mut queued = Vec::with_capacity(records.len());
        for record in records {
            match keys::vt_text(record).filter(|_| vt) {
                Some(text) => queued.extend(text.encode_utf16().map(record_char)),
                None => queued.push(*record),
            }
        }
        let mut state = lock(&self.0.state);
        if state.closed {
            return 0;
        }
        if append {
            state.records.extend(queued);
        } else {
            for record in queued.into_iter().rev() {
                state.records.push_front(record);
            }
        }
        drop(state);
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.0.event.raw());
        }
        self.0.changed.notify_all();
        records.len()
    }
    pub fn close(&self) {
        lock(&self.0.state).closed = true;
        self.0.changed.notify_all();
    }
    pub fn write(&self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = lock(&self.0.state);
        if state.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if state.records.len() >= 65536 {
            drop(state);
            self.input_ready(false);
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = bytes.len().min(4096).min(65536 - state.records.len());
        let mode = self.input_mode();
        let controls = decode_input(
            &mut state,
            &bytes[..count],
            mode & 1 != 0,
            mode & VIRTUAL_TERMINAL_INPUT != 0,
        );
        drop(state);
        if controls.any() {
            let event = if controls.ctrl_break {
                CTRL_BREAK
            } else {
                CTRL_C
            };
            for pid in self.client_pids() {
                let _ = crate::control::send_ctrl_event(pid, event, 0)?;
            }
        }
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.0.event.raw());
        }
        self.0.changed.notify_all();
        Ok(count)
    }
    pub fn run(self) {
        loop {
            let request = {
                let mut state = lock(&self.0.state);
                loop {
                    if state.closed {
                        break state.requests.pop_front();
                    }
                    if state.alert.any() {
                        let alerted = self.take_alerted(&mut state);
                        drop(state);
                        for mut request in alerted {
                            self.finish_empty(&mut request, STATUS_ALERTED);
                        }
                        state = lock(&self.0.state);
                        continue;
                    }
                    let owed = state.requests.front().is_some_and(|r| {
                        !r.records && state.pending.contains_key(&r.descriptor.object)
                    });
                    if !state.requests.is_empty()
                        && (!state.records.is_empty()
                            || owed
                            || state.requests.front().is_some_and(|r| r.nowait))
                    {
                        break state.requests.pop_front();
                    }
                    // Reads queued now are waiting for input.
                    for request in &mut state.requests {
                        request.waited = true;
                    }
                    state = self
                        .0
                        .changed
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            };
            let Some(mut request) = request else {
                break;
            };
            let closed = lock(&self.0.state).closed;
            if closed {
                self.cancel(&mut request);
                continue;
            }
            match self.fulfill(&mut request) {
                Ok(Some(bytes)) => {
                    let _ = self.complete(&mut request, bytes);
                }
                Ok(None) => {
                    let mut state = lock(&self.0.state);
                    if !state.clients.contains(&request.descriptor.process)
                        || !state.handles.contains(&request.descriptor.object)
                    {
                        continue;
                    }
                    request.waited = true;
                    state.requests.push_front(request);
                    // Wait until more records arrive, even if a cooked line is partial.
                    let old = state.records.len();
                    while !state.closed
                        && !state.requests.is_empty()
                        && !state.alert.any()
                        && state.records.len() == old
                    {
                        state = self
                            .0
                            .changed
                            .wait(state)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                    self.finish_error(&mut request, 0xc0000023u32 as i32);
                }
                Err(_) => self.cancel(&mut request),
            }
        }
    }
    /// Removes the reads a pending Ctrl+C/Ctrl+Break ends, as OpenConsole's
    /// TerminateRead: cooked reads end on either, raw reads only on Break, and
    /// ReadConsoleInput ignores both. An unterminated cooked line is discarded.
    fn take_alerted(&self, state: &mut State) -> VecDeque<Request> {
        let alert = std::mem::take(&mut state.alert);
        let cooked = self.input_mode() & ENABLE_LINE_INPUT != 0;
        let (alerted, kept): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut state.requests)
            .into_iter()
            .partition(|request| !request.records && (cooked || alert.ctrl_break));
        state.requests = kept;
        alerted
    }
    fn fulfill(&self, request: &mut Request) -> io::Result<Option<Vec<u8>>> {
        let capacity = request.descriptor.output_size as usize - request.body.len();
        let mut state = lock(&self.0.state);
        if !state.clients.contains(&request.descriptor.process)
            || !state.handles.contains(&request.descriptor.object)
        {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        // The run loop applies the alert first; consuming input now would let
        // keys typed after Ctrl+C join the line it discards.
        if state.alert.any() {
            return Ok(None);
        }
        if request.records {
            state.first_event = None;
            // The Enter release that finished a cooked read is not input.
            if std::mem::take(&mut state.ignore_next_keyup)
                && state
                    .records
                    .front()
                    .is_some_and(|record| record[..2] == [1, 0] && u32_at(record, 4) == 0)
            {
                state.records.pop_front();
            }
            let (bytes, count) = if request.unicode {
                let count = (capacity / 20).min(state.records.len());
                let bytes = state
                    .records
                    .iter()
                    .take(count)
                    .flatten()
                    .copied()
                    .collect();
                (bytes, count)
            } else {
                ansi_records(state.records.iter(), capacity / 20, self.code_page())
            };
            if count == 0 && !request.nowait {
                return Ok(None);
            }
            if !request.peek {
                state.records.drain(..count);
            }
            let input_ready = state.records.len() < 65536;
            drop(state);
            self.input_ready(input_ready);
            return Ok(Some(bytes));
        }
        drop(state);
        self.read_text(request)
    }
    fn complete(&self, request: &mut Request, mut bytes: Vec<u8>) -> io::Result<()> {
        if !request.body.is_empty() {
            let offset = if request.records { 0 } else { 16 };
            let count = if request.records {
                bytes.len() / 20
            } else {
                bytes.len()
            };
            set_u32(&mut request.body, offset, count as u32);
            if !request.records && request.body.len() >= 16 {
                // CONSOLE_READCONSOLE_MSG.ControlKeyState, for wakeup keys.
                set_u32(&mut request.body, 12, request.control_key_state);
            }
        }
        native::transfer(
            &self.0.server,
            WRITE_OUTPUT,
            request.descriptor.id,
            request.body.len() as u32,
            &mut bytes,
        )?;
        let completion = Completion::new(request.descriptor.id, 0, bytes.len(), &mut request.body);
        native::ioctl(
            &self.0.server,
            COMPLETE_IO,
            (&completion as *const Completion).cast(),
            size_of::<Completion>(),
            std::ptr::null_mut(),
            0,
        )?;
        let state = lock(&self.0.state);
        if state.records.is_empty() {
            unsafe {
                windows_sys::Win32::System::Threading::ResetEvent(self.0.event.raw());
            }
        }
        Ok(())
    }
    fn cancel(&self, request: &mut Request) {
        self.finish_error(request, 0xc0000120u32 as i32);
    }
    /// Completes a read with no data but its message body, so the client sees
    /// a zero count rather than the one it sent.
    fn finish_empty(&self, request: &mut Request, status: i32) {
        if !request.body.is_empty() {
            let offset = if request.records { 0 } else { 16 };
            set_u32(&mut request.body, offset, 0);
        }
        let completion = Completion::new(request.descriptor.id, status, 0, &mut request.body);
        let _ = native::ioctl(
            &self.0.server,
            COMPLETE_IO,
            (&completion as *const Completion).cast(),
            size_of::<Completion>(),
            std::ptr::null_mut(),
            0,
        );
    }
    /// FlushConsoleInputBuffer: queued records go; pending reads stay pending.
    pub fn flush(&self) {
        let mut state = lock(&self.0.state);
        state.records.clear();
        state.first_event = None;
        unsafe {
            windows_sys::Win32::System::Threading::ResetEvent(self.0.event.raw());
        }
        drop(state);
        self.input_ready(true);
    }
    fn finish_error(&self, request: &mut Request, status: i32) {
        let completion = Completion::new(request.descriptor.id, status, 0, &mut []);
        let _ = native::ioctl(
            &self.0.server,
            COMPLETE_IO,
            (&completion as *const Completion).cast(),
            size_of::<Completion>(),
            std::ptr::null_mut(),
            0,
        );
    }
}
const STATUS_ALERTED: i32 = 0x101;
const ENABLE_LINE_INPUT: u32 = 0x2;
const CTRL_C: u32 = 0;
const CTRL_BREAK: u32 = 1;
fn record(vk: u16, scan: u16, ch: u16, down: u32, control: u32, repeat: u16) -> [u8; 20] {
    let mut bytes = [0; 20];
    bytes[..2].copy_from_slice(&1u16.to_le_bytes());
    set_u32(&mut bytes, 4, down);
    bytes[8..10].copy_from_slice(&repeat.to_le_bytes());
    bytes[10..12].copy_from_slice(&vk.to_le_bytes());
    bytes[12..14].copy_from_slice(&scan.to_le_bytes());
    bytes[14..16].copy_from_slice(&ch.to_le_bytes());
    set_u32(&mut bytes, 16, control);
    bytes
}
/// Control keys found while decoding; they are consumed, never queued.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Controls {
    pub ctrl_c: bool,
    pub ctrl_break: bool,
}
/// OpenConsole's HandleGenericKeyEvent: Ctrl+C only in processed mode, while
/// Ctrl+Break always applies. Either key is consumed rather than queued.
fn control_key(bytes: &[u8; 20], processed: bool) -> Controls {
    let mut controls = Controls::default();
    if u32_at(bytes, 4) != 0 {
        let vk = u16::from_le_bytes([bytes[10], bytes[11]]);
        let ch = u16::from_le_bytes([bytes[14], bytes[15]]);
        let state_flags = u32_at(bytes, 16);
        let ctrl = state_flags & (LEFT_CTRL | RIGHT_CTRL) != 0;
        let alt = state_flags & (LEFT_ALT | RIGHT_ALT) != 0;
        if ctrl && vk == VK_CANCEL {
            controls.ctrl_break = true;
        } else if processed && (ch == 3 || ctrl && !alt && vk == u16::from(b'C')) {
            controls.ctrl_c = true;
        }
    }
    controls
}
/// Ctrl+Break also discards everything queued before it.
fn apply_control(state: &mut State, controls: Controls) {
    if controls.ctrl_break {
        state.records.clear();
        state.first_event = None;
        state.alert.ctrl_break = true;
    } else if controls.ctrl_c {
        state.alert.ctrl_c = true;
    }
}
impl std::ops::BitOrAssign for Controls {
    fn bitor_assign(&mut self, other: Self) {
        self.ctrl_c |= other.ctrl_c;
        self.ctrl_break |= other.ctrl_break;
    }
}
impl Controls {
    fn any(self) -> bool {
        self.ctrl_c || self.ctrl_break
    }
}
const RIGHT_ALT: u32 = 0x1;
const LEFT_ALT: u32 = 0x2;
const RIGHT_CTRL: u32 = 0x4;
const LEFT_CTRL: u32 = 0x8;
const VK_CANCEL: u16 = 0x03;
const VIRTUAL_TERMINAL_INPUT: u32 = 0x200;
const INSERT_MODE: u32 = 0x20;
/// Applies terminal input to the buffer as InteractDispatch does: win32
/// keys pass the Ctrl+C/Ctrl+Break check, synthesized records are queued,
/// and text is stored as character key-downs. In VT input mode a win32 key
/// that slipped in before the terminal left win32-input-mode is re-encoded.
fn decode_input(state: &mut State, bytes: &[u8], processed: bool, vt: bool) -> Controls {
    let mut controls = Controls::default();
    let was_empty = state.records.is_empty();
    for event in state.keys.decode(bytes, vt) {
        if was_empty && state.first_event.is_none() && !state.records.is_empty() {
            state.first_event = Some(state.records.len());
        }
        match event {
            keys::Event::Records(records) => state.records.extend(records),
            keys::Event::Key(record) => {
                let found = control_key(&record, processed);
                if found.any() {
                    apply_control(state, found);
                    controls |= found;
                } else if vt {
                    if let Some(text) = keys::vt_text(&record) {
                        push_raw(state, &text);
                    }
                } else {
                    state.records.push_back(record);
                }
            }
            keys::Event::Raw(text) => push_raw(state, &text),
        }
    }
    if was_empty && state.first_event.is_none() && !state.records.is_empty() {
        state.first_event = Some(state.records.len());
    }
    controls
}
/// InputBuffer::_writeString: one key-down per UTF-16 unit, no virtual key.
fn push_raw(state: &mut State, text: &str) {
    state.records.extend(text.encode_utf16().map(record_char));
}
fn record_char(unit: u16) -> [u8; 20] {
    record(0, 0, unit, 1, 0, 1)
}
/// ReadConsoleInputA: a key's character in the input code page, one record
/// per byte, as OpenConsole splits a DBCS character into lead and trail.
/// Returns the records' bytes and how many queued records they consumed.
fn ansi_records<'a>(
    records: impl Iterator<Item = &'a [u8; 20]>,
    capacity: usize,
    code_page: u32,
) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut consumed = 0;
    for record in records {
        let expanded = expand_record(record, code_page);
        if out.len() + expanded.len() * 20 > capacity * 20 {
            break;
        }
        out.extend(expanded.iter().flatten());
        consumed += 1;
    }
    (out, consumed)
}
fn expand_record(record: &[u8; 20], code_page: u32) -> Vec<[u8; 20]> {
    let ch = u16::from_le_bytes([record[14], record[15]]);
    if u16::from_le_bytes([record[0], record[1]]) != 1 || ch < 0x80 {
        return vec![*record];
    }
    let encoded = codepage::from_utf16(code_page, &[ch]);
    let encoded = if encoded.is_empty() {
        vec![b'?']
    } else {
        encoded
    };
    encoded
        .into_iter()
        .map(|byte| {
            let mut copy = *record;
            copy[14..16].copy_from_slice(&u16::from(byte).to_le_bytes());
            copy
        })
        .collect()
}

#[path = "read.rs"]
mod read;

#[cfg(test)]
#[path = "tests/input.rs"]
mod tests;
