use super::*;

#[cfg(unix)]
#[test]
fn ready_connection_requires_a_successful_ping() {
    use std::os::unix::net::UnixListener;

    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("zmux.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: 4242,
        socket_path,
        token: "test-token".to_owned(),
    }
    .write(&directory.path().join("zmux.json"))
    .unwrap();

    let server = std::thread::spawn(move || {
        // `connect_endpoint` checks socket liveness first. The readiness probe
        // is the next connection and is the one that must receive a response.
        let _ = listener.accept().unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut connection = Connection::new(stream);
        let (envelope, _) = connection.receive::<Envelope>().unwrap();
        assert!(matches!(envelope.request, Request::Ping));
        connection.send(&Response::Ok).unwrap();
    });

    assert!(
        Client::connect_ready_at(directory.path())
            .unwrap()
            .is_some()
    );
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn ready_connection_rejects_a_failed_ping() {
    use std::os::unix::net::UnixListener;

    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("zmux.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: 4242,
        socket_path,
        token: "test-token".to_owned(),
    }
    .write(&directory.path().join("zmux.json"))
    .unwrap();

    let server = std::thread::spawn(move || {
        let _ = listener.accept().unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut connection = Connection::new(stream);
        let (envelope, _) = connection.receive::<Envelope>().unwrap();
        assert!(matches!(envelope.request, Request::Ping));
        connection
            .send(&Response::Error {
                message: "not ready".to_owned(),
            })
            .unwrap();
    });

    let error = match Client::connect_ready_at(directory.path()) {
        Err(error) => error,
        Ok(Some(_)) => panic!("a failed ping was accepted"),
        Ok(None) => panic!("the readiness endpoint disappeared"),
    };
    assert!(error.to_string().contains("not ready"), "{error:#}");
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn ready_connection_refreshes_a_stale_endpoint_token() {
    use std::os::unix::net::UnixListener;

    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("zmux.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let endpoint_path = directory.path().join("zmux.json");
    let old = Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: 4242,
        socket_path: socket_path.clone(),
        token: "old-token".to_owned(),
    };
    old.write(&endpoint_path).unwrap();

    let server = std::thread::spawn({
        let endpoint_path = endpoint_path.clone();
        let socket_path = socket_path.clone();
        move || {
            // The first connection is the liveness check performed while the
            // endpoint is read. The second is the readiness ping, which sees
            // the token from the stale endpoint file.
            let _ = listener.accept().unwrap();
            let (stream, _) = listener.accept().unwrap();
            let mut connection = Connection::new(stream);
            let (envelope, _) = connection.receive::<Envelope>().unwrap();
            assert!(matches!(envelope.request, Request::Ping));
            assert_eq!(envelope.token, "old-token");
            connection
                .send(&Response::Error {
                    message: "invalid multiplexer token".to_owned(),
                })
                .unwrap();

            Endpoint {
                version: crate::transport::ENDPOINT_VERSION,
                protocol_version: PROTOCOL_VERSION,
                process_id: 4243,
                socket_path,
                token: "new-token".to_owned(),
            }
            .write(&endpoint_path)
            .unwrap();

            let (stream, _) = listener.accept().unwrap();
            let mut connection = Connection::new(stream);
            let (envelope, _) = connection.receive::<Envelope>().unwrap();
            assert!(matches!(envelope.request, Request::Ping));
            assert_eq!(envelope.token, "new-token");
            connection.send(&Response::Ok).unwrap();
        }
    });

    let client = Client::connect_ready_at(directory.path()).unwrap().unwrap();
    assert_eq!(client.process_id(), 4243);
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn configure_retries_after_a_pre_dispatch_stale_endpoint_token() {
    use std::os::unix::net::UnixListener;

    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("zmux.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let endpoint_path = directory.path().join("zmux.json");
    let old = Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: 4242,
        socket_path: socket_path.clone(),
        token: "old-token".to_owned(),
    };
    old.write(&endpoint_path).unwrap();

    let server = std::thread::spawn({
        let endpoint_path = endpoint_path.clone();
        let socket_path = socket_path.clone();
        move || {
            // The first connection is the liveness probe from
            // `connect_existing_at`.
            let _ = listener.accept().unwrap();

            // `configure_raw` settles readiness before sending the request.
            let (stream, _) = listener.accept().unwrap();
            let mut connection = Connection::new(stream);
            let (envelope, _) = connection.receive::<Envelope>().unwrap();
            assert!(matches!(envelope.request, Request::Ping));
            assert_eq!(envelope.token, "old-token");
            connection.send(&Response::Ok).unwrap();

            let (stream, _) = listener.accept().unwrap();
            let mut connection = Connection::new(stream);
            let (envelope, _) = connection.receive::<Envelope>().unwrap();
            assert!(matches!(envelope.request, Request::Configure { .. }));
            assert_eq!(envelope.token, "old-token");
            connection
                .send(&Response::Error {
                    message: "invalid multiplexer token".to_owned(),
                })
                .unwrap();

            Endpoint {
                version: crate::transport::ENDPOINT_VERSION,
                protocol_version: PROTOCOL_VERSION,
                process_id: 4243,
                socket_path,
                token: "new-token".to_owned(),
            }
            .write(&endpoint_path)
            .unwrap();

            let (stream, _) = listener.accept().unwrap();
            let mut connection = Connection::new(stream);
            let (envelope, _) = connection.receive::<Envelope>().unwrap();
            assert!(matches!(envelope.request, Request::Ping));
            assert_eq!(envelope.token, "new-token");
            connection.send(&Response::Ok).unwrap();

            let (stream, _) = listener.accept().unwrap();
            let mut connection = Connection::new(stream);
            let (envelope, _) = connection.receive::<Envelope>().unwrap();
            assert!(matches!(envelope.request, Request::Configure { .. }));
            assert_eq!(envelope.token, "new-token");
            connection.send(&Response::Ok).unwrap();
        }
    });

    let client = Client::connect_existing_at(directory.path())
        .unwrap()
        .unwrap();
    client
        .configure(Retention::Memory { bytes: 4096 }, Vec::new())
        .unwrap();
    assert_eq!(client.process_id(), 4243);
    server.join().unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn temporary_recipient_failure_degrades_disk_to_memory_without_recipients() {
    let fallback = Retention::Memory { bytes: 16_384 };
    let (effective, reason, recipients) = resolve_effective_retention(
        Retention::Disk,
        &["github:zetta-user".to_owned()],
        fallback,
        |_| {
            Err(crate::persistence::RecipientResolutionError::Temporary(
                anyhow::anyhow!("GitHub DNS lookup failed"),
            ))
        },
    )
    .unwrap();

    assert_eq!(effective, fallback);
    assert!(reason.unwrap().contains("GitHub DNS lookup failed"));
    assert!(recipients.is_empty());
}

#[cfg(feature = "session-persistence")]
#[test]
fn permanent_recipient_failure_does_not_degrade_disk_to_memory() {
    let error = resolve_effective_retention(
        Retention::Disk,
        &["github:zetta-user".to_owned()],
        Retention::Memory { bytes: 16_384 },
        |_| {
            Err(crate::persistence::RecipientResolutionError::Permanent(
                anyhow::anyhow!("invalid GitHub SSH key"),
            ))
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("invalid GitHub SSH key"));
}

#[test]
fn the_multiplexer_is_resolved_beside_this_executable_not_from_the_path() {
    // Resolving through PATH would let an unrelated `zmux` earlier in it be
    // handed a session's terminals.
    let (executable, arguments) = multiplexer_command().unwrap();

    assert!(
        executable.is_absolute(),
        "{} must not depend on PATH lookup",
        executable.display()
    );
    assert!(arguments.contains(&"--daemon".to_owned()));
    // Running as the test binary, there is no `zmux` beside it, so the
    // fallback routes through this executable's own subcommand.
    let name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let is_zmux = name == "zmux" || (cfg!(windows) && name.eq_ignore_ascii_case("zmux.exe"));
    if !is_zmux {
        assert_eq!(arguments.first().map(String::as_str), Some("mux"));
    }
}

#[test]
fn only_an_unknown_configure_variant_triggers_the_upgrade_fallback() {
    for message in [
        "unknown variant `configure`, expected `spawn`",
        "unknown variant 'configure', expected 'spawn'",
        "unknown variant \"configure\", expected \"spawn\"",
    ] {
        assert!(
            is_unsupported_configure(&anyhow::anyhow!(message)),
            "{message}"
        );
    }

    for message in [
        "unknown variant `spawn`, expected `configure`",
        "the daemon rejected the configure request",
        "unknown field `configure`",
    ] {
        assert!(
            !is_unsupported_configure(&anyhow::anyhow!(message)),
            "{message}"
        );
    }
}

#[test]
fn a_configured_zetta_daemon_starts_without_a_retention_argument() {
    let mut arguments = vec!["--daemon".to_owned()];
    append_startup_retention_arguments(&mut arguments, None);
    assert_eq!(arguments, ["--daemon"]);
}

#[test]
fn an_independent_daemon_can_still_receive_a_retention_bootstrap() {
    let mut arguments = vec!["--daemon".to_owned()];
    append_startup_retention_arguments(&mut arguments, Some(Retention::Memory { bytes: 4096 }));
    assert_eq!(
        arguments,
        [
            "--daemon",
            "--retention",
            "memory",
            "--retention-bytes",
            "4096"
        ]
    );
}

#[test]
fn an_exit_report_waits_for_a_late_shared_reporter() {
    let reporters = ExitReporters::default();
    let (sender, receiver) = async_channel::unbounded();

    reporters.report(42, Some(1792), false);
    reporters.register_shared(42, sender);

    assert_eq!(
        receiver.recv_blocking().unwrap(),
        PaneExitReport {
            raw_status: Some(1792),
            input_sent: false,
            disconnected: false,
        }
    );
}
