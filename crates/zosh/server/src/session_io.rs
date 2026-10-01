//! The session loop's I/O: its UDP socket, its end of the PTY, and how it
//! waits on both.
//!
//! On Unix both are descriptors, so the loop reads and writes them itself,
//! nonblocking, and waits for either — and for the wake-up pipe the remaining
//! helper threads write to — in one `poll`, the way stock mosh-server waits in
//! one `select`. Handing every PTY read and every datagram across a channel
//! from a thread of its own cost more than the work it handed over: profiled
//! while htop was scrolled, half the server's time was in the kernel, a third
//! of it futex wake-ups and the context switches around them.
//!
//! On Windows a ConPTY's pipes cannot be waited on together with a socket, so
//! there each input keeps a thread that blocks on it and wakes the parked loop
//! (see `wake`). Unix falls back to the same threads for a PTY whose master
//! has no descriptor, which `portable_pty`'s native backend never produces.

use crate::timing;
use crate::wake::{WakeDeadline, Waker, WakingSender};
use anyhow::{Result, bail};
use portable_pty::MasterPty;
#[cfg(unix)]
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
#[cfg(windows)]
use std::time::Duration;

pub const PTY_CHUNK: usize = 8192;
pub const PTY_QUEUE_DEPTH: usize = 256;
#[cfg(windows)]
const UDP_QUEUE_DEPTH: usize = 256;
#[cfg(windows)]
const UDP_BUFFER: usize = 65_535;
// The Windows UDP reader thread blocks on the socket, and a socket's blocking
// mode is shared by every handle to it, so the loop's sends would block too.
// This bounds them instead: a send that cannot go out at once fails the way a
// nonblocking one would, and SSP retransmits it.
#[cfg(windows)]
const UDP_SEND_TIMEOUT: Duration = Duration::from_millis(2);

/// What the PTY hands the session loop.
pub enum PtyEvent {
    Output(Vec<u8>),
    /// Every byte queued ahead of this input frame has reached the PTY.
    InputWritten(u64),
    Eof,
    Error(String),
}

/// What the session loop hands the PTY, in order.
pub enum PtyWrite {
    Bytes(Vec<u8>),
    /// A marker: once every byte queued before it is written, the PTY end
    /// reports `PtyEvent::InputWritten` for this frame.
    InputFrame(u64),
}

/// The loop's end of the PTY.
pub enum PtyIo {
    /// Read and written by the loop itself, through a nonblocking master.
    #[cfg(unix)]
    Direct(DirectPty),
    /// A reader and a writer thread, each waking the loop.
    Threads {
        events: Receiver<PtyEvent>,
        writes: SyncSender<PtyWrite>,
    },
}

impl PtyIo {
    /// The loop's end of `master`, waking it through `waker` where a thread
    /// is involved.
    pub fn open(master: &dyn MasterPty, waker: &Waker) -> Result<Self> {
        #[cfg(unix)]
        if let Some(fd) = master.as_raw_fd() {
            return Ok(Self::Direct(DirectPty::new(fd)?));
        }
        Self::threads(master, waker)
    }

    fn threads(master: &dyn MasterPty, waker: &Waker) -> Result<Self> {
        use anyhow::Context as _;

        let reader = master.try_clone_reader().context("cloning PTY reader")?;
        let writer = master.take_writer().context("taking PTY writer")?;
        let (event_tx, events) = mpsc::sync_channel(PTY_QUEUE_DEPTH);
        let event_sender = WakingSender::new(event_tx, waker.clone());
        let (writes, write_rx) = mpsc::sync_channel(PTY_QUEUE_DEPTH);
        spawn_pty_reader(reader, event_sender.clone());
        spawn_pty_writer(writer, write_rx, event_sender);
        Ok(Self::Threads { events, writes })
    }

    /// The next thing the PTY has for the loop, without waiting.
    pub fn next_event(&mut self) -> Option<PtyEvent> {
        match self {
            #[cfg(unix)]
            Self::Direct(pty) => pty.next_event(),
            Self::Threads { events, .. } => match events.try_recv() {
                Ok(event) => Some(event),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(PtyEvent::Eof),
            },
        }
    }

    /// Queue a write. It fails only when the queue is full, which means the
    /// program has stopped reading its input, or when the PTY is gone.
    pub fn queue(&mut self, request: PtyWrite) -> Result<()> {
        match self {
            #[cfg(unix)]
            Self::Direct(pty) => pty.queue(request),
            Self::Threads { writes, .. } => match writes.try_send(request) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_)) => saturated(),
                Err(TrySendError::Disconnected(_)) => bail!("PTY writer has stopped"),
            },
        }
    }

    /// Write whatever the PTY will now take. The threaded end writes on its
    /// own.
    pub fn flush(&mut self) {
        #[cfg(unix)]
        if let Self::Direct(pty) = self {
            pty.flush();
        }
    }
}

fn saturated() -> Result<()> {
    bail!("PTY input queue saturated; child is not consuming terminal input")
}

/// A PTY master the loop reads and writes itself.
///
/// The descriptor is borrowed from the `MasterPty` it came from, which the
/// session keeps for resizing; this never closes it.
#[cfg(unix)]
pub struct DirectPty {
    fd: std::os::fd::RawFd,
    writes: VecDeque<PtyWrite>,
    /// How much of the front write has gone out.
    written: usize,
    /// Events produced by writing, reported ahead of anything read.
    pending: VecDeque<PtyEvent>,
    /// Set once the PTY has ended; it is then neither read nor watched.
    closed: bool,
    buffer: Box<[u8; PTY_CHUNK]>,
}

#[cfg(unix)]
impl DirectPty {
    fn new(fd: std::os::fd::RawFd) -> Result<Self> {
        use anyhow::Context as _;

        // SAFETY: only reads and replaces the flags of a descriptor the
        // caller's `MasterPty` owns.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(io::Error::last_os_error()).context("making the PTY nonblocking");
            }
        }
        Ok(Self {
            fd,
            writes: VecDeque::new(),
            written: 0,
            pending: VecDeque::new(),
            closed: false,
            buffer: Box::new([0; PTY_CHUNK]),
        })
    }

    fn next_event(&mut self) -> Option<PtyEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }
        if self.closed {
            return None;
        }
        loop {
            // SAFETY: the buffer is as long as the count passed with it, and
            // the descriptor is the live master.
            let read = unsafe { libc::read(self.fd, self.buffer.as_mut_ptr().cast(), PTY_CHUNK) };
            if read > 0 {
                let read = read as usize;
                timing::record("pty_read", read as u64, 0);
                return Some(PtyEvent::Output(self.buffer[..read].to_vec()));
            }
            if read == 0 {
                timing::record("pty_eof", 0, 0);
                self.closed = true;
                return Some(PtyEvent::Eof);
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::WouldBlock => return None,
                // Linux reports the last slave closing as EIO: the program
                // and everything it started have gone, which is an end of
                // file rather than a failure.
                _ if error.raw_os_error() == Some(libc::EIO) => {
                    timing::record("pty_eof", 0, 0);
                    self.closed = true;
                    return Some(PtyEvent::Eof);
                }
                _ => {
                    self.closed = true;
                    return Some(PtyEvent::Error(error.to_string()));
                }
            }
        }
    }

    fn queue(&mut self, request: PtyWrite) -> Result<()> {
        if self.closed {
            bail!("PTY writer has stopped");
        }
        if self.writes.len() >= PTY_QUEUE_DEPTH {
            return saturated();
        }
        self.writes.push_back(request);
        self.flush();
        Ok(())
    }

    fn flush(&mut self) {
        while !self.closed {
            match self.writes.front() {
                None => return,
                Some(PtyWrite::InputFrame(frame)) => {
                    timing::record("input_written", *frame, 0);
                    self.pending.push_back(PtyEvent::InputWritten(*frame));
                    self.writes.pop_front();
                }
                Some(PtyWrite::Bytes(data)) => {
                    if self.written == 0 {
                        timing::record("pty_write_begin", data.len() as u64, 0);
                    }
                    let rest = &data[self.written..];
                    // SAFETY: `rest` is a live slice and the count is its
                    // length; the descriptor is the live master.
                    let wrote = unsafe { libc::write(self.fd, rest.as_ptr().cast(), rest.len()) };
                    if wrote >= 0 {
                        self.written += wrote as usize;
                        if self.written == data.len() {
                            timing::record("pty_write_end", data.len() as u64, 0);
                            self.writes.pop_front();
                            self.written = 0;
                        }
                        continue;
                    }
                    let error = io::Error::last_os_error();
                    match error.kind() {
                        io::ErrorKind::Interrupted => {}
                        io::ErrorKind::WouldBlock => return,
                        _ => {
                            timing::record("pty_write_error", 0, 0);
                            self.closed = true;
                            self.writes.clear();
                            self.pending.push_back(PtyEvent::Error(error.to_string()));
                        }
                    }
                }
            }
        }
    }

    /// What `poll` should watch the master for, if anything.
    fn interest(&self, read: bool) -> Option<libc::pollfd> {
        if self.closed {
            return None;
        }
        let mut events = 0;
        if read {
            events |= libc::POLLIN;
        }
        if !self.writes.is_empty() {
            events |= libc::POLLOUT;
        }
        (events != 0).then_some(libc::pollfd {
            fd: self.fd,
            events,
            revents: 0,
        })
    }
}

/// The session's UDP socket, as the loop reads it.
pub struct UdpIo {
    socket: UdpSocket,
    #[cfg(windows)]
    events: Receiver<UdpEvent>,
}

/// What the Windows UDP reader thread hands the session loop.
#[cfg(windows)]
enum UdpEvent {
    Datagram {
        bytes: Vec<u8>,
        from: SocketAddr,
    },
    /// The socket failed; the reader has stopped.
    Failed(io::Error),
}

impl UdpIo {
    pub fn new(socket: UdpSocket, waker: &Waker) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = waker;
            socket.set_nonblocking(true)?;
            Ok(Self { socket })
        }
        #[cfg(windows)]
        {
            socket.set_write_timeout(Some(UDP_SEND_TIMEOUT))?;
            let events = spawn_udp_reader(socket.try_clone()?, waker.clone())?;
            Ok(Self { socket, events })
        }
    }

    /// The next datagram, copied into `buffer`, without waiting.
    pub fn recv(&mut self, buffer: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        #[cfg(unix)]
        loop {
            match self.socket.recv_from(buffer) {
                Ok(received) => return Ok(Some(received)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        #[cfg(windows)]
        match self.events.try_recv() {
            Ok(UdpEvent::Datagram { bytes, from }) => {
                let length = bytes.len().min(buffer.len());
                buffer[..length].copy_from_slice(&bytes[..length]);
                Ok(Some((length, from)))
            }
            Ok(UdpEvent::Failed(error)) => Err(error),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(io::Error::other("the UDP reader stopped")),
        }
    }

    pub fn send_to(&self, datagram: &[u8], peer: SocketAddr) -> io::Result<usize> {
        self.socket.send_to(datagram, peer)
    }
}

/// How the loop waits for its socket, its PTY and its wake-ups.
pub struct LoopWait {
    #[cfg(unix)]
    pipe: crate::wake::WakePipe,
    #[cfg(unix)]
    descriptors: Vec<libc::pollfd>,
    #[cfg(windows)]
    waker: Waker,
}

impl LoopWait {
    /// Built on the loop's own thread, after the Unix bootstrap has forked.
    pub fn new() -> io::Result<Self> {
        #[cfg(unix)]
        return Ok(Self {
            pipe: crate::wake::WakePipe::new()?,
            descriptors: Vec::with_capacity(3),
        });
        #[cfg(windows)]
        return Ok(Self {
            waker: Waker::current_thread(),
        });
    }

    /// What another thread wakes the loop with.
    pub fn waker(&self) -> Waker {
        #[cfg(unix)]
        return self.pipe.waker();
        #[cfg(windows)]
        return self.waker.clone();
    }

    /// Waits until the socket or the PTY has something, a waker fires, or
    /// `deadline` passes. `read_pty` is false while the loop is holding the
    /// program back, so output it will not read does not end the wait.
    #[cfg_attr(
        windows,
        allow(
            unused_variables,
            reason = "the Windows loop parks; its inputs wake it from their threads"
        )
    )]
    pub fn wait(
        &mut self,
        deadline: WakeDeadline,
        udp: &UdpIo,
        pty: Option<&PtyIo>,
        read_pty: bool,
    ) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;

            let readable = |fd| libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            self.descriptors.clear();
            self.descriptors.push(readable(self.pipe.descriptor()));
            self.descriptors.push(readable(udp.socket.as_raw_fd()));
            if let Some(PtyIo::Direct(pty)) = pty
                && let Some(interest) = pty.interest(read_pty)
            {
                self.descriptors.push(interest);
            }
            let timeout = deadline.earliest().map_or(-1, |at| {
                let remaining = at.saturating_duration_since(std::time::Instant::now());
                i32::try_from(remaining.as_micros().div_ceil(1_000)).unwrap_or(i32::MAX)
            });
            // SAFETY: every descriptor is borrowed from a live pipe, socket or
            // PTY master, and the vector stays allocated until poll returns.
            let result = unsafe {
                libc::poll(
                    self.descriptors.as_mut_ptr(),
                    self.descriptors.len() as libc::nfds_t,
                    timeout,
                )
            };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            if self.descriptors[0].revents != 0 {
                self.pipe.drain();
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            deadline.park();
            Ok(())
        }
    }
}

/// Sets `O_NONBLOCK` and `FD_CLOEXEC` on a descriptor the caller owns.
#[cfg(unix)]
pub fn set_nonblocking_cloexec(descriptor: &std::os::fd::OwnedFd) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;

    let fd = descriptor.as_raw_fd();
    // SAFETY: `fd` is owned by the caller and both calls only read or
    // replace its flags.
    unsafe {
        if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Starts the thread that blocks on the session's UDP socket, waking the loop
/// with each datagram.
#[cfg(windows)]
fn spawn_udp_reader(socket: UdpSocket, waker: Waker) -> io::Result<Receiver<UdpEvent>> {
    let (sender, events) = mpsc::sync_channel(UDP_QUEUE_DEPTH);
    let sender = WakingSender::new(sender, waker);
    thread::Builder::new()
        .name("zosh-udp-reader".to_owned())
        .spawn(move || {
            let mut buf = vec![0u8; UDP_BUFFER];
            loop {
                let event = match socket.recv_from(&mut buf) {
                    Ok((n, from)) => UdpEvent::Datagram {
                        bytes: buf[..n].to_vec(),
                        from,
                    },
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => UdpEvent::Failed(error),
                };
                let failed = matches!(event, UdpEvent::Failed(_));
                if sender.send(event).is_err() || failed {
                    return;
                }
            }
        })?;
    Ok(events)
}

fn spawn_pty_reader(mut reader: Box<dyn Read + Send>, tx: WakingSender<PtyEvent>) {
    thread::spawn(move || {
        let mut buf = [0u8; PTY_CHUNK];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    timing::record("pty_eof", 0, 0);
                    let _ = tx.send(PtyEvent::Eof);
                    break;
                }
                Ok(n) => {
                    timing::record("pty_read", n as u64, 0);
                    if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = tx.send(PtyEvent::Error(error.to_string()));
                    break;
                }
            }
        }
    });
}

pub fn spawn_pty_writer(
    mut writer: Box<dyn Write + Send>,
    rx: Receiver<PtyWrite>,
    event_tx: WakingSender<PtyEvent>,
) {
    thread::spawn(move || {
        while let Ok(request) = rx.recv() {
            match request {
                PtyWrite::Bytes(data) => {
                    timing::record("pty_write_begin", data.len() as u64, 0);
                    if let Err(error) = writer.write_all(&data) {
                        timing::record("pty_write_error", 0, 0);
                        let _ = event_tx.send(PtyEvent::Error(error.to_string()));
                        return;
                    }
                    timing::record("pty_write_end", data.len() as u64, 0);
                }
                PtyWrite::InputFrame(frame) => {
                    timing::record("input_written", frame, 0);
                    if event_tx.send(PtyEvent::InputWritten(frame)).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

#[cfg(test)]
#[path = "tests/session_io.rs"]
mod tests;
