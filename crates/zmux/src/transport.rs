//! The local transport between a client and the multiplexer.
//!
//! Two things travel over it: length-prefixed JSON control messages, and —
//! on attach — the PTY master file descriptor itself. Passing the descriptor is
//! what keeps an attached pane exactly as fast as one this process spawned:
//! the client reads and writes the real PTY, and the daemon does not sit in the
//! middle copying bytes.
//!
//! Nothing here authenticates a *session*. The endpoint token authenticates the
//! channel, and the peer's user ID is checked because a descriptor must never
//! cross a user boundary; reattaching a protected session still requires its
//! secret, which is checked in [`crate::auth`].

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};

#[cfg(unix)]
use std::os::fd::{BorrowedFd, OwnedFd};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
mod windows_handles;
#[cfg(windows)]
pub use windows_handles::*;

/// The socket both sides speak over.
///
/// Windows has had `AF_UNIX` since Windows 10, so the addressing and the
/// framing are the same on both platforms. Only the way a terminal's handles
/// travel differs, because `SCM_RIGHTS` has no Windows equivalent.
#[cfg(unix)]
pub type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
pub type Stream = uds_windows::UnixStream;
#[cfg(unix)]
pub type Listener = std::os::unix::net::UnixListener;
#[cfg(windows)]
pub type Listener = uds_windows::UnixListener;

use anyhow::{Context as _, Result};
use serde::{Serialize, de::DeserializeOwned};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use crate::private_fs::{
    MAX_ENDPOINT_BYTES, create_private_dir, read_private_file, validate_private_dir,
    write_private_file,
};

/// A control message longer than this is refused rather than buffered, so a
/// peer cannot make the daemon allocate without bound.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

pub const ENDPOINT_VERSION: u32 = 1;

/// How a client finds the daemon: the socket to connect to, and the token that
/// authenticates the channel. Written `0600` inside the `0700` session
/// directory, so only this user can read the token.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub version: u32,
    /// The wire protocol this multiplexer speaks.
    ///
    /// Published here so a client can tell *before* connecting that a daemon
    /// left over from an older build cannot serve it, and fall back rather
    /// than failing to open a terminal at all.
    ///
    /// Deliberately not defaulted: an endpoint written before this field
    /// existed would otherwise read as protocol zero and, now that zero is a
    /// real version, appear compatible with a build it cannot talk to. An
    /// endpoint without it is unreadable, which is the truth — it belongs to a
    /// multiplexer this build knows nothing about.
    pub protocol_version: u32,
    pub process_id: u32,
    pub socket_path: PathBuf,
    pub token: String,
}

impl Endpoint {
    pub fn read(path: &Path) -> Result<Self> {
        let endpoint = Self::read_unchecked(path)?;
        anyhow::ensure!(
            endpoint.version == ENDPOINT_VERSION,
            "multiplexer endpoint {} has unsupported version {}",
            path.display(),
            endpoint.version
        );
        Ok(endpoint)
    }

    /// Reads the Windows pseudoconsole host endpoint with the current schema.
    #[cfg(windows)]
    pub fn read_host(path: &Path) -> Result<Self> {
        let endpoint = Self::read_unchecked(path)?;
        anyhow::ensure!(
            endpoint.version == ENDPOINT_VERSION,
            "pseudoconsole host endpoint {} has unsupported version {}",
            path.display(),
            endpoint.version
        );
        Ok(endpoint)
    }

    /// Reads an endpoint only out of a private directory, only as a regular
    /// file this user owns, and only if the socket it names sits beside it:
    /// whoever could write an endpoint anywhere else could point a client, and
    /// the secrets it sends, at a socket of their own.
    fn read_unchecked(path: &Path) -> Result<Self> {
        let directory = path.parent().context("endpoint path has no parent")?;
        validate_private_dir(directory).with_context(|| {
            format!(
                "checking the multiplexer endpoint directory {}",
                directory.display()
            )
        })?;
        let contents = read_private_file(path, MAX_ENDPOINT_BYTES)
            .with_context(|| format!("reading multiplexer endpoint {}", path.display()))?;
        let endpoint: Self = serde_json::from_slice(&contents)
            .with_context(|| format!("parsing multiplexer endpoint {}", path.display()))?;
        anyhow::ensure!(
            endpoint.socket_path.parent() == Some(directory),
            "multiplexer endpoint {} names a socket outside its directory",
            path.display()
        );
        Ok(endpoint)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("endpoint path has no parent")?;
        create_private_dir(parent)?;
        let contents = serde_json::to_vec_pretty(self).context("serializing endpoint")?;
        write_private_file(path, &contents)
            .with_context(|| format!("writing multiplexer endpoint {}", path.display()))
    }
}

/// Compares an offered token with the expected one in constant time, so a
/// wrong token cannot be refined a byte at a time by measuring the reply.
pub fn token_matches(supplied: &str, expected: &str) -> bool {
    supplied.as_bytes().ct_eq(expected.as_bytes()).into()
}

pub fn random_hex(byte_count: usize) -> Result<String> {
    let mut bytes = vec![0; byte_count];
    getrandom::fill(&mut bytes)?;
    Ok(encode_hex(&bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0xf) as usize] as char);
    }
    encoded
}

pub fn write_message(stream: &mut impl Write, message: &impl Serialize) -> Result<()> {
    let frame = encode_secret_message(message)?;
    stream.write_all(&frame)?;
    stream.flush()?;
    Ok(())
}

/// [`encode_message`]'s frame, built in memory that is erased when dropped and
/// is never reallocated on the way (see [`crate::auth::encode_secret_json`]).
///
/// What [`write_message`] sends, because a request can carry a session secret.
/// Queued frames go through [`encode_message`]: they carry pane output, which
/// is what makes a second serialization pass worth avoiding there.
pub(crate) fn encode_secret_message(message: &impl Serialize) -> Result<Zeroizing<Vec<u8>>> {
    let mut frame = crate::auth::encode_secret_json(message, 4)?;
    let length = frame.len() - 4;
    anyhow::ensure!(
        length <= MAX_MESSAGE_BYTES,
        "multiplexer message is too long"
    );
    let length = u32::try_from(length).context("multiplexer message length overflow")?;
    frame[..4].copy_from_slice(&length.to_be_bytes());
    Ok(frame)
}

/// Serializes a message exactly as [`write_message`] puts it on the wire.
///
/// For a sender that queues frames rather than writing them where it stands: a
/// frame has to be complete before it is queued, because a message split across
/// two queue entries would be read as the tail of whatever followed it.
pub fn encode_message(message: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(message)?;
    anyhow::ensure!(
        bytes.len() <= MAX_MESSAGE_BYTES,
        "multiplexer message is too long"
    );
    let length = u32::try_from(bytes.len()).context("multiplexer message length overflow")?;
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&bytes);
    Ok(frame)
}

pub fn read_message<T: DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    // Zeroized because a request may carry a session secret.
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    anyhow::ensure!(
        length <= MAX_MESSAGE_BYTES,
        "multiplexer message is too long"
    );
    let mut bytes = Zeroizing::new(vec![0; length]);
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).context("parsing multiplexer message")
}

fn take_frame(buffer: &mut Vec<u8>) -> Result<Option<Zeroizing<Vec<u8>>>> {
    if buffer.len() < 4 {
        return Ok(None);
    }
    let length = u32::from_be_bytes(buffer[..4].try_into().unwrap()) as usize;
    anyhow::ensure!(
        length <= MAX_MESSAGE_BYTES,
        "multiplexer message is too long"
    );
    let frame_length = 4 + length;
    if buffer.len() < frame_length {
        return Ok(None);
    }
    let mut frame = Zeroizing::new(buffer.drain(..frame_length).collect::<Vec<_>>());
    frame.drain(..4);
    Ok(Some(frame))
}

/// A connection carrying both control messages and descriptors.
///
/// Reading has to be buffered — a stream socket splits and coalesces writes
/// freely — but the buffer must belong to the *connection*, not to a single
/// read. A `BufReader` constructed per message would read ahead and then throw
/// away whatever it had already pulled in, which here would silently eat the
/// beginning of the replay that follows an attach response.
/// The error a closed connection produces.
///
/// Carries an `UnexpectedEof` in its chain so a caller can tell "the peer is
/// finished" from "the peer sent something wrong" — the difference between a
/// stream that ended and one that broke. The shared reader turns the first into an
/// ordinary end of stream; treating it as a failure made a relay that was
/// *deliberately* retired print an error into the terminal, which shifted a
/// full-screen program's display by the lines it took.
pub fn connection_closed() -> anyhow::Error {
    anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
        .context("the multiplexer connection closed")
}

pub struct Connection {
    stream: Stream,
    leftover: Vec<u8>,
}

/// What arrives beside a message: descriptors on Unix. Windows carries a
/// terminal's handles inside the message itself.
#[cfg(unix)]
pub type ReceivedHandle = OwnedFd;
#[cfg(windows)]
pub type ReceivedHandle = ();

/// How much of a raw payload is read, and allocated, at a time.
const RAW_READ_CHUNK: usize = 64 * 1024;

/// Whether a read failed only because its timeout ran out, which a read
/// against a deadline answers by checking the deadline again.
fn io_timed_out(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn is_timeout(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| io_timed_out(error) || error.kind() == std::io::ErrorKind::Interrupted)
}

impl Connection {
    pub fn new(stream: Stream) -> Self {
        Self {
            stream,
            leftover: Vec::new(),
        }
    }

    /// Bounds how long a read on this connection waits.
    ///
    /// The daemon deliberately leaves most connections unbounded: a
    /// subscription and a shared relay are supposed to sit idle for as long as
    /// the client lives. It is for the exchanges where a peer that never
    /// answers costs something more than its own thread.
    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> Result<()> {
        self.stream
            .set_read_timeout(timeout)
            .context("setting a multiplexer read timeout")
    }

    /// Bounds how long a write on this connection waits.
    ///
    /// Event subscriptions have their own writer threads, so one stalled
    /// remote socket must eventually retire only that subscription rather than
    /// holding the daemon's session or subscriber registry locks hostage.
    pub fn set_write_timeout(&self, timeout: Option<std::time::Duration>) -> Result<()> {
        self.stream
            .set_write_timeout(timeout)
            .context("setting a multiplexer write timeout")
    }

    pub fn stream(&self) -> &Stream {
        &self.stream
    }

    pub fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            stream: self.stream.try_clone()?,
            // A response and the raw replay (and possibly the first shared
            // events) can arrive in one socket read. The reader clone must
            // inherit bytes already parsed off the socket or they disappear
            // when the original connection is kept for full-duplex input.
            leftover: self.leftover.clone(),
        })
    }

    pub fn send(&mut self, message: &impl Serialize) -> Result<()> {
        write_message(&mut self.stream, message)
    }

    /// Sends a message with descriptors attached to it, so the receiver cannot
    /// observe one without the other.
    #[cfg(unix)]
    pub fn send_with(
        &mut self,
        message: &impl Serialize,
        descriptors: &[BorrowedFd<'_>],
    ) -> Result<()> {
        let payload = encode_message(message)?;
        send_with_descriptors(&self.stream, &payload, descriptors)
    }

    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<(T, Vec<ReceivedHandle>)> {
        self.receive_until(None)
    }

    /// As [`Self::receive`], but the whole message has to have arrived by
    /// `deadline`, however it is split.
    ///
    /// A read timeout bounds only the wait for the next piece, so a peer that
    /// sends a byte at a time keeps a connection for as long as the message
    /// limit allows. Leaves the connection with no read timeout, as an accepted
    /// connection starts.
    pub fn receive_by<T: DeserializeOwned>(
        &mut self,
        deadline: Instant,
    ) -> Result<(T, Vec<ReceivedHandle>)> {
        let received = self.receive_until(Some(deadline));
        self.set_read_timeout(None)?;
        received
    }

    fn receive_until<T: DeserializeOwned>(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<(T, Vec<ReceivedHandle>)> {
        let mut handles = Vec::new();
        loop {
            if let Some(frame) = take_frame(&mut self.leftover)? {
                let message =
                    serde_json::from_slice(&frame).context("parsing multiplexer message")?;
                return Ok((message, handles));
            }
            anyhow::ensure!(
                self.leftover.len() <= MAX_MESSAGE_BYTES + 4,
                "multiplexer message is too long"
            );
            self.wait_until(deadline)?;

            let mut buffer = [0; 8192];
            let read = match self.read_some(&mut buffer, &mut handles) {
                Ok(read) => read,
                Err(error) if deadline.is_some() && is_timeout(&error) => continue,
                Err(error) => return Err(error),
            };
            if read == 0 {
                return Err(connection_closed());
            }
            self.leftover.extend_from_slice(&buffer[..read]);
        }
    }

    /// Lets the next read wait only until `deadline`, failing once it has
    /// passed. Does nothing without one.
    fn wait_until(&self, deadline: Option<Instant>) -> Result<()> {
        let Some(deadline) = deadline else {
            return Ok(());
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(
            !remaining.is_zero(),
            "the multiplexer peer did not send in time"
        );
        self.set_read_timeout(Some(remaining))
    }

    #[cfg(unix)]
    fn read_some(&mut self, buffer: &mut [u8], descriptors: &mut Vec<OwnedFd>) -> Result<usize> {
        let (read, mut received) = receive_with_descriptors(&self.stream, buffer)?;
        descriptors.append(&mut received);
        Ok(read)
    }

    #[cfg(windows)]
    fn read_some(&mut self, buffer: &mut [u8], _handles: &mut Vec<()>) -> Result<usize> {
        Ok(self.stream.read(buffer)?)
    }

    /// Reads exactly `length` raw bytes that follow a message, starting with
    /// anything already buffered.
    pub fn read_exact(&mut self, length: usize) -> Result<Vec<u8>> {
        self.read_exact_until(length, None)
    }

    /// As [`Self::read_exact`], but every byte has to have arrived by
    /// `deadline`. Leaves the connection with no read timeout.
    pub fn read_exact_by(&mut self, length: usize, deadline: Instant) -> Result<Vec<u8>> {
        let bytes = self.read_exact_until(length, Some(deadline));
        self.set_read_timeout(None)?;
        bytes
    }

    /// Grows the buffer as the bytes arrive rather than allocating `length`
    /// up front: the length is the peer's claim, and allocating what it
    /// claims would let a peer that sends nothing reserve whatever it likes.
    fn read_exact_until(&mut self, length: usize, deadline: Option<Instant>) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(length.min(RAW_READ_CHUNK));
        let buffered = self.leftover.len().min(length);
        bytes.extend(self.leftover.drain(..buffered));
        while bytes.len() < length {
            self.wait_until(deadline)?;
            let start = bytes.len();
            bytes.resize(start + (length - start).min(RAW_READ_CHUNK), 0);
            match self.stream.read(&mut bytes[start..]) {
                Ok(0) => {
                    return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
                }
                Ok(read) => bytes.truncate(start + read),
                Err(error)
                    if error.kind() == std::io::ErrorKind::Interrupted
                        || (deadline.is_some() && io_timed_out(&error)) =>
                {
                    bytes.truncate(start);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(bytes)
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()?;
        Ok(())
    }
}

/// One attach hands over one master descriptor, so the ceiling is deliberately
/// small: it bounds the control buffer and rejects a peer claiming more.
pub const MAX_DESCRIPTORS: usize = 4;

#[cfg(test)]
#[path = "tests/transport.rs"]
mod tests;

#[cfg(windows)]
impl Connection {
    /// Windows carries a terminal's handles inside the message, already
    /// duplicated into the receiver, so there is nothing to attach.
    pub fn send_with(&mut self, message: &impl Serialize, _handles: &[()]) -> Result<()> {
        self.send(message)
    }
}
