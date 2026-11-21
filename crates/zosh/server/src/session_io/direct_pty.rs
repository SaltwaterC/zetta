//! The Unix PTY path: the master descriptor read and written directly by the
//! session loop, non-blocking, rather than through reader and writer threads.

use super::*;

/// A PTY master the loop reads and writes itself.
///
/// The descriptor is borrowed from the `MasterPty` it came from, which the
/// session keeps for resizing; this never closes it.
pub struct DirectPty {
    pub(super) fd: std::os::fd::RawFd,
    pub(super) writes: VecDeque<PtyWrite>,
    /// How much of the front write has gone out.
    pub(super) written: usize,
    /// Events produced by writing, reported ahead of anything read.
    pub(super) pending: VecDeque<PtyEvent>,
    /// Set once the PTY has ended; it is then neither read nor watched.
    pub(super) closed: bool,
    pub(super) buffer: Box<[u8; PTY_CHUNK]>,
}

impl DirectPty {
    pub(super) fn new(fd: std::os::fd::RawFd) -> Result<Self> {
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

    pub(super) fn next_event(&mut self) -> Option<PtyEvent> {
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

    pub(super) fn queue(&mut self, request: PtyWrite) -> Result<()> {
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

    pub(super) fn flush(&mut self) {
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
    pub(super) fn interest(&self, read: bool) -> Option<libc::pollfd> {
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

/// Sets `O_NONBLOCK` and `FD_CLOEXEC` on a descriptor the caller owns.
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
