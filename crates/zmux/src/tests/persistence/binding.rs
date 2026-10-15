use super::*;

fn every_kind() -> [Binding; 4] {
    [
        Binding::metadata(7, 0x0123_4567_89ab_cdef_0011_2233_4455_6677),
        Binding::authentication(7, [0xa5; 32]),
        Binding::snapshots(7, 3, u128::MAX),
        Binding::scrollback(7, 2, 9),
    ]
}

fn bound(binding: &Binding, payload: &[u8]) -> Vec<u8> {
    let mut plaintext = binding.header();
    plaintext.extend_from_slice(payload);
    plaintext
}

#[test]
fn every_kind_reads_back_as_written_with_its_payload_intact() {
    for binding in every_kind() {
        let plaintext = bound(&binding, b"{\"id\":7}\n");
        let (found, payload) = split(&plaintext).unwrap();
        assert_eq!(found, Some(binding));
        assert_eq!(payload, b"{\"id\":7}\n");
        assert!(binding.header().len() < MAX_HEADER_BYTES);
    }
}

#[test]
fn payloads_from_before_bindings_are_passed_through_unbound() {
    for payload in [
        b"{\"id\":7}".as_slice(),
        b"\x1b[2Jprompt$ ",
        b"",
        // A prefix of the magic that ends before it does is unbound output.
        b"\0zmux",
    ] {
        assert_eq!(split(payload).unwrap(), (None, payload));
    }
}

#[test]
fn a_malformed_header_is_refused_rather_than_read_as_output() {
    let header = Binding::scrollback(7, 2, 9).header();
    let cut_off = &header[..header.len() - 1];
    let mut overlong = MAGIC.to_vec();
    overlong.extend(std::iter::repeat_n(b'a', MAX_HEADER_BYTES));
    for plaintext in [
        cut_off.to_vec(),
        overlong,
        [MAGIC, b"kind=spare record=7\n"].concat(),
        [MAGIC, b"kind=metadata record=7\n"].concat(),
        [MAGIC, b"kind=metadata record=7 generation=00\n"].concat(),
        [MAGIC, b"kind=scrollback record=7 sequence=9 pane=2\n"].concat(),
        [
            MAGIC,
            b"kind=scrollback record=7 pane=2 sequence=9 generation=0\n",
        ]
        .concat(),
        [MAGIC, b"kind=authentication record=7 metadata=zz\n"].concat(),
        [
            MAGIC,
            b"kind=metadata record=-1 generation=00000000000000000000000000000000\n",
        ]
        .concat(),
    ] {
        assert!(split(&plaintext).is_err(), "{plaintext:?} was accepted");
    }
}

#[test]
fn a_binding_matches_only_its_own_slot() {
    let path = Path::new("slot.age");
    let found = Binding::scrollback(7, 2, 9);
    found.check(&Binding::scrollback(7, 2, 9), path).unwrap();
    for expected in [
        Binding::scrollback(8, 2, 9),
        Binding::scrollback(7, 3, 9),
        Binding::scrollback(7, 2, 10),
        Binding::snapshots(7, 9, 1),
    ] {
        let error = found.check(&expected, path).unwrap_err().to_string();
        assert!(error.contains("copied from another file"), "{error}");
    }

    let snapshots = Binding::snapshots(7, 3, 1);
    snapshots.check(&Binding::snapshots(7, 3, 1), path).unwrap();
    let error = snapshots
        .check(&Binding::snapshots(7, 3, 2), path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("different write"), "{error}");

    Binding::metadata(7, 5)
        .check(&Binding::metadata_slot(7), path)
        .unwrap();
    assert!(
        Binding::metadata(8, 5)
            .check(&Binding::metadata_slot(7), path)
            .is_err()
    );
    Binding::authentication(7, [1; 32])
        .check(&Binding::authentication_slot(7), path)
        .unwrap();
}

/// Feeds `plaintext` in chunks of `size`, as age's reader hands it over.
fn stream(plaintext: &[u8], expected: Binding, size: usize) -> Result<Vec<u8>> {
    let path = Path::new("segment.age");
    let mut output = Vec::new();
    let mut check = StreamCheck::new(expected, path);
    for chunk in plaintext.chunks(size) {
        check.feed(chunk, |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        })?;
    }
    check.finish(|bytes| {
        output.extend_from_slice(bytes);
        Ok(())
    })?;
    Ok(output)
}

#[test]
fn a_streamed_header_is_checked_however_it_is_split() {
    let binding = Binding::scrollback(7, 2, 9);
    let plaintext = bound(&binding, b"output after the header");
    for size in [1, 2, 7, 64, 4096] {
        assert_eq!(
            stream(&plaintext, binding, size).unwrap(),
            b"output after the header"
        );
        assert!(stream(&plaintext, Binding::scrollback(7, 2, 10), size).is_err());
        assert!(stream(&plaintext[..binding.header().len() - 1], binding, size).is_err());
    }
    // A header with nothing after it is an empty segment.
    assert_eq!(stream(&binding.header(), binding, 3).unwrap(), b"");
}

#[test]
fn a_streamed_payload_from_before_bindings_passes_through_whole() {
    let binding = Binding::scrollback(7, 2, 9);
    for payload in [b"\0zm".as_slice(), b"plain output", b""] {
        for size in [1, 4, 4096] {
            assert_eq!(stream(payload, binding, size).unwrap(), payload);
        }
    }
}

/// A store sealed to one identity, and that identity to read it back with.
struct Store {
    _directory: tempfile::TempDir,
    store: PersistenceStore,
    identity: age::x25519::Identity,
}

impl Store {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let identity = age::x25519::Identity::generate();
        let mut store =
            PersistenceStore::open(directory.path(), &[identity.to_public().to_string()])
                .unwrap()
                .unwrap();
        for id in [7, 8] {
            store.seal(id, &Seal::Store).unwrap();
        }
        Self {
            _directory: directory,
            store,
            identity,
        }
    }

    /// Saves record `id` with two panes whose screens are restored with
    /// their scrollback, and one flushed scrollback segment for each pane.
    fn save(&mut self, id: u64) {
        self.store.save_session(&session(id)).unwrap();
        for pane in [1, 2] {
            self.store
                .append_scrollback(id, pane, format!("record {id} pane {pane}\r\n").as_bytes())
                .unwrap();
        }
        self.store.flush_segments().unwrap();
    }

    fn path(&self, name: &str) -> PathBuf {
        self.store.directory.join(name)
    }

    fn copy(&self, from: &str, to: &str) {
        fs::copy(self.path(from), self.path(to)).unwrap();
    }

    /// Replaces `name` with `payload` encrypted to the store without a
    /// binding, as a build from before bindings wrote it.
    fn write_unbound(&self, name: &str, payload: &[u8]) {
        fs::write(
            self.path(name),
            self.store.recipients.encrypt(payload).unwrap(),
        )
        .unwrap();
    }

    fn decrypt(&self, name: &str) -> Vec<u8> {
        age::decrypt(&self.identity, &fs::read(self.path(name)).unwrap()).unwrap()
    }

    fn load(&self, id: u64) -> Result<PersistedSession> {
        let identities = IdentitySet {
            identities: vec![Box::new(self.identity.clone())],
        };
        self.store.load_session(id, &identities)
    }

    fn refused(&self, id: u64) -> String {
        format!("{:#}", self.load(id).unwrap_err())
    }
}

fn session(id: u64) -> PersistedSession {
    PersistedSession {
        id,
        created_at: 1,
        updated_at: 2,
        summary: crate::protocol::BackgroundSessionSummary {
            id,
            title: format!("record {id}"),
            authentication_required: false,
            active_pane: 1,
            layout: crate::protocol::BackgroundPaneLayout::Pane { pane_id: 1 },
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
        snapshots: [1, 2]
            .into_iter()
            .map(|pane_id| PersistedSnapshot {
                pane_id,
                bytes: format!("screen {id}.{pane_id}\r\n").into_bytes(),
                columns: Some(40),
                lines: Some(4),
            })
            .collect(),
    }
}

fn restored_text(session: &PersistedSession, pane: u64) -> String {
    let snapshot = session
        .snapshots
        .iter()
        .find(|snapshot| snapshot.pane_id == pane)
        .unwrap();
    String::from_utf8_lossy(&snapshot.bytes).into_owned()
}

#[test]
fn every_file_a_record_is_written_as_names_its_own_slot() {
    let mut store = Store::new();
    store.save(7);
    store.store.update_authentication(7, 3, 1, 2).unwrap();
    for (name, expected) in [
        ("session-7.age", PayloadKind::Metadata),
        ("session-7-auth.age", PayloadKind::Authentication),
        ("session-7-bytes-segment-1.age", PayloadKind::Snapshots),
        ("session-7-pane-2-segment-1.age", PayloadKind::Scrollback),
    ] {
        let plaintext = store.decrypt(name);
        let (binding, _) = split(&plaintext).unwrap();
        assert_eq!(
            binding.map(|binding| binding.kind),
            Some(expected),
            "{name}"
        );
    }
    let restored = store.load(7).unwrap();
    assert_eq!(restored.id, 7);
    assert_eq!(restored.failed_authentications, 1);
    assert!(restored_text(&restored, 1).contains("screen 7.1"));
    assert!(restored_text(&restored, 2).contains("record 7 pane 2"));
}

#[test]
fn metadata_of_another_kind_in_a_records_slot_is_refused() {
    let mut store = Store::new();
    store.save(7);
    store.store.update_authentication(7, 3, 1, 2).unwrap();
    store.copy("session-7-auth.age", "session-7.age");
    let error = store.refused(7);
    assert!(error.contains("copied from another file"), "{error}");
}

#[test]
fn metadata_from_another_record_is_refused() {
    let mut store = Store::new();
    store.save(7);
    store.save(8);
    store.copy("session-8.age", "session-7.age");
    let error = store.refused(7);
    assert!(error.contains("copied from another file"), "{error}");
}

#[test]
fn unbound_metadata_from_another_record_is_refused_by_the_record_it_names() {
    let mut store = Store::new();
    store.save(8);
    let plaintext = store.decrypt("session-8.age");
    let (_, metadata) = split(&plaintext).unwrap();
    store.write_unbound("session-7.age", metadata);
    let error = store.refused(7);
    assert!(
        error.contains("the metadata of record 8, not of record 7"),
        "{error}"
    );
}

#[test]
fn a_scrollback_segment_from_another_pane_is_refused() {
    let mut store = Store::new();
    store.save(7);
    store.copy(
        "session-7-pane-1-segment-1.age",
        "session-7-pane-2-segment-1.age",
    );
    let error = store.refused(7);
    assert!(error.contains("copied from another file"), "{error}");
}

#[test]
fn a_scrollback_segment_under_another_sequence_is_refused() {
    let mut store = Store::new();
    store.save(7);
    store.copy(
        "session-7-pane-1-segment-1.age",
        "session-7-pane-1-segment-2.age",
    );
    let error = store.refused(7);
    assert!(error.contains("copied from another file"), "{error}");
}

#[test]
fn a_scrollback_segment_from_another_record_is_refused() {
    let mut store = Store::new();
    store.save(7);
    store.save(8);
    store.copy(
        "session-8-pane-1-segment-1.age",
        "session-7-pane-1-segment-1.age",
    );
    let error = store.refused(7);
    assert!(error.contains("copied from another file"), "{error}");
}

#[test]
fn a_snapshot_segment_in_a_scrollback_slot_is_refused() {
    let mut store = Store::new();
    store.save(7);
    store.copy(
        "session-7-bytes-segment-1.age",
        "session-7-pane-1-segment-2.age",
    );
    let error = store.refused(7);
    assert!(error.contains("copied from another file"), "{error}");
}

#[test]
fn a_scrollback_segment_in_a_snapshot_slot_is_refused() {
    let mut store = Store::new();
    store.save(7);
    store.copy(
        "session-7-pane-1-segment-1.age",
        "session-7-bytes-segment-1.age",
    );
    let error = store.refused(7);
    assert!(error.contains("copied from another file"), "{error}");
}

#[test]
fn a_snapshot_segment_from_an_earlier_write_of_the_record_is_refused() {
    let mut store = Store::new();
    store.save(7);
    let earlier = fs::read(store.path("session-7-bytes-segment-1.age")).unwrap();
    // With no segment left, the next write numbers its segment 1 again, so the
    // two differ only in which write they belong to.
    fs::remove_file(store.path("session-7-bytes-segment-1.age")).unwrap();
    store.save(7);
    store.load(7).unwrap();
    fs::write(store.path("session-7-bytes-segment-1.age"), earlier).unwrap();
    let error = store.refused(7);
    assert!(error.contains("different write"), "{error}");
}

#[test]
fn an_authentication_sidecar_from_before_the_current_metadata_is_ignored() {
    let mut store = Store::new();
    store.save(7);
    store.store.update_authentication(7, 3, 5, 16).unwrap();
    let stale = fs::read(store.path("session-7-auth.age")).unwrap();
    // A complete write carries the counters itself and removes the sidecar.
    store.save(7);
    assert!(!store.path("session-7-auth.age").exists());
    fs::write(store.path("session-7-auth.age"), stale).unwrap();
    let restored = store.load(7).unwrap();
    assert_eq!(restored.failed_authentications, 0);
    assert_eq!(restored.backoff_seconds, 0);
}

#[test]
fn an_authentication_sidecar_from_another_record_is_refused() {
    let mut store = Store::new();
    store.save(7);
    store.save(8);
    store.store.update_authentication(8, 3, 5, 16).unwrap();
    store.copy("session-8-auth.age", "session-7-auth.age");
    let error = store.refused(7);
    assert!(error.contains("copied from another file"), "{error}");
}

#[test]
fn bound_metadata_does_not_take_unbound_companions_written_with_it() {
    let mut store = Store::new();
    store.save(7);
    let plaintext = store.decrypt("session-7-bytes-segment-1.age");
    store.write_unbound(
        "session-7-bytes-segment-1.age",
        split(&plaintext).unwrap().1,
    );
    let error = store.refused(7);
    assert!(error.contains("were not written together"), "{error}");

    let mut store = Store::new();
    store.save(7);
    let counters = serde_json::to_vec(&PersistedAuthentication {
        updated_at: 3,
        failed_authentications: 5,
        backoff_seconds: 16,
    })
    .unwrap();
    store.write_unbound("session-7-auth.age", &counters);
    assert_eq!(store.load(7).unwrap().failed_authentications, 0);
}

/// Writes record 7 as a build without bindings did: the same files, with
/// nothing in front of their payloads.
fn write_unbound_record(store: &Store) {
    let metadata = PersistedSessionMetadata {
        id: 7,
        created_at: 1,
        updated_at: 2,
        summary: session(7).summary,
        state: serde_json::Value::Null,
        shared_state: None,
        verifier: None,
        key_envelope: None,
        failed_authentications: 0,
        backoff_seconds: 0,
        snapshots: vec![SnapshotLocation {
            pane_id: 1,
            segment: 1,
            offset: 0,
            length: 15,
            columns: Some(40),
            lines: Some(4),
        }],
    };
    store.write_unbound("session-7.age", &serde_json::to_vec(&metadata).unwrap());
    store.write_unbound("session-7-bytes-segment-1.age", b"old screen 7.1\n");
    store.write_unbound(
        "session-7-auth.age",
        &serde_json::to_vec(&PersistedAuthentication {
            updated_at: 3,
            failed_authentications: 4,
            backoff_seconds: 8,
        })
        .unwrap(),
    );
    store.write_unbound("session-7-pane-1-segment-1.age", b"old scrollback\r\n");
}

#[test]
fn a_record_written_before_bindings_still_loads() {
    let store = Store::new();
    write_unbound_record(&store);
    let restored = store.load(7).unwrap();
    assert_eq!(restored.id, 7);
    assert_eq!(restored.failed_authentications, 4);
    assert_eq!(restored.backoff_seconds, 8);
    let text = restored_text(&restored, 1);
    assert!(text.contains("old screen 7.1"), "{text}");
    assert!(text.contains("old scrollback"), "{text}");
}

#[test]
fn a_record_written_before_bindings_is_bound_when_it_is_next_saved() {
    let mut store = Store::new();
    write_unbound_record(&store);
    let mut restored = store.load(7).unwrap();
    restored.failed_authentications = 0;
    store.store.save_session(&restored).unwrap();
    for name in ["session-7.age", "session-7-bytes-segment-2.age"] {
        assert!(split(&store.decrypt(name)).unwrap().0.is_some(), "{name}");
    }
    // Scrollback written before the rewrite stays as it was, and still loads.
    let text = restored_text(&store.load(7).unwrap(), 1);
    assert!(text.contains("old scrollback"), "{text}");
}
