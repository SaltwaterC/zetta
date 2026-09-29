use super::*;

/// A relay's two ends, as the daemon holds them: the serve loop's connection
/// and the client's.
fn relay_pair() -> (Connection, Connection, Relay) {
    let (daemon_side, client_side) = Stream::pair().unwrap();
    client_side
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    daemon_side
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let serve = Connection::new(daemon_side);
    let relay = spawn_relay(&serve, 1, 2, std::process::id()).unwrap();
    (serve, Connection::new(client_side), relay)
}

#[test]
fn a_retired_relay_tells_its_client_the_stream_ended_cleanly() {
    let (_serve, mut client, relay) = relay_pair();
    // Leaving the shared set drops the relay, which is its queue closing.
    drop(relay);
    assert!(matches!(
        client.receive::<Event>().unwrap().0,
        Event::SharedClosed {
            session_id: 1,
            pane_id: 2
        }
    ));
}

/// An evicted viewer did not ask to go. Telling it the stream ended cleanly
/// is what made a Zetta pane's reader finish while the window was already
/// reattaching, leaving the replacement stream with nothing to read it.
#[test]
fn an_evicted_relay_ends_its_stream_as_broken_and_wakes_its_serve_loop() {
    let (serve, mut client, relay) = relay_pair();
    relay.evict();
    drop(relay);

    let received = client.receive::<Event>();
    assert!(
        received.is_err(),
        "an evicted viewer must not be told the stream ended cleanly: {:?}",
        received.map(|(event, _)| event)
    );
    // The serve loop reads the same socket. It has to see the end too, since
    // its ending is what reports the stream failed and prompts the client to
    // reattach now rather than on its next keystroke.
    let mut byte = [0; 1];
    assert_eq!(
        serve.stream().try_clone().unwrap().read(&mut byte).unwrap(),
        0
    );
}

#[test]
fn shared_attachments_are_never_reused() {
    let first = next_shared_attachment();
    let second = next_shared_attachment();
    assert_ne!(first, second);
}
