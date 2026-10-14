//! ConDrv request dispatch and object ownership. Output/state hooks belong to
//! the embedder; pending input requests are completed by the input worker.
//! Unsupported console APIs fail explicitly.
use crate::codepage::{self, Decoder};
use crate::editing::{self, SharedEditing};
use crate::input::ReadParams;
use crate::native::{self, Handle};
use crate::output::ConsoleOutput;
use crate::protocol::*;
use crate::state::{ConsoleState, Palette};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::mem::size_of;
use std::thread::JoinHandle;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Object {
    Input,
    /// A screen buffer: 0 is the main one, others were created.
    Output(u32),
}
/// One attached process, as OpenConsole's ConsoleProcessHandle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    pub pid: u32,
    pub group: u32,
    /// Executable name, which aliases and command history belong to.
    pub application: String,
}
pub struct Server {
    pub sink: Box<dyn ConsoleOutput>,
    pub state: ConsoleState,
    pub(crate) input: Option<crate::input::Input>,
    /// Aliases and command history, shared with the input thread.
    pub(crate) editing: SharedEditing,
    pub objects: BTreeMap<usize, Object>,
    pub processes: BTreeMap<usize, Client>,
    next_token: usize,
    /// Created screen buffers, and which buffer is on screen.
    buffers: BTreeMap<u32, buffers::ScreenBuffer>,
    next_buffer: u32,
    active: u32,
    /// The main buffer's settings while another buffer is on screen.
    main_saved: Option<buffers::Saved>,
    /// Text for the main buffer while another buffer is on screen.
    deferred_main: Vec<buffers::Deferred>,
    /// GetConsoleWindow's window, its owner, and where it reports ShowWindow.
    window: Option<crate::window::PseudoWindow>,
    pub(crate) window_owner: Option<usize>,
    pub(crate) window_output: Option<crate::input::SharedOutput>,
    /// The embedder stopped taking output (the pane is closing): writes are
    /// discarded so that clients can still finish and exit.
    detached: bool,
    /// A palette the embedder changed, applied before the next request.
    pub(crate) pending_palette: std::sync::Arc<std::sync::Mutex<Option<Palette>>>,
    /// Output code page state; the ASCII shortcut is cached per code page.
    decoder: Decoder,
    ascii_output: bool,
    /// Last SetConsoleTextAttribute, reported when the embedder has no pen.
    attributes: u16,
    pub requests: u64,
    pub writes: u64,
    pub source_bytes: u64,
    pub connected: u64,
    pub unsupported: BTreeSet<u32>,
    pub ctrl_handled: bool,
    ctrl_probe: bool,
    probe_tail: Vec<u8>,
    pub control: Option<JoinHandle<io::Result<i32>>>,
    reply: Vec<u8>,
    data: Vec<u8>,
}
impl Server {
    pub fn new(sink: impl ConsoleOutput + 'static, ctrl_probe: bool) -> Self {
        Self::with_state(
            sink,
            ctrl_probe,
            ConsoleState::new(codepage::default(), Palette::default()),
        )
    }
    pub fn with_state(
        sink: impl ConsoleOutput + 'static,
        ctrl_probe: bool,
        state: ConsoleState,
    ) -> Self {
        Self {
            sink: Box::new(sink),
            ascii_output: codepage::ascii_compatible(state.output_cp),
            attributes: state.palette.attributes,
            state,
            input: None,
            editing: SharedEditing::default(),
            objects: BTreeMap::new(),
            processes: BTreeMap::new(),
            next_token: 1,
            buffers: BTreeMap::new(),
            next_buffer: 1,
            active: buffers::MAIN,
            main_saved: None,
            deferred_main: Vec::new(),
            window: None,
            window_owner: None,
            window_output: None,
            detached: false,
            pending_palette: Default::default(),
            decoder: Decoder::default(),
            requests: 0,
            writes: 0,
            source_bytes: 0,
            connected: 0,
            unsupported: BTreeSet::new(),
            ctrl_handled: false,
            ctrl_probe,
            probe_tail: Vec::new(),
            control: None,
            reply: Vec::new(),
            data: Vec::new(),
        }
    }
    fn token(&mut self) -> usize {
        let token = self.next_token;
        self.next_token += 1;
        token
    }
    fn object(&mut self, object: Object) -> usize {
        let token = self.token();
        self.objects.insert(token, object);
        match object {
            Object::Input => {
                if let Some(input) = &self.input {
                    input.object_opened(token);
                }
            }
            Object::Output(buffer) => self.output_opened(buffer),
        }
        token
    }
    pub fn serve(&mut self, handle: &Handle) -> io::Result<()> {
        let mut packet = Packet::default();
        let mut completion: Option<Completion> = None;
        loop {
            // The previous reply's pointers stay live through this call.
            // Completing and receiving in one ioctl removes a round trip per write.
            let (input, len) = completion.as_ref().map_or((std::ptr::null(), 0), |value| {
                ((value as *const Completion).cast(), size_of::<Completion>())
            });
            let received = match native::ioctl(
                handle,
                READ_IO,
                input,
                len,
                (&mut packet as *mut Packet).cast(),
                size_of::<Packet>(),
            ) {
                Ok(received) => received as usize,
                Err(error) if matches!(error.raw_os_error(), Some(109 | 233)) => break,
                Err(error) => return Err(error),
            };
            if received < size_of::<Descriptor>() {
                return Err(io::Error::other("ConDrv returned a truncated descriptor"));
            }
            self.requests += 1;
            self.reply.clear();
            self.apply_pending_palette();
            let available = received - size_of::<Descriptor>();
            let (status, information) = self.dispatch(handle, &packet, available)?;
            if status == 0x103 {
                completion = None;
                continue;
            }
            completion = Some(Completion::new(
                packet.descriptor.id,
                status,
                information,
                &mut self.reply,
            ));
        }
        if !self.processes.is_empty() {
            return Err(io::Error::other("console pipe broke with attached clients"));
        }
        self.stop_terminal_input();
        self.sink.ended();
        Ok(())
    }
    fn dispatch(
        &mut self,
        handle: &Handle,
        packet: &Packet,
        available: usize,
    ) -> io::Result<(i32, usize)> {
        let d = packet.descriptor;
        match d.function {
            1 => self.connect(handle, d),
            2 => {
                if let Some(client) = self.processes.remove(&d.process) {
                    editing::lock(&self.editing).histories.free(d.process);
                    if let Some(input) = &self.input {
                        input.disconnected(d.process, client.pid);
                    }
                }
                Ok((0, 0))
            }
            3 => self.create_object(packet, available),
            4 => {
                match self.objects.remove(&d.object) {
                    Some(Object::Output(buffer)) => self.output_closed(buffer)?,
                    Some(Object::Input) => {
                        if let Some(input) = &self.input {
                            input.object_closed(d.object);
                        }
                    }
                    None => {}
                }
                Ok((0, 0))
            }
            5 => self.write_request(handle, d, 0, false),
            // CONSOLE_IO_RAW_READ: ReadFile, for which ^Z ends input when
            // processed input is on.
            6 => {
                let read = ReadParams {
                    control_z: self.state.input_mode() & crate::state::mode::PROCESSED_INPUT != 0,
                    ..Default::default()
                };
                self.read_input(d, 0, false, false, read)
            }
            7 => self.api(handle, packet, available),
            8 => Ok(self.flush_input(d.object)),
            _ => Ok((NOT_SUPPORTED, 0)),
        }
    }
    /// ConsoleHandleConnectionRequest: the first client names the console,
    /// and each client gets its process token plus input and output handles.
    fn connect(&mut self, handle: &Handle, d: Descriptor) -> io::Result<(i32, usize)> {
        if d.input_size < 36 {
            return Ok((INVALID_PARAMETER, 0));
        }
        let mut message = vec![0; (d.input_size as usize).min(crate::connect::SIZE)];
        native::transfer(handle, READ_INPUT, d.id, 0, &mut message)?;
        let connect = crate::connect::parse(&message);
        if connect.console_app {
            let status = crate::control::notify_application(d.process as u32)?;
            if status < 0 {
                eprintln!("ConsoleControl(NotifyApplication) returned {status:#010x}");
            }
        }
        self.state.connected(&connect.title);
        let process = self.token();
        let application: Vec<u16> = connect.application.encode_utf16().collect();
        editing::lock(&self.editing)
            .histories
            .allocate(&application, process);
        self.processes.insert(
            process,
            Client {
                pid: d.process as u32,
                group: connect.process_group,
                application: connect.application,
            },
        );
        if let Some(input) = &self.input {
            input.connected(process, d.process as u32);
        }
        let input = self.object(Object::Input);
        let output = self.object(Object::Output(self.active));
        for token in [process, input, output] {
            self.reply.extend_from_slice(&token.to_ne_bytes());
        }
        self.connected += 1;
        Ok((0, self.reply.len()))
    }
    fn create_object(&mut self, packet: &Packet, available: usize) -> io::Result<(i32, usize)> {
        if available < 12 {
            return Ok((INVALID_PARAMETER, 0));
        }
        // CD_CREATE_OBJECT_INFORMATION: ObjectType, ShareMode, DesiredAccess.
        // CONOUT$ and write-only CON open whichever buffer is on screen.
        let kind = match u32_at(&packet.payload, 0) {
            1 => Object::Input,
            2 => Object::Output(self.active),
            3 => return Ok((0, self.create_screen_buffer()?)),
            4 if u32_at(&packet.payload, 8) & 0xc0000000 == 0x80000000 => Object::Input,
            4 if u32_at(&packet.payload, 8) & 0xc0000000 == 0x40000000 => {
                Object::Output(self.active)
            }
            _ => return Ok((INVALID_PARAMETER, 0)),
        };
        Ok((0, self.object(kind)))
    }
    pub fn write_request(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        offset: u32,
        unicode: bool,
    ) -> io::Result<(i32, usize)> {
        let Some(buffer) = self.output_buffer(d.object) else {
            return Ok((INVALID_HANDLE, 0));
        };
        let Some(length) = d.input_size.checked_sub(offset) else {
            return Ok((INVALID_PARAMETER, 0));
        };
        if unicode && (length % 2 != 0 || length > 16 * 1024 * 1024) {
            return Ok((INVALID_PARAMETER, 0));
        }
        self.writes += 1;
        let mut consumed = 0;
        // Reuse one bounded buffer for raw writes. Unicode writes need a complete
        // request so a surrogate pair cannot be split by our transport chunks.
        let mut data = std::mem::take(&mut self.data);
        let result = (|| {
            while consumed < length {
                let size = if unicode {
                    length
                } else {
                    (length - consumed).min(128 * 1024)
                };
                data.resize(size as usize, 0);
                native::transfer(handle, READ_INPUT, d.id, offset + consumed, &mut data)?;
                if unicode {
                    let wide: Vec<u16> = data
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect();
                    if buffer == self.active {
                        self.write_wide(&wide, d.process)?;
                    } else {
                        self.write_off_screen(buffer, buffers::Deferred::Wide(wide));
                    }
                } else if buffer == self.active {
                    self.write_bytes(&data, d.process)?;
                } else {
                    self.write_off_screen(buffer, buffers::Deferred::Bytes(data.clone()));
                }
                consumed += size;
            }
            self.source_bytes += length as u64;
            Ok((0, length as usize))
        })();
        self.data = data;
        result
    }
    /// A write the embedder refuses because it has closed detaches the
    /// console from it; anything else is a real error.
    pub(super) fn sink_write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.detached {
            return Ok(());
        }
        match self.sink.write(bytes, self.output_mode()) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                self.detached = true;
                Ok(())
            }
            result => result,
        }
    }
    /// Hands text to the embedder and runs the Ctrl+C probe over it.
    fn consume(&mut self, bytes: &[u8], process: usize) -> io::Result<()> {
        self.sink_write(bytes)?;
        if self.ctrl_probe {
            self.probe_tail.extend_from_slice(bytes);
            if self.control.is_none()
                && self
                    .probe_tail
                    .windows(16)
                    .any(|w| w == b"ZETTA_CTRL_READY")
            {
                let pid = self
                    .processes
                    .get(&process)
                    .ok_or_else(|| io::Error::other("probe wrote from an unknown process"))?
                    .pid;
                self.control = Some(std::thread::spawn(move || crate::control::send_ctrl_c(pid)));
            }
            self.ctrl_handled |= self
                .probe_tail
                .windows(18)
                .any(|w| w == b"ZETTA_CTRL_HANDLED");
            if self.probe_tail.len() > 32 {
                self.probe_tail.drain(..self.probe_tail.len() - 32);
            }
        }
        Ok(())
    }
    /// A theme change replaces the color table and the default colors, as
    /// ConPTY's palette helper does through SetConsoleScreenBufferInfoEx.
    fn apply_pending_palette(&mut self) {
        let pending = self
            .pending_palette
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(palette) = pending {
            if self.attributes == self.state.palette.attributes {
                self.attributes = palette.attributes;
            }
            self.state.palette = palette;
        }
    }
    /// CONSOLE_IO_RAW_FLUSH and FlushConsoleInputBuffer share one meaning.
    fn flush_input(&mut self, object: usize) -> (i32, usize) {
        if self.objects.get(&object) != Some(&Object::Input) {
            return (INVALID_HANDLE, 0);
        }
        if let Some(input) = &self.input {
            input.flush();
        }
        (0, 0)
    }
    pub fn unsupported_api(&mut self, api: u32) -> (i32, usize) {
        if self.unsupported.insert(api) {
            eprintln!("unsupported console API {api:#010x}");
        }
        (NOT_SUPPORTED, 0)
    }
    pub(crate) fn read_input(
        &mut self,
        descriptor: Descriptor,
        body_size: usize,
        unicode: bool,
        records: bool,
        read: ReadParams,
    ) -> io::Result<(i32, usize)> {
        if self.objects.get(&descriptor.object) != Some(&Object::Input) {
            return Ok((INVALID_HANDLE, 0));
        }
        let Some(input) = &self.input else {
            return Ok((NOT_SUPPORTED, 0));
        };
        let minimum = if records {
            20
        } else if unicode {
            2
        } else {
            1
        };
        if (descriptor.output_size as usize).saturating_sub(body_size) < minimum {
            return Ok((INVALID_PARAMETER, 0));
        }
        let body = self.reply[..body_size].to_vec();
        let flags = if records {
            u16::from_le_bytes([body[4], body[5]])
        } else {
            0
        };
        let accepted = input.request(crate::input::Request {
            descriptor,
            body,
            records,
            unicode,
            peek: flags & 1 != 0,
            nowait: flags & 2 != 0,
            read,
            cooked: None,
            control_key_state: 0,
            waited: false,
        });
        if accepted {
            Ok((0x103, 0))
        } else {
            Ok((0xc000009au32 as i32, 0))
        }
    }
}

#[path = "api.rs"]
mod api;
#[path = "buffers.rs"]
mod buffers;
#[path = "doskey.rs"]
mod doskey;
#[path = "info.rs"]
mod info;
#[path = "modes.rs"]
mod modes;
#[path = "screen.rs"]
mod screen;
#[path = "signals.rs"]
mod signals;
#[path = "title.rs"]
mod title;
#[path = "write.rs"]
mod write;

#[cfg(all(test, feature = "harness"))]
#[path = "tests/server.rs"]
mod tests;
