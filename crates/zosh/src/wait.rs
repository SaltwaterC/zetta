//! How a session loop waits: on its Mosh sockets, on a wake-up other
//! threads can raise, and on whatever else its front end watches, until the
//! next instant the session asked to be looked at again.
//!
//! Both loops used to wait at most 100 ms whatever the session said, which
//! made an idle pane wake ten times a second, and on Windows they could not
//! wait on the sockets at all: a datagram that arrived during the wait sat
//! there until it ran out, so a remote echo could reach the screen up to
//! 100 ms after it reached the machine. Here everything a loop reacts to is
//! a handle one system call waits on — `poll` on Unix, `WSAPoll` on Windows —
//! and the timeout is the session's own deadline, so nothing has to be
//! polled for.
//!
//! On Windows `WSAPoll` waits only on sockets, which is why the wake-up is a
//! connected pair of loopback UDP sockets there rather than a pipe: input
//! that does not arrive on a socket (console events, piped stdin, a pane's
//! commands) is read by a thread that raises it.

use std::io;

use mosh_rs::session::SocketHandle;

/// A handle the wait can watch alongside the session's sockets. On Unix any
/// descriptor; on Windows only a socket, which is all `WSAPoll` takes.
pub(crate) type WaitHandle = SocketHandle;

/// Which of the extra handles given to [`Waiter::wait`] became ready, by
/// position. A session socket being ready needs no flag: the loop pumps the
/// session on every pass anyway.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ready(u32);

impl Ready {
    #[cfg_attr(
        all(windows, not(test)),
        allow(
            dead_code,
            reason = "on Windows the loops watch only their wake-up, which they drain unconditionally"
        )
    )]
    pub(crate) fn is_ready(self, index: usize) -> bool {
        index < 32 && self.0 & (1 << index) != 0
    }
}

/// The wait set, kept from pass to pass so building it allocates nothing.
#[derive(Default)]
pub(crate) struct Waiter {
    #[cfg(unix)]
    descriptors: Vec<libc::pollfd>,
    #[cfg(windows)]
    descriptors: Vec<windows::Win32::Networking::WinSock::WSAPOLLFD>,
}

impl Waiter {
    /// Waits until one of `watched` or one of `sockets` is readable, or
    /// `timeout_ms` passes. An interrupted wait returns early with nothing
    /// ready, which a loop treats like any other pass.
    pub(crate) fn wait(
        &mut self,
        watched: &[WaitHandle],
        sockets: impl Iterator<Item = SocketHandle>,
        timeout_ms: u64,
    ) -> io::Result<Ready> {
        debug_assert!(watched.len() <= 32, "Ready holds 32 flags");
        let timeout = i32::try_from(timeout_ms).unwrap_or(i32::MAX);
        self.descriptors.clear();
        for handle in watched.iter().copied().chain(sockets) {
            self.descriptors.push(platform::readable(handle));
        }
        if !platform::wait(&mut self.descriptors, timeout)? {
            return Ok(Ready::default());
        }
        let mut ready = 0;
        for (index, descriptor) in self.descriptors.iter().take(watched.len()).enumerate() {
            if platform::is_ready(descriptor) {
                ready |= 1 << index;
            }
        }
        Ok(Ready(ready))
    }
}

/// The earliest of a loop's deadlines, in milliseconds from now. A deadline
/// that is `None` asks for nothing.
pub(crate) fn earliest_ms(deadlines: impl IntoIterator<Item = Option<u64>>) -> u64 {
    deadlines.into_iter().flatten().min().unwrap_or(u64::MAX)
}

/// Counts the passes a session loop makes, so a test can tell a loop that
/// sleeps from one that polls. Outside tests it is nothing at all.
#[derive(Clone, Default)]
pub(crate) struct PassCounter(#[cfg(test)] std::sync::Arc<std::sync::atomic::AtomicU64>);

impl PassCounter {
    pub(crate) fn tick(&self) {
        #[cfg(test)]
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(all(test, unix))]
    pub(crate) fn count(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// How another thread gets a session loop out of its wait.
///
/// Raising it is cheap and never blocks, and any number of raises before
/// the loop next drains it count as one.
pub(crate) struct Wake {
    #[cfg(unix)]
    read: std::os::fd::OwnedFd,
    #[cfg(unix)]
    write: std::os::fd::OwnedFd,
    #[cfg(windows)]
    receiver: std::net::UdpSocket,
    #[cfg(windows)]
    sender: std::net::UdpSocket,
}

impl Wake {
    /// The handle to watch for the wake-up.
    pub(crate) fn handle(&self) -> WaitHandle {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;
            self.read.as_raw_fd()
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawSocket as _;
            self.receiver.as_raw_socket()
        }
    }
}

#[cfg(unix)]
impl Wake {
    pub(crate) fn new() -> io::Result<Self> {
        use std::os::fd::{FromRawFd as _, OwnedFd};

        let mut descriptors = [0 as libc::c_int; 2];
        // SAFETY: `pipe` fills the two-element array it is given.
        if unsafe { libc::pipe(descriptors.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `pipe` returned these descriptors and nothing else owns
        // them, so the pipe is closed exactly once, when this value is
        // dropped.
        let pipe = unsafe {
            Self {
                read: OwnedFd::from_raw_fd(descriptors[0]),
                write: OwnedFd::from_raw_fd(descriptors[1]),
            }
        };
        // A wake-up must never block the caller, and the loop must be able to
        // empty the pipe without blocking either.
        for descriptor in [pipe.handle(), pipe.write_descriptor()] {
            set_descriptor_flags(descriptor)?;
        }
        Ok(pipe)
    }

    fn write_descriptor(&self) -> libc::c_int {
        use std::os::fd::AsRawFd as _;
        self.write.as_raw_fd()
    }

    pub(crate) fn notify(&self) {
        let byte = [1_u8];
        // SAFETY: the descriptor is owned by this value and the buffer is one
        // byte long. A full pipe already means a pending wake-up, so a short
        // or failed write needs no handling.
        unsafe {
            libc::write(self.write_descriptor(), byte.as_ptr().cast(), 1);
        }
    }

    pub(crate) fn drain(&self) {
        let mut bytes = [0_u8; 64];
        loop {
            // SAFETY: the descriptor is owned by this value and the buffer is
            // as long as the count passed with it.
            let read = unsafe { libc::read(self.handle(), bytes.as_mut_ptr().cast(), bytes.len()) };
            if read <= 0 {
                return;
            }
        }
    }
}

#[cfg(unix)]
pub(crate) fn set_descriptor_flags(descriptor: libc::c_int) -> io::Result<()> {
    // SAFETY: `descriptor` is owned by the caller and both calls only read or
    // replace its flags.
    unsafe {
        if libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
        let flags = libc::fcntl(descriptor, libc::F_GETFL);
        if flags < 0 || libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
impl Wake {
    /// Two loopback UDP sockets connected to each other. Connected, the
    /// receiver accepts datagrams from the sender alone, so nothing else on
    /// the machine can wake the loop, and the pair never leaves loopback,
    /// which is what keeps the firewall out of it.
    pub(crate) fn new() -> io::Result<Self> {
        use std::net::{Ipv4Addr, UdpSocket};

        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        sender.connect(receiver.local_addr()?)?;
        receiver.connect(sender.local_addr()?)?;
        receiver.set_nonblocking(true)?;
        sender.set_nonblocking(true)?;
        Ok(Self { receiver, sender })
    }

    pub(crate) fn notify(&self) {
        // A full socket buffer already holds a wake-up.
        let _ = self.sender.send(&[1]);
    }

    pub(crate) fn drain(&self) {
        let mut bytes = [0_u8; 64];
        while self.receiver.recv(&mut bytes).is_ok() {}
    }
}

#[cfg(unix)]
mod platform {
    use std::io;

    use super::WaitHandle;

    pub(super) fn readable(fd: WaitHandle) -> libc::pollfd {
        libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        }
    }

    pub(super) fn is_ready(descriptor: &libc::pollfd) -> bool {
        descriptor.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
    }

    /// `false` when the wait was interrupted by a signal.
    pub(super) fn wait(descriptors: &mut [libc::pollfd], timeout: i32) -> io::Result<bool> {
        // SAFETY: every descriptor is borrowed from a live socket, pipe or
        // stdin, and `descriptors` stays allocated until poll has returned.
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                timeout,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error);
        }
        Ok(true)
    }
}

#[cfg(windows)]
mod platform {
    use std::io;

    use windows::Win32::Networking::WinSock::{
        POLLERR, POLLHUP, POLLRDNORM, SOCKET, WSAGetLastError, WSAPOLLFD, WSAPoll,
    };

    use super::WaitHandle;

    pub(super) fn readable(socket: WaitHandle) -> WSAPOLLFD {
        WSAPOLLFD {
            fd: SOCKET(socket as usize),
            events: POLLRDNORM,
            revents: Default::default(),
        }
    }

    pub(super) fn is_ready(descriptor: &WSAPOLLFD) -> bool {
        (descriptor.revents.0 & (POLLRDNORM.0 | POLLHUP.0 | POLLERR.0)) != 0
    }

    pub(super) fn wait(descriptors: &mut [WSAPOLLFD], timeout: i32) -> io::Result<bool> {
        // SAFETY: every entry names a live socket owned by the session or by
        // a `Wake`, and the slice stays allocated until WSAPoll has returned.
        let result = unsafe {
            WSAPoll(
                descriptors.as_mut_ptr(),
                u32::try_from(descriptors.len()).unwrap_or(u32::MAX),
                timeout,
            )
        };
        if result < 0 {
            // SAFETY: reads the calling thread's Winsock error.
            let code = unsafe { WSAGetLastError() };
            return Err(io::Error::from_raw_os_error(code.0));
        }
        Ok(true)
    }
}

#[cfg(test)]
#[path = "tests/wait.rs"]
mod tests;
