use super::*;

#[test]
fn pane_controls_idle_delay_resets_and_expires() {
    let start = Instant::now();

    assert_eq!(
        pane_controls_hide_delay(start, start + Duration::from_millis(200)),
        Some(Duration::from_millis(1000))
    );
    assert_eq!(
        pane_controls_hide_delay(start, start + PANE_CONTROLS_IDLE_DELAY),
        None
    );
    assert_eq!(
        pane_controls_hide_delay(start, start + Duration::from_secs(5)),
        None
    );
}

#[test]
fn reconnect_is_immediate_only_for_one_background_session() {
    assert_eq!(reconnect_request(0), ReconnectRequest::None);
    assert_eq!(reconnect_request(1), ReconnectRequest::Immediate(0));
    assert_eq!(reconnect_request(2), ReconnectRequest::Choose);
}
