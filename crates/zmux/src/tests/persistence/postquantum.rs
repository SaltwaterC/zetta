use super::*;

#[test]
fn pq_identity_has_a_canonical_round_trip() {
    let identity = MlKem768X25519Identity::generate();
    let encoded = identity.to_string();
    assert!(encoded.starts_with("AGE-SECRET-KEY-PQ-1"));
    assert_eq!(encoded, encoded.to_ascii_uppercase());
    let parsed = MlKem768X25519Identity::from_str(&encoded).unwrap();
    assert_eq!(parsed.to_string(), encoded);
    let recipient = parsed.to_recipient().to_string();
    assert!(recipient.starts_with("age1pq1"));
    assert_eq!(
        MlKem768X25519Recipient::from_str(&recipient)
            .unwrap()
            .to_string(),
        recipient
    );
}

#[test]
fn pq_age_stanza_round_trips_through_the_age_stream() {
    let identity = MlKem768X25519Identity::generate();
    let recipient = identity.to_recipient().to_string();
    let recipients = RecipientSet::parse(&[recipient]).unwrap();
    let ciphertext = recipients.encrypt(b"post-quantum session state").unwrap();
    let identity = IdentitySet {
        identities: vec![Box::new(identity)],
    };
    assert_eq!(
        identity.decrypt(&ciphertext).unwrap(),
        b"post-quantum session state"
    );
}

#[test]
fn pq_recipients_cannot_mix_with_classical_recipients() {
    let pq = MlKem768X25519Identity::generate()
        .to_recipient()
        .to_string();
    let classical = age::x25519::Identity::generate().to_public().to_string();
    let error = RecipientSet::parse(&[pq, classical]).unwrap_err();
    assert!(error.to_string().contains("cannot be mixed"));
}
