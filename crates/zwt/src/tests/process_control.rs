use super::*;

#[cfg(unix)]
use std::{io::BufReader, os::unix::net::UnixListener, thread};

#[cfg(unix)]
#[test]
fn sends_authenticated_set_and_clear_requests_to_a_zetta_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("control.sock");
    let endpoint_path = directory.path().join("control-42.json");
    let listener = UnixListener::bind(&socket_path).unwrap();
    std::fs::write(
        &endpoint_path,
        serde_json::to_vec(&serde_json::json!({
            "version": zmux::protocol::CONTROL_VERSION,
            "process_id": 42,
            "socket_path": socket_path,
            "token": "test-token"
        }))
        .unwrap(),
    )
    .unwrap();

    let server = thread::spawn(move || {
        for expected in [
            serde_json::json!({
                "token": "test-token",
                "command": "set_worktree_name",
                "attention_id": 7,
                "worktree_name": "feature/api"
            }),
            serde_json::json!({
                "token": "test-token",
                "command": "set_worktree_name",
                "attention_id": 7,
                "worktree_name": null
            }),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&request).unwrap(),
                expected
            );
            stream
                .write_all(
                    br#"{"status":"ok"}
"#,
                )
                .unwrap();
        }
    });

    assert!(
        request_process_worktree_name_at(
            &endpoint_path,
            42,
            WorktreeNameRequest {
                attention_id: 7,
                name: Some("feature/api".to_owned()),
            },
        )
        .unwrap()
    );
    assert!(
        request_process_worktree_name_at(
            &endpoint_path,
            42,
            WorktreeNameRequest {
                attention_id: 7,
                name: None,
            },
        )
        .unwrap()
    );
    server.join().unwrap();
}

#[test]
fn rejects_invalid_process_and_attention_ids_before_reading_an_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = directory.path().join("missing.json");
    assert!(
        request_process_worktree_name_at(
            &endpoint,
            0,
            WorktreeNameRequest {
                attention_id: 1,
                name: None,
            },
        )
        .is_err()
    );
    assert!(
        request_process_worktree_name_at(
            &endpoint,
            1,
            WorktreeNameRequest {
                attention_id: 0,
                name: None,
            },
        )
        .is_err()
    );
}
