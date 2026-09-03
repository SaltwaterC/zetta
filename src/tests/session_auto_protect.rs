use std::fs;

use age::secrecy::ExposeSecret as _;

use super::*;

/// Writes an age identity and returns a persistence configuration that seals to
/// it, with automatic protection turned on.
fn configured(directory: &std::path::Path) -> SessionPersistenceConfig {
    let identity = age::x25519::Identity::generate();
    let path = directory.join("identity.txt");
    fs::write(&path, format!("{}\n", identity.to_string().expose_secret())).unwrap();
    SessionPersistenceConfig {
        recipients: vec![identity.to_public().to_string()],
        identity: Some(path),
        auto_protect: true,
    }
}

#[test]
fn a_sealed_session_round_trips_through_the_configured_key_pair() {
    let directory = tempfile::tempdir().unwrap();
    let auto_protect = SessionAutoProtect::resolve(&configured(directory.path()))
        .unwrap()
        .expect("a recipient and a readable identity are configured");

    let sealed = auto_protect.seal().unwrap();
    let envelope = sealed
        .authentication
        .key_envelope()
        .expect("a sealed key carries its envelope");
    let recovered = auto_protect.open(envelope, None).unwrap();

    assert_eq!(recovered.expose(), sealed.secret.expose());
    // The recovered key is what the daemon will be given, so it has to satisfy
    // the verifier that went with it.
    assert!(sealed.authentication.verify(recovered.expose()).is_some());
}

#[test]
fn automatic_protection_needs_the_flag_a_recipient_and_an_identity() {
    let directory = tempfile::tempdir().unwrap();
    let complete = configured(directory.path());
    assert!(complete.auto_protect_is_configured());

    let without_flag = SessionPersistenceConfig {
        auto_protect: false,
        ..complete.clone()
    };
    let without_recipients = SessionPersistenceConfig {
        recipients: Vec::new(),
        ..complete.clone()
    };
    for config in [&without_flag, &without_recipients] {
        assert!(!config.auto_protect_is_configured());
        assert!(
            SessionAutoProtect::resolve(config).unwrap().is_none(),
            "{config:?} should not resolve to automatic protection"
        );
    }

    let without_identity = SessionPersistenceConfig {
        identity: None,
        ..complete
    };
    let has_default_identity = crate::config::default_session_identity_path().is_some();
    assert_eq!(
        without_identity.auto_protect_is_configured(),
        has_default_identity
    );
    assert_eq!(
        SessionAutoProtect::resolve(&without_identity)
            .unwrap()
            .is_some(),
        has_default_identity
    );
}

/// A path that points at nothing is the same footgun as no path at all, and the
/// worst moment to find out is when a session is being reattached. Resolution
/// declines instead, so the dialog is used and the session stays openable.
#[test]
fn an_identity_that_is_not_there_declines_rather_than_sealing() {
    let directory = tempfile::tempdir().unwrap();
    let config = SessionPersistenceConfig {
        identity: Some(directory.path().join("absent.txt")),
        ..configured(directory.path())
    };
    assert!(config.auto_protect_is_configured());
    assert!(SessionAutoProtect::resolve(&config).unwrap().is_none());
}

#[test]
fn an_unusable_recipient_is_reported_rather_than_silently_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let config = SessionPersistenceConfig {
        recipients: vec!["not-an-age-recipient".to_owned()],
        ..configured(directory.path())
    };
    assert!(SessionAutoProtect::resolve(&config).is_err());
}

#[test]
fn a_stranger_key_pair_cannot_open_a_sealed_session() {
    let directory = tempfile::tempdir().unwrap();
    let auto_protect = SessionAutoProtect::resolve(&configured(directory.path()))
        .unwrap()
        .unwrap();
    let sealed = auto_protect.seal().unwrap();

    let other = tempfile::tempdir().unwrap();
    let stranger = SessionAutoProtect::resolve(&configured(other.path()))
        .unwrap()
        .unwrap();

    assert!(
        stranger
            .open(sealed.authentication.key_envelope().unwrap(), None)
            .is_err()
    );
}

/// Only a `github:` alias needs the network; everything else is parsed in
/// microseconds and is resolved where the window is being built.
#[test]
fn only_github_recipients_make_resolution_blocking() {
    let directory = tempfile::tempdir().unwrap();
    let local = configured(directory.path());
    assert!(!SessionAutoProtect::resolution_is_blocking(&local));

    let remote = SessionPersistenceConfig {
        recipients: vec!["github:octocat".to_owned()],
        ..local.clone()
    };
    assert!(SessionAutoProtect::resolution_is_blocking(&remote));

    let mixed = SessionPersistenceConfig {
        recipients: vec![local.recipients[0].clone(), " github:octocat".to_owned()],
        ..local
    };
    assert!(SessionAutoProtect::resolution_is_blocking(&mixed));
}

/// An identity file that is itself encrypted, which is what a passphrase-
/// protected SSH key is. `ssh-keygen` output is used verbatim rather than
/// synthesised, because the failure this guards against was specific to how
/// `age` loads a real one.
fn encrypted_ssh_identity(directory: &std::path::Path) -> Option<SessionPersistenceConfig> {
    let path = directory.join("id_ed25519");
    let generated = std::process::Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "hunter2",
            "-q",
            "-C",
            "zetta-test",
            "-f",
        ])
        .arg(&path)
        .status()
        .ok()?;
    if !generated.success() {
        return None;
    }
    let recipient = fs::read_to_string(path.with_extension("pub")).ok()?;
    Some(SessionPersistenceConfig {
        recipients: vec![recipient.trim().to_owned()],
        identity: Some(path),
        auto_protect: true,
    })
}

/// The bug this exists for: an encrypted identity was loaded with no passphrase,
/// so `age` decrypted nothing and reported "No matching keys found" — naming the
/// wrong problem entirely. The disk-resume path had always supplied one; this
/// one had not.
#[test]
fn an_encrypted_identity_opens_a_sealed_key_when_its_passphrase_is_supplied() {
    let directory = tempfile::tempdir().unwrap();
    let Some(config) = encrypted_ssh_identity(directory.path()) else {
        eprintln!("skipped: ssh-keygen is unavailable");
        return;
    };
    let auto_protect = SessionAutoProtect::resolve(&config).unwrap().unwrap();

    // The window has to be able to tell that asking is necessary; without this
    // it silently takes the path that cannot work.
    assert!(auto_protect.identity_passphrase_required().unwrap());

    let sealed = auto_protect.seal().unwrap();
    let envelope = sealed.authentication.key_envelope().unwrap();

    let without = auto_protect.open(envelope, None);
    assert!(
        without.is_err(),
        "an encrypted identity cannot be opened without its passphrase"
    );

    let opened = auto_protect
        .open(envelope, Some(SessionSecret::new("hunter2".to_owned())))
        .expect("the passphrase opens the identity");
    assert_eq!(opened.expose(), sealed.secret.expose());
    assert!(sealed.authentication.verify(opened.expose()).is_some());
}

/// A plain, unencrypted identity must not start asking for a passphrase it does
/// not have — that would put a dialog in front of every silent unlock.
#[test]
fn an_unencrypted_identity_needs_no_passphrase() {
    let directory = tempfile::tempdir().unwrap();
    let auto_protect = SessionAutoProtect::resolve(&configured(directory.path()))
        .unwrap()
        .unwrap();
    assert!(!auto_protect.identity_passphrase_required().unwrap());
}
