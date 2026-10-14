//! Bounded hosted output transport with parser-completion fences.
//! Source bytes are copied once into recycled transport buffers; LF is untouched.
use super::{HostedOutput, OutputAcknowledgement, OutputMode};
use polling::os::iocp::{CompletionPacket, PollerIocpExt};
use polling::{Event, PollMode, Poller};
use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::{Arc, Condvar, Mutex};

const CAPACITY: usize = crate::event_loop::READ_BUFFER_SIZE;
const BUFFER_SIZE: usize = 128 * 1024;
struct Frame {
    bytes: Vec<u8>,
    offset: usize,
    mode: OutputMode,
}
struct State {
    ready: VecDeque<Frame>,
    free: Vec<Vec<u8>>,
    queued: usize,
    published: u64,
    parsed: u64,
    ended: bool,
    input_ready: bool,
    cancelled: bool,
    interest: Option<(Arc<Poller>, Event)>,
    armed: bool,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn notify(state: &mut State) {
        if !state.armed {
            return;
        }
        if let Some((poller, event)) = &state.interest {
            let mut ready = *event;
            ready.readable &= !state.ready.is_empty() || state.ended || state.cancelled;
            ready.writable &= state.input_ready;
            if (ready.readable || ready.writable)
                && poller.post(CompletionPacket::new(ready)).is_ok()
            {
                state.armed = false;
            }
        }
    }
}
impl OutputAcknowledgement for Shared {
    fn parsed(&self, bytes: usize) {
        self.lock().parsed += bytes as u64;
        self.changed.notify_all();
    }
}
#[derive(Clone)]
pub struct OutputWriter(Arc<Shared>);
pub struct OutputReader {
    shared: Arc<Shared>,
    sampled_empty: bool,
}
pub fn channel() -> (OutputWriter, OutputReader) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            ready: VecDeque::new(),
            free: (0..CAPACITY / BUFFER_SIZE).map(|_| Vec::with_capacity(BUFFER_SIZE)).collect(),
            queued: 0,
            published: 0,
            parsed: 0,
            ended: false,
            input_ready: true,
            cancelled: false,
            interest: None,
            armed: false,
        }),
        changed: Condvar::new(),
    });
    (OutputWriter(Arc::clone(&shared)), OutputReader { shared, sampled_empty: false })
}
impl OutputWriter {
    pub fn write(&self, bytes: &[u8], mode: OutputMode) -> io::Result<()> {
        for chunk in bytes.chunks(BUFFER_SIZE) {
            let mut state = self.0.lock();
            while (state.queued + chunk.len() > CAPACITY || state.ready.len() >= 256)
                && !state.cancelled
                && !state.ended
            {
                state = self.0.changed.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if state.cancelled || state.ended {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "hosted output closed"));
            }
            if let Some(frame) = state.ready.back_mut().filter(|frame| {
                frame.mode == mode && frame.bytes.len() + chunk.len() <= BUFFER_SIZE
            }) {
                frame.bytes.extend_from_slice(chunk);
            } else {
                let mut data = state.free.pop().unwrap_or_default();
                data.clear();
                data.extend_from_slice(chunk);
                state.ready.push_back(Frame { bytes: data, offset: 0, mode });
            }
            state.queued += chunk.len();
            state.published += chunk.len() as u64;
            Shared::notify(&mut state);
        }
        Ok(())
    }
    /// Wait for all preceding output to reach the grid, not just the reader.
    pub fn flush(&self) -> io::Result<()> {
        let mut state = self.0.lock();
        let target = state.published;
        while state.parsed < target && !state.cancelled {
            state = self.0.changed.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if state.cancelled {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "hosted parser closed"))
        } else {
            Ok(())
        }
    }
    pub fn set_input_ready(&self, ready: bool) {
        let mut state = self.0.lock();
        state.input_ready = ready;
        Shared::notify(&mut state);
    }
    pub fn end(&self) {
        let mut state = self.0.lock();
        state.ended = true;
        Shared::notify(&mut state);
        self.0.changed.notify_all();
    }
    pub fn cancel(&self) {
        let mut state = self.0.lock();
        state.cancelled = true;
        Shared::notify(&mut state);
        self.0.changed.notify_all();
    }
}
impl Read for OutputReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut state = self.shared.lock();
        if std::mem::take(&mut self.sampled_empty) {
            return if state.ended && state.ready.is_empty() || state.cancelled {
                Ok(0)
            } else {
                Err(io::ErrorKind::WouldBlock.into())
            };
        }
        let Some(frame) = state.ready.front_mut() else {
            return if state.ended || state.cancelled {
                Ok(0)
            } else {
                Err(io::ErrorKind::WouldBlock.into())
            };
        };
        let count = buffer.len().min(frame.bytes.len() - frame.offset);
        buffer[..count].copy_from_slice(&frame.bytes[frame.offset..frame.offset + count]);
        frame.offset += count;
        if frame.offset == frame.bytes.len() {
            let frame = state.ready.pop_front().unwrap();
            if state.free.len() < CAPACITY / BUFFER_SIZE {
                state.free.push(frame.bytes);
            }
        }
        state.queued -= count;
        self.shared.changed.notify_all();
        Ok(count)
    }
}
impl HostedOutput for OutputReader {
    fn mode(&mut self) -> io::Result<Option<OutputMode>> {
        let mode = self.shared.lock().ready.front().map(|frame| frame.mode);
        self.sampled_empty = mode.is_none();
        Ok(mode)
    }
    fn ended(&self) -> bool {
        let state = self.shared.lock();
        state.ended && state.ready.is_empty()
    }
    fn acknowledgement(&self) -> Arc<dyn OutputAcknowledgement> {
        self.shared.clone()
    }
    fn register(&mut self, poller: &Arc<Poller>, event: Event, _: PollMode) {
        let mut state = self.shared.lock();
        state.interest = Some((Arc::clone(poller), event));
        state.armed = true;
        Shared::notify(&mut state);
    }
    fn deregister(&mut self) {
        self.shared.lock().interest = None;
    }
    fn rearm(&mut self) {
        let mut state = self.shared.lock();
        state.armed = true;
        Shared::notify(&mut state);
    }
}
impl Drop for OutputReader {
    fn drop(&mut self) {
        OutputWriter(Arc::clone(&self.shared)).cancel();
    }
}

#[cfg(test)]
#[path = "../tests/hosted_output.rs"]
mod tests;
