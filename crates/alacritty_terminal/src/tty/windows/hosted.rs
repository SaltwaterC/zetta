//! Windows native/hosted stream adapters; hosted control is shared only with
//! the input writer. The output reader never locks the console control object.
use super::{ReadPipe, WritePipe};
use crate::hosted_console::{HostedConsole, HostedOutput, OutputAcknowledgement, OutputMode};
use polling::{Event, PollMode, Poller};
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

pub(super) type Control = Arc<Mutex<Box<dyn HostedConsole>>>;
pub enum PtyReader {
    Native(ReadPipe),
    Hosted(Box<dyn HostedOutput>),
}
pub enum PtyWriter {
    Native(WritePipe),
    Hosted(Control),
}
impl From<ReadPipe> for PtyReader {
    fn from(value: ReadPipe) -> Self {
        Self::Native(value)
    }
}
impl From<WritePipe> for PtyWriter {
    fn from(value: WritePipe) -> Self {
        Self::Native(value)
    }
}
impl Read for PtyReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Native(reader) => reader.read(bytes),
            Self::Hosted(reader) => reader.read(bytes),
        }
    }
}
impl Write for PtyWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Native(writer) => writer.write(bytes),
            Self::Hosted(console) => hosted_input(
                console.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_mut(),
                bytes,
            ),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Native(writer) => writer.flush(),
            Self::Hosted(_) => Ok(()),
        }
    }
}
/// Input for a console whose clients have all gone is dropped, as a ConPTY
/// input pipe accepts it after its console exits. Failing instead would turn
/// a terminal reply that races the child's exit into a backend failure; and
/// retrying is no better, since a child can free its console and keep running.
fn hosted_input(console: &mut dyn HostedConsole, bytes: &[u8]) -> io::Result<usize> {
    match console.input(bytes) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(bytes.len()),
        result => result,
    }
}

impl PtyReader {
    pub fn try_read(&mut self, bytes: &mut [u8]) -> usize {
        match self {
            Self::Native(reader) => reader.try_read(bytes),
            Self::Hosted(reader) => reader.read(bytes).unwrap_or(0),
        }
    }
    pub fn register(&mut self, poller: &Arc<Poller>, event: Event, mode: PollMode) {
        match self {
            Self::Native(reader) => reader.register(poller, event, mode),
            Self::Hosted(reader) => reader.register(poller, event, mode),
        }
    }
    pub fn deregister(&mut self) {
        match self {
            Self::Native(reader) => reader.deregister(),
            Self::Hosted(reader) => reader.deregister(),
        }
    }
    pub(super) fn mode(&mut self) -> io::Result<Option<OutputMode>> {
        match self {
            Self::Native(_) => Ok(None),
            Self::Hosted(reader) => reader.mode(),
        }
    }
    pub(super) fn acknowledgement(&self) -> Option<Arc<dyn OutputAcknowledgement>> {
        match self {
            Self::Native(_) => None,
            Self::Hosted(reader) => Some(reader.acknowledgement()),
        }
    }
    pub(super) fn source_ended(&self) -> bool {
        match self {
            Self::Native(reader) => reader.source_ended(),
            Self::Hosted(reader) => reader.ended(),
        }
    }
    pub(super) fn pause(&mut self) -> io::Result<()> {
        match self {
            Self::Native(reader) => reader.pause(),
            Self::Hosted(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "hosted consoles cannot hand over byte-only pipes",
            )),
        }
    }
    pub(super) fn resume(&mut self) {
        if let Self::Native(reader) = self {
            reader.resume();
        }
    }
    pub(super) fn try_read_buffered(&mut self, buffer: &mut [u8]) -> usize {
        match self {
            Self::Native(reader) => reader.try_read_buffered(buffer),
            Self::Hosted(_) => 0,
        }
    }
    pub fn rearm(&mut self) {
        match self {
            Self::Native(reader) => reader.rearm(),
            Self::Hosted(reader) => reader.rearm(),
        }
    }
}
impl PtyWriter {
    pub fn register(&mut self, poller: &Arc<Poller>, event: Event, mode: PollMode) {
        if let Self::Native(writer) = self {
            writer.register(poller, event, mode);
        }
    }
    pub fn deregister(&mut self) {
        if let Self::Native(writer) = self {
            writer.deregister();
        }
    }
}

#[cfg(test)]
#[path = "../../tests/tty/windows/hosted.rs"]
mod tests;
