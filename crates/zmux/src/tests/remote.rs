use super::*;

#[test]
fn remote_program_paths_use_the_remote_hosts_posix_rules() {
    assert_eq!(
        parse_remote_program_path(b"/home/qodfanzksn/bin/zmux\n").unwrap(),
        PathBuf::from("/home/qodfanzksn/bin/zmux")
    );
    assert!(parse_remote_program_path(b"bin/zmux\n").is_err());
    assert!(parse_remote_program_path(b"~/bin/zmux\n").is_err());
}

#[cfg(unix)]
#[test]
fn remote_program_query_expands_a_home_shortened_path() {
    use std::os::unix::fs::PermissionsExt as _;

    let home = tempfile::tempdir().unwrap();
    let shell = home.path().join("shell");
    std::fs::write(
        &shell,
        "#!/bin/sh\nprintf 'startup noise\\n'\nprintf '~/bin/zmux\\n' >&3\n",
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(REMOTE_PROGRAM_COMMAND)
        .env("SHELL", &shell)
        .env("HOME", home.path())
        .env("PATH", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", home.path().join("bin/zmux").display())
    );
}

#[cfg(unix)]
#[test]
fn remote_queries_use_an_existing_noninteractive_zmux() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let zmux = directory.path().join("zmux");
    std::fs::write(&zmux, "#!/bin/sh\nprintf '[\"System\"]\\n'\n").unwrap();
    std::fs::set_permissions(&zmux, std::fs::Permissions::from_mode(0o700)).unwrap();
    let shell = directory.path().join("shell");
    std::fs::write(&shell, "#!/bin/sh\nexit 77\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(REMOTE_PROFILES_COMMAND)
        .env("PATH", directory.path())
        .env("SHELL", &shell)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"[\"System\"]\n");
}

#[test]
fn a_stale_endpoint_does_not_claim_a_remote_daemon_is_running() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("daemon.sock");
    let endpoint = Endpoint {
        version: ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: 1234,
        socket_path: socket.clone(),
        token: "test-token".to_owned(),
    };
    endpoint
        .write(&crate::server::endpoint_path(directory.path()))
        .unwrap();

    assert!(
        format!("{:#}", live_endpoint(directory.path()).unwrap_err())
            .contains("no multiplexer is running")
    );
    let listener = crate::transport::Listener::bind(&socket).unwrap();
    assert_eq!(live_endpoint(directory.path()).unwrap(), endpoint);
    drop(listener);
    assert!(live_endpoint(directory.path()).is_err());
}

#[cfg(unix)]
#[test]
fn stdio_proxy_copies_daemon_bytes_without_changing_them() {
    use std::io::{Cursor, Read as _, Write as _};

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("daemon.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 5];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(&request, b"\0\xffmux");
        stream.write_all(b"\xff\0reply").unwrap();
    });
    let stream = Stream::connect(&socket).unwrap();
    let mut output = Vec::new();
    proxy_stream(stream, Cursor::new(b"\0\xffmux".to_vec()), &mut output).unwrap();
    assert_eq!(output, b"\xff\0reply");
    server.join().unwrap();
}

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
            REMOTE_ENDPOINT_COMMAND,
        ]
    );
}

#[cfg(windows)]
#[test]
fn windows_stdio_proxy_uses_the_configured_ssh_target_without_a_forward() {
    let target = RemoteTarget::new("pi").with_port(Some(2222));
    let arguments = stdio_arguments(&target);
    assert_eq!(
        arguments[0..6],
        ["-T", "-o", "ClearAllForwardings=yes", "-p", "2222", "pi"]
    );
    assert_eq!(arguments[6], REMOTE_STDIO_COMMAND);
    assert!(!arguments.iter().any(|argument| argument == "-L"));
    assert!(!arguments.iter().any(|argument| argument == "-N"));

    let agent_off = stdio_arguments(&RemoteTarget::new("pi").with_forward_agent(false));
    assert!(agent_off.iter().any(|argument| argument == "-a"));
    assert!(!agent_off.iter().any(|argument| argument == "-A"));
}

#[cfg(windows)]
#[test]
fn windows_agent_holder_keeps_forwarded_agent_available_to_later_panes() {
    let target = RemoteTarget::new("pi").with_forward_agent(true);
    let arguments = agent_holder_arguments(
        &target,
        Path::new("/run/user/1000/zetta/forwarded-agent.sock"),
    );
    assert_eq!(
        arguments[0..5],
        ["-T", "-o", "ClearAllForwardings=yes", "-A", "pi"]
    );
    assert!(arguments[5].contains("SSH_AUTH_SOCK"));
    assert!(arguments[5].contains("exec sleep"));
    assert!(!arguments.iter().any(|argument| argument == "-L"));
}

#[cfg(windows)]
#[test]
fn windows_stdio_bridge_preserves_binary_bytes_and_eof() {
    use std::io::{Read, Write};

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("stdio.sock");
    let listener = crate::transport::Listener::bind(&socket).unwrap();
    let mut client = Stream::connect(&socket).unwrap();
    let (relay, _) = listener.accept().unwrap();
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OpenStandardInput().CopyTo([Console]::OpenStandardOutput())",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pump = thread::spawn(move || copy_stdio_child(&mut child, relay, "test"));
    let payload = b"\0\xff\r\nzmux\x1b[31m";
    client.write_all(payload).unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    let mut echoed = Vec::new();
    client.read_to_end(&mut echoed).unwrap();
    assert_eq!(echoed, payload);
    pump.join().unwrap();
}

#[cfg(windows)]
#[test]
fn windows_stdio_bridge_reports_the_remote_process_error() {
    use std::io::Read as _;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("stdio.sock");
    let listener = crate::transport::Listener::bind(&socket).unwrap();
    let mut client = Stream::connect(&socket).unwrap();
    let (relay, _) = listener.accept().unwrap();
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Error.WriteLine('remote proxy failed'); exit 27",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pump = thread::spawn(move || copy_stdio_child(&mut child, relay, "test"));
    let mut output = Vec::new();
    client.read_to_end(&mut output).unwrap();
    assert!(output.is_empty());
    assert!(pump.join().unwrap().contains("remote proxy failed"));
}

#[cfg(windows)]
#[test]
fn windows_stdio_bridge_returns_a_response_before_the_client_closes() {
    use std::io::{Read as _, Write as _};

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("stdio.sock");
    let ready = directory.path().join("ready");
    let listener = crate::transport::Listener::bind(&socket).unwrap();
    let mut client = Stream::connect(&socket).unwrap();
    let (relay, _) = listener.accept().unwrap();
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[IO.File]::WriteAllText($env:ZETTA_TEST_READY,'ready'); $b=New-Object byte[] 5; $n=[Console]::OpenStandardInput().Read($b,0,5); [Console]::OpenStandardOutput().Write($b,0,$n)",
        ])
        .env("ZETTA_TEST_READY", &ready)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pump = thread::spawn(move || copy_stdio_child(&mut child, relay, "test"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() {
        assert!(Instant::now() < deadline, "the echo process did not start");
        thread::sleep(Duration::from_millis(10));
    }
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    client.write_all(b"hello").unwrap();
    let mut response = [0; 5];
    client.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"hello");
    drop(client);
    pump.join().unwrap();
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
        !forward_arguments(&target, "/tmp/local.sock:/run/zmux.sock", None)
            .iter()
            .any(|argument| argument == "-i")
    );
}

#[test]
fn remote_targets_explicitly_control_native_agent_forwarding() {
    let enabled = RemoteTarget::new("alias").with_forward_agent(true);
    let disabled = RemoteTarget::new("alias").with_forward_agent(false);

    for arguments in [
        endpoint_arguments(&enabled),
        program_arguments(&enabled),
        profiles_arguments(&enabled),
        start_daemon_arguments(&enabled, Path::new("/tmp/zmux")),
        forward_arguments(
            &enabled,
            "/tmp/local.sock:/run/zmux.sock",
            Some(Path::new("/run/zetta/forwarded-agent.sock")),
        ),
    ] {
        assert!(arguments.iter().any(|argument| argument == "-A"));
        assert!(!arguments.iter().any(|argument| argument == "-a"));
    }
    for arguments in [
        endpoint_arguments(&disabled),
        program_arguments(&disabled),
        profiles_arguments(&disabled),
        start_daemon_arguments(&disabled, Path::new("/tmp/zmux")),
        forward_arguments(&disabled, "/tmp/local.sock:/run/zmux.sock", None),
    ] {
        assert!(arguments.iter().any(|argument| argument == "-a"));
        assert!(!arguments.iter().any(|argument| argument == "-A"));
    }
}

#[test]
fn forwards_are_stream_local_and_do_not_request_a_shell() {
    let target = RemoteTarget::new("alias").with_port(Some(2200));

    assert_eq!(
        forward_arguments(&target, "/tmp/local.sock:/run/user/1000/zmux.sock", None,),
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

#[test]
fn an_agent_forward_keeps_a_remote_session_channel_open() {
    let target = RemoteTarget::new("alias").with_forward_agent(true);

    let arguments = forward_arguments(
        &target,
        "/tmp/local.sock:/run/user/1000/zmux.sock",
        Some(Path::new("/run/user/1000/zetta/forwarded-agent.sock")),
    );

    assert!(arguments.iter().any(|argument| argument == "-A"));
    assert!(!arguments.iter().any(|argument| argument == "-N"));
    let command = arguments.last().expect("the remote holder command");
    assert!(command.contains("SSH_AUTH_SOCK"));
    assert!(command.contains("/run/user/1000/zetta/forwarded-agent.sock"));
    assert!(command.contains("exec sleep"));
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
    // Darwin can carry the listener's nonblocking mode onto accepted sockets;
    // reset each one below because the framed protocol reader expects blocking
    // I/O once the polling accept loop has handed a connection over.
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
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    break stream;
                }
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
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    break stream;
                }
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

#[cfg(unix)]
#[test]
fn remote_attach_uses_the_transport_probe_as_its_only_readiness_check() {
    use std::{
        os::unix::net::UnixListener,
        process::{Command, Stdio},
        sync::Arc,
        thread,
    };

    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("forward.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
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
    let transport = RemoteTransport {
        target: RemoteTarget::new("test"),
        ssh_program: "ssh".into(),
        state: std::sync::Mutex::new(RemoteState {
            forward: Some(ForwardState {
                child,
                directory,
                local_socket: socket_path,
                endpoint: endpoint.clone(),
            }),
        }),
    };
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut probe = Connection::new(stream);
        let (request, _) = probe.receive::<Envelope>().unwrap();
        assert!(matches!(request.request, Request::Ping));
        probe.send(&Response::Ok).unwrap();

        let (stream, _) = listener.accept().unwrap();
        let mut request = Connection::new(stream);
        let (envelope, _) = request.receive::<Envelope>().unwrap();
        assert!(matches!(envelope.request, Request::Attach { .. }));
        request
            .send(&Response::SharedAttached {
                pane_id: 2,
                child_pid: 3,
                replay_length: 0,
                state: serde_json::Value::Null,
                summary: Box::new(crate::protocol::BackgroundSessionSummary {
                    id: 1,
                    title: "remote".to_owned(),
                    authentication_required: false,
                    active_pane: 2,
                    layout: crate::protocol::BackgroundPaneLayout::Pane { pane_id: 2 },
                    panes: Vec::new(),
                    held: false,
                    scoped_to: None,
                    key_envelope: None,
                }),
                columns: 80,
                lines: 24,
            })
            .unwrap();
    });

    let client =
        crate::client::Client::from_remote_transport_for_test(Arc::new(transport), endpoint);
    assert!(matches!(
        client.attach_with_secret(1, None, None).unwrap(),
        crate::client::AttachOutcome::SharedAttached { .. }
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
