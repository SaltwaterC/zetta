use super::*;

use std::{
    os::unix::net::UnixStream,
    sync::mpsc,
    time::{Duration, Instant},
};

fn relay_with_sender(
    capacity: usize,
    id: u64,
    client_id: ClientId,
) -> (Arc<SubscriberRelay>, mpsc::Receiver<Event>) {
    let (sender, receiver) = mpsc::sync_channel(capacity);
    let relay = Arc::new(SubscriberRelay {
        id,
        client_id,
        sender,
        closed: Arc::new(AtomicBool::new(false)),
        receiver: Arc::new(Mutex::new(None)),
        connection: Arc::new(Mutex::new(None)),
    });
    (relay, receiver)
}

fn test_daemon() -> (Arc<Daemon>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let image_directory = directory.path().join("clipboard");
    create_private_dir(&image_directory).unwrap();
    let daemon = Arc::new(Daemon::new(
        directory.path(),
        Retention::None,
        image_directory,
        #[cfg(feature = "session-persistence")]
        None,
        1,
        1,
        -1,
    ));
    (daemon, directory)
}

#[test]
fn enqueue_does_not_wait_for_a_stalled_writer() {
    let (relay, _receiver) = relay_with_sender(1, 1, ClientId::new("stalled"));
    let started = Instant::now();

    assert!(relay.enqueue(Event::Replacing));

    assert!(
        started.elapsed() < Duration::from_millis(50),
        "enqueue waited for a subscriber writer"
    );
}

#[test]
fn relay_delivers_events_in_enqueue_order() {
    let (writer, reader) = UnixStream::pair().unwrap();
    let (relay, receiver) = relay_with_sender(16, 1, ClientId::new("ordered"));
    let closed = Arc::clone(&relay.closed);
    let worker = std::thread::spawn(move || {
        run_subscriber_relay(
            std::sync::Weak::new(),
            ClientId::new("ordered"),
            1,
            closed,
            Connection::new(writer),
            receiver,
        );
    });

    assert!(relay.enqueue(Event::Grant {
        session_id: 1,
        pane_id: 2,
    }));
    assert!(relay.enqueue(Event::Revoke {
        session_id: 1,
        pane_id: 2,
    }));

    let mut reader = Connection::new(reader);
    reader
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    assert!(matches!(
        reader.receive::<Event>().unwrap().0,
        Event::Grant { .. }
    ));
    assert!(matches!(
        reader.receive::<Event>().unwrap().0,
        Event::Revoke { .. }
    ));

    drop(relay);
    worker.join().unwrap();
}

#[test]
fn a_full_relay_queue_disconnects_that_subscription() {
    let (relay, _receiver) = relay_with_sender(1, 1, ClientId::new("full"));
    assert!(relay.enqueue(Event::Replacing));
    assert!(!relay.enqueue(Event::Replacing));
    assert!(relay.closed.load(Ordering::Acquire));
}

#[test]
fn an_old_relay_cannot_remove_a_replacement_subscription() {
    let (daemon, _directory) = test_daemon();
    let client_id = ClientId::new("replacement");
    let (old, _old_receiver) = relay_with_sender(1, 1, client_id.clone());
    let (replacement, _replacement_receiver) = relay_with_sender(1, 2, client_id.clone());
    daemon
        .subscribers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            client_id.clone(),
            Subscriber {
                process_id: 1,
                relay: Arc::clone(&replacement),
            },
        );

    remove_subscriber_if_matches(&daemon, &client_id, old.id);

    let subscribers = daemon
        .subscribers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        subscribers.get(&client_id).unwrap().relay.id,
        replacement.id
    );
}
