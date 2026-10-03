use super::*;

#[test]
fn an_empty_shared_set_reports_no_input() {
    // The attribution a pane's exit carries. With no viewers there is nobody to
    // have typed, and an exclusive holder's own keystrokes are the truth for it
    // rather than something the daemon can see.
    assert!(!shared_input_sent(&Attachment::Shared(Vec::new())));
    assert!(!shared_input_sent(&Attachment::Exclusive(7)));
    assert!(!shared_input_sent(&Attachment::None));
    assert!(!shared_input_sent(&Attachment::Revoking { holder: 7 }));
}

fn viewer() -> (SharedClient, crate::transport::Stream) {
    let (daemon_side, client_side) = crate::transport::Stream::pair().unwrap();
    let relay = super::super::attachment::spawn_relay(
        &crate::transport::Connection::new(daemon_side),
        1,
        2,
        std::process::id(),
        std::sync::Weak::new(),
    )
    .unwrap();
    let client = SharedClient {
        process_id: std::process::id(),
        client_id: ClientId::new("viewer"),
        attachment: 1,
        stream_only: true,
        relaying_for: None,
        relay,
        written_seen: 0,
        wrote_at: Instant::now(),
        size: None,
        input_sent: false,
    };
    (client, client_side)
}

/// Every eviction of an idle Zosh relay: the pane had been quiet for longer
/// than the stall timeout, printed again, and the drain looked before the
/// relay thread had written the new frame.
#[cfg(unix)]
#[test]
fn an_idle_viewer_is_not_stalled_when_its_pane_prints_again() {
    let (mut client, _peer) = viewer();
    let start = Instant::now();
    // Quiet for well past the timeout: nothing queued, nothing written.
    assert!(!viewer_has_stalled(
        &mut client,
        0,
        start + RELAY_STALL_TIMEOUT * 3
    ));
    // Output arrives and has not been written yet.
    assert!(!viewer_has_stalled(
        &mut client,
        46,
        start + RELAY_STALL_TIMEOUT * 3 + Duration::from_millis(1)
    ));
}

#[cfg(unix)]
#[test]
fn a_backlog_left_unserved_for_the_timeout_is_a_stall() {
    let (mut client, _peer) = viewer();
    let start = Instant::now();
    assert!(!viewer_has_stalled(&mut client, 0, start));
    assert!(!viewer_has_stalled(
        &mut client,
        4096,
        start + Duration::from_secs(1)
    ));
    assert!(viewer_has_stalled(
        &mut client,
        4096,
        start + RELAY_STALL_TIMEOUT + Duration::from_secs(1)
    ));
}

#[cfg(unix)]
#[test]
fn any_write_resets_the_stall_clock() {
    let (mut client, _peer) = viewer();
    let start = Instant::now();
    assert!(!viewer_has_stalled(&mut client, 0, start));
    client.relay.written.fetch_add(10, Ordering::Relaxed);
    assert!(!viewer_has_stalled(
        &mut client,
        4096,
        start + RELAY_STALL_TIMEOUT + Duration::from_secs(1)
    ));
}
