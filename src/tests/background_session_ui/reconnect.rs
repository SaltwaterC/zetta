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
