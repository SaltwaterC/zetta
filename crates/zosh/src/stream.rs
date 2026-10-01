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
//! and write input from wherever it happens to be. It sleeps until the
//! network, the embedder or one of the session's own deadlines needs it (see
//! [`crate::wait`]), so an idle pane costs a wakeup per heartbeat or
//! keep-alive rather than ten a second.

use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    path::PathBuf,
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
    agent::{AGENT_PROTOCOL_VERSION, AgentBridge, AgentClientCommand},
    client::{ProxiedInput, TerminalQueryProxy, forward_terminal_queries},
    display::DisplayScreen,
    frame::{self, ClientSession, Frame},
    wait::{self, PassCounter, Waiter, Wake},
};

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
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PaneSessionSettings {
    /// How long the session may go without sending before it emits a
    /// keep-alive, or `None` for Mosh's own three-second heartbeat.
    pub keep_alive: Option<u64>,
    pub prediction: DisplayPreference,
    pub predict_overwrite: bool,
    /// KiB of scrolled-off history to ask the server to carry, or zero to ask
    /// for none.
    ///
    /// A pane has a scrollback of its own and an embedder that expects to be
    /// able to scroll it, so this is the setting a pane is least able to do
    /// without. `Default` asks for none, because a default has to be the inert
    /// one; [`crate::SCROLLBACK_DEFAULT_KIB`] is what to pass.
    pub scrollback_kib: u32,
    /// Request the opt-in Zosh SSH-agent extension.
    pub forward_agent: bool,
    /// The native SSH forwarding binding captured during bootstrap, if any.
    pub agent_binding: Option<Vec<u8>>,
    /// The local agent OpenSSH selected for this destination. This can differ
    /// from `SSH_AUTH_SOCK` when `IdentityAgent` is set in SSH configuration.
    pub agent_path: Option<PathBuf>,
}

struct AgentSettings {
    forward: bool,
    binding: Option<Vec<u8>>,
    path: Option<PathBuf>,
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
    #[cfg_attr(
        not(all(test, unix)),
        allow(
            dead_code,
            reason = "only the Unix interop tests count the loop's passes"
        )
    )]
    passes: PassCounter,
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
        let PaneSessionSettings {
            keep_alive,
            prediction,
            predict_overwrite,
            scrollback_kib,
            forward_agent,
            agent_binding,
            agent_path,
        } = settings;
        let columns = columns.max(1);
        let rows = rows.max(1);
        let agent_settings = AgentSettings {
            forward: forward_agent,
            binding: agent_binding,
            path: agent_path,
        };
        let mut session =
            MoshSession::connect_with_screen(host, port, key, DisplayScreen::new(rows, columns))
                .context("connecting to the Mosh UDP endpoint")?;
        session.prediction_mut().set_display_preference(prediction);
        if predict_overwrite {
            session.prediction_mut().set_predict_overwrite(true);
        }
        session.set_keep_alive(keep_alive);
        session.request_clipboard_relay();
        if scrollback_kib > 0 {
            // Before the loop starts, so it rides the first instruction and
            // the server carries history from the first row that scrolls.
            session.request_scrollback(scrollback_kib);
        }

        let wake = Arc::new(Wake::new().context("creating the session wake-up pipe")?);
        let output = Arc::new(OutputPipe::default());
        let finished = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (commands, command_receiver) = mpsc::channel();
        let passes = PassCounter::default();

        let thread = thread::Builder::new()
            .name("zosh-pane-session".to_owned())
            .spawn({
                let wake = wake.clone();
                let output = output.clone();
                let finished = finished.clone();
                let error = error.clone();
                let passes = passes.clone();
                move || {
                    let ends = LoopEnds {
                        commands: &command_receiver,
                        wake: &wake,
                        output: &output,
                        passes: &passes,
                    };
                    let result = drive(session, (columns, rows), agent_settings, ends);
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
            passes,
        })
    }

    /// How many passes the session loop has made.
    #[cfg(all(test, unix))]
    pub(crate) fn passes(&self) -> u64 {
        self.passes.count()
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

/// What the session loop talks to besides the session itself.
#[derive(Clone, Copy)]
struct LoopEnds<'a> {
    commands: &'a Receiver<Command>,
    wake: &'a Wake,
    output: &'a Arc<OutputPipe>,
    passes: &'a PassCounter,
}

/// Drives one session to its end.
///
/// The pass order matters and matches the standalone client's: wait for
/// something to do, apply everything the embedder asked for, pump the network,
/// forward any terminal query, then paint at most one frame.
///
/// The wait ends on a datagram, on the embedder's wake-up, or at the earliest
/// deadline the session or the agent negotiation has, and at no other time.
fn drive(
    mut session: ClientSession,
    size: (u16, u16),
    agent_settings: AgentSettings,
    ends: LoopEnds<'_>,
) -> Result<()> {
    let LoopEnds {
        commands,
        wake,
        output,
        passes,
    } = ends;
    let mut sink = OutputSink {
        pipe: Arc::clone(output),
    };
    let mut pending_resize = None;
    let mut query_proxy = TerminalQueryProxy::default();
    let mut agent = AgentBridge::with_agent(
        agent_settings.forward,
        agent_settings.binding,
        agent_settings.path,
    );
    if agent.enabled() {
        session.request_agent_forwarding(AGENT_PROTOCOL_VERSION);
    }
    session.send_resize(i32::from(size.0), i32::from(size.1));
    let mut embedder = Embedder::Present;
    let mut waiter = Waiter::default();
    loop {
        let wait = wait::earliest_ms([Some(session.next_wake_ms()), agent.wait_ms()]);
        waiter
            .wait(&[wake.handle()], session.socket_handle_iter(), wait)
            .context("waiting for Mosh network or session input")?;
        wake.drain();
        passes.tick();
        if embedder == Embedder::Present {
            embedder = apply_commands(
                &mut session,
                commands,
                &mut query_proxy,
                &mut pending_resize,
            );
        }
        let events = session.pump_ready().context("pumping the Mosh session")?;
        for command in agent.handle_events(&events) {
            apply_agent_command(&mut session, command);
        }
        agent.negotiation_timed_out();
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

fn apply_agent_command(session: &mut ClientSession, command: AgentClientCommand) {
    match command {
        AgentClientCommand::Response {
            connection_id,
            request_id,
            frame,
            closed,
        } => session.send_agent_response(connection_id, request_id, &frame, closed),
    }
}

/// Whether anything can still send the loop a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Embedder {
    Present,
    /// Every handle has gone. Nothing can arrive any more, so the channel is
    /// not asked again: it would answer "disconnected" every time.
    Gone,
}

/// Applies everything the embedder has queued, without blocking.
fn apply_commands(
    session: &mut ClientSession,
    commands: &Receiver<Command>,
    query_proxy: &mut TerminalQueryProxy,
    pending_resize: &mut Option<(u16, u16)>,
) -> Embedder {
    drain_commands(commands, |command| match command {
        Command::Input(bytes) => apply_input(session, query_proxy, &bytes),
        Command::Resize(columns, rows) => {
            session.send_resize(i32::from(columns), i32::from(rows));
            *pending_resize = Some((columns, rows));
        }
        Command::Shutdown => session.shutdown(),
    })
}

/// Hands every queued command to `apply`, then says whether more can come.
///
/// A closed channel means every handle has gone, which is the same request as
/// an explicit shutdown, so it is applied as one — once. The loop keeps
/// running afterwards so the session can finish its shutdown handshake, and
/// stops asking the channel. Treating "disconnected" as one more command to
/// act on, and then asking again, is what spun a dropped pane's session
/// thread at full speed for ever: it never returned to its wait, never read
/// its socket again, and never finished the shutdown it kept restarting.
fn drain_commands(commands: &Receiver<Command>, mut apply: impl FnMut(Command)) -> Embedder {
    loop {
        match commands.try_recv() {
            Ok(command) => apply(command),
            Err(TryRecvError::Disconnected) => {
                apply(Command::Shutdown);
                return Embedder::Gone;
            }
            Err(TryRecvError::Empty) => return Embedder::Present,
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

#[cfg(test)]
#[path = "tests/stream.rs"]
mod tests;
