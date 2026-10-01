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
        unix,
        allow(
            dead_code,
            reason = "on Unix only tests wake a thread rather than the loop's pipe"
        )
    )]
    Thread(Thread),
    #[cfg(unix)]
    Pipe(Arc<std::os::fd::OwnedFd>),
}

impl Waker {
    /// Wakes the calling thread out of a park.
    #[cfg_attr(
        all(unix, not(test)),
        allow(dead_code, reason = "the Unix loop is woken through its pipe")
    )]
    pub fn current_thread() -> Self {
        Self(WakeTarget::Thread(thread::current()))
    }

    pub fn wake(&self) {
        match &self.0 {
            WakeTarget::Thread(thread) => thread.unpark(),
            #[cfg(unix)]
            WakeTarget::Pipe(write) => {
                use std::os::fd::AsRawFd as _;
                let byte = [1_u8];
                // SAFETY: the descriptor is owned by the pipe this waker holds
                // a reference to, and the buffer is one byte long. A full pipe
                // already means a pending wake-up, so a failed write needs no
                // handling.
                unsafe {
                    libc::write(write.as_raw_fd(), byte.as_ptr().cast(), 1);
                }
            }
        }
    }
}

/// The read end of the Unix loop's wake-up pipe, and where its [`Waker`]s
/// come from.
#[cfg(unix)]
pub struct WakePipe {
    read: std::os::fd::OwnedFd,
    write: Arc<std::os::fd::OwnedFd>,
}

#[cfg(unix)]
impl WakePipe {
    pub fn new() -> std::io::Result<Self> {
        use std::os::fd::{FromRawFd as _, OwnedFd};

        let mut descriptors = [0 as libc::c_int; 2];
        // SAFETY: `pipe` fills the two-element array it is given.
        if unsafe { libc::pipe(descriptors.as_mut_ptr()) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `pipe` returned these descriptors and nothing else owns them.
        let (read, write) = unsafe {
            (
                OwnedFd::from_raw_fd(descriptors[0]),
                OwnedFd::from_raw_fd(descriptors[1]),
            )
        };
        // Neither end may block: a wake-up must not stall the thread raising
        // it, and the loop empties the pipe without waiting.
        for descriptor in [&read, &write] {
            crate::session_io::set_nonblocking_cloexec(descriptor)?;
        }
        Ok(Self {
            read,
            write: Arc::new(write),
        })
    }

    pub fn waker(&self) -> Waker {
        Waker(WakeTarget::Pipe(Arc::clone(&self.write)))
    }

    pub fn descriptor(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd as _;
        self.read.as_raw_fd()
    }

    pub fn drain(&self) {
        let mut bytes = [0_u8; 64];
        // SAFETY: the descriptor is owned by this value and the buffer is as
        // long as the count passed with it.
        while unsafe { libc::read(self.descriptor(), bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {
        }
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
