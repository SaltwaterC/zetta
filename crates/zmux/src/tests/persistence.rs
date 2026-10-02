use super::*;
use crate::protocol::BackgroundPaneLayout;

#[test]
fn scrollback_rotation_keeps_sequences_and_existing_segments() {
    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let mut store = PersistenceStore::open(directory.path(), &[identity.to_public().to_string()])
        .unwrap()
        .unwrap();
    let full = vec![b'x'; SEGMENT_BYTES];
    store.append_scrollback(7, 1, &full).unwrap();
    assert!(store.segments.is_empty(), "size threshold must rotate");

    store.append_scrollback(7, 1, b"timed").unwrap();
    store.segments.get_mut(&(7, 1)).unwrap().started_at = unix_now() - SEGMENT_INTERVAL.as_secs();
    store.append_scrollback(7, 1, b" segment").unwrap();
    assert!(store.segments.is_empty(), "time threshold must rotate");

    store.append_scrollback(7, 1, b"explicit flush").unwrap();
    store.flush_segments().unwrap();
    store.append_scrollback(7, 1, b"buffered").unwrap();
    store.append_scrollback(7, 1, b" tail").unwrap();
    assert_eq!(store.segments[&(7, 1)].sequence, 4);
    assert_eq!(store.segments[&(7, 1)].bytes, b"buffered tail");
    let paths = scrollback_paths(&store.directory, 7).unwrap();
    assert_eq!(paths.len(), 3, "small fresh appends must stay buffered");
    for (index, expected) in [full.as_slice(), b"timed segment", b"explicit flush"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            scrollback_path_parts(&paths[index], 7),
            Some((1, index as u64 + 1))
        );
        assert_eq!(
            age::decrypt(&identity, &fs::read(&paths[index]).unwrap()).unwrap(),
            *expected
        );
    }
}

#[test]
fn scrollback_recovery_reconciles_orphans_and_each_panes_highest_sequence() {
    for handoff in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let identity = age::x25519::Identity::generate();
        let mut store =
            PersistenceStore::open(directory.path(), &[identity.to_public().to_string()])
                .unwrap()
                .unwrap();
        store.append_scrollback(7, 1, b"first").unwrap();
        store.flush_segments().unwrap();
        // No manifest record: these also model publication followed by a crash
        // before the manifest could be written. Recovery must still reserve them.
        for (session, pane, sequence) in [(7, 1, 10), (7, 1, 3), (7, 2, 20), (8, 1, 30)] {
            let path = store.directory.join(format!(
                "session-{session}-pane-{pane}-segment-{sequence}.age"
            ));
            fs::write(path, store.recipients.encrypt(b"orphan").unwrap()).unwrap();
        }
        for name in [
            "session-7-pane-1-segment-999.tmp-123",
            "session-7-pane-1-segment-invalid.age",
            "session-7-bytes-segment-999.age",
            "unrelated.age",
        ] {
            fs::write(store.directory.join(name), []).unwrap();
        }
        drop(store);
        let mut store = PersistenceStore::open_with_recovery_state(directory.path(), None, handoff)
            .unwrap()
            .unwrap();
        for (session, pane, expected) in [(7, 1, 11), (7, 2, 21), (8, 1, 31), (8, 2, 1)] {
            store.append_scrollback(session, pane, b"next").unwrap();
            assert_eq!(store.segments[&(session, pane)].sequence, expected);
        }
        store.flush_segments().unwrap();
        let paths = scrollback_paths(&store.directory, 7).unwrap();
        assert_eq!(paths.len(), 6);
        assert_eq!(
            age::decrypt(&identity, &fs::read(&paths[0]).unwrap()).unwrap(),
            b"first"
        );
    }
}

#[test]
fn scrollback_publication_skips_collisions_without_overwriting() {
    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let mut store = PersistenceStore::open(directory.path(), &[identity.to_public().to_string()])
        .unwrap()
        .unwrap();
    store.append_scrollback(7, 1, b"new").unwrap();
    // Arrive after the sequence is reserved, including a second collision.
    for sequence in [1, 2] {
        fs::write(
            store
                .directory
                .join(format!("session-7-pane-1-segment-{sequence}.age")),
            store.recipients.encrypt(b"existing").unwrap(),
        )
        .unwrap();
    }
    store.flush_segments().unwrap();
    store.append_scrollback(7, 1, b"next").unwrap();
    store.flush_segments().unwrap();
    let paths = scrollback_paths(&store.directory, 7).unwrap();
    assert_eq!(paths.len(), 4);
    for (path, expected) in paths
        .iter()
        .zip([b"existing".as_slice(), b"existing", b"new", b"next"])
    {
        assert_eq!(
            age::decrypt(&identity, &fs::read(path).unwrap()).unwrap(),
            expected
        );
    }
    assert!(
        fs::read_dir(&store.directory).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp")
        }),
        "publication must clean up temporary files"
    );
}

#[test]
fn scrollback_publication_reserves_its_sequence_even_if_the_manifest_write_fails() {
    let directory = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    let mut store = PersistenceStore::open(directory.path(), &[identity.to_public().to_string()])
        .unwrap()
        .unwrap();
    store
        .append_scrollback(7, 1, b"published before error")
        .unwrap();
    fs::remove_file(&store.manifest_path).unwrap();
    fs::create_dir(&store.manifest_path).unwrap();
    assert!(store.flush_segments().is_err());
    fs::remove_dir(&store.manifest_path).unwrap();

    store
        .append_scrollback(7, 1, b"published after error")
        .unwrap();
    store.flush_segments().unwrap();
    let paths = scrollback_paths(&store.directory, 7).unwrap();
    assert_eq!(paths.len(), 2);
    for (path, expected) in paths.iter().zip([
        b"published before error".as_slice(),
        b"published after error",
    ]) {
        assert_eq!(
            age::decrypt(&identity, &fs::read(path).unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn scrollback_sequence_exhaustion_never_reuses_the_last_file() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = store_with_one_live_record(directory.path());
    store.next_sequences.insert((3, 1), Some(u64::MAX));
    store.append_scrollback(3, 1, b"last").unwrap();
    store.append_scrollback(3, 1, b" segment").unwrap();
    store.flush_segments().unwrap();
    let path = store
        .directory
        .join(format!("session-3-pane-1-segment-{}.age", u64::MAX));
    let original = fs::read(&path).unwrap();
    assert!(store.append_scrollback(3, 1, b"overflow").is_err());
    drop(store);
    let mut recovered = PersistenceStore::open_with_recovery(directory.path(), None)
        .unwrap()
        .unwrap();
    assert!(
        recovered
            .append_scrollback(3, 1, b"overflow after restart")
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn scrollback_pruning_and_forgetting_release_only_the_removed_sessions() {
    for forget in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = store_with_one_live_record(directory.path());
        store.append_scrollback(3, 1, b"removed").unwrap();
        store.append_scrollback(4, 1, b"retained").unwrap();
        store.flush_segments().unwrap();
        store.append_scrollback(3, 1, b"pending removal").unwrap();
        store.append_scrollback(4, 1, b"pending retention").unwrap();
        if forget {
            store.forget(3).unwrap();
        } else {
            store.manifest.records[0].restorable = true;
            store.manifest.records[0].updated_at = 0;
            store.prune(&HashSet::from([3])).unwrap();
            assert!(
                store.segments.contains_key(&(3, 1)),
                "live sessions must survive pruning"
            );
            store.prune(&HashSet::new()).unwrap();
        }
        assert!(!store.segments.contains_key(&(3, 1)));
        assert!(!store.next_sequences.contains_key(&(3, 1)));
        assert!(scrollback_paths(&store.directory, 3).unwrap().is_empty());
        assert_eq!(store.segments[&(4, 1)].sequence, 2);
        store.flush_segments().unwrap();
        store.append_scrollback(4, 1, b"still increasing").unwrap();
        assert_eq!(store.segments[&(4, 1)].sequence, 3);
        store.append_scrollback(3, 1, b"new session").unwrap();
        store.flush_segments().unwrap();
        assert_eq!(scrollback_paths(&store.directory, 3).unwrap().len(), 1);
        assert_eq!(scrollback_paths(&store.directory, 4).unwrap().len(), 3);
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
            shared_state: None,
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
            shared_state: None,
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
            shared_state: None,
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
            shared_state: None,
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

/// A store holding one record the daemon still owns, so restorability has
/// something to be wrong about.
fn store_with_one_live_record(directory: &Path) -> PersistenceStore {
    let recipient = age::x25519::Identity::generate().to_public().to_string();
    let mut store = PersistenceStore::open(directory, &[recipient])
        .unwrap()
        .unwrap();
    store
        .save_session(&PersistedSession {
            id: 3,
            created_at: 1,
            updated_at: 2,
            summary: BackgroundSessionSummary {
                id: 3,
                title: "held by this daemon".to_owned(),
                authentication_required: false,
                active_pane: 1,
                layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
                held: false,
                scoped_to: None,
                key_envelope: None,
            },
            state: serde_json::Value::Null,
            shared_state: None,
            verifier: None,
            key_envelope: None,
            failed_authentications: 0,
            backoff_seconds: 0,
            snapshots: Vec::new(),
        })
        .unwrap();
    assert!(!store.records()[0].restorable);
    store
}

fn manifest_path(directory: &Path) -> PathBuf {
    directory.join("persistence").join("manifest.json")
}

fn read_manifest(directory: &Path) -> Manifest {
    serde_json::from_slice(&fs::read(manifest_path(directory)).unwrap()).unwrap()
}

/// Backdates the recorded stamp to one no boot of this machine can produce.
fn record_foreign_boot_stamp(directory: &Path) {
    let mut manifest = read_manifest(directory);
    manifest.boot_stamp = "00000000-0000-0000-0000-000000000000".to_owned();
    fs::write(
        manifest_path(directory),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn an_upgrade_handoff_keeps_records_live_whatever_stamp_was_recorded() {
    let directory = tempfile::tempdir().unwrap();
    drop(store_with_one_live_record(directory.path()));
    record_foreign_boot_stamp(directory.path());

    let reopened = PersistenceStore::open_with_recovery_state(directory.path(), None, true)
        .unwrap()
        .unwrap();
    assert_eq!(reopened.records().len(), 1);
    assert!(
        !reopened.records()[0].restorable,
        "a handoff must not hand the record to a client, whatever the stamp says"
    );
}

#[test]
fn a_fresh_daemon_start_recovers_records_whatever_stamp_was_recorded() {
    // Once with a stamp from another boot, once with this boot's own, which is
    // what the store just wrote.
    let foreign = tempfile::tempdir().unwrap();
    drop(store_with_one_live_record(foreign.path()));
    record_foreign_boot_stamp(foreign.path());
    let matching = tempfile::tempdir().unwrap();
    drop(store_with_one_live_record(matching.path()));
    assert_eq!(read_manifest(matching.path()).boot_stamp, boot_stamp());

    for directory in [foreign.path(), matching.path()] {
        let reopened = PersistenceStore::open_with_recovery_state(directory, None, false)
            .unwrap()
            .unwrap();
        assert_eq!(reopened.records().len(), 1);
        assert!(
            reopened.records()[0].restorable,
            "a start that is not a handoff recovers, whatever the stamp says"
        );
    }
}

#[test]
fn opening_refreshes_the_recorded_boot_stamp_even_when_nothing_was_recovered() {
    let directory = tempfile::tempdir().unwrap();
    drop(store_with_one_live_record(directory.path()));
    record_foreign_boot_stamp(directory.path());

    // `replacing_daemon` is the open that recovers nothing. A rolled-back
    // image reading a stale stamp would decide a reboot happened during a
    // handoff and prune the records of running sessions.
    let reopened = PersistenceStore::open_with_recovery_state(directory.path(), None, true)
        .unwrap()
        .unwrap();
    assert!(!reopened.records()[0].restorable);
    drop(reopened);
    assert_eq!(read_manifest(directory.path()).boot_stamp, boot_stamp());
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
                shared_state: None,
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
