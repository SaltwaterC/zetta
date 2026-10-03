use super::*;

#[cfg(unix)]
#[test]
fn a_relay_releasing_backpressure_wakes_the_drain() {
    let directory = tempfile::tempdir().unwrap();
    let daemon = Arc::new(Daemon::new(
        directory.path(),
        Retention::None,
        directory.path().join("clipboard"),
        #[cfg(feature = "session-persistence")]
        None,
        1,
        1,
        #[cfg(not(target_os = "macos"))]
        -1,
    ));
    let (notify, mut wait) = Stream::pair().unwrap();
    notify.set_nonblocking(true).unwrap();
    wait.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    *daemon
        .drain_wake
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(notify);
    let (server, mut client) = Stream::pair().unwrap();
    let relay = spawn_relay(
        &Connection::new(server),
        1,
        2,
        std::process::id(),
        Arc::downgrade(&daemon),
    )
    .unwrap();
    let payload = vec![b'x'; RELAY_BACKPRESSURE_BYTES + 1];
    relay.queued.store(payload.len(), Ordering::Relaxed);
    relay.frames.send_blocking(payload.clone().into()).unwrap();
    let mut received = vec![0; payload.len()];
    client.read_exact(&mut received).unwrap();
    assert_eq!(received, payload);
    let mut byte = [0];
    wait.read_exact(&mut byte)
        .expect("releasing relay pressure must wake the drain");
    assert_eq!(&byte, b".");
    assert_eq!(relay.queued.load(Ordering::Relaxed), 0);
}

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
    let relay = spawn_relay(&serve, 1, 2, std::process::id(), std::sync::Weak::new()).unwrap();
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
    // reattach now rather than on its next keystroke. Unix reports EOF after
    // local read shutdown; Winsock can instead report WSAESHUTDOWN (10058).
    // Both end the serve loop, but a timeout or an unrelated error must fail.
    let mut byte = [0; 1];
    let read = serve.stream().try_clone().unwrap().read(&mut byte);
    assert!(
        matches!(&read, Ok(0))
            || matches!(&read, Err(error) if cfg!(windows) && error.raw_os_error() == Some(10058)),
        "an evicted relay must end its serve loop's read: {read:?}"
    );
}

#[test]
fn shared_attachments_are_never_reused() {
    let first = next_shared_attachment();
    let second = next_shared_attachment();
    assert_ne!(first, second);
}

#[cfg(all(unix, not(target_os = "macos")))]
mod authorization {
    use super::*;
    use crate::server::secret_check::tests::fixtures::*;

    fn attacher(client_process_id: u32) -> Attacher {
        Attacher {
            client_process_id,
            client_id: ClientId::new("attacher"),
            stream_only: false,
            relaying_for: None,
        }
    }

    fn owner(daemon: &Daemon) -> Option<u32> {
        daemon.sessions.lock().unwrap()[0].owner
    }

    #[test]
    fn an_attach_claims_an_unclaimed_protected_session_only_once_authorized() {
        let (daemon, _directory) = daemon_with(vec![protected_session(1, None)]);

        let refusal = authorize_attach(&daemon, 1, None, &attacher(9999)).err();
        assert!(matches!(
            refusal.as_deref(),
            Some(Response::AuthenticationRequired)
        ));
        assert_eq!(owner(&daemon), None, "asking for the secret claimed it");

        let refusal = authorize_attach(&daemon, 1, Some("wrong"), &attacher(9999)).err();
        assert!(matches!(
            refusal.as_deref(),
            Some(Response::AuthenticationFailed)
        ));
        assert_eq!(owner(&daemon), None, "a wrong secret claimed it");

        daemon.sessions.lock().unwrap()[0].refuse_until = None;
        drop(
            authorize_attach(&daemon, 1, Some(SECRET), &attacher(4321))
                .ok()
                .unwrap(),
        );
        assert_eq!(owner(&daemon), Some(4321));
    }

    #[test]
    fn an_attach_whose_session_ends_during_the_check_is_told_so() {
        let (daemon, _directory) = daemon_with(vec![protected_session(1, None)]);
        let attach = {
            let daemon = Arc::clone(&daemon);
            thread::spawn(move || {
                authorize_attach(&daemon, 1, Some(SECRET), &attacher(4321))
                    .err()
                    .map(|refusal| format!("{refusal:?}"))
            })
        };
        // Ended while its check runs, which is the window releasing the lock
        // opened: the attach must notice rather than act on a session that is
        // no longer there.
        while !check_in_flight(&daemon) {
            assert!(
                !attach.is_finished(),
                "the check ended before it was observed"
            );
            thread::yield_now();
        }
        daemon.sessions.lock().unwrap().clear();

        let refusal = attach.join().unwrap().expect("the attach was refused");
        assert!(refusal.contains("does not exist"), "{refusal}");
    }
}
