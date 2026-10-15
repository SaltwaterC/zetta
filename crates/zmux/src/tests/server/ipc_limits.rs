use super::*;

#[test]
fn connection_permits_stop_at_the_limit_and_come_back() {
    let in_use = Arc::new(AtomicUsize::new(0));
    let first = ConnectionPermit::claim(&in_use, 2).unwrap();
    let _second = ConnectionPermit::claim(&in_use, 2).unwrap();
    assert!(ConnectionPermit::claim(&in_use, 2).is_none());

    drop(first);
    assert!(ConnectionPermit::claim(&in_use, 2).is_some());
}

#[test]
fn the_snapshot_budget_waits_for_room_and_then_refuses() {
    let budget = Arc::new(SnapshotBudget::with_capacity(10));
    let held = budget.reserve(8, Instant::now()).unwrap();
    assert!(
        budget
            .reserve(11, Instant::now() + Duration::from_secs(5))
            .is_err()
    );

    // Room that comes back while a request waits is given to it.
    let waiting = {
        let budget = Arc::clone(&budget);
        thread::spawn(move || {
            budget
                .reserve(5, Instant::now() + Duration::from_secs(5))
                .map(|_| ())
        })
    };
    thread::sleep(Duration::from_millis(50));
    drop(held);
    waiting.join().unwrap().unwrap();
    assert_eq!(budget.reserved(), 0);

    // Room that does not come back in time is a refusal, not a hang.
    let _held = budget.reserve(8, Instant::now()).unwrap();
    let started = Instant::now();
    assert!(
        budget
            .reserve(5, Instant::now() + Duration::from_millis(100))
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
fn connected_pair() -> (Connection, Stream) {
    let (ours, theirs) = Stream::pair().unwrap();
    (Connection::new(ours), theirs)
}

#[cfg(unix)]
#[test]
fn declared_snapshots_past_a_limit_are_refused_before_reading() {
    let budget = SnapshotBudget::default();
    let (mut connection, _peer) = connected_pair();

    let too_many = (0..=MAX_SNAPSHOTS_PER_REQUEST as u64)
        .map(|pane| (pane, 1))
        .collect::<Vec<_>>();
    assert!(read_snapshots(&budget, &mut connection, &too_many).is_err());

    let too_large = (0..17)
        .map(|pane| (pane, crate::retention::MAX_SNAPSHOT_BYTES))
        .collect::<Vec<_>>();
    assert!(read_snapshots(&budget, &mut connection, &too_large).is_err());

    let overflowing = [(1, usize::MAX), (2, usize::MAX)];
    assert!(read_snapshots(&budget, &mut connection, &overflowing).is_err());
    // Nothing was read and nothing stays reserved.
    assert_eq!(budget.reserved(), 0);
}

#[cfg(unix)]
#[test]
fn received_snapshots_hold_their_bytes_against_the_budget() {
    use std::io::Write as _;

    let budget = SnapshotBudget::with_capacity(16);
    let (mut connection, mut peer) = connected_pair();
    peer.write_all(b"abcdefgh").unwrap();

    let received = read_snapshots(&budget, &mut connection, &[(1, 3), (2, 5)]).unwrap();
    assert_eq!(received.bytes[&1], b"abc");
    assert_eq!(received.bytes[&2], b"defgh");
    assert_eq!(budget.reserved(), 8);
    drop(received);
    assert_eq!(budget.reserved(), 0);
}
