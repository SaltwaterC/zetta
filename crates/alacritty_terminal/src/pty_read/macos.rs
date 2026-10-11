//! Bounded retries across macOS PTY refill gaps. The kernel hands the master roughly 1 KiB
//! per read, even with a larger buffer; returning to kqueue at every gap makes a bulk producer
//! and both PTY threads wake each other for each small batch. Consecutive chunks share a bulk
//! classification until the stream goes quiet, so short reads at a chunk boundary keep their
//! refill budget. Spaced small bursts only retry sixteen times, keeping interactive and idle
//! CPU costs low. Empty reads, EOF and errors never retry; deadlines bound gathering latency.

use std::time::{Duration, Instant};

pub(super) const CHUNK_SIZE: usize = 256 * 1024;

pub(super) const POLL_MODE: polling::PollMode = polling::PollMode::Oneshot;
pub(super) const REARM_READ: bool = true;

const MAX_RETRIES: usize = 256;
const SMALL_RETRIES: usize = 16;
const BULK_THRESHOLD: usize = 8 * 1024;
const CLOCK_INTERVAL: usize = 16;
const BURST_GAP: Duration = Duration::from_millis(1);
const RETRY_BUDGET: Duration = Duration::from_millis(3);

/// Stream activity plus one batch's refill budget. Successful reads restart the attempt count,
/// but never the deadline; a trickling producer cannot keep the batch in the reader indefinitely.
#[derive(Default)]
pub(super) struct ReadRetry {
    attempts: usize,
    started: Option<Instant>,
    until_check: usize,
    recent_bytes: usize,
    delivered: Option<Instant>,
}

impl ReadRetry {
    fn retry_limit(&self, bytes: usize) -> usize {
        if self.recent_bytes.max(bytes) < BULK_THRESHOLD { SMALL_RETRIES } else { MAX_RETRIES }
    }

    pub(super) fn begin_batch(&mut self) {
        self.begin_at(Instant::now());
    }

    fn begin_at(&mut self, now: Instant) {
        if self.delivered.is_none_or(|last| now.duration_since(last) >= BURST_GAP) {
            self.recent_bytes = 0;
        }
        self.attempts = 0;
        self.started = None;
        self.until_check = 0;
    }

    pub(super) fn finish_batch(&mut self, bytes: usize) {
        if bytes != 0 {
            self.delivered = Some(Instant::now());
        }
    }

    pub(super) fn on_read(&mut self, bytes: usize) {
        self.recent_bytes = self.recent_bytes.saturating_add(bytes).min(BULK_THRESHOLD);
        self.attempts = 0;
    }

    pub(super) fn should_retry(&mut self, bytes: usize) -> bool {
        if bytes == 0 {
            return false;
        }
        if self.attempts >= self.retry_limit(bytes) {
            return false;
        }
        if self.until_check != 0 {
            self.until_check -= 1;
            self.attempts += 1;
            return true;
        }
        // Keep clock reads out of most refill attempts. Success does not reset this interval,
        // so the deadline is checked after at most sixteen failed reads, even with trickles.
        self.until_check = CLOCK_INTERVAL - 1;
        self.retry_at(bytes, Instant::now())
    }

    fn retry_at(&mut self, bytes: usize, now: Instant) -> bool {
        if bytes == 0 || self.attempts >= self.retry_limit(bytes) {
            return false;
        }
        let started = *self.started.get_or_insert(now);
        if now.duration_since(started) >= RETRY_BUDGET {
            return false;
        }
        self.attempts += 1;
        true
    }
}

#[cfg(test)]
#[path = "../tests/pty_read/macos.rs"]
mod tests;
