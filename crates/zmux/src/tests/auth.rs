use super::*;

#[test]
fn session_authentication_uses_a_salted_argon2id_verifier() {
    let first = SessionAuthentication::create("sensitive session").unwrap();
    let second = SessionAuthentication::create("sensitive session").unwrap();

    assert!(first.encoded().starts_with("$argon2id$"));
    assert!(!first.encoded().contains("sensitive session"));
    assert_ne!(first.encoded(), second.encoded());
    assert!(first.verify("sensitive session").is_some());
    assert!(first.verify("changed value").is_none());

    // Authorization is scoped to the session whose secret was checked.
    let authorization = first.verify("sensitive session").unwrap();
    assert!(first.authorizes(&authorization));
    assert!(!second.authorizes(&authorization));
}

#[test]
fn only_verifying_a_secret_produces_a_reconnect_authorization() {
    let authentication = SessionAuthentication::create("secret").unwrap();

    // A clone of the verifier is not itself authorization: `authorizes` takes a
    // `VerifiedSession`, and `verify` is the only way to construct one. This is
    // the invariant reattaching a protected session relies on, so if a future
    // refactor reintroduces a public constructor this stops compiling.
    assert!(authentication.verify("wrong").is_none());
    let authorization = authentication
        .verify("secret")
        .expect("the correct secret must authorize");
    assert!(authentication.clone().authorizes(&authorization));
}

#[test]
fn failed_authentication_backoff_doubles_and_saturates() {
    assert_eq!(failed_authentication_delay(0), Duration::from_secs(1));
    assert_eq!(failed_authentication_delay(1), Duration::from_secs(1));
    assert_eq!(failed_authentication_delay(2), Duration::from_secs(2));
    assert_eq!(failed_authentication_delay(3), Duration::from_secs(4));
    assert_eq!(failed_authentication_delay(4), Duration::from_secs(8));
    assert_eq!(failed_authentication_delay(5), Duration::from_secs(16));
    // Capped, and no overflow at absurd failure counts.
    assert_eq!(failed_authentication_delay(6), Duration::from_secs(30));
    assert_eq!(failed_authentication_delay(64), Duration::from_secs(30));
    assert_eq!(
        failed_authentication_delay(u32::MAX),
        Duration::from_secs(30)
    );
}

#[test]
fn session_secrets_are_not_rendered_by_debug() {
    let secret = SessionSecret::new("hunter2".to_owned());

    assert_eq!(format!("{secret:?}"), "SessionSecret(<redacted>)");
    assert!(!format!("{secret:?}").contains("hunter2"));
    assert_eq!(secret.expose(), "hunter2");
}

/// A verifier with every field of an ordinary one, but the given costs. The
/// hash is not a real one: these are refused before anything is hashed.
fn verifier_with_costs(memory_kib: u32, passes: u32, lanes: u32) -> String {
    format!(
        "$argon2id$v=19$m={memory_kib},t={passes},p={lanes}$c29tZXNhbHRzb21lc2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    )
}

#[test]
fn a_created_verifier_is_within_the_import_policy() {
    let created = SessionAuthentication::create("sensitive session").unwrap();
    assert!(created.encoded().contains("$v=19$m=19456,t=2,p=1$"));

    let imported = SessionAuthentication::from_verifier(created.encoded().to_owned())
        .expect("whatever `create` writes must import, or protecting a session breaks");
    assert!(imported.verify("sensitive session").is_some());
}

/// A verifier written by argon2 0.5.3's defaults before the policy existed —
/// the shape every persisted record and handed-over session carries — still
/// imports and still opens with its secret.
#[test]
fn a_verifier_from_before_the_policy_still_imports() {
    // Written by `SessionAuthentication::create("kept secret")` with argon2
    // 0.5.3, its salt fixed so the fixture is reproducible.
    let recorded = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$8L48kLA1ei86vwDWfBzujaQRnXkgRhEZoLY+1X6H35c".to_owned();

    let imported = SessionAuthentication::from_verifier(recorded).unwrap();
    assert!(imported.verify("kept secret").is_some());
    assert!(imported.verify("another secret").is_none());
}

#[test]
fn a_verifier_asking_for_excessive_costs_is_refused_on_import() {
    for (memory_kib, passes, lanes) in [
        (4 * 1024 * 1024, 2, 1),
        (MAX_VERIFIER_MEMORY_KIB + 1, 2, 1),
        (19456, 3, 1),
        (19456, 1_000_000, 1),
        (19456, 2, 8),
    ] {
        let verifier = verifier_with_costs(memory_kib, passes, lanes);
        let error = SessionAuthentication::from_verifier(verifier.clone())
            .err()
            .unwrap_or_else(|| panic!("{verifier} must be refused"));
        assert!(error.to_string().contains("exceed"), "{error:#}");
    }
    // The ceiling itself is accepted.
    assert!(SessionAuthentication::from_verifier(verifier_with_costs(19456, 2, 1)).is_ok());
    assert!(SessionAuthentication::from_verifier(verifier_with_costs(8192, 1, 1)).is_ok());
}

#[test]
fn a_verifier_of_another_algorithm_or_shape_is_refused_on_import() {
    let salt = "c29tZXNhbHRzb21lc2FsdA";
    let hash = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    for verifier in [
        format!("$argon2i$v=19$m=19456,t=2,p=1${salt}${hash}"),
        format!("$argon2d$v=19$m=19456,t=2,p=1${salt}${hash}"),
        format!("$argon2id$v=16$m=19456,t=2,p=1${salt}${hash}"),
        format!("$argon2id$m=19456,t=2,p=1${salt}${hash}"),
        // An eight-byte salt, and a 16-byte hash.
        format!("$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ${hash}"),
        format!("$argon2id$v=19$m=19456,t=2,p=1${salt}$AAAAAAAAAAAAAAAAAAAAAA"),
        format!("$argon2id$v=19$m=19456,t=2,p=1${salt}"),
        format!("$argon2id$v=19$m=19456,t=2,p=1,keyid=AAAA${salt}${hash}"),
        format!("$scrypt$ln=16,r=8,p=1${salt}${hash}"),
        "not a verifier".to_owned(),
    ] {
        assert!(
            SessionAuthentication::from_verifier(verifier.clone()).is_err(),
            "{verifier} must be refused"
        );
    }
}

/// A handed-over verifier outside the policy keeps its session protected, and
/// is refused by `verify` without Argon2 running — even for its own secret.
#[test]
fn an_adopted_verifier_outside_the_policy_is_never_run() {
    let salt = SaltString::from_b64("c29tZXNhbHRzb21lc2FsdA").unwrap();
    let costly = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(MAX_VERIFIER_MEMORY_KIB, MAX_VERIFIER_PASSES + 1, 1, None).unwrap(),
    )
    .hash_password(b"handed over", &salt)
    .unwrap()
    .to_string();
    assert!(SessionAuthentication::from_verifier(costly.clone()).is_err());

    let adopted = SessionAuthentication::adopt_verifier(costly.clone())
        .expect("an over-cost verifier must not fail the handover it came in");
    let probe = verification_probe::watch(&costly, Duration::ZERO);
    assert!(adopted.verify("handed over").is_none());
    assert_eq!(probe.runs(), 0, "Argon2 must not run for it at all");

    assert!(SessionAuthentication::adopt_verifier("not a verifier".to_owned()).is_err());
}

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
fn a_session_secret_is_carried_as_the_plain_string_it_always_was() {
    let secret = SessionSecret::new("hunter2".to_owned());
    assert_eq!(serde_json::to_string(&secret).unwrap(), "\"hunter2\"");
    let read: SessionSecret = serde_json::from_str("\"hunter\\u0032\"").unwrap();
    assert_eq!(read, secret);
    assert_eq!(Some(read).as_deref(), Some("hunter2"));
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
