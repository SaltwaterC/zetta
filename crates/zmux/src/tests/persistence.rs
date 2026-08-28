use super::*;
use crate::auth::SessionSecret;
use crate::protocol::BackgroundPaneLayout;

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
fn classical_age_round_trip_is_interoperable_with_the_age_crate() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let recipients = RecipientSet::parse(&[recipient]).unwrap();
    let ciphertext = recipients.encrypt(b"classical session state").unwrap();
    assert!(ciphertext.starts_with(b"age-encryption.org/v1\n"));
    let plaintext = age::decrypt(&identity, &ciphertext).unwrap();
    assert_eq!(plaintext, b"classical session state");
}

#[test]
fn identity_set_decrypts_armored_age_files() {
    let identity = age::x25519::Identity::generate();
    let ciphertext = age::encrypt_and_armor(&identity.to_public(), b"armored state").unwrap();
    let identities = IdentitySet {
        identities: vec![Box::new(identity)],
    };
    assert_eq!(
        identities.decrypt(ciphertext.as_bytes()).unwrap(),
        b"armored state"
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

#[test]
fn ssh_ed25519_and_rsa_recipients_use_age_validation() {
    let ed25519 =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uObz5dvMgjz1OxfM/XXUB+VHtZ6isGN";
    let rsa = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDE7nIXTGNuaRBN9toI/wNALuQec8mvlt0iJ7o3OaD2UvoKHJ7S8rmIn4FiQDUed/Vac3OhUibei1k+TBmm16u2Rj3klgWZOIDgi8d4vXKI5N3YBhxr3jsQ+kz1c+iZ4z/tTtz306+4K46XViVMWwyyg9j82Jn41mOAy9vdeDIfQ5fLeaGqn5KwlT61GNkZ+ozWK/ZNlQIlNCcoXxhJULIs9XrtczWyVBAea1nlDo0WHODePxoJjmsNHrpQXn5mf9O83xs10qfTUjnRUt48jRmedFy4tcra3QGmSTQ3KZne+wXXSb0cIpXLGvZjQSPHgG1hc4r3uBpiSzvesGLv79XL";
    assert!(parse_recipient(ed25519).is_ok());
    assert!(parse_recipient(rsa).is_ok());
}

#[test]
fn encrypted_store_has_no_files_without_recipients() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        PersistenceStore::open(directory.path(), &[])
            .unwrap()
            .is_none()
    );
    assert!(!directory.path().join("persistence").exists());
}

#[test]
fn github_entries_are_validated_without_logging_or_networking_in_the_parser() {
    for username in ["", "-zetta", "zetta-", "zetta--user", "zetta/user"] {
        assert!(validate_github_username(username).is_err(), "{username:?}");
    }
    assert!(validate_github_username("zetta-user").is_ok());

    let body = b"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEinvalid\n";
    assert!(parse_github_keys(body).is_err());
    let unsupported = b"ssh-dss AAAAB3NzaC1kc3MAAACBfake\n";
    assert!(parse_github_keys(unsupported).is_err());
}

#[test]
fn recipient_resolution_preserves_temporary_and_permanent_failures() {
    let temporary = resolve_recipient_strings_with(&["github:zetta-user".to_owned()], |_| {
        Err(RecipientResolutionError::Temporary(anyhow::anyhow!(
            "DNS lookup failed"
        )))
    })
    .unwrap_err();
    assert!(temporary.is_temporary());
    assert!(temporary.to_string().contains("DNS lookup failed"));

    let permanent = resolve_recipient_strings_with(&["github:zetta-user".to_owned()], |_| {
        Err(RecipientResolutionError::Permanent(anyhow::anyhow!(
            "malformed GitHub SSH key"
        )))
    })
    .unwrap_err();
    assert!(!permanent.is_temporary());
    assert!(permanent.to_string().contains("malformed GitHub SSH key"));
}

#[test]
fn invalid_direct_recipients_are_rejected_before_a_github_lookup() {
    let error = resolve_recipient_strings_with(
        &[
            "github:zetta-user".to_owned(),
            "not-an-age-recipient".to_owned(),
        ],
        |_| panic!("a permanent local configuration error must not fetch GitHub"),
    )
    .unwrap_err();
    assert!(!error.is_temporary());
    assert!(error.to_string().contains("invalid age recipient"));
}

#[test]
fn github_retryable_statuses_are_distinguished_from_configuration_responses() {
    for status in [
        reqwest::StatusCode::REQUEST_TIMEOUT,
        reqwest::StatusCode::TOO_EARLY,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        reqwest::StatusCode::BAD_GATEWAY,
    ] {
        assert!(is_retryable_github_status(status), "{status}");
    }
    for status in [
        reqwest::StatusCode::BAD_REQUEST,
        reqwest::StatusCode::UNAUTHORIZED,
        reqwest::StatusCode::NOT_FOUND,
    ] {
        assert!(!is_retryable_github_status(status), "{status}");
    }
}

#[test]
fn disk_segments_are_encrypted_and_manifest_sizes_are_updated() {
    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let mut store = PersistenceStore::open(directory.path(), &[recipient])
        .unwrap()
        .unwrap();
    store
        .save_session(&PersistedSession {
            id: 7,
            created_at: 10,
            updated_at: 11,
            summary: BackgroundSessionSummary {
                id: 7,
                title: "secret title".to_owned(),
                authentication_required: false,
                active_pane: 1,
                layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
                held: false,
                scoped_to: None,
                key_envelope: None,
            },
            state: serde_json::json!({"cwd": "/secret"}),
            verifier: None,
            key_envelope: None,
            failed_authentications: 0,
            backoff_seconds: 0,
            snapshots: vec![PersistedSnapshot {
                pane_id: 2,
                bytes: b"private screen".to_vec(),
            }],
        })
        .unwrap();
    store
        .append_scrollback(7, 1, b"private scrollback")
        .unwrap();
    store.flush_segments().unwrap();
    assert!(!store.records()[0].restorable);

    drop(store);
    let recovered = PersistenceStore::open_with_recovery(directory.path(), None)
        .unwrap()
        .unwrap();
    assert!(recovered.records()[0].restorable);
    assert!(
        PersistenceStore::open_with_recovery(directory.path(), Some(&[]))
            .unwrap()
            .is_none()
    );

    let manifest = fs::read_to_string(directory.path().join("persistence/manifest.json")).unwrap();
    assert!(!manifest.contains("secret title"));
    assert!(manifest.contains(r#""scrollback_bytes": 18"#));
    let metadata = age::decrypt(
        &identity,
        &fs::read(directory.path().join("persistence/session-7.age")).unwrap(),
    )
    .unwrap();
    let metadata: serde_json::Value = serde_json::from_slice(&metadata).unwrap();
    assert!(metadata["snapshots"][0].get("bytes").is_none());
    assert_eq!(metadata["snapshots"][0]["length"], 14);
    let segment = fs::read_dir(directory.path().join("persistence"))
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("pane-1-segment")
        })
        .unwrap();
    let ciphertext = fs::read(segment).unwrap();
    assert!(
        !ciphertext
            .windows(b"private scrollback".len())
            .any(|window| { window == b"private scrollback" })
    );
    let identities = IdentitySet {
        identities: vec![Box::new(identity)],
    };
    assert_eq!(
        recovered.read_scrollback(7, &identities).unwrap(),
        b"private scrollback"
    );
    assert_eq!(
        recovered.load_session(7, &identities).unwrap().snapshots[0].bytes,
        b"private screen"
    );
}

#[test]
fn reopening_an_existing_store_recovers_its_private_recipient_options() {
    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let store = PersistenceStore::open(directory.path(), &[recipient])
        .unwrap()
        .unwrap();
    drop(store);
    let mut reopened = PersistenceStore::open(directory.path(), &[])
        .unwrap()
        .unwrap();
    reopened
        .save_session(&PersistedSession {
            id: 8,
            created_at: 1,
            updated_at: 2,
            summary: BackgroundSessionSummary {
                id: 8,
                title: "reopened".to_owned(),
                authentication_required: false,
                active_pane: 1,
                layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
                held: false,
                scoped_to: None,
                key_envelope: None,
            },
            state: serde_json::Value::Null,
            verifier: None,
            key_envelope: None,
            failed_authentications: 0,
            backoff_seconds: 0,
            snapshots: Vec::new(),
        })
        .unwrap();
    let identities = IdentitySet {
        identities: vec![Box::new(identity)],
    };
    assert_eq!(reopened.load_session(8, &identities).unwrap().id, 8);
}

#[test]
fn cleanup_keeps_the_fixed_record_bound() {
    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let mut store = PersistenceStore::open(directory.path(), &[recipient])
        .unwrap()
        .unwrap();
    let now = unix_now();
    for id in 1..=MAX_RECORDS as u64 + 3 {
        store
            .save_session(&PersistedSession {
                id,
                created_at: now,
                updated_at: now,
                summary: BackgroundSessionSummary {
                    id,
                    title: format!("session {id}"),
                    authentication_required: false,
                    active_pane: 1,
                    layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                    panes: Vec::new(),
                    held: false,
                    scoped_to: None,
                    key_envelope: None,
                },
                state: serde_json::Value::Null,
                verifier: None,
                key_envelope: None,
                failed_authentications: 0,
                backoff_seconds: 0,
                snapshots: Vec::new(),
            })
            .unwrap();
    }
    drop(store);
    let mut store = PersistenceStore::open_with_recovery(directory.path(), None)
        .unwrap()
        .unwrap();
    store.prune(&HashSet::new()).unwrap();
    assert!(store.records().len() <= MAX_RECORDS);
}

#[test]
fn identity_file_loader_accepts_native_age_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("identity.txt");
    let identity = age::x25519::Identity::generate();
    let encoded = identity.to_string();
    std::fs::write(&path, format!("# comment\n{}\n", encoded.expose_secret())).unwrap();
    let identities = IdentitySet::from_paths(std::slice::from_ref(&path)).unwrap();
    let recipients = RecipientSet::parse(&[identity.to_public().to_string()]).unwrap();
    let ciphertext = recipients.encrypt(b"identity file").unwrap();
    assert_eq!(identities.decrypt(&ciphertext).unwrap(), b"identity file");
}

#[test]
fn identity_file_loader_accepts_pq_age_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pq-identity.txt");
    let identity = MlKem768X25519Identity::generate();
    std::fs::write(&path, format!("{identity}\n")).unwrap();
    let identities = IdentitySet::from_paths(std::slice::from_ref(&path)).unwrap();
    let recipients = RecipientSet::parse(&[identity.to_recipient().to_string()]).unwrap();
    let ciphertext = recipients.encrypt(b"pq identity file").unwrap();
    assert_eq!(
        identities.decrypt(&ciphertext).unwrap(),
        b"pq identity file"
    );
}

#[test]
fn identity_file_loader_accepts_passphrase_protected_ssh_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("identity");
    std::fs::write(
        &path,
        "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jYmMAAAAGYmNyeXB0AAAAGAAAABC0OgNmiw\n\
QW/kJ8kCmmTA2TAAAAEAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uOb\n\
z5dvMgjz1OxfM/XXUB+VHtZ6isGNAAAAkPhBKsZoNmaeuWYJQxOl+ofEmue/sFJnW+4IOt\n\
oTrS/orMBJ4b/phQcv/ejWYJ4RYYVhSLiI6hf0KwNGefxI90E8iG/yDOKcrxb34tqDEYrY\n\
FARDaJVRd9QtWLEqoP7pgdBR2BTP7aK1y6Mx3eFDgiQI9f/0Sjxd8V0apOPXv4i4kuQ1Nt\n\
LF7kNlDznn/nyZlg==\n\
-----END OPENSSH PRIVATE KEY-----\n",
    )
    .unwrap();
    assert!(identity_path_requires_passphrase(&path).unwrap());

    let identities = IdentitySet::from_paths_with_passphrases(
        std::slice::from_ref(&path),
        &[Some(SessionSecret::new("passphrase".to_owned()))],
    )
    .unwrap();
    let recipients = RecipientSet::parse(&[
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uObz5dvMgjz1OxfM/XXUB+VHtZ6isGN"
            .to_owned(),
    ])
    .unwrap();
    let ciphertext = recipients
        .encrypt(b"passphrase-protected identity")
        .unwrap();
    assert_eq!(
        identities.decrypt(&ciphertext).unwrap(),
        b"passphrase-protected identity"
    );
}
