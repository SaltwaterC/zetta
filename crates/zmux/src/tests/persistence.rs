use super::*;
use crate::protocol::BackgroundPaneLayout;

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
                columns: None,
                lines: None,
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

#[cfg(feature = "scrollback-buffer")]
#[test]
fn dimensioned_disk_snapshots_replay_each_encrypted_pane_scrollback() {
    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let mut store = PersistenceStore::open(directory.path(), &[recipient])
        .unwrap()
        .unwrap();
    store
        .save_session(&PersistedSession {
            id: 17,
            created_at: 1,
            updated_at: 2,
            summary: BackgroundSessionSummary {
                id: 17,
                title: "dimensioned".to_owned(),
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
            snapshots: vec![
                PersistedSnapshot {
                    pane_id: 1,
                    bytes: b"saved one\r\n".to_vec(),
                    columns: Some(10),
                    lines: Some(3),
                },
                PersistedSnapshot {
                    pane_id: 2,
                    bytes: b"saved two\r\n".to_vec(),
                    columns: Some(20),
                    lines: Some(3),
                },
            ],
        })
        .unwrap();
    store.append_scrollback(17, 1, b"0123456789X").unwrap();
    store.append_scrollback(17, 2, b"pane-two-only").unwrap();
    store.flush_segments().unwrap();
    store.update_authentication(17, 3, 1, 2).unwrap();

    let identities = IdentitySet {
        identities: vec![Box::new(identity)],
    };
    let restored = store.load_session(17, &identities).unwrap();
    assert_eq!(restored.updated_at, 3);
    assert_eq!(restored.failed_authentications, 1);
    assert_eq!(restored.backoff_seconds, 2);
    assert_eq!(restored.snapshots[0].columns, Some(10));
    assert_eq!(restored.snapshots[0].lines, Some(3));
    let first = String::from_utf8_lossy(&restored.snapshots[0].bytes);
    let second = String::from_utf8_lossy(&restored.snapshots[1].bytes);
    assert!(
        first.contains("0123456789"),
        "first pane lost its scrollback: {first:?}"
    );
    assert!(
        first.contains('X'),
        "the narrow pane did not wrap its final cell: {first:?}"
    );
    assert!(
        !first.contains("pane-two-only"),
        "pane scrollback crossed panes: {first:?}"
    );
    assert!(
        second.contains("pane-two-only"),
        "second pane lost its scrollback: {second:?}"
    );
    assert!(
        !second.contains("0123456789"),
        "pane scrollback crossed panes: {second:?}"
    );
}

#[cfg(feature = "scrollback-buffer")]
#[test]
fn disk_restore_keeps_scrollback_when_replayed_into_a_fresh_terminal() {
    use alacritty_terminal::{
        event::VoidListener,
        grid::Dimensions,
        term::{Config, Term},
        vte::ansi::{Processor, StdSyncHandler},
    };

    struct Size;

    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            5
        }

        fn screen_lines(&self) -> usize {
            5
        }

        fn columns(&self) -> usize {
            40
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let mut store = PersistenceStore::open(directory.path(), &[recipient])
        .unwrap()
        .unwrap();
    let mut saved = Vec::new();
    for line in 0..30 {
        saved.extend_from_slice(format!("line {line}\r\n").as_bytes());
    }
    store
        .save_session(&PersistedSession {
            id: 18,
            created_at: 1,
            updated_at: 2,
            summary: BackgroundSessionSummary {
                id: 18,
                title: "scrollback".to_owned(),
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
            snapshots: vec![PersistedSnapshot {
                pane_id: 1,
                bytes: saved,
                columns: Some(40),
                lines: Some(5),
            }],
        })
        .unwrap();
    let mut later_output = Vec::new();
    for line in 30..60 {
        later_output.extend_from_slice(format!("line {line}\r\n").as_bytes());
    }
    store.append_scrollback(18, 1, &later_output).unwrap();
    store.flush_segments().unwrap();

    let identities = IdentitySet {
        identities: vec![Box::new(identity)],
    };
    let restored = store.load_session(18, &identities).unwrap();
    let replay = &restored.snapshots[0].bytes;
    let mut term = Term::new(Config::default(), &Size, VoidListener);
    Processor::<StdSyncHandler>::new().advance(&mut term, replay);

    assert!(
        term.history_size() > 0,
        "disk restore produced only one viewport of output"
    );
    let replay = String::from_utf8_lossy(replay);
    assert!(
        replay.contains("line 0"),
        "old scrollback was lost: {replay:?}"
    );
    assert!(
        replay.contains("line 59"),
        "latest persisted output was lost: {replay:?}"
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
