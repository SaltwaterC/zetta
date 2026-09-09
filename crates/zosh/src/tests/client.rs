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

#[test]
fn terminal_query_proxy_strips_only_the_matching_split_response() {
    let mut proxy = TerminalQueryProxy::default();
    proxy.register_query(b"\x1b]10;?\x07");
    assert_eq!(
        proxy.filter(b"typed\x1b]10;rgb:aaaa/bbbb/cccc"),
        vec![ProxiedInput::User(b"typed".to_vec())]
    );
    assert_eq!(
        proxy.filter(b"\x07after"),
        vec![
            ProxiedInput::TerminalResponse(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07".to_vec()),
            ProxiedInput::User(b"after".to_vec()),
        ]
    );
}

#[test]
fn terminal_query_proxy_preserves_unmatched_osc_input() {
    let mut proxy = TerminalQueryProxy::default();
    proxy.register_query(b"\x1b]10;?\x07");
    let unmatched = b"\x1b]11;rgb:aaaa/bbbb/cccc\x07";
    assert_eq!(
        proxy.filter(unmatched),
        vec![ProxiedInput::User(unmatched.to_vec())]
    );
    assert_eq!(
        proxy.filter(b"\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\"),
        vec![ProxiedInput::TerminalResponse(
            b"\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\".to_vec()
        )]
    );
}

#[test]
fn terminal_query_proxy_keeps_keyboard_order_around_a_response() {
    let mut proxy = TerminalQueryProxy::default();
    proxy.register_query(b"\x1b]11;?\x07");
    assert_eq!(
        proxy.filter(b"a\x1b]11;rgb:aaaa/bbbb/cccc\x07b"),
        vec![
            ProxiedInput::User(b"a".to_vec()),
            ProxiedInput::TerminalResponse(b"\x1b]11;rgb:aaaa/bbbb/cccc\x07".to_vec()),
            ProxiedInput::User(b"b".to_vec()),
        ]
    );
}

#[test]
fn terminal_query_proxy_does_not_consume_an_escape_before_a_response() {
    let mut proxy = TerminalQueryProxy::default();
    proxy.register_query(b"\x1b]10;?\x07");
    assert!(proxy.filter(b"\x1b").is_empty());
    assert_eq!(
        proxy.filter(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07"),
        vec![
            ProxiedInput::User(vec![0x1b]),
            ProxiedInput::TerminalResponse(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07".to_vec()),
        ]
    );
}

#[test]
fn terminal_query_proxy_matches_color_responses_by_kind() {
    let mut proxy = TerminalQueryProxy::default();
    proxy.register_query(b"\x1b]10;?\x07");
    proxy.register_query(b"\x1b]11;?\x07");
    assert_eq!(
        proxy.filter(b"\x1b]11;rgb:aaaa/bbbb/cccc\x07\x1b]10;rgb:aaaa/bbbb/cccc\x07"),
        vec![
            ProxiedInput::TerminalResponse(b"\x1b]11;rgb:aaaa/bbbb/cccc\x07".to_vec()),
            ProxiedInput::TerminalResponse(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07".to_vec()),
        ]
    );
}

#[test]
fn forwarded_terminal_queries_are_written_and_registered() {
    let events = vec![HostEvent::TerminalQuery {
        id: 4,
        bytes: b"\x1b]10;?\x07".to_vec(),
    }];
    let mut proxy = TerminalQueryProxy::default();
    let mut output = Vec::new();
    forward_terminal_queries(&events, &mut proxy, &mut output).unwrap();
    assert_eq!(output, b"\x1b]10;?\x07");
    assert_eq!(
        proxy.filter(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07"),
        vec![ProxiedInput::TerminalResponse(
            b"\x1b]10;rgb:aaaa/bbbb/cccc\x07".to_vec()
        )]
    );
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
