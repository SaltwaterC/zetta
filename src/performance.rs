use super::*;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
struct PerformanceCpuMetrics {
    process_cpu_time_ms: u64,
    average_core_utilization_percent: f64,
    average_machine_utilization_percent: f64,
}

impl PerformanceCpuMetrics {
    fn from_interval(
        process_cpu_time_ms: u64,
        elapsed: Duration,
        logical_cpu_count: usize,
    ) -> Self {
        if elapsed.is_zero() || logical_cpu_count == 0 {
            return Self::default();
        }
        let average_core_utilization_percent =
            process_cpu_time_ms as f64 / elapsed.as_secs_f64() / 10.0;
        Self {
            process_cpu_time_ms,
            average_core_utilization_percent,
            average_machine_utilization_percent: average_core_utilization_percent
                / logical_cpu_count as f64,
        }
    }
}

struct ProcessCpuClock {
    system: System,
    pid: sysinfo::Pid,
}

impl ProcessCpuClock {
    fn new() -> Option<Self> {
        Some(Self {
            system: System::new(),
            pid: sysinfo::get_current_pid().ok()?,
        })
    }

    fn read_millis(&mut self) -> Option<u64> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
        self.system
            .process(self.pid)
            .map(|process| process.accumulated_cpu_time())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub(crate) struct PerformanceMetrics {
    pub(crate) draw_fps: f64,
    pub(crate) average_draw_ms: f64,
    pub(crate) p50_draw_ms: f64,
    pub(crate) p95_draw_ms: f64,
    pub(crate) p99_draw_ms: f64,
    pub(crate) average_latency_ms: f64,
    pub(crate) slow_120_hz: usize,
    pub(crate) slow_60_hz: usize,
}

impl PerformanceMetrics {
    pub(crate) fn from_timings(timings: &[FrameTiming], elapsed: Duration) -> Self {
        if timings.is_empty() || elapsed.is_zero() {
            return Self::default();
        }

        let mut draw_durations = timings
            .iter()
            .map(FrameTiming::draw_duration)
            .collect::<Vec<_>>();
        draw_durations.sort_unstable();
        let total_draw = draw_durations.iter().sum::<Duration>();
        let percentile = |value: f64| {
            let index = ((draw_durations.len() as f64 * value).ceil() as usize)
                .saturating_sub(1)
                .min(draw_durations.len() - 1);
            draw_durations[index].as_secs_f64() * 1_000.0
        };
        let latencies = timings
            .iter()
            .filter_map(FrameTiming::dirty_to_draw_duration)
            .collect::<Vec<_>>();
        let average_latency_ms = if latencies.is_empty() {
            0.0
        } else {
            latencies.iter().sum::<Duration>().as_secs_f64() * 1_000.0 / latencies.len() as f64
        };

        Self {
            draw_fps: timings.len() as f64 / elapsed.as_secs_f64(),
            average_draw_ms: total_draw.as_secs_f64() * 1_000.0 / timings.len() as f64,
            p50_draw_ms: percentile(0.50),
            p95_draw_ms: percentile(0.95),
            p99_draw_ms: percentile(0.99),
            average_latency_ms,
            slow_120_hz: draw_durations
                .iter()
                .filter(|duration| **duration > FRAME_BUDGET_120_HZ)
                .count(),
            slow_60_hz: draw_durations
                .iter()
                .filter(|duration| **duration > FRAME_BUDGET_60_HZ)
                .count(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PerformanceReportOptions {
    pub(crate) path: PathBuf,
    pub(crate) duration: Duration,
    pub(crate) workload: PerformanceWorkload,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PerformanceWorkload {
    #[default]
    Standard,
    CheckerboardBackground,
    SparseUpdates,
}

impl PerformanceWorkload {
    pub(crate) fn producer_hz(self) -> u16 {
        match self {
            Self::Standard | Self::CheckerboardBackground => 240,
            Self::SparseUpdates => 40,
        }
    }
}

pub(crate) type PerformanceReportStatus = Arc<Mutex<Option<std::result::Result<(), String>>>>;

#[derive(Serialize)]
struct PerformanceReportSample {
    elapsed_ms: u64,
    frame_count: usize,
    metrics: PerformanceMetrics,
    cpu: Option<PerformanceCpuMetrics>,
}

struct PerformanceReportCapture {
    started_at: Instant,
    started_unix_ms: u128,
    cpu_clock: Option<ProcessCpuClock>,
    started_process_cpu_ms: Option<u64>,
    sampled_process_cpu_ms: Option<u64>,
    timings: Vec<FrameTiming>,
    samples: Vec<PerformanceReportSample>,
}

#[derive(Serialize)]
struct PerformanceReportTarget {
    os: &'static str,
    architecture: &'static str,
    logical_cpu_count: usize,
}

#[derive(Serialize)]
struct PerformanceReportWorkload {
    pattern: PerformanceWorkload,
    producer_hz: u16,
    rows: u8,
    pane_count: usize,
    minimized_pane_count: usize,
}

#[derive(Serialize)]
struct PerformanceReportSummary {
    frame_count: usize,
    metrics: PerformanceMetrics,
    cpu: Option<PerformanceCpuMetrics>,
}

#[derive(Serialize)]
struct PerformanceReport {
    schema_version: u8,
    zetta_version: &'static str,
    build_profile: &'static str,
    target: PerformanceReportTarget,
    workload: PerformanceReportWorkload,
    started_unix_ms: u128,
    requested_duration_ms: u64,
    elapsed_ms: u64,
    sample_interval_ms: u64,
    summary: PerformanceReportSummary,
    samples: Vec<PerformanceReportSample>,
}

pub(crate) struct PerformanceOverlay {
    pub(crate) collector: FrameTimingCollector,
    pub(crate) window_id: WindowId,
    pub(crate) sampled_at: Instant,
    pub(crate) metrics: PerformanceMetrics,
    pub(crate) generation: u64,
    report: Option<PerformanceReportCapture>,
    pane_count: usize,
    minimized_pane_count: usize,
    pub(crate) workload: PerformanceWorkload,
}

impl PerformanceOverlay {
    pub(crate) fn new(
        window_id: WindowId,
        generation: u64,
        pane_count: usize,
        minimized_pane_count: usize,
    ) -> Self {
        Self {
            collector: FrameTimingCollector::new(),
            window_id,
            sampled_at: Instant::now(),
            metrics: PerformanceMetrics::default(),
            generation,
            report: None,
            pane_count,
            minimized_pane_count,
            workload: PerformanceWorkload::default(),
        }
    }

    pub(crate) fn begin_report(&mut self) {
        self.collector = FrameTimingCollector::new();
        let mut cpu_clock = ProcessCpuClock::new();
        let started_process_cpu_ms = cpu_clock.as_mut().and_then(ProcessCpuClock::read_millis);
        self.sampled_at = Instant::now();
        self.report = Some(PerformanceReportCapture {
            started_at: self.sampled_at,
            started_unix_ms: SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            cpu_clock,
            started_process_cpu_ms,
            sampled_process_cpu_ms: started_process_cpu_ms,
            timings: Vec::new(),
            samples: Vec::new(),
        });
    }

    pub(crate) fn sample(&mut self) {
        let process_cpu_ms = self
            .report
            .as_mut()
            .and_then(|report| report.cpu_clock.as_mut())
            .and_then(ProcessCpuClock::read_millis);
        let now = Instant::now();
        let sample_elapsed = now - self.sampled_at;
        let timings = self
            .collector
            .collect_unseen()
            .into_iter()
            .filter(|timing| timing.window_id == self.window_id)
            .collect::<Vec<_>>();
        self.metrics = PerformanceMetrics::from_timings(&timings, sample_elapsed);
        self.sampled_at = now;
        if let Some(report) = self.report.as_mut() {
            let cpu =
                report
                    .sampled_process_cpu_ms
                    .zip(process_cpu_ms)
                    .map(|(previous, current)| {
                        PerformanceCpuMetrics::from_interval(
                            current.saturating_sub(previous),
                            sample_elapsed,
                            logical_cpu_count(),
                        )
                    });
            report.sampled_process_cpu_ms = process_cpu_ms;
            report.samples.push(PerformanceReportSample {
                elapsed_ms: duration_millis(now - report.started_at),
                frame_count: timings.len(),
                metrics: self.metrics,
                cpu,
            });
            report.timings.extend(timings);
        }
    }

    pub(crate) fn write_report(&mut self, path: &Path, requested_duration: Duration) -> Result<()> {
        let needs_final_sample = self
            .report
            .as_ref()
            .is_some_and(|report| report.samples.is_empty())
            || self.sampled_at.elapsed() >= Duration::from_millis(10);
        if needs_final_sample {
            self.sample();
        }
        let mut capture = self
            .report
            .take()
            .context("performance report was not started")?;
        let finished_process_cpu_ms = capture
            .cpu_clock
            .as_mut()
            .and_then(ProcessCpuClock::read_millis);
        let elapsed = capture.started_at.elapsed();
        let logical_cpu_count = logical_cpu_count();
        let summary_cpu = capture
            .started_process_cpu_ms
            .zip(finished_process_cpu_ms)
            .map(|(started, finished)| {
                PerformanceCpuMetrics::from_interval(
                    finished.saturating_sub(started),
                    elapsed,
                    logical_cpu_count,
                )
            });
        let report = PerformanceReport {
            schema_version: 3,
            zetta_version: env!("CARGO_PKG_VERSION"),
            build_profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            target: PerformanceReportTarget {
                os: std::env::consts::OS,
                architecture: std::env::consts::ARCH,
                logical_cpu_count,
            },
            workload: PerformanceReportWorkload {
                pattern: self.workload,
                producer_hz: self.workload.producer_hz(),
                rows: 34,
                pane_count: self.pane_count,
                minimized_pane_count: self.minimized_pane_count,
            },
            started_unix_ms: capture.started_unix_ms,
            requested_duration_ms: duration_millis(requested_duration),
            elapsed_ms: duration_millis(elapsed),
            sample_interval_ms: duration_millis(PERFORMANCE_SAMPLE_INTERVAL),
            summary: PerformanceReportSummary {
                frame_count: capture.timings.len(),
                metrics: PerformanceMetrics::from_timings(&capture.timings, elapsed),
                cpu: summary_cpu,
            },
            samples: capture.samples,
        };
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating report directory {}", parent.display()))?;
        }
        let json = serde_json::to_vec_pretty(&report).context("serializing performance report")?;
        fs::write(path, json)
            .with_context(|| format!("writing performance report {}", path.display()))
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn logical_cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

#[cfg(test)]
#[path = "tests/performance.rs"]
mod tests;
