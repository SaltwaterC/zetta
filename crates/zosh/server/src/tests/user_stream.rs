use super::*;

fn message(instructions: &[UserInstruction]) -> Vec<u8> {
    UserInstruction::encode_message(instructions)
}

fn state(old: u64, new: u64, throwaway: u64, diff: Vec<u8>) -> ReceivedState {
    ReceivedState {
        old_num: old,
        new_num: new,
        ack_num: 0,
        throwaway_num: throwaway,
        diff,
    }
}

#[test]
fn cumulative_retransmission_does_not_replay_keys() {
    let mut tracker = UserStreamTracker::new();

    let one = state(
        0,
        1,
        0,
        message(&[UserInstruction::keystroke(b"l".to_vec())]),
    );
    assert_eq!(
        tracker.accept(&one).unwrap().events,
        vec![UserEvent::Byte(b'l')]
    );

    // A newer parallel state is based on state 0 and contains the complete
    // cumulative stream "ls". Only 's' is new to the PTY.
    let two = state(
        0,
        2,
        0,
        message(&[UserInstruction::keystroke(b"ls".to_vec())]),
    );
    assert_eq!(
        tracker.accept(&two).unwrap().events,
        vec![UserEvent::Byte(b's')]
    );
}

#[test]
fn out_of_order_older_state_is_retained_but_not_executed() {
    let mut tracker = UserStreamTracker::new();
    let newer = state(
        0,
        3,
        0,
        message(&[UserInstruction::keystroke(b"abc".to_vec())]),
    );
    assert_eq!(tracker.accept(&newer).unwrap().events.len(), 3);

    let older = state(
        0,
        2,
        0,
        message(&[UserInstruction::keystroke(b"ab".to_vec())]),
    );
    assert!(tracker.accept(&older).unwrap().events.is_empty());
}

/// A keep-alive as the client encodes it: `UserMessage.instruction`
/// holding `Instruction` field 20, carrying the interval.  Written
/// out by hand because the point is to track the wire, not
/// `moshcatty`'s encoder, which has no idea the field exists.
///
/// The interval is a real varint, so anything from 128 up spans two
/// bytes: writing it as one is what an earlier version of this
/// helper did, and it silently produced a truncated field.
fn keep_alive(interval_ms: u32) -> Vec<u8> {
    let mut varint = Vec::new();
    let mut value = interval_ms;
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        varint.push(if value == 0 { byte } else { byte | 0x80 });
        if value == 0 {
            break;
        }
    }
    let mut instruction = vec![0xA0, 0x01];
    instruction.extend_from_slice(&varint);
    let mut message = vec![0x0A, instruction.len() as u8];
    message.extend_from_slice(&instruction);
    message
}

#[test]
fn a_keep_alive_is_answered_without_reaching_the_pty() {
    let mut tracker = UserStreamTracker::new();
    let accepted = tracker.accept(&state(0, 1, 0, keep_alive(100))).unwrap();
    assert_eq!(accepted.keep_alive_ms, Some(100));
    assert!(accepted.events.is_empty(), "a keep-alive is not input");
    assert_eq!(
        tracker.committed_events, 0,
        "a keep-alive must not move the committed prefix, or every \
         later cumulative diff is skipped by one event"
    );

    // And the input either side of it still lands exactly once.
    let typed = tracker
        .accept(&state(
            1,
            2,
            0,
            message(&[UserInstruction::keystroke(b"ls".to_vec())]),
        ))
        .unwrap();
    assert_eq!(
        typed.events,
        vec![UserEvent::Byte(b'l'), UserEvent::Byte(b's')]
    );
    assert_eq!(typed.keep_alive_ms, None);
}

#[test]
fn a_keep_alive_beside_typing_yields_only_the_typing() {
    let mut tracker = UserStreamTracker::new();
    // One diff carrying a keystroke instruction and a keep-alive,
    // which is what a keep-alive minted in the same tick as input
    // looks like on the wire.
    let mut diff = message(&[UserInstruction::keystroke(b"a".to_vec())]);
    diff.extend_from_slice(&keep_alive(100));
    let accepted = tracker.accept(&state(0, 1, 0, diff)).unwrap();
    assert_eq!(accepted.keep_alive_ms, Some(100));
    assert_eq!(accepted.events, vec![UserEvent::Byte(b'a')]);
    assert_eq!(tracker.committed_events, 1);
}

#[test]
fn an_ordinary_diff_is_not_mistaken_for_a_keep_alive() {
    assert_eq!(keep_alive_interval(&[]), None);
    assert_eq!(
        keep_alive_interval(&message(&[UserInstruction::keystroke(b"ls -l\r".to_vec())])),
        None
    );
    assert_eq!(
        keep_alive_interval(&message(&[UserInstruction::resize(120, 40)])),
        None
    );
    // Some other extension nobody here knows: field 21, varint.
    assert_eq!(keep_alive_interval(&[0x0A, 0x03, 0xA8, 0x01, 0x01]), None);
    // Truncated rather than absent. The scan gives up; `decode_events`
    // is what reports the diff as malformed.
    assert_eq!(keep_alive_interval(&[0x0A, 0x03, 0xA0]), None);
    assert_eq!(keep_alive_interval(&[0x0A, 0x7F]), None);
}

#[test]
fn a_multi_byte_interval_and_the_newest_of_several_are_read() {
    // 500 ms needs a two-byte varint, and the helper must agree with
    // the bytes PROTOCOL.md documents.
    assert_eq!(keep_alive(500), vec![0x0A, 0x04, 0xA0, 0x01, 0xF4, 0x03]);
    assert_eq!(
        keep_alive_interval(&[0x0A, 0x04, 0xA0, 0x01, 0xF4, 0x03]),
        Some(500)
    );
    // A cumulative diff can carry several. The last is the interval
    // now in force, so it is the one that wins.
    let mut diff = keep_alive(100);
    diff.extend_from_slice(&keep_alive(250));
    assert_eq!(keep_alive_interval(&diff), Some(250));
}

#[test]
fn clipboard_capability_is_an_announcement_without_keyboard_events() {
    let mut instruction = vec![0xC0, 0x01, 0x01]; // field 24, version 1
    let mut diff = vec![0x0A, instruction.len() as u8];
    diff.append(&mut instruction);
    assert_eq!(announcements(&diff).clipboard_version, Some(1));
    assert!(decode_events(&diff).unwrap().is_empty());
}

#[test]
fn a_keep_alive_survives_a_long_keystroke_field_in_front_of_it() {
    // The walk must skip a length-delimited field by its length
    // rather than scanning through it, or a paste containing the
    // bytes 0xA0 0x01 would read as a keep-alive.
    let mut diff = message(&[UserInstruction::keystroke(vec![0xA0, 0x01, 0x09])]);
    assert_eq!(keep_alive_interval(&diff), None);
    diff.extend_from_slice(&keep_alive(100));
    assert_eq!(keep_alive_interval(&diff), Some(100));
}

/// The keep-alive half of what a diff announces, which is all these
/// tests are about.
fn keep_alive_interval(diff: &[u8]) -> Option<u32> {
    announcements(diff).keep_alive_ms
}

fn terminal_response(bytes: &[u8]) -> Vec<u8> {
    let mut instruction = vec![0xAA, 0x01, bytes.len() as u8];
    instruction.extend_from_slice(bytes);
    let mut message = vec![0x0A, instruction.len() as u8];
    message.extend_from_slice(&instruction);
    message
}

#[test]
fn stock_user_decoder_skips_terminal_response_extension() {
    let decoded = UserInstruction::decode_message(&terminal_response(b"response")).unwrap();
    assert_eq!(decoded, vec![UserInstruction::default()]);
}

#[test]
fn terminal_responses_are_counted_and_replayed_in_stream_order() {
    let mut tracker = UserStreamTracker::new();
    let first = state(
        0,
        1,
        0,
        message(&[UserInstruction::keystroke(b"a".to_vec())]),
    );
    assert_eq!(
        tracker.accept(&first).unwrap().events,
        vec![UserEvent::Byte(b'a')]
    );

    // This newer state is cumulative from state 0. The already committed
    // key is skipped, while the response remains the next event.
    let mut diff = message(&[UserInstruction::keystroke(b"a".to_vec())]);
    diff.extend_from_slice(&terminal_response(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07"));
    let accepted = tracker.accept(&state(0, 2, 0, diff.clone())).unwrap();
    assert_eq!(
        accepted.events,
        vec![UserEvent::TerminalResponse(
            b"\x1b]10;rgb:aaaa/bbbb/cccc\x07".to_vec()
        )]
    );
    assert_eq!(tracker.committed_events, 2);

    let replay = tracker.accept(&state(0, 2, 0, diff)).unwrap();
    assert!(replay.events.is_empty());
}

#[test]
fn resize_is_a_userstream_event() {
    let mut tracker = UserStreamTracker::new();
    let resize = state(0, 1, 0, message(&[UserInstruction::resize(120, 40)]));
    assert_eq!(
        tracker.accept(&resize).unwrap().events,
        vec![UserEvent::Resize {
            cols: 120,
            rows: 40
        }]
    );
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        bytes.push(if value == 0 { byte } else { byte | 0x80 });
        if value == 0 {
            break;
        }
    }
    bytes
}

fn field_varint(field: u64, value: u64) -> Vec<u8> {
    let mut bytes = varint(field << 3);
    bytes.extend(varint(value));
    bytes
}

fn field_bytes(field: u64, value: &[u8]) -> Vec<u8> {
    let mut bytes = varint((field << 3) | WIRE_BYTES);
    bytes.extend(varint(value.len() as u64));
    bytes.extend_from_slice(value);
    bytes
}

fn agent_hello(version: u32) -> Vec<u8> {
    let hello = field_varint(1, u64::from(version));
    let user_agent = field_bytes(1, &hello);
    let instruction = field_bytes(AGENT_FIELD, &user_agent);
    field_bytes(INSTRUCTION_FIELD, &instruction)
}

fn agent_response(connection_id: u64, request_id: u64, frame: &[u8], closed: bool) -> Vec<u8> {
    let mut response = field_varint(1, connection_id);
    response.extend(field_varint(2, request_id));
    response.extend(field_bytes(3, frame));
    response.extend(field_varint(4, u64::from(closed)));
    let user_agent = field_bytes(2, &response);
    let instruction = field_bytes(AGENT_FIELD, &user_agent);
    field_bytes(INSTRUCTION_FIELD, &instruction)
}

#[test]
fn agent_extensions_are_cumulative_and_unknown_to_stock_mosh() {
    let hello = agent_hello(1);
    let frame = [0, 0, 0, 1, 6];
    let mut cumulative = hello.clone();
    cumulative.extend(agent_response(7, 9, &frame, false));

    let stock = UserInstruction::decode_message(&hello).unwrap();
    assert_eq!(stock, vec![UserInstruction::default()]);

    let mut tracker = UserStreamTracker::new();
    assert_eq!(
        tracker.accept(&state(0, 1, 0, hello)).unwrap().events,
        vec![UserEvent::AgentHello { version: 1 }]
    );
    assert_eq!(
        tracker
            .accept(&state(0, 2, 0, cumulative.clone()))
            .unwrap()
            .events,
        vec![UserEvent::AgentResponse {
            connection_id: 7,
            request_id: 9,
            frame: frame.to_vec(),
            closed: false,
        }]
    );
    assert!(
        tracker
            .accept(&state(0, 2, 0, cumulative))
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn agent_response_frames_have_a_hard_size_limit() {
    let oversized = vec![0_u8; MAX_AGENT_FRAME + 1];
    let _error = UserStreamTracker::new()
        .accept(&state(0, 1, 0, agent_response(1, 1, &oversized, false)))
        .expect_err("oversized agent frames must be rejected");
}
