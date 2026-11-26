//! The Unix PTY path: the master descriptor read and written directly by the
//! session loop, non-blocking, rather than through reader and writer threads.

use super::*;
use rustix::io::Errno;

/// A PTY master the loop reads and writes itself.
///
/// The descriptor is a duplicate of the `MasterPty`'s, which the session keeps
/// beside this for resizing and drops with it, so the terminal hangs up when
/// the session ends exactly as it did when this borrowed the master's own.
pub struct DirectPty {
    pub(super) fd: std::os::fd::OwnedFd,
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
    pub(super) fn new(master: std::os::fd::BorrowedFd<'_>) -> Result<Self> {
        use anyhow::Context as _;
        use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

        let fd = master
            .try_clone_to_owned()
            .context("duplicating the PTY master")?;
        // The flag belongs to the open file, so the master is nonblocking too.
        fcntl_getfl(&fd)
            .and_then(|flags| fcntl_setfl(&fd, flags | OFlags::NONBLOCK))
            .map_err(io::Error::from)
            .context("making the PTY nonblocking")?;
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
            match rustix::io::read(&self.fd, &mut self.buffer[..]) {
                Ok(0) => {
                    timing::record("pty_eof", 0, 0);
                    self.closed = true;
                    return Some(PtyEvent::Eof);
                }
                Ok(read) => {
                    timing::record("pty_read", read as u64, 0);
                    return Some(PtyEvent::Output(self.buffer[..read].to_vec()));
                }
                Err(Errno::INTR) => {}
                Err(Errno::WOULDBLOCK) => return None,
                // Linux reports the last slave closing as EIO: the program
                // and everything it started have gone, which is an end of
                // file rather than a failure.
                Err(Errno::IO) => {
                    timing::record("pty_eof", 0, 0);
                    self.closed = true;
                    return Some(PtyEvent::Eof);
                }
                Err(error) => {
                    self.closed = true;
                    return Some(PtyEvent::Error(io::Error::from(error).to_string()));
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
                    match rustix::io::write(&self.fd, rest) {
                        Ok(wrote) => {
                            self.written += wrote;
                            if self.written == data.len() {
                                timing::record("pty_write_end", data.len() as u64, 0);
                                self.writes.pop_front();
                                self.written = 0;
                            }
                        }
                        Err(Errno::INTR) => {}
                        Err(Errno::WOULDBLOCK) => return,
                        Err(error) => {
                            timing::record("pty_write_error", 0, 0);
                            self.closed = true;
                            self.writes.clear();
                            self.pending
                                .push_back(PtyEvent::Error(io::Error::from(error).to_string()));
                        }
                    }
                }
            }
        }
    }

    /// What `poll` should watch the master for, if anything.
    pub(super) fn interest(&self, read: bool) -> Option<rustix::event::PollFlags> {
        use rustix::event::PollFlags;

        if self.closed {
            return None;
        }
        let mut events = PollFlags::empty();
        events.set(PollFlags::IN, read);
        events.set(PollFlags::OUT, !self.writes.is_empty());
        (!events.is_empty()).then_some(events)
    }
}
