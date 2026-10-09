use super::*;
use crate::messages::{ClientId, SharedOperationId, SharedSpawnBatchRequest, TerminalSize};

const CONNECTION: &str = "192.0.2.10 43210 192.0.2.20 22";

fn create_request() -> Request {
    Request::CreateShared(
        crate::headless::single_pane(
            "remote".to_owned(),
            None,
            None,
            HashMap::from([("ZETTA_PANE_ROUTING_ID".to_owned(), "42".to_owned())]),
        )
        .request(SharedOperationId::new(ClientId::new("viewer"), 1), None),
    )
}

#[test]
fn connection_information_accepts_open_ssh_ipv4_ipv6_and_windows_line_endings() {
    assert_eq!(
        parse_connection(format!("{CONNECTION}\r\n").as_bytes())
            .unwrap()
            .as_deref(),
        Some(CONNECTION)
    );
    let ipv6 = "2001:db8::10 43210 2001:db8::20 22";
    assert_eq!(
        parse_connection(ipv6.as_bytes()).unwrap().as_deref(),
        Some(ipv6)
    );
    assert_eq!(parse_connection(b"\r\n").unwrap(), None);
    for invalid in [
        "hello",
        "local 123 remote 22",
        "192.0.2.10 0 192.0.2.20 22",
        "192.0.2.10 70000 192.0.2.20 22",
        "noise\n192.0.2.10 43210 192.0.2.20 22",
    ] {
        assert!(parse_connection(invalid.as_bytes()).is_err());
    }
}

#[test]
fn a_remote_create_carries_ssh_provenance_to_every_pane() {
    let mut request = create_request();
    if let Request::CreateShared(request) = &mut request {
        let mut second = request.panes[0].clone();
        second.draft_id = 2;
        request.panes.push(second);
    }
    apply_connection(&mut request, CONNECTION);
    let Request::CreateShared(request) = request else {
        unreachable!()
    };
    for pane in request.panes {
        assert_eq!(
            pane.env.get("SSH_CONNECTION").map(String::as_str),
            Some(CONNECTION)
        );
        assert_eq!(
            pane.env.get("ZETTA_PANE_ROUTING_ID").map(String::as_str),
            Some("42")
        );
        assert!(
            !pane.env.contains_key("SSH_TTY"),
            "a stream-only SSH login has no SSH tty"
        );
        assert!(
            !pane.env.contains_key("PATH"),
            "the viewer's environment must not cross"
        );
    }
}

#[test]
fn remote_splits_and_legacy_spawns_carry_the_same_provenance() {
    let Request::CreateShared(create) = create_request() else {
        unreachable!()
    };
    let mut batch = Request::SpawnSharedBatch(SharedSpawnBatchRequest {
        session_id: 1,
        base_revision: crate::messages::SessionRevision(1),
        operation_id: create.operation_id,
        target_pane_id: None,
        replacement: create.replacement,
        panes: create.panes,
        active_pane: create.active_pane,
    });
    apply_connection(&mut batch, CONNECTION);
    let Request::SpawnSharedBatch(batch) = batch else {
        unreachable!()
    };
    assert_eq!(
        batch.panes[0].env.get("SSH_CONNECTION").map(String::as_str),
        Some(CONNECTION)
    );

    let mut spawn = Request::SpawnShared(crate::messages::SharedSpawnRequest {
        session_id: 1,
        base_revision: crate::messages::SessionRevision(1),
        operation_id: batch.operation_id,
        program: None,
        args: Vec::new(),
        env: HashMap::new(),
        working_directory: None,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 0,
            cell_height: 0,
        },
        console_palette: Default::default(),
    });
    apply_connection(&mut spawn, CONNECTION);
    let Request::SpawnShared(spawn) = spawn else {
        unreachable!()
    };
    assert_eq!(
        spawn.env.get("SSH_CONNECTION").map(String::as_str),
        Some(CONNECTION)
    );
}

#[test]
fn ordinary_requests_never_query_the_ssh_environment() {
    let transport = RemoteTransport::for_creation_with_ssh_program(
        RemoteTarget::new("unused-host"),
        "nonexistent-ssh-program",
    )
    .unwrap();
    transport.inherit_login_context(&mut Request::Ping).unwrap();
}

#[test]
fn the_windows_query_only_reads_the_remote_login_variable() {
    use base64::Engine as _;
    let command = connection_command(HostPlatform::Windows);
    let encoded = command.split_whitespace().last().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    let utf16 = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    assert_eq!(
        String::from_utf16(&utf16).unwrap(),
        "[Console]::Out.WriteLine([Environment]::GetEnvironmentVariable('SSH_CONNECTION'))"
    );
}

/// Exercise the client hook, SSH query and serialized daemon request together.
/// A daemon started locally must receive the login context for every new pane.
#[cfg(unix)]
#[test]
fn remote_client_sends_login_context_and_reuses_its_query() {
    use crate::remote::tests::{fake_ssh, write_script};
    use std::os::unix::net::UnixListener;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let endpoint = Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: 4242,
        socket_path: socket,
        token: "test-token".to_owned(),
    };
    let (ssh, log) = fake_ssh(directory.path(), &endpoint);
    let login_ssh = directory.path().join("login-ssh");
    write_script(
        &login_ssh,
        format!(
            "#!/bin/sh\nSSH_CONNECTION='{CONNECTION}' exec '{}' \"$@\"\n",
            ssh.display()
        ),
    );
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        while requests.len() < 2 {
            let (stream, _) = listener.accept().unwrap();
            let mut connection = Connection::new(stream);
            let (envelope, _) = connection.receive::<Envelope>().unwrap();
            match envelope.request {
                Request::Ping => connection.send(&Response::Ok).unwrap(),
                Request::CreateShared(request) => {
                    requests.push(request);
                    connection
                        .send(&Response::Error {
                            message: "request captured".to_owned(),
                        })
                        .unwrap();
                }
                other => panic!("unexpected request: {other:?}"),
            }
        }
        requests
    });
    let transport = Arc::new(
        RemoteTransport::for_creation_with_ssh_program(
            RemoteTarget::new("remote-client-login-context-test"),
            login_ssh,
        )
        .unwrap(),
    );
    let endpoint = transport.ensure_endpoint().unwrap();
    let client = crate::client::Client::from_remote_transport_for_test(transport, endpoint);
    let Request::CreateShared(request) = create_request() else {
        unreachable!()
    };
    for _ in 0..2 {
        let error = client.create_shared(request.clone()).err().unwrap();
        assert_eq!(error.to_string(), "request captured");
    }
    for request in server.join().unwrap() {
        assert_eq!(
            request.panes[0]
                .env
                .get("SSH_CONNECTION")
                .map(String::as_str),
            Some(CONNECTION)
        );
    }
    let calls = std::fs::read_to_string(log).unwrap();
    assert_eq!(calls.lines().filter(|call| *call == "master").count(), 1);
    assert_eq!(
        calls
            .lines()
            .filter(|call| *call == "shared-command")
            .count(),
        2,
        "one endpoint query and one cached login-context query"
    );
}
