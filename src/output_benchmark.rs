use std::{
    io,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};

pub(crate) const OUTPUT_BENCHMARK_BYTES: usize = 10 * 1024 * 1024;
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

fn output_benchmark_payload() -> Vec<u8> {
    (0..OUTPUT_BENCHMARK_BYTES)
        .map(|index| {
            let column = index % OUTPUT_BENCHMARK_LINE_BYTES;
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
) -> io::Result<OutputBenchmarkResult> {
    let payload = output_benchmark_payload();
    let started_at = Instant::now();
    for chunk in payload.chunks(OUTPUT_BENCHMARK_WRITE_BYTES) {
        output.write_all(chunk)?;
    }
    output.flush()?;
    Ok(OutputBenchmarkResult {
        bytes: payload.len(),
        elapsed: started_at.elapsed(),
    })
}

pub(crate) fn run_output_benchmark() -> Result<()> {
    let stdout = io::stdout();
    let result = write_output_benchmark(&mut stdout.lock())
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
