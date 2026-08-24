//! Interoperability with the reference `age` implementation.
//!
//! The encrypted store is an age v1 file format, not a Zetta one, and both
//! recipient kinds it writes are upstream: X25519, and the ML-KEM-768/X25519
//! hybrid that `age` v1.3.0 and later generate with `age-keygen -pq` and
//! decrypt natively. The post-quantum arm is the one worth pinning, because it
//! is a Rust reimplementation of a Go recipient: a round trip through our own
//! crate would keep passing if the stanza drifted, while the reference
//! implementation would stop being able to read it.
//!
//! Skipped, rather than failed, where the `age` binary is missing — it is not a
//! build dependency of Zetta.

#![cfg(all(unix, feature = "session-persistence"))]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use zmux::{
    persistence::{IdentitySet, PersistedSession, PersistedSnapshot, PersistenceStore},
    protocol::{BackgroundPaneLayout, BackgroundSessionSummary},
};

/// The reference implementation, if this machine has it.
fn age_binary(name: &str) -> Option<PathBuf> {
    let path = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name}"))
        .output()
        .ok()?;
    if !path.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&path.stdout).trim());
    path.is_file().then_some(path)
}

/// `age`'s own version, so the post-quantum arm can be skipped on a build that
/// predates it rather than reported as an interoperability failure.
fn age_supports_post_quantum(age: &Path) -> bool {
    let Ok(output) = Command::new(age).arg("--version").output() else {
        return false;
    };
    let reported = String::from_utf8_lossy(&output.stdout);
    let version = reported.trim().trim_start_matches('v');
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or_default());
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    (major, minor) >= (1, 3)
}

/// Generates a key pair with `age-keygen`, returning the identity file and the
/// recipient it printed into that file.
fn generate_identity(keygen: &Path, directory: &Path, post_quantum: bool) -> (PathBuf, String) {
    let path = directory.join(if post_quantum {
        "pq.txt"
    } else {
        "classical.txt"
    });
    let mut command = Command::new(keygen);
    if post_quantum {
        command.arg("-pq");
    }
    let generated = command
        .arg("-o")
        .arg(&path)
        .output()
        .expect("running age-keygen");
    assert!(
        generated.status.success(),
        "age-keygen failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let contents = fs::read_to_string(&path).expect("reading the generated identity");
    let recipient = contents
        .lines()
        .find_map(|line| line.strip_prefix("# public key: "))
        .expect("age-keygen must record the recipient in the file")
        .to_owned();
    (path, recipient)
}

const SCROLLBACK: &[u8] = b"interoperability scrollback\n";
const SCREEN: &[u8] = b"interoperability screen";

fn store_session(directory: &Path, recipient: &str) {
    let mut store = PersistenceStore::open(directory, std::slice::from_ref(&recipient.to_owned()))
        .expect("opening the encrypted store")
        .expect("recipients were configured, so a store must exist");
    store
        .save_session(&PersistedSession {
            id: 3,
            created_at: 1,
            updated_at: 2,
            summary: BackgroundSessionSummary {
                id: 3,
                title: "interop".to_owned(),
                authentication_required: false,
                active_pane: 1,
                layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
                held: false,
                scoped_to: None,
            },
            state: serde_json::json!({"cwd": "/interop"}),
            verifier: None,
            failed_authentications: 0,
            backoff_seconds: 0,
            snapshots: vec![PersistedSnapshot {
                pane_id: 1,
                bytes: SCREEN.to_vec(),
            }],
        })
        .expect("saving the session record");
    store
        .append_scrollback(3, 1, SCROLLBACK)
        .expect("appending scrollback");
    store.flush_segments().expect("finalizing the segment");
}

fn segment_path(directory: &Path) -> PathBuf {
    fs::read_dir(directory.join("persistence"))
        .expect("reading the store")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().contains("pane-1-segment"))
        })
        .expect("a finalized scrollback segment")
}

fn age_decrypt(age: &Path, identity: &Path, file: &Path) -> Vec<u8> {
    let decrypted = Command::new(age)
        .arg("--decrypt")
        .arg("-i")
        .arg(identity)
        .arg(file)
        .output()
        .expect("running age --decrypt");
    assert!(
        decrypted.status.success(),
        "the reference implementation could not read {}: {}",
        file.display(),
        String::from_utf8_lossy(&decrypted.stderr)
    );
    decrypted.stdout
}

/// The stanza tag is asserted separately from the decryption, so a rename shows
/// up as itself rather than as an unexplained decryption failure.
fn assert_stanza_tag(file: &Path, expected: &str) {
    let header = fs::read(file).expect("reading the encrypted file");
    let header = String::from_utf8_lossy(&header[..header.len().min(256)]).into_owned();
    assert!(
        header.starts_with("age-encryption.org/v1\n"),
        "not an age v1 file: {header:?}"
    );
    assert!(
        header.contains(&format!("-> {expected}")),
        "expected a {expected} stanza, got {header:?}"
    );
}

#[test]
fn the_reference_implementation_reads_a_classical_store() {
    let (Some(age), Some(keygen)) = (age_binary("age"), age_binary("age-keygen")) else {
        eprintln!("skipping: no age binary on PATH");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (identity, recipient) = generate_identity(&keygen, directory.path(), false);
    store_session(directory.path(), &recipient);

    let segment = segment_path(directory.path());
    assert_stanza_tag(&segment, "X25519");
    assert_eq!(age_decrypt(&age, &identity, &segment), SCROLLBACK);
    // The metadata record too: the same envelope carries the title, layout and
    // working directory that a locked store must not disclose.
    let metadata = age_decrypt(
        &age,
        &identity,
        &directory.path().join("persistence/session-3.age"),
    );
    let metadata: serde_json::Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(metadata["summary"]["title"], "interop");
}

#[test]
fn the_reference_implementation_reads_a_post_quantum_store() {
    let (Some(age), Some(keygen)) = (age_binary("age"), age_binary("age-keygen")) else {
        eprintln!("skipping: no age binary on PATH");
        return;
    };
    if !age_supports_post_quantum(&age) {
        eprintln!("skipping: this age predates built-in post-quantum recipients");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let (identity, recipient) = generate_identity(&keygen, directory.path(), true);
    assert!(
        recipient.starts_with("age1pq1"),
        "age-keygen -pq must produce a post-quantum recipient, got {recipient}"
    );
    store_session(directory.path(), &recipient);

    let segment = segment_path(directory.path());
    assert_stanza_tag(&segment, "mlkem768x25519");
    assert_eq!(age_decrypt(&age, &identity, &segment), SCROLLBACK);
}

/// The other direction: a file the reference implementation encrypted, read
/// with the identity loader Zetta resumes a store with.
#[test]
fn a_reference_encrypted_file_is_readable_with_our_identities() {
    let (Some(age), Some(keygen)) = (age_binary("age"), age_binary("age-keygen")) else {
        eprintln!("skipping: no age binary on PATH");
        return;
    };
    let post_quantum = age_supports_post_quantum(&age);
    let directory = tempfile::tempdir().unwrap();
    let (identity, recipient) = generate_identity(&keygen, directory.path(), post_quantum);

    let plaintext = directory.path().join("plain");
    fs::write(&plaintext, SCROLLBACK).unwrap();
    let encrypted = Command::new(&age)
        .arg("--encrypt")
        .arg("-r")
        .arg(&recipient)
        .arg("-o")
        .arg(directory.path().join("cipher.age"))
        .arg(&plaintext)
        .output()
        .expect("running age --encrypt");
    assert!(
        encrypted.status.success(),
        "age --encrypt failed: {}",
        String::from_utf8_lossy(&encrypted.stderr)
    );

    let identities = IdentitySet::from_paths(&[identity]).expect("loading the generated identity");
    let ciphertext = fs::read(directory.path().join("cipher.age")).unwrap();
    assert_eq!(
        identities
            .decrypt(&ciphertext)
            .expect("decrypting a file the reference implementation wrote"),
        SCROLLBACK
    );
}
