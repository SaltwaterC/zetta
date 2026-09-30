//! The console `zosh-server.exe` gives a relay: its size, a raw VT mode, and
//! a resize check.
//!
//! `zosh-server.exe` runs the relay inside a ConPTY, so the relay's stdio is a
//! console rather than a terminal. Raw mode is the console's VT modes: input
//! arrives as the bytes the viewer typed rather than as edited lines, and
//! output is interpreted as VT rather than printed. There is no `SIGWINCH`;
//! the console's size is polled instead.

use std::{
    fs::File,
    io,
    mem::ManuallyDrop,
    os::windows::io::{AsRawHandle as _, FromRawHandle as _},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::Result;
use windows::Win32::{
    Foundation::HANDLE,
    System::Console::{
        CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_ECHO_INPUT,
        ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleCP,
        GetConsoleMode, GetConsoleOutputCP, GetConsoleScreenBufferInfo, SetConsoleCP,
        SetConsoleMode, SetConsoleOutputCP,
    },
};

/// How often the console's size is checked for a change.
const RESIZE_POLL: Duration = Duration::from_millis(50);

const UTF8_CODE_PAGE: u32 = 65001;

fn stdin_handle() -> HANDLE {
    HANDLE(io::stdin().as_raw_handle())
}

fn stdout_handle() -> HANDLE {
    HANDLE(io::stdout().as_raw_handle())
}

/// The console's visible size in columns and lines, if it is a console.
pub(super) fn size() -> Option<(u16, u16)> {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: the handle is this process's own standard output and `info` is
    // the structure the call fills in.
    unsafe { GetConsoleScreenBufferInfo(stdout_handle(), &mut info) }.ok()?;
    let columns = u16::try_from(info.srWindow.Right - info.srWindow.Left + 1).ok()?;
    let lines = u16::try_from(info.srWindow.Bottom - info.srWindow.Top + 1).ok()?;
    (columns != 0 && lines != 0).then_some((columns, lines))
}

/// Where the pane's output is written.
///
/// Rust's console `Stdout` insists on valid UTF-8 and fails the write
/// otherwise, which would end the relay over one stray byte of pane output.
/// Writing the handle as a file passes the bytes to the console as they are,
/// decoded in the UTF-8 code page [`RawMode::enter`] selects.
pub(super) fn output() -> ConsoleOutput {
    // SAFETY: the handle is this process's standard output, which stays open
    // for the life of the process; `ManuallyDrop` keeps this from closing it.
    ConsoleOutput(ManuallyDrop::new(unsafe {
        File::from_raw_handle(io::stdout().as_raw_handle())
    }))
}

pub(super) struct ConsoleOutput(ManuallyDrop<File>);

impl io::Write for ConsoleOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// The console modes and code pages this process found, restored when it
/// ends. A relay whose stdin is not a console — a test, or a pipe — has
/// nothing to change and nothing to restore.
pub(super) struct RawMode {
    previous: Option<Previous>,
}

struct Previous {
    input: CONSOLE_MODE,
    output: Option<CONSOLE_MODE>,
    input_code_page: u32,
    output_code_page: u32,
}

impl RawMode {
    pub(super) fn enter() -> io::Result<Self> {
        let mut input = CONSOLE_MODE(0);
        // SAFETY: the handle is this process's own standard input and `input`
        // is writable.
        if unsafe { GetConsoleMode(stdin_handle(), &mut input) }.is_err() {
            return Ok(Self { previous: None });
        }
        let mut output = CONSOLE_MODE(0);
        // SAFETY: as above, for standard output.
        let output = unsafe { GetConsoleMode(stdout_handle(), &mut output) }
            .ok()
            .map(|()| output);
        // SAFETY: these only read the console's code pages.
        let (input_code_page, output_code_page) = unsafe { (GetConsoleCP(), GetConsoleOutputCP()) };
        let previous = Previous {
            input,
            output,
            input_code_page,
            output_code_page,
        };

        let raw_input = (input & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        // SAFETY: the handle is this process's standard input and the mode is
        // the one it reported, with documented flags changed.
        unsafe { SetConsoleMode(stdin_handle(), raw_input) }.map_err(io::Error::other)?;
        if let Some(output) = output {
            let vt_output = output
                | ENABLE_PROCESSED_OUTPUT
                | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                | DISABLE_NEWLINE_AUTO_RETURN;
            // SAFETY: as above, for standard output.
            unsafe { SetConsoleMode(stdout_handle(), vt_output) }.map_err(io::Error::other)?;
        }
        // SAFETY: selecting a code page changes only how this console decodes
        // and encodes text.
        unsafe {
            let _ = SetConsoleCP(UTF8_CODE_PAGE);
            let _ = SetConsoleOutputCP(UTF8_CODE_PAGE);
        }
        Ok(Self {
            previous: Some(previous),
        })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let Some(previous) = self.previous.take() else {
            return;
        };
        // SAFETY: each value is one this process read from the same console.
        unsafe {
            let _ = SetConsoleMode(stdin_handle(), previous.input);
            if let Some(output) = previous.output {
                let _ = SetConsoleMode(stdout_handle(), output);
            }
            let _ = SetConsoleCP(previous.input_code_page);
            let _ = SetConsoleOutputCP(previous.output_code_page);
        }
    }
}

/// Sets `resized` whenever the console's size changes, until `ending`.
pub(super) fn watch_resizes(resized: Arc<AtomicBool>, ending: Arc<AtomicBool>) -> Result<()> {
    thread::Builder::new()
        .name("zmux-relay-resize".to_owned())
        .spawn(move || {
            let mut last = size();
            while !ending.load(Ordering::Acquire) {
                thread::sleep(RESIZE_POLL);
                let current = size();
                if current != last {
                    last = current;
                    resized.store(true, Ordering::SeqCst);
                }
            }
        })?;
    Ok(())
}
