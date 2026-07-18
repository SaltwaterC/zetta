use super::*;

#[test]
fn performance_metrics_report_fps_percentiles_and_slow_frames() {
    let draw_start = Instant::now();
    let timing = |milliseconds| FrameTiming {
        window_id: WindowId::from(1),
        dirty_at: Some(draw_start),
        invalidations: 1,
        draw_start,
        draw_end: draw_start + Duration::from_millis(milliseconds),
    };
    let metrics = PerformanceMetrics::from_timings(
        &[timing(5), timing(10), timing(20)],
        Duration::from_secs(1),
    );

    assert!((metrics.draw_fps - 3.0).abs() < f64::EPSILON);
    assert!((metrics.average_draw_ms - 11.666_666).abs() < 0.001);
    assert!((metrics.p95_draw_ms - 20.0).abs() < f64::EPSILON);
    assert!((metrics.average_latency_ms - 11.666_666).abs() < 0.001);
    assert_eq!(metrics.slow_120_hz, 2);
    assert_eq!(metrics.slow_60_hz, 1);
}

#[test]
fn performance_metrics_handle_an_idle_sample() {
    assert_eq!(
        PerformanceMetrics::from_timings(&[], Duration::from_secs(1)),
        PerformanceMetrics::default()
    );
}
