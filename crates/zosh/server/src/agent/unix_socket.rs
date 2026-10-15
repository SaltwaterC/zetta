//! Who may reach the forwarded-agent Unix socket.
//!
//! A forwarded agent signs with the user's keys, so its socket has to be the
//! user's alone. Two things make it so, and neither is left to `chmod` after
//! the fact:
//!
//! - The directory holding the socket is created `0700`, so there is no moment
//!   in which the umask has left it open and the socket inside it reachable.
//! - Every accepted connection's peer is checked against the user the listener
//!   was opened for, so a socket that ended up reachable anyway — a platform
//!   that ignores socket permissions, or a directory someone else could enter —
//!   still answers only that user. `root` is let through, as `ssh-agent` does:
//!   it can read the agent's memory regardless, and refusing it would break
//!   `sudo` commands that use the forwarded agent.
//!
//! The zosh client's bootstrap relay has its own copy of this: the two are
//! separate crates.

use std::{
    io,
    os::{
        fd::AsRawFd as _,
        unix::{fs::DirBuilderExt as _, net::UnixStream},
    },
    path::Path,
};

/// Creates `path` as a directory only its owner can enter.
///
/// Fails if it already exists, so a directory someone else made at a
/// predictable name is never adopted.
pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

/// The user this process acts as, which is who a listener it opens serves.
pub(super) fn this_user() -> libc::uid_t {
    // SAFETY: geteuid only reads the calling process's effective user ID.
    unsafe { libc::geteuid() }
}

/// Whether the peer of `stream` may use an agent socket opened for `owner`.
/// A peer whose credentials cannot be read is refused.
pub(super) fn peer_may_connect(stream: &UnixStream, owner: libc::uid_t) -> bool {
    peer_uid(stream).is_ok_and(|uid| uid_may_connect(uid, owner))
}

pub(super) fn uid_may_connect(uid: libc::uid_t, owner: libc::uid_t) -> bool {
    uid == owner || uid == 0
}

fn peer_uid(stream: &UnixStream) -> io::Result<libc::uid_t> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
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
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: getsockopt filled the structure after returning zero.
        Ok(unsafe { credentials.assume_init() }.uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let mut uid = 0;
        let mut gid = 0;
        // SAFETY: the socket is connected and both destinations are writable.
        let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
}
