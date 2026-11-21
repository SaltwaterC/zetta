//! The Unix half of the transport: asking the kernel who is on the far end of
//! a socket, and passing a terminal's descriptors with `SCM_RIGHTS`.

use super::*;

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
pub fn peer_is_this_user(stream: &Stream) -> Result<bool> {
    // SAFETY: geteuid only reads the calling process's effective user ID.
    Ok(peer_uid(stream)? == unsafe { libc::geteuid() })
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
        let _ = stream;
        Ok(None)
    }
}

pub(super) fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// Sends `payload` together with `descriptors` over a connected socket.
///
/// At least one payload byte is required: a `SCM_RIGHTS` control message is
/// attached to data, and a zero-length send would drop the descriptors.
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
pub(super) const RECEIVE_MSG_FLAGS: libc::c_int = libc::MSG_CMSG_CLOEXEC;

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) const RECEIVE_MSG_FLAGS: libc::c_int = 0;

/// Receives a payload and any descriptors sent with it.
///
/// Linux asks `recvmsg` to set close-on-exec atomically. Other Unix platforms
/// do not expose that flag through `libc`, so the descriptors are marked before
/// this function returns them to the caller.
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
