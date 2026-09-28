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

    let mut server = AgentServer::new();
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
            .records()
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

#[cfg(windows)]
#[test]
fn windows_agent_pipe_is_ready_and_bridges_a_complete_frame() {
    use std::time::Instant;

    let mut server = AgentServer::new();
    let path = server.socket_path().expect("private pipe").to_path_buf();
    let mut client = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("listener must exist before announcing readiness");
    client.write_all(&frame(&[11])).unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let request = loop {
        server.poll();
        if let Some(request) = server
            .records()
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
    assert!(
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .is_err()
    );
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
    let handle = windows_pipe::create(windows::core::PCWSTR(wide.as_ptr()), 1024)
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
