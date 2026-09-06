use super::*;

#[test]
fn remote_targets_keep_open_ssh_destination_syntax_intact() {
    let target = RemoteTarget::new("alias.example").with_port(Some(2222));

    assert_eq!(target.destination(), "alias.example");
    assert_eq!(target.port(), Some(2222));
    assert!(target.validate().is_ok());
}

#[test]
fn remote_targets_reject_option_injection_and_empty_destinations() {
    assert!(RemoteTarget::new("").validate().is_err());
    assert!(RemoteTarget::new("-oProxyCommand=bad").validate().is_err());
    assert!(
        RemoteTarget::new("host")
            .with_port(Some(0))
            .validate()
            .is_err()
    );
}

#[test]
fn endpoint_queries_preserve_the_user_ssh_configuration() {
    let target = RemoteTarget::new("dev@example.test").with_port(Some(2222));

    assert_eq!(
        endpoint_arguments(&target),
        [
            "-T",
            "-p",
            "2222",
            "dev@example.test",
            r#"/bin/sh -c 'exec 3>&1 1>/dev/null; exec "${SHELL:-/bin/sh}" -lic "command zmux endpoint --json >&3"'"#,
        ]
    );
}

#[test]
fn remote_target_does_not_override_open_ssh_identity_selection() {
    let target = RemoteTarget::new("alias");

    assert!(
        !endpoint_arguments(&target)
            .iter()
            .any(|argument| argument == "-i")
    );
    assert!(
        !forward_arguments(&target, "/tmp/local.sock:/run/zmux.sock")
            .iter()
            .any(|argument| argument == "-i")
    );
}

#[test]
fn forwards_are_stream_local_and_do_not_request_a_shell() {
    let target = RemoteTarget::new("alias").with_port(Some(2200));

    assert_eq!(
        forward_arguments(&target, "/tmp/local.sock:/run/user/1000/zmux.sock"),
        [
            "-T",
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-p",
            "2200",
            "-L",
            "/tmp/local.sock:/run/user/1000/zmux.sock",
            "alias",
        ]
    );
}

#[cfg(unix)]
#[test]
fn mux_probe_uses_a_different_connection_than_the_real_request() {
    use std::{
        io::ErrorKind,
        os::unix::net::UnixListener,
        process::{Command, Stdio},
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("forward.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = crate::transport::Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: crate::messages::PROTOCOL_VERSION,
        process_id: 4242,
        socket_path: socket_path.clone(),
        token: "test-token".to_owned(),
    };
    let child = Command::new("sh")
        .args(["-c", "sleep 60"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let forward = ForwardState {
        child,
        directory,
        local_socket: socket_path,
        endpoint: endpoint.clone(),
    };
    let transport = RemoteTransport {
        target: RemoteTarget::new("test"),
        ssh_program: "ssh".into(),
        state: std::sync::Mutex::new(RemoteState {
            forward: Some(forward),
        }),
    };
    let (report_sender, report_receiver) = mpsc::channel::<Result<(), String>>();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        let first = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        report_sender
                            .send(Err("the probe connection was not opened".to_owned()))
                            .unwrap();
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => {
                    report_sender.send(Err(error.to_string())).unwrap();
                    return;
                }
            }
        };
        let mut first = Connection::new(first);
        let (request, _) = first.receive::<Envelope>().unwrap();
        assert!(matches!(request.request, Request::Ping));
        first.send(&Response::Ok).unwrap();
        drop(first);

        let second = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        report_sender
                            .send(Err(
                                "the real request reused the probe connection".to_owned()
                            ))
                            .unwrap();
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => {
                    report_sender.send(Err(error.to_string())).unwrap();
                    return;
                }
            }
        };
        report_sender.send(Ok(())).unwrap();
        let mut second = Connection::new(second);
        let (request, _) = second.receive::<Envelope>().unwrap();
        assert!(matches!(request.request, Request::List));
        second
            .send(&Response::Sessions {
                sessions: Vec::new(),
                restorable: Vec::new(),
            })
            .unwrap();
    });

    let (returned_endpoint, stream) = transport.connect().unwrap();
    assert_eq!(returned_endpoint, endpoint);
    let report = report_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert!(report.is_ok(), "{report:?}");
    let mut connection = Connection::new(stream);
    connection
        .send(&Envelope {
            version: crate::messages::PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id: std::process::id(),
            client_id: crate::messages::ClientId::new("test-client"),
            stream_only: true,
            session_secret: None,
            request: Request::List,
        })
        .unwrap();
    assert!(matches!(
        connection.receive::<Response>().unwrap().0,
        Response::Sessions { .. }
    ));
    server.join().unwrap();
}

#[test]
fn client_ids_are_random_and_serializable() {
    let first = crate::messages::ClientId::random().unwrap();
    let second = crate::messages::ClientId::random().unwrap();
    assert_ne!(first, second);
    assert_eq!(first.as_str().len(), 32);
    let wire = serde_json::to_string(&first).unwrap();
    assert_eq!(
        serde_json::from_str::<crate::messages::ClientId>(&wire).unwrap(),
        first
    );
}
