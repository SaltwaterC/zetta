//! The main event loop which performs I/O on the pseudoterminal.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use log::error;
use polling::{Event as PollingEvent, Events, PollMode, Poller};

use crate::event::{self, Event, EventListener, WindowSize};
use crate::pty_parser::ParserThread;
use crate::sync::FairMutex;
use crate::term::Term;
use crate::{thread, tty};
use vte::ansi;

/// Max bytes to read from the PTY before forced terminal synchronization.
pub(crate) const READ_BUFFER_SIZE: usize = 0x10_0000;

/// How long a foreign child's PTY may stay hung up before the loop stops
/// waiting for an exit report that is not coming.
///
/// A hung-up master means the program on the far side is gone. For a child this
/// process spawned, the matching exit notification is already on its way and the
/// loop can simply wait. For a child that belongs to the multiplexer, the only
/// route is a report over the control channel, so if that channel is broken the
/// wait is forever: the pane accepts no input, shows no exit, and cannot be
/// closed. Long enough that an ordinary report always wins the race, short
/// enough that a user pressing Ctrl-D does not sit in front of a dead pane.
const FOREIGN_CHILD_HANGUP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// How long to wait between polls of a hung-up PTY.
///
/// The poller is level-triggered, so a persistent hangup makes `wait` return
/// immediately every time. Without this the loop would burn a core for as long
/// as it waited — which, before the grace period above existed, was forever.
const HANGUP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

/// Records a hangup and says whether waiting for a child event should stop.
///
/// Returns `true` only for a foreign child whose grace period has run out. A
/// child this process spawned is always waited for, exactly as upstream does,
/// because its exit notification cannot fail to arrive — and nothing is recorded
/// for it either, so a pty that somehow changed answer could not inherit a stale
/// deadline. While a foreign child is still within its grace, this paces the
/// level-triggered poller so the wait costs a short sleep per iteration rather
/// than a spinning core.
fn hungup_too_long<T: tty::EventedPty + ?Sized>(
    pty: &T,
    hangup_since: &mut Option<Instant>,
) -> bool {
    if !pty.child_is_foreign() {
        return false;
    }
    let since = *hangup_since.get_or_insert_with(Instant::now);
    if since.elapsed() >= FOREIGN_CHILD_HANGUP_GRACE {
        error!("the multiplexer never reported the exit of a terminal that has hung up");
        return true;
    }
    std::thread::sleep(HANGUP_POLL_INTERVAL);
    false
}

/// Messages that may be sent to the `EventLoop`.
#[derive(Debug)]
pub enum Msg {
    /// Data that should be written to the PTY.
    Input(Cow<'static, [u8]>),

    /// Indicates that the `EventLoop` should shut down, as Alacritty is shutting down.
    Shutdown,

    /// Instruction to resize the PTY.
    Resize(WindowSize),

    /// Asks the foreground process to repaint even when the PTY's dimensions
    /// did not change.
    Redraw,

    /// Updates the legacy Win32 colors associated with the pseudoconsole.
    #[cfg(windows)]
    SetConsolePalette(tty::ConsolePalette),
}

/// Keeps a backend reader behind a retained-screen replay until the terminal
/// has applied that replay at its first real layout.
///
/// The barrier is deliberately separate from the terminal lock. A reader may
/// wait here while the UI thread is waiting for layout, and teardown must be
/// able to wake that wait without needing the terminal lock first.
#[derive(Clone)]
pub struct ReplayBarrier {
    state: Arc<ReplayBarrierState>,
}

struct ReplayBarrierState {
    state: Mutex<ReplayBarrierStatus>,
    changed: Condvar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayBarrierStatus {
    Closed,
    Open,
    Aborted,
}

impl ReplayBarrier {
    /// Creates a barrier that is open for a backend with no retained replay.
    pub fn open() -> Self {
        Self::new(ReplayBarrierStatus::Open)
    }

    /// Creates a barrier that waits for the retained replay to be applied.
    pub fn closed() -> Self {
        Self::new(ReplayBarrierStatus::Closed)
    }

    fn new(status: ReplayBarrierStatus) -> Self {
        Self {
            state: Arc::new(ReplayBarrierState {
                state: Mutex::new(status),
                changed: Condvar::new(),
            }),
        }
    }

    /// Closes an as-yet-unused barrier before a replay-backed reader starts.
    pub fn close(&self) {
        let mut status = self.state.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *status == ReplayBarrierStatus::Open {
            *status = ReplayBarrierStatus::Closed;
        }
    }

    /// Releases readers after the replay has been written to the correctly
    /// sized grid. An aborted barrier cannot be reopened.
    pub fn release(&self) {
        let mut status = self.state.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *status == ReplayBarrierStatus::Closed {
            *status = ReplayBarrierStatus::Open;
            self.state.changed.notify_all();
        }
    }

    /// Wakes every reader and tells it not to parse any more bytes.
    pub fn abort(&self) {
        let mut status = self.state.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *status != ReplayBarrierStatus::Aborted {
            *status = ReplayBarrierStatus::Aborted;
            self.state.changed.notify_all();
        }
    }

    /// Waits until parsing is allowed, returning `false` when teardown won the
    /// race and the backend must return without touching the terminal.
    pub fn wait(&self) -> bool {
        let mut status = self.state.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while *status == ReplayBarrierStatus::Closed {
            status =
                self.state.changed.wait(status).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *status == ReplayBarrierStatus::Open
    }

    /// Aborts only a reader that is still waiting for replay. An open reader
    /// remains drainable when a backend is replaced.
    pub fn abort_if_closed(&self) {
        let mut status = self.state.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *status == ReplayBarrierStatus::Closed {
            *status = ReplayBarrierStatus::Aborted;
            self.state.changed.notify_all();
        }
    }
}

/// The main event loop.
///
/// Handles all the PTY I/O and runs the PTY parser which updates terminal
/// state.
pub struct EventLoop<T: tty::EventedPty, U: EventListener> {
    poll: Arc<Poller>,
    pty: T,
    rx: PeekableReceiver<Msg>,
    tx: Sender<Msg>,
    terminal: Arc<FairMutex<Term<U>>>,
    event_proxy: U,
    drain_on_exit: bool,
    ref_test: bool,
    replay_barrier: ReplayBarrier,
}

impl<T, U> EventLoop<T, U>
where
    T: tty::EventedPty + event::OnResize + Send + 'static,
    U: EventListener + Clone + Send + 'static,
{
    /// Create a new event loop.
    pub fn new(
        terminal: Arc<FairMutex<Term<U>>>,
        event_proxy: U,
        pty: T,
        drain_on_exit: bool,
        ref_test: bool,
        replay_barrier: ReplayBarrier,
    ) -> io::Result<EventLoop<T, U>> {
        let (tx, rx) = mpsc::channel();
        let poll = Poller::new()?.into();
        Ok(EventLoop {
            poll,
            pty,
            tx,
            rx: PeekableReceiver::new(rx),
            terminal,
            event_proxy,
            drain_on_exit,
            ref_test,
            replay_barrier,
        })
    }

    pub fn channel(&self) -> EventLoopSender {
        EventLoopSender { sender: self.tx.clone(), poller: self.poll.clone() }
    }

    /// Drain the channel.
    ///
    /// Returns `false` when a shutdown message was received.
    fn drain_recv_channel(&mut self, state: &mut State) -> bool {
        while let Some(msg) = self.rx.recv() {
            match msg {
                Msg::Input(input) => state.write_list.push_back(input),
                Msg::Resize(window_size) => self.pty.on_resize(window_size),
                Msg::Redraw => self.pty.redraw(),
                #[cfg(windows)]
                Msg::SetConsolePalette(palette) => self.pty.set_console_palette(palette),
                Msg::Shutdown => return false,
            }
        }

        true
    }

    #[inline]
    /// Reads what the PTY has ready, up to one chunk, and queues it for the parser thread.
    fn pty_read<X>(
        &mut self,
        state: &mut State,
        parser: &mut ParserThread,
        writer: Option<&mut X>,
    ) -> io::Result<()>
    where
        X: Write,
    {
        if !self.replay_barrier.wait() {
            return Ok(());
        }
        #[cfg(windows)]
        {
            state.profile.read_batches += 1;
        }

        let mut chunk = parser.take_chunk()?;
        let result = loop {
            if chunk.unfilled().is_empty() {
                break Ok(());
            }
            #[cfg(windows)]
            {
                state.profile.read_calls += 1;
            }
            match self.pty.reader().read(chunk.unfilled()) {
                // This is received on Windows/macOS when no more data is readable from the PTY.
                Ok(0) => break Ok(()),
                Ok(got) => {
                    #[cfg(windows)]
                    {
                        state.profile.bytes += got as u64;
                    }
                    chunk.advance(got);
                },
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => continue,
                    // Go back to mio once the PTY would block.
                    ErrorKind::WouldBlock => break Ok(()),
                    _ => break Err(err),
                },
            }
        };

        // Bytes read before an error were still produced by the child, so they are parsed
        // whatever the error means for the next read.
        let read = chunk.filled();

        // Write a copy of the bytes to the ref test file.
        if let Some(writer) = writer {
            writer.write_all(read).unwrap();
        }

        // Detect the one window-manipulation escape sequence Zetta
        // supports before the generic parser discards unsupported xterm
        // window operations. Keeping this scanner in the PTY reader makes
        // it work identically for Unix PTYs and Windows ConPTY.
        state.resize_requests.advance(read, |rows, columns| {
            self.event_proxy.send_event(Event::ResizeRequest { rows, columns });
        });

        // A private OSC carries requests from interactive SSH children.
        // The parser ignores it for display; this scanner reports each
        // complete frame to the owning terminal instead.
        state.clipboard_frames.observe(read, |frame| {
            self.event_proxy.send_event(Event::ClipboardFrame(frame));
        });

        parser.parse(chunk)?;
        result
    }

    #[inline]
    fn pty_write(&mut self, state: &mut State) -> io::Result<()> {
        state.ensure_next();

        'write_many: while let Some(mut current) = state.take_current() {
            'write_one: loop {
                match self.pty.writer().write(current.remaining_bytes()) {
                    Ok(0) => {
                        state.set_current(Some(current));
                        break 'write_many;
                    },
                    Ok(n) => {
                        current.advance(n);
                        if current.finished() {
                            state.goto_next();
                            break 'write_one;
                        }
                    },
                    Err(err) => {
                        state.set_current(Some(current));
                        match err.kind() {
                            ErrorKind::Interrupted | ErrorKind::WouldBlock => break 'write_many,
                            _ => return Err(err),
                        }
                    },
                }
            }
        }

        Ok(())
    }

    pub fn spawn(mut self) -> JoinHandle<(Self, State)> {
        thread::spawn_named("PTY reader", move || {
            let mut state = State::default();
            // A normal child exit and an explicit shutdown both leave the
            // loop intentionally. Any other exit is an infrastructure
            // failure and must be visible to the owning terminal.
            let mut backend_shutdown = true;
            if !self.replay_barrier.wait() {
                // A terminal can be dropped before its first layout. The
                // barrier's abort is the intentional shutdown in that case,
                // not a backend failure to report to the terminal.
                return (self, state);
            }
            // When the master first reported a hangup that produced no child
            // event, so a foreign child's missing exit report can time out
            // instead of being waited on forever.
            let mut hangup_since: Option<Instant> = None;

            let poll_opts = PollMode::Level;
            let mut interest = PollingEvent::readable(0);

            // Register TTY through EventedRW interface.
            if let Err(err) = unsafe { self.pty.register(&self.poll, interest, poll_opts) } {
                error!("Event loop registration error: {err}");
                self.event_proxy.send_event(Event::BackendShutdown);
                return (self, state);
            }

            let mut events = Events::with_capacity(NonZeroUsize::new(1024).unwrap());

            let mut pipe = if self.ref_test {
                Some(File::create("./alacritty.recording").expect("create alacritty recording"))
            } else {
                None
            };

            // Parsing, and with it the synchronized update timeout, happens on its own thread;
            // this one only does the I/O. Every exit below finishes the parser before it reports
            // anything, so the grid holds all the child's output by the time its exit is seen.
            let mut parser = ParserThread::spawn(
                self.terminal.clone(),
                self.event_proxy.clone(),
                self.replay_barrier.clone(),
                std::mem::take(&mut state.parser),
            );

            'event_loop: loop {
                events.clear();
                if let Err(err) = self.poll.wait(&mut events, None) {
                    match err.kind() {
                        ErrorKind::Interrupted => continue,
                        _ => {
                            error!("Event loop polling error: {err}");
                            break 'event_loop;
                        },
                    }
                }

                // Handle channel events, if there are any.
                if !self.drain_recv_channel(&mut state) {
                    backend_shutdown = false;
                    break;
                }

                for event in events.iter() {
                    match event.key {
                        tty::PTY_CHILD_EVENT_TOKEN => {
                            if let Some(child_event) = self.pty.next_child_event() {
                                if self.drain_on_exit {
                                    let _ = self.pty_read(&mut state, &mut parser, pipe.as_mut());
                                }
                                state.finish_parsing(&mut parser);

                                // Report an exit only after the configured final drain. Its
                                // consumer can release the PTY resources as soon as it sees the
                                // event, so reporting first could abort the drain and discard the
                                // child's last output.
                                //
                                // `Term::exit` sends `Event::Exit`, which the owning terminal
                                // reads as "the child ended with no usable status". That is true
                                // of the two exit events and false of a watcher disconnect, where
                                // the child's fate is simply unknown — so a disconnect must not
                                // send it, or it would overrule whatever the consumer decided a
                                // disconnect means.
                                let child_ended =
                                    !matches!(&child_event, tty::ChildEvent::WatcherDisconnected);
                                match child_event {
                                    tty::ChildEvent::Exited(status) => {
                                        self.event_proxy.send_event(Event::ChildExit(status));
                                    },
                                    tty::ChildEvent::ExitStatusUnavailable => {
                                        self.event_proxy
                                            .send_event(Event::ChildExitStatusUnavailable);
                                    },
                                    tty::ChildEvent::WatcherDisconnected => {
                                        self.event_proxy
                                            .send_event(Event::ChildWatcherDisconnected);
                                    },
                                }
                                if child_ended {
                                    self.terminal.lock().exit();
                                }
                                self.event_proxy.send_event(Event::Wakeup);
                                backend_shutdown = false;
                                break 'event_loop;
                            }
                        },

                        tty::PTY_READ_WRITE_TOKEN => {
                            if event.is_interrupt() {
                                // Don't try to do I/O on a dead PTY.
                                if hungup_too_long(&self.pty, &mut hangup_since) {
                                    state.finish_parsing(&mut parser);
                                    self.event_proxy.send_event(Event::ChildExitStatusUnavailable);
                                    self.terminal.lock().exit();
                                    self.event_proxy.send_event(Event::Wakeup);
                                    backend_shutdown = false;
                                    break 'event_loop;
                                }
                                continue;
                            }

                            if event.readable {
                                if let Err(err) =
                                    self.pty_read(&mut state, &mut parser, pipe.as_mut())
                                {
                                    // On Linux, a `read` on the master side of a PTY can fail
                                    // with `EIO` if the client side hangs up.  In that case,
                                    // just loop back round for the inevitable `Exited` event.
                                    // This sucks, but checking the process is either racy or
                                    // blocking.
                                    #[cfg(target_os = "linux")]
                                    if err.raw_os_error() == Some(libc::EIO) {
                                        if hungup_too_long(&self.pty, &mut hangup_since) {
                                            state.finish_parsing(&mut parser);
                                            self.event_proxy
                                                .send_event(Event::ChildExitStatusUnavailable);
                                            self.terminal.lock().exit();
                                            self.event_proxy.send_event(Event::Wakeup);
                                            backend_shutdown = false;
                                            break 'event_loop;
                                        }
                                        continue;
                                    }

                                    error!("Error reading from PTY in event loop: {err}");
                                    break 'event_loop;
                                }
                                // The master produced bytes, so it is not hung up after
                                // all and any earlier hangup must not count towards the
                                // grace period.
                                hangup_since = None;

                                // Adaptors backed by an in-memory pipe must explicitly register
                                // the next wake after this deliberately bounded read batch.
                                if let Err(err) = self.pty.rearm_read() {
                                    error!("Error re-arming PTY read interest: {err}");
                                    break 'event_loop;
                                }
                            }

                            if event.writable
                                && let Err(err) = self.pty_write(&mut state)
                            {
                                error!("Error writing to PTY in event loop: {err}");
                                break 'event_loop;
                            }
                        },
                        _ => (),
                    }
                }

                // Register write interest if necessary.
                let needs_write = state.needs_write();
                if needs_write != interest.writable {
                    interest.writable = needs_write;

                    // Re-register with new interest.
                    if let Err(err) = self.pty.reregister(&self.poll, interest, poll_opts) {
                        error!("Event loop reregistration error: {err}");
                        break 'event_loop;
                    }
                }
            }

            // Whatever was read is parsed before the loop is seen to end: a reader taking over the
            // terminal waits for this thread, and its bytes are newer than these.
            state.finish_parsing(&mut parser);

            // The evented instances are not dropped here so deregister them explicitly.
            let _ = self.pty.deregister(&self.poll);

            if backend_shutdown {
                self.event_proxy.send_event(Event::BackendShutdown);
            }

            #[cfg(windows)]
            if let Ok(path) = std::env::var("ZETTA_PTY_PROFILE_REPORT") {
                let report = format!(
                    "bytes={}\nread_batches={}\nread_calls={}\nparse_calls={}\nparse_ns={}\n",
                    state.profile.bytes,
                    state.profile.read_batches,
                    state.profile.read_calls,
                    state.profile.parse_calls,
                    state.profile.parse_ns,
                );
                let _ = std::fs::write(path, report);
            }

            (self, state)
        })
    }
}

/// Helper type which tracks how much of a buffer has been written.
struct Writing {
    source: Cow<'static, [u8]>,
    written: usize,
}

#[derive(Clone)]
pub struct Notifier(pub EventLoopSender);

impl event::Notify for Notifier {
    fn notify<B>(&self, bytes: B)
    where
        B: Into<Cow<'static, [u8]>>,
    {
        let bytes = bytes.into();
        // Terminal hangs if we send 0 bytes through.
        if bytes.is_empty() {
            return;
        }

        let _ = self.0.send(Msg::Input(bytes));
    }
}

impl event::OnResize for Notifier {
    fn on_resize(&mut self, window_size: WindowSize) {
        let _ = self.0.send(Msg::Resize(window_size));
    }
}

#[derive(Debug)]
pub enum EventLoopSendError {
    /// Error polling the event loop.
    Io(io::Error),

    /// Error sending a message to the event loop.
    Send(mpsc::SendError<Msg>),
}

impl Display for EventLoopSendError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            EventLoopSendError::Io(err) => err.fmt(f),
            EventLoopSendError::Send(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for EventLoopSendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EventLoopSendError::Io(err) => err.source(),
            EventLoopSendError::Send(err) => err.source(),
        }
    }
}

#[derive(Clone)]
pub struct EventLoopSender {
    sender: Sender<Msg>,
    poller: Arc<Poller>,
}

impl EventLoopSender {
    pub fn send(&self, msg: Msg) -> Result<(), EventLoopSendError> {
        self.sender.send(msg).map_err(EventLoopSendError::Send)?;
        self.poller.notify().map_err(EventLoopSendError::Io)
    }
}

/// All of the mutable state needed to run the event loop.
///
/// Contains list of items to write, current write state, etc. Anything that
/// would otherwise be mutated on the `EventLoop` goes here.
#[derive(Default)]
pub struct State {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<Writing>,
    parser: ansi::Processor,
    resize_requests: ResizeRequestParser,
    clipboard_frames: zclip::protocol::Scanner,
    #[cfg(windows)]
    profile: PtyProfile,
}

/// A small streaming recognizer for `CSI 8 ; rows ; columns t`.
///
/// The VTE parser deliberately ignores unsupported window operations, but
/// Zetta owns pane geometry and can implement this one safely. Keeping only
/// partial CSI state means split writes from a PTY are handled without
/// buffering arbitrary terminal output.
#[derive(Default)]
struct ResizeRequestParser {
    state: ResizeRequestParserState,
}

#[derive(Default)]
enum ResizeRequestParserState {
    #[default]
    Ground,
    Escape,
    Csi {
        params: [u32; 3],
        count: usize,
        value: Option<u32>,
        valid: bool,
    },
}

/// The first ESC in `bytes`. `contains` rules out escape-free text a word at a time, where
/// `position` alone tests every byte of it.
#[inline]
fn find_escape(bytes: &[u8]) -> Option<usize> {
    if !bytes.contains(&0x1b) {
        return None;
    }
    bytes.iter().position(|byte| *byte == 0x1b)
}

impl ResizeRequestParser {
    fn advance(&mut self, mut bytes: &[u8], mut on_request: impl FnMut(u16, u16)) {
        // Ordinary output carries no escapes, so `Ground` skips ahead to the
        // next one rather than testing every byte.
        //
        // The skip is re-applied every time the machine returns to `Ground`, not
        // just once on entry. Coloured output and TUIs carry escapes throughout,
        // so a single skip at the start left the whole rest of the buffer going
        // through the state machine a byte at a time — for exactly the traffic
        // this is meant to be cheap for. This scanner runs over every byte the
        // pty produces, in addition to the vte parser.
        while !bytes.is_empty() {
            if matches!(self.state, ResizeRequestParserState::Ground) {
                let Some(escape) = find_escape(bytes) else {
                    return;
                };
                self.state = ResizeRequestParserState::Escape;
                bytes = &bytes[escape + 1..];
                continue;
            }
            let mut consumed = 0;
            for &byte in bytes {
                consumed += 1;
                self.advance_escaped(byte, &mut on_request);
                if matches!(self.state, ResizeRequestParserState::Ground) {
                    break;
                }
            }
            bytes = &bytes[consumed..];
        }
    }

    /// One byte, with the machine already out of `Ground`.
    fn advance_escaped(&mut self, byte: u8, on_request: &mut impl FnMut(u16, u16)) {
        {
            match &mut self.state {
                ResizeRequestParserState::Ground => {
                    if byte == 0x1b {
                        self.state = ResizeRequestParserState::Escape;
                    }
                },
                ResizeRequestParserState::Escape => match byte {
                    b'[' => {
                        self.state = ResizeRequestParserState::Csi {
                            params: [0; 3],
                            count: 0,
                            value: None,
                            valid: true,
                        };
                    },
                    0x1b => {},
                    _ => self.state = ResizeRequestParserState::Ground,
                },
                ResizeRequestParserState::Csi { params, count, value, valid } => match byte {
                    b'0'..=b'9' => {
                        let digit = u32::from(byte - b'0');
                        let next = value
                            .unwrap_or(0)
                            .checked_mul(10)
                            .and_then(|value| value.checked_add(digit));
                        if next.is_none() {
                            *valid = false;
                        }
                        *value = next;
                    },
                    b';' => {
                        if *count < params.len() {
                            params[*count] = value.unwrap_or(0);
                        } else {
                            *valid = false;
                        }
                        *count += 1;
                        *value = None;
                    },
                    b't' => {
                        if *count < params.len() {
                            params[*count] = value.unwrap_or(0);
                        } else {
                            *valid = false;
                        }
                        *count += 1;
                        if *valid
                            && *count == 3
                            && params[0] == 8
                            && let (Ok(rows), Ok(columns)) =
                                (u16::try_from(params[1]), u16::try_from(params[2]))
                            && rows > 0
                            && columns > 0
                        {
                            on_request(rows, columns);
                        }
                        self.state = ResizeRequestParserState::Ground;
                    },
                    0x1b => self.state = ResizeRequestParserState::Escape,
                    _ => self.state = ResizeRequestParserState::Ground,
                },
            }
        }
    }
}

#[cfg(windows)]
#[derive(Default)]
struct PtyProfile {
    bytes: u64,
    read_batches: u64,
    read_calls: u64,
    parse_calls: u64,
    parse_ns: u128,
}

impl State {
    /// Waits for everything read so far to be parsed, and takes the parser back.
    fn finish_parsing(&mut self, parser: &mut ParserThread) {
        let Some(outcome) = parser.finish() else {
            return;
        };
        self.parser = outcome.parser;
        #[cfg(windows)]
        {
            self.profile.parse_calls += outcome.parse_calls;
            self.profile.parse_ns += outcome.parse_time.as_nanos();
        }
    }

    #[inline]
    fn ensure_next(&mut self) {
        if self.writing.is_none() {
            self.goto_next();
        }
    }

    #[inline]
    fn goto_next(&mut self) {
        self.writing = self.write_list.pop_front().map(Writing::new);
    }

    #[inline]
    fn take_current(&mut self) -> Option<Writing> {
        self.writing.take()
    }

    #[inline]
    fn needs_write(&self) -> bool {
        self.writing.is_some() || !self.write_list.is_empty()
    }

    #[inline]
    fn set_current(&mut self, new: Option<Writing>) {
        self.writing = new;
    }
}

impl Writing {
    #[inline]
    fn new(c: Cow<'static, [u8]>) -> Writing {
        Writing { source: c, written: 0 }
    }

    #[inline]
    fn advance(&mut self, n: usize) {
        self.written += n;
    }

    #[inline]
    fn remaining_bytes(&self) -> &[u8] {
        &self.source[self.written..]
    }

    #[inline]
    fn finished(&self) -> bool {
        self.written >= self.source.len()
    }
}

struct PeekableReceiver<T> {
    rx: Receiver<T>,
    peeked: Option<T>,
}

impl<T> PeekableReceiver<T> {
    fn new(rx: Receiver<T>) -> Self {
        Self { rx, peeked: None }
    }

    fn recv(&mut self) -> Option<T> {
        if self.peeked.is_some() {
            self.peeked.take()
        } else {
            match self.rx.try_recv() {
                Err(TryRecvError::Disconnected) => panic!("event loop channel closed"),
                res => res.ok(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_barrier_releases_waiters_in_order() {
        let barrier = ReplayBarrier::closed();
        let waiter_barrier = barrier.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            waiter_barrier.wait()
        });
        started_rx.recv().unwrap();
        barrier.release();
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn replay_barrier_aborts_waiters_without_reopening() {
        let barrier = ReplayBarrier::closed();
        let waiter_barrier = barrier.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            waiter_barrier.wait()
        });
        started_rx.recv().unwrap();
        barrier.abort();
        assert!(!waiter.join().unwrap());
        barrier.release();
        assert!(!barrier.wait());
    }

    /// The skip has to be re-applied every time the machine returns to
    /// `Ground`. Coloured output puts escapes throughout the buffer, so a
    /// sequence that arrives after earlier escapes is the normal case, not an
    /// edge case.
    #[test]
    fn resize_request_parser_finds_a_request_after_earlier_escapes() {
        let mut parser = ResizeRequestParser::default();
        let mut seen = Vec::new();
        parser.advance(
            b"\x1b[31mred\x1b[0m plain \x1b[1;32mgreen\x1b[0m\x1b[8;24;80t after",
            |rows, columns| seen.push((rows, columns)),
        );
        assert_eq!(seen, vec![(24, 80)]);
    }

    /// Escapes that are not window operations must leave the machine back in
    /// `Ground` so the next skip starts from the right place.
    #[test]
    fn resize_request_parser_returns_to_ground_between_sequences() {
        let mut parser = ResizeRequestParser::default();
        let mut seen = Vec::new();
        parser.advance(b"\x1b[0m\x1b[8;10;20t\x1b[0m\x1b[8;30;40t", |rows, columns| {
            seen.push((rows, columns))
        });
        assert_eq!(seen, vec![(10, 20), (30, 40)]);
    }

    /// A buffer of ordinary output must not be walked byte by byte, and must
    /// leave no state behind for the next one.
    #[test]
    fn resize_request_parser_ignores_output_without_escapes() {
        let mut parser = ResizeRequestParser::default();
        let mut seen = Vec::new();
        parser.advance(b"no escapes here at all", |rows, columns| seen.push((rows, columns)));
        assert!(seen.is_empty());
        parser.advance(b"\x1b[8;5;6t", |rows, columns| seen.push((rows, columns)));
        assert_eq!(seen, vec![(5, 6)]);
    }

    #[test]
    fn resize_request_parser_handles_split_xterm_sequences() {
        let mut parser = ResizeRequestParser::default();
        let mut requests = Vec::new();

        parser.advance(b"ignored\x1b[8;4", |rows, columns| {
            requests.push((rows, columns));
        });
        parser.advance(b"0;120t", |rows, columns| {
            requests.push((rows, columns));
        });

        assert_eq!(requests, [(40, 120)]);
    }

    #[test]
    fn resize_request_parser_ignores_invalid_window_operations() {
        let mut parser = ResizeRequestParser::default();
        let mut requests = Vec::new();

        parser.advance(b"\x1b[8;0;120t\x1b[8;40;70000t\x1b[18t", |rows, columns| {
            requests.push((rows, columns));
        });

        assert!(requests.is_empty());
    }
    /// A stand-in that only has to answer whether its child is somebody else's.
    struct StubPty(bool);

    impl crate::tty::EventedReadWrite for StubPty {
        type Reader = std::io::Empty;
        type Writer = std::io::Sink;

        unsafe fn register(
            &mut self,
            _: &Arc<Poller>,
            _: PollingEvent,
            _: PollMode,
        ) -> io::Result<()> {
            Ok(())
        }
        fn reregister(&mut self, _: &Arc<Poller>, _: PollingEvent, _: PollMode) -> io::Result<()> {
            Ok(())
        }
        fn deregister(&mut self, _: &Arc<Poller>) -> io::Result<()> {
            Ok(())
        }
        fn reader(&mut self) -> &mut Self::Reader {
            unimplemented!("the hangup decision reads nothing")
        }
        fn writer(&mut self) -> &mut Self::Writer {
            unimplemented!("the hangup decision writes nothing")
        }
    }

    impl tty::EventedPty for StubPty {
        fn child_is_foreign(&self) -> bool {
            self.0
        }
        fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
            None
        }
    }

    #[test]
    fn an_owned_childs_hangup_is_always_waited_out() {
        // Upstream's behaviour, unchanged: the exit notification for a child
        // this process spawned is already on its way, so a hung-up master is a
        // reason to loop back round rather than to give up. Nothing is even
        // recorded, so a later change of mind cannot inherit a stale deadline.
        let mut since = None;
        assert!(!hungup_too_long(&StubPty(false), &mut since));
        assert!(since.is_none());

        let mut long_ago = Some(Instant::now() - FOREIGN_CHILD_HANGUP_GRACE * 10);
        assert!(!hungup_too_long(&StubPty(false), &mut long_ago));
    }

    #[test]
    fn a_foreign_childs_hangup_is_given_a_bounded_grace_period() {
        // The first hangup starts the clock and waits: an exit report that is
        // merely slow must still win.
        let mut since = None;
        assert!(!hungup_too_long(&StubPty(true), &mut since));
        assert!(since.is_some(), "the grace period never started");

        // Once it has run out there is nothing left to wait for. Waiting anyway
        // is what left a pane accepting no input, showing no exit, and
        // impossible to close after its multiplexer stopped reporting.
        let mut elapsed = Some(Instant::now() - FOREIGN_CHILD_HANGUP_GRACE);
        assert!(hungup_too_long(&StubPty(true), &mut elapsed));
    }

    /// Output from a real child through a real pty, into an `EventLoop` exactly as the
    /// application spawns one.
    #[cfg(unix)]
    mod through_a_pty {
        use std::time::Duration;

        use super::*;
        use crate::grid::Dimensions;
        use crate::index::Column;
        use crate::term::Config;
        use crate::term::test::TermSize;

        /// What the grid showed at the moment the child's exit was reported.
        struct Exit {
            at: Instant,
            cursor_line: String,
        }

        /// Records the child's exit report, reading the grid as it arrives: the application acts
        /// on the report itself, not on whatever the grid shows a moment later.
        #[derive(Clone, Default)]
        struct ExitListener(Arc<ExitState>);

        #[derive(Default)]
        struct ExitState {
            terminal: std::sync::OnceLock<std::sync::Weak<FairMutex<Term<ExitListener>>>>,
            exit: Mutex<Option<Exit>>,
            changed: Condvar,
        }

        impl EventListener for ExitListener {
            fn send_event(&self, event: Event) {
                if matches!(event, Event::ChildExit(_) | Event::ChildExitStatusUnavailable) {
                    let at = Instant::now();
                    let terminal = self.0.terminal.get().and_then(std::sync::Weak::upgrade);
                    let cursor_line = terminal
                        .map(|terminal| cursor_line_text(&terminal.lock()))
                        .unwrap_or_default();
                    *self.0.exit.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(Exit { at, cursor_line });
                    self.0.changed.notify_all();
                }
            }
        }

        impl ExitListener {
            fn wait(&self) -> Exit {
                let exit = self.0.exit.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let (mut exit, timeout) = self
                    .0
                    .changed
                    .wait_timeout_while(exit, Duration::from_secs(120), |exit| exit.is_none())
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                assert!(!timeout.timed_out(), "the child's exit was never reported");
                exit.take().unwrap()
            }
        }

        /// Runs `sh -c script` in a `columns`x`lines` terminal, keeping the terminal locked for
        /// `held` after the event loop starts so that its parser falls behind. Returns the line
        /// the cursor was on when the exit was reported, and how long the report took from
        /// spawning the child.
        fn run(
            script: &str,
            columns: usize,
            lines: usize,
            history: usize,
            held: Duration,
        ) -> (String, Duration) {
            let listener = ExitListener::default();
            let config = Config { scrolling_history: history, ..Config::default() };
            let terminal = Arc::new(FairMutex::new(Term::new(
                config,
                &TermSize::new(columns, lines),
                listener.clone(),
            )));
            listener.0.terminal.set(Arc::downgrade(&terminal)).unwrap();
            let options = tty::Options {
                shell: Some(tty::Shell::new("sh".into(), vec!["-c".into(), script.into()])),
                drain_on_exit: true,
                ..tty::Options::default()
            };
            let window_size = WindowSize {
                num_lines: lines as u16,
                num_cols: columns as u16,
                cell_width: 8,
                cell_height: 16,
            };
            let started = Instant::now();
            let pty = tty::new(&options, window_size, 0).unwrap();
            let event_loop = EventLoop::new(
                terminal.clone(),
                listener.clone(),
                pty,
                true,
                false,
                ReplayBarrier::open(),
            )
            .unwrap();
            let held_terminal = terminal.lock();
            let join = event_loop.spawn();
            std::thread::sleep(held);
            drop(held_terminal);
            let exit = listener.wait();
            join.join().unwrap();
            (exit.cursor_line, exit.at - started)
        }

        fn cursor_line_text(term: &Term<ExitListener>) -> String {
            let line = term.grid().cursor.point.line;
            (0..term.columns()).map(|column| term.grid()[line][Column(column)].c).collect()
        }

        fn payload_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
            let path = std::env::temp_dir()
                .join(format!("alacritty-event-loop-{}-{name}", std::process::id()));
            std::fs::write(&path, bytes).unwrap();
            path
        }

        /// Seeded random bytes, as `cat` of a binary file delivers them.
        fn random_bytes(len: usize) -> Vec<u8> {
            let mut state = 0x5117_u64;
            (0..len)
                .map(|_| {
                    state =
                        state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    (state >> 56) as u8
                })
                .collect()
        }

        /// The application closes a pane on the exit report, so the child's last output has to
        /// be on the grid by then, however far the parser had fallen behind the reads. Holding
        /// the terminal while the child runs puts the whole output in the parser's queue, which
        /// still fits it, so the child has exited before the parser starts.
        #[test]
        fn the_last_output_is_on_the_grid_when_the_exit_is_reported() {
            let line = [b'x'; 79].iter().copied().chain(*b"\n").collect::<Vec<_>>();
            let path = payload_file("tail", &line.repeat(768 * 1024 / line.len()));
            let script = format!("cat '{}'; printf 'the end'", path.display());
            let (text, _) = run(&script, 80, 24, 500, Duration::from_millis(300));
            std::fs::remove_file(&path).unwrap();
            assert_eq!(text.trim_end(), "the end");
        }

        fn throughput(name: &str, payload: &[u8]) {
            let path = payload_file(name, payload);
            let script = format!("cat '{}'", path.display());
            let (_, elapsed) = run(&script, 80, 24, 500, Duration::ZERO);
            std::fs::remove_file(&path).unwrap();
            let mib = payload.len() as f64 / (1024.0 * 1024.0);
            eprintln!(
                "{name}: {mib:.0} MiB in {elapsed:.2?}, {:.0} MiB/s",
                mib / elapsed.as_secs_f64()
            );
        }

        /// The ASCII half of the comparison against other terminals: `cat` of one repeated
        /// 80-column line into an 80x24 grid with 500 lines of history.
        #[test]
        #[ignore = "manual optimized-build throughput check"]
        fn ascii_through_the_event_loop_throughput_benchmark() {
            let line = (0..80u8).map(|index| b' ' + index % 95).chain(*b"\n").collect::<Vec<_>>();
            throughput("ascii", &line.repeat(256 * 1024 * 1024 / line.len()));
        }

        /// The random half of the same comparison.
        #[test]
        #[ignore = "manual optimized-build throughput check"]
        fn random_through_the_event_loop_throughput_benchmark() {
            throughput("random", &random_bytes(32 * 1024 * 1024));
        }
    }
}
