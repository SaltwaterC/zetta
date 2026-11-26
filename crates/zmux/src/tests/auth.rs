use super::*;

#[test]
fn a_request_carrying_a_secret_is_framed_without_growing_its_buffer() {
    let envelope = crate::messages::Envelope {
        version: crate::messages::PROTOCOL_VERSION,
        token: "token".to_owned(),
        client_process_id: 1,
        client_id: crate::messages::ClientId::default(),
        stream_only: false,
        session_secret: Some(SessionSecret::new("s".repeat(3000))),
        request: crate::messages::Request::Ping,
    };
    let frame = crate::transport::encode_secret_message(&envelope).unwrap();
    assert_eq!(frame.capacity(), frame.len());
    // On the wire it is exactly the frame it always was.
    assert_eq!(
        frame.as_slice(),
        crate::transport::encode_message(&envelope).unwrap()
    );
}

#[test]
fn secret_json_is_built_at_its_final_size_after_its_prefix() {
    let secret = SessionSecret::new("hunter2 \"quoted\" \u{e9}".to_owned());
    let encoded = encode_secret_json(&Some(&secret), 4).unwrap();
    assert_eq!(encoded.capacity(), encoded.len());
    assert_eq!(&encoded[..4], &[0; 4]);
    assert_eq!(&encoded[4..], serde_json::to_vec(&Some(&secret)).unwrap());
}

#[test]
fn secret_bytes_keep_their_contents_as_they_grow() {
    let mut buffer = SecretBytes::default();
    for byte in 0..=255_u8 {
        buffer.extend_from_slice(&[byte; 3]);
    }
    assert_eq!(buffer.len(), 768);
    let expected = (0..=255_u8).flat_map(|byte| [byte; 3]).collect::<Vec<_>>();
    assert_eq!(buffer.as_slice(), expected);
}

#[test]
fn a_secret_read_with_its_length_known_is_never_reallocated() {
    // Longer than one read, so a buffer sized by what has arrived would grow.
    let bytes = (0..20_000).map(|index| index as u8).collect::<Vec<_>>();
    let read = read_secret_to_end(&mut bytes.as_slice(), bytes.len(), 1 << 20).unwrap();
    assert_eq!(*read, bytes);
    assert_eq!(read.capacity(), bytes.len());

    let error = read_secret_to_end(&mut bytes.as_slice(), bytes.len(), 19_999).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn secret_text_that_is_not_utf8_is_refused_without_panicking() {
    let text = secret_utf8(Zeroizing::new(b"hunter2".to_vec())).unwrap();
    assert_eq!(text.as_str(), "hunter2");
    let error = secret_utf8(Zeroizing::new(b"hunt\xffer2".to_vec())).unwrap_err();
    assert!(error.to_string().contains("byte 4"), "{error}");
}

#[test]
fn a_typed_secret_keeps_every_character_as_it_grows() {
    let mut secret = Zeroizing::new(String::new());
    let typed = "p\u{e4}ss\u{1f512}w\u{f6}rd".repeat(20);
    for character in typed.chars() {
        push_secret_char(&mut secret, character);
    }
    assert_eq!(secret.as_str(), typed);
}
