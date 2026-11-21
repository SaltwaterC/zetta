//! The log a running server shows in its pane: lines its workers send down a
//! channel, read back as one byte stream.
//!
//! `ztftp` includes this file by path rather than depending on `zhttp`: both
//! servers stream their logs the same way, and neither needs the other.

use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// How long a read waits for the next line before reporting that there is
/// none yet.
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct LogReader {
    receiver: Receiver<Vec<u8>>,
    pending: Vec<u8>,
    offset: usize,
    /// What a read reports while no line has arrived, naming the server.
    idle: &'static str,
}

impl LogReader {
    pub(crate) fn new(receiver: Receiver<Vec<u8>>, idle: &'static str) -> Self {
        Self {
            receiver,
            pending: Vec::new(),
            offset: 0,
            idle,
        }
    }
}

impl Read for LogReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.offset == self.pending.len() {
            match self.receiver.recv_timeout(LOG_POLL_INTERVAL) {
                Ok(message) => {
                    self.pending = message;
                    self.offset = 0;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(io::ErrorKind::WouldBlock, self.idle));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let count = output.len().min(self.pending.len() - self.offset);
        output[..count].copy_from_slice(&self.pending[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}
