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

const THIS_HOST: &str = "SHA256:bTBKZhY1f9R2YWiSdzVWcmJworfDpk06n3w9YpZPWJU";
const ANOTHER_HOST: &str = "SHA256:2c0Q0m9v5fW9hX0p8yq8m7l6k5j4h3g2f1e0d9c8b7a";

/// An identity on disk and the recipients that seal to it.
fn sealing_pair(directory: &std::path::Path) -> (IdentitySet, RecipientSet) {
    let (path, recipient) = identity_file(directory);
    (
        IdentitySet::from_paths(&[path]).unwrap(),
        RecipientSet::parse(&[recipient]).unwrap(),
    )
}

fn trusting(host_keys: &'static [&'static str]) -> impl FnOnce() -> Result<Vec<String>> {
    move || Ok(host_keys.iter().map(|key| (*key).to_owned()).collect())
}

/// The substitution: a host offering an envelope that was sealed on another
/// host — one the user's identity opens perfectly well — must not be sent the
/// key inside it.
#[test]
fn a_key_sealed_on_another_host_is_not_opened_for_it() {
    let directory = tempfile::tempdir().unwrap();
    let (identities, recipients) = sealing_pair(directory.path());
    let elsewhere = seal_for_host(&recipients, &[ANOTHER_HOST.to_owned()]).unwrap();
    assert!(
        open(envelope(&elsewhere), &identities).is_ok(),
        "the envelope is one this identity can open"
    );

    let error = open_for_remote_with(
        envelope(&elsewhere),
        &identities,
        "this-host",
        trusting(&[THIS_HOST]),
    )
    .expect_err("another host's sealed key must not be released for this one");

    assert!(error.to_string().contains("was not sent"), "{error:#}");
    let key = elsewhere.secret.expose().rsplit("key=").next().unwrap();
    assert!(!format!("{error:#}").contains(key));
}

#[test]
fn a_key_sealed_on_the_destination_opens_for_it() {
    let directory = tempfile::tempdir().unwrap();
    let (identities, recipients) = sealing_pair(directory.path());
    let sealed = seal_for_host(
        &recipients,
        &[ANOTHER_HOST.to_owned(), THIS_HOST.to_owned()],
    )
    .unwrap();

    let opened = open_for_remote_with(
        envelope(&sealed),
        &identities,
        "this-host",
        trusting(&[THIS_HOST]),
    )
    .unwrap();

    assert_eq!(opened.expose(), sealed.secret.expose());
    assert!(sealed.authentication.verify(opened.expose()).is_some());
}

/// The binding is inside what the verifier was computed over, so it cannot be
/// stripped off: the bare key on its own does not open the session.
#[test]
fn the_binding_is_part_of_the_secret() {
    let directory = tempfile::tempdir().unwrap();
    let (_, recipients) = sealing_pair(directory.path());
    let sealed = seal_for_host(&recipients, &[THIS_HOST.to_owned()]).unwrap();

    assert_eq!(
        SealedKey::parse(sealed.secret.expose()).unwrap(),
        SealedKey::Bound {
            host_keys: vec![THIS_HOST.to_owned()]
        }
    );
    let bare_key = sealed.secret.expose().rsplit("key=").next().unwrap();
    assert_eq!(STANDARD_NO_PAD.decode(bare_key).unwrap().len(), KEY_BYTES);
    assert!(sealed.authentication.verify(bare_key).is_none());
}

/// A version 1 envelope — the bare key — still opens for this machine's own
/// multiplexer, but names no host, so it is never released to a remote one.
#[test]
fn an_unbound_envelope_opens_locally_but_not_for_a_remote_host() {
    let directory = tempfile::tempdir().unwrap();
    let (identities, recipients) = sealing_pair(directory.path());
    let key = STANDARD_NO_PAD.encode([7; KEY_BYTES]);
    let legacy = STANDARD_NO_PAD.encode(recipients.encrypt(key.as_bytes()).unwrap());

    assert_eq!(open(&legacy, &identities).unwrap().expose(), key);

    let mut asked = false;
    let error = open_for_remote_with(&legacy, &identities, "this-host", || {
        asked = true;
        Ok(vec![THIS_HOST.to_owned()])
    })
    .expect_err("an unbound key must not be sent to a remote host");
    assert!(error.to_string().contains("older Zetta"), "{error:#}");
    assert!(!asked, "refused before the destination is even considered");
}

#[test]
fn a_key_sealed_without_host_keys_is_only_opened_locally() {
    let directory = tempfile::tempdir().unwrap();
    let (identities, recipients) = sealing_pair(directory.path());
    let sealed = seal_for_host(&recipients, &[]).unwrap();

    assert!(open(envelope(&sealed), &identities).is_ok());
    assert!(
        open_for_remote_with(
            envelope(&sealed),
            &identities,
            "this-host",
            trusting(&[THIS_HOST])
        )
        .is_err()
    );
}

#[test]
fn a_destination_with_no_trusted_host_keys_is_sent_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let (identities, recipients) = sealing_pair(directory.path());
    let sealed = seal_for_host(&recipients, &[THIS_HOST.to_owned()]).unwrap();

    assert!(
        open_for_remote_with(envelope(&sealed), &identities, "this-host", trusting(&[])).is_err()
    );
    assert!(
        open_for_remote_with(envelope(&sealed), &identities, "this-host", || {
            anyhow::bail!("no ssh")
        })
        .is_err()
    );
}

#[test]
fn a_sealed_text_for_another_purpose_or_version_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let (identities, recipients) = sealing_pair(directory.path());
    let seal_text =
        |text: &str| STANDARD_NO_PAD.encode(recipients.encrypt(text.as_bytes()).unwrap());

    for text in [
        format!("{SEALED_KEY_TAG}2;purpose=disk-unlock;host={THIS_HOST};key=abc"),
        format!("{SEALED_KEY_TAG}3;purpose=session-authentication;host={THIS_HOST};key=abc"),
        format!("{SEALED_KEY_TAG}2;purpose=session-authentication;host={THIS_HOST};key="),
        format!("{SEALED_KEY_TAG}2;purpose=session-authentication;key=abc"),
        format!(
            "{SEALED_KEY_TAG}2;purpose=session-authentication;host={ANOTHER_HOST};host={THIS_HOST};key=abc"
        ),
        format!(
            "{SEALED_KEY_TAG}2;purpose=session-authentication;host={THIS_HOST};key=abc;extra=1"
        ),
    ] {
        let envelope = seal_text(&text);
        assert!(open(&envelope, &identities).is_err(), "{text}");
        assert!(
            open_for_remote_with(&envelope, &identities, "this-host", trusting(&[THIS_HOST]))
                .is_err(),
            "{text}"
        );
    }
}
