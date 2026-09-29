//! Many daemon connections over one byte stream.
//!
//! Win32-OpenSSH forwards Unix sockets in neither direction, so a Windows
//! client — or any client of a Windows host — reaches a remote daemon through
//! `ssh HOST zmux proxy-mux` instead: one SSH session whose stdin and stdout
//! carry every connection the client opens. This module is both halves of that
//! link. It is platform-neutral — either half runs on either OS, and nothing
//! here depends on which is which, so both are tested together.
//!
//! The daemon protocol is untouched: each logical stream is one ordinary
//! daemon connection, byte for byte, and the far side opens a fresh one for
//! every stream the near side opens. What this adds is the framing around it:
//!
//! ```text
//! far  → near   HELLO (8 bytes), an Info frame, then frames
//! frame         stream: u32 BE | kind: u8 | length: u32 BE | payload
//! ```
//!
//! Stream 0 is the link itself and carries [`BridgeInfo`] requests and
//! replies. Every other stream is opened by the near side, so ids never
//! collide and are never reused.
//!
//! **Flow control is per stream.** A shared pane's output can arrive far faster
//! than its terminal drains it; without a window, one stalled pane would sit in
//! front of every control reply on the same link. Each direction of each
//! stream starts with [`WINDOW`] bytes of credit, a sender never exceeds what it
//! has been granted, and a receiver grants credit back only once the bytes are
//! written into the local socket. So the link reader never blocks on a local
//! socket, and what is buffered per stream is bounded by the window.
//!
//! **The link reader never waits on the link writer.** Everything it has to
//! send in reply (a `Close` for a refused open, an info reply) is written from
//! another thread, because a reader blocked on a full outbound pipe while the
//! peer's reader is blocked the same way is a deadlock across the SSH link.

use std::time::Duration;
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::Shutdown,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    thread,
};

use anyhow::{Context as _, Result};

use crate::transport::{Endpoint, Stream};

/// Sent first by the far side, so the near side can tell a bridge from a
/// remote shell that printed something else — an older `zmux` that does not
/// know `proxy-mux` exits with an error instead.
pub(crate) const HELLO: &[u8; 8] = b"ZMUXMUX1";
const HEADER_BYTES: usize = 9;
/// The largest payload one frame carries. Small enough that one stream's bulk
/// output interleaves with other streams' replies at a fine grain.
pub(crate) const MAX_PAYLOAD: usize = 32 * 1024;
/// Credit each direction of a stream starts with.
pub(crate) const WINDOW: u32 = 256 * 1024;
/// A frame whose declared length exceeds this is a corrupt link, not a frame.
const MAX_FRAME: u32 = 1024 * 1024;
const LINK_STREAM: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    /// Near → far: connect this stream to the daemon.
    Open = 1,
    Data = 2,
    /// The sender will write no more on this stream; the receiver half-closes.
    Eof = 3,
    /// The stream is gone in both directions.
    Close = 4,
    /// The receiver has written this many more bytes locally.
    Credit = 5,
    /// Stream 0 only: ask for, or answer with, a [`BridgeInfo`].
    Info = 6,
}

impl Kind {
    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::Open,
            2 => Self::Data,
            3 => Self::Eof,
            4 => Self::Close,
            5 => Self::Credit,
            6 => Self::Info,
            _ => return None,
        })
    }
}

struct Frame {
    stream: u32,
    kind: Kind,
    payload: Vec<u8>,
}

fn encode_frame(stream: u32, kind: Kind, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES + payload.len());
    bytes.extend_from_slice(&stream.to_be_bytes());
    bytes.push(kind as u8);
    bytes.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("frame payloads are bounded by MAX_PAYLOAD")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(payload);
    bytes
}

/// Reads one frame, or `None` at a clean end of the link between frames.
fn read_frame(reader: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut header = [0; HEADER_BYTES];
    let mut filled = 0;
    while filled < HEADER_BYTES {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let stream = u32::from_be_bytes(header[0..4].try_into().expect("four header bytes"));
    let kind = Kind::from_byte(header[4]).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown bridge frame kind {}", header[4]),
        )
    })?;
    let length = u32::from_be_bytes(header[5..9].try_into().expect("four header bytes"));
    if length > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bridge frame of {length} bytes exceeds the {MAX_FRAME}-byte limit"),
        ));
    }
    let mut payload = vec![0; length as usize];
    reader.read_exact(&mut payload)?;
    Ok(Some(Frame {
        stream,
        kind,
        payload,
    }))
}

/// What the far side knows that the near side would otherwise need a second
/// SSH login to ask: which daemon is running, and where the far side's own
/// `zmux` is installed.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BridgeInfo {
    /// The far side's own executable, which is the `zmux` a daemon start or a
    /// Mosh relay should run.
    pub(crate) program: Option<PathBuf>,
    /// The running daemon, or `None` when there is none — the reason is in
    /// `error`.
    pub(crate) endpoint: Option<Endpoint>,
    pub(crate) error: Option<String>,
}

/// The direction-independent state of one stream.
struct Channel {
    /// Kept to shut the local socket down when the peer resets the stream or
    /// the link ends, which is what wakes a pump blocked reading it.
    local: Stream,
    credit: Mutex<Credit>,
    credit_changed: Condvar,
    /// Where the link reader hands this stream's inbound bytes. Taken (and so
    /// closed) once the peer will send nothing more.
    inbound: Mutex<Option<mpsc::Sender<Inbound>>>,
    /// Pumps still running; the last one out removes the stream.
    pumps: AtomicU8,
}

struct Credit {
    available: u32,
    closed: bool,
}

enum Inbound {
    Data(Vec<u8>),
    Eof,
}

impl Channel {
    fn new(local: Stream, inbound: mpsc::Sender<Inbound>) -> Self {
        Self {
            local,
            credit: Mutex::new(Credit {
                available: WINDOW,
                closed: false,
            }),
            credit_changed: Condvar::new(),
            inbound: Mutex::new(Some(inbound)),
            pumps: AtomicU8::new(2),
        }
    }

    fn grant(&self, bytes: u32) {
        let mut credit = self
            .credit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        credit.available = credit.available.saturating_add(bytes);
        self.credit_changed.notify_all();
    }

    /// Waits for credit and takes up to `wanted` of it, or `None` once the
    /// stream is gone.
    fn take_credit(&self, wanted: usize) -> Option<usize> {
        let mut credit = self
            .credit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if credit.closed {
                return None;
            }
            if credit.available > 0 {
                let taken = wanted.min(credit.available as usize);
                credit.available -= taken as u32;
                return Some(taken);
            }
            credit = self
                .credit_changed
                .wait(credit)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn deliver(&self, inbound: Inbound) {
        let sender = self
            .inbound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(inbound);
        }
    }

    /// Ends the stream in both directions without waiting for either pump.
    fn reset(&self) {
        {
            let mut credit = self
                .credit
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            credit.closed = true;
            self.credit_changed.notify_all();
        }
        self.inbound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let _ = self.local.shutdown(Shutdown::Both);
    }
}

/// What both halves share: the link writer and the open streams.
struct Link {
    writer: Mutex<Box<dyn Write + Send>>,
    streams: Mutex<HashMap<u32, Arc<Channel>>>,
    closed: AtomicBool,
}

impl Link {
    fn new(writer: Box<dyn Write + Send>) -> Arc<Self> {
        Arc::new(Self {
            writer: Mutex::new(writer),
            streams: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        })
    }

    fn send(&self, stream: u32, kind: Kind, payload: &[u8]) -> io::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let bytes = encode_frame(stream, kind, payload);
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = writer.write_all(&bytes).and_then(|()| writer.flush());
        if result.is_err() {
            self.closed.store(true, Ordering::Release);
        }
        result
    }

    fn channel(&self, stream: u32) -> Option<Arc<Channel>> {
        self.streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&stream)
            .cloned()
    }

    fn remove(&self, stream: u32) -> Option<Arc<Channel>> {
        self.streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&stream)
    }

    /// Registers a stream and starts both of its pumps.
    fn install(self: &Arc<Self>, stream: u32, local: Stream) -> io::Result<()> {
        let (sender, receiver) = mpsc::channel();
        let outbound_local = local.try_clone()?;
        let inbound_local = local.try_clone()?;
        let channel = Arc::new(Channel::new(local, sender));
        self.streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(stream, channel.clone());
        let link = self.clone();
        let outbound_channel = channel.clone();
        thread::Builder::new()
            .name("zmux-bridge-out".to_owned())
            .spawn(move || {
                pump_outbound(&link, stream, &outbound_channel, outbound_local);
                link.pump_finished(stream, &outbound_channel);
            })?;
        let link = self.clone();
        thread::Builder::new()
            .name("zmux-bridge-in".to_owned())
            .spawn(move || {
                pump_inbound(&link, stream, receiver, inbound_local);
                link.pump_finished(stream, &channel);
            })?;
        Ok(())
    }

    fn pump_finished(&self, stream: u32, channel: &Arc<Channel>) {
        if channel.pumps.fetch_sub(1, Ordering::AcqRel) == 1 {
            let mut streams = self
                .streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if streams
                .get(&stream)
                .is_some_and(|current| Arc::ptr_eq(current, channel))
            {
                streams.remove(&stream);
            }
        }
    }

    /// The link has ended: every stream on it has too.
    fn shut_down(&self) {
        self.closed.store(true, Ordering::Release);
        let streams = std::mem::take(
            &mut *self
                .streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for channel in streams.into_values() {
            channel.reset();
        }
    }

    /// Routes one frame for a stream. Returns the frames the caller should
    /// not handle itself (`Open` and `Info`).
    fn dispatch(self: &Arc<Self>, frame: Frame) -> Option<Frame> {
        match frame.kind {
            Kind::Open | Kind::Info => return Some(frame),
            Kind::Data => {
                if let Some(channel) = self.channel(frame.stream) {
                    channel.deliver(Inbound::Data(frame.payload));
                }
            }
            Kind::Eof => {
                if let Some(channel) = self.channel(frame.stream) {
                    channel.deliver(Inbound::Eof);
                    channel
                        .inbound
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                }
            }
            Kind::Close => {
                if let Some(channel) = self.remove(frame.stream) {
                    channel.reset();
                }
            }
            Kind::Credit => {
                if let (Some(channel), Ok(bytes)) = (
                    self.channel(frame.stream),
                    <[u8; 4]>::try_from(frame.payload.as_slice()),
                ) {
                    channel.grant(u32::from_be_bytes(bytes));
                }
            }
        }
        None
    }

    /// Sends a frame from a thread of its own, for the link reader.
    fn send_detached(self: &Arc<Self>, stream: u32, kind: Kind, payload: Vec<u8>) {
        let link = self.clone();
        let spawned = thread::Builder::new()
            .name("zmux-bridge-reply".to_owned())
            .spawn(move || {
                let _ = link.send(stream, kind, &payload);
            });
        if spawned.is_err() {
            self.closed.store(true, Ordering::Release);
        }
    }
}

/// Local socket → link, within the stream's credit.
fn pump_outbound(link: &Link, stream: u32, channel: &Channel, mut local: Stream) {
    let mut buffer = vec![0; MAX_PAYLOAD];
    loop {
        let count = match local.read(&mut buffer) {
            Ok(0) => {
                let _ = link.send(stream, Kind::Eof, &[]);
                return;
            }
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                let _ = link.send(stream, Kind::Close, &[]);
                channel.reset();
                return;
            }
        };
        let mut sent = 0;
        while sent < count {
            let Some(granted) = channel.take_credit(count - sent) else {
                return;
            };
            if link
                .send(stream, Kind::Data, &buffer[sent..sent + granted])
                .is_err()
            {
                channel.reset();
                return;
            }
            sent += granted;
        }
    }
}

/// Link → local socket, granting credit back as bytes are written.
fn pump_inbound(link: &Link, stream: u32, inbound: mpsc::Receiver<Inbound>, mut local: Stream) {
    while let Ok(message) = inbound.recv() {
        match message {
            Inbound::Data(bytes) => {
                if local.write_all(&bytes).is_err() {
                    let _ = link.send(stream, Kind::Close, &[]);
                    if let Some(channel) = link.remove(stream) {
                        channel.reset();
                    }
                    return;
                }
                let granted = u32::try_from(bytes.len()).expect("frames are bounded");
                let _ = link.send(stream, Kind::Credit, &granted.to_be_bytes());
            }
            Inbound::Eof => {
                let _ = local.shutdown(Shutdown::Write);
                return;
            }
        }
    }
    // The link ended or the stream was reset without an orderly end.
    let _ = local.shutdown(Shutdown::Both);
}

/// The far side: serves streams until the link's input ends.
///
/// `connect` opens one daemon connection per stream; `info` answers stream 0.
/// Both are called afresh each time, so a daemon replaced while the link is up
/// is found again rather than served from a stale endpoint.
pub(crate) fn serve(
    mut reader: impl Read,
    writer: impl Write + Send + 'static,
    connect: impl Fn() -> io::Result<Stream>,
    info: impl Fn() -> BridgeInfo,
) -> Result<()> {
    let link = Link::new(Box::new(writer));
    {
        let mut writer = link
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // The first info reply goes out unasked: every near side wants it
        // straight away, and asking would cost a round trip.
        let mut greeting = HELLO.to_vec();
        greeting.extend(encode_frame(
            LINK_STREAM,
            Kind::Info,
            &serde_json::to_vec(&info()).unwrap_or_default(),
        ));
        writer
            .write_all(&greeting)
            .context("writing the bridge greeting")?;
        writer.flush().context("flushing the bridge greeting")?;
    }
    let result = loop {
        let frame = match read_frame(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => break Ok(()),
            Err(error) => break Err(error).context("reading the bridge link"),
        };
        let Some(frame) = link.dispatch(frame) else {
            continue;
        };
        match frame.kind {
            Kind::Open if frame.stream != LINK_STREAM => {
                if link.channel(frame.stream).is_some() {
                    continue;
                }
                let opened = connect().and_then(|daemon| link.install(frame.stream, daemon));
                if let Err(error) = opened {
                    log::debug!(
                        "bridge stream {} could not reach the daemon: {error}",
                        frame.stream
                    );
                    link.send_detached(frame.stream, Kind::Close, Vec::new());
                }
            }
            Kind::Info if frame.stream == LINK_STREAM => {
                let payload = serde_json::to_vec(&info()).unwrap_or_default();
                link.send_detached(LINK_STREAM, Kind::Info, payload);
            }
            _ => {}
        }
    };
    link.shut_down();
    result
}

/// The near side of a link: opens streams over it.
pub(crate) struct MuxBridge {
    link: Arc<Link>,
    next_stream: Mutex<u32>,
    /// Info replies, in the order they were asked for. One question at a time,
    /// under this lock, so a reply always belongs to its question.
    info: Mutex<mpsc::Receiver<Vec<u8>>>,
    /// The first info reply, which the far side sends with its greeting.
    initial: BridgeInfo,
    pairs: LocalPairs,
}

impl MuxBridge {
    /// Starts the near side over a link whose far side has just been started.
    ///
    /// Waits up to `timeout` for the greeting. What arrives instead — nothing,
    /// or the start of some other output — means the far side is not a bridge.
    pub(crate) fn connect(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        timeout: Duration,
    ) -> Result<Self> {
        let pairs = LocalPairs::new().context("creating the bridge's local sockets")?;
        let link = Link::new(Box::new(writer));
        let (greeted_sender, greeted) = mpsc::channel();
        let (info_sender, info) = mpsc::channel();
        let reader_link = link.clone();
        thread::Builder::new()
            .name("zmux-bridge-link".to_owned())
            .spawn(move || run_near_reader(reader, &reader_link, &greeted_sender, &info_sender))
            .context("starting the bridge reader")?;
        match greeted.recv_timeout(timeout) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                link.shut_down();
                return Err(error);
            }
            Err(_) => {
                link.shut_down();
                anyhow::bail!("the remote zmux bridge did not answer within {timeout:?}");
            }
        }
        let initial = info
            .recv_timeout(timeout)
            .context("the remote zmux bridge did not report its endpoint")
            .and_then(|reply| {
                serde_json::from_slice(&reply).context("reading the remote zmux bridge's endpoint")
            });
        let initial = match initial {
            Ok(initial) => initial,
            Err(error) => {
                link.shut_down();
                return Err(error);
            }
        };
        Ok(Self {
            link,
            next_stream: Mutex::new(LINK_STREAM + 1),
            info: Mutex::new(info),
            initial,
            pairs,
        })
    }

    pub(crate) fn is_alive(&self) -> bool {
        !self.link.closed.load(Ordering::Acquire)
    }

    /// What the far side reported when the link came up.
    pub(crate) fn initial_info(&self) -> &BridgeInfo {
        &self.initial
    }

    /// Asks the far side again. One round trip, no new login.
    pub(crate) fn query_info(&self, timeout: Duration) -> Result<BridgeInfo> {
        let replies = self
            .info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.link
            .send(LINK_STREAM, Kind::Info, &[])
            .context("asking the remote zmux bridge for its endpoint")?;
        let reply = replies
            .recv_timeout(timeout)
            .context("the remote zmux bridge did not report its endpoint")?;
        serde_json::from_slice(&reply).context("reading the remote zmux bridge's endpoint")
    }

    /// Opens one daemon connection over the link.
    pub(crate) fn open(&self) -> Result<Stream> {
        anyhow::ensure!(self.is_alive(), "the remote zmux bridge has closed");
        let stream = {
            let mut next = self
                .next_stream
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let stream = *next;
            *next = next
                .checked_add(1)
                .context("the bridge ran out of stream ids")?;
            stream
        };
        let (client, relay) = self
            .pairs
            .pair()
            .context("creating a local bridge socket")?;
        // Registered before the far side hears of it, so a reply can never
        // arrive for a stream this side does not know yet.
        self.link
            .install(stream, relay)
            .context("starting the bridge stream")?;
        if let Err(error) = self.link.send(stream, Kind::Open, &[]) {
            if let Some(channel) = self.link.remove(stream) {
                channel.reset();
            }
            return Err(error).context("opening a stream over the remote zmux bridge");
        }
        Ok(client)
    }
}

impl Drop for MuxBridge {
    fn drop(&mut self) {
        self.link.shut_down();
    }
}

fn run_near_reader(
    mut reader: impl Read,
    link: &Arc<Link>,
    greeted: &mpsc::Sender<Result<()>>,
    info: &mpsc::Sender<Vec<u8>>,
) {
    let mut hello = [0; HELLO.len()];
    let greeting = reader
        .read_exact(&mut hello)
        .context("the remote host closed the zmux bridge before greeting it")
        .and_then(|()| {
            anyhow::ensure!(
                &hello == HELLO,
                "the remote host did not start a zmux bridge; install a zmux with `proxy-mux` \
                 there"
            );
            Ok(())
        });
    let failed = greeting.is_err();
    let _ = greeted.send(greeting);
    if !failed {
        loop {
            match read_frame(&mut reader) {
                Ok(Some(frame)) => {
                    if let Some(frame) = link.dispatch(frame)
                        && frame.kind == Kind::Info
                        && frame.stream == LINK_STREAM
                    {
                        let _ = info.send(frame.payload);
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    log::debug!("the zmux bridge link failed: {error}");
                    break;
                }
            }
        }
    }
    link.shut_down();
}

/// Where the near side gets the connected pairs of local sockets its streams
/// are: one end for the caller, one relayed over the link.
#[cfg(unix)]
struct LocalPairs;

#[cfg(unix)]
impl LocalPairs {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    fn pair(&self) -> io::Result<(Stream, Stream)> {
        Stream::pair()
    }
}

/// Windows has no socket pair, so a private listener accepts each pair's
/// second end. One per bridge, for the bridge's lifetime: its directory holds
/// the socket file, and nothing is left behind per stream.
#[cfg(windows)]
struct LocalPairs {
    listener: Mutex<crate::transport::Listener>,
    socket: PathBuf,
    _directory: tempfile::TempDir,
}

#[cfg(windows)]
impl LocalPairs {
    fn new() -> io::Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("zetta-zmux-bridge-")
            .tempdir()?;
        let socket = directory.path().join("pair.sock");
        let listener = crate::transport::Listener::bind(&socket)?;
        Ok(Self {
            listener: Mutex::new(listener),
            socket,
            _directory: directory,
        })
    }

    /// Connects and accepts under one lock, so two streams opened at once
    /// cannot each accept the other's connection.
    fn pair(&self) -> io::Result<(Stream, Stream)> {
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let client = Stream::connect(&self.socket)?;
        let (relay, _) = listener.accept()?;
        Ok((client, relay))
    }
}

// The tests build both halves over Unix socket pairs, which Windows lacks.
#[cfg(all(test, unix))]
#[path = "tests/mux_bridge.rs"]
mod tests;
