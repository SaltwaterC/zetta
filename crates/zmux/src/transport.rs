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
};

#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

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

use crate::catalog::{create_private_dir, write_private_file};

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
        let contents = std::fs::read(path)
            .with_context(|| format!("reading multiplexer endpoint {}", path.display()))?;
        let endpoint: Self = serde_json::from_slice(&contents)
            .with_context(|| format!("parsing multiplexer endpoint {}", path.display()))?;
        anyhow::ensure!(
            endpoint.version == ENDPOINT_VERSION,
            "multiplexer endpoint {} has unsupported version {}",
            path.display(),
            endpoint.version
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
    let frame = encode_message(message)?;
    stream.write_all(&frame)?;
    stream.flush()?;
    Ok(())
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

#[cfg(unix)]
/// The user ID on the far end of a connected socket.
///
/// Checked before a descriptor is handed over: the socket's permissions
/// already restrict who can connect, but a descriptor to a session's terminal
/// is the one thing that bypasses every other check, so it is worth confirming
/// rather than inferring.
pub fn peer_uid(stream: &Stream) -> Result<u32> {
    #[cfg(target_os = "linux")]
    {
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: the socket is connected, and the destination and its length
        // describe the same `ucred`.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut length,
            )
        };
        anyhow::ensure!(result == 0, "reading peer credentials: {}", last_error());
        // SAFETY: getsockopt filled the structure after returning zero.
        Ok(unsafe { credentials.assume_init() }.uid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut uid = 0;
        let mut gid = 0;
        // SAFETY: the socket is connected and both destinations are writable.
        let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        anyhow::ensure!(result == 0, "reading peer credentials: {}", last_error());
        Ok(uid)
    }
}

/// Whether the peer of a connection is this same user.
///
/// Unix asks the kernel about the connected socket. Windows has no equivalent
/// for `AF_UNIX`, so the check moves to the point where it matters: a terminal
/// handle is only duplicated into a process whose token matches this one's.
#[cfg(unix)]
pub fn peer_is_this_user(stream: &Stream) -> Result<bool> {
    // SAFETY: geteuid only reads the calling process's effective user ID.
    Ok(peer_uid(stream)? == unsafe { libc::geteuid() })
}

/// The kernel-reported process on the other end of a local socket.
///
/// The envelope still carries a process id because Windows handle duplication
/// needs a target and because test/remote transports may not expose one. On
/// Linux, however, administrative authorization must not trust that field: a
/// same-user client can read the endpoint token and otherwise claim to be the
/// owner of a protected session.
#[cfg(unix)]
pub fn peer_process_id(stream: &Stream) -> Result<Option<u32>> {
    #[cfg(target_os = "linux")]
    {
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: the socket is connected, and the destination and its length
        // describe the same `ucred`.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut length,
            )
        };
        anyhow::ensure!(
            result == 0,
            "reading peer process credentials: {}",
            last_error()
        );
        // SAFETY: getsockopt filled the structure after returning zero.
        Ok(Some(unsafe { credentials.assume_init() }.pid as u32))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = stream;
        Ok(None)
    }
}

#[cfg(windows)]
pub fn peer_process_id(_stream: &Stream) -> Result<Option<u32>> {
    Ok(None)
}

#[cfg(windows)]
pub fn peer_is_this_user(_stream: &Stream) -> Result<bool> {
    Ok(true)
}

#[cfg(unix)]
fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// Sends `payload` together with `descriptors` over a connected socket.
///
/// At least one payload byte is required: a `SCM_RIGHTS` control message is
/// attached to data, and a zero-length send would drop the descriptors.
#[cfg(unix)]
pub fn send_with_descriptors(
    stream: &Stream,
    payload: &[u8],
    descriptors: &[BorrowedFd<'_>],
) -> Result<()> {
    anyhow::ensure!(
        !payload.is_empty(),
        "descriptors must accompany at least one byte"
    );
    anyhow::ensure!(
        descriptors.len() <= MAX_DESCRIPTORS,
        "at most {MAX_DESCRIPTORS} descriptors may be sent at once"
    );

    let mut control = [0u8; CONTROL_BUFFER_LEN];
    let payload_length = descriptors.len() * size_of::<RawFd>();
    let mut iov = libc::iovec {
        iov_base: payload.as_ptr() as *mut libc::c_void,
        iov_len: payload.len(),
    };
    // SAFETY: `msghdr` is a plain C structure with no invalid bit patterns.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    if !descriptors.is_empty() {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = unsafe { libc::CMSG_SPACE(payload_length as u32) } as _;
        // SAFETY: the control buffer is large enough for MAX_DESCRIPTORS, and
        // `msg_controllen` was set from the same count.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(payload_length as u32) as _;
            let data = libc::CMSG_DATA(header).cast::<RawFd>();
            for (index, descriptor) in descriptors.iter().enumerate() {
                std::ptr::write_unaligned(data.add(index), descriptor.as_raw_fd());
            }
        }
    }

    loop {
        // SAFETY: the message describes buffers owned by this frame.
        let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &message, 0) };
        if sent >= 0 {
            anyhow::ensure!(
                sent as usize == payload.len(),
                "the multiplexer handover was truncated"
            );
            return Ok(());
        }
        let error = last_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("sending descriptors to the multiplexer peer");
        }
    }
}

#[cfg(target_os = "linux")]
const RECEIVE_MSG_FLAGS: libc::c_int = libc::MSG_CMSG_CLOEXEC;

#[cfg(all(unix, not(target_os = "linux")))]
const RECEIVE_MSG_FLAGS: libc::c_int = 0;

/// Receives a payload and any descriptors sent with it.
///
/// Linux asks `recvmsg` to set close-on-exec atomically. Other Unix platforms
/// do not expose that flag through `libc`, so the descriptors are marked before
/// this function returns them to the caller.
#[cfg(unix)]
pub fn receive_with_descriptors(
    stream: &Stream,
    buffer: &mut [u8],
) -> Result<(usize, Vec<OwnedFd>)> {
    let mut control = [0u8; CONTROL_BUFFER_LEN];
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    // SAFETY: `msghdr` is a plain C structure with no invalid bit patterns.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;

    let received = loop {
        // SAFETY: the message describes buffers owned by this frame.
        let received =
            unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, RECEIVE_MSG_FLAGS) };
        if received >= 0 {
            break received as usize;
        }
        let error = last_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("receiving descriptors from the multiplexer");
        }
    };

    let mut descriptors = Vec::new();
    // SAFETY: recvmsg filled `message`, and the iteration follows the kernel's
    // own alignment macros.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let payload_length = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                for index in 0..payload_length / size_of::<RawFd>() {
                    descriptors.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(
                        data.add(index),
                    )));
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }

    // A peer that overruns the control buffer would otherwise leave descriptors
    // closed by the kernel and a caller believing it received them.
    anyhow::ensure!(
        message.msg_flags & libc::MSG_CTRUNC == 0,
        "the multiplexer handover was truncated"
    );

    #[cfg(all(unix, not(target_os = "linux")))]
    for descriptor in &descriptors {
        // `MSG_CMSG_CLOEXEC` is unavailable on these platforms, so preserve
        // the same no-leak guarantee before exposing the descriptor.
        let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
        anyhow::ensure!(
            flags >= 0,
            "reading received descriptor flags: {}",
            last_error()
        );
        let result = unsafe {
            libc::fcntl(
                descriptor.as_raw_fd(),
                libc::F_SETFD,
                flags | libc::FD_CLOEXEC,
            )
        };
        anyhow::ensure!(
            result >= 0,
            "setting received descriptor close-on-exec: {}",
            last_error()
        );
    }

    Ok((received, descriptors))
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

impl Connection {
    pub fn new(stream: Stream) -> Self {
        Self {
            stream,
            leftover: Vec::new(),
        }
    }

    pub fn stream(&self) -> &Stream {
        &self.stream
    }

    pub fn try_clone(&self) -> Result<Self> {
        Ok(Self::new(self.stream.try_clone()?))
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

    #[cfg(unix)]
    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<(T, Vec<OwnedFd>)> {
        let mut descriptors = Vec::new();
        loop {
            if let Some(frame) = take_frame(&mut self.leftover)? {
                let message =
                    serde_json::from_slice(&frame).context("parsing multiplexer message")?;
                return Ok((message, descriptors));
            }
            anyhow::ensure!(
                self.leftover.len() <= MAX_MESSAGE_BYTES + 4,
                "multiplexer message is too long"
            );

            let mut buffer = [0; 8192];
            let (read, mut received) = receive_with_descriptors(&self.stream, &mut buffer)?;
            descriptors.append(&mut received);
            if read == 0 {
                return Err(connection_closed());
            }
            self.leftover.extend_from_slice(&buffer[..read]);
        }
    }

    /// Reads exactly `length` raw bytes that follow a message, starting with
    /// anything already buffered.
    pub fn read_exact(&mut self, length: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(length);
        let buffered = self.leftover.len().min(length);
        bytes.extend(self.leftover.drain(..buffered));
        if bytes.len() < length {
            let mut rest = vec![0; length - bytes.len()];
            self.stream.read_exact(&mut rest)?;
            bytes.append(&mut rest);
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
#[cfg(unix)]
const CONTROL_BUFFER_LEN: usize = 128;

#[cfg(unix)]
const _: () = assert!(
    CONTROL_BUFFER_LEN >= 16 + MAX_DESCRIPTORS * size_of::<RawFd>(),
    "the control buffer must hold a cmsg header plus MAX_DESCRIPTORS"
);

#[cfg(test)]
#[path = "tests/transport.rs"]
mod tests;

/// Sends a terminal's handles to the client that asked for it.
///
/// Windows has no `SCM_RIGHTS`: a handle is moved by duplicating it into the
/// target process, which means the sender needs a handle *to that process*.
/// The client therefore names itself in its request, and the duplicated values
/// travel in the reply as plain integers.
///
/// The peer's user is checked first. On Unix the kernel answers that question
/// about the connected socket; here the process is opened and its token
/// compared, because the process identifier came from the client and a handle
/// to a session's terminal must not cross a user boundary on the strength of
/// something the other side claimed.
#[cfg(windows)]
pub fn duplicate_to(
    process_id: u32,
    handles: &[std::os::windows::io::BorrowedHandle<'_>],
) -> Result<Vec<i64>> {
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::{
        CloseHandle, DUPLICATE_HANDLE_OPTIONS, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: both access rights are valid for `OpenProcess`, and the returned
    // handle is closed below on every path.
    let target = unsafe {
        OpenProcess(
            PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            process_id,
        )
    }
    .with_context(|| format!("opening process {process_id} to hand over a terminal"))?;

    let duplicated = (|| -> Result<Vec<i64>> {
        anyhow::ensure!(
            process_belongs_to_this_user(target)?,
            "refusing to hand a terminal to process {process_id}, which belongs to another user"
        );
        let mut duplicated = Vec::with_capacity(handles.len());
        for handle in handles {
            let mut target_handle = HANDLE::default();
            // SAFETY: the source handle is owned by this process for the
            // duration, and the destination is a valid writable slot.
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    HANDLE(handle.as_raw_handle()),
                    target,
                    &mut target_handle,
                    0,
                    false,
                    DUPLICATE_HANDLE_OPTIONS(DUPLICATE_SAME_ACCESS.0),
                )
            }
            .context("duplicating a terminal handle into the client")?;
            duplicated.push(target_handle.0 as i64);
        }
        Ok(duplicated)
    })();

    // SAFETY: `target` came from `OpenProcess` and is closed exactly once.
    unsafe {
        let _ = CloseHandle(target);
    }
    duplicated
}

/// Whether `process` runs as the same user as this one.
#[cfg(windows)]
fn process_belongs_to_this_user(process: windows::Win32::Foundation::HANDLE) -> Result<bool> {
    let theirs = user_sid(process)?;
    // SAFETY: a pseudo-handle for the current process, which must not be closed.
    let ours = user_sid(unsafe { windows::Win32::System::Threading::GetCurrentProcess() })?;
    Ok(theirs == ours)
}

/// The user SID of a process's token, as raw bytes for comparison.
#[cfg(windows)]
fn user_sid(process: windows::Win32::Foundation::HANDLE) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows::Win32::System::Threading::OpenProcessToken;

    let mut token = windows::Win32::Foundation::HANDLE::default();
    // SAFETY: `process` is open with at least query access, and `token` is a
    // valid writable slot closed below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
        .context("opening a process token")?;

    let sid = (|| -> Result<Vec<u8>> {
        let mut needed = 0;
        // SAFETY: asking for the required size with a null buffer is the
        // documented first step and is expected to fail.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
        anyhow::ensure!(needed > 0, "a process token reported no user information");
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: the buffer is exactly the size the call just asked for.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .context("reading a process token's user")?;

        // SAFETY: the buffer holds a `TOKEN_USER` whose `Sid` points inside it.
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        // SAFETY: the SID is valid for as long as the buffer is.
        let length = unsafe { windows::Win32::Security::GetLengthSid(user.User.Sid) } as usize;
        anyhow::ensure!(length > 0, "a process token reported an empty user");
        // SAFETY: `length` is the SID's own reported size.
        Ok(unsafe { std::slice::from_raw_parts(user.User.Sid.0 as *const u8, length) }.to_vec())
    })();

    // SAFETY: `token` came from `OpenProcessToken` and is closed exactly once.
    unsafe {
        let _ = CloseHandle(token);
    }
    sid
}

/// Claims handles the multiplexer duplicated into this process.
#[cfg(windows)]
pub fn claim_duplicated(values: &[i64]) -> Vec<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::FromRawHandle as _;
    values
        .iter()
        .map(|value| {
            // SAFETY: the multiplexer duplicated this handle into this process
            // and does not retain the duplicate, so ownership is taken once.
            unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(*value as *mut _) }
        })
        .collect()
}

#[cfg(windows)]
impl Connection {
    /// Windows carries a terminal's handles inside the message, already
    /// duplicated into the receiver, so there is nothing to attach.
    pub fn send_with(&mut self, message: &impl Serialize, _handles: &[()]) -> Result<()> {
        self.send(message)
    }

    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<(T, Vec<()>)> {
        loop {
            if let Some(frame) = take_frame(&mut self.leftover)? {
                let message =
                    serde_json::from_slice(&frame).context("parsing multiplexer message")?;
                return Ok((message, Vec::new()));
            }
            anyhow::ensure!(
                self.leftover.len() <= MAX_MESSAGE_BYTES + 4,
                "multiplexer message is too long"
            );

            let mut buffer = [0; 8192];
            let read = self.stream.read(&mut buffer)?;
            if read == 0 {
                return Err(connection_closed());
            }
            self.leftover.extend_from_slice(&buffer[..read]);
        }
    }
}
