//! The Unix half of the transport: asking the kernel who is on the far end of
//! a socket, and passing a terminal's descriptors with `SCM_RIGHTS`.

use super::*;
#[cfg(not(target_os = "linux"))]
use std::os::fd::AsRawFd as _;

/// The user ID on the far end of a connected socket.
///
/// Checked before a descriptor is handed over: the socket's permissions
/// already restrict who can connect, but a descriptor to a session's terminal
/// is the one thing that bypasses every other check, so it is worth confirming
/// rather than inferring.
pub fn peer_uid(stream: &Stream) -> Result<u32> {
    #[cfg(target_os = "linux")]
    {
        let credentials = rustix::net::sockopt::socket_peercred(stream)
            .map_err(std::io::Error::from)
            .context("reading peer credentials")?;
        Ok(credentials.uid.as_raw())
    }
    // rustix wraps no `getpeereid`, so the other platforms stay on `libc`.
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
pub fn peer_is_this_user(stream: &Stream) -> Result<bool> {
    Ok(peer_uid(stream)? == rustix::process::geteuid().as_raw())
}

/// The process on the far end of an accepted connection, once it has been
/// confirmed to run as this user. The Windows counterpart also holds the
/// process open; here the id is only what the kernel reported.
pub struct PeerProcess {
    id: Option<u32>,
}

impl PeerProcess {
    /// Refuses a peer running as another user.
    pub fn of(stream: &Stream) -> Result<Self> {
        anyhow::ensure!(
            peer_is_this_user(stream)?,
            "refusing a connection from another user"
        );
        Ok(Self {
            id: peer_process_id(stream)?,
        })
    }

    /// The peer's process id, where the kernel reported one.
    pub fn id(&self) -> Option<u32> {
        self.id
    }
}

/// FreeBSD's spelling of the option that answers with a `struct xucred`.
/// `libc` defines the neighbouring `LOCAL_CREDS` values but not this one.
#[cfg(target_os = "freebsd")]
pub(super) const LOCAL_PEERCRED: libc::c_int = 1;

/// The kernel-reported process on the other end of a local socket.
///
/// The envelope still carries a process id because Windows handle duplication
/// needs a target and because test/remote transports may not expose one.
/// Administrative authorization must not trust that field: a same-user client
/// can read the endpoint token and otherwise claim to be the owner of a
/// protected session. `None` therefore means "nothing vouched for this peer",
/// and the daemon treats it as an identity it will not act on rather than
/// falling back to what the peer said about itself.
#[cfg_attr(
    not(any(target_os = "linux", target_vendor = "apple", target_os = "freebsd")),
    expect(
        unused_variables,
        reason = "no other platform reports a peer's process"
    )
)]
pub fn peer_process_id(stream: &Stream) -> Result<Option<u32>> {
    #[cfg(target_os = "linux")]
    {
        let credentials = rustix::net::sockopt::socket_peercred(stream)
            .map_err(std::io::Error::from)
            .context("reading peer process credentials")?;
        Ok(Some(credentials.pid.as_raw_nonzero().get().cast_unsigned()))
    }
    // `LOCAL_PEERPID` and FreeBSD's `xucred` are not in rustix, so these stay
    // on `libc`.
    // Darwin answers the same question under its own name, on a socket-local
    // option level. `LOCAL_PEERPID` is the process that created the socket,
    // which is the one whose authority is being asked about; `LOCAL_PEEREPID`
    // is the *effective* pid a process can have set on its own behalf, and is
    // therefore not usable as proof.
    #[cfg(target_vendor = "apple")]
    {
        let mut process_id: libc::pid_t = 0;
        let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: the socket is connected, and the destination and its length
        // describe the same `pid_t`.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&raw mut process_id).cast(),
                &mut length,
            )
        };
        anyhow::ensure!(
            result == 0,
            "reading peer process credentials: {}",
            last_error()
        );
        Ok(u32::try_from(process_id).ok().filter(|pid| *pid > 0))
    }
    #[cfg(target_os = "freebsd")]
    {
        let mut credentials = std::mem::MaybeUninit::<libc::xucred>::zeroed();
        let mut length = std::mem::size_of::<libc::xucred>() as libc::socklen_t;
        // SAFETY: the socket is connected, and the destination and its length
        // describe the same `xucred`.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_LOCAL,
                LOCAL_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut length,
            )
        };
        anyhow::ensure!(
            result == 0,
            "reading peer process credentials: {}",
            last_error()
        );
        // SAFETY: getsockopt filled the structure after returning zero, and the
        // union was zeroed beforehand so reading the pid arm is defined even on
        // a kernel too old to fill it — where it reads as zero and is rejected
        // below rather than being trusted.
        let process_id = unsafe { credentials.assume_init().cr_pid__c_anonymous_union.cr_pid };
        Ok(u32::try_from(process_id).ok().filter(|pid| *pid > 0))
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple", target_os = "freebsd")))]
    {
        Ok(None)
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// Control-message space for [`MAX_DESCRIPTORS`], aligned by rustix.
const CONTROL_SPACE: usize = rustix::cmsg_space!(ScmRights(MAX_DESCRIPTORS));

/// Sends `payload` together with `descriptors` over a connected socket.
///
/// At least one payload byte is required: a `SCM_RIGHTS` control message is
/// attached to data, and a zero-length send would drop the descriptors.
pub fn send_with_descriptors(
    stream: &Stream,
    payload: &[u8],
    descriptors: &[BorrowedFd<'_>],
) -> Result<()> {
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};

    anyhow::ensure!(
        !payload.is_empty(),
        "descriptors must accompany at least one byte"
    );
    anyhow::ensure!(
        descriptors.len() <= MAX_DESCRIPTORS,
        "at most {MAX_DESCRIPTORS} descriptors may be sent at once"
    );

    let mut space = [std::mem::MaybeUninit::uninit(); CONTROL_SPACE];
    let mut control = SendAncillaryBuffer::new(&mut space);
    if !descriptors.is_empty() {
        anyhow::ensure!(
            control.push(SendAncillaryMessage::ScmRights(descriptors)),
            "the descriptors did not fit their control message"
        );
    }

    let sent = loop {
        match sendmsg(
            stream,
            &[std::io::IoSlice::new(payload)],
            &mut control,
            SendFlags::empty(),
        ) {
            Ok(sent) => break sent,
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => {
                return Err(std::io::Error::from(error))
                    .context("sending descriptors to the multiplexer peer");
            }
        }
    };
    anyhow::ensure!(
        sent == payload.len(),
        "the multiplexer handover was truncated"
    );
    Ok(())
}

/// Receives a payload and any descriptors sent with it.
///
/// Where the platform has `MSG_CMSG_CLOEXEC`, `recvmsg` sets close-on-exec
/// atomically. Apple platforms do not, so there the descriptors are marked
/// before this function returns them to the caller.
pub fn receive_with_descriptors(
    stream: &Stream,
    buffer: &mut [u8],
) -> Result<(usize, Vec<OwnedFd>)> {
    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, recvmsg};

    #[cfg(not(target_vendor = "apple"))]
    let flags = RecvFlags::CMSG_CLOEXEC;
    #[cfg(target_vendor = "apple")]
    let flags = RecvFlags::empty();

    let mut space = [std::mem::MaybeUninit::uninit(); CONTROL_SPACE];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let received = loop {
        match recvmsg(
            stream,
            &mut [std::io::IoSliceMut::new(buffer)],
            &mut control,
            flags,
        ) {
            Ok(received) => break received,
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => {
                return Err(std::io::Error::from(error))
                    .context("receiving descriptors from the multiplexer");
            }
        }
    };

    let mut descriptors = Vec::new();
    for message in control.drain() {
        if let RecvAncillaryMessage::ScmRights(received) = message {
            descriptors.extend(received);
        }
    }

    // A peer that overruns the control buffer would otherwise leave descriptors
    // closed by the kernel and a caller believing it received them.
    anyhow::ensure!(
        !received.flags.contains(ReturnFlags::CTRUNC),
        "the multiplexer handover was truncated"
    );

    #[cfg(target_vendor = "apple")]
    for descriptor in &descriptors {
        // `MSG_CMSG_CLOEXEC` is unavailable here, so preserve the same no-leak
        // guarantee before exposing the descriptor.
        rustix::io::fcntl_setfd(descriptor, rustix::io::FdFlags::CLOEXEC)
            .map_err(std::io::Error::from)
            .context("setting received descriptor close-on-exec")?;
    }

    Ok((received.bytes, descriptors))
}
