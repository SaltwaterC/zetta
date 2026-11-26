//! How the session loop sleeps, and how what feeds it from other threads
//! wakes it.
//!
//! On Unix the loop waits in one `poll` on its socket and PTY (see
//! `session_io`), so what wakes it from another thread is a pipe in that same
//! set; on Windows it parks, and is unparked. [`Waker`] is either, and the
//! threads that remain — agent connections, the child-exit watcher, and on
//! Windows the UDP and PTY readers — publish through a [`WakingSender`] that
//! wakes the loop after each send. Everything else the loop does is driven by
//! a clock, and [`WakeDeadline`] collects those clocks into the one instant it
//! waits until.
//!
//! A wait can also end spuriously, which is harmless because every pass of
//! the loop is safe to repeat.

#[cfg(unix)]
use std::sync::Arc;
use std::sync::mpsc::{SendError, SyncSender, TrySendError};
use std::thread::{self, Thread};
use std::time::Instant;

/// What wakes the session loop: a thread to unpark, or on Unix a pipe the
/// loop's `poll` watches.
#[derive(Clone)]
pub struct Waker(WakeTarget);

#[derive(Clone)]
enum WakeTarget {
    #[cfg_attr(
        all(unix, not(test)),
        expect(
            dead_code,
            reason = "on Unix only tests wake a thread rather than the loop's pipe"
        )
    )]
    Thread(Thread),
    #[cfg(unix)]
    Pipe(Arc<std::os::unix::net::UnixStream>),
}

impl Waker {
    /// Wakes the calling thread out of a park.
    #[cfg_attr(
        all(unix, not(test)),
        expect(dead_code, reason = "the Unix loop is woken through its pipe")
    )]
    pub fn current_thread() -> Self {
        Self(WakeTarget::Thread(thread::current()))
    }

    pub fn wake(&self) {
        match &self.0 {
            WakeTarget::Thread(thread) => thread.unpark(),
            #[cfg(unix)]
            WakeTarget::Pipe(write) => {
                use std::io::Write as _;
                // A full socket buffer already means a pending wake-up, so a
                // failed write needs no handling.
                let _ = (&**write).write(&[1]);
            }
        }
    }
}

/// The read end of the Unix loop's wake-up pipe, and where its [`Waker`]s
/// come from.
#[cfg(unix)]
pub struct WakePipe {
    read: std::os::unix::net::UnixStream,
    write: Arc<std::os::unix::net::UnixStream>,
}

#[cfg(unix)]
impl WakePipe {
    /// A connected socket pair rather than a pipe: `UnixStream::pair` creates
    /// both ends close-on-exec in the same call, so a shell spawned meanwhile
    /// cannot inherit either.
    pub fn new() -> std::io::Result<Self> {
        let (read, write) = std::os::unix::net::UnixStream::pair()?;
        // Neither end may block: a wake-up must not stall the thread raising
        // it, and the loop empties the socket without waiting.
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        Ok(Self {
            read,
            write: Arc::new(write),
        })
    }

    pub fn waker(&self) -> Waker {
        Waker(WakeTarget::Pipe(Arc::clone(&self.write)))
    }

    pub fn descriptor(&self) -> std::os::fd::BorrowedFd<'_> {
        use std::os::fd::AsFd as _;
        self.read.as_fd()
    }

    pub fn drain(&self) {
        use std::io::Read as _;
        let mut bytes = [0_u8; 64];
        while (&self.read).read(&mut bytes).is_ok_and(|read| read > 0) {}
    }
}

/// A bounded channel sender that wakes the session loop after each send.
///
/// The order matters: a wake-up that precedes the event it announces could
/// be consumed by a pass that then finds the queue empty and waits again.
/// Waking afterwards leaves a pending wake-up behind instead, which also
/// covers an event published between the loop draining its queue and
/// waiting.
pub struct WakingSender<T> {
    sender: SyncSender<T>,
    waker: Waker,
}

impl<T> Clone for WakingSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            waker: self.waker.clone(),
        }
    }
}

impl<T> WakingSender<T> {
    pub fn new(sender: SyncSender<T>, waker: Waker) -> Self {
        Self { sender, waker }
    }

    /// Wakes the thread that is creating this sender.
    #[cfg(test)]
    pub fn to_current(sender: SyncSender<T>) -> Self {
        Self::new(sender, Waker::current_thread())
    }

    pub fn send(&self, event: T) -> Result<(), SendError<T>> {
        self.sender.send(event)?;
        self.waker.wake();
        Ok(())
    }

    pub fn try_send(&self, event: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(event)?;
        self.waker.wake();
        Ok(())
    }
}

/// The earliest of the instants the loop has to wake for.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WakeDeadline(Option<Instant>);

impl WakeDeadline {
    /// Also wake at `at`, when there is one.
    pub fn at(&mut self, at: Option<Instant>) {
        if let Some(at) = at {
            self.0 = Some(self.0.map_or(at, |earliest| earliest.min(at)));
        }
    }

    pub fn earliest(self) -> Option<Instant> {
        self.0
    }

    /// Parks the calling thread until the deadline, or until it is unparked.
    /// With no deadline at all it parks until unparked. This is the Windows
    /// loop's wait; the Unix loop waits in `poll` instead.
    #[cfg(any(windows, test))]
    pub fn park(self) {
        match self.0 {
            Some(at) => thread::park_timeout(at.saturating_duration_since(Instant::now())),
            None => thread::park(),
        }
    }
}

#[cfg(test)]
#[path = "tests/wake.rs"]
mod tests;
