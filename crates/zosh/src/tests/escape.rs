use super::*;

#[test]
fn escape_state_matches_mosh_prefix_rules() {
    let mut state = EscapeState::new(EscapeKey::default());
    assert_eq!(state.feed_all(b"a"), (b"a".to_vec(), None));
    assert_eq!(
        state.feed_all(b"\x1e."),
        (Vec::new(), Some(EscapeAction::Quit))
    );
    assert_eq!(
        state.feed_all(b"\x1e\x1a"),
        (Vec::new(), Some(EscapeAction::Suspend))
    );
    assert_eq!(state.feed_all(b"\x1e^"), (vec![0x1e], None));
}

#[test]
fn custom_escape_key_accepts_both_letter_cases() {
    let key = EscapeKey::from_env(Some("\x01"));
    let mut state = EscapeState::new(key);
    assert_eq!(state.feed_all(b"\x01A"), (vec![1], None));
    assert_eq!(key.name().as_deref(), Some("Ctrl-A"));
}
