//! Protected sessions: scoping a backgrounded session, the secret a protected
//! pane needs, and the sealed keys of automatically protected ones.

use super::*;

/// A privately backgrounded session belongs to the window that backgrounded it.
///
/// Before the multiplexer held these sessions they lived inside the process
/// that made them, and no other Zetta could see or take one. Moving them out
/// published every one of them to every process, so a privately kept tab put
/// away in one window turned up in another window's reconnect picker.
/// `Ctrl-Shift-K` and the shared keep-running path are the explicit exceptions.
#[test]
fn a_backgrounded_session_is_scoped_to_the_process_that_backgrounded_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    // Published as this process's, so another window's picker can leave it out.
    let listed = client.list().unwrap();
    assert_eq!(
        listed.first().map(|session| session.scoped_to),
        Some(Some(std::process::id())),
        "a backgrounded session says whose it is: {listed:?}"
    );

    // And refused to anybody else, whatever their picker happens to show.
    let stranger = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let error = match client.attach_as(
        pane.session_id,
        pane.pane_id,
        stranger.id(),
        Some(TEST_SECRET.to_owned()),
    ) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("another process must not attach a scoped session"),
    };
    assert!(error.contains("scoped to"), "{error}");

    // Its own window still may, which is what reconnecting is.
    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("the owning process must attach"),
    }
    reap(stranger);
}

/// The CLI's half of the sharing toggle, for a session with no window to
/// toggle it from.
#[test]
fn a_backgrounded_session_can_be_shared_and_scoped_back() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    // Already protected — the detach above had to give it a secret — so sharing
    // it needs no new one. `a_session_cannot_be_shared_without_a_secret` covers
    // the session that has none.
    client
        .set_session_scope(pane.session_id, true, None)
        .unwrap();
    let listed = client.list().unwrap();
    assert_eq!(
        listed.first().map(|session| session.scoped_to),
        Some(None),
        "a shared session is nobody's in particular: {listed:?}"
    );

    // Which is what another process attaching it means.
    let stranger = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            stranger.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("a shared session must attach from anywhere"),
    }
    // Off screen again — the multiplexer reclaims a pane whose window has gone —
    // because scoping a session back is refused while a window still has it.
    reap(stranger);
    let deadline = Instant::now() + Duration::from_secs(15);
    while client
        .list()
        .unwrap()
        .first()
        .is_some_and(|session| session.held)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(100));
    }

    client
        .set_session_scope(pane.session_id, false, None)
        .unwrap();
    let listed = client.list().unwrap();
    assert_eq!(
        listed.first().map(|session| session.scoped_to),
        Some(Some(std::process::id())),
        "scoping back returns the session to the window that backgrounded it: {listed:?}"
    );
}

/// Protecting a session is the user's choice, and the same choice everywhere.
///
/// Detaching a tab has always taken an empty dialog to mean "leave it
/// unprotected", and keeping one running the same. Sharing is the same choice:
/// the dialog asks, and the multiplexer takes what it is given rather than
/// insisting. What a secret buys is identical in all three cases — nothing
/// attaches the session without it.
#[test]
fn protecting_a_session_is_optional_whichever_way_it_is_made_reachable() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    let unprotected = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(unprotected.descriptor);
    read_until(&descriptor, "ready");

    // Shared with no secret, as the dialog's empty pair asks for.
    client
        .share(
            unprotected.session_id,
            summary(unprotected.session_id, unprotected.pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .expect("sharing without a secret is a choice, not an error");
    // And kept with no secret, which is what detaching an unprotected tab does.
    drop(descriptor);
    client
        .detach(
            unprotected.session_id,
            summary(unprotected.session_id, unprotected.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .expect("keeping without a secret is a choice, not an error");
    let stranger = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    match client
        .attach_as(
            unprotected.session_id,
            unprotected.pane_id,
            stranger.id(),
            None,
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("an unprotected shared session attaches without a secret"),
    }

    // The other choice, on a second session: a secret, and nothing joins without
    // it.
    let protected = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(protected.descriptor);
    read_until(&descriptor, "ready");
    share_session(&client, protected.session_id, protected.pane_id);
    assert!(
        matches!(
            client
                .attach_as(protected.session_id, protected.pane_id, stranger.id(), None)
                .unwrap(),
            AttachOutcome::AuthenticationRequired
        ),
        "joining a protected session must need its secret"
    );
    assert!(
        matches!(
            client
                .attach_as(
                    protected.session_id,
                    protected.pane_id,
                    stranger.id(),
                    Some("not-the-secret".to_owned())
                )
                .unwrap(),
            AttachOutcome::AuthenticationFailed
        ),
        "a wrong secret must not join a protected session"
    );
    reap(stranger);
}

/// A protected session's administrative boundary is tied to the local socket
/// peer, not to the process id a client writes into its JSON envelope.
#[test]
fn protected_controls_reject_a_claimed_owner_without_peer_authority() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let owner = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let owner_pid = owner.id();
    let pane = client
        .spawn(spawn_request_as(None, "printf ready; sleep 120", owner_pid))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    // The session starts unprotected, so this test can set up an owner label
    // that differs from the actual peer without bypassing the boundary it is
    // testing. Once protected, changing only the claimed process id must not
    // make the current test process an administrator.
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            owner_pid,
        )
        .unwrap();
    client
        .set_session_scope(pane.session_id, true, Some(&test_verifier()))
        .unwrap();

    for error in [
        client.kill(pane.session_id).unwrap_err().to_string(),
        client.forget(pane.session_id).unwrap_err().to_string(),
        client
            .resize(pane.session_id, pane.pane_id, 100, 30)
            .unwrap_err()
            .to_string(),
        client
            .close_pane(pane.session_id, pane.pane_id)
            .unwrap_err()
            .to_string(),
        client
            .discard_pane(pane.session_id, pane.pane_id)
            .unwrap_err()
            .to_string(),
        client
            .set_session_scope(pane.session_id, false, None)
            .unwrap_err()
            .to_string(),
    ] {
        assert!(
            error.contains("protected"),
            "unexpected authorization error: {error}"
        );
    }
    let state = client.pane_states(vec![pane.pane_id]).unwrap();
    assert!(
        state[0].unknown,
        "protected pane state leaked to a foreign peer"
    );
    assert_eq!(
        client.list().unwrap().len(),
        1,
        "unauthorized controls changed the session"
    );

    reap(owner);
}

/// Sends one request on a fresh local connection, naming whichever process the
/// test likes. The kernel still reports this test process as the peer, which
/// is exactly the gap between a claim and an identity these tests probe.
fn connect_claiming(daemon: &TestDaemon, client_process_id: u32, request: Request) -> Connection {
    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    let mut connection = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
    connection
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    connection
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id,
            client_id: ClientId::new("claimant"),
            stream_only: false,
            session_secret: None,
            request,
        })
        .unwrap();
    connection
}

/// A window whose detach the daemon refused has already stopped reading its
/// panes, and takes them back with an ordinary attach — without the secret,
/// which it never kept. Holding the pane is what lets it: once the session
/// really is in the background the same process is only its owner, and
/// asking for it back needs the secret again.
#[test]
fn the_holder_of_a_protected_pane_takes_it_back_without_the_secret() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    share_session(&client, pane.session_id, pane.pane_id);
    drop(descriptor);

    let outcome = client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap();
    let AttachOutcome::Attached { pane: again, .. } = outcome else {
        panic!("the process holding the pane was asked for the secret");
    };
    drop(std::fs::File::from(again.descriptor));

    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    assert!(matches!(
        client
            .attach(pane.session_id, Some(pane.pane_id), None)
            .unwrap(),
        AttachOutcome::AuthenticationRequired
    ));
}

/// Naming the window that is handing a protected pane over is not being it.
///
/// The holder answering its own revoke re-attaches without the secret, which
/// is what makes a protected session shareable at all. Decided on the claimed
/// process, that let anything holding the endpoint token join the relay —
/// reading the pane and typing into it — for as long as a handover was open.
#[test]
fn a_claimed_holder_does_not_skip_the_secret_during_a_handover() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    // A real process, so the liveness sweep leaves its handover open, but not
    // this one, so the kernel can tell the two apart.
    let holder = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let holder_pid = holder.id();
    let pane = client
        .spawn(spawn_request_as(
            None,
            "printf ready; sleep 120",
            holder_pid,
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    // Protected and offered while nobody shows it, then taken by the holder with
    // the secret, so the pane is held, by a process this test is not.
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            holder_pid,
        )
        .unwrap();
    client
        .set_session_scope(pane.session_id, true, Some(&test_verifier()))
        .unwrap();
    let AttachOutcome::Attached { pane: held, .. } = client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            holder_pid,
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    else {
        panic!("the holder did not take the pane");
    };
    let descriptor = std::fs::File::from(held.descriptor);

    // A second window joins with the secret, which revokes the holder's pane.
    // Nobody answers for the holder, so the handover stays open.
    let joiner = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let joining = {
        let client = daemon.client();
        let (session_id, pane_id, joiner_pid) = (pane.session_id, pane.pane_id, joiner.id());
        std::thread::spawn(move || {
            client
                .attach_as(
                    session_id,
                    pane_id,
                    joiner_pid,
                    Some(TEST_SECRET.to_owned()),
                )
                .map(|_| ())
        })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while client
        .list()
        .unwrap()
        .iter()
        .any(|session| session.id == pane.session_id && session.held)
    {
        assert!(Instant::now() < deadline, "the handover never started");
        std::thread::sleep(Duration::from_millis(20));
    }

    let outcome = client
        .attach_as(pane.session_id, pane.pane_id, holder_pid, None)
        .map_err(|error| error.to_string());
    assert!(
        matches!(outcome, Ok(AttachOutcome::AuthenticationRequired)),
        "claiming the holder's process must not skip the secret: {:?}",
        outcome.map(|_| ())
    );

    // Ending the holder is what releases the waiting joiner.
    reap(holder);
    let _ = joining.join().unwrap();
    drop(descriptor);
    reap(joiner);
}

/// A plain spawn naming a live protected session adds a pane to it, running
/// whatever the caller chose, and keeps the session alive. That is control of
/// the session, so it needs the owner, a holder, or the secret — as
/// `SpawnShared` always did.
#[test]
fn an_unauthenticated_spawn_cannot_join_a_live_protected_session() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let owner = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let pane = client
        .spawn(spawn_request_as(
            None,
            "printf ready; sleep 120",
            owner.id(),
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            owner.id(),
        )
        .unwrap();
    client
        .set_session_scope(pane.session_id, true, Some(&test_verifier()))
        .unwrap();

    let error = match client.spawn(spawn_request(
        Some(pane.session_id),
        "printf intruder; sleep 120",
    )) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a spawn without the secret joined a protected session"),
    };
    assert!(error.contains("not authorized"), "{error}");

    reap(owner);
}

/// Taking a shared pane's terminal is for its sole viewer, and which process
/// that is was vouched for when it attached. A connection that only names the
/// viewer's process is somebody else.
#[test]
fn a_claimed_viewer_cannot_take_a_protected_pane() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf first-ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "first-ready");
    share_session(&client, pane.session_id, pane.pane_id);
    let request = spawn_request(Some(pane.session_id), "printf shared-ready; cat");
    let spawned = client
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
        .unwrap();
    let shared_pane_id = spawned.pane.pane_id();
    drop(spawned);

    // The viewer, as the multiplexer recorded it, names another process.
    let viewer = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let viewer_pid = viewer.id();
    let AttachOutcome::SharedAttached { pane: viewing, .. } = client
        .attach_shared_as(
            pane.session_id,
            shared_pane_id,
            viewer_pid,
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    else {
        panic!("the viewer did not join the shared pane");
    };

    let mut claimant = connect_claiming(
        &daemon,
        viewer_pid,
        Request::TakeExclusive {
            session_id: pane.session_id,
            pane_id: shared_pane_id,
        },
    );
    // Before the fix the refusal, when there was one, was about the number of
    // viewers; this one is decided before the viewers are looked at.
    match claimant.receive::<Response>().unwrap().0 {
        Response::Error { message } => {
            assert!(message.contains("could not confirm"), "{message}");
        }
        other => panic!("a claimed viewer was answered with {other:?}"),
    }

    drop(viewing);
    drop(descriptor);
    reap(viewer);
}

/// A backgrounded session stays its window's, even after that window is gone.
///
/// Releasing the scope when the process exited made backgrounding a slow way of
/// sharing: leave the window, and the session another Zetta could not see became
/// one it could attach. Widening a session is an explicit request, so the scope
/// outlives the window and `zmux share` is the way back in.
#[test]
fn a_backgrounded_session_stays_scoped_after_its_window_exits() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    // Backgrounded by a window that then exits, which is what a crash looks
    // like from here.
    let window = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let window_pid = window.id();
    let pane = client
        .spawn(spawn_request_as(
            None,
            "printf ready; sleep 120",
            window_pid,
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            window_pid,
        )
        .unwrap();
    reap(window);

    // Long enough for the liveness sweep to have noticed, which is what used to
    // hand the session to everybody.
    std::thread::sleep(Duration::from_secs(3));
    let listed = client.list().unwrap();
    assert_eq!(
        listed.first().map(|session| session.scoped_to),
        Some(Some(window_pid)),
        "the scope outlives the window: {listed:?}"
    );
    let error = match client.attach(pane.session_id, Some(pane.pane_id), None) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a departed window's session must not attach elsewhere"),
    };
    assert!(error.contains("has exited"), "{error}");

    // Explicitly shared, it attaches — the one way in, and a deliberate one.
    client
        .set_session_scope(pane.session_id, true, None)
        .unwrap();
    match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("an explicitly shared session attaches from anywhere"),
    }
}

/// A session protected with a key sealed to the user's recipients is attached by
/// opening that key, with nothing typed. The daemon is unchanged by this: it
/// checks an Argon2id verifier exactly as it does for a secret someone chose.
#[cfg(feature = "session-persistence")]
#[test]
fn an_automatically_protected_session_is_attached_by_opening_its_sealed_key() {
    let identity = age::x25519::Identity::generate();
    let recipients =
        zmux::persistence::RecipientSet::parse(&[identity.to_public().to_string()]).unwrap();
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let sealed = zmux::auto_protect::seal(&recipients).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"sealed": true}),
            Some(&sealed.authentication),
            Vec::new(),
        )
        .unwrap();

    // The envelope is published, because it is public ciphertext and the party
    // it helps is the one holding the private key.
    let listed = client
        .list()
        .unwrap()
        .into_iter()
        .find(|session| session.id == pane.session_id)
        .expect("the detached session is listed");
    assert!(listed.authentication_required);
    let envelope = listed
        .key_envelope
        .expect("an automatically protected session publishes its sealed key");

    // Nothing typed: the key comes out of the envelope with the identity.
    let identity_path = daemon.config.join("identity.txt");
    std::fs::write(
        &identity_path,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let identities =
        zmux::persistence::IdentitySet::from_paths(std::slice::from_ref(&identity_path)).unwrap();
    let secret = zmux::auto_protect::open(&envelope, &identities).unwrap();

    assert!(
        matches!(
            client
                .attach(
                    pane.session_id,
                    Some(pane.pane_id),
                    Some(secret.expose().to_owned())
                )
                .unwrap(),
            AttachOutcome::Attached { .. }
        ),
        "the recovered key should attach the session"
    );
}

/// Without the sealed key the session is exactly as closed as any other
/// protected one — the envelope being published does not weaken it.
#[cfg(feature = "session-persistence")]
#[test]
fn a_published_envelope_does_not_let_an_unopened_session_be_attached() {
    let identity = age::x25519::Identity::generate();
    let recipients =
        zmux::persistence::RecipientSet::parse(&[identity.to_public().to_string()]).unwrap();
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let sealed = zmux::auto_protect::seal(&recipients).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({}),
            Some(&sealed.authentication),
            Vec::new(),
        )
        .unwrap();

    let listed = client
        .list()
        .unwrap()
        .into_iter()
        .find(|session| session.id == pane.session_id)
        .unwrap();
    let envelope = listed.key_envelope.unwrap();

    // A stranger holds the envelope but not the key it was sealed to.
    let other = age::x25519::Identity::generate();
    let stranger_path = daemon.config.join("stranger.txt");
    std::fs::write(
        &stranger_path,
        format!("{}\n", other.to_string().expose_secret()),
    )
    .unwrap();
    let strangers =
        zmux::persistence::IdentitySet::from_paths(std::slice::from_ref(&stranger_path)).unwrap();
    assert!(zmux::auto_protect::open(&envelope, &strangers).is_err());

    // And the envelope itself is not a secret the daemon will take.
    assert!(
        matches!(
            client
                .attach(pane.session_id, Some(pane.pane_id), Some(envelope))
                .unwrap(),
            AttachOutcome::AuthenticationFailed
        ),
        "the envelope must not authenticate"
    );
}

/// The deadlock automatic protection created: a protected session whose window
/// has exited could not be reached at all.
///
/// Attaching it refuses because it is still scoped to that window and says to
/// share it — which is what `attach` tells the user to do — and sharing refused
/// in turn because the owner it named was gone. Before every backgrounded
/// session was protected this was rare; afterwards it was the ordinary outcome
/// of closing a window, and the session and its processes were stranded.
#[test]
fn a_protected_session_whose_window_exited_can_still_be_offered() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    // A real process, so the daemon's liveness check is exercised rather than a
    // pid that never existed; it is reaped before the session is shared.
    let owner = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let owner_pid = owner.id();
    let pane = client
        .spawn(spawn_request_as(None, "printf ready; sleep 120", owner_pid))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            Some(&test_verifier()),
            owner_pid,
        )
        .unwrap();

    // While that window is alive its scope is its own business, and a stranger
    // must not override it.
    let refused = client
        .set_session_scope(pane.session_id, true, None)
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("protected"),
        "a live owner's scope must stand: {refused}"
    );

    reap(owner);

    // With the window gone, offering it is the documented way out and now works.
    client
        .set_session_scope(pane.session_id, true, None)
        .unwrap();

    // Offering changed no secret: the session is still gated by the one it had.
    assert!(
        matches!(
            client
                .attach(pane.session_id, Some(pane.pane_id), None)
                .unwrap(),
            AttachOutcome::AuthenticationRequired
        ),
        "sharing a stranded session must not unprotect it"
    );
    assert!(
        matches!(
            client
                .attach(
                    pane.session_id,
                    Some(pane.pane_id),
                    Some(TEST_SECRET.to_owned())
                )
                .unwrap(),
            AttachOutcome::Attached { .. }
        ),
        "the session's own secret still opens it"
    );
}

/// The half that stays with the owner: offering a stranded session may not carry
/// a new verifier, or anyone could seize a protected session by replacing the
/// secret that gates it and then attaching with their own.
#[test]
fn a_stranded_session_cannot_be_seized_by_replacing_its_verifier() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let owner = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let owner_pid = owner.id();
    let pane = client
        .spawn(spawn_request_as(None, "printf ready; sleep 120", owner_pid))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            Some(&test_verifier()),
            owner_pid,
        )
        .unwrap();
    reap(owner);

    let seized = zmux::auth::SessionAuthentication::create("attacker's secret").unwrap();
    let refused = client
        .set_session_scope(pane.session_id, true, Some(&seized))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("protected"),
        "replacing the verifier is not part of the escape hatch: {refused}"
    );

    // And the original secret is untouched.
    client
        .set_session_scope(pane.session_id, true, None)
        .unwrap();
    assert!(matches!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some("attacker's secret".to_owned())
            )
            .unwrap(),
        AttachOutcome::AuthenticationFailed
    ));
}
