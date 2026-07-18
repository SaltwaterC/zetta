use super::*;

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
}

pub(crate) type PerformanceReportStatus = Arc<Mutex<Option<std::result::Result<(), String>>>>;

#[derive(Serialize)]
struct PerformanceReportSample {
    elapsed_ms: u64,
    frame_count: usize,
    metrics: PerformanceMetrics,
}

struct PerformanceReportCapture {
    started_at: Instant,
    started_unix_ms: u128,
    timings: Vec<FrameTiming>,
    samples: Vec<PerformanceReportSample>,
}

#[derive(Serialize)]
struct PerformanceReportTarget {
    os: &'static str,
    architecture: &'static str,
}

#[derive(Serialize)]
struct PerformanceReportWorkload {
    producer_hz: u16,
    rows: u8,
}

#[derive(Serialize)]
struct PerformanceReportSummary {
    frame_count: usize,
    metrics: PerformanceMetrics,
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
}

impl PerformanceOverlay {
    pub(crate) fn new(window_id: WindowId, generation: u64) -> Self {
        Self {
            collector: FrameTimingCollector::new(),
            window_id,
            sampled_at: Instant::now(),
            metrics: PerformanceMetrics::default(),
            generation,
            report: None,
        }
    }

    pub(crate) fn begin_report(&mut self) {
        self.collector = FrameTimingCollector::new();
        self.sampled_at = Instant::now();
        self.report = Some(PerformanceReportCapture {
            started_at: self.sampled_at,
            started_unix_ms: SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            timings: Vec::new(),
            samples: Vec::new(),
        });
    }

    pub(crate) fn sample(&mut self) {
        let now = Instant::now();
        let timings = self
            .collector
            .collect_unseen()
            .into_iter()
            .filter(|timing| timing.window_id == self.window_id)
            .collect::<Vec<_>>();
        self.metrics = PerformanceMetrics::from_timings(&timings, now - self.sampled_at);
        self.sampled_at = now;
        if let Some(report) = self.report.as_mut() {
            report.samples.push(PerformanceReportSample {
                elapsed_ms: duration_millis(now - report.started_at),
                frame_count: timings.len(),
                metrics: self.metrics,
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
        let capture = self
            .report
            .take()
            .context("performance report was not started")?;
        let elapsed = capture.started_at.elapsed();
        let report = PerformanceReport {
            schema_version: 1,
            zetta_version: env!("CARGO_PKG_VERSION"),
            build_profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            target: PerformanceReportTarget {
                os: std::env::consts::OS,
                architecture: std::env::consts::ARCH,
            },
            workload: PerformanceReportWorkload {
                producer_hz: 240,
                rows: 34,
            },
            started_unix_ms: capture.started_unix_ms,
            requested_duration_ms: duration_millis(requested_duration),
            elapsed_ms: duration_millis(elapsed),
            sample_interval_ms: duration_millis(PERFORMANCE_SAMPLE_INTERVAL),
            summary: PerformanceReportSummary {
                frame_count: capture.timings.len(),
                metrics: PerformanceMetrics::from_timings(&capture.timings, elapsed),
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

#[cfg(test)]
#[path = "tests/performance.rs"]
mod tests;
