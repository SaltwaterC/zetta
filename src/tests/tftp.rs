use super::*;

#[test]
fn interrupted_socket_operations_are_retryable() {
    let interrupted = io::Error::from(io::ErrorKind::Interrupted);
    assert!(socket_operation_was_interrupted(&interrupted));
    assert!(!socket_operation_was_interrupted(&io::Error::from(
        io::ErrorKind::TimedOut
    )));
}
