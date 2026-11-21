//! What a shared pane's viewers ask of the daemon: subscriptions, clipboard
//! requests and image pastes.

use super::*;

/// The smallest valid PNG: an 8-byte signature, an IHDR for a 1x1 greyscale
/// image, and IEND. The daemon checks the signature, so the payload cannot be
/// arbitrary bytes.
fn test_png() -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend([0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]);
    png.extend([0x3a, 0x7e, 0x9b, 0x55]);
    png.extend([0, 0, 0, 0]);
    png.extend(b"IEND");
    png.extend([0xae, 0x42, 0x60, 0x82]);
    png
}

/// Opens a shared session with one daemon-owned pane, which is what a window
/// attached to a session — remote or local — is actually looking at.
fn shared_pane_with_command(
    daemon: &TestDaemon,
    client: &Client,
    protection: Option<&zmux::auth::SessionAuthentication>,
    command: &str,
) -> (u64, zmux::client::SharedSpawnedPane) {
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let offered = summary(pane.session_id, pane.pane_id);
    client
        .share(
            pane.session_id,
            offered,
            serde_json::Value::Null,
            protection,
            true,
        )
        .unwrap();

    let request = spawn_request(Some(pane.session_id), command);
    let shared = client
        .spawn_shared(SharedSpawnRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            program: request.program,
            args: request.args,
            env: request.env,
            working_directory: request.working_directory,
            size: request.size,
            console_palette: request.console_palette,
            console_backend: Default::default(),
        })
        .unwrap_or_else(|error| {
            panic!(
                "spawning the shared pane: {error:#}\ndaemon log:\n{}",
                daemon.log()
            )
        });
    (pane.session_id, shared)
}

fn shared_pane_for_image_paste(
    daemon: &TestDaemon,
    client: &Client,
    protection: Option<&zmux::auth::SessionAuthentication>,
) -> (u64, zmux::client::SharedSpawnedPane) {
    shared_pane_with_command(daemon, client, protection, "sleep 60")
}

fn clipboard_probe_frame() -> zclip::protocol::Frame {
    zclip::protocol::Frame {
        id: [0x42; 16],
        message: zclip::protocol::Message::Probe,
    }
}

fn shell_octal_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:03o}")).collect()
}

#[test]
fn shared_clipboard_request_reaches_one_viewer_and_is_not_replayed() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let frame = clipboard_probe_frame().encode();
    let command = format!(
        "stty raw -echo; printf ready; dd bs=1 count=1 2>/dev/null; \
         printf '{}'; printf after-frame; dd bs=1 count=1 2>/dev/null; \
         printf reattached; sleep 60",
        shell_octal_bytes(&frame)
    );
    let (session_id, spawned) = shared_pane_with_command(&daemon, &client, None, &command);
    let pane_id = spawned.pane.pane_id();
    let mut reader = spawned.pane.reader();
    read_until_reader(&mut reader, "ready");
    spawned.pane.send_input(b"x").unwrap();
    let seen = read_until_reader(&mut reader, "after-frame");
    assert!(
        seen.as_bytes()
            .windows(frame.len())
            .any(|part| part == frame)
    );
    drop(reader);
    drop(spawned);

    let AttachOutcome::SharedAttached { pane: viewer, .. } = client
        .attach_shared_with_secret(session_id, pane_id, None)
        .unwrap()
    else {
        panic!("the pane did not reattach as shared")
    };
    let mut replay_reader = viewer.reader();
    viewer.send_input(b"y").unwrap();
    let replay = read_until_reader(&mut replay_reader, "reattached");
    assert!(
        !replay.contains("zclip;1;"),
        "clipboard request was retained for a later viewer: {replay:?}"
    );
}

#[test]
fn shared_clipboard_request_with_two_viewers_returns_ambiguity_error() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let frame = clipboard_probe_frame().encode();
    let error = zclip::protocol::Frame {
        id: clipboard_probe_frame().id,
        message: zclip::protocol::Message::Error(
            "multiple Zetta viewers are attached; clipboard destination is ambiguous".into(),
        ),
    }
    .encode();
    let expected_hex: String = error.iter().map(|byte| format!("{byte:02x}")).collect();
    let command = format!(
        "stty raw -echo; printf ready; dd bs=1 count=1 2>/dev/null; \
         printf '{}'; dd bs=1 count={} 2>/dev/null | od -An -tx1 | tr -d ' \\n'; \
         printf done; sleep 60",
        shell_octal_bytes(&frame),
        error.len()
    );
    let (session_id, spawned) = shared_pane_with_command(&daemon, &client, None, &command);
    let pane_id = spawned.pane.pane_id();
    let AttachOutcome::SharedAttached { pane: second, .. } = client
        .attach_shared_with_secret(session_id, pane_id, None)
        .unwrap()
    else {
        panic!("the second viewer did not attach as shared")
    };
    let mut reader = spawned.pane.reader();
    read_until_reader(&mut reader, "ready");
    spawned.pane.send_input(b"x").unwrap();
    let seen = read_until_reader(&mut reader, "done");
    assert!(
        seen.contains(&expected_hex),
        "missing ambiguity reply: {seen:?}"
    );
    assert!(
        !seen.contains("zclip;1;"),
        "request reached a viewer: {seen:?}"
    );
    drop(second);
}

/// Subscribing is answered before anything is delivered on the connection, and
/// the answer is what says the subscription is in the daemon's table.
///
/// Without it registration was one-way: a client that subscribed and then acted
/// could have the event its own action produced broadcast while its handle was
/// not yet published, and that event was simply lost. The window is short, so
/// what it produced was a test that passed on an idle machine and failed on a
/// loaded one — which is the same thing a window does when it misses the first
/// exit report after subscribing.
#[test]
fn subscribing_is_answered_before_any_event_is_delivered() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let (session_id, _shared) = shared_pane_for_image_paste(&daemon, &client, None);

    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    let mut subscription = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
    subscription
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    subscription
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id: std::process::id(),
            client_id: ClientId::new("acknowledged-subscriber"),
            stream_only: false,
            session_secret: None,
            request: Request::Subscribe,
        })
        .expect("opening the subscription");

    match subscription.receive::<Response>() {
        Ok((Response::Ok, _)) => {}
        Ok((other, _)) => panic!(
            "a subscription is acknowledged before anything else reaches it, not {other:?}\ndaemon log:\n{}",
            daemon.log()
        ),
        Err(error) => panic!(
            "the subscription was never acknowledged: {error:#}\ndaemon log:\n{}",
            daemon.log()
        ),
    }

    // And the acknowledgement is not an event standing in for one: what follows
    // on the same connection is the broadcast for something done afterwards,
    // which is exactly what a client that has the acknowledgement may rely on.
    let revision = client
        .shared_snapshot(session_id)
        .expect("the session's current revision")
        .revision;
    client
        .apply_shared(
            session_id,
            revision,
            SharedSessionOperation::SetTabState {
                state: serde_json::json!({"after": "the acknowledgement"}),
            },
        )
        .expect("changing the shared session");
    match subscription.receive::<Event>() {
        Ok((Event::SharedSessionUpdated { .. }, _)) => {}
        Ok((other, _)) => panic!("unexpected first event: {other:?}"),
        Err(error) => panic!(
            "the event after the acknowledgement was lost: {error:#}\ndaemon log:\n{}",
            daemon.log()
        ),
    }

    client.kill(session_id).unwrap();
}

/// A client that lets go of a shared stream is told the stream failed.
///
/// The daemon cannot tell a viewer that closed its socket from one whose
/// transport broke, so it reports both the same way. That matters to whoever
/// receives it: a window that releases a pane's stream on purpose — which is
/// what it does for a pane it has taken over Mosh — gets a failure report for
/// it, and must not answer by attaching a replacement it will release again.
/// Doing so is a loop that feeds itself one report per attempt.
#[test]
fn releasing_a_shared_stream_is_reported_to_its_own_client_as_a_failure() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let (session_id, spawned) = shared_pane_for_image_paste(&daemon, &client, None);
    let pane_id = spawned.pane.pane_id();
    drop(spawned);

    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    let mut subscription = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
    subscription
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    subscription
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id: std::process::id(),
            // The window subscribes and attaches as one client, which is what
            // makes the report reach it: a failure goes only to the client
            // whose stream it was.
            client_id: client.client_id().clone(),
            stream_only: false,
            session_secret: None,
            request: Request::Subscribe,
        })
        .expect("opening the subscription");
    assert!(
        matches!(subscription.receive::<Response>(), Ok((Response::Ok, _))),
        "the subscription has to be acknowledged before anything is delivered"
    );

    match client
        .attach_shared_with_secret(session_id, pane_id, None)
        .expect("attaching the pane's stream")
    {
        AttachOutcome::SharedAttached { pane, .. } => drop(pane),
        _other => panic!("a shared pane must attach as shared"),
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match subscription.receive::<Event>() {
            Ok((
                Event::SharedStreamFailed {
                    session_id: failed_session,
                    pane_id: failed_pane,
                },
                _,
            )) => {
                assert_eq!((failed_session, failed_pane), (session_id, pane_id));
                break;
            }
            Ok(_) => {}
            Err(error) => panic!(
                "the released stream was never reported: {error:#}\ndaemon log:\n{}",
                daemon.log()
            ),
        }
        assert!(
            Instant::now() < deadline,
            "the released stream was never reported\ndaemon log:\n{}",
            daemon.log()
        );
    }

    client.kill(session_id).unwrap();
}

/// Image paste in a remote session is exactly this request: the window uploads
/// a PNG and pastes back a path the pane's own process can open. Nothing
/// exercised the round trip on either side, so the whole feature rested on a
/// serialization test.
#[test]
fn a_shared_viewer_stores_an_image_the_panes_process_can_open() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let (session_id, shared) = shared_pane_for_image_paste(&daemon, &client, None);

    let png = test_png();
    let path = client
        .store_image(session_id, shared.pane.pane_id(), png.clone())
        .unwrap_or_else(|error| {
            panic!(
                "storing a clipboard image: {error:#}\ndaemon log:\n{}",
                daemon.log()
            )
        });

    assert!(
        Path::new(&path).is_absolute(),
        "a pasted image path has to be absolute: {path}"
    );
    assert_eq!(
        std::fs::read(&path).expect("the stored image must be readable"),
        png
    );

    // A second paste is a second file: overwriting the first would replace an
    // image a program may still be reading.
    let again = client
        .store_image(session_id, shared.pane.pane_id(), png.clone())
        .unwrap();
    assert_ne!(again, path);
    assert_eq!(std::fs::read(&again).unwrap(), png);

    // The staging directory goes with the session.
    client.kill(session_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Path::new(&path).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !Path::new(&path).exists(),
        "a killed session must not leave its staged images behind"
    );
}

/// A refusal has to reach the user as the reason it was refused.
///
/// The payload is written after the request, so the daemon can reject an
/// unknown session and close while that write is still going — and a paste
/// large enough not to fit in a socket buffer then fails with a broken pipe
/// rather than with what the daemon said. The response is the authority.
#[test]
fn a_refused_image_reports_the_refusal_rather_than_the_broken_write() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let (session_id, shared) = shared_pane_for_image_paste(&daemon, &client, None);

    // Big enough that the write cannot complete into a socket buffer before
    // the daemon has refused the request and gone.
    let mut png = test_png();
    png.resize(4 * 1024 * 1024, 0);

    let error = client
        .store_image(session_id + 1_000, shared.pane.pane_id(), png)
        .expect_err("an unknown session is refused");
    let error = format!("{error:#}");
    assert!(
        error.contains("does not exist"),
        "the refusal has to survive the broken write: {error}"
    );
}

/// The upload is addressed to a pane, and authorized by watching it. Each of
/// these refusals is a message a user would otherwise see as a failed paste.
#[test]
fn storing_an_image_is_refused_for_anything_but_a_pane_this_client_is_watching() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let (session_id, shared) = shared_pane_for_image_paste(&daemon, &client, None);
    let pane_id = shared.pane.pane_id();
    let png = test_png();

    let refused = |description: &str, error: anyhow::Error, expected: &str| {
        assert!(
            format!("{error:#}").contains(expected),
            "{description}: expected {expected:?}, got {error:#}"
        );
    };
    let stranger = daemon.client();

    refused(
        "an unknown session",
        client
            .store_image(session_id + 4096, pane_id, png.clone())
            .expect_err("an unknown session"),
        "does not exist",
    );
    refused(
        "an unknown pane",
        client
            .store_image(session_id, pane_id + 4096, png.clone())
            .expect_err("an unknown pane"),
        "no pane",
    );
    refused(
        // A second `Client` is a second logical viewer: it has its own identity
        // and never attached, which is what the daemon checks.
        "a client that is not watching the pane",
        stranger
            .store_image(session_id, pane_id, png.clone())
            .expect_err("a client that is not watching the pane"),
        "not an active shared viewer",
    );

    // The watching client, so this reaches the payload check rather than being
    // refused as a viewer before the bytes are read.
    refused(
        "a payload that is not a PNG",
        client
            .store_image(session_id, pane_id, b"GIF89a not a png".to_vec())
            .expect_err("a payload that is not a PNG"),
        "not a PNG",
    );

    // The refusals leave the real viewer working.
    assert!(
        client
            .store_image(session_id, pane_id, png)
            .is_ok_and(|path| Path::new(&path).exists())
    );
    client.kill(session_id).unwrap();
}

/// The shape a remote window actually uses: one stream-only attach whose
/// connection stays open for the pane's relay, and a *separate* request
/// connection carrying the same client identity for the upload. The identity
/// crossing between those two connections is what the daemon's viewer check
/// matches on, and nothing else exercises it.
///
/// The session is protected so the authorized path is the one under test. This
/// cannot also pin the refusal: `session_control_authorized` admits the
/// session's owner outright, and in one test process the peer credentials of
/// every connection are the owner's. A client identity that never attached is
/// refused by the test above.
#[test]
fn a_remote_shaped_viewer_uploads_over_its_own_request_connection() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let (session_id, shared) =
        shared_pane_for_image_paste(&daemon, &client, Some(&test_verifier()));
    let pane_id = shared.pane.pane_id();
    let png = test_png();

    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    let viewer = ClientId::new("remote-image-viewer");

    // The viewer first, exactly as a remote window does: one stream-only attach
    // whose connection stays open, then a separate request connection for the
    // upload carrying the same identity.
    let mut attach = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
    attach
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    attach
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token.clone(),
            client_process_id: std::process::id(),
            client_id: viewer.clone(),
            stream_only: true,
            session_secret: Some(SessionSecret::new(TEST_SECRET.to_owned())),
            request: Request::Attach(zmux::messages::AttachRequest {
                session_id,
                pane_id: Some(pane_id),
                secret: Some(SessionSecret::new(TEST_SECRET.to_owned())),
                force_shared: false,
                relaying_for: None,
            }),
        })
        .unwrap();
    let replay_length = match attach.receive::<Response>().unwrap().0 {
        Response::SharedAttached { replay_length, .. } => replay_length,
        other => panic!("unexpected attach response: {other:?}"),
    };
    attach.read_exact(replay_length).unwrap();

    let upload = |secret: Option<&str>| -> Response {
        let mut connection = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
        connection
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        connection
            .send(&Envelope {
                version: zmux::messages::PROTOCOL_VERSION,
                token: endpoint.token.clone(),
                client_process_id: std::process::id(),
                client_id: viewer.clone(),
                stream_only: true,
                session_secret: secret.map(|secret| SessionSecret::new(secret.to_owned())),
                request: Request::StoreImage(zmux::messages::StoreImageRequest {
                    session_id,
                    pane_id,
                    length: png.len(),
                }),
            })
            .unwrap();
        connection.write_all(&png).unwrap();
        connection.receive::<Response>().unwrap().0
    };

    match upload(Some(TEST_SECRET)) {
        Response::ImageStored { path } => {
            assert_eq!(std::fs::read(&path).unwrap(), png);
        }
        other => panic!(
            "unexpected response to an authorized image upload: {other:?}\ndaemon log:\n{}",
            daemon.log()
        ),
    }

    drop(attach);
    client.kill(session_id).unwrap();
}

/// The shape a window whose panes travel over Mosh uses, which has no
/// attachment of its own at all: `zmux relay-pane` holds the pane's stream here
/// on the daemon's host and copies it onto the Mosh link, so the window is in
/// no pane's shared set and its image paste used to be refused outright — every
/// time, for every Mosh-carried pane.
///
/// What makes it authorized is the relay naming the window it is relaying to,
/// and the authorization lasts exactly as long as that relay does.
#[test]
fn a_window_a_relay_is_showing_a_pane_to_may_store_an_image() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let (session_id, shared) = shared_pane_for_image_paste(&daemon, &client, None);
    let pane_id = shared.pane.pane_id();
    let png = test_png();

    // A second logical client, with no attachment: this is the window at the
    // far end of the Mosh link.
    let window = daemon.client();
    let viewer = window.client_id().clone();
    let refusal = |error: anyhow::Error| {
        let message = format!("{error:#}");
        assert!(
            message.contains("not an active shared viewer"),
            "a window no relay is serving must be refused as a viewer: {message}"
        );
    };
    refusal(
        window
            .store_image(session_id, pane_id, png.clone())
            .expect_err("a window nothing is relaying to is not a viewer"),
    );

    let relay = daemon.client();
    let relayed = match relay
        .attach_shared_relaying_for(session_id, pane_id, None, viewer)
        .unwrap_or_else(|error| {
            panic!(
                "attaching the pane as a relay: {error:#}\ndaemon log:\n{}",
                daemon.log()
            )
        }) {
        zmux::client::AttachOutcome::SharedAttached { pane, .. } => pane,
        _ => panic!("a relay attaches as a shared stream, and got something else"),
    };

    let path = window
        .store_image(session_id, pane_id, png.clone())
        .unwrap_or_else(|error| {
            panic!(
                "storing an image for the window the relay is serving: {error:#}\ndaemon log:\n{}",
                daemon.log()
            )
        });
    assert_eq!(std::fs::read(&path).expect("the stored image"), png);

    // The declaration is the relay's, so it goes when the relay does: a window
    // whose Mosh pane has ended is not still a viewer of it.
    drop(relayed);
    let deadline = Instant::now() + Duration::from_secs(10);
    let error = loop {
        match window.store_image(session_id, pane_id, png.clone()) {
            Err(error) => break error,
            Ok(_) => assert!(
                Instant::now() < deadline,
                "the window stayed authorized after its relay left\ndaemon log:\n{}",
                daemon.log()
            ),
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    refusal(error);

    client.kill(session_id).unwrap();
}
