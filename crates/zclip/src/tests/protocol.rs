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

#[test]
fn filter_cow_borrows_output_without_an_osc_and_still_finds_a_split_frame() {
    let frame = Frame {
        id: [2; 16],
        message: Message::Probe,
    };
    let mut scanner = Scanner::default();
    let mut found = Vec::new();
    assert!(matches!(
        scanner.filter_cow(b"plain text\r\n", |_| panic!("no frame")),
        Cow::Borrowed(_)
    ));
    assert!(matches!(
        scanner.filter_cow(b"\x1b[1mbold\x1b[0m", |_| panic!("no frame")),
        Cow::Borrowed(_)
    ));

    // The second read carries no ESC at all, so only the held partial frame sends it through
    // the state machine.
    let encoded = frame.encode();
    let (escape, rest) = encoded.split_at(1);
    let mut first = b"text".to_vec();
    first.extend_from_slice(escape);
    let mut output = scanner
        .filter_cow(&first, |frame| found.push(frame))
        .into_owned();
    assert!(!rest.contains(&0x1b));
    output.extend_from_slice(&scanner.filter_cow(rest, |frame| found.push(frame)));

    assert_eq!(output, b"text");
    assert_eq!(found, vec![frame]);
}
