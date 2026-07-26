use std::{
    io,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};

pub(crate) const DEFAULT_OUTPUT_BENCHMARK_MIB: usize = 10;
pub(crate) const MIB_BYTES: usize = 1024 * 1024;
const OUTPUT_BENCHMARK_WRITE_BYTES: usize = 128 * 1024;
const OUTPUT_BENCHMARK_LINE_BYTES: usize = 80;
const OUTPUT_BENCHMARK_TEXT: &[u8] =
    b"0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputBenchmarkResult {
    pub(crate) bytes: usize,
    pub(crate) elapsed: Duration,
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

pub(crate) fn write_output_benchmark(
    output: &mut impl io::Write,
    bytes: usize,
) -> io::Result<OutputBenchmarkResult> {
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

    let started_at = Instant::now();
    let mut remaining = bytes;
    let mut payload_index = 0;
    while remaining > 0 {
        let payload = &payloads[payload_index];
        output.write_all(&payload[..remaining.min(payload.len())])?;
        remaining = remaining.saturating_sub(payload.len());
        payload_index = (payload_index + 1) % payloads.len();
    }
    output.flush()?;
    Ok(OutputBenchmarkResult {
        bytes,
        elapsed: started_at.elapsed(),
    })
}

pub(crate) fn run_output_benchmark(size_mib: usize) -> Result<()> {
    let bytes = size_mib
        .checked_mul(MIB_BYTES)
        .context("benchmark output size is too large")?;
    let stdout = io::stdout();
    let result = write_output_benchmark(&mut stdout.lock(), bytes)
        .context("writing the output benchmark payload")?;
    eprintln!(
        "Zetta output benchmark: {:.3} MiB in {:.3} s ({:.3} MiB/s)",
        result.bytes as f64 / (1024.0 * 1024.0),
        result.elapsed.as_secs_f64(),
        result.throughput_mib_per_second(),
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/output_benchmark.rs"]
mod tests;
