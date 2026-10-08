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

/// The review's reproducer: 32 bytes of ID whose first pair ends inside `€`.
#[test]
fn a_multi_byte_request_id_is_rejected_without_panicking() {
    let payload = format!("\x1b]777;zclip;1;€{};probe\x07", "0".repeat(29));
    assert_eq!(payload.len(), PREFIX.len() + 32 + ";probe\x07".len());
    assert!(Frame::parse(payload.as_bytes()).is_none());
    let mut scanner = Scanner::default();
    assert_eq!(
        scanner.filter(payload.as_bytes(), |_| panic!("malformed frame")),
        payload.as_bytes()
    );
}

/// Every multi-byte width, at every offset of a 32-byte ID, delivered in every
/// fragment size the scanner can see.
#[test]
fn unicode_request_ids_at_every_boundary_are_preserved_unparsed() {
    for character in ['é', '€', '😀'] {
        let width = character.len_utf8();
        for offset in 0..=32 - width {
            let id = format!(
                "{}{character}{}",
                "a".repeat(offset),
                "b".repeat(32 - width - offset)
            );
            assert_eq!(id.len(), 32);
            for kind in ["probe", "copy", "end", "ack;1", "data;0;aGk"] {
                for terminator in ["\x07", "\x1b\\"] {
                    let payload = format!("\x1b]777;zclip;1;{id};{kind}{terminator}");
                    assert!(Frame::parse(payload.as_bytes()).is_none(), "{payload:?}");
                    for fragment in 1..=payload.len() {
                        let mut scanner = Scanner::default();
                        let mut output = Vec::new();
                        for part in payload.as_bytes().chunks(fragment) {
                            output.extend(
                                scanner
                                    .filter_cow(part, |_| panic!("malformed frame"))
                                    .iter(),
                            );
                        }
                        output.extend(scanner.finish());
                        assert_eq!(output, payload.as_bytes());
                    }
                }
            }
        }
    }
}

#[test]
fn request_ids_accept_only_hex_digits() {
    let valid = Frame {
        id: [0xab; 16],
        message: Message::Probe,
    };
    let upper = String::from_utf8(valid.encode())
        .unwrap()
        .replace("abab", "ABAB");
    assert_eq!(Frame::parse(upper.as_bytes()), Some(valid));
    // `u8::from_str_radix` takes a leading sign, so `+1` used to decode.
    let signed = format!("\x1b]777;zclip;1;+1{};probe\x07", "0".repeat(30));
    assert!(Frame::parse(signed.as_bytes()).is_none());
    let non_hex = format!("\x1b]777;zclip;1;g0{};probe\x07", "0".repeat(30));
    assert!(Frame::parse(non_hex.as_bytes()).is_none());
}

/// Deterministic fuzzing: arbitrary bytes, biased towards the scanner's own
/// tokens, through every scanner entry point in arbitrary fragments.
#[test]
fn arbitrary_output_never_panics_the_scanner() {
    const TOKENS: &[&[u8]] = &[
        b"\x1b",
        b"\x1b]",
        b"\x1b]777;zclip;1;",
        b"\x1b]52;c;",
        b"\x07",
        b"\x1b\\",
        b"\x18",
        b";",
        b"probe",
        b"data;0;",
        b"ack;",
        b"0123456789abcdef",
        "€".as_bytes(),
        "😀".as_bytes(),
        b"\xe2\x82",
        b"\xff",
    ];
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        // xorshift64*: reproducible without a dependency.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    };
    let valid = Frame {
        id: [3; 16],
        message: Message::Ack { next_sequence: 9 },
    }
    .encode();
    for _ in 0..4_000 {
        let mut input = Vec::new();
        for _ in 0..next() % 48 {
            match next() % 8 {
                0..=3 => input.extend_from_slice(TOKENS[(next() % TOKENS.len() as u64) as usize]),
                4 => input.extend_from_slice(&valid),
                5 => {
                    // Frame-shaped: an ID of about the right byte length, mixing
                    // hex digits with multi-byte characters.
                    input.extend_from_slice(PREFIX);
                    let target = 28 + (next() % 8) as usize;
                    let start = input.len();
                    while input.len() - start < target {
                        if next() % 4 == 0 {
                            input.extend_from_slice(TOKENS[12 + (next() % 2) as usize]);
                        } else {
                            input.push(b"0123456789abcdef"[(next() % 16) as usize]);
                        }
                    }
                    input.push(b';');
                    input.extend_from_slice(TOKENS[8 + (next() % 3) as usize]);
                    input.extend_from_slice(TOKENS[4 + (next() % 2) as usize]);
                }
                _ => input.push(next() as u8),
            }
        }
        // Frame::parse is also called directly on relayed sequences.
        let _ = Frame::parse(&input);
        let start = (next() as usize) % (input.len() + 1);
        let _ = Frame::parse(&input[start..]);

        let mut scanner = Scanner::default();
        let mut observer = Scanner::default();
        let mut output = Vec::new();
        let mut frames = 0;
        let mut rest = &input[..];
        while !rest.is_empty() {
            let take = 1 + (next() as usize) % rest.len();
            let (part, tail) = rest.split_at(take);
            if next() % 2 == 0 {
                output.extend_from_slice(&scanner.filter(part, |_| frames += 1));
            } else {
                output.extend_from_slice(&scanner.filter_cow(part, |_| frames += 1));
            }
            observer.observe(part, |_| {});
            rest = tail;
        }
        output.extend(scanner.finish());
        if frames == 0 {
            assert_eq!(output, input);
        } else {
            assert!(output.len() < input.len());
        }
    }
}
