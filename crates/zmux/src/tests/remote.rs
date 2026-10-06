use super::*;

/// Writes an executable test script and waits until it can be run.
///
/// A file just written can briefly refuse to execute (`ETXTBSY`): a process
/// another test forks while the file is still open for writing inherits that
/// descriptor until it execs. So run the script once, harmlessly, until the
/// kernel lets it — after the first success no writer can appear again. The
/// guard line is what makes that run harmless.
#[cfg(unix)]
pub(super) fn write_script(path: &Path, content: impl AsRef<str>) {
    use std::os::unix::fs::PermissionsExt as _;

    const PROBE: &str = "--zetta-test-probe";
    let content = content.as_ref();
    let body = content
        .strip_prefix("#!/bin/sh\n")
        .expect("test scripts are /bin/sh scripts");
    std::fs::write(
        path,
        format!("#!/bin/sh\ntest \"$1\" = {PROBE} && exit 0\n{body}"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let run = Command::new(path)
            .arg(PROBE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match run {
            Err(error) if error.raw_os_error() == Some(26) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            run => {
                run.unwrap();
                return;
            }
        }
    }
}

#[test]
fn remote_program_paths_use_the_remote_hosts_posix_rules() {
    assert_eq!(
        parse_remote_program_path(b"/home/qodfanzksn/bin/zmux\n", HostPlatform::Posix).unwrap(),
        PathBuf::from("/home/qodfanzksn/bin/zmux")
    );
    assert!(parse_remote_program_path(b"bin/zmux\n", HostPlatform::Posix).is_err());
    assert!(parse_remote_program_path(b"~/bin/zmux\n", HostPlatform::Posix).is_err());
    assert!(parse_remote_program_path(b"C:\\Zetta\\zmux.exe\r\n", HostPlatform::Posix).is_err());
}

#[test]
fn remote_program_paths_on_a_windows_host_use_its_rules() {
    assert_eq!(
        parse_remote_program_path(
            b"C:\\Users\\dev\\AppData\\Local\\Programs\\Zetta\\zmux.exe\r\n",
            HostPlatform::Windows
        )
        .unwrap(),
        PathBuf::from(r"C:\Users\dev\AppData\Local\Programs\Zetta\zmux.exe")
    );
    assert!(parse_remote_program_path(b"/usr/bin/zmux\n", HostPlatform::Windows).is_err());
}

#[cfg(unix)]
#[test]
fn remote_program_query_expands_a_home_shortened_path() {
    let home = tempfile::tempdir().unwrap();
    let shell = home.path().join("shell");
    write_script(
        &shell,
        "#!/bin/sh\nprintf 'startup noise\\n'\nprintf '~/bin/zmux\\n' >&3\n",
    );

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(remote_program_command())
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
    let directory = tempfile::tempdir().unwrap();
    let zmux = directory.path().join("zmux");
    write_script(&zmux, "#!/bin/sh\nprintf '[\"System\"]\\n'\n");
    let shell = directory.path().join("shell");
    write_script(&shell, "#!/bin/sh\nexit 77\n");

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
    // A process another test forks while the listener is open holds a copy of
    // it until that child execs, so the socket can accept for a moment after
    // the drop. It stops once that copy is gone.
    let deadline = Instant::now() + Duration::from_secs(5);
    while live_endpoint(directory.path()).is_ok() {
        assert!(
            Instant::now() < deadline,
            "a dropped listener still reads as a live daemon"
        );
        thread::sleep(Duration::from_millis(10));
    }
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
        endpoint_arguments(&target, None),
        [
            "-T".to_owned(),
            "-p".to_owned(),
            "2222".to_owned(),
            "dev@example.test".to_owned(),
            remote_endpoint_command(),
        ]
    );
}

#[test]
fn commands_on_a_shared_login_name_its_socket_and_add_no_forwards() {
    let target = RemoteTarget::new("alias").with_port(Some(2222));
    let control = Path::new("/tmp/zetta-zmux-x/ctl");

    let mut commands = vec![
        endpoint_arguments(&target, Some(control)),
        agent_holder_arguments(&target, control, Path::new("/run/agent.sock")),
    ];
    for platform in [HostPlatform::Posix, HostPlatform::Windows] {
        commands.extend([
            program_arguments(&target, Some(control), platform),
            profiles_arguments(&target, Some(control), platform),
            start_daemon_arguments(&target, Some(control), Path::new("/opt/zmux"), platform),
            bridge_arguments(&target, Some(control), platform),
        ]);
    }
    for arguments in commands {
        assert_eq!(
            arguments[..7],
            [
                "-T",
                "-S",
                "/tmp/zetta-zmux-x/ctl",
                "-o",
                "ControlMaster=no",
                "-o",
                "ClearAllForwardings=yes",
            ]
        );
        assert_eq!(arguments[7..9], ["-p", "2222"]);
        assert_eq!(arguments[9], "alias");
    }
}

#[test]
fn the_login_is_a_foreground_master_that_runs_nothing() {
    let target = RemoteTarget::new("alias").with_port(Some(2200));

    assert_eq!(
        master_arguments(&target, Path::new("/tmp/zetta-zmux-x/ctl")),
        [
            "-T",
            "-N",
            "-M",
            "-S",
            "/tmp/zetta-zmux-x/ctl",
            "-o",
            "ControlPersist=no",
            "-o",
            "ExitOnForwardFailure=yes",
            "-p",
            "2200",
            "alias",
        ]
    );
}

#[test]
fn forwards_are_stream_local_requests_to_the_master() {
    let target = RemoteTarget::new("alias").with_port(Some(2200));

    assert_eq!(
        forward_request_arguments(
            &target,
            Path::new("/tmp/zetta-zmux-x/ctl"),
            "forward",
            "/tmp/zetta-zmux-x/mux-0.sock:/run/user/1000/zmux.sock",
        ),
        [
            "-S",
            "/tmp/zetta-zmux-x/ctl",
            "-O",
            "forward",
            "-L",
            "/tmp/zetta-zmux-x/mux-0.sock:/run/user/1000/zmux.sock",
            "alias",
        ]
    );
}

#[test]
fn remote_target_does_not_override_open_ssh_identity_selection() {
    let target = RemoteTarget::new("alias");

    for arguments in [
        endpoint_arguments(&target, None),
        master_arguments(&target, Path::new("/tmp/ctl")),
        bridge_arguments(&target, None, HostPlatform::Posix),
    ] {
        assert!(!arguments.iter().any(|argument| argument == "-i"));
    }
}

#[test]
fn remote_targets_explicitly_control_native_agent_forwarding() {
    let enabled = RemoteTarget::new("alias").with_forward_agent(true);
    let disabled = RemoteTarget::new("alias").with_forward_agent(false);
    let control = Path::new("/tmp/ctl");
    let socket = Path::new("/run/zetta/forwarded-agent.sock");

    for arguments in [
        endpoint_arguments(&enabled, None),
        program_arguments(&enabled, Some(control), HostPlatform::Posix),
        profiles_arguments(&enabled, None, HostPlatform::Windows),
        start_daemon_arguments(
            &enabled,
            Some(control),
            Path::new("/tmp/zmux"),
            HostPlatform::Posix,
        ),
        master_arguments(&enabled, control),
        agent_holder_arguments(&enabled, control, socket),
        bridge_arguments(&enabled, None, HostPlatform::Posix),
        bridge_arguments(&enabled, Some(control), HostPlatform::Windows),
    ] {
        assert!(arguments.iter().any(|argument| argument == "-A"));
        assert!(!arguments.iter().any(|argument| argument == "-a"));
    }
    for arguments in [
        endpoint_arguments(&disabled, None),
        program_arguments(&disabled, Some(control), HostPlatform::Windows),
        profiles_arguments(&disabled, None, HostPlatform::Posix),
        start_daemon_arguments(
            &disabled,
            Some(control),
            Path::new("/tmp/zmux"),
            HostPlatform::Windows,
        ),
        master_arguments(&disabled, control),
        bridge_arguments(&disabled, None, HostPlatform::Posix),
    ] {
        assert!(arguments.iter().any(|argument| argument == "-a"));
        assert!(!arguments.iter().any(|argument| argument == "-A"));
    }
}

#[test]
fn an_agent_holder_is_a_session_on_the_shared_login() {
    let target = RemoteTarget::new("alias").with_forward_agent(true);

    let arguments = agent_holder_arguments(
        &target,
        Path::new("/tmp/ctl"),
        Path::new("/run/user/1000/zetta/forwarded-agent.sock"),
    );

    assert!(arguments.iter().any(|argument| argument == "-A"));
    assert!(arguments.windows(2).any(|pair| pair == ["-S", "/tmp/ctl"]));
    let command = arguments.last().expect("the remote holder command");
    assert!(command.contains("SSH_AUTH_SOCK"));
    assert!(command.contains("/run/user/1000/zetta/forwarded-agent.sock"));
    assert!(command.contains("exec sleep"));
}

#[test]
fn the_windows_bridge_is_one_login_without_forwards() {
    let target = RemoteTarget::new("pi").with_port(Some(2222));
    let arguments = bridge_arguments(&target, None, HostPlatform::Posix);
    assert_eq!(
        arguments[0..6],
        ["-T", "-o", "ClearAllForwardings=yes", "-p", "2222", "pi"]
    );
    assert_eq!(arguments[6], remote_bridge_command(false));
    assert!(arguments[6].contains("proxy-mux"));
    assert!(!arguments[6].contains("--forward-agent"));
    assert!(!arguments.iter().any(|argument| argument == "-L"));
    assert!(!arguments.iter().any(|argument| argument == "-N"));

    let linked = bridge_arguments(
        &RemoteTarget::new("pi").with_forward_agent(true),
        None,
        HostPlatform::Posix,
    );
    assert!(linked.last().unwrap().contains("proxy-mux --forward-agent"));
}

#[test]
fn a_windows_host_is_sent_powershell_rather_than_a_posix_wrapper() {
    let target = RemoteTarget::new("thinkpad");
    let control = Path::new("/tmp/ctl");
    for arguments in [
        program_arguments(&target, Some(control), HostPlatform::Windows),
        profiles_arguments(&target, None, HostPlatform::Windows),
        start_daemon_arguments(
            &target,
            Some(control),
            Path::new(r"C:\Zetta\zmux.exe"),
            HostPlatform::Windows,
        ),
        bridge_arguments(&target, Some(control), HostPlatform::Windows),
        bridge_arguments(&target, None, HostPlatform::Windows),
    ] {
        let command = arguments.last().unwrap();
        assert!(command.starts_with("powershell.exe "), "{command}");
        assert!(!command.contains("/bin/sh"), "{command}");
    }
    let script = remote_host::decoded(
        bridge_arguments(&target, None, HostPlatform::Windows)
            .last()
            .unwrap(),
    )
    .unwrap();
    assert!(script.contains("proxy-mux"), "{script}");
}

#[test]
fn the_endpoint_query_names_the_program_before_the_endpoint() {
    let endpoint = Endpoint {
        version: ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: 7,
        socket_path: PathBuf::from("/run/user/1000/zetta/zmux.sock"),
        token: "token".to_owned(),
    };
    let output = format!(
        "\n/home/dev/.local/bin/zmux\n{}\n",
        serde_json::to_string(&endpoint).unwrap()
    );
    assert_eq!(
        parse_endpoint_output(output.as_bytes()).unwrap(),
        (PathBuf::from("/home/dev/.local/bin/zmux"), endpoint.clone())
    );
    assert!(parse_endpoint_output(b"/home/dev/zmux\n").is_err());
    assert!(parse_endpoint_output(b"zmux\n{}\n").is_err());
    let mismatched = Endpoint {
        protocol_version: PROTOCOL_VERSION + 1,
        ..endpoint
    };
    let output = format!(
        "/home/dev/zmux\n{}\n",
        serde_json::to_string(&mismatched).unwrap()
    );
    assert!(parse_endpoint_output(output.as_bytes()).is_err());
}

/// The rc-file fallback is only used to find `zmux`: what it found is run
/// directly, so a slow rc file runs once per query rather than twice.
#[cfg(unix)]
#[test]
fn the_endpoint_query_runs_the_program_an_interactive_shell_found() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let zmux = bin.join("zmux");
    write_script(
        &zmux,
        "#!/bin/sh\ntest \"$1 $2\" = 'endpoint --json' && printf '{\"endpoint\":1}\\n'\n",
    );
    let runs = home.path().join("runs");
    let shell = home.path().join("shell");
    write_script(
        &shell,
        format!(
            "#!/bin/sh\necho run >> '{}'\nprintf 'startup noise\\n'\nprintf '~/bin/zmux\\n' >&3\n",
            runs.display()
        ),
    );

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(remote_endpoint_command())
        .env("SHELL", &shell)
        .env("HOME", home.path())
        .env("PATH", home.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n{{\"endpoint\":1}}\n", zmux.display())
    );
    assert_eq!(std::fs::read_to_string(&runs).unwrap(), "run\n");
}

#[test]
fn shared_transports_are_one_per_target() {
    keep_idle_transports();
    let first = RemoteTransport::shared(RemoteTarget::new("shared-one.invalid")).unwrap();
    let again = RemoteTransport::shared(RemoteTarget::new("shared-one.invalid")).unwrap();
    let other_port =
        RemoteTransport::shared(RemoteTarget::new("shared-one.invalid").with_port(Some(2222)))
            .unwrap();
    let other_agent =
        RemoteTransport::shared(RemoteTarget::new("shared-one.invalid").with_forward_agent(true))
            .unwrap();

    assert!(Arc::ptr_eq(&first, &again));
    assert!(!Arc::ptr_eq(&first, &other_port));
    assert!(!Arc::ptr_eq(&first, &other_agent));
    assert!(RemoteTransport::shared(RemoteTarget::new("-oProxyCommand=x")).is_err());

    // Released, a process no longer shares: nothing it made outlives it.
    release_idle_transports();
    let unshared = RemoteTransport::shared(RemoteTarget::new("shared-one.invalid")).unwrap();
    assert!(!Arc::ptr_eq(&first, &unshared));
    assert_eq!(Arc::strong_count(&first), 2, "the registry let go of it");
}

/// A transport whose login and forward are already up: the login is a
/// stand-in process, and the forward's local socket is whatever the test
/// listens on.
#[cfg(unix)]
fn transport_with_forward(
    directory: tempfile::TempDir,
    local_socket: PathBuf,
    endpoint: Endpoint,
) -> RemoteTransport {
    let child = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut state = RemoteState::empty();
    state.master = Some(Master {
        child,
        control_path: directory.path().join("ctl"),
        directory,
        stderr: CapturedOutput::default(),
    });
    state.forward = Some(ForwardState {
        local_socket,
        forwarding: String::new(),
        endpoint,
        agent_holder: None,
        verified_at: None,
    });
    RemoteTransport {
        target: RemoteTarget::new("test"),
        ssh_program: "ssh".into(),
        state: Mutex::new(state),
    }
}

#[cfg(unix)]
#[test]
fn mux_probe_uses_a_different_connection_than_the_real_request() {
    use std::{
        io::ErrorKind,
        os::unix::net::UnixListener,
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
    let transport = transport_with_forward(directory, socket_path, endpoint.clone());
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
    use std::{os::unix::net::UnixListener, sync::Arc, thread};

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
    let transport = transport_with_forward(directory, socket_path, endpoint.clone());
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

/// A stand-in daemon that answers `Ping` and `List`, counting each.
#[cfg(unix)]
struct CountingDaemon {
    socket: PathBuf,
    pings: Arc<std::sync::atomic::AtomicUsize>,
    lists: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(unix)]
impl CountingDaemon {
    fn start(directory: &Path) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let socket = directory.join("daemon.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let pings = Arc::new(AtomicUsize::new(0));
        let lists = Arc::new(AtomicUsize::new(0));
        let (ping_counter, list_counter) = (pings.clone(), lists.clone());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut connection = Connection::new(stream);
                let Ok((envelope, _)) = connection.receive::<Envelope>() else {
                    continue;
                };
                let response = match envelope.request {
                    Request::Ping => {
                        ping_counter.fetch_add(1, Ordering::SeqCst);
                        Response::Ok
                    }
                    Request::List => {
                        list_counter.fetch_add(1, Ordering::SeqCst);
                        Response::Sessions {
                            sessions: Vec::new(),
                            restorable: Vec::new(),
                        }
                    }
                    _ => Response::Error {
                        message: "unexpected request".to_owned(),
                    },
                };
                let _ = connection.send(&response);
            }
        });
        Self {
            socket,
            pings,
            lists,
        }
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint {
            version: ENDPOINT_VERSION,
            protocol_version: PROTOCOL_VERSION,
            process_id: 4242,
            socket_path: self.socket.clone(),
            token: "test-token".to_owned(),
        }
    }

    fn pings(&self) -> usize {
        self.pings.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn lists(&self) -> usize {
        self.lists.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(unix)]
#[test]
fn a_proven_endpoint_is_not_probed_again_until_something_fails() {
    let directory = tempfile::tempdir().unwrap();
    let daemon = CountingDaemon::start(directory.path());
    let endpoint = daemon.endpoint();
    let transport = Arc::new(transport_with_forward(
        tempfile::tempdir().unwrap(),
        daemon.socket.clone(),
        endpoint.clone(),
    ));
    let client = crate::client::Client::from_remote_transport_for_test(transport.clone(), endpoint);

    for _ in 0..5 {
        client.list().unwrap();
    }
    assert_eq!(daemon.lists(), 5);
    assert_eq!(
        daemon.pings(),
        1,
        "only the first connection needed a probe"
    );

    transport.distrust();
    client.list().unwrap();
    assert_eq!(daemon.pings(), 2, "a distrusted endpoint is probed again");
}

/// An `ssh` that logs what it was asked to do and does just enough of it: a
/// master creates its control path and waits, `-O forward` links the local
/// socket to the remote one, and anything else runs the remote command
/// locally, with a `zmux` on `PATH` that reports `endpoint`.
#[cfg(unix)]
pub(super) fn fake_ssh(directory: &Path, endpoint: &Endpoint) -> (PathBuf, PathBuf) {
    let bin = directory.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let zmux = bin.join("zmux");
    write_script(
        &zmux,
        format!(
            "#!/bin/sh\ncase \"$1\" in endpoint) printf '%s\\n' '{}';; profiles) printf '[\"System\"]\\n';; esac\n",
            serde_json::to_string(endpoint).unwrap()
        ),
    );
    let log = directory.join("ssh.log");
    let ssh = directory.join("ssh");
    write_script(
        &ssh,
        format!(
            r#"#!/bin/sh
log='{log}'
control=''; operation=''; forwarding=''; master=0; previous=''; last=''
for argument in "$@"; do
  case "$previous" in -S) control="$argument";; -O) operation="$argument";; -L) forwarding="$argument";; esac
  test "$argument" = -M && master=1
  previous="$argument"; last="$argument"
done
if test "$master" = 1; then echo master >> "$log"; : > "$control"; exec sleep 60; fi
if test -n "$operation"; then
  echo "$operation" >> "$log"
  test "$operation" = forward && ln -s "${{forwarding#*:}}" "${{forwarding%%:*}}"
  test "$operation" = cancel && rm -f "${{forwarding%%:*}}"
  exit 0
fi
test -n "$control" && echo shared-command >> "$log" || echo login-command >> "$log"
PATH='{bin}':"$PATH" exec /bin/sh -c "$last"
"#,
            log = log.display(),
            bin = bin.display(),
        ),
    );
    (ssh, log)
}

/// What this whole transport exists to do: one login, however many requests.
#[cfg(unix)]
#[test]
fn connecting_and_requesting_costs_one_login() {
    let directory = tempfile::tempdir().unwrap();
    let daemon = CountingDaemon::start(directory.path());
    let (ssh, log) = fake_ssh(directory.path(), &daemon.endpoint());
    let transport = Arc::new(
        RemoteTransport::for_creation_with_ssh_program(RemoteTarget::new("fake"), &ssh).unwrap(),
    );

    let endpoint = transport.ensure_endpoint().unwrap();
    assert!(transport.control_path().is_some());
    let client = crate::client::Client::from_remote_transport_for_test(transport.clone(), endpoint);
    for _ in 0..4 {
        client.list().unwrap();
    }
    assert_eq!(
        transport.resolve_remote_program().unwrap(),
        directory.path().join("bin/zmux"),
        "the program comes back with the endpoint"
    );
    assert_eq!(transport.query_profiles().unwrap(), ["System"]);

    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        ["master", "shared-command", "forward", "shared-command"],
        "one login, the endpoint query, the forward, and the profile query"
    );
    assert_eq!(daemon.lists(), 4);
    assert_eq!(daemon.pings(), 1);

    // A refresh re-reads the endpoint over the same login.
    transport.refresh().unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.lines().skip(4).collect::<Vec<_>>(),
        ["cancel", "shared-command", "forward"]
    );
}

/// An `ssh` to a Windows host whose account shell is PowerShell: a master
/// works as it does anywhere, a `/bin/sh` command fails the way PowerShell
/// fails it, the platform probe expands `$env:OS`, and an encoded script that
/// asks for profiles gets them.
#[cfg(unix)]
fn fake_windows_ssh(directory: &Path) -> (PathBuf, PathBuf) {
    let log = directory.join("ssh.log");
    let ssh = directory.join("ssh");
    write_script(
        &ssh,
        format!(
            r#"#!/bin/sh
log='{log}'
control=''; master=0; previous=''; last=''
for argument in "$@"; do
  test "$previous" = -S && control="$argument"
  test "$argument" = -M && master=1
  previous="$argument"; last="$argument"
done
if test "$master" = 1; then echo master >> "$log"; : > "$control"; exec sleep 60; fi
case "$last" in
  /bin/sh*) echo posix >> "$log"; echo "/bin/sh : The term '/bin/sh' is not recognized" >&2; exit 1;;
  "{probe}") echo probe >> "$log"; printf 'ZMUX_OS_A%%OS%%\r\nZMUX_OS_BWindows_NT\r\n'; exit 0;;
  powershell.exe*)
    echo powershell >> "$log"
    script=$(printf '%s' "${{last##* }}" | base64 -d | iconv -f UTF-16LE -t UTF-8)
    case "$script" in *"profiles --json"*) printf '["Windows PowerShell","Command Prompt"]\r\n'; exit 0;; esac
    exit 1;;
esac
exit 1
"#,
            log = log.display(),
            probe = remote_host::PLATFORM_PROBE.replace('$', "\\$"),
        ),
    );
    (ssh, log)
}

#[cfg(unix)]
#[test]
fn a_windows_host_is_found_out_once_and_then_sent_powershell() {
    let directory = tempfile::tempdir().unwrap();
    let (ssh, log) = fake_windows_ssh(directory.path());
    let target = RemoteTarget::new("windows-host-detection-test");
    let transport = RemoteTransport::for_creation_with_ssh_program(target.clone(), &ssh).unwrap();

    assert_eq!(
        transport.query_profiles().unwrap(),
        ["Command Prompt", "Windows PowerShell"]
    );
    assert_eq!(remote_host::learned(&target), Some(HostPlatform::Windows));
    assert_eq!(
        transport.query_profiles().unwrap(),
        ["Command Prompt", "Windows PowerShell"]
    );

    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        ["master", "posix", "probe", "powershell", "powershell"],
        "the POSIX form is tried once, and never again once the host is known"
    );
}

#[cfg(unix)]
#[test]
fn a_posix_host_never_pays_for_the_probe() {
    let directory = tempfile::tempdir().unwrap();
    let daemon = CountingDaemon::start(directory.path());
    let (ssh, log) = fake_ssh(directory.path(), &daemon.endpoint());
    let target = RemoteTarget::new("posix-host-detection-test");
    let transport = RemoteTransport::for_creation_with_ssh_program(target.clone(), &ssh).unwrap();

    assert_eq!(transport.query_profiles().unwrap(), ["System"]);
    transport.ensure_endpoint().unwrap();

    assert_eq!(remote_host::learned(&target), Some(HostPlatform::Posix));
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(!calls.contains("probe"), "{calls}");
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        ["master", "shared-command", "shared-command", "forward"]
    );
}
