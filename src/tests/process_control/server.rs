use super::*;
use crate::process_control::client::send_open_window_request;
use crate::process_control::tests::request;
use futures::StreamExt as _;

#[test]
fn reconnect_results_use_distinct_control_statuses() {
    assert_eq!(
        reconnect_session_status(ReconnectSessionResult::AuthenticationFailed),
        "authentication_failed"
    );
    assert_eq!(
        reconnect_session_status(ReconnectSessionResult::SessionNotFound),
        "session_not_found"
    );
    assert_eq!(
        reconnect_session_status(ReconnectSessionResult::StillStarting),
        "session_starting"
    );
}

#[test]
fn an_authentication_failure_still_fits_the_completion_budget() {
    // A wrong secret costs one Argon2 verification. Guess-rate limiting refuses
    // early attempts rather than sleeping on them, precisely so it stays out of
    // this budget: a delay long enough to matter would exceed the timeout, and
    // `zmux reconnect` would report that Zetta refused the request
    // rather than that the secret was wrong. If verification alone ever
    // approaches the budget, raise the budget rather than the Argon2 cost.
    let authentication =
        crate::background_sessions::SessionAuthentication::create("secret").unwrap();
    let started = Instant::now();
    assert!(authentication.verify("wrong").is_none());
    let verification = started.elapsed();

    assert!(
        verification * 4 < RECONNECT_COMPLETION_TIMEOUT,
        "verification takes {verification:?}, too close to the \
         {RECONNECT_COMPLETION_TIMEOUT:?} reconnect budget"
    );
}

#[cfg(unix)]
#[test]
fn the_control_socket_is_not_reachable_by_other_users() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("sessions").join("control.json");
    let (commands, _received) = futures::channel::mpsc::unbounded();
    let server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();

    // Connecting to a Unix socket requires write permission on it, so the mode
    // is what keeps another local user off the control channel regardless of
    // the umask Zetta happened to inherit.
    let socket = fs::metadata(&server.socket_path).unwrap().permissions();
    assert_eq!(socket.mode() & 0o777, 0o600);

    let endpoint = fs::metadata(&endpoint_path).unwrap().permissions();
    assert_eq!(endpoint.mode() & 0o777, 0o600);

    let parent = fs::metadata(endpoint_path.parent().unwrap())
        .unwrap()
        .permissions();
    assert_eq!(parent.mode() & 0o777, 0o700);
}

#[test]
fn a_waiting_run_connection_does_not_block_other_control_requests() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let mut wait_stream = UnixStream::connect(&endpoint.socket_path).unwrap();
    let mut wait = request(&endpoint.token, "run_wait");
    wait.attention_id = Some(9);
    wait.pane_id = Some(4);
    wait.config_path = Some(
        serde_json::to_string(&RunWaitPayload {
            dependencies: vec!["api".to_owned()],
            allow_failure: false,
            command: vec!["echo".to_owned()],
        })
        .unwrap(),
    );
    write_message(&mut wait_stream, &wait).unwrap();

    let wait_command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::RunWait { completion, .. } = wait_command else {
        panic!("unexpected process control command");
    };

    let (next_command_sender, next_command_receiver) = channel();
    thread::spawn(move || {
        let command = futures::executor::block_on(received.next());
        let _ = next_command_sender.send(command);
    });
    let client_endpoint = endpoint.clone();
    let client = thread::spawn(move || send_open_window_request(&client_endpoint));
    let concurrent_command = next_command_receiver
        .recv_timeout(Duration::from_millis(500))
        .unwrap()
        .expect("the listener should accept the ordinary request concurrently");
    let ProcessControlCommand::OpenWindow {
        completion: open_completion,
    } = concurrent_command
    else {
        panic!("unexpected process control command");
    };
    open_completion.send(true).unwrap();
    assert!(client.join().unwrap().unwrap());

    completion
        .send(Err("test run rejection".to_owned()))
        .unwrap();
    drop(wait_stream);
}

#[test]
fn shutting_down_closes_a_waiting_run_connection() {
    use std::io::Read as _;

    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(endpoint_path).unwrap()).unwrap();

    let mut wait_stream = UnixStream::connect(&endpoint.socket_path).unwrap();
    let mut wait = request(&endpoint.token, "run_wait");
    wait.attention_id = Some(9);
    wait.pane_id = Some(4);
    wait.config_path = Some(
        serde_json::to_string(&RunWaitPayload {
            dependencies: vec!["api".to_owned()],
            allow_failure: false,
            command: vec!["echo".to_owned()],
        })
        .unwrap(),
    );
    write_message(&mut wait_stream, &wait).unwrap();

    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::RunWait { completion, .. } = command else {
        panic!("unexpected process control command");
    };
    wait_stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let reader = thread::spawn(move || {
        let mut byte = [0; 1];
        wait_stream.read(&mut byte)
    });

    server.begin_shutdown();
    let result = reader.join().unwrap();
    let closed = matches!(&result, Ok(0) | Err(_));
    assert!(
        closed,
        "the run connection remained open after shutdown: {result:?}"
    );
    drop(completion);
}

#[test]
fn shutdown_rejects_an_in_flight_window_handoff() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(&endpoint_path).unwrap()).unwrap();

    let client = thread::spawn(move || send_open_window_request(&endpoint).unwrap());
    let command = futures::executor::block_on(received.next()).unwrap();
    let ProcessControlCommand::OpenWindow {
        completion: _completion,
    } = command
    else {
        panic!("unexpected process control command");
    };
    server.begin_shutdown();

    assert!(!client.join().unwrap());
    assert!(!endpoint_path.exists());
    assert!(!server.is_accepting());
}

#[test]
fn a_request_that_does_not_decode_is_answered_rather_than_dropped() {
    // A refusal has to come back on the connection. A client that instead finds
    // the socket closed cannot tell "Zetta refused this" from "Zetta died", and
    // the decode returning `None` sits one early return away from doing exactly
    // that.
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control.json");
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    let _server = ProcessControlServer::start_at(commands, endpoint_path.clone()).unwrap();
    let endpoint: ControlEndpoint =
        serde_json::from_slice(&fs::read(&endpoint_path).unwrap()).unwrap();

    for mut rejected in [
        request("not-the-token", "open_window"),
        request(&endpoint.token, "no_such_command"),
    ] {
        rejected.attention_id = Some(42);
        let mut stream = UnixStream::connect(&endpoint.socket_path).unwrap();
        stream
            .set_read_timeout(Some(CONTROL_CLIENT_TIMEOUT))
            .unwrap();
        write_message(&mut stream, &rejected).unwrap();
        let response = read_message::<ControlResponse>(&mut stream).unwrap();
        assert_eq!(response.status, "rejected");
    }

    assert!(
        received.try_recv().is_err(),
        "a rejected request must not reach the window"
    );
}
