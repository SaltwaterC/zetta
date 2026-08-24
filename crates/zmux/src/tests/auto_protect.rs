use std::fs;

use age::secrecy::ExposeSecret as _;

use super::*;

/// Writes an age identity where [`IdentitySet::from_paths`] can load it, and
/// returns the matching recipient. Going through a file rather than constructing
/// an `IdentitySet` directly is deliberate: it is the same path the application
/// and the CLI take, so a change to identity loading is exercised here too.
/// The envelope a seal produced. It lives on the authentication, so that the
/// verifier and the way back in cannot be carried separately; these tests are
/// the one place that wants it on its own.
fn envelope(sealed: &SealedSessionKey) -> &str {
    sealed
        .authentication
        .key_envelope()
        .expect("a sealed key always carries its envelope")
}

fn identity_file(directory: &std::path::Path) -> (std::path::PathBuf, String) {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let path = directory.join("identity.txt");
    fs::write(&path, format!("{}\n", identity.to_string().expose_secret())).unwrap();
    (path, recipient)
}

#[test]
fn a_sealed_key_is_recovered_by_the_matching_identity() {
    let directory = tempfile::tempdir().unwrap();
    let (path, recipient) = identity_file(directory.path());
    let recipients = RecipientSet::parse(&[recipient]).unwrap();

    let sealed = seal(&recipients).unwrap();
    let identities = IdentitySet::from_paths(&[path]).unwrap();
    let opened = open(envelope(&sealed), &identities).unwrap();

    assert_eq!(opened.expose(), sealed.secret.expose());
}

#[test]
fn a_sealed_key_verifies_against_its_own_verifier_and_no_other() {
    let recipient = age::x25519::Identity::generate().to_public().to_string();
    let recipients = RecipientSet::parse(&[recipient]).unwrap();

    let sealed = seal(&recipients).unwrap();
    let other = seal(&recipients).unwrap();

    assert!(
        sealed
            .authentication
            .verify(sealed.secret.expose())
            .is_some()
    );
    assert!(
        sealed
            .authentication
            .verify(other.secret.expose())
            .is_none()
    );
}

#[test]
fn an_unrelated_identity_cannot_open_the_envelope() {
    let directory = tempfile::tempdir().unwrap();
    let (_, recipient) = identity_file(directory.path());
    let recipients = RecipientSet::parse(&[recipient]).unwrap();
    let sealed = seal(&recipients).unwrap();

    let other = tempfile::tempdir().unwrap();
    let (stranger, _) = identity_file(other.path());
    let identities = IdentitySet::from_paths(&[stranger]).unwrap();

    assert!(open(envelope(&sealed), &identities).is_err());
}

/// The envelope is published — in the catalog, and inside the record — so the
/// one thing it must never be is the key with an encoding wrapped round it.
#[test]
fn the_envelope_is_an_age_file_and_not_the_key() {
    let recipient = age::x25519::Identity::generate().to_public().to_string();
    let recipients = RecipientSet::parse(&[recipient]).unwrap();
    let sealed = seal(&recipients).unwrap();

    assert!(!envelope(&sealed).contains(sealed.secret.expose()));
    let ciphertext = STANDARD_NO_PAD.decode(envelope(&sealed)).unwrap();
    assert!(ciphertext.starts_with(b"age-encryption.org/v1\n"));
    assert!(
        !ciphertext
            .windows(sealed.secret.expose().len())
            .any(|window| window == sealed.secret.expose().as_bytes())
    );
}

#[test]
fn two_seals_never_produce_the_same_key() {
    let recipient = age::x25519::Identity::generate().to_public().to_string();
    let recipients = RecipientSet::parse(&[recipient]).unwrap();

    let first = seal(&recipients).unwrap();
    let second = seal(&recipients).unwrap();

    assert_ne!(first.secret.expose(), second.secret.expose());
    assert_ne!(envelope(&first), envelope(&second));
}

#[test]
fn sealing_without_recipients_is_refused() {
    let recipients = RecipientSet::parse(&[]).unwrap();
    assert!(seal(&recipients).is_err());
}

#[test]
fn post_quantum_recipients_seal_and_open() {
    let directory = tempfile::tempdir().unwrap();
    let identity = crate::persistence::MlKem768X25519Identity::generate();
    let path = directory.path().join("identity-pq.txt");
    fs::write(&path, format!("{identity}\n")).unwrap();
    let recipients = RecipientSet::parse(&[identity.to_recipient().to_string()]).unwrap();

    let sealed = seal(&recipients).unwrap();
    let identities = IdentitySet::from_paths(&[path]).unwrap();

    assert_eq!(
        open(envelope(&sealed), &identities).unwrap().expose(),
        sealed.secret.expose()
    );
}

#[test]
fn a_corrupt_envelope_is_an_error_rather_than_a_panic() {
    let directory = tempfile::tempdir().unwrap();
    let (path, recipient) = identity_file(directory.path());
    let identities = IdentitySet::from_paths(&[path]).unwrap();
    let recipients = RecipientSet::parse(&[recipient]).unwrap();
    let sealed = seal(&recipients).unwrap();

    assert!(open("not base64 at all !!", &identities).is_err());
    assert!(open("", &identities).is_err());
    let truncated = &envelope(&sealed)[..envelope(&sealed).len() / 2];
    assert!(open(truncated, &identities).is_err());
}
