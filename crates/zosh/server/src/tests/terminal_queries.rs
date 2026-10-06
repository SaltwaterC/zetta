use super::*;

#[test]
fn osc52_writes_are_forwarded_once_across_split_reads() {
    for terminator in [&b"\x07"[..], b"\x1b\\"] {
        let mut responder = QueryResponder::new();
        let mut sequence = b"\x1b]52;c;c2VsZWN0ZWQgdGV4dA==".to_vec();
        sequence.extend_from_slice(terminator);
        for chunk in sequence.chunks(3) {
            assert!(responder.feed(chunk, (0, 0), (24, 80)).is_empty());
        }
        assert_eq!(responder.take_terminal_queries(), vec![sequence]);
        assert!(responder.take_terminal_queries().is_empty());
    }
}

#[test]
fn osc52_reads_are_not_forwarded() {
    let mut responder = QueryResponder::new();
    responder.feed(b"\x1b]52;c;?\x07\x1b]52;p;?\x1b\\", (0, 0), (24, 80));
    assert!(responder.take_terminal_queries().is_empty());
}

#[test]
fn a_large_osc52_copy_survives_the_scanner() {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(vec![b'x'; 100_000]);
    let sequence = format!("\x1b]52;c;{encoded}\x07").into_bytes();
    let mut responder = QueryResponder::new();
    for chunk in sequence.chunks(4096) {
        responder.feed(chunk, (0, 0), (24, 80));
    }
    assert_eq!(responder.take_terminal_queries(), vec![sequence]);
}

#[test]
fn an_oversized_copy_is_dropped_without_losing_the_next_query() {
    let mut sequence = b"\x1b]52;c;".to_vec();
    sequence.resize(zclip::osc52::MAX_SEQUENCE_BYTES + 1, b'A');
    sequence.extend_from_slice(b"\x07\x1b]10;?\x07");
    let mut responder = QueryResponder::new();
    responder.feed(&sequence, (0, 0), (24, 80));
    assert_eq!(
        responder.take_terminal_queries(),
        vec![b"\x1b]10;?\x07".to_vec()]
    );
}

#[test]
fn query_split_across_reads() {
    let mut responder = QueryResponder::new();
    assert!(responder.feed(b"hello\x1b[", (2, 3), (24, 80)).is_empty());
    assert_eq!(
        responder.feed(b"6n", (2, 3), (24, 80)),
        vec![b"\x1b[3;4R".to_vec()]
    );
}

#[test]
fn osc_color_queries_forward_with_bel_and_st_terminators() {
    let mut responder = QueryResponder::new();
    assert!(responder.feed(b"\x1b]10;?", (0, 0), (24, 80)).is_empty());
    assert!(responder.take_terminal_queries().is_empty());
    assert!(responder.feed(b"\x07", (0, 0), (24, 80)).is_empty());
    assert_eq!(
        responder.take_terminal_queries(),
        vec![b"\x1b]10;?\x07".to_vec()]
    );

    assert!(
        responder
            .feed(b"\x1b]11;?\x1b", (0, 0), (24, 80))
            .is_empty()
    );
    assert_eq!(
        responder.feed(b"\\", (0, 0), (24, 80)),
        Vec::<Vec<u8>>::new()
    );
    assert_eq!(
        responder.take_terminal_queries(),
        vec![b"\x1b]11;?\x1b\\".to_vec()]
    );
}

#[test]
fn malformed_osc_sequences_do_not_become_color_queries() {
    let mut responder = QueryResponder::new();
    responder.feed(
        b"\x1b]10;not-a-query\x07\x1b]12;?\x07\x1b]10;?\x18\x1b]10;?\x07",
        (0, 0),
        (24, 80),
    );
    assert_eq!(
        responder.take_terminal_queries(),
        vec![b"\x1b]10;?\x07".to_vec()]
    );
}

#[test]
fn clipboard_frames_are_forwarded_once_even_when_split() {
    let mut responder = QueryResponder::new();
    let frame = zclip::protocol::Frame {
        id: [4; 16],
        message: zclip::protocol::Message::Data {
            sequence: 7,
            bytes: vec![b'x'; zclip::protocol::CHUNK_SIZE],
        },
    }
    .encode();
    for part in frame.chunks(500) {
        assert!(responder.feed(part, (0, 0), (24, 80)).is_empty());
    }
    assert_eq!(responder.take_terminal_queries(), vec![frame]);
    assert!(responder.take_terminal_queries().is_empty());
}

#[test]
fn ordinary_csi_queries_keep_their_local_replies() {
    let mut responder = QueryResponder::new();
    assert_eq!(
        responder.feed(b"\x1b[6n\x1b[5n", (2, 3), (24, 80)),
        vec![b"\x1b[3;4R".to_vec(), b"\x1b[0n".to_vec()]
    );
    assert!(responder.take_terminal_queries().is_empty());
}
