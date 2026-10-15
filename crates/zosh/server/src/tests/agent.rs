use super::*;

fn frame(body: &[u8]) -> Vec<u8> {
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    frame
}

#[test]
fn agent_frame_validation_requires_a_matching_bounded_length() {
    let valid = frame(&[6]);
    assert!(valid_frame(&valid));

    let mut truncated = valid.clone();
    truncated.pop();
    assert!(!valid_frame(&truncated));

    let mut mismatched = valid;
    mismatched[3] = 2;
    assert!(!valid_frame(&mismatched));

    let too_large = (MAX_FRAME as u32).to_be_bytes().to_vec();
    assert!(!valid_frame(&too_large));
}

#[cfg(unix)]
#[test]
fn bootstrap_probe_requests_identities_once() {
    use std::os::unix::net::UnixStream;

    let (mut client, mut agent) = UnixStream::pair().unwrap();
    let server_thread = std::thread::spawn(move || {
        let mut request = [0; 5];
        agent.read_exact(&mut request).unwrap();
        assert_eq!(request, [0, 0, 0, 1, 11]);
        agent.write_all(&[0, 0, 0, 5, 12, 0, 0, 0, 0]).unwrap();
        agent.flush().unwrap();
    });

    prime_agent_stream(&mut client).unwrap();
    server_thread.join().unwrap();
}

#[cfg(unix)]
#[test]
fn unix_agent_socket_bridges_ordered_frames_and_cleans_up() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut server = AgentServer::new(crate::wake::Waker::current_thread());
    let path = server.socket_path().unwrap().to_path_buf();
    let socket_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let directory_mode = fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket_mode, 0o600);
    assert_eq!(directory_mode, 0o700);

    let mut client = UnixStream::connect(&path).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    client.write_all(&frame(&[6])).unwrap();

    let request = (0..100).find_map(|_| {
        server.poll();
        let request = server
            .records_after(None)
            .into_iter()
            .find_map(|record| match record {
                AgentHostRecord::Request {
                    id,
                    connection_id,
                    frame,
                } => Some((id, connection_id, frame)),
                _ => None,
            });
        if request.is_none() {
            std::thread::sleep(Duration::from_millis(1));
        }
        request
    });
    let (request_id, connection_id, request_frame) = request.expect("agent request");
    assert_eq!(request_frame, frame(&[6]));

    let response = frame(&[5]);
    assert!(server.apply_response(connection_id, request_id, &response, false));
    let mut received = vec![0; response.len()];
    client.read_exact(&mut received).unwrap();
    assert_eq!(received, response);

    drop(client);
    drop(server);
    assert!(!path.exists(), "the private agent socket must be removed");
}

#[cfg(unix)]
#[test]
fn unix_agent_connection_waits_for_a_frame_written_after_accept() {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut server = AgentServer::new(crate::wake::Waker::current_thread());
    let path = server.socket_path().unwrap().to_path_buf();
    let mut client = UnixStream::connect(&path).unwrap();
    // Accept while the socket is still empty, as it is when ssh connects and
    // only then composes its request. On macOS an accepted socket inherits the
    // listener's O_NONBLOCK, and a reader that kept it saw WouldBlock here.
    server.poll();
    std::thread::sleep(Duration::from_millis(50));
    server.poll();
    client.write_all(&frame(&[11])).unwrap();

    let saw_request = (0..100).any(|_| {
        server.poll();
        let records = server.records_after(None);
        assert!(
            !records
                .iter()
                .any(|record| matches!(record, AgentHostRecord::Close { .. })),
            "the connection closed before its request arrived"
        );
        records
            .iter()
            .any(|record| matches!(record, AgentHostRecord::Request { .. }))
            || {
                std::thread::sleep(Duration::from_millis(10));
                false
            }
    });
    assert!(saw_request);
}

#[cfg(windows)]
#[test]
fn windows_agent_pipe_is_ready_and_bridges_a_complete_frame() {
    use std::time::Instant;

    let mut server = AgentServer::new(crate::wake::Waker::current_thread());
    let path = server.socket_path().expect("private pipe").to_path_buf();
    let mut client =
        windows_pipe::open(&path).expect("listener must exist before announcing readiness");
    client.write_all(&frame(&[11])).unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let request = loop {
        server.poll();
        if let Some(request) =
            server
                .records_after(None)
                .into_iter()
                .find_map(|record| match record {
                    AgentHostRecord::Request {
                        id,
                        connection_id,
                        frame,
                    } => Some((id, connection_id, frame)),
                    _ => None,
                })
        {
            break request;
        }
        assert!(Instant::now() < deadline, "agent request was not forwarded");
        thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(request.2, frame(&[11]));
    let response = frame(&[12, 0, 0, 0, 0]);
    assert!(server.apply_response(request.1, request.0, &response, false));
    let (tx, rx) = mpsc::sync_channel(1);
    let response_len = response.len();
    thread::spawn(move || {
        let mut received = vec![0; response_len];
        let result = client.read_exact(&mut received).map(|()| received);
        let _ = tx.send(result);
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(),
        response
    );
    drop(server);
    assert!(windows_pipe::open(&path).is_err());
}

#[cfg(windows)]
#[test]
fn windows_bootstrap_primes_the_forwarded_agent_pipe() {
    use std::os::windows::{ffi::OsStrExt as _, io::FromRawHandle as _};
    use windows::Win32::System::Pipes::ConnectNamedPipe;

    let path = PathBuf::from(format!(
        r"\\.\pipe\zosh-prime-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = windows_pipe::create(windows::core::PCWSTR(wide.as_ptr()), 1024, true)
        .expect("create test agent pipe");
    let handle = handle.0 as usize;
    let agent = thread::spawn(move || {
        let handle = windows::Win32::Foundation::HANDLE(handle as _);
        if let Err(error) = unsafe { ConnectNamedPipe(handle, None) } {
            assert_eq!(
                error.code().0 as u32 & 0xffff,
                windows::Win32::Foundation::ERROR_PIPE_CONNECTED.0
            );
        }
        let mut stream = unsafe { std::fs::File::from_raw_handle(handle.0 as _) };
        let mut request = [0_u8; 5];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(request, [0, 0, 0, 1, 11]);
        stream.write_all(&frame(&[12, 0, 0, 0, 0])).unwrap();
    });
    prime_agent_path(&path).expect("prime a Windows forwarded agent");
    agent.join().unwrap();
}

/// Serves one connection on a fresh pipe at `path` the way an agent would:
/// reads `request_len` bytes, then reports the impersonation level the client
/// granted, then answers with `reply`.
#[cfg(windows)]
fn impersonation_level_of_next_client(
    path: &Path,
    request_len: usize,
    reply: Vec<u8>,
) -> thread::JoinHandle<windows::Win32::Security::SECURITY_IMPERSONATION_LEVEL> {
    use std::os::windows::{ffi::OsStrExt as _, io::FromRawHandle as _};
    use windows::Win32::{
        Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE},
        Security::{
            GetTokenInformation, RevertToSelf, SECURITY_IMPERSONATION_LEVEL, TOKEN_QUERY,
            TokenImpersonationLevel,
        },
        System::{
            Pipes::{ConnectNamedPipe, ImpersonateNamedPipeClient},
            Threading::{GetCurrentThread, OpenThreadToken},
        },
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = windows_pipe::create(windows::core::PCWSTR(wide.as_ptr()), 1024, true)
        .expect("create test agent pipe")
        .0 as usize;
    thread::spawn(move || {
        let handle = HANDLE(handle as _);
        if let Err(error) = unsafe { ConnectNamedPipe(handle, None) } {
            assert_eq!(error.code().0 as u32 & 0xffff, ERROR_PIPE_CONNECTED.0);
        }
        let mut stream = unsafe { std::fs::File::from_raw_handle(handle.0 as _) };
        let mut request = vec![0_u8; request_len];
        stream.read_exact(&mut request).unwrap();

        unsafe { ImpersonateNamedPipeClient(handle) }.expect("impersonating the client");
        let mut token = HANDLE::default();
        // As self: an identification-level token cannot be opened as itself.
        let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) };
        unsafe { RevertToSelf() }.expect("reverting");
        opened.expect("opening the impersonation token");
        let mut level = SECURITY_IMPERSONATION_LEVEL::default();
        let mut length = 0_u32;
        let queried = unsafe {
            GetTokenInformation(
                token,
                TokenImpersonationLevel,
                Some((&raw mut level).cast()),
                std::mem::size_of::<SECURITY_IMPERSONATION_LEVEL>() as u32,
                &mut length,
            )
        };
        let _ = unsafe { CloseHandle(token) };
        queried.expect("reading the impersonation level");

        stream.write_all(&reply).unwrap();
        level
    })
}

/// Priming connects to the forwarded agent's pipe, which another process
/// serves: it may learn who connected but not act as zosh-server.
#[cfg(windows)]
#[test]
fn windows_priming_grants_the_agent_pipe_identification_only() {
    use windows::Win32::Security::SecurityIdentification;

    let path = PathBuf::from(format!(
        r"\\.\pipe\zosh-prime-sqos-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let agent = impersonation_level_of_next_client(&path, 5, frame(&[12, 0, 0, 0, 0]));
    prime_agent_path(&path).expect("prime a Windows forwarded agent");
    assert_eq!(agent.join().unwrap(), SecurityIdentification);
}

/// A pipe name somebody already holds is refused rather than joined: a later
/// instance would carry the first creator's DACL.
#[cfg(windows)]
#[test]
fn windows_agent_pipe_refuses_a_name_that_already_exists() {
    use windows::Win32::Foundation::CloseHandle;

    let wide = format!(r"\\.\pipe\zosh-first-instance-test-{}", std::process::id())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let name = windows::core::PCWSTR(wide.as_ptr());
    let squatter = windows_pipe::create(name, 1024, true).expect("the first instance");

    assert!(windows_pipe::create(name, 1024, true).is_err());

    let _ = unsafe { CloseHandle(squatter) };
}

/// The directory is never more open than `0700`, not just once `bind` is done:
/// checked before anything else touches its mode, under whatever umask the
/// test runs with (a typical `022` or `002` would leave a plain `create_dir`
/// group- and world-readable).
#[cfg(unix)]
#[test]
fn unix_agent_socket_directory_is_private_from_creation() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("zosh-agent-dir-test-{}", std::process::id()));
    unix_socket::create_private_dir(&root).unwrap();
    let mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    fs::remove_dir(&root).unwrap();
    assert_eq!(mode & 0o077, 0, "created {mode:o}");
    assert!(
        unix_socket::create_private_dir(std::env::temp_dir().as_path()).is_err(),
        "an existing directory must not be adopted"
    );
}

#[cfg(unix)]
#[test]
fn unix_agent_socket_admits_its_owner_and_root_only() {
    assert!(unix_socket::uid_may_connect(1000, 1000));
    assert!(unix_socket::uid_may_connect(0, 1000));
    assert!(!unix_socket::uid_may_connect(1001, 1000));
}

/// A connection from someone other than the user the socket serves is closed
/// at once and never reaches the session loop — the listener is opened for a
/// user this test is not, so the test's own connection stands in for theirs.
#[cfg(unix)]
#[test]
fn unix_agent_socket_refuses_a_peer_that_is_not_its_owner() {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let owner = unix_socket::this_user();
    if owner == 0 {
        // Root is always admitted, so there is no other user to be.
        return;
    }
    let (event_tx, events) = mpsc::sync_channel(EVENT_QUEUE_DEPTH);
    let event_tx = WakingSender::new(event_tx, crate::wake::Waker::current_thread());
    let listener = create_listener_for(event_tx, owner + 1).unwrap();

    let mut client = UnixStream::connect(&listener.path).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    // A frame it would otherwise forward; written before the refusal can
    // close the socket, so a failed write is as good as the closed read.
    let _ = client.write_all(&frame(&[11]));
    let mut byte = [0; 1];
    let read = client.read(&mut byte);
    assert!(
        matches!(read, Ok(0))
            || read
                .as_ref()
                .is_err_and(|error| { error.kind() == io::ErrorKind::ConnectionReset }),
        "the refused connection must be closed, got {read:?}"
    );
    assert!(
        matches!(events.try_recv(), Err(TryRecvError::Empty)),
        "a refused connection must not reach the session loop"
    );

    let path = listener.path.clone();
    drop(listener);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(path.parent().unwrap());
}
