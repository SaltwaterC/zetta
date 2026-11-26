//! Session retention and the encrypted records kept on disk: what each
//! retention mode writes, how records are sealed and listed, and resuming one
//! after its daemon is gone.

use super::*;

#[cfg(feature = "session-persistence")]
#[test]
fn disk_retention_without_recipients_writes_no_persistence_files() {
    let daemon = TestDaemon::start_with(&["--retention", "disk"]);
    assert!(!daemon.sessions_dir().join("persistence").exists());
}

#[cfg(feature = "session-persistence")]
#[test]
fn a_new_client_applies_disk_retention_to_an_existing_daemon() {
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start();
    let client = Client::connect_at_with_retention_and_persistence(
        &daemon.sessions_dir(),
        Retention::Disk,
        zmux::persistence::PersistenceOptions {
            recipients: vec![identity.to_public().to_string()],
            identity: None,
        },
    )
    .unwrap();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"configured_after_startup": true}),
            None,
            vec![(pane.pane_id, b"encrypted after reconfiguration".to_vec())],
        )
        .unwrap();

    let persistence = daemon.sessions_dir().join("persistence");
    assert!(persistence.join("manifest.json").is_file());
    assert!(
        persistence
            .join(format!("session-{}.age", pane.session_id))
            .is_file()
    );
    client.kill(pane.session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn disk_recovery_after_memory_fallback_reenables_orphaned_records() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let daemon = TestDaemon::start();
    let session_id = 91;

    // A previous disk daemon would have marked its live record unavailable.
    // Leave that record behind while the replacement daemon is still using
    // the memory fallback, which is the state reached when recipient
    // resolution temporarily cannot contact GitHub.
    {
        let mut store = PersistenceStore::open(&daemon.sessions_dir(), &[recipient])
            .unwrap()
            .unwrap();
        seal_fabricated_records(&mut store);
        store
            .save_session(&PersistedSession {
                id: session_id,
                created_at: 1,
                updated_at: 2,
                summary: summary(session_id, 1),
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

    let client = daemon.client();
    client
        .configure_with_retention_and_persistence(
            Retention::Disk,
            zmux::persistence::PersistenceOptions {
                recipients: vec![identity.to_public().to_string()],
                identity: None,
            },
        )
        .unwrap();

    let (_, records) = client.list_with_restorable().unwrap();
    assert!(
        records
            .iter()
            .any(|record| record.id == session_id && record.restorable)
    );
    client.forget(session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn an_existing_memory_daemon_can_resume_a_disk_record() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let daemon = TestDaemon::start();
    let session_id = 92;

    // Create the record after the memory daemon is already running. This
    // specifically exercises the recovery-only handle used when an older or
    // fallback daemon has no persistence store of its own.
    {
        let mut store = PersistenceStore::open(&daemon.sessions_dir(), &[recipient])
            .unwrap()
            .unwrap();
        seal_fabricated_records(&mut store);
        store
            .save_session(&PersistedSession {
                id: session_id,
                created_at: 1,
                updated_at: 2,
                summary: summary(session_id, 1),
                state: serde_json::json!({"from": "disk"}),
                shared_state: None,
                verifier: None,
                key_envelope: None,
                failed_authentications: 0,
                backoff_seconds: 0,
                snapshots: vec![PersistedSnapshot {
                    pane_id: 1,
                    bytes: b"saved before the daemon restarted\r\n".to_vec(),
                    columns: Some(80),
                    lines: Some(24),
                }],
            })
            .unwrap();
    }

    let identity_path = daemon.config.join("identity.txt");
    std::fs::write(
        &identity_path,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let client = daemon.client();
    let restored = client
        .resume(session_id, std::slice::from_ref(&identity_path))
        .unwrap();
    assert_eq!(restored.state, serde_json::json!({"from": "disk"}));
    assert!(
        String::from_utf8_lossy(&restored.snapshots[0].bytes)
            .contains("saved before the daemon restarted"),
        "the saved screen was not reconstructed: {:?}",
        restored.snapshots[0].bytes
    );

    let pane = client
        .spawn(spawn_request(
            Some(session_id),
            "printf 'fresh shell'; sleep 60",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "fresh shell");
    drop(descriptor);

    match client.attach(session_id, Some(pane.pane_id), None).unwrap() {
        AttachOutcome::Attached { pane, .. } => {
            assert!(
                String::from_utf8_lossy(&pane.replay).contains("saved before the daemon restarted"),
                "the memory fallback lost the saved pane output"
            );
            drop(pane);
        }
        _ => panic!("expected the restored pane to remain PTY-backed"),
    }
    client.kill(session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn starting_in_memory_mode_keeps_old_disk_records_visible() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().to_path_buf();
    let sessions = daemon_sessions_dir(&config);
    std::fs::create_dir_all(&sessions).unwrap();
    let session_id = 95;
    {
        let mut store = PersistenceStore::open(&sessions, &[recipient])
            .unwrap()
            .unwrap();
        seal_fabricated_records(&mut store);
        store
            .save_session(&PersistedSession {
                id: session_id,
                created_at: 1,
                updated_at: 2,
                summary: summary(session_id, 1),
                state: serde_json::json!({"survived": true}),
                shared_state: None,
                verifier: None,
                key_envelope: None,
                failed_authentications: 0,
                backoff_seconds: 0,
                snapshots: Vec::new(),
            })
            .unwrap();
        store
            .save_session(&PersistedSession {
                id: 97,
                created_at: 1,
                updated_at: 2,
                summary: summary(97, 1),
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

    let process = Command::new(daemon_binary())
        .args(["--daemon", "--retention", "memory"])
        .env("XDG_CONFIG_HOME", &config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(config.join("zmux.log")).unwrap())
        .spawn()
        .unwrap();
    let daemon = TestDaemon {
        process,
        _directory: directory,
        config,
    };
    daemon.wait_for_endpoint();
    let client = daemon.client();

    // This is the configuration request Zetta sends after connecting. It
    // must not discard the recovery-only store opened during daemon startup.
    client
        .configure(Retention::Memory { bytes: 4096 }, Vec::new())
        .unwrap();
    let (_, records) = client.list_with_restorable().unwrap();
    assert!(
        records
            .iter()
            .any(|record| record.id == session_id && record.restorable),
        "the memory startup hid the old disk record"
    );
    client.kill(97).unwrap();
    assert!(
        client
            .list_with_restorable()
            .unwrap()
            .1
            .iter()
            .all(|record| record.id != 97),
        "killing a stale disk record left it in the daemon catalog"
    );

    let identity_path = daemon.config.join("identity.txt");
    std::fs::write(
        &identity_path,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let restored = client
        .resume(session_id, std::slice::from_ref(&identity_path))
        .unwrap();
    assert_eq!(restored.state, serde_json::json!({"survived": true}));
    client.forget(session_id).unwrap();

    // The recovery-only store must not turn the active memory policy back
    // into disk persistence for sessions created after startup.
    let pane = client.spawn(spawn_request(None, "sleep 60")).unwrap();
    drop(std::fs::File::from(pane.descriptor));
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    assert!(
        zmux::persistence::read_opaque_records(&daemon.sessions_dir())
            .unwrap()
            .iter()
            .all(|record| record.id != pane.session_id),
        "memory-mode sessions were written to the recovery-only store"
    );
    client.kill(pane.session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn cli_kill_and_forget_remove_orphaned_disk_records_with_a_live_memory_daemon() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let daemon = TestDaemon::start();

    for session_id in [93, 94] {
        let mut store =
            PersistenceStore::open(&daemon.sessions_dir(), std::slice::from_ref(&recipient))
                .unwrap()
                .unwrap();
        seal_fabricated_records(&mut store);
        store
            .save_session(&PersistedSession {
                id: session_id,
                created_at: 1,
                updated_at: 2,
                summary: summary(session_id, 1),
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

    let run_cli = |command: &str, session_id: u64| {
        Command::new(daemon_binary())
            .arg(command)
            .arg(session_id.to_string())
            .env("XDG_CONFIG_HOME", &daemon.config)
            .output()
            .unwrap()
    };
    let killed = run_cli("kill", 93);
    assert!(killed.status.success(), "kill stderr: {:?}", killed.stderr);
    assert!(
        String::from_utf8_lossy(&killed.stdout).contains("Forgot stale disk session 93"),
        "kill stdout: {:?}",
        killed.stdout
    );
    let forgotten = run_cli("forget", 94);
    assert!(
        forgotten.status.success(),
        "forget stderr: {:?}",
        forgotten.stderr
    );
    assert!(
        zmux::persistence::read_opaque_records(&daemon.sessions_dir())
            .unwrap()
            .iter()
            .all(|record| record.id != 93 && record.id != 94),
        "orphaned records remain after CLI cleanup"
    );
}

#[cfg(feature = "session-persistence")]
#[test]
fn cli_kill_removes_an_orphaned_disk_record_without_a_daemon() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().to_path_buf();
    let sessions = daemon_sessions_dir(&config);
    std::fs::create_dir_all(&sessions).unwrap();
    let mut store = PersistenceStore::open(&sessions, &[recipient])
        .unwrap()
        .unwrap();
    seal_fabricated_records(&mut store);
    store
        .save_session(&PersistedSession {
            id: 96,
            created_at: 1,
            updated_at: 2,
            summary: summary(96, 1),
            state: serde_json::Value::Null,
            shared_state: None,
            verifier: None,
            key_envelope: None,
            failed_authentications: 0,
            backoff_seconds: 0,
            snapshots: Vec::new(),
        })
        .unwrap();
    drop(store);

    let output = Command::new(daemon_binary())
        .args(["kill", "96"])
        .env("XDG_CONFIG_HOME", &config)
        .output()
        .unwrap();
    assert!(output.status.success(), "kill stderr: {:?}", output.stderr);
    assert!(
        zmux::persistence::read_opaque_records(&sessions)
            .unwrap()
            .iter()
            .all(|record| record.id != 96),
        "the stale record survived kill without a daemon"
    );
}

/// A record whose contents are all secrets, so an opaque listing that leaked
/// any of them would say so.
#[cfg(feature = "session-persistence")]
fn secret_session(session_id: u64) -> PersistedSession {
    let mut summary = summary(session_id, 1);
    summary.title = "secret-title".to_owned();
    summary.panes = vec![zmux::protocol::BackgroundPaneSummary {
        id: 1,
        label: "secret-label".to_owned(),
        profile: "secret-profile".to_owned(),
        configured_command: "secret-command".to_owned(),
        application: "secret-application".to_owned(),
        foreground_command: Some(vec!["secret-foreground".to_owned()]),
        terminal_title: Some("secret-terminal-title".to_owned()),
        working_directory: Some(PathBuf::from("/secret-directory")),
        state: zmux::protocol::BackgroundPaneState::Running,
        exit: None,
    }];
    PersistedSession {
        id: session_id,
        created_at: 1,
        updated_at: 2,
        summary,
        state: serde_json::json!({"cwd": "/secret-directory"}),
        shared_state: None,
        verifier: None,
        key_envelope: None,
        failed_authentications: 0,
        backoff_seconds: 0,
        snapshots: Vec::new(),
    }
}

#[cfg(feature = "session-persistence")]
const SECRETS: &[&str] = &[
    "secret-title",
    "secret-label",
    "secret-profile",
    "secret-command",
    "secret-application",
    "secret-foreground",
    "secret-terminal-title",
    "secret-directory",
];

#[cfg(feature = "session-persistence")]
#[test]
fn an_opaque_listing_offers_a_record_whose_daemon_is_gone_and_reveals_nothing_of_it() {
    let recipient = age::x25519::Identity::generate().to_public().to_string();
    let directory = tempfile::tempdir().unwrap();
    let sessions = daemon_sessions_dir(directory.path());
    std::fs::create_dir_all(&sessions).unwrap();
    let mut store = PersistenceStore::open(&sessions, &[recipient])
        .unwrap()
        .unwrap();
    seal_fabricated_records(&mut store);
    store.save_session(&secret_session(41)).unwrap();
    drop(store);

    // The record was written as the live daemon's own; restorability has to be
    // the listing's decision, not something already true on disk.
    let manifest = std::fs::read_to_string(sessions.join("persistence/manifest.json")).unwrap();
    assert!(
        manifest.contains(r#""restorable": false"#),
        "manifest: {manifest}"
    );

    let records = zmux::persistence::read_opaque_records(&sessions).unwrap();
    let record = records
        .iter()
        .find(|record| record.id == 41)
        .expect("the record was not listed");
    assert!(
        record.restorable,
        "no daemon is answering, so the record must be offered: {record:?}"
    );
    let listed = format!("{records:?}\n{manifest}");
    for secret in SECRETS {
        assert!(
            !listed.contains(secret),
            "the opaque listing revealed {secret}: {listed}"
        );
    }
}

#[cfg(feature = "session-persistence")]
#[test]
fn an_opaque_listing_leaves_a_record_alone_while_its_daemon_answers() {
    let recipient = age::x25519::Identity::generate().to_public().to_string();
    let daemon = TestDaemon::start();
    // Proves the daemon is answering; without that this test passes for the
    // wrong reason.
    drop(daemon.client());
    let mut store = PersistenceStore::open(&daemon.sessions_dir(), &[recipient])
        .unwrap()
        .unwrap();
    seal_fabricated_records(&mut store);
    store.save_session(&secret_session(42)).unwrap();
    drop(store);

    let records = zmux::persistence::read_opaque_records(&daemon.sessions_dir()).unwrap();
    let record = records
        .iter()
        .find(|record| record.id == 42)
        .expect("the record was not listed");
    assert!(
        !record.restorable,
        "a record its daemon still holds must not be offered: {record:?}"
    );
}

#[cfg(feature = "session-persistence")]
#[test]
fn cli_listing_never_offers_a_disk_record_for_a_live_catalog_session() {
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    // Model the bad state left by the old Unix upgrade: the live catalog and
    // the cleartext manifest name the same session, but the manifest claims it
    // is recoverable. Listing must resolve that disagreement toward the live
    // process rather than offering a destructive duplicate restore.
    let manifest_path = daemon.sessions_dir().join("persistence/manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["records"][0]["restorable"] = serde_json::Value::Bool(true);
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let output = Command::new(daemon_binary())
        .args(["list", "--json"])
        .env("XDG_CONFIG_HOME", &daemon.config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed["catalogs"][0]["sessions"][0]["id"], pane.session_id);
    assert_eq!(listed["restorable"], serde_json::json!([]));

    client.kill(pane.session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn reconfiguring_to_disk_keeps_an_existing_process_and_creates_encrypted_persistence() {
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    let configuration = client
        .configure_with_retention_and_persistence_resilient(
            Retention::Disk,
            zmux::persistence::PersistenceOptions {
                recipients: vec![identity.to_public().to_string()],
                identity: None,
            },
            Retention::Memory { bytes: 4096 },
        )
        .unwrap();
    assert_eq!(configuration.requested_retention, Retention::Disk);
    assert_eq!(configuration.effective_retention, Retention::Disk);
    assert!(process_is_alive(pane.child_pid));

    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"configured_after_startup": true}),
            None,
            vec![(pane.pane_id, b"encrypted after reconfiguration".to_vec())],
        )
        .unwrap();

    let persistence = daemon.sessions_dir().join("persistence");
    assert!(persistence.join("manifest.json").is_file());
    assert!(
        persistence
            .join(format!("session-{}.age", pane.session_id))
            .is_file()
    );
    client.kill(pane.session_id).unwrap();
}

/// Records these tests fabricate through the store directly stand for ones a
/// daemon wrote for an unprotected session, so they are sealed the same way.
#[cfg(feature = "session-persistence")]
fn seal_fabricated_records(store: &mut PersistenceStore) {
    for id in 0..=128 {
        store.seal(id, &zmux::persistence::Seal::Store).unwrap();
    }
}

/// Whether `session_id`'s metadata record opens with `identity`.
#[cfg(feature = "session-persistence")]
fn record_opens_with(
    daemon: &TestDaemon,
    session_id: u64,
    identity: &age::x25519::Identity,
) -> bool {
    let path = daemon
        .sessions_dir()
        .join("persistence")
        .join(format!("session-{session_id}.age"));
    age::decrypt(identity, &std::fs::read(path).unwrap()).is_ok()
}

/// Decrypts a file the persistence store wrote and returns what follows the
/// line naming its slot, which every such file starts its plaintext with.
#[cfg(feature = "session-persistence")]
fn store_payload(identity: &impl age::Identity, ciphertext: &[u8]) -> Vec<u8> {
    let plaintext = age::decrypt(identity, ciphertext).unwrap();
    assert!(
        plaintext.starts_with(b"\0zmux-binding/1 "),
        "a store file without its binding"
    );
    let end = plaintext.iter().position(|byte| *byte == b'\n').unwrap();
    plaintext[end + 1..].to_vec()
}

/// `Configure` replaces the store's recipients and any token holder may send
/// it. Re-encrypting a protected session under the sender's key would hand its
/// verifier, state and scrollback to whoever chose that key, without the secret.
#[cfg(feature = "session-persistence")]
#[test]
fn another_client_cannot_reseal_a_protected_session_to_its_own_recipient() {
    let owner = age::x25519::Identity::generate();
    let attacker = age::x25519::Identity::generate();
    let daemon = TestDaemon::start_with_recipient(&owner.to_public().to_string());
    let client = daemon.client();
    // A real process the session belongs to, so the kernel can tell it apart
    // from this one, which plays the other client.
    let mut window = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let pane = client
        .spawn(spawn_request_as(
            None,
            "printf ready; sleep 60",
            window.id(),
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"private": true}),
            Some(&test_verifier()),
            window.id(),
        )
        .unwrap();
    assert!(record_opens_with(&daemon, pane.session_id, &owner));

    client
        .configure(Retention::Disk, vec![attacker.to_public().to_string()])
        .unwrap();

    // Reconfiguring rewrote the kept session, under its own recipients.
    assert!(
        !record_opens_with(&daemon, pane.session_id, &attacker),
        "a protected session was re-encrypted to the reconfiguring client's key"
    );
    assert!(record_opens_with(&daemon, pane.session_id, &owner));
    window.kill().ok();
    window.wait().ok();
}

/// The session's own window rotating its recipients is the legitimate case:
/// configuring alone does not move a pinned session, because a reload can be
/// asked for by anything holding the control token; protecting it again does.
#[cfg(feature = "session-persistence")]
#[test]
fn a_protected_sessions_own_window_rotates_its_recipients_by_protecting_it_again() {
    let before = age::x25519::Identity::generate();
    let after = age::x25519::Identity::generate();
    let daemon = TestDaemon::start_with_recipient(&before.to_public().to_string());
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    let detach = |state: serde_json::Value| {
        client
            .detach(
                pane.session_id,
                summary(pane.session_id, pane.pane_id),
                state,
                Some(&test_verifier()),
                Vec::new(),
            )
            .unwrap();
    };
    detach(serde_json::json!({"private": 1}));
    assert!(record_opens_with(&daemon, pane.session_id, &before));

    client
        .configure(Retention::Disk, vec![after.to_public().to_string()])
        .unwrap();
    assert!(
        record_opens_with(&daemon, pane.session_id, &before),
        "configuring re-pinned an already protected session"
    );

    detach(serde_json::json!({"private": 2}));
    assert!(record_opens_with(&daemon, pane.session_id, &after));
    assert!(!record_opens_with(&daemon, pane.session_id, &before));
    client.kill(pane.session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn disk_detach_encrypts_metadata_and_keeps_it_opaque_to_listing() {
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let state = serde_json::json!({
        "secret_title": "metadata must stay encrypted",
        "cwd": "/private/worktree"
    });
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            state.clone(),
            None,
            vec![(pane.pane_id, b"private screen".to_vec())],
        )
        .unwrap();

    let persistence = daemon.sessions_dir().join("persistence");
    let manifest = std::fs::read_to_string(persistence.join("manifest.json")).unwrap();
    assert!(!manifest.contains("metadata must stay encrypted"));
    assert!(!manifest.contains("private/worktree"));
    assert!(
        serde_json::from_str::<serde_json::Value>(&manifest)
            .unwrap()
            .get("records")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|records| records.len() == 1),
        "manifest was {manifest}"
    );

    let ciphertext =
        std::fs::read(persistence.join(format!("session-{}.age", pane.session_id))).unwrap();
    assert!(
        !ciphertext
            .windows(b"metadata must stay encrypted".len())
            .any(|window| window == b"metadata must stay encrypted")
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&store_payload(&identity, &ciphertext)).unwrap();
    assert_eq!(metadata["state"], state);
    assert!(metadata["snapshots"][0].get("bytes").is_none());
    assert_eq!(metadata["snapshots"][0]["length"], 14);
    let snapshot_path = std::fs::read_dir(&persistence)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("bytes-segment")
        })
        .unwrap();
    assert_eq!(
        store_payload(&identity, &std::fs::read(snapshot_path).unwrap()),
        b"private screen"
    );

    assert!(client.list_with_restorable().unwrap().1.is_empty());
    client.kill(pane.session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn attaching_a_disk_session_removes_its_stale_persistence_record() {
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let pane = client.spawn(spawn_request(None, "sleep 60")).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();

    let record_path = daemon
        .sessions_dir()
        .join(format!("persistence/session-{}.age", pane.session_id));
    assert!(record_path.is_file());
    let attached = client.attach(pane.session_id, None, None).unwrap();
    match attached {
        AttachOutcome::Attached { pane, .. } => drop(pane),
        _ => panic!("expected an exclusive attach"),
    }
    assert!(!record_path.exists());
    client.kill(pane.session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn daemon_loss_makes_disk_records_restorable_and_resume_consumes_them() {
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"restored": true}),
            None,
            vec![(pane.pane_id, vec![b'x'; 96 * 1024])],
        )
        .unwrap();
    drop(client);

    let mut daemon = daemon;
    daemon.restart_with_recovery();
    let client = daemon.client();
    let (_, records) = client.list_with_restorable().unwrap();
    let record = records
        .iter()
        .find(|record| record.id == pane.session_id && record.restorable)
        .expect("daemon loss should make the record restorable");
    assert!(!record.protected);

    let identity_path = daemon.config.join("identity.txt");
    let encoded = identity.to_string();
    std::fs::write(&identity_path, format!("{}\n", encoded.expose_secret())).unwrap();
    let restored = client
        .resume(pane.session_id, std::slice::from_ref(&identity_path))
        .unwrap();
    assert_eq!(restored.state, serde_json::json!({"restored": true}));
    assert_eq!(restored.snapshots[0].columns, Some(80));
    assert_eq!(restored.snapshots[0].lines, Some(24));
    assert!(!restored.snapshots[0].bytes.is_empty());
    assert!(
        restored.snapshots[0].bytes.len() < 96 * 1024,
        "dimensioned restore should replace the raw snapshot with its bounded screen"
    );
    let (_, records) = client.list_with_restorable().unwrap();
    assert!(
        records
            .iter()
            .any(|record| record.id == pane.session_id && !record.restorable)
    );
    client.forget(pane.session_id).unwrap();
    assert!(client.list_with_restorable().unwrap().1.is_empty());
}

#[cfg(feature = "session-persistence")]
#[test]
fn live_share_checkpoints_the_screen_before_daemon_loss() {
    let identity = age::x25519::Identity::generate();
    let mut daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // This is the checkpoint Zetta sends while the pane is still exclusively
    // attached to the window. Sharing used to persist the daemon's deliberately
    // empty retained screen because only the window had been reading the PTY.
    client
        .send_snapshot(
            pane.session_id,
            pane.pane_id,
            b"screen from live share\r\n".to_vec(),
            80,
            24,
        )
        .expect("checkpointing the live pane");
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .expect("sharing the live pane");
    drop(descriptor);
    drop(client);
    daemon.restart_with_recovery();

    let client = daemon.client();
    let (_, records) = client.list_with_restorable().unwrap();
    assert!(
        records
            .iter()
            .any(|record| record.id == pane.session_id && record.restorable),
        "the live share did not leave a restorable disk record: {records:?}"
    );

    let identity_path = daemon.config.join("identity.txt");
    std::fs::write(
        &identity_path,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let restored = client
        .resume(pane.session_id, std::slice::from_ref(&identity_path))
        .expect("resuming the live share after daemon loss");
    assert!(
        String::from_utf8_lossy(&restored.snapshots[0].bytes).contains("screen from live share"),
        "the live share restored an empty screen: {:?}",
        restored.snapshots[0].bytes
    );
    client.forget(pane.session_id).unwrap();
}

#[cfg(feature = "session-persistence")]
#[test]
fn disk_resume_spawns_a_fresh_shell_in_the_original_session() {
    let identity = age::x25519::Identity::generate();
    let mut daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf old-process; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "old-process");
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"fresh_shell": true}),
            None,
            vec![(pane.pane_id, b"saved screen\r\n".to_vec())],
        )
        .unwrap();
    drop(client);
    daemon.restart_with_recovery();

    let identity_path = daemon.config.join("identity.txt");
    std::fs::write(
        &identity_path,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let client = daemon.client();
    client
        .resume(pane.session_id, std::slice::from_ref(&identity_path))
        .unwrap();

    let restored = client
        .spawn(spawn_request(
            Some(pane.session_id),
            "printf fresh-shell; read value; printf 'got:%s' \"$value\"; sleep 60",
        ))
        .unwrap();
    assert_eq!(restored.session_id, pane.session_id);
    let mut descriptor = std::fs::File::from(restored.descriptor);
    read_until(&descriptor, "fresh-shell");
    descriptor.write_all(b"from-user\n").unwrap();
    read_until(&descriptor, "got:from-user");
    drop(descriptor);

    // The daemon also seeds its normal retained screen, so a later handoff
    // sees the restored pane even though the first Zetta terminal consumed
    // the one-shot replay locally.
    match client
        .attach(pane.session_id, Some(restored.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => {
            assert!(
                String::from_utf8_lossy(&pane.replay).contains("saved screen"),
                "the restored daemon pane lost its saved screen"
            );
            drop(pane);
        }
        _ => panic!("unexpected attach result"),
    }

    client.kill(pane.session_id).unwrap();
    assert!(client.list_with_restorable().unwrap().1.is_empty());
}

#[cfg(feature = "session-persistence")]
#[test]
fn protected_disk_resume_preserves_failed_authentication_backoff() {
    let identity = age::x25519::Identity::generate();
    let mut daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    let verifier = zmux::auth::SessionAuthentication::create("correct secret").unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"protected": true}),
            Some(&verifier),
            Vec::new(),
        )
        .unwrap();
    drop(client);
    daemon.restart_with_recovery();

    let identity_path = daemon.config.join("identity.txt");
    let encoded = identity.to_string();
    std::fs::write(&identity_path, format!("{}\n", encoded.expose_secret())).unwrap();
    let client = daemon.client();
    let (_, records) = client.list_with_restorable().unwrap();
    let record = records
        .iter()
        .find(|record| record.id == pane.session_id)
        .expect("the protected record survives the daemon");
    assert!(record.protected);
    // The other half of what `auto_protected` means: a secret someone typed is
    // not something a key can be recovered for, so a caller must still ask.
    assert!(!record.auto_protected);

    let wrong = client.resume_with_secret(
        pane.session_id,
        std::slice::from_ref(&identity_path),
        Some(&zmux::auth::SessionSecret::new("wrong secret".to_owned())),
    );
    assert!(
        wrong
            .unwrap_err()
            .to_string()
            .contains("authentication failed")
    );

    let sessions_dir = daemon.sessions_dir();
    let authentication_path =
        sessions_dir.join(format!("persistence/session-{}-auth.age", pane.session_id));
    let read_authentication = || -> serde_json::Value {
        let ciphertext = std::fs::read(&authentication_path).unwrap();
        serde_json::from_slice(&store_payload(&identity, &ciphertext)).unwrap()
    };
    let update_backoff = |updated_at, backoff_seconds| {
        let metadata = read_authentication();
        let mut store = PersistenceStore::open_with_recovery(&sessions_dir, None)
            .unwrap()
            .unwrap();
        seal_fabricated_records(&mut store);
        store
            .update_authentication(
                pane.session_id,
                updated_at,
                metadata["failed_authentications"]
                    .as_u64()
                    .unwrap()
                    .try_into()
                    .unwrap(),
                backoff_seconds,
            )
            .unwrap();
    };
    let metadata = read_authentication();
    assert_eq!(metadata["failed_authentications"], 1);
    assert_eq!(metadata["backoff_seconds"], 1);

    // Expire the persisted refusal window explicitly, and wait out the
    // daemon's own: that one is the daemon's, so no request can expire it.
    // Both return AuthenticationFailed, so the failure count below is what
    // tells a checked wrong secret from a refused attempt.
    update_backoff(0, 1);
    std::thread::sleep(zmux::auth::failed_authentication_delay(1) + Duration::from_millis(100));
    let wrong = client.resume_with_secret(
        pane.session_id,
        std::slice::from_ref(&identity_path),
        Some(&zmux::auth::SessionSecret::new("wrong secret".to_owned())),
    );
    assert!(
        wrong
            .unwrap_err()
            .to_string()
            .contains("authentication failed")
    );

    let metadata = read_authentication();
    assert_eq!(metadata["failed_authentications"], 2);
    assert!(metadata["backoff_seconds"].as_u64().unwrap() >= 2);

    // Give the restart ample time without weakening the refusal assertion.
    // The daemon already wrote the real doubled delay, checked above.
    update_backoff(metadata["updated_at"].as_u64().unwrap(), 30);
    daemon.restart_with_recovery();
    let client = daemon.client();
    let immediate = client.resume_with_secret(
        pane.session_id,
        std::slice::from_ref(&identity_path),
        Some(&zmux::auth::SessionSecret::new("correct secret".to_owned())),
    );
    assert!(
        immediate
            .unwrap_err()
            .to_string()
            .contains("authentication failed")
    );
    assert_eq!(read_authentication()["failed_authentications"], 2);
    update_backoff(0, 2);
    client
        .resume_with_secret(
            pane.session_id,
            std::slice::from_ref(&identity_path),
            Some(&zmux::auth::SessionSecret::new("correct secret".to_owned())),
        )
        .unwrap();
    client.forget(pane.session_id).unwrap();
}

/// A disk record carries its own way in, so resuming it needs no secret — and
/// the manifest says so before anything is decrypted, which is what lets a
/// caller decide not to prompt.
#[cfg(feature = "session-persistence")]
#[test]
fn an_automatically_protected_disk_record_resumes_without_a_secret() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let recipients =
        zmux::persistence::RecipientSet::parse(std::slice::from_ref(&recipient)).unwrap();
    let mut daemon = TestDaemon::start_with_recipient(&recipient);
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let sealed = zmux::auto_protect::seal(&recipients).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"resumed": true}),
            Some(&sealed.authentication),
            Vec::new(),
        )
        .unwrap();
    drop(client);
    daemon.restart_with_recovery();

    let client = daemon.client();
    let record = client
        .list_with_restorable()
        .unwrap()
        .1
        .into_iter()
        .find(|record| record.id == pane.session_id)
        .expect("the record survives the daemon");
    assert!(record.protected);
    assert!(
        record.auto_protected,
        "the manifest has to say so before the record can be opened"
    );

    let identity_path = daemon.config.join("identity.txt");
    std::fs::write(
        &identity_path,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let restored = client
        .resume_with_secret(pane.session_id, std::slice::from_ref(&identity_path), None)
        .expect("the sealed key inside the record opens it");
    assert_eq!(restored.state, serde_json::json!({"resumed": true}));
}

/// Rotating a disk segment — encrypting it, publishing it and rewriting the
/// manifest — happens on the persistence worker, not under the lock every
/// other pane's input and output go through. A busy pane rotating several
/// segments leaves an unrelated shared pane answering, and what it rotated is
/// published whole and in the order it was read.
///
/// Deliberately not a latency bound: while a pane prints this fast, a shared
/// pane's echo waits on the drain parsing that output into the retained
/// screen, which costs far more than rotation did and is not this test's
/// subject. The rotation stall itself was measured separately; see
/// `docs/PERFORMANCE_PERSISTENCE_WORKER_2026-10-02.md`.
#[cfg(all(target_os = "linux", feature = "session-persistence"))]
#[test]
fn rotating_disk_segments_leaves_other_panes_live_and_keeps_output_in_order() {
    const LINES: u32 = 3_000_000;
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start();
    let client = Client::connect_at_with_retention_and_persistence(
        &daemon.sessions_dir(),
        Retention::Disk,
        zmux::persistence::PersistenceOptions {
            recipients: vec![identity.to_public().to_string()],
            identity: None,
        },
    )
    .unwrap();
    // Detached, so the daemon reads, retains and persists all of it.
    let created = client
        .create_shared(one_pane_shared_request(
            &client,
            &format!("seq 1 {LINES}; sleep 60"),
        ))
        .unwrap();
    let session_id = created.session_id;
    let echo = client
        .create_shared(one_pane_shared_request(&client, "exec cat"))
        .unwrap();
    let echo_pane_id = echo.state.operation_receipts[0].draft_mappings[0].pane_id;
    let AttachOutcome::SharedAttached {
        pane: echo_pane, ..
    } = client
        .attach_shared_with_secret(echo.session_id, echo_pane_id, None)
        .unwrap()
    else {
        panic!("a shared session's pane must attach shared")
    };
    let mut echo_reader = echo_pane.reader();
    let persistence = daemon.sessions_dir().join("persistence");
    let segments = || {
        let prefix = format!("session-{session_id}-pane-");
        let mut segments = std::fs::read_dir(&persistence)
            .unwrap()
            .filter_map(|entry| {
                let name = entry.unwrap().file_name().into_string().unwrap();
                let sequence = name
                    .strip_prefix(&prefix)?
                    .strip_suffix(".age")?
                    .split_once("-segment-")?
                    .1
                    .parse::<u64>()
                    .ok()?;
                Some((sequence, persistence.join(name)))
            })
            .collect::<Vec<_>>();
        segments.sort();
        segments
    };

    let deadline = Instant::now() + Duration::from_secs(120);
    let mut echoes = 0;
    while segments().len() < 2 {
        assert!(
            Instant::now() < deadline,
            "the busy pane never rotated two segments"
        );
        let marker = format!("echo-during-rotation-{echoes}");
        echo_pane
            .send_input(format!("{marker}\n").as_bytes())
            .unwrap();
        read_until_reader(&mut echo_reader, &marker);
        echoes += 1;
    }

    let published = segments()
        .into_iter()
        .take(2)
        .flat_map(|(_, path)| store_payload(&identity, &std::fs::read(path).unwrap()))
        .collect::<Vec<_>>();
    assert!(published.len() >= 2 * 8 * 1024 * 1024);
    let published = String::from_utf8(published).unwrap().replace("\r\n", "\n");
    let start = published
        .find("1\n2\n3\n")
        .expect("the published output must start with the program's first lines");
    let expected = (1..=LINES)
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    // The second segment ends wherever its read did: possibly mid-line, or
    // between the `\r` and `\n` the terminal turned a newline into.
    let published = published[start..].trim_end_matches('\r');
    assert!(
        expected.starts_with(published),
        "published output diverged from what the program printed after {} bytes",
        published
            .bytes()
            .zip(expected.bytes())
            .take_while(|(published, expected)| published == expected)
            .count()
    );
    drop(echo_reader);
    drop(echo_pane);
    client.kill(echo.session_id).unwrap();
    client.kill(session_id).unwrap();
}

/// Closing one pane of an ordinary tab must not write that tab to disk.
///
/// Nothing asked to keep it, and no window has published its tab state yet, so
/// the record would carry a null state. A daemon that then died with the tab
/// still open left it behind, and the next window was offered a "disk session"
/// it could not rebuild: "invalid type: null, expected struct TabState".
#[cfg(feature = "session-persistence")]
#[test]
fn closing_a_pane_of_a_tab_nobody_detached_writes_no_disk_record() {
    let identity = age::x25519::Identity::generate();
    let daemon = TestDaemon::start_with_recipient(&identity.to_public().to_string());
    let client = daemon.client();
    let first = client
        .spawn(spawn_request(None, "printf first-ready; sleep 60"))
        .unwrap();
    let second = client
        .spawn(spawn_request(
            Some(first.session_id),
            "printf second-ready; sleep 60",
        ))
        .unwrap();
    let first_descriptor = std::fs::File::from(first.descriptor);
    let second_descriptor = std::fs::File::from(second.descriptor);
    read_until(&first_descriptor, "first-ready");
    read_until(&second_descriptor, "second-ready");

    client
        .discard_pane(first.session_id, second.pane_id)
        .unwrap();

    let persistence = daemon.sessions_dir().join("persistence");
    assert!(
        !persistence
            .join(format!("session-{}.age", first.session_id))
            .exists(),
        "a private tab was written to disk when one of its panes closed"
    );
    let manifest: serde_json::Value = std::fs::read(persistence.join("manifest.json")).map_or_else(
        |_| serde_json::json!({ "records": [] }),
        |bytes| serde_json::from_slice(&bytes).unwrap(),
    );
    assert_eq!(manifest["records"], serde_json::json!([]), "{manifest}");

    client.kill(first.session_id).unwrap();
    drop(second_descriptor);
}
