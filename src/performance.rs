use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PerformanceMetrics {
    pub(crate) draw_fps: f64,
    pub(crate) average_draw_ms: f64,
    pub(crate) p95_draw_ms: f64,
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
        let p95_index = ((draw_durations.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(draw_durations.len() - 1);
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
            p95_draw_ms: draw_durations[p95_index].as_secs_f64() * 1_000.0,
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

pub(crate) struct PerformanceOverlay {
    pub(crate) collector: FrameTimingCollector,
    pub(crate) window_id: WindowId,
    pub(crate) sampled_at: Instant,
    pub(crate) metrics: PerformanceMetrics,
    pub(crate) generation: u64,
}

impl PerformanceOverlay {
    pub(crate) fn new(window_id: WindowId, generation: u64) -> Self {
        Self {
            collector: FrameTimingCollector::new(),
            window_id,
            sampled_at: Instant::now(),
            metrics: PerformanceMetrics::default(),
            generation,
        }
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
    }
}

#[cfg(test)]
#[path = "tests/performance.rs"]
mod tests;
