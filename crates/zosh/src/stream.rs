//! A Mosh session an application drives, with no terminal of its own.
//!
//! The standalone client in [`crate::client`] owns a terminal: raw mode,
//! signals, an escape key, stdin and stdout. An embedding application has none
//! of those. What it has is a pane, and what it wants from Mosh is exactly what
//! the terminal would have seen — a stream of display bytes to feed to its own
//! emulator, and somewhere to put keystrokes.
//!
//! So this is the same session loop with its two ends replaced: stdout becomes
//! an in-process pipe the embedder reads, and stdin becomes a command channel
//! carrying input and resizes. The frame decision both loops make is shared
//! through [`crate::frame`] rather than copied.
//!
//! The loop runs on its own thread. That thread is the only one that touches
//! the session, which is what lets the embedder call [`PaneSession::resize`]
//! and write input from wherever it happens to be.

use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
};

use anyhow::{Context as _, Result};
use mosh_rs::{Base64Key, DisplayPreference, MoshSession};

use crate::{
    client::{ProxiedInput, TerminalQueryProxy, forward_terminal_queries},
    display::DisplayScreen,
    frame::{self, ClientSession, Frame},
};

/// The longest the loop sleeps with nothing to do. The session's own deadline
/// is usually shorter; this is the ceiling that keeps a quiet link's timers
/// honest, and matches the standalone client's.
const IDLE_WAIT_MS: u64 = 100;

/// How much rendered output may sit unread before the loop waits for the
/// consumer.
///
/// A pane's reader is a dedicated thread that does nothing but drain this, so
/// the bound is never reached in practice; it exists so that a consumer which
/// has stopped reading stalls its own session instead of growing this buffer
/// without limit. A single frame is always accepted whole, however large.
const OUTPUT_CAPACITY: usize = 1024 * 1024;

/// What the session is configured with. The launcher's other settings
/// (`--init`, the escape key, the prediction *display* the user asked for on
/// the command line) belong to a terminal, and have no meaning here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PaneSessionSettings {
    /// How long the session may go without sending before it emits a
    /// keep-alive, or `None` for Mosh's own three-second heartbeat.
    pub keep_alive: Option<u64>,
    pub prediction: DisplayPreference,
    pub predict_overwrite: bool,
}

/// A live Mosh session rendered into a byte stream.
///
/// Dropping it shuts the session down and closes the reader.
pub struct PaneSession {
    commands: Sender<Command>,
    wake: Arc<Wake>,
    reader: Option<PaneReader>,
    finished: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

enum Command {
    Input(Vec<u8>),
    Resize(u16, u16),
    Shutdown,
}

impl PaneSession {
    /// Connects to a Mosh endpoint and starts rendering it.
    ///
    /// The connection itself is made on the calling thread, so a bad key, an
    /// unreachable endpoint or a refused handshake is reported here rather
    /// than arriving later as a dead session.
    pub fn connect(
        host: &str,
        port: u16,
        key: &Base64Key,
        columns: u16,
        rows: u16,
        settings: PaneSessionSettings,
    ) -> Result<Self> {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let mut session =
            MoshSession::connect_with_screen(host, port, key, DisplayScreen::new(rows, columns))
                .context("connecting to the Mosh UDP endpoint")?;
        session
            .prediction_mut()
            .set_display_preference(settings.prediction);
        if settings.predict_overwrite {
            session.prediction_mut().set_predict_overwrite(true);
        }
        session.set_keep_alive(settings.keep_alive);

        let wake = Arc::new(Wake::new().context("creating the session wake-up pipe")?);
        let output = Arc::new(OutputPipe::default());
        let finished = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (commands, command_receiver) = mpsc::channel();

        let thread = thread::Builder::new()
            .name("zosh-pane-session".to_owned())
            .spawn({
                let wake = wake.clone();
                let output = output.clone();
                let finished = finished.clone();
                let error = error.clone();
                move || {
                    let result = drive(session, (columns, rows), &command_receiver, &wake, &output);
                    if let Err(failure) = result {
                        *error
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(format!("{failure:#}"));
                    }
                    finished.store(true, Ordering::SeqCst);
                    output.close();
                }
            })
            .context("starting the Mosh session thread")?;

        Ok(Self {
            commands,
            wake,
            reader: Some(PaneReader { pipe: output }),
            finished,
            error,
            thread: Some(thread),
        })
    }

    /// The rendered display bytes, once. The stream ends when the session
    /// does, so a consumer reading it to EOF learns that the pane is over.
    pub fn take_reader(&mut self) -> Option<PaneReader> {
        self.reader.take()
    }

    /// A handle that turns writes into Mosh user input. Cheap to clone, and
    /// writing to it after the session ends reports a broken pipe.
    pub fn writer(&self) -> PaneWriter {
        PaneWriter {
            commands: self.commands.clone(),
            wake: self.wake.clone(),
        }
    }

    /// Tells the remote terminal it has a new size. The frame that answers is
    /// painted as a whole-screen repaint, so the two geometries are never
    /// mixed.
    pub fn resize(&self, columns: u16, rows: u16) {
        self.send(Command::Resize(columns.max(1), rows.max(1)));
    }

    /// Whether the session loop has ended, for any reason.
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Why the session ended, when it ended in a failure.
    pub fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Asks the remote side to end the session and stops the loop. The reader
    /// reaches EOF once the loop has gone.
    pub fn shutdown(&self) {
        self.send(Command::Shutdown);
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            self.wake.notify();
        }
    }
}

impl Drop for PaneSession {
    fn drop(&mut self) {
        self.shutdown();
        // The loop ends once the shutdown handshake completes or times out,
        // and closes the reader then. Waiting for that here would block
        // whichever thread dropped the pane, which is usually the one drawing
        // it. A reader still held elsewhere keeps draining until then; one
        // dropped along with this handle abandons the pipe itself.
        drop(self.thread.take());
    }
}

/// The display bytes of a [`PaneSession`], as a blocking reader.
pub struct PaneReader {
    pipe: Arc<OutputPipe>,
}

impl Read for PaneReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.pipe.read(buffer)
    }
}

impl Drop for PaneReader {
    fn drop(&mut self) {
        self.pipe.abandon();
    }
}

/// Keyboard input for a [`PaneSession`].
#[derive(Clone)]
pub struct PaneWriter {
    commands: Sender<Command>,
    wake: Arc<Wake>,
}

impl Write for PaneWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.commands
            .send(Command::Input(bytes.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the Mosh session has ended"))?;
        self.wake.notify();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Drives one session to its end.
///
/// The pass order matters and matches the standalone client's: wait for
/// something to do, apply everything the embedder asked for, pump the network,
/// forward any terminal query, then paint at most one frame.
fn drive(
    mut session: ClientSession,
    size: (u16, u16),
    commands: &Receiver<Command>,
    wake: &Wake,
    output: &Arc<OutputPipe>,
) -> Result<()> {
    let mut sink = OutputSink {
        pipe: Arc::clone(output),
    };
    let mut pending_resize = None;
    let mut query_proxy = TerminalQueryProxy::default();
    session.send_resize(i32::from(size.0), i32::from(size.1));
    loop {
        let wait = session.wait_time_ms().min(IDLE_WAIT_MS);
        wait_for_command_or_network(&session, wake, wait)?;
        wake.drain();
        apply_commands(
            &mut session,
            commands,
            &mut query_proxy,
            &mut pending_resize,
        );
        let events = session.pump_ready().context("pumping the Mosh session")?;
        forward_terminal_queries(&events, &mut query_proxy, &mut sink)
            .context("forwarding a terminal query")?;
        match frame::next_frame(&session, &events, pending_resize) {
            Frame::Repaint { resolves_pending } => {
                frame::write_repaint(&mut session, &mut sink)
                    .context("painting a resized screen")?;
                if resolves_pending {
                    pending_resize = None;
                }
            }
            Frame::Paint => {
                let bytes = session.render();
                if !bytes.is_empty() {
                    sink.write_all(&bytes).context("painting a frame")?;
                }
            }
            // A resize is in flight: the server's next frame carries the new
            // geometry, and it is repainted atomically when it arrives.
            Frame::Skip => {}
        }
        if session.finished() {
            return Ok(());
        }
    }
}

/// Applies everything the embedder has queued, without blocking.
///
/// A closed channel means every handle has gone, which is the same request as
/// an explicit shutdown; the loop keeps running afterwards so the session can
/// finish its shutdown handshake.
fn apply_commands(
    session: &mut ClientSession,
    commands: &Receiver<Command>,
    query_proxy: &mut TerminalQueryProxy,
    pending_resize: &mut Option<(u16, u16)>,
) {
    loop {
        match commands.try_recv() {
            Ok(Command::Input(bytes)) => apply_input(session, query_proxy, &bytes),
            Ok(Command::Resize(columns, rows)) => {
                session.send_resize(i32::from(columns), i32::from(rows));
                *pending_resize = Some((columns, rows));
            }
            Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => session.shutdown(),
            Err(TryRecvError::Empty) => return,
        }
    }
}

/// Input, with an OSC 10/11 response taken out of it.
///
/// The remote program's colour query was written into the pane's own byte
/// stream, so the answer arrives here mixed into ordinary keystrokes exactly
/// as it does on a real terminal's stdin.
fn apply_input(session: &mut ClientSession, query_proxy: &mut TerminalQueryProxy, bytes: &[u8]) {
    for input in query_proxy.filter(bytes) {
        match input {
            ProxiedInput::User(bytes) => session.send_input(&bytes),
            ProxiedInput::TerminalResponse(bytes) => session.send_terminal_response(&bytes),
        }
    }
}

#[cfg(unix)]
fn wait_for_command_or_network(
    session: &ClientSession,
    wake: &Wake,
    timeout_ms: u64,
) -> Result<()> {
    let mut descriptors = Vec::with_capacity(session.socket_handles().len() + 1);
    descriptors.push(libc::pollfd {
        fd: wake.read_descriptor(),
        events: libc::POLLIN,
        revents: 0,
    });
    descriptors.extend(session.socket_handles().into_iter().map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }));
    let timeout = timeout_ms.min(i32::MAX as u64) as i32;
    // SAFETY: every descriptor is borrowed from a live Mosh socket or from the
    // wake pipe this session owns, and `descriptors` stays allocated until
    // poll has returned.
    let result = unsafe {
        libc::poll(
            descriptors.as_mut_ptr(),
            descriptors.len() as libc::nfds_t,
            timeout,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error).context("waiting for Mosh network or session input");
        }
    }
    Ok(())
}

/// Without `poll` there is no way to wait on the Mosh sockets and the
/// embedder at once, so the wait is on the embedder alone and the network is
/// pumped when it expires. The standalone client does the same on these
/// platforms: input is immediate, and output waits at most [`IDLE_WAIT_MS`].
#[cfg(not(unix))]
fn wait_for_command_or_network(
    _session: &ClientSession,
    wake: &Wake,
    timeout_ms: u64,
) -> Result<()> {
    wake.wait(timeout_ms);
    Ok(())
}

/// A pipe the loop paints into and the embedder reads.
#[derive(Default)]
struct OutputPipe {
    state: Mutex<OutputState>,
    changed: Condvar,
}

#[derive(Default)]
struct OutputState {
    bytes: VecDeque<u8>,
    /// The loop has ended: a reader that drains the rest sees EOF.
    closed: bool,
    /// The reader has gone: the loop stops rendering into a buffer nothing
    /// will ever read.
    abandoned: bool,
}

impl OutputPipe {
    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while state.bytes.len() >= OUTPUT_CAPACITY && !state.abandoned {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if state.abandoned {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the pane stopped reading its Mosh session",
            ));
        }
        state.bytes.extend(bytes);
        drop(state);
        self.changed.notify_all();
        Ok(())
    }

    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while state.bytes.is_empty() && !state.closed {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        // Copied a run at a time rather than a byte at a time: a pane reading
        // a fast program moves megabytes through here, and a `VecDeque` is at
        // most two contiguous runs.
        let count = state.bytes.len().min(buffer.len());
        let (front, back) = state.bytes.as_slices();
        let from_front = front.len().min(count);
        buffer[..from_front].copy_from_slice(&front[..from_front]);
        let from_back = count - from_front;
        buffer[from_front..count].copy_from_slice(&back[..from_back]);
        state.bytes.drain(..count);
        drop(state);
        self.changed.notify_all();
        Ok(count)
    }

    fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .closed = true;
        self.changed.notify_all();
    }

    fn abandon(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .abandoned = true;
        self.changed.notify_all();
    }
}

/// The painting end of [`OutputPipe`], as a [`Write`].
struct OutputSink {
    pipe: Arc<OutputPipe>,
}

impl Write for OutputSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pipe.write(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// How a caller on another thread gets the session loop out of its wait.
///
/// On Unix the loop is inside `poll`, so there has to be a descriptor in the
/// set it can be woken through; elsewhere the wait is the command channel
/// itself and sending is already the wake-up.
struct Wake {
    #[cfg(unix)]
    read: std::os::fd::OwnedFd,
    #[cfg(unix)]
    write: std::os::fd::OwnedFd,
    #[cfg(not(unix))]
    notified: Mutex<bool>,
    #[cfg(not(unix))]
    signal: Condvar,
}

#[cfg(unix)]
impl Wake {
    fn new() -> io::Result<Self> {
        use std::os::fd::{FromRawFd as _, OwnedFd};

        let mut descriptors = [0 as libc::c_int; 2];
        // SAFETY: `pipe` fills the two-element array it is given.
        if unsafe { libc::pipe(descriptors.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `pipe` returned these descriptors and nothing else owns
        // them, so the pipe is closed exactly once, when this value is
        // dropped.
        let pipe = unsafe {
            Self {
                read: OwnedFd::from_raw_fd(descriptors[0]),
                write: OwnedFd::from_raw_fd(descriptors[1]),
            }
        };
        // A wake-up must never block the caller, and the loop must be able to
        // empty the pipe without blocking either.
        for descriptor in [pipe.read_descriptor(), pipe.write_descriptor()] {
            set_descriptor_flags(descriptor)?;
        }
        Ok(pipe)
    }

    fn read_descriptor(&self) -> libc::c_int {
        use std::os::fd::AsRawFd as _;
        self.read.as_raw_fd()
    }

    fn write_descriptor(&self) -> libc::c_int {
        use std::os::fd::AsRawFd as _;
        self.write.as_raw_fd()
    }

    fn notify(&self) {
        let byte = [1_u8];
        // SAFETY: the descriptor is owned by this value and the buffer is one
        // byte long. A full pipe already means a pending wake-up, so a short
        // or failed write needs no handling.
        unsafe {
            libc::write(self.write_descriptor(), byte.as_ptr().cast(), 1);
        }
    }

    fn drain(&self) {
        let mut bytes = [0_u8; 64];
        loop {
            // SAFETY: the descriptor is owned by this value and the buffer is
            // as long as the count passed with it.
            let read = unsafe {
                libc::read(
                    self.read_descriptor(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                )
            };
            if read <= 0 {
                return;
            }
        }
    }
}

#[cfg(unix)]
fn set_descriptor_flags(descriptor: libc::c_int) -> io::Result<()> {
    // SAFETY: `descriptor` is owned by the caller and both calls only read or
    // replace its flags.
    unsafe {
        if libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
        let flags = libc::fcntl(descriptor, libc::F_GETFL);
        if flags < 0 || libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(not(unix))]
impl Wake {
    fn new() -> io::Result<Self> {
        Ok(Self {
            notified: Mutex::new(false),
            signal: Condvar::new(),
        })
    }

    fn notify(&self) {
        *self
            .notified
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.signal.notify_all();
    }

    fn wait(&self, timeout_ms: u64) {
        let notified = self
            .notified
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *notified {
            return;
        }
        let _ = self.signal.wait_timeout(
            notified,
            std::time::Duration::from_millis(timeout_ms.max(1)),
        );
    }

    fn drain(&self) {
        *self
            .notified
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
    }
}

#[cfg(test)]
#[path = "tests/stream.rs"]
mod tests;
