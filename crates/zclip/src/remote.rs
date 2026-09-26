//! Clipboard helper transport over a separate controlling terminal.

use crate::protocol::{CHUNK_SIZE, Frame, Message, Scanner, WINDOW_SIZE, new_request_id};
use anyhow::{Context as _, Result, bail, ensure};
use std::{
    collections::VecDeque,
    env,
    fs::File,
    io::{self, Read as _, Seek as _, Write as _},
    time::{Duration, Instant},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);

pub fn should_probe() -> bool {
    should_probe_with(
        |name| env::var_os(name).is_some(),
        cfg!(feature = "backend"),
    )
}

fn should_probe_with(mut has: impl FnMut(&str) -> bool, native_backend: bool) -> bool {
    !has("ZCLIP_HOST_BACKEND")
        && (has("SSH_CONNECTION")
            || has("SSH_TTY")
            || has("ZOSH_CLIPBOARD_CHANNEL")
            // Local Zetta shells have this marker too. Only a backend-free
            // helper needs it to recognize a remote shared pane whose daemon
            // was started outside SSH or zosh.
            || (!native_backend && has("ZETTA_TERM")))
}

/// `None` means that no Zetta channel answered the probe. Once a channel
/// answers, all failures are reported instead of silently using another
/// machine's clipboard.
pub fn copy() -> Result<Option<()>> {
    let Some(mut channel) = Channel::probe()? else {
        return Ok(None);
    };
    channel.send(Message::Copy)?;
    channel.expect_ack(0)?;
    let mut stdin = io::stdin().lock();
    let mut sent = 0_u64;
    let mut acknowledged = 0_u64;
    let mut eof = false;
    let mut chunk = [0; CHUNK_SIZE];
    while !eof || acknowledged < sent {
        while !eof && sent - acknowledged < WINDOW_SIZE as u64 {
            let count = stdin.read(&mut chunk).context("reading standard input")?;
            if count == 0 {
                eof = true;
                break;
            }
            channel.send(Message::Data {
                sequence: sent,
                bytes: chunk[..count].to_vec(),
            })?;
            sent += 1;
        }
        if acknowledged < sent {
            match channel.receive(TRANSFER_TIMEOUT)? {
                Message::Ack { next_sequence }
                    if next_sequence > acknowledged && next_sequence <= sent =>
                {
                    acknowledged = next_sequence;
                }
                other => bail!("unexpected clipboard acknowledgement: {other:?}"),
            }
        }
    }
    channel.send(Message::End)?;
    channel.expect_done()?;
    Ok(Some(()))
}

pub fn paste() -> Result<Option<()>> {
    let Some(mut channel) = Channel::probe()? else {
        return Ok(None);
    };
    channel.send(Message::Paste)?;
    let mut spool = tempfile::tempfile().context("creating private clipboard spool")?;
    let mut sequence = 0;
    loop {
        match channel.receive(TRANSFER_TIMEOUT)? {
            Message::Data {
                sequence: received,
                bytes,
            } if received == sequence => {
                spool.write_all(&bytes).context("spooling clipboard text")?;
                sequence += 1;
                channel.send(Message::Ack {
                    next_sequence: sequence,
                })?;
            }
            Message::Done => break,
            other => bail!("unexpected clipboard response: {other:?}"),
        }
    }
    spool.rewind().context("rewinding clipboard spool")?;
    let mut bytes = Vec::new();
    spool
        .read_to_end(&mut bytes)
        .context("reading clipboard spool")?;
    std::str::from_utf8(&bytes).context("clipboard contents are not UTF-8 text")?;
    io::stdout()
        .lock()
        .write_all(&bytes)
        .context("writing clipboard text to standard output")?;
    Ok(Some(()))
}

struct Channel {
    tty: ControllingTty,
    id: [u8; 16],
    scanner: Scanner,
    responses: VecDeque<Message>,
}

impl Channel {
    fn probe() -> Result<Option<Self>> {
        if !should_probe() {
            return Ok(None);
        }
        let Some(tty) = ControllingTty::open()? else {
            return Ok(None);
        };
        let mut channel = Self {
            tty,
            id: new_request_id().context("generating clipboard request ID")?,
            scanner: Scanner::default(),
            responses: VecDeque::new(),
        };
        channel.send(Message::Probe)?;
        match channel.receive(PROBE_TIMEOUT) {
            Ok(Message::Ready) => Ok(Some(channel)),
            Err(error) if error.is::<ProbeTimeout>() => Ok(None),
            Err(error) => Err(error),
            Ok(other) => bail!("unexpected clipboard probe response: {other:?}"),
        }
    }

    fn send(&mut self, message: Message) -> Result<()> {
        self.tty
            .write_all(
                &Frame {
                    id: self.id,
                    message,
                }
                .encode(),
            )
            .context("sending clipboard request")
    }

    fn receive(&mut self, timeout: Duration) -> Result<Message> {
        let mut input = [0; 8192];
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.responses.pop_front() {
                if let Message::Error(error) = message {
                    bail!("remote clipboard: {error}");
                }
                return Ok(message);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ProbeTimeout.into());
            }
            let count = self.tty.read_timeout(&mut input, remaining)?;
            if count == 0 {
                return Err(ProbeTimeout.into());
            }
            self.scanner.filter(&input[..count], |frame| {
                if frame.id == self.id {
                    self.responses.push_back(frame.message);
                }
            });
        }
    }

    fn expect_ack(&mut self, next_sequence: u64) -> Result<()> {
        ensure!(
            self.receive(TRANSFER_TIMEOUT)? == (Message::Ack { next_sequence }),
            "unexpected clipboard acknowledgement"
        );
        Ok(())
    }

    fn expect_done(&mut self) -> Result<()> {
        ensure!(
            self.receive(TRANSFER_TIMEOUT)? == Message::Done,
            "unexpected clipboard completion"
        );
        Ok(())
    }
}

#[derive(Debug)]
struct ProbeTimeout;

impl std::fmt::Display for ProbeTimeout {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("clipboard channel timed out")
    }
}

impl std::error::Error for ProbeTimeout {}

#[cfg(test)]
#[path = "tests/remote.rs"]
mod tests;

#[cfg(unix)]
struct ControllingTty {
    file: File,
    original: libc::termios,
}

#[cfg(unix)]
impl ControllingTty {
    fn open() -> Result<Option<Self>> {
        use std::fs::OpenOptions;
        use std::os::fd::AsRawFd as _;
        let file = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => return Ok(None),
            Err(error) => return Err(error).context("opening controlling terminal"),
        };
        // SAFETY: termios is initialized by tcgetattr before it is read.
        let mut original = unsafe { std::mem::zeroed() };
        // SAFETY: the file descriptor and termios pointer are valid for this call.
        if unsafe { libc::tcgetattr(file.as_raw_fd(), &raw mut original) } == -1 {
            return Err(io::Error::last_os_error()).context("reading terminal mode");
        }
        let mut raw = original;
        // SAFETY: raw is initialized and owned by this function.
        unsafe { libc::cfmakeraw(&raw mut raw) };
        // SAFETY: the descriptor stays open while the mode is changed.
        if unsafe { libc::tcsetattr(file.as_raw_fd(), libc::TCSANOW, &raw) } == -1 {
            return Err(io::Error::last_os_error()).context("setting terminal mode");
        }
        Ok(Some(Self { file, original }))
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)
    }

    fn read_timeout(&mut self, bytes: &mut [u8], timeout: Duration) -> Result<usize> {
        use std::os::fd::AsRawFd as _;
        let mut poll = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let milliseconds = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: poll points to one valid initialized pollfd.
        let ready = unsafe { libc::poll(&raw mut poll, 1, milliseconds) };
        if ready == 0 {
            return Ok(0);
        }
        if ready == -1 {
            return Err(io::Error::last_os_error()).context("waiting for clipboard response");
        }
        self.file.read(bytes).context("reading clipboard response")
    }
}

#[cfg(unix)]
impl Drop for ControllingTty {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd as _;
        // SAFETY: this descriptor and saved termios remain valid until drop ends.
        unsafe {
            libc::tcsetattr(
                self.file.as_raw_fd(),
                libc::TCSANOW,
                &raw const self.original,
            )
        };
    }
}

#[cfg(windows)]
struct ControllingTty {
    input: File,
    output: File,
    original_input: windows::Win32::System::Console::CONSOLE_MODE,
    original_output: windows::Win32::System::Console::CONSOLE_MODE,
}

#[cfg(windows)]
impl ControllingTty {
    fn open() -> Result<Option<Self>> {
        use std::fs::OpenOptions;
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Console::{
            CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
            SetConsoleMode,
        };

        let input = match OpenOptions::new().read(true).open("CONIN$") {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("opening controlling console input"),
        };
        let output = OpenOptions::new()
            .write(true)
            .open("CONOUT$")
            .context("opening controlling console output")?;
        let input_handle = HANDLE(input.as_raw_handle());
        let output_handle = HANDLE(output.as_raw_handle());
        let mut original_input = CONSOLE_MODE(0);
        let mut original_output = CONSOLE_MODE(0);
        // SAFETY: the files own valid console handles and the mode values are writable.
        unsafe {
            GetConsoleMode(input_handle, &raw mut original_input)?;
            GetConsoleMode(output_handle, &raw mut original_output)?;
        }
        let raw_input = (original_input
            & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        // SAFETY: the handles and bitsets are valid console API values.
        unsafe { SetConsoleMode(input_handle, raw_input) }
            .context("enabling virtual terminal input")?;
        // SAFETY: the handles and bitsets are valid console API values.
        if let Err(error) = unsafe {
            SetConsoleMode(
                output_handle,
                original_output | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            )
        } {
            // SAFETY: restore the mode captured from this input handle.
            let _ = unsafe { SetConsoleMode(input_handle, original_input) };
            return Err(error).context("enabling virtual terminal output");
        }
        Ok(Some(Self {
            input,
            output,
            original_input,
            original_output,
        }))
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.output.write_all(bytes)
    }

    fn read_timeout(&mut self, bytes: &mut [u8], timeout: Duration) -> Result<usize> {
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows::Win32::System::Threading::WaitForSingleObject;
        let handle = HANDLE(self.input.as_raw_handle());
        let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: the input file owns a live console handle during this call.
        match unsafe { WaitForSingleObject(handle, milliseconds) } {
            WAIT_TIMEOUT => Ok(0),
            WAIT_OBJECT_0 => self.input.read(bytes).context("reading clipboard response"),
            _ => Err(io::Error::last_os_error()).context("waiting for clipboard response"),
        }
    }
}

#[cfg(windows)]
impl Drop for ControllingTty {
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Console::SetConsoleMode;
        // SAFETY: both handles and modes were captured from these files in open.
        let _ = unsafe { SetConsoleMode(HANDLE(self.input.as_raw_handle()), self.original_input) };
        // SAFETY: both handles and modes were captured from these files in open.
        let _ =
            unsafe { SetConsoleMode(HANDLE(self.output.as_raw_handle()), self.original_output) };
    }
}
