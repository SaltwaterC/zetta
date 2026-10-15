use super::*;
use crate::auth::SessionSecret;

#[test]
fn identity_set_decrypts_armored_age_files() {
    let identity = age::x25519::Identity::generate();
    let ciphertext = age::encrypt_and_armor(&identity.to_public(), b"armored state").unwrap();
    let identities = IdentitySet {
        identities: vec![Box::new(identity)],
    };
    assert_eq!(
        *identities.decrypt(ciphertext.as_bytes()).unwrap(),
        b"armored state"
    );
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
    assert_eq!(*identities.decrypt(&ciphertext).unwrap(), b"identity file");
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
        *identities.decrypt(&ciphertext).unwrap(),
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
        *identities.decrypt(&ciphertext).unwrap(),
        b"passphrase-protected identity"
    );
}

#[test]
fn an_identity_file_is_read_into_erased_memory_without_reallocating() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("identity.txt");
    let identity = age::x25519::Identity::generate();
    // Longer than one read, so a buffer sized by what has arrived would grow.
    let comments = "# a comment that makes the file span several reads\n".repeat(400);
    fs::write(
        &path,
        format!("{comments}{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let bytes: Zeroizing<Vec<u8>> = read_identity_file(&path).unwrap();
    assert_eq!(bytes.len() as u64, fs::metadata(&path).unwrap().len());
    assert!(bytes.len() > 8192);
    assert_eq!(bytes.capacity(), bytes.len());
    IdentitySet::from_paths(&[path]).unwrap();
}

#[test]
fn an_oversized_identity_file_is_refused_before_it_is_read() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("identity.txt");
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_IDENTITY_FILE_BYTES as u64 + 1).unwrap();
    let error = read_identity_file(&path).unwrap_err().to_string();
    assert!(error.contains("too large"), "{error}");
}
