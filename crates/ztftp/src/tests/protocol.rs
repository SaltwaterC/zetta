use super::*;

#[test]
fn interrupted_socket_operations_are_retryable() {
    let interrupted = io::Error::from(io::ErrorKind::Interrupted);
    assert!(socket_operation_was_interrupted(&interrupted));
    assert!(!socket_operation_was_interrupted(&io::Error::from(
        io::ErrorKind::TimedOut
    )));
}

#[test]
fn peer_error_text_cannot_write_control_sequences() {
    let mut packet = vec![0, OP_ERROR as u8, 0, 1];
    packet.extend_from_slice(b"no such file\x1b]0;owned\x07\r\n\xc2\x9b2J\0");

    let message = error_message(&packet);

    assert!(
        !message.chars().any(char::is_control),
        "{message:?} reaches the terminal with control characters"
    );
    assert_eq!(message, "no such file\\u{1b}]0;owned\\u{7}\\r\\n\\u{9b}2J");
}
