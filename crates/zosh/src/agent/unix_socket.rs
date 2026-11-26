//! Who may reach an agent's Unix socket: the zosh client's bootstrap relay,
//! and the forwarded-agent socket zosh-server opens.
//!
//! Either one ends at the user's own agent, which signs with the user's keys,
//! so its socket has to be the user's alone. Two things make it so, and
//! neither is left to `chmod` after the fact:
//!
//! - The directory holding the socket is created `0700`, so there is no moment
//!   in which the umask has left it open and the socket inside it reachable.
//! - Every accepted connection's peer is checked against the user the socket
//!   was opened for, so a socket that ended up reachable anyway — a platform
//!   that ignores socket permissions, or a directory someone else could enter —
//!   still answers only that user. `root` is let through, as `ssh-agent` does:
//!   it can read the agent's memory regardless, and refusing it would break
//!   `sudo` commands that use a forwarded agent.
//!
//! zosh-server is a separate crate and includes this file by path, as it does
//! `windows_pipe.rs`.

#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::os::fd::AsRawFd as _;
use std::{
    io,
    os::unix::{fs::DirBuilderExt as _, net::UnixStream},
    path::Path,
};

/// Creates `path` as a directory only its owner can enter.
///
/// Fails if it already exists, so a directory someone else made at a
/// predictable name is never adopted.
pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

/// The user this process acts as, which is who a socket it opens serves.
pub(super) fn this_user() -> libc::uid_t {
    rustix::process::geteuid().as_raw()
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
        Ok(rustix::net::sockopt::socket_peercred(stream)?.uid.as_raw())
    }
    // rustix wraps no `getpeereid`, so the other platforms stay on `libc`.
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
