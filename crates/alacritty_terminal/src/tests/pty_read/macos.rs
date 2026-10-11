use super::*;

#[test]
fn an_empty_read_does_not_spin() {
    let mut retry = ReadRetry::default();
    assert!(!retry.should_retry(0));
    assert!(retry.started.is_none());
}

#[test]
fn short_reads_at_a_chunk_boundary_can_bridge_the_next_refill() {
    let mut retry = ReadRetry::default();
    assert!(retry.should_retry(1));
    assert!(retry.should_retry(1023));
}

#[test]
fn a_saturated_burst_retries_only_a_bounded_number_of_reads() {
    let mut retry = ReadRetry::default();
    let now = Instant::now();
    for _ in 0..MAX_RETRIES {
        assert!(retry.retry_at(BULK_THRESHOLD, now));
    }
    assert!(!retry.retry_at(BULK_THRESHOLD, now));

    retry.on_read(1);
    assert!(retry.retry_at(BULK_THRESHOLD, now));
}

#[test]
fn refills_do_not_restart_the_batch_deadline() {
    let mut retry = ReadRetry::default();
    let now = Instant::now();
    assert!(retry.retry_at(BULK_THRESHOLD, now));
    retry.on_read(1);
    assert!(retry.retry_at(BULK_THRESHOLD, now + RETRY_BUDGET - Duration::from_nanos(1)));
    retry.on_read(1);
    assert!(!retry.retry_at(BULK_THRESHOLD, now + RETRY_BUDGET));
    retry.on_read(1);
    assert!(!retry.retry_at(BULK_THRESHOLD, now + RETRY_BUDGET * 2));
}

#[test]
fn consecutive_small_chunks_build_a_bulk_stream_but_idle_resets_it() {
    let now = Instant::now();
    let mut retry = ReadRetry::default();
    retry.begin_at(now);
    assert_eq!(retry.retry_limit(1), SMALL_RETRIES);
    for part in 0..8 {
        retry.on_read(1024);
        retry.delivered = Some(now);
        retry.begin_at(now + Duration::from_micros(part));
    }
    assert_eq!(retry.retry_limit(1), MAX_RETRIES);
    retry.begin_at(now + BURST_GAP);
    assert_eq!(retry.retry_limit(1), SMALL_RETRIES);
}

#[test]
fn refills_do_not_reset_the_clock_check_interval() {
    let mut retry = ReadRetry::default();
    assert!(retry.should_retry(1));
    for remaining in (0..CLOCK_INTERVAL - 1).rev() {
        retry.on_read(1);
        assert!(retry.should_retry(1));
        assert_eq!(retry.until_check, remaining);
    }
    retry.started = Some(Instant::now() - RETRY_BUDGET);
    retry.on_read(1);
    assert!(!retry.should_retry(1));
}

#[test]
fn small_bursts_use_the_short_retry_budget() {
    let mut retry = ReadRetry::default();
    let now = Instant::now();
    for _ in 0..SMALL_RETRIES {
        assert!(retry.retry_at(1, now));
    }
    assert!(!retry.retry_at(1, now));
}
