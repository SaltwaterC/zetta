use super::*;

#[cfg(unix)]
use std::{
    io::{BufRead as _, BufReader, Write as _},
    os::unix::net::UnixListener,
    thread,
};

#[cfg(unix)]
#[test]
fn sends_authenticated_set_and_clear_requests_to_a_zetta_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint_path = directory.path().join("control-42.json");
    let socket_path = endpoint_path.with_extension("sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    zmux::private_fs::write_private_file(
        &endpoint_path,
        &serde_json::to_vec(&serde_json::json!({
            "version": zmux::protocol::CONTROL_VERSION,
            "process_id": 42,
            "socket_path": socket_path,
            "token": "test-token"
        }))
        .unwrap(),
    )
    .unwrap();
    let endpoint = process_control::read_control_endpoint(&endpoint_path, 42).unwrap();

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
            stream.write_all(b"{\"status\":\"ok\"}\n").unwrap();
        }
    });

    for name in [Some("feature/api".to_owned()), None] {
        assert!(
            send_worktree_name(
                &endpoint,
                &WorktreeNameRequest {
                    attention_id: 7,
                    name,
                },
            )
            .unwrap()
        );
    }
    server.join().unwrap();
}

#[test]
fn rejects_invalid_process_and_attention_ids_before_reading_an_endpoint() {
    for (process_id, attention_id) in [(0, 1), (1, 0)] {
        let error = request_process_worktree_name(
            process_id,
            WorktreeNameRequest {
                attention_id,
                name: None,
            },
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("must be positive"),
            "{error:#}"
        );
    }
}
