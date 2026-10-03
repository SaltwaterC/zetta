use super::*;
use std::sync::mpsc;

#[test]
fn a_cancelled_reconnect_reports_rejected_once() {
    let (sender, receiver) = mpsc::channel();
    {
        let _completion = ReconnectCompletion::new(sender);
    }

    assert_eq!(receiver.recv().unwrap(), ReconnectSessionResult::Rejected);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn a_reconnect_result_wins_over_the_cancellation_fallback() {
    let (sender, receiver) = mpsc::channel();
    let mut completion = ReconnectCompletion::new(sender);
    completion.send(ReconnectSessionResult::Reconnected);
    drop(completion);

    assert_eq!(
        receiver.recv().unwrap(),
        ReconnectSessionResult::Reconnected
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn a_reconnect_acknowledgement_waits_for_catalog_publication() {
    let timeout = std::time::Duration::from_secs(10);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    crate::background_sessions::after_pending_publications(move || {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(timeout).unwrap();
    });
    started_rx.recv_timeout(timeout).unwrap();

    let (sender, receiver) = mpsc::channel();
    ReconnectCompletion::new(sender).send_after_publication(ReconnectSessionResult::Reconnected);
    assert!(receiver.try_recv().is_err());
    release_tx.send(()).unwrap();
    assert_eq!(
        receiver.recv_timeout(timeout).unwrap(),
        ReconnectSessionResult::Reconnected
    );
    assert!(receiver.try_recv().is_err());
}
