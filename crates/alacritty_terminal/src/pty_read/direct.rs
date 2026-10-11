//! Other platforms publish a batch as soon as the PTY would block. macOS's small kernel
//! queue needs a separate refill policy; keeping this one empty preserves the existing loop
//! without clock reads or additional read syscalls on Linux and Windows.

pub(super) const CHUNK_SIZE: usize = u16::MAX as usize;

pub(super) const POLL_MODE: polling::PollMode = polling::PollMode::Level;
pub(super) const REARM_READ: bool = false;

#[derive(Default)]
pub(super) struct ReadRetry {}

impl ReadRetry {
    pub(super) fn begin_batch(&mut self) {}

    pub(super) fn finish_batch(&mut self, _: usize) {}

    pub(super) fn on_read(&mut self, _: usize) {}

    pub(super) fn should_retry(&mut self, _: usize) -> bool {
        false
    }
}
