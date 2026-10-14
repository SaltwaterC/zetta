//! Contract for a console hosted by an application rather than a native PTY.
//!
//! Output is an ordered byte stream with semantics attached to each batch.
//! Backends stop reads at mode boundaries; the parser applies that batch's mode
//! on its own thread. A global atomic mode is not sufficient: earlier bytes may
//! still be queued when a child changes its output settings.
//!
//! Input is the terminal's normal encoded keyboard/paste stream. Resize, exit,
//! close and output EOF are independent; child exit never means discard output.
use crate::event::WindowSize;
use polling::{Event, PollMode, Poller};
use std::io::{self, Read, Write};
use std::sync::Arc;

/// Host semantics for the next output batch, independent of any driver ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputMode {
    pub lf_implies_cr: bool,
    pub wrap: bool,
}

/// The production-facing control contract. Implementations own the child tree.
/// Input accepts partial writes or WouldBlock; it must never wait for a console
/// read while holding the parser/grid lock. Paste uses the same ordered stream.
pub trait HostedConsole: Send {
    fn child_pid(&self) -> u32;
    #[cfg(windows)]
    fn child_handle(&self) -> io::Result<std::os::windows::io::OwnedHandle>;
    fn input(&mut self, bytes: &[u8]) -> io::Result<usize>;
    fn resize(&mut self, size: WindowSize) -> io::Result<()>;
    /// Cancellation must unblock pending reads, writes and output publication.
    /// The final drain runs before close; close is idempotent.
    fn close(&mut self);
}

/// Output transport supplied to a hosted backend before starting the child.
/// Buffers and publication are bounded; EOF means every published byte remains
/// readable before Read returns zero. Polling wakes on output, EOF and exit.
pub trait HostedOutput: Read + Send {
    /// Describes the next successful read. If sampled while empty, a newly
    /// published batch must wait for another mode sample before it is read.
    fn mode(&mut self) -> io::Result<Option<OutputMode>>;
    fn ended(&self) -> bool;
    fn acknowledgement(&self) -> Arc<dyn OutputAcknowledgement>;
    fn register(&mut self, poller: &Arc<Poller>, event: Event, mode: PollMode);
    fn deregister(&mut self);
    fn rearm(&mut self);
}

/// The spawn result handed to the existing event loop, with no driver handles
/// or driver message layouts crossing this boundary.
pub struct HostedPty {
    pub console: Box<dyn HostedConsole>,
    pub output: Box<dyn HostedOutput>,
}

impl Write for Box<dyn HostedConsole> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.input(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Parser completion, not merely bytes taken out of the source queue.
pub trait OutputAcknowledgement: Send + Sync {
    fn parsed(&self, bytes: usize);
}

#[cfg(windows)]
#[path = "hosted_console/queue.rs"]
pub mod queue;
