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

    let too_large = (AGENT_MAX_FRAME as u32).to_be_bytes().to_vec();
    assert!(!valid_frame(&too_large));
}

#[cfg(unix)]
#[test]
fn local_agent_worker_preserves_order_across_complete_frames() {
    use std::fs;
    use std::io::{Read as _, Write as _};
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
    spawn_worker(4, path.clone(), work_rx, result_tx);
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
