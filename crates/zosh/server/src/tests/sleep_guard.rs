use super::*;

#[test]
fn a_peer_heard_from_within_the_linger_is_present() {
    assert!(peer_present(true, Duration::ZERO));
    assert!(peer_present(
        true,
        PRESENCE_LINGER - Duration::from_millis(1)
    ));
}

#[test]
fn a_peer_silent_for_the_linger_is_absent() {
    assert!(!peer_present(true, PRESENCE_LINGER));
    assert!(!peer_present(true, PRESENCE_LINGER * 10));
}

#[test]
fn an_unassociated_server_keeps_nothing_awake() {
    assert!(!peer_present(false, Duration::ZERO));
}

#[test]
fn the_linger_outlasts_the_keep_alive_linger() {
    // Releasing the assertion during an outage the keep-alive still covers
    // would let the host sleep under a session that is about to resume.
    assert!(PRESENCE_LINGER > crate::server::KEEP_ALIVE_LINGER);
}

#[test]
fn the_guard_follows_presence_transitions() {
    let mut guard = IdleSleepGuard::new();
    guard.update(true, false);
    assert!(guard.present);
    assert_eq!(guard.assertion.is_some(), cfg!(target_os = "macos"));
    guard.update(true, false);
    assert_eq!(guard.assertion.is_some(), cfg!(target_os = "macos"));
    guard.update(false, false);
    assert!(!guard.present);
    assert!(guard.assertion.is_none());
}
