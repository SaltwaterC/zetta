use std::{
    io::{self, Write as _},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};

pub(crate) const DEFAULT_OUTPUT_BENCHMARK_MIB: usize = 10;
pub(crate) const MIB_BYTES: usize = 1024 * 1024;
const OUTPUT_BENCHMARK_WRITE_BYTES: usize = 128 * 1024;
const OUTPUT_BENCHMARK_LINE_BYTES: usize = 80;
const OUTPUT_BENCHMARK_TEXT: &[u8] =
    b"0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputBenchmarkType {
    RepeatedLines,
    UniqueLines,
}

impl OutputBenchmarkType {
    fn name(self) -> &'static str {
        match self {
            Self::RepeatedLines => "repeated lines",
            Self::UniqueLines => "unique lines",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalSize {
    columns: usize,
    rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputBenchmarkResult {
    pub(crate) bytes: usize,
    pub(crate) elapsed: Duration,
    pub(crate) output_type: OutputBenchmarkType,
}

impl OutputBenchmarkResult {
    fn throughput_mib_per_second(self) -> f64 {
        self.bytes as f64 / (1024.0 * 1024.0) / self.elapsed.as_secs_f64()
    }
}

fn output_benchmark_payload(start_column: usize, bytes: usize) -> Vec<u8> {
    (0..bytes)
        .map(|index| {
            let column = (start_column + index) % OUTPUT_BENCHMARK_LINE_BYTES;
            if column == OUTPUT_BENCHMARK_LINE_BYTES - 1 {
                b'\n'
            } else {
                OUTPUT_BENCHMARK_TEXT[column % OUTPUT_BENCHMARK_TEXT.len()]
            }
        })
        .collect()
}

fn unique_output_benchmark_payload(bytes: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(bytes);
    let mut line_index = 0u64;

    while payload.len() < bytes {
        let mut line = [b' '; OUTPUT_BENCHMARK_LINE_BYTES];
        line[..5].copy_from_slice(b"line ");
        for (offset, byte) in line[5..21].iter_mut().enumerate() {
            let shift = 4 * (15 - offset);
            *byte = HEX[((line_index >> shift) & 0xf) as usize];
        }
        line[21] = b' ';
        for (offset, byte) in line[22..OUTPUT_BENCHMARK_LINE_BYTES - 1]
            .iter_mut()
            .enumerate()
        {
            *byte =
                OUTPUT_BENCHMARK_TEXT[(line_index as usize + offset) % OUTPUT_BENCHMARK_TEXT.len()];
        }
        line[OUTPUT_BENCHMARK_LINE_BYTES - 1] = b'\n';

        let remaining = bytes - payload.len();
        payload.extend_from_slice(&line[..remaining.min(line.len())]);
        line_index += 1;
    }

    payload
}

fn repeated_output_benchmark_payloads() -> Vec<Vec<u8>> {
    let mut payloads = Vec::new();
    let mut start_column = 0;
    loop {
        payloads.push(output_benchmark_payload(
            start_column,
            OUTPUT_BENCHMARK_WRITE_BYTES,
        ));
        start_column = (start_column + OUTPUT_BENCHMARK_WRITE_BYTES) % OUTPUT_BENCHMARK_LINE_BYTES;
        if start_column == 0 {
            break;
        }
    }
    payloads
}

pub(crate) fn write_output_benchmark(
    output: &mut impl io::Write,
    bytes: usize,
    output_type: OutputBenchmarkType,
) -> io::Result<OutputBenchmarkResult> {
    let payloads = match output_type {
        OutputBenchmarkType::RepeatedLines => repeated_output_benchmark_payloads(),
        OutputBenchmarkType::UniqueLines => vec![unique_output_benchmark_payload(bytes)],
    };

    let started_at = Instant::now();
    let mut remaining = bytes;
    let mut payload_index = 0;
    let mut payload_offset = 0;
    while remaining > 0 {
        let payload = &payloads[payload_index];
        let written = remaining
            .min(payload.len() - payload_offset)
            .min(OUTPUT_BENCHMARK_WRITE_BYTES);
        output.write_all(&payload[payload_offset..payload_offset + written])?;
        remaining -= written;
        payload_offset += written;
        if payload_offset == payload.len() {
            payload_index = (payload_index + 1) % payloads.len();
            payload_offset = 0;
        }
    }
    output.flush()?;
    Ok(OutputBenchmarkResult {
        bytes,
        elapsed: started_at.elapsed(),
        output_type,
    })
}

#[cfg(unix)]
fn terminal_size_from_fd(fd: std::os::fd::RawFd) -> Option<TerminalSize> {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, size.as_mut_ptr()) } != 0 {
        return None;
    }
    let size = unsafe { size.assume_init() };
    let columns = usize::from(size.ws_col);
    let rows = usize::from(size.ws_row);
    (columns > 0 && rows > 0).then_some(TerminalSize { columns, rows })
}

#[cfg(unix)]
fn current_terminal_size() -> Option<TerminalSize> {
    use std::os::fd::AsRawFd;

    terminal_size_from_fd(libc::STDOUT_FILENO)
        .or_else(|| terminal_size_from_fd(libc::STDERR_FILENO))
        .or_else(|| {
            let terminal = std::fs::File::open("/dev/tty").ok()?;
            terminal_size_from_fd(terminal.as_raw_fd())
        })
}

#[cfg(windows)]
fn terminal_size_from_handle(handle: windows::Win32::Foundation::HANDLE) -> Option<TerminalSize> {
    use windows::Win32::System::Console::{CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo};

    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    unsafe { GetConsoleScreenBufferInfo(handle, &mut info) }.ok()?;
    let columns = usize::try_from(info.srWindow.Right - info.srWindow.Left + 1).ok()?;
    let rows = usize::try_from(info.srWindow.Bottom - info.srWindow.Top + 1).ok()?;
    (columns > 0 && rows > 0).then_some(TerminalSize { columns, rows })
}

#[cfg(windows)]
fn current_terminal_size() -> Option<TerminalSize> {
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};

    unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
        .ok()
        .and_then(terminal_size_from_handle)
        .or_else(|| {
            unsafe { GetStdHandle(STD_ERROR_HANDLE) }
                .ok()
                .and_then(terminal_size_from_handle)
        })
}

fn terminal_size_summary(terminal_size: Option<TerminalSize>) -> String {
    terminal_size.map_or_else(
        || "terminal size unavailable".to_owned(),
        |size| format!("{} columns x {} rows", size.columns, size.rows),
    )
}

fn terminal_size_json(terminal_size: Option<TerminalSize>) -> String {
    let (columns, rows) =
        terminal_size.map_or((None, None), |size| (Some(size.columns), Some(size.rows)));
    serde_json::json!({ "columns": columns, "rows": rows }).to_string()
}

pub(crate) fn print_terminal_size(json: bool) {
    let terminal_size = current_terminal_size();
    if json {
        println!("{}", terminal_size_json(terminal_size));
    } else {
        println!("{}", terminal_size_summary(terminal_size));
    }
}

/// Emit xterm's character-cell resize request for the terminal attached to
/// standard output. Zetta intercepts this request for the pane that produced
/// it, on every supported PTY backend including Windows ConPTY.
pub(crate) fn request_terminal_resize(columns: Option<usize>, rows: Option<usize>) -> Result<()> {
    let current = current_terminal_size().context("terminal size is unavailable")?;
    let columns = columns.unwrap_or(current.columns);
    let rows = rows.unwrap_or(current.rows);
    anyhow::ensure!(
        columns > 0 && rows > 0,
        "terminal dimensions must be positive"
    );
    anyhow::ensure!(
        columns <= usize::from(u16::MAX) && rows <= usize::from(u16::MAX),
        "terminal dimensions must not exceed {}",
        u16::MAX
    );

    let mut stdout = io::stdout().lock();
    write!(stdout, "{}", terminal_resize_sequence(columns, rows))?;
    stdout.flush()?;
    Ok(())
}

fn terminal_resize_sequence(columns: usize, rows: usize) -> String {
    format!("\x1b[8;{rows};{columns}t")
}

pub(crate) fn run_output_benchmark(
    size_mib: usize,
    output_type: OutputBenchmarkType,
) -> Result<()> {
    let bytes = size_mib
        .checked_mul(MIB_BYTES)
        .context("benchmark output size is too large")?;
    let stdout = io::stdout();
    let terminal_size = current_terminal_size();
    let result = write_output_benchmark(&mut stdout.lock(), bytes, output_type)
        .context("writing the output benchmark payload")?;
    eprintln!(
        "Zetta output benchmark ({}; {}): {:.3} MiB in {:.3} s ({:.3} MiB/s)",
        result.output_type.name(),
        terminal_size_summary(terminal_size),
        result.bytes as f64 / (1024.0 * 1024.0),
        result.elapsed.as_secs_f64(),
        result.throughput_mib_per_second(),
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/output_benchmark.rs"]
mod tests;
