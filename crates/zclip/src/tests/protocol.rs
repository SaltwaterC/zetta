use super::*;

#[test]
fn frame_round_trip_and_size_boundary() {
    let id = [0x5a; 16];
    for message in [
        Message::Probe,
        Message::Ready,
        Message::Copy,
        Message::Paste,
        Message::Data {
            sequence: 42,
            bytes: vec![0xff; CHUNK_SIZE],
        },
        Message::Ack { next_sequence: 43 },
        Message::End,
        Message::Done,
        Message::Error("paste access denied".into()),
    ] {
        let frame = Frame { id, message };
        assert_eq!(Frame::parse(&frame.encode()), Some(frame));
    }
    let too_large = Frame {
        id,
        message: Message::Data {
            sequence: 0,
            bytes: vec![0; CHUNK_SIZE + 1],
        },
    };
    assert!(Frame::parse(&too_large.encode()).is_none());
}

#[test]
fn scanner_removes_only_complete_clipboard_frames() {
    let frame = Frame {
        id: [1; 16],
        message: Message::Probe,
    };
    let mut scanner = Scanner::default();
    let mut found = Vec::new();
    let mut data = b"before\x1b]52;c;SGk=\x07".to_vec();
    data.extend(frame.encode());
    data.extend_from_slice(b"after");
    let mut output = Vec::new();
    for part in data.chunks(3) {
        output.extend(scanner.filter(part, |frame| found.push(frame)));
    }
    assert_eq!(output, b"before\x1b]52;c;SGk=\x07after");
    assert_eq!(found, vec![frame]);
    assert!(scanner.finish().is_empty());
}

#[test]
fn malformed_and_duplicate_fields_are_preserved() {
    let mut scanner = Scanner::default();
    let bytes = b"\x1b]777;zclip;1;00000000000000000000000000000000;probe;extra\x07";
    assert_eq!(scanner.filter(bytes, |_| panic!("malformed frame")), bytes);
}
