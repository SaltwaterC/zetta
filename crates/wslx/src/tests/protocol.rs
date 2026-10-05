use super::*;
use crate::test_support::agent_frame;
use std::io::Cursor;

fn encoded(message: &Message) -> Vec<u8> {
    let mut bytes = Vec::new();
    message.write_to(&mut bytes).unwrap();
    bytes
}

#[test]
fn every_message_survives_a_round_trip() {
    let messages = [
        Message::Open(1),
        Message::Request(2, agent_frame(&[11])),
        Message::Reply(u32::MAX, agent_frame(&[12, 0, 1])),
        Message::Close(7),
    ];
    let mut stream = Vec::new();
    for message in &messages {
        message.write_to(&mut stream).unwrap();
    }
    let mut input = Cursor::new(stream);
    for message in messages {
        assert_eq!(Message::read_from(&mut input).unwrap(), Some(message));
    }
    assert_eq!(Message::read_from(&mut input).unwrap(), None);
}

#[test]
fn a_stream_cut_inside_a_message_is_an_error_not_an_end() {
    let bytes = encoded(&Message::Request(1, agent_frame(&[11, 1, 2])));
    for cut in 1..bytes.len() {
        let error = Message::read_from(&mut Cursor::new(&bytes[..cut])).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
    }
}

#[test]
fn a_connection_message_with_a_payload_is_rejected() {
    let mut bytes = encoded(&Message::Close(1));
    bytes[8] = 1;
    bytes.push(0);
    assert!(Message::read_from(&mut Cursor::new(bytes)).is_err());
}

#[test]
fn a_payload_that_is_not_exactly_one_agent_message_is_rejected() {
    for payload in [
        vec![],
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 2, 11],
        agent_frame(&[11]).repeat(2),
    ] {
        let mut bytes = vec![REQUEST, 0, 0, 0, 1];
        bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(&payload);
        assert!(
            Message::read_from(&mut Cursor::new(bytes)).is_err(),
            "{payload:?}"
        );
    }
}

#[test]
fn an_oversized_message_is_refused_before_its_payload_is_read() {
    let mut bytes = vec![REPLY, 0, 0, 0, 1];
    bytes.extend_from_slice(&u32::try_from(MAX_AGENT_FRAME + 1).unwrap().to_be_bytes());
    let error = Message::read_from(&mut Cursor::new(bytes)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);

    let oversized = Message::Reply(1, vec![0; MAX_AGENT_FRAME + 1]);
    assert!(oversized.write_to(&mut Vec::new()).is_err());
}

#[test]
fn an_unknown_kind_is_rejected() {
    let bytes = [9, 0, 0, 0, 1, 0, 0, 0, 0];
    assert!(Message::read_from(&mut Cursor::new(bytes)).is_err());
}

#[test]
fn agent_frames_are_read_whole_and_end_cleanly() {
    let first = agent_frame(&[11]);
    let second = agent_frame(&[13, 1, 2, 3]);
    let mut input = Cursor::new([first.clone(), second.clone()].concat());
    assert_eq!(read_agent_frame(&mut input).unwrap(), Some(first));
    assert_eq!(read_agent_frame(&mut input).unwrap(), Some(second));
    assert_eq!(read_agent_frame(&mut input).unwrap(), None);
}

#[test]
fn an_empty_or_oversized_agent_frame_is_rejected() {
    assert!(read_agent_frame(&mut Cursor::new([0, 0, 0, 0])).is_err());
    let too_long = u32::try_from(MAX_AGENT_FRAME).unwrap().to_be_bytes();
    assert!(read_agent_frame(&mut Cursor::new(too_long)).is_err());
}
