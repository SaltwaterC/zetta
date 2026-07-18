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
    assert!((metrics.p50_draw_ms - 10.0).abs() < f64::EPSILON);
    assert!((metrics.p95_draw_ms - 20.0).abs() < f64::EPSILON);
    assert!((metrics.p99_draw_ms - 20.0).abs() < f64::EPSILON);
    assert!((metrics.average_latency_ms - 11.666_666).abs() < 0.001);
    assert_eq!(metrics.slow_120_hz, 2);
    assert_eq!(metrics.slow_60_hz, 1);
}

#[test]
fn performance_report_is_portable_json() {
    let report_path = env::temp_dir().join(format!(
        "zetta-performance-report-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut overlay = PerformanceOverlay::new(WindowId::from(7), 1);
    overlay.begin_report();
    overlay.sample();
    overlay
        .write_report(&report_path, Duration::from_secs(10))
        .unwrap();

    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    fs::remove_file(report_path).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["zetta_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["requested_duration_ms"], 10_000);
    assert_eq!(report["target"]["os"], std::env::consts::OS);
    assert_eq!(report["target"]["architecture"], std::env::consts::ARCH);
    assert_eq!(report["workload"]["producer_hz"], 240);
    assert_eq!(report["workload"]["rows"], 34);
    assert_eq!(report["summary"]["frame_count"], 0);
    assert!(report["samples"].is_array());
    assert_eq!(report["samples"].as_array().unwrap().len(), 1);
}

#[test]
fn performance_metrics_handle_an_idle_sample() {
    assert_eq!(
        PerformanceMetrics::from_timings(&[], Duration::from_secs(1)),
        PerformanceMetrics::default()
    );
}
