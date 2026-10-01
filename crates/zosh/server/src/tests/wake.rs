use super::*;
use std::sync::mpsc;
use std::time::Duration;

/// A bounded park, so a wakeup that never comes fails the test instead of
/// hanging it.
fn within(limit: Duration) -> WakeDeadline {
    let mut deadline = WakeDeadline::default();
    deadline.at(Some(Instant::now() + limit));
    deadline
}

#[test]
fn publishing_an_event_preserves_the_wakeup_before_park() {
    // Use a fresh thread so another test cannot leave a park token behind.
    thread::spawn(|| {
        let (sender, receiver) = mpsc::sync_channel(1);
        let events = WakingSender::to_current(sender);
        events.send(7_u8).unwrap();
        let started = Instant::now();
        within(Duration::from_secs(2)).park();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(receiver.try_recv(), Ok(7));
    })
    .join()
    .unwrap();
}

#[test]
fn a_sender_on_another_thread_wakes_the_park() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let events = WakingSender::to_current(sender);
    let started = Instant::now();
    let producer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        events.send(()).unwrap();
    });
    // Spurious wakeups are allowed, so wait for the event rather than for
    // the first return from park.
    while receiver.try_recv().is_err() {
        within(Duration::from_secs(2)).park();
        assert!(started.elapsed() < Duration::from_secs(1), "never woken");
    }
    producer.join().unwrap();
}

#[test]
fn the_deadline_is_the_earliest_instant_offered() {
    let now = Instant::now();
    let mut deadline = WakeDeadline::default();
    assert_eq!(deadline.earliest(), None);
    deadline.at(None);
    assert_eq!(deadline.earliest(), None);
    deadline.at(Some(now + Duration::from_secs(3)));
    deadline.at(None);
    deadline.at(Some(now + Duration::from_millis(50)));
    deadline.at(Some(now + Duration::from_secs(1)));
    assert_eq!(deadline.earliest(), Some(now + Duration::from_millis(50)));
}

#[test]
fn a_past_deadline_does_not_park() {
    let mut deadline = WakeDeadline::default();
    deadline.at(Some(Instant::now() - Duration::from_secs(1)));
    let started = Instant::now();
    deadline.park();
    assert!(started.elapsed() < Duration::from_millis(500));
}
