//! Who may control a session on Windows: a protected session's own window, a
//! claimed owner without the peer's authority, and the wrong token.

use super::*;

/// The verifier for a protected test session, as a client creates one before
/// sharing.
fn test_verifier() -> zmux::auth::SessionAuthentication {
    zmux::auth::SessionAuthentication::create("correct horse battery staple")
        .expect("creating the session verifier")
}

/// Set in the subprocess that plays the impostor; holds an [`ImpostorRun`].
const IMPOSTOR_ENV: &str = "ZMUX_TEST_IMPOSTOR";

/// What the impostor subprocess is asked to do, and where it reports back.
#[derive(serde::Serialize, serde::Deserialize)]
struct ImpostorRun {
    endpoint: PathBuf,
    claimed_process_id: u32,
    requests: Vec<zmux::messages::Request>,
    answers: PathBuf,
}

/// Sends `requests` from another process claiming to be `claimed_process_id`,
/// and returns the daemon's answers in order.
///
/// It has to be another process: the daemon asks the kernel which process
/// connected, so a request from this one would really be from the owner.
fn impostor_requests(
    daemon: &TestDaemon,
    claimed_process_id: u32,
    requests: Vec<zmux::messages::Request>,
) -> Vec<zmux::messages::Response> {
    let answers = daemon.config.join("impostor-answers.json");
    let run = ImpostorRun {
        endpoint: daemon.sessions_dir().join("zmux.json"),
        claimed_process_id,
        requests,
        answers: answers.clone(),
    };
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "protection::impostor_subprocess", "--nocapture"])
        .env(IMPOSTOR_ENV, serde_json::to_string(&run).unwrap())
        .stdin(Stdio::null())
        .output()
        .expect("starting the impostor process");
    assert!(
        output.status.success(),
        "the impostor process failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&std::fs::read(&answers).expect("reading the impostor's answers"))
        .expect("parsing the impostor's answers")
}

/// The impostor itself, run by [`impostor_requests`] and a no-op otherwise.
#[test]
fn impostor_subprocess() {
    let Ok(run) = std::env::var(IMPOSTOR_ENV) else {
        return;
    };
    let run: ImpostorRun = serde_json::from_str(&run).expect("parsing the impostor's task");
    let answers: Vec<_> = run
        .requests
        .into_iter()
        .map(|request| impostor_request(&run.endpoint, run.claimed_process_id, request))
        .collect();
    std::fs::write(&run.answers, serde_json::to_vec(&answers).unwrap())
        .expect("writing the impostor's answers");
}

/// Speaks the protocol directly, as any same-user process that can read the
/// endpoint token could, naming whichever process it likes as its own and
/// answering any challenge with that process's nonce.
fn impostor_request(
    endpoint: &Path,
    claimed_process_id: u32,
    request: zmux::messages::Request,
) -> zmux::messages::Response {
    use zmux::{
        messages::{Envelope, PROTOCOL_VERSION, Request, Response},
        transport::{Connection, Endpoint, Stream},
    };

    let endpoint = Endpoint::read(endpoint).expect("reading the multiplexer endpoint");
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
    let Response::AttestationRequired { handle } = response else {
        return response;
    };
    // Only where the kernel does not report the peer. The handle lives in the
    // process being impersonated, so read it the way any same-user impostor
    // could: open that process, copy the handle out and answer correctly.
    connection
        .send(&Request::Attested {
            nonce: steal_nonce(claimed_process_id, handle),
        })
        .expect("answering the attestation challenge");
    connection
        .receive::<Response>()
        .expect("reading the answer after the attestation")
        .0
}

/// Reads the nonce behind a challenge `handle` issued to another process of
/// this user, which a same-user process of the same integrity can always do.
fn steal_nonce(victim: u32, handle: i64) -> String {
    use std::io::Read as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};

    // SAFETY: the access right is valid for `OpenProcess`.
    let opened = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, victim) }
        .expect("opening the impersonated process");
    // SAFETY: `OpenProcess` succeeded, so this function owns the handle.
    let victim = unsafe { OwnedHandle::from_raw_handle(opened.0) };
    let mut stolen = HANDLE::default();
    // SAFETY: `victim` is open with duplication access, `handle` is the value
    // the daemon duplicated into it, and `stolen` is a writable slot.
    unsafe {
        DuplicateHandle(
            HANDLE(victim.as_raw_handle()),
            HANDLE(handle as *mut _),
            GetCurrentProcess(),
            &mut stolen,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    }
    .expect("copying the challenge handle out of the impersonated process");
    // SAFETY: `DuplicateHandle` gave this process its own handle.
    let pipe = unsafe { OwnedHandle::from_raw_handle(stolen.0) };
    let mut nonce = String::new();
    std::fs::File::from(pipe)
        .read_to_string(&mut nonce)
        .expect("reading the stolen nonce");
    nonce
}

/// A same-user process that can read the endpoint token, naming a protected
/// session's owner as itself, controls nothing: the daemon asks the kernel
/// which process connected rather than taking the envelope's word for it.
/// Without that, naming the owner was all it took to detach, kill or rescope
/// the session — the endpoint token would have been an administrator
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
    // requests below come from another process claiming to be this one.
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

    let controls = vec![
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
    ];
    let names: Vec<_> = controls
        .iter()
        .map(|request| format!("{request:?}"))
        .collect();
    let mut requests = controls;
    requests.push(Request::PaneStates {
        pane_ids: vec![pane_id],
    });
    let mut answers = impostor_requests(&daemon, std::process::id(), requests).into_iter();

    for name in names {
        match answers.next().expect("an answer for every control") {
            Response::Error { message } => assert!(
                message.contains("protected"),
                "unexpected authorization error for {name}: {message}"
            ),
            other => panic!("{name} was not refused: {other:?}"),
        }
    }
    match answers.next().expect("an answer for the pane states") {
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
/// names still gets through, because the kernel names it too.
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
