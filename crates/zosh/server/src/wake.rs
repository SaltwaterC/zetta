//! How the session loop sleeps, and how everything that feeds it wakes it.
//!
//! The loop owns no file descriptor it waits on. Every input arrives from a
//! thread of its own — UDP datagrams, PTY output and write completions, agent
//! connections, the child's exit — through a [`WakingSender`], which unparks
//! the loop after publishing. Everything else the loop does is driven by a
//! clock, and [`WakeDeadline`] collects those clocks into the one instant it
//! parks until. Between the two there is nothing to poll: an idle session
//! wakes for its transport's heartbeat and the client's keep-alive, not every
//! few milliseconds.
//!
//! A park can also end spuriously, which is harmless because every pass of
//! the loop is safe to repeat.

use std::sync::mpsc::{SendError, SyncSender, TrySendError};
use std::thread::{self, Thread};
use std::time::Instant;

/// A bounded channel sender that unparks the session loop after each send.
///
/// The order matters: an unpark that precedes the event it announces could
/// be consumed by a pass that then finds the queue empty and parks again.
/// Unparking afterwards leaves a park token behind instead, which also
/// covers an event published between the loop draining its queue and
/// parking.
pub struct WakingSender<T> {
    sender: SyncSender<T>,
    consumer: Thread,
}

impl<T> Clone for WakingSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            consumer: self.consumer.clone(),
        }
    }
}

impl<T> WakingSender<T> {
    /// Wakes `consumer`, which must be the thread that drains the channel.
    pub fn new(sender: SyncSender<T>, consumer: Thread) -> Self {
        Self { sender, consumer }
    }

    /// Wakes the thread that is creating this sender. The session loop builds
    /// its inputs on its own thread, so this is how it names itself.
    pub fn to_current(sender: SyncSender<T>) -> Self {
        Self::new(sender, thread::current())
    }

    pub fn send(&self, event: T) -> Result<(), SendError<T>> {
        self.sender.send(event)?;
        self.consumer.unpark();
        Ok(())
    }

    pub fn try_send(&self, event: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(event)?;
        self.consumer.unpark();
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
    /// With no deadline at all it parks until unparked.
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
