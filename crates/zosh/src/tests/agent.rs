use super::*;

fn frame(body: &[u8]) -> Vec<u8> {
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    frame
}

fn session_bind_frame(is_forwarding: bool) -> Vec<u8> {
    fn string(body: &mut Vec<u8>, value: &[u8]) {
        body.extend_from_slice(&(value.len() as u32).to_be_bytes());
        body.extend_from_slice(value);
    }

    let mut body = vec![27];
    string(&mut body, b"session-bind@openssh.com");
    string(&mut body, b"hostkey");
    string(&mut body, b"session-id");
    string(&mut body, b"signature");
    body.push(u8::from(is_forwarding));
    frame(&body)
}

fn read_test_frame(stream: &mut impl Read) -> Vec<u8> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).unwrap();
    let size = u32::from_be_bytes(length) as usize;
    let mut frame = length.to_vec();
    frame.resize(size + 4, 0);
    stream.read_exact(&mut frame[4..]).unwrap();
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

    let too_large = (AGENT_MAX_FRAME as u32).to_be_bytes().to_vec();
    assert!(!valid_frame(&too_large));
}

#[test]
fn session_bind_parser_captures_only_forwarding_bindings() {
    let forwarding = session_bind_frame(true);
    assert!(is_forwarding_session_bind_frame(&forwarding));
    assert!(!is_forwarding_session_bind_frame(&session_bind_frame(
        false
    )));
    assert!(!is_forwarding_session_bind_frame(&frame(&[27])));

    let mut other_extension = forwarding;
    other_extension[8] = b'x';
    assert!(!is_forwarding_session_bind_frame(&other_extension));
}

#[cfg(unix)]
#[test]
fn local_agent_worker_preserves_order_across_complete_frames() {
    use std::fs;
    use std::io::Write as _;
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    // Unix socket addresses are capped at 108 bytes, and the macOS test
    // temp directory already has a long randomized prefix.
    let directory = std::env::temp_dir().join(format!("za-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("agent.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let first = frame(&[11, 1]);
    let second = frame(&[11, 2]);
    let first_reply = frame(&[6, 1]);
    let second_reply = frame(&[6, 2]);
    let server_thread = thread::spawn({
        let first = first.clone();
        let second = second.clone();
        let first_reply = first_reply.clone();
        let second_reply = second_reply.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            for (request, response) in [(first, first_reply), (second, second_reply)] {
                let mut received = vec![0; request.len()];
                stream.read_exact(&mut received).unwrap();
                assert_eq!(received, request);
                stream.write_all(&response).unwrap();
                stream.flush().unwrap();
            }
        }
    });

    let (work_tx, work_rx) = mpsc::sync_channel(2);
    let (result_tx, result_rx) = mpsc::sync_channel(2);
    spawn_worker(4, path.clone(), None, work_rx, result_tx);
    work_tx
        .send(Work {
            request_id: 1,
            frame: first,
        })
        .unwrap();
    work_tx
        .send(Work {
            request_id: 2,
            frame: second,
        })
        .unwrap();

    assert!(matches!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        WorkerResult::Response {
            connection_id: 4,
            request_id: 1,
            frame,
        } if frame == first_reply
    ));
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        WorkerResult::Response {
            connection_id: 4,
            request_id: 2,
            frame,
        } if frame == second_reply
    ));

    drop(work_tx);
    server_thread.join().unwrap();
    fs::remove_file(path).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn bootstrap_relay_captures_binding_and_forwards_the_probe() {
    use std::fs;
    use std::os::unix::net::{UnixListener, UnixStream};

    let directory = PathBuf::from(format!("/tmp/zbr-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("agent.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let binding = session_bind_frame(true);
    let response = frame(&[6]);
    let identities = frame(&[12, 0, 0, 0, 0]);
    let server_thread = thread::spawn({
        let binding = binding.clone();
        let response = response.clone();
        let identities = identities.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert_eq!(read_test_frame(&mut stream), binding);
            stream.write_all(&response).unwrap();
            stream.flush().unwrap();
            assert_eq!(read_test_frame(&mut stream), frame(&[11]));
            stream.write_all(&identities).unwrap();
            stream.flush().unwrap();
        }
    });

    let relay = BootstrapAgentRelay::for_agent_path(&path).unwrap();
    let mut forwarded = UnixStream::connect(relay.path()).unwrap();
    let mut requests = binding.clone();
    requests.extend_from_slice(&frame(&[11]));
    forwarded.write_all(&requests).unwrap();
    forwarded.flush().unwrap();
    assert_eq!(read_test_frame(&mut forwarded), response);
    assert_eq!(read_test_frame(&mut forwarded), identities);
    assert_eq!(relay.binding(), Some(binding));

    drop(forwarded);
    server_thread.join().unwrap();
    drop(relay);
    fs::remove_file(path).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn agent_worker_replays_binding_before_forwarding_identities() {
    use std::fs;
    use std::io::Write as _;
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    let directory = PathBuf::from(format!("/tmp/zbw-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("agent.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let binding = session_bind_frame(true);
    let request = frame(&[11]);
    let mut identities_body = vec![12, 0, 0, 0, 1];
    for value in [b"key".as_slice(), b"comment".as_slice()] {
        identities_body.extend_from_slice(&(value.len() as u32).to_be_bytes());
        identities_body.extend_from_slice(value);
    }
    let identities = frame(&identities_body);
    let server_thread = thread::spawn({
        let binding = binding.clone();
        let request = request.clone();
        let identities = identities.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert_eq!(read_test_frame(&mut stream), binding);
            stream.write_all(&frame(&[6])).unwrap();
            stream.flush().unwrap();
            assert_eq!(read_test_frame(&mut stream), request);
            stream.write_all(&identities).unwrap();
            stream.flush().unwrap();
        }
    });

    let (work_tx, work_rx) = mpsc::sync_channel(1);
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    spawn_worker(9, path.clone(), Some(binding), work_rx, result_tx);
    work_tx
        .send(Work {
            request_id: 1,
            frame: request,
        })
        .unwrap();
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        WorkerResult::Response {
            connection_id: 9,
            request_id: 1,
            frame,
        } if frame == identities
    ));

    drop(work_tx);
    server_thread.join().unwrap();
    fs::remove_file(path).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn binding_failure_keeps_raw_agent_requests_working() {
    use std::fs;
    use std::os::unix::net::UnixListener;

    let directory = PathBuf::from(format!("/tmp/zbf-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("agent.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let binding = session_bind_frame(true);
    let raw_request = frame(&[11]);
    let raw_response = frame(&[12, 0, 0, 0, 0]);
    let server_thread = thread::spawn({
        let binding = binding.clone();
        let raw_request = raw_request.clone();
        let raw_response = raw_response.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert_eq!(read_test_frame(&mut stream), binding);
            stream.write_all(&frame(&[5])).unwrap();
            stream.flush().unwrap();
            assert_eq!(read_test_frame(&mut stream), raw_request);
            stream.write_all(&raw_response).unwrap();
            stream.flush().unwrap();
        }
    });

    let (work_tx, work_rx) = mpsc::sync_channel(1);
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    spawn_worker(8, path.clone(), Some(binding), work_rx, result_tx);
    work_tx
        .send(Work {
            request_id: 1,
            frame: raw_request,
        })
        .unwrap();
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        WorkerResult::Response {
            connection_id: 8,
            request_id: 1,
            frame,
        } if frame == raw_response
    ));

    drop(work_tx);
    server_thread.join().unwrap();
    fs::remove_file(path).unwrap();
    fs::remove_dir(directory).unwrap();
}
