//! Who may control a session on Windows: a protected session's own window, a
//! claimed owner without the peer's authority, and the wrong token.

use super::*;

/// The verifier for a protected test session, as a client creates one before
/// sharing.
fn test_verifier() -> zmux::auth::SessionAuthentication {
    zmux::auth::SessionAuthentication::create("correct horse battery staple")
        .expect("creating the session verifier")
}

/// Speaks the protocol directly, as any same-user process that can read the
/// endpoint token could, naming whichever process it likes as its own.
fn impostor_request(
    daemon: &TestDaemon,
    claimed_process_id: u32,
    request: zmux::messages::Request,
) -> zmux::messages::Response {
    use zmux::{
        messages::{Envelope, PROTOCOL_VERSION, Request, Response},
        transport::{Connection, Endpoint, Stream},
    };

    let endpoint = Endpoint::read(&daemon.sessions_dir().join("zmux.json"))
        .expect("reading the multiplexer endpoint");
    let stream = Stream::connect(&endpoint.socket_path).expect("connecting as an impostor");
    let mut connection = Connection::new(stream);
    connection
        .send(&Envelope {
            version: PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id: claimed_process_id,
            client_id: zmux::messages::ClientId::default(),
            stream_only: false,
            session_secret: None,
            request,
        })
        .expect("sending an impostor request");
    let (response, _) = connection
        .receive::<Response>()
        .expect("reading the answer to an impostor request");
    let Response::AttestationRequired { .. } = response else {
        return response;
    };
    // The handle was duplicated into the process being impersonated, not into
    // this one, so the nonce behind it cannot be read. Answering wrongly is the
    // best an impostor can do.
    connection
        .send(&Request::Attested {
            nonce: "0".repeat(64),
        })
        .expect("answering the attestation challenge");
    connection
        .receive::<Response>()
        .expect("reading the answer after a failed attestation")
        .0
}

/// Windows has no peer credentials for `AF_UNIX`, so the daemon proves whose
/// connection this is by duplicating a nonce into the process the peer names.
/// Without that, naming a protected session's owner was all it took to detach,
/// kill or rescope it — the endpoint token would have been an administrator
/// credential.
#[test]
fn protected_controls_reject_a_claimed_owner_without_peer_authority() {
    use zmux::messages::{Request, Response};

    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    drop(pane);
    // Protect the session while it is still owned by this process. The
    // requests below claim that owner but deliberately fail the attestation,
    // so the setup does not depend on the boundary it tests.
    client
        .detach(
            session_id,
            session_summary(session_id, pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .expect("backgrounding the session");
    client
        .set_session_scope(session_id, true, Some(&test_verifier()))
        .expect("protecting the session");

    for request in [
        Request::Kill { session_id },
        Request::Forget { session_id },
        Request::Resize {
            session_id,
            pane_id,
            revision: None,
            columns: 100,
            lines: 30,
        },
        Request::ClosePane {
            session_id,
            pane_id,
        },
        Request::SetSessionScope(zmux::messages::SessionScopeRequest {
            session_id,
            shared: false,
            verifier: None,
            key_envelope: None,
        }),
    ] {
        let name = format!("{request:?}");
        match impostor_request(&daemon, std::process::id(), request) {
            Response::Error { message } => assert!(
                message.contains("protected"),
                "unexpected authorization error for {name}: {message}"
            ),
            other => panic!("{name} was not refused: {other:?}"),
        }
    }
    match impostor_request(
        &daemon,
        std::process::id(),
        Request::PaneStates {
            pane_ids: vec![pane_id],
        },
    ) {
        Response::PaneStates { panes } => assert!(
            panes[0].unknown,
            "protected pane state leaked to a peer claiming to be its owner"
        ),
        other => panic!("unexpected pane state answer: {other:?}"),
    }
    assert_eq!(
        client.list().expect("listing sessions").len(),
        1,
        "unauthorized controls changed the session"
    );

    let _ = client.shutdown();
}

/// The other half of the same boundary: a client that really is the process it
/// names still gets through, because it can read the nonce the daemon
/// duplicated into it.
#[test]
fn a_protected_sessions_own_window_still_controls_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request())
        .expect("creating a pseudoconsole pane");
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    drop(pane);
    client
        .detach(
            session_id,
            session_summary(session_id, pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .expect("backgrounding the session");
    client
        .set_session_scope(session_id, true, Some(&test_verifier()))
        .expect("protecting the session");

    client
        .kill(session_id)
        .expect("the owning process must still be able to end its own session");
    assert!(
        client.list().expect("listing sessions").is_empty(),
        "the session outlived its owner ending it"
    );
    let _ = client.shutdown();
}

/// The endpoint token authenticates the channel on Windows exactly as it does
/// on Unix, where the socket's mode is the first line of defence and there is
/// no mode to rely on here.
#[test]
fn a_windows_connection_with_the_wrong_token_gets_nothing() {
    use zmux::{
        messages::{Envelope, PROTOCOL_VERSION, Request, Response},
        transport::{Connection, Endpoint, Stream},
    };

    let daemon = TestDaemon::start();
    let endpoint = Endpoint::read(&daemon.sessions_dir().join("zmux.json"))
        .expect("reading the multiplexer endpoint");
    let stream = Stream::connect(&endpoint.socket_path).expect("connecting to the multiplexer");
    let mut connection = Connection::new(stream);
    connection
        .send(&Envelope {
            version: PROTOCOL_VERSION,
            token: "0".repeat(64),
            client_process_id: std::process::id(),
            client_id: zmux::messages::ClientId::default(),
            stream_only: false,
            session_secret: None,
            request: Request::List,
        })
        .expect("sending a request with the wrong token");

    match connection
        .receive::<Response>()
        .expect("reading the refusal")
        .0
    {
        Response::Error { message } => assert!(
            message.contains("invalid multiplexer token"),
            "unexpected refusal: {message}"
        ),
        other => panic!("the wrong token was not refused: {other:?}"),
    }
}
