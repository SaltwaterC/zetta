use super::*;
use std::fs;
#[cfg(unix)]
use std::io::Seek as _;
#[cfg(unix)]
use std::os::fd::AsFd;

#[test]
fn a_wrong_token_is_rejected() {
    assert!(token_matches("abcd", "abcd"));
    assert!(!token_matches("abcd", "abce"));
    // Length differences must not short-circuit into a different answer.
    assert!(!token_matches("abcd", "abcdef"));
    assert!(!token_matches("", "abcd"));
}

#[test]
fn tokens_are_random_and_hex_encoded() {
    let first = random_hex(16).unwrap();
    let second = random_hex(16).unwrap();

    assert_eq!(first.len(), 32);
    assert_ne!(first, second);
    assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
}

#[cfg(unix)]
#[test]
fn the_endpoint_round_trips_and_stays_private_to_this_user() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions").join("zmux.json");
    let endpoint = Endpoint {
        version: ENDPOINT_VERSION,
        protocol_version: crate::messages::PROTOCOL_VERSION,
        process_id: std::process::id(),
        socket_path: directory.path().join("sessions").join("zmux.sock"),
        token: random_hex(16).unwrap(),
    };
    endpoint.write(&path).unwrap();

    assert_eq!(Endpoint::read(&path).unwrap(), endpoint);

    use std::os::unix::fs::PermissionsExt as _;
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "the token must not be readable by others"
    );
    let directory_mode = fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(directory_mode & 0o777, 0o700);
}

#[test]
fn an_endpoint_from_a_future_version_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("zmux.json");
    fs::write(
        &path,
        serde_json::json!({
            "version": ENDPOINT_VERSION + 1,
            "process_id": 1,
            "socket_path": "/tmp/zmux.sock",
            "token": "00",
        })
        .to_string(),
    )
    .unwrap();

    // Guessing at a newer layout would mean connecting with a misread token.
    assert!(Endpoint::read(&path).is_err());
}

#[test]
fn messages_are_newline_framed_and_bounded() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Message {
        session: u64,
    }

    let mut wire = Vec::new();
    write_message(&mut wire, &Message { session: 7 }).unwrap();
    assert_eq!(wire.last(), Some(&b'\n'));
    assert_eq!(
        read_message::<Message>(&mut wire.as_slice()).unwrap(),
        Message { session: 7 }
    );

    // An unterminated message is refused rather than buffered without bound.
    let oversized = vec![b'a'; MAX_MESSAGE_BYTES + 2];
    assert!(read_message::<Message>(&mut oversized.as_slice()).is_err());
}

#[cfg(unix)]
#[test]
fn a_descriptor_survives_the_handover_and_still_refers_to_the_same_file() {
    let (sender, receiver) = Stream::pair().unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"session output").unwrap();

    send_with_descriptors(&sender, b"attach\n", &[file.as_fd()]).unwrap();

    let mut buffer = [0; 32];
    let (read, mut descriptors) = receive_with_descriptors(&receiver, &mut buffer).unwrap();
    assert_eq!(&buffer[..read], b"attach\n");
    assert_eq!(descriptors.len(), 1);

    // The point of passing the descriptor rather than the bytes: the receiver
    // holds the same open file, not a copy of its contents.
    let mut received = std::fs::File::from(descriptors.remove(0));
    received.rewind().unwrap();
    let mut contents = String::new();
    received.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "session output");
}

#[cfg(unix)]
#[test]
fn a_message_without_descriptors_still_arrives() {
    let (sender, receiver) = Stream::pair().unwrap();
    send_with_descriptors(&sender, b"detach\n", &[]).unwrap();

    let mut buffer = [0; 32];
    let (read, descriptors) = receive_with_descriptors(&receiver, &mut buffer).unwrap();
    assert_eq!(&buffer[..read], b"detach\n");
    assert!(descriptors.is_empty());
}

#[cfg(unix)]
#[test]
fn descriptors_arrive_close_on_exec() {
    // A client that spawns a process must not leak a session's terminal into
    // it; the receiving side asks for this rather than trusting the sender.
    let (sender, receiver) = Stream::pair().unwrap();
    let file = tempfile::tempfile().unwrap();
    send_with_descriptors(&sender, b".", &[file.as_fd()]).unwrap();

    let mut buffer = [0; 8];
    let (_, descriptors) = receive_with_descriptors(&receiver, &mut buffer).unwrap();
    let flags = unsafe { libc::fcntl(descriptors[0].as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0);
    assert_ne!(flags & libc::FD_CLOEXEC, 0);
}

#[cfg(unix)]
#[test]
fn sending_more_descriptors_than_the_buffer_holds_is_refused() {
    let (sender, _receiver) = Stream::pair().unwrap();
    let file = tempfile::tempfile().unwrap();
    let descriptors = vec![file.as_fd(); MAX_DESCRIPTORS + 1];

    // Refused rather than silently truncated, which would hand over the wrong
    // descriptor for a pane.
    assert!(send_with_descriptors(&sender, b".", &descriptors).is_err());
}

#[cfg(unix)]
#[test]
fn the_peer_of_a_socket_pair_is_this_user() {
    let (sender, _receiver) = Stream::pair().unwrap();
    assert_eq!(peer_uid(&sender).unwrap(), unsafe { libc::getuid() });
}

#[test]
fn an_endpoint_written_before_versioning_is_unreadable_rather_than_version_zero() {
    // Zero is a real protocol version, so defaulting a missing field to it
    // would make a multiplexer that predates the field look compatible with a
    // build that cannot talk to it — the exact confusion the field exists to
    // prevent.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("zmux.json");
    fs::write(
        &path,
        serde_json::json!({
            "version": ENDPOINT_VERSION,
            "process_id": 1,
            "socket_path": "/tmp/zmux.sock",
            "token": "00",
        })
        .to_string(),
    )
    .unwrap();

    assert!(Endpoint::read(&path).is_err());
}
