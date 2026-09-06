use super::*;

#[test]
fn raw_terminal_sequences_are_forwarded_without_key_reencoding() {
    let mut escape = EscapeState::new(EscapeKey::default());
    assert_eq!(
        process_input_bytes(&mut escape, b"\x1bOA\x1b[<0;12;8M"),
        (b"\x1bOA\x1b[<0;12;8M".to_vec(), None)
    );
}

#[test]
fn ctrl_c_is_forwarded_as_remote_input() {
    let mut escape = EscapeState::new(EscapeKey::default());
    assert_eq!(process_input_bytes(&mut escape, b"\x03"), (vec![3], None));
}

#[cfg(not(unix))]
#[test]
fn control_and_special_keys_are_encoded_as_terminal_bytes() {
    assert_eq!(control_byte('c'), Some(3));
    assert_eq!(control_byte('['), Some(0x1b));
    assert_eq!(
        key_bytes(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        )),
        b"\r".to_vec()
    );
    assert_eq!(
        key_bytes(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::F(1),
            crossterm::event::KeyModifiers::NONE,
        )),
        b"\x1bOP".to_vec()
    );
    assert_eq!(control_byte(' '), Some(0));
    assert_eq!(control_byte('?'), Some(0x7f));
}
