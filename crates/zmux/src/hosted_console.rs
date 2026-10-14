//! A console-server console held by the pseudoconsole host.
//!
//! The host hands every console to the daemon as two byte pipes, and the
//! daemon and its clients parse what comes out as a terminal stream. ConPTY
//! makes its output self-contained for exactly that reason; this does the
//! same for console-server's, the way OpenConsole's VtIo writer does: LF
//! becomes CRLF while the console's mode says so, wrap changes are DECAWM,
//! and SetConsoleTitle is an OSC. The bytes are then copied to the daemon's
//! pipe and, separately, parsed into a grid this host keeps so that screen
//! queries (cursor, cells, colors) have something to read. The daemon's pipe
//! never waits for that parse; only a query does.
#![cfg(windows)]

use std::collections::VecDeque;
use std::io::{self, Read as _, Write as _};
use std::os::windows::io::{AsRawHandle as _, OwnedHandle};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{Config, Term, test::TermSize};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use anyhow::{Context as _, Result};
use console_server::output::Cell;
use console_server::{ConsoleOutput, Launch, OutputMode, Screen, Session, grid};

/// Bytes queued towards the daemon or the host's parser before writes wait.
const QUEUE_LIMIT: usize = 1024 * 1024;
/// Matches the kernel pipe ConPTY's output uses.
const OUTPUT_PIPE_CAPACITY: u32 = 128 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Turns console output into a stream that needs no side channel.
#[derive(Default)]
pub(crate) struct Translator {
    wrap: Option<bool>,
    /// The last byte written was CR, so an LF right after it needs none.
    after_cr: bool,
}
impl Translator {
    pub(crate) fn translate(&mut self, bytes: &[u8], mode: OutputMode) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len() + 8);
        if self.wrap != Some(mode.wrap) {
            // The terminal's own default is to wrap; only a change is news.
            if self.wrap.is_some() || !mode.wrap {
                out.extend_from_slice(if mode.wrap { b"\x1b[?7h" } else { b"\x1b[?7l" });
            }
            self.wrap = Some(mode.wrap);
        }
        if bytes.is_empty() {
            return out;
        }
        if !mode.lf_implies_cr {
            out.extend_from_slice(bytes);
        } else {
            // WriteUTF16TranslateCRLF: a CR before every LF not already after one.
            let mut start = 0;
            let mut previous_cr = self.after_cr;
            for (index, &byte) in bytes.iter().enumerate() {
                if byte == b'\n' && !previous_cr {
                    out.extend_from_slice(&bytes[start..index]);
                    out.push(b'\r');
                    start = index;
                }
                previous_cr = byte == b'\r';
            }
            out.extend_from_slice(&bytes[start..]);
        }
        self.after_cr = bytes.last() == Some(&b'\r');
        out
    }
}

/// VtIo::Writer::WriteWindowTitle: OSC 0 with controls made harmless.
pub(crate) fn title_sequence(title: &str) -> Vec<u8> {
    let mut out = b"\x1b]0;".to_vec();
    for ch in title.chars() {
        if (ch as u32) < 0x20 || ch == '\x7f' || ('\u{80}'..='\u{9f}').contains(&ch) {
            out.push(b' ');
        } else {
            let mut buffer = [0; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
        }
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

#[derive(Default)]
struct Queues {
    to_daemon: VecDeque<Arc<[u8]>>,
    to_parser: VecDeque<Arc<[u8]>>,
    daemon_bytes: usize,
    parser_bytes: usize,
    published: u64,
    parsed: u64,
    ended: bool,
    cancelled: bool,
}
#[derive(Default)]
struct Pipeline {
    queues: Mutex<Queues>,
    changed: Condvar,
}
impl Pipeline {
    fn publish(&self, chunk: Vec<u8>) -> io::Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        let length = chunk.len();
        let chunk: Arc<[u8]> = chunk.into();
        let mut queues = lock(&self.queues);
        while (queues.daemon_bytes > QUEUE_LIMIT || queues.parser_bytes > QUEUE_LIMIT)
            && !queues.cancelled
        {
            queues = self
                .changed
                .wait(queues)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if queues.cancelled {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        queues.to_daemon.push_back(Arc::clone(&chunk));
        queues.to_parser.push_back(chunk);
        queues.daemon_bytes += length;
        queues.parser_bytes += length;
        queues.published += length as u64;
        self.changed.notify_all();
        Ok(())
    }
    /// Waits for the next chunk for one consumer; None once output ended.
    fn next(&self, daemon: bool) -> Option<Arc<[u8]>> {
        let mut queues = lock(&self.queues);
        loop {
            let queue = if daemon {
                &mut queues.to_daemon
            } else {
                &mut queues.to_parser
            };
            if let Some(chunk) = queue.pop_front() {
                if daemon {
                    queues.daemon_bytes -= chunk.len();
                } else {
                    queues.parser_bytes -= chunk.len();
                }
                self.changed.notify_all();
                return Some(chunk);
            }
            if queues.ended || queues.cancelled {
                return None;
            }
            queues = self
                .changed
                .wait(queues)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
    fn parsed(&self, bytes: usize) {
        lock(&self.queues).parsed += bytes as u64;
        self.changed.notify_all();
    }
    fn flush(&self) -> io::Result<()> {
        let mut queues = lock(&self.queues);
        let target = queues.published;
        while queues.parsed < target && !queues.cancelled {
            queues = self
                .changed
                .wait(queues)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if queues.cancelled {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
    fn end(&self) {
        lock(&self.queues).ended = true;
        self.changed.notify_all();
    }
    fn cancel(&self) {
        lock(&self.queues).cancelled = true;
        self.changed.notify_all();
    }
}

type SharedTerm = Arc<Mutex<Term<VoidListener>>>;

struct HostOutput {
    pipeline: Arc<Pipeline>,
    term: SharedTerm,
    translator: Translator,
}
impl ConsoleOutput for HostOutput {
    fn write(&mut self, bytes: &[u8], mode: OutputMode) -> io::Result<()> {
        let chunk = self.translator.translate(bytes, mode);
        self.pipeline.publish(chunk)
    }
    fn flush(&self) -> io::Result<()> {
        self.pipeline.flush()
    }
    fn size(&self) -> (i16, i16) {
        let screen = grid::screen(&lock(&self.term));
        (screen.columns, screen.rows)
    }
    fn cursor(&self) -> (i16, i16) {
        grid::screen(&lock(&self.term)).cursor
    }
    fn screen(&self) -> Screen {
        grid::screen(&lock(&self.term))
    }
    fn read_cells(
        &self,
        rows: std::ops::Range<i16>,
        columns: std::ops::Range<i16>,
    ) -> Option<Vec<Cell>> {
        Some(grid::read_cells(&lock(&self.term), rows, columns))
    }
    fn title(&self) -> Option<String> {
        lock(&self.term).title().map(str::to_owned)
    }
    /// In band, so the daemon, every client and this host's grid all see it.
    fn set_title(&mut self, title: &str) -> io::Result<()> {
        self.pipeline.publish(title_sequence(title))
    }
    fn cancellation(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let pipeline = Arc::clone(&self.pipeline);
        Some(Arc::new(move || pipeline.cancel()))
    }
    fn ended(&mut self) {
        self.pipeline.end();
    }
}

/// An anonymous pipe: (read end, write end).
fn pipe(capacity: u32) -> Result<(OwnedHandle, OwnedHandle)> {
    use std::os::windows::io::FromRawHandle as _;
    use windows::Win32::{Foundation::HANDLE, System::Pipes::CreatePipe};
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    // SAFETY: both destinations are valid writable slots.
    unsafe { CreatePipe(&mut read, &mut write, None, capacity) }
        .context("creating a console pipe")?;
    // SAFETY: CreatePipe succeeded, so each handle is owned here exactly once.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read.0.cast()),
            OwnedHandle::from_raw_handle(write.0.cast()),
        )
    })
}

pub(crate) struct OpenHosted {
    pub(crate) options: tty::Options,
    pub(crate) columns: u16,
    pub(crate) lines: u16,
}

pub(crate) struct HostedConsole {
    session: Arc<Mutex<Session>>,
    term: SharedTerm,
    pid: u32,
    process: OwnedHandle,
    /// The ends handed to the daemon, kept so a replacement daemon can be
    /// handed them again.
    conout: OwnedHandle,
    conin: OwnedHandle,
    input_thread: Option<JoinHandle<()>>,
}

impl HostedConsole {
    pub(crate) fn open(request: OpenHosted) -> Result<Self> {
        let OpenHosted {
            options,
            columns,
            lines,
        } = request;
        let size = TermSize::new(usize::from(columns.max(1)), usize::from(lines.max(1)));
        let term: SharedTerm = Arc::new(Mutex::new(Term::new(
            Config {
                scrolling_history: 0,
                ..Config::default()
            },
            &size,
            VoidListener,
        )));
        let (conout, output_writer) = pipe(OUTPUT_PIPE_CAPACITY)?;
        let (input_reader, conin) = pipe(0)?;
        let pipeline = Arc::new(Pipeline::default());
        spawn_daemon_writer(Arc::clone(&pipeline), output_writer)?;
        spawn_parser(Arc::clone(&pipeline), Arc::clone(&term))?;

        let (_, command) = tty::windows::command(&options).context("resolving the program")?;
        let session = Session::spawn(
            Launch {
                command,
                environment: options.env.clone(),
                directory: options
                    .working_directory
                    .as_ref()
                    .map(|path| tty::windows::normalize_working_directory(path)),
                palette: Some(grid::console_palette(options.console_palette)),
                code_page: None,
            },
            HostOutput {
                pipeline,
                term: Arc::clone(&term),
                translator: Translator::default(),
            },
        )
        .context("starting the console-server console")?;
        session.resize(columns.max(1), lines.max(1));
        let pid = session.child_pid();
        let process = session.child_handle()?;
        let session = Arc::new(Mutex::new(session));
        let input_thread = spawn_input(Arc::clone(&session), input_reader)?;
        Ok(Self {
            session,
            term,
            pid,
            process,
            conout,
            conin,
            input_thread: Some(input_thread),
        })
    }

    pub(crate) fn child_pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn handover_handles(&self) -> (&OwnedHandle, &OwnedHandle) {
        (&self.conout, &self.conin)
    }

    pub(crate) fn resize(&self, columns: u16, lines: u16) {
        let (columns, lines) = (columns.max(1), lines.max(1));
        lock(&self.term).resize(TermSize::new(usize::from(columns), usize::from(lines)));
        lock(&self.session).resize(columns, lines);
    }

    pub(crate) fn set_console_palette(&self, palette: tty::ConsolePalette) {
        lock(&self.session).set_palette(grid::console_palette(palette));
    }

    /// The exit code once the child has exited.
    pub(crate) fn exit_code(&self) -> Option<Option<i32>> {
        use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        let handle = HANDLE(self.process.as_raw_handle().cast());
        // SAFETY: the handle is a live process handle owned by `self`.
        if unsafe { WaitForSingleObject(handle, 0) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: as above; `code` is a valid destination.
        Some(
            unsafe { GetExitCodeProcess(handle, &mut code) }
                .ok()
                .map(|()| code as i32),
        )
    }
}

impl Drop for HostedConsole {
    /// Closing the console sends its clients CTRL_CLOSE_EVENT and returns;
    /// the input thread stops reading from a daemon that may never write.
    fn drop(&mut self) {
        lock(&self.session).close();
        if let Some(thread) = self.input_thread.take() {
            use windows::Win32::{Foundation::HANDLE, System::IO::CancelSynchronousIo};
            // SAFETY: the thread handle is live until `thread` is dropped.
            let _ = unsafe { CancelSynchronousIo(HANDLE(thread.as_raw_handle().cast())) };
        }
    }
}

/// Copies output to the daemon's pipe. Ends at the end of output, closing
/// the write end so the daemon reads EOF.
fn spawn_daemon_writer(pipeline: Arc<Pipeline>, writer: OwnedHandle) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("console-host-output".into())
        .spawn(move || {
            let mut file = std::fs::File::from(writer);
            while let Some(chunk) = pipeline.next(true) {
                if file.write_all(&chunk).is_err() {
                    // Nothing reads any more; keep draining so the parser
                    // and the console are never held up by it.
                    while pipeline.next(true).is_some() {}
                    return;
                }
            }
        })
        .context("starting the console output thread")
}

/// Parses output into the host's grid, then acknowledges it for queries.
fn spawn_parser(pipeline: Arc<Pipeline>, term: SharedTerm) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("console-host-parser".into())
        .spawn(move || {
            let mut parser: Processor<StdSyncHandler> = Processor::new();
            while let Some(chunk) = pipeline.next(false) {
                parser.advance(&mut *lock(&term), &chunk);
                pipeline.parsed(chunk.len());
            }
        })
        .context("starting the console parser thread")
}

/// Feeds what the daemon writes to the console's input.
fn spawn_input(session: Arc<Mutex<Session>>, reader: OwnedHandle) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("console-host-input".into())
        .spawn(move || {
            let mut file = std::fs::File::from(reader);
            let mut buffer = vec![0u8; 16 * 1024];
            loop {
                let count = match file.read(&mut buffer) {
                    Ok(0) | Err(_) => return,
                    Ok(count) => count,
                };
                let mut offset = 0;
                while offset < count {
                    match lock(&session).input(&buffer[offset..count]) {
                        Ok(written) => offset += written,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(_) => return,
                    }
                }
            }
        })
        .context("starting the console input thread")
}

#[cfg(test)]
#[path = "tests/hosted_console.rs"]
mod tests;
