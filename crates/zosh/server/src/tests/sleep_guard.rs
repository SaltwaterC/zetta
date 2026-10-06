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

/// Whether this host is expected to grant the assertion. Linux grants it only
/// where logind is reachable on the system bus, which a build container may
/// not have, so there the release half is asserted and the acquire half is
/// whatever the host allows.
fn assertion_expected(guard: &IdleSleepGuard) -> bool {
    if cfg!(target_os = "linux") {
        guard.assertion.is_some()
    } else {
        cfg!(any(target_os = "macos", windows))
    }
}

#[test]
fn the_guard_follows_presence_transitions() {
    let mut guard = IdleSleepGuard::new();
    guard.update(true, false);
    assert!(guard.present);
    let expected = assertion_expected(&guard);
    assert_eq!(guard.assertion.is_some(), expected);
    guard.update(true, false);
    assert_eq!(guard.assertion.is_some(), expected);
    guard.update(false, false);
    assert!(!guard.present);
    assert!(guard.assertion.is_none());
}
