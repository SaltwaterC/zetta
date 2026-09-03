//! End-to-end tests against a real daemon process.
//!
//! Each test gets its own configuration directory, so the daemon it starts is
//! its own and cannot find, or be found by, the developer's running sessions.

#![cfg(unix)]

use std::{
    collections::HashMap,
    io::{Read, Write as _},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[cfg(feature = "session-persistence")]
use age::secrecy::ExposeSecret as _;

#[cfg(feature = "session-persistence")]
use zmux::persistence::{PersistedSession, PersistedSnapshot, PersistenceStore};

use zmux::{
    client::{AttachOutcome, Client},
    messages::{SpawnRequest, TerminalSize},
    protocol::{BackgroundPaneLayout, BackgroundSessionSummary},
    retention::Retention,
};

/// A daemon with a private configuration directory, stopped when dropped.
struct TestDaemon {
    process: Child,
    _directory: tempfile::TempDir,
    config: PathBuf,
}

/// Blocks until a value arrives or the deadline passes.
///
/// The test threads are not main tasks, so blocking is fine; the helper only
/// exists because `async_channel` has no `recv_timeout` of its own.
fn recv_timeout<T>(receiver: &async_channel::Receiver<T>, timeout: Duration) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Ok(value) => return Some(value),
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => return None,
        }
    }
}

impl TestDaemon {
    fn start() -> Self {
        Self::start_with(&[])
    }

    fn start_with(extra: &[&str]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().to_path_buf();
        let process = Command::new(daemon_binary())
            .arg("--daemon")
            .args(extra)
            .env("XDG_CONFIG_HOME", &config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Into a file rather than inherited: a daemon that replaced itself
            // and then failed leaves its reason here, which is otherwise lost.
            .stderr(std::fs::File::create(config.join("zmux.log")).unwrap())
            .spawn()
            .expect("starting zmux");

        let daemon = Self {
            process,
            _directory: directory,
            config,
        };
        daemon.wait_for_endpoint();
        daemon
    }

    fn wait_for_endpoint(&self) {
        let _ = self.wait_for_ready();
    }

    fn process_id(&self) -> u32 {
        self.process.id()
    }

    fn sessions_dir(&self) -> PathBuf {
        daemon_sessions_dir(&self.config)
    }

    /// A client pointed at this daemon. The directory is passed explicitly so
    /// tests running in parallel do not fight over an environment variable.
    fn client(&self) -> Client {
        self.wait_for_ready()
    }

    fn wait_for_ready(&self) -> Client {
        let endpoint = self.sessions_dir().join("zmux.json");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last_error = None;
        loop {
            match Client::connect_ready_at(&self.sessions_dir()) {
                Ok(Some(client)) => return client,
                Ok(None) => {}
                Err(error) => last_error = Some(format!("{error:#}")),
            }
            if Instant::now() >= deadline {
                panic!(
                    "the daemon was not ready within 10s (process: {}; endpoint: {}; last error: {})\ndaemon log:\n{}",
                    self.process.id(),
                    endpoint.display(),
                    last_error.as_deref().unwrap_or("none"),
                    self.log()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(feature = "session-persistence")]
    fn start_with_recipient(recipient: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().to_path_buf();
        let sessions = daemon_sessions_dir(&config);
        std::fs::create_dir_all(&sessions).unwrap();
        let options_path = sessions.join("daemon-options-test.json");
        let options = zmux::persistence::DaemonOptionsFile {
            recipients: vec![recipient.to_owned()],
        };
        std::fs::write(&options_path, serde_json::to_vec(&options).unwrap()).unwrap();
        let process = Command::new(daemon_binary())
            .args([
                "--daemon",
                "--retention",
                "disk",
                &format!("--daemon-options={}", options_path.display()),
            ])
            .env("XDG_CONFIG_HOME", &config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(config.join("zmux.log")).unwrap())
            .spawn()
            .expect("starting disk-retention zmux");
        let daemon = Self {
            process,
            _directory: directory,
            config,
        };
        daemon.wait_for_endpoint();
        daemon
    }

    #[cfg(feature = "session-persistence")]
    fn restart_with_recovery(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        self.process = Command::new(daemon_binary())
            .args(["--daemon", "--retention", "disk"])
            .env("XDG_CONFIG_HOME", &self.config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.config.join("zmux.log"))
                    .unwrap(),
            )
            .spawn()
            .expect("restarting disk-retention zmux");
        self.wait_for_endpoint();
    }
}

fn daemon_sessions_dir(config: &Path) -> PathBuf {
    let name = if cfg!(debug_assertions) {
        format!("sessions-debug-v{}", zmux::messages::PROTOCOL_VERSION)
    } else {
        "sessions".to_owned()
    };
    config.join("zetta").join(name)
}

impl TestDaemon {
    fn log(&self) -> String {
        std::fs::read_to_string(self.config.join("zmux.log")).unwrap_or_default()
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn daemon_binary() -> PathBuf {
    // The integration test binary lives beside the crate's own binaries.
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let binary = path.join("zmux");

    // These tests drive a *separately built* executable, so cargo's rebuild of
    // the test target says nothing about whether the daemon is current. A
    // stale binary would quietly test the previous implementation, which is
    // exactly how an assertion here becomes vacuous without anyone noticing.
    assert!(
        binary.is_file(),
        "{} is missing; run `cargo build --bin zmux` first",
        binary.display()
    );
    let daemon_source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server.rs");
    let built = std::fs::metadata(&binary).unwrap().modified().unwrap();
    let edited = std::fs::metadata(&daemon_source)
        .unwrap()
        .modified()
        .unwrap();
    assert!(
        built >= edited,
        "{} is older than {}; run `cargo build --bin zmux` so these tests \
         exercise the current daemon",
        binary.display(),
        daemon_source.display()
    );
    binary
}

fn system_executable(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH must be set for this test");
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{name} was not found in PATH"))
}

/// A spawn attributed to another process, so a session can be owned by a window
/// this test is able to end.
fn spawn_request_as(
    session_id: Option<u64>,
    command: &str,
    client_process_id: u32,
) -> SpawnRequest {
    SpawnRequest {
        client_process_id,
        ..spawn_request(session_id, command)
    }
}

fn spawn_request(session_id: Option<u64>, command: &str) -> SpawnRequest {
    let mut env = HashMap::new();
    env.insert("TERM".to_owned(), "xterm-256color".to_owned());
    SpawnRequest {
        session_id,
        client_process_id: std::process::id(),
        program: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), command.to_owned()],
        env,
        working_directory: None,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 8,
            cell_height: 16,
        },
        console_palette: Default::default(),
    }
}

fn summary(session_id: u64, pane_id: u64) -> BackgroundSessionSummary {
    BackgroundSessionSummary {
        id: session_id,
        title: "test".to_owned(),
        authentication_required: false,
        active_pane: pane_id,
        layout: BackgroundPaneLayout::Pane { pane_id },
        panes: Vec::new(),
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

fn write_json_frame(stream: &mut std::os::unix::net::UnixStream, value: &serde_json::Value) {
    let bytes = serde_json::to_vec(value).unwrap();
    stream
        .write_all(&(u32::try_from(bytes.len()).unwrap()).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}

fn read_json_frame(stream: &mut std::os::unix::net::UnixStream) -> String {
    let mut length = [0; 4];
    stream.read_exact(&mut length).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}

fn try_read_json_frame(stream: &mut UnixStream) -> Option<serde_json::Value> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).ok()?;
    let mut bytes = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The secret every shared session in these tests is protected with.
///
/// Sharing one is what makes it joinable from another process, so the
/// multiplexer refuses to offer a session that has no secret: joining a session
/// is being handed whatever its terminals can already do.
const TEST_SECRET: &str = "test-secret";

/// Exercises the client-side retry without needing an executable from an older
/// checkout. The first server image rejects Configure exactly as a pre-Configure
/// daemon does; its accepted Upgrade flips the listener into the replacement
/// image, which accepts the raw retry. The real daemon tests cover the PTY and
/// session state carried by an upgrade separately.
#[test]
fn configure_retries_once_after_an_unsupported_request() {
    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("zmux.sock");
    let endpoint_path = directory.path().join("zmux.json");
    let token = "test-token".to_owned();
    let listener = UnixListener::bind(&socket_path).unwrap();
    std::fs::write(
        &endpoint_path,
        serde_json::json!({
            "version": zmux::transport::ENDPOINT_VERSION,
            "protocol_version": zmux::messages::PROTOCOL_VERSION,
            "process_id": 4242,
            "socket_path": socket_path,
            "token": token,
        })
        .to_string(),
    )
    .unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let upgraded = Arc::new(AtomicBool::new(false));
    let configure_requests = Arc::new(AtomicUsize::new(0));
    let upgrade_requests = Arc::new(AtomicUsize::new(0));
    let server = std::thread::spawn({
        let stop = stop.clone();
        let upgraded = upgraded.clone();
        let configure_requests = configure_requests.clone();
        let upgrade_requests = upgrade_requests.clone();
        move || {
            while !stop.load(Ordering::Acquire) {
                let (mut stream, _) = listener.accept().unwrap();
                let Some(request) = try_read_json_frame(&mut stream) else {
                    continue;
                };
                let name = request["request"]["request"].as_str();
                let response = match name {
                    // `configure_raw` verifies that the daemon is serving
                    // requests before each attempt. A pre-Configure daemon
                    // can answer Ping even though it rejects Configure.
                    Some("ping") => serde_json::json!({"response": "ok"}),
                    Some("configure") => {
                        configure_requests.fetch_add(1, Ordering::Relaxed);
                        if upgraded.load(Ordering::Acquire) {
                            serde_json::json!({"response": "ok"})
                        } else {
                            serde_json::json!({
                                "response": "error",
                                "message": "unknown variant `configure`, expected `spawn`"
                            })
                        }
                    }
                    Some("upgrade") => {
                        upgrade_requests.fetch_add(1, Ordering::Relaxed);
                        upgraded.store(true, Ordering::Release);
                        serde_json::json!({"response": "ok"})
                    }
                    _ => serde_json::json!({
                        "response": "error",
                        "message": "unexpected request in configure retry test"
                    }),
                };
                write_json_frame(&mut stream, &response);
            }
        }
    });

    let client = Client::connect_existing_at(directory.path())
        .unwrap()
        .unwrap();
    client.configure(Retention::None, Vec::new()).unwrap();
    assert_eq!(configure_requests.load(Ordering::Acquire), 2);
    assert_eq!(upgrade_requests.load(Ordering::Acquire), 1);

    stop.store(true, Ordering::Release);
    let _ = UnixStream::connect(&socket_path);
    server.join().unwrap();
}

/// Opens a session to other processes, as `Ctrl-Shift-K` does to a tab.
///
/// Every test that attaches from a second process needs it: a session belongs
/// to the process that made it until somebody says otherwise, and the
/// multiplexer refuses an attach from anywhere else.
/// The verifier for [`TEST_SECRET`], as a client creates one before sharing.
fn test_verifier() -> zmux::auth::SessionAuthentication {
    zmux::auth::SessionAuthentication::create(TEST_SECRET).expect("creating the session verifier")
}

fn share_session(client: &Client, session_id: u64, pane_id: u64) {
    let verifier = test_verifier();
    client
        .share(
            session_id,
            summary(session_id, pane_id),
            serde_json::Value::Null,
            Some(&verifier),
            true,
        )
        .expect("sharing the session");
}

/// Reads from a descriptor until `expected` appears, or gives up.
fn read_until(descriptor: &std::fs::File, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = String::new();
    let mut buffer = [0; 4096];
    while Instant::now() < deadline {
        let mut file = descriptor;
        match file.read(&mut buffer) {
            Ok(0) => {}
            Ok(read) => seen.push_str(&String::from_utf8_lossy(&buffer[..read])),
            Err(_) => {}
        }
        if seen.contains(expected) {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never saw {expected:?}; read {seen:?}");
}

fn process_is_alive(pid: u32) -> bool {
    // Signal 0 checks for existence. The child belongs to the daemon, not to
    // this process, so it cannot become a zombie we would misread as alive.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

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
        store
            .save_session(&PersistedSession {
                id: session_id,
                created_at: 1,
                updated_at: 2,
                summary: summary(session_id, 1),
                state: serde_json::Value::Null,
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
        store
            .save_session(&PersistedSession {
                id: session_id,
                created_at: 1,
                updated_at: 2,
                summary: summary(session_id, 1),
                state: serde_json::json!({"from": "disk"}),
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
        store
            .save_session(&PersistedSession {
                id: session_id,
                created_at: 1,
                updated_at: 2,
                summary: summary(session_id, 1),
                state: serde_json::json!({"survived": true}),
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
        store
            .save_session(&PersistedSession {
                id: session_id,
                created_at: 1,
                updated_at: 2,
                summary: summary(session_id, 1),
                state: serde_json::Value::Null,
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
    store
        .save_session(&PersistedSession {
            id: 96,
            created_at: 1,
            updated_at: 2,
            summary: summary(96, 1),
            state: serde_json::Value::Null,
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
        serde_json::from_slice(&age::decrypt(&identity, &ciphertext).unwrap()).unwrap();
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
        age::decrypt(&identity, &std::fs::read(snapshot_path).unwrap()).unwrap(),
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

    // Establish a two-second window so a busy test machine cannot let the
    // daemon restart consume the entire one-second first-failure window before
    // the post-restart assertion below runs.
    std::thread::sleep(Duration::from_millis(1_100));
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

    let ciphertext = std::fs::read(
        daemon
            .sessions_dir()
            .join(format!("persistence/session-{}-auth.age", pane.session_id)),
    )
    .unwrap();
    let metadata: serde_json::Value =
        serde_json::from_slice(&age::decrypt(&identity, &ciphertext).unwrap()).unwrap();
    assert_eq!(metadata["failed_authentications"], 2);
    assert!(metadata["backoff_seconds"].as_u64().unwrap() >= 2);

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
    std::thread::sleep(Duration::from_secs(3));
    client
        .resume_with_secret(
            pane.session_id,
            std::slice::from_ref(&identity_path),
            Some(&zmux::auth::SessionSecret::new("correct secret".to_owned())),
        )
        .unwrap();
    client.forget(pane.session_id).unwrap();
}

#[test]
fn a_session_outlives_the_client_that_started_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    // A command that keeps running, and announces itself so the test can tell
    // when the shell is actually up.
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    assert!(process_is_alive(pane.child_pid));

    // Detaching is the client dropping the terminal and telling the daemon to
    // hold the session.
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"tab": "state"}),
            None,
            Vec::new(),
        )
        .unwrap();

    // The client is gone, but the process is the daemon's child.
    drop(client);
    assert!(process_is_alive(pane.child_pid));

    let client = daemon.client();
    assert_eq!(client.list().unwrap().len(), 1);

    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached {
            pane: attached,
            state,
            ..
        } => {
            assert_eq!(attached.child_pid, pane.child_pid);
            assert_eq!(state, serde_json::json!({"tab": "state"}));
        }
        _ => panic!("an unprotected session must attach without a secret"),
    }
}

/// A session's identity belongs to the daemon, not to whatever id a client
/// puts in the summary it detaches with.
///
/// Zetta used to publish its own tab id there, which diverged from the mux
/// session id: the catalog then listed a session under one id while attach,
/// kill and resize looked for it under another. The session showed up in the
/// list but "did not exist" when anything tried to address it.
#[test]
fn a_client_summary_cannot_rename_a_session() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    // A summary whose id is deliberately wrong, as a client publishing its own
    // identifier would have been.
    let mut impostor = summary(pane.session_id, pane.pane_id);
    impostor.id = pane.session_id + 1000;
    eprintln!("MARKER: before first detach");
    client
        .detach(
            pane.session_id,
            impostor,
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    eprintln!("MARKER: after first detach");

    // Listed under the mux session id, not the id the client claimed.
    eprintln!("MARKER: before list");
    let listed = client.list().unwrap();
    eprintln!("MARKER: after list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, pane.session_id);

    // And addressable under that id.
    let attached = match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => pane,
        _ => panic!("the session must still attach under its own id"),
    };
    drop(attached);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    client.kill(pane.session_id).unwrap();
    assert!(client.list().unwrap().is_empty());
}

/// Only meaningful when the ring is compiled in: without it there is, by
/// design, nothing to replay.
#[cfg(feature = "scrollback-buffer")]
#[test]
fn output_produced_while_detached_is_replayed_on_attach() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; sleep 0.3; printf 'while-away'; sleep 60",
        ))
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
            vec![(pane.pane_id, b"snapshot-of-screen".to_vec())],
        )
        .unwrap();

    // Give the pane time to produce output with nobody attached.
    std::thread::sleep(Duration::from_millis(800));

    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => {
            let replay = String::from_utf8_lossy(&pane.replay).into_owned();
            // The screen as it was, then what happened while nobody was there.
            assert!(
                replay.starts_with("snapshot-of-screen"),
                "replay was {replay:?}"
            );
            assert!(replay.contains("while-away"), "replay was {replay:?}");
        }
        _ => panic!("attach failed"),
    }
}

#[test]
fn a_protected_session_needs_its_secret() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let verifier = zmux::auth::SessionAuthentication::create("correct horse").unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            Some(&verifier),
            Vec::new(),
        )
        .unwrap();

    assert!(matches!(
        client
            .attach(pane.session_id, Some(pane.pane_id), None)
            .unwrap(),
        AttachOutcome::AuthenticationRequired
    ));
    assert!(matches!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some("wrong".to_owned())
            )
            .unwrap(),
        AttachOutcome::AuthenticationFailed
    ));

    // The catalog must not describe a protected session, and must never carry
    // the verifier.
    let catalog = std::fs::read_to_string(
        std::fs::read_dir(daemon.sessions_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("zetta-"))
            })
            .expect("a published catalog"),
    )
    .unwrap();
    assert!(catalog.contains("Protected session"));
    assert!(!catalog.contains("argon2"));

    // The socket must not say more than the catalog does. The endpoint token
    // authenticates the channel, not a session: anything that could read it
    // would otherwise learn a protected session's commands and directories
    // without ever presenting its secret.
    let listed = client.list().unwrap();
    let session = listed
        .iter()
        .find(|listed| listed.id == pane.session_id)
        .expect("the session is held");
    assert!(session.authentication_required);
    assert_eq!(session.title, "Protected session");
    assert!(
        session.panes.is_empty(),
        "listing revealed a protected session's panes: {:?}",
        session.panes
    );

    // And attaching cannot be used to enumerate its panes either.
    assert!(matches!(
        client
            .attach(pane.session_id, Some(pane.pane_id), None)
            .unwrap(),
        AttachOutcome::AuthenticationRequired
    ));
}

#[test]
fn killing_a_session_ends_what_it_was_running() {
    let daemon = TestDaemon::start();
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

    client.kill(pane.session_id).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "the session kept running"
    );
    assert!(client.list().unwrap().is_empty());
}

#[test]
fn an_attached_pane_learns_that_its_process_exited() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();

    // An attached client reads the real terminal, so only the daemon — the
    // process's actual parent — can tell it how the process ended.
    let pane = client
        .spawn(spawn_request(None, "printf ready; exit 7"))
        .unwrap();
    let (mut pty, events) =
        alacritty_terminal::tty::attach(pane.descriptor, pane.child_pid).unwrap();
    // Drain the command's initial output. macOS can keep a PTY child in the
    // exiting path until the master has consumed those bytes.
    read_until(pty.file(), "ready");
    reporters.register(pane.pane_id, events);

    use alacritty_terminal::tty::EventedPty as _;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(event) = pty.next_child_event() {
            match event {
                alacritty_terminal::tty::ChildEvent::Exited(status) => {
                    assert_eq!(status.code(), Some(7));
                    return;
                }
                other => panic!("expected an exit status, got {other:?}"),
            }
        }
        assert!(Instant::now() < deadline, "the exit was never reported");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_socket_and_its_token_are_private_to_this_user() {
    use std::os::unix::fs::PermissionsExt as _;
    let daemon = TestDaemon::start();
    let sessions = daemon.sessions_dir();

    for name in ["zmux.json", "zmux.sock"] {
        let mode = std::fs::metadata(sessions.join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{name} must not be shared");
    }
    let directory = std::fs::metadata(&sessions).unwrap().permissions().mode();
    assert_eq!(directory & 0o777, 0o700);
}

#[test]
fn a_connection_with_the_wrong_token_gets_nothing() {
    let daemon = TestDaemon::start();
    let endpoint_path = daemon.sessions_dir().join("zmux.json");
    let endpoint: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&endpoint_path).unwrap()).unwrap();
    let socket = endpoint["socket_path"].as_str().unwrap();

    let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
    let request = serde_json::json!({
        "version": zmux::messages::PROTOCOL_VERSION,
        "token": "0".repeat(64),
        "request": {"request": "list"},
    });
    write_json_frame(&mut stream, &request);
    let response = read_json_frame(&mut stream);
    assert!(response.contains("invalid multiplexer token"), "{response}");
    assert!(!response.contains("\"sessions\""));
}

/// Fails early, and with a clearer message than a timeout, when the daemon
/// these tests drive was not rebuilt after a change.
#[test]
fn the_daemon_binary_is_current() {
    daemon_binary();
}

#[test]
fn retention_none_keeps_a_session_running_without_holding_its_output() {
    // The memory-constrained case: the multiplexer must still read a detached
    // pane, or its child blocks once the terminal buffer fills, but it keeps
    // none of what it reads.
    let daemon = TestDaemon::start_with(&["--retention", "none"]);
    let client = daemon.client();

    // Writing more than a terminal buffer holds only completes if something is
    // draining it. The marker file is the evidence: a child blocked mid-write
    // is still "alive", so liveness alone would prove nothing.
    let marker = daemon.config.join("finished-writing");
    let pane = client
        .spawn(spawn_request(
            None,
            &format!(
                "printf ready; i=0; while [ $i -lt 400 ]; do \
                 printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n'; \
                 i=$((i+1)); done; : > {}; sleep 60",
                marker.display()
            ),
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    assert!(
        !marker.exists(),
        "the writes finished before the pane detached"
    );

    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, b"a snapshot that must not be kept".to_vec())],
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        marker.exists(),
        "the detached pane blocked on a terminal buffer nobody was draining"
    );

    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => assert!(
            pane.replay.is_empty(),
            "retention \"none\" retained {} bytes",
            pane.replay.len()
        ),
        _ => panic!("attach failed"),
    }
}

#[test]
fn reconfiguring_to_none_discards_a_snapshot_but_keeps_the_session_process() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    client.configure(Retention::None, Vec::new()).unwrap();
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, b"this snapshot is discarded".to_vec())],
        )
        .unwrap();

    match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane: attached, .. } => {
            assert_eq!(attached.child_pid, pane.child_pid);
            assert!(attached.replay.is_empty(), "retention none replayed output");
            drop(std::fs::File::from(attached.descriptor));
        }
        _ => panic!("the reconfigured session must attach exclusively"),
    }
    assert!(process_is_alive(pane.child_pid));
    client.kill(pane.session_id).unwrap();
}

#[cfg(feature = "scrollback-buffer")]
#[test]
fn reconfiguring_to_memory_retains_a_snapshot_and_keeps_the_session_process() {
    let daemon = TestDaemon::start_with(&["--retention", "none"]);
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    client
        .configure(Retention::Memory { bytes: 4096 }, Vec::new())
        .unwrap();
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, b"snapshot retained after reload".to_vec())],
        )
        .unwrap();

    match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane: attached, .. } => {
            assert_eq!(attached.child_pid, pane.child_pid);
            assert!(
                String::from_utf8_lossy(&attached.replay)
                    .starts_with("snapshot retained after reload"),
                "the configured memory ring did not retain the snapshot: {:?}",
                attached.replay
            );
            drop(std::fs::File::from(attached.descriptor));
        }
        _ => panic!("the reconfigured session must attach exclusively"),
    }
    assert!(process_is_alive(pane.child_pid));
    client.kill(pane.session_id).unwrap();
}

#[test]
fn an_upgrade_keeps_the_sessions_and_their_processes() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let verifier = zmux::auth::SessionAuthentication::create("correct horse").unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"tab": "state"}),
            Some(&verifier),
            Vec::new(),
        )
        .unwrap();

    // One wrong secret, so the backoff window is open across the upgrade.
    assert!(matches!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some("wrong".to_owned())
            )
            .unwrap(),
        AttachOutcome::AuthenticationFailed
    ));

    client.upgrade().unwrap();

    // Waiting for a connection to succeed is not enough: until the replacement
    // has rebound, a connection can still land in the old listener's backlog
    // and be reset when the exec tears it down. Readiness is a request that
    // answers, not a socket that accepts.
    //
    // Generous, because an upgrade is several steps — a pre-flight subprocess,
    // the exec, re-adopting the sessions, rebinding — and this test is about
    // whether they happen at all, not how quickly. A machine busy compiling
    // was enough to make a ten-second bound report a failure that was not one.
    let deadline = Instant::now() + Duration::from_secs(60);
    let client = loop {
        if let Ok(Some(connected)) = Client::connect_ready_at(&daemon.sessions_dir())
            && connected.list().is_ok()
        {
            break connected;
        }
        assert!(
            Instant::now() < deadline,
            "the replacement never came back (process: {}); daemon log:\n{}",
            daemon.process.id(),
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(
        process_is_alive(pane.child_pid),
        "the session was restarted"
    );
    assert_eq!(client.list().unwrap().len(), 1);

    // Protection survives, and so does the rate limit: if `--upgrade` cleared
    // it, anyone able to trigger an upgrade could guess without penalty.
    assert!(matches!(
        client
            .attach(pane.session_id, Some(pane.pane_id), None)
            .unwrap(),
        AttachOutcome::AuthenticationRequired
    ));
    assert!(matches!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some("wrong again".to_owned())
            )
            .unwrap(),
        AttachOutcome::AuthenticationFailed
    ));

    // And the correct secret still works once the window has passed.
    std::thread::sleep(Duration::from_secs(3));
    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some("correct horse".to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached {
            pane: attached,
            state,
            ..
        } => {
            assert_eq!(attached.child_pid, pane.child_pid);
            assert_eq!(state, serde_json::json!({"tab": "state"}));
        }
        _ => panic!("the correct secret must still attach after an upgrade"),
    }
}

#[test]
fn a_dead_multiplexers_endpoint_does_not_block_starting_a_new_one() {
    // An endpoint outlives the daemon that wrote it. Judging the version
    // before checking whether anything is listening let a stale file from a
    // previous build refuse every attempt to start a live multiplexer —
    // permanently, and complaining about a process that no longer existed.
    let directory = tempfile::tempdir().unwrap();
    let sessions = directory.path().join("zetta").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("zmux.json"),
        serde_json::json!({
            "version": 1,
            "protocol_version": zmux::messages::PROTOCOL_VERSION - 1,
            "process_id": 999_999,
            "socket_path": sessions.join("zmux.sock"),
            "token": "00",
        })
        .to_string(),
    )
    .unwrap();

    assert!(
        Client::connect_existing_at(&sessions).unwrap().is_none(),
        "a stale endpoint must read as no multiplexer, not as a refusal"
    );
}

#[test]
fn a_multiplexer_from_another_build_is_refused_before_it_is_used() {
    // The failure this guards against: a daemon left over from an earlier
    // build accepts the connection, cannot parse the request, and closes —
    // leaving the client with no reason and every terminal failing to open.
    let daemon = TestDaemon::start();
    let endpoint_path = daemon.sessions_dir().join("zmux.json");
    let mut endpoint: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&endpoint_path).unwrap()).unwrap();
    endpoint["protocol_version"] = serde_json::json!(999);
    std::fs::write(&endpoint_path, endpoint.to_string()).unwrap();

    let error = match Client::connect_existing_at(&daemon.sessions_dir()) {
        Ok(_) => panic!("a multiplexer speaking another protocol must be refused"),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("999"), "{error}");
    assert!(error.contains("protocol version"), "{error}");
}

#[test]
fn a_client_from_a_newer_build_is_told_why_rather_than_dropped() {
    let daemon = TestDaemon::start();
    let endpoint: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(daemon.sessions_dir().join("zmux.json")).unwrap(),
    )
    .unwrap();

    let mut stream =
        std::os::unix::net::UnixStream::connect(endpoint["socket_path"].as_str().unwrap()).unwrap();
    // A version this daemon does not speak, carrying a field it has never seen.
    let request = serde_json::json!({
        "version": 999,
        "token": endpoint["token"],
        "client_process_id": std::process::id(),
        "request": {"request": "list"},
        "something_new": true,
    });
    write_json_frame(&mut stream, &request);
    let response = read_json_frame(&mut stream);
    assert!(
        response.contains("protocol version"),
        "expected a version error, got {response:?}"
    );
}

/// Replacing the multiplexer has to work across a protocol version boundary,
/// because crossing one is what it is for.
///
/// Rebuilding leaves a new client and an old daemon. Refusing `--upgrade` for
/// disagreeing about the version — as the general rule does, correctly, for every
/// other request — made every protocol bump a choice between running the new
/// build and keeping the sessions the old daemon holds.
#[test]
fn an_upgrade_is_accepted_from_a_client_that_disagrees_about_the_protocol() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    drop(descriptor);

    #[cfg(not(target_os = "macos"))]
    use std::os::unix::fs::MetadataExt as _;
    #[cfg(not(target_os = "macos"))]
    let socket_path = daemon.sessions_dir().join("zmux.sock");
    #[cfg(not(target_os = "macos"))]
    let socket_metadata = std::fs::metadata(&socket_path).unwrap();
    #[cfg(not(target_os = "macos"))]
    let before_socket_identity = (socket_metadata.dev(), socket_metadata.ino());
    let before_runner_id = zmux::catalog::read_session_catalogs(&daemon.sessions_dir())
        .unwrap()
        .first()
        .map(|catalog| catalog.runner_id)
        .unwrap();

    let endpoint: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(daemon.sessions_dir().join("zmux.json")).unwrap(),
    )
    .unwrap();
    let mut stream =
        std::os::unix::net::UnixStream::connect(endpoint["socket_path"].as_str().unwrap()).unwrap();
    let request = serde_json::json!({
        "version": 999,
        "token": endpoint["token"],
        "client_process_id": std::process::id(),
        "request": {"request": "upgrade"},
    });
    write_json_frame(&mut stream, &request);
    let response = read_json_frame(&mut stream);
    assert!(
        !response.contains("protocol version"),
        "the upgrade was refused over the version: {response:?}"
    );

    // And it really replaced itself, keeping what it held.
    let client = wait_for_multiplexer(&daemon);
    let sessions = client.list().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "the replacement did not keep the session: {sessions:?}"
    );
    assert!(
        process_is_alive(pane.child_pid),
        "the replacement lost the session's process"
    );
    #[cfg(not(target_os = "macos"))]
    {
        let socket_metadata = std::fs::metadata(&socket_path).unwrap();
        assert_eq!(
            before_socket_identity,
            (socket_metadata.dev(), socket_metadata.ino()),
            "this platform's upgrade must keep the listening socket rather than rebind it"
        );
    }
    let after_catalogs = zmux::catalog::read_session_catalogs(&daemon.sessions_dir()).unwrap();
    assert_eq!(
        after_catalogs.first().map(|catalog| catalog.runner_id),
        Some(before_runner_id)
    );
    assert_eq!(
        sessions.first().map(|session| session.id),
        Some(pane.session_id)
    );
}

#[test]
fn a_detached_session_is_drained_again_when_its_client_dies() {
    // Detaching is what asks the multiplexer to keep a session. If the window
    // showing it then dies without letting go, the pane has to be read again:
    // a pane nobody reads blocks its program the moment the terminal's buffer
    // fills, so the session would look alive while being frozen.
    let daemon = TestDaemon::start();
    let client = daemon.client();

    // The pane waits to be told before producing more than a terminal buffer
    // holds, so the writing happens after the client has died, not before.
    let trigger = daemon.config.join("go");
    let marker = daemon.config.join("kept-writing");
    let pane = client
        .spawn(spawn_request(
            None,
            &format!(
                "printf ready; while [ ! -f {} ]; do sleep 0.1; done; \
                 i=0; while [ $i -lt 400 ]; do \
                 printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n'; \
                 i=$((i+1)); done; : > {}; sleep 60",
                trigger.display(),
                marker.display()
            ),
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    // Shared while it is still held, so the window that picks it up below may
    // be a different process; a backgrounded session is otherwise its own
    // window's alone.
    share_session(&client, pane.session_id, pane.pane_id);
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

    // Taken by a window that is already gone, and dropped without detaching.
    let departed = reaped_process();
    match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            departed,
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("the held session should have attached"),
    }

    std::fs::write(&trigger, b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        marker.exists(),
        "the pane was never drained again, so its program stayed blocked"
    );
    assert!(
        client
            .list()
            .unwrap()
            .iter()
            .any(|session| session.id == pane.session_id),
        "a held session must stay available after its window dies"
    );
}

#[test]
fn a_session_nobody_detached_ends_with_the_window_that_had_it() {
    // Detaching is explicit. A window that dies without detaching leaves
    // sessions nobody asked to keep: promoting them turned every Zetta that
    // was killed into a pile of held sessions holding stray shells, listed
    // with no title and nothing the user could recognise or use.
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let departed = reaped_process();
    let mut request = spawn_request(None, "printf ready; sleep 60");
    request.client_process_id = departed;
    let pane = client.spawn(request).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    let deadline = Instant::now() + Duration::from_secs(30);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "a session nobody detached kept running after its window died"
    );
    assert!(
        client.list().unwrap().is_empty(),
        "a session nobody detached was offered as a background session"
    );
}

/// A process that has exited *and been reaped*, standing in for a client that
/// died. Waiting matters: an unreaped child is a zombie, and a zombie still
/// reports as an existing process.
fn reaped_process() -> u32 {
    let mut child = Command::new(system_executable("true")).spawn().unwrap();
    let id = child.id();
    child.wait().unwrap();
    id
}

#[test]
fn replacing_the_binary_out_from_under_the_daemon_does_not_lose_its_sessions() {
    // Rebuilding unlinks and recreates the executable, after which Linux reads
    // `/proc/self/exe` as "<path> (deleted)". Resolving the path at upgrade
    // time rather than at startup made `--upgrade` try to execute that, and
    // fail with a confusing error about a path nobody wrote.
    let directory = tempfile::tempdir().unwrap();
    let copied = directory.path().join("zmux");
    std::fs::copy(daemon_binary(), &copied).unwrap();

    let config = tempfile::tempdir().unwrap();
    let mut process = spawn_copied_daemon(&copied, config.path());
    // The copied binary is deliberately outside Cargo's target/debug tree, so
    // it uses the installed application's ordinary session directory.
    let sessions = config.path().join("zetta/sessions");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sessions.join("zmux.json").is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = loop {
        if let Ok(Some(client)) = Client::connect_ready_at(&sessions) {
            break client;
        }
        assert!(
            Instant::now() < deadline,
            "the copied daemon never became ready; process: {}; log:\n{}",
            process.id(),
            std::fs::read_to_string(config.path().join("zmux.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Exactly what a rebuild does to the running daemon's executable.
    std::fs::remove_file(&copied).unwrap();

    let error = client
        .upgrade()
        .expect_err("an upgrade with no executable to run must be refused");
    assert!(
        error.to_string().contains("no longer exists"),
        "expected a clear reason, got {error:#}"
    );

    // The point: refusing costs nothing, whereas attempting it would have
    // taken the sessions down.
    assert!(process_is_alive(pane.child_pid), "the session was lost");
    assert_eq!(client.list().unwrap().len(), 0);
    assert!(
        client.spawn(spawn_request(None, "sleep 1")).is_ok(),
        "the multiplexer must still be serving"
    );

    let _ = process.kill();
    let _ = process.wait();
}

#[test]
fn a_session_holding_no_panes_is_never_offered() {
    // The failure this guards against: a session with zero panes appearing in
    // the listing, with no title and nothing to attach. "Every pane is
    // unattached" is vacuously true of no panes, so the reclaim marked it as
    // available and published it.
    let daemon = TestDaemon::start();
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

    // Ending the session leaves the multiplexer holding nothing for it. Give
    // the reclaim more than one pass to notice.
    client.kill(pane.session_id).unwrap();
    std::thread::sleep(Duration::from_secs(5));

    assert!(
        client.list().unwrap().is_empty(),
        "a session with no panes was offered: {:?}",
        client.list().unwrap()
    );

    // And the published catalog agrees, since that is what `zmux list`
    // and the reconnect picker read.
    let catalog = std::fs::read_dir(daemon.sessions_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("zetta-"))
        });
    if let Some(catalog) = catalog {
        let contents = std::fs::read_to_string(catalog).unwrap();
        assert!(
            !contents.contains("\"panes\": []"),
            "the catalog offered a session with no panes: {contents}"
        );
    }
}

/// A session whose process has ended is not offered, and attaching to it is
/// refused rather than handed a terminal that can never produce output again.
#[test]
fn a_session_whose_process_ended_is_never_offered() {
    let daemon = TestDaemon::start();
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

    // The daemon keeps the shell as its child, so the test cannot wait() on
    // it. Kill it the way a process ends, and poll until the reaper has seen
    // it — the reaper only runs on SIGCHLD.
    unsafe { libc::kill(pane.child_pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "the pane's shell never exited"
    );

    // The session must be gone from the listing, not merely marked unavailable:
    // the reaper ends it as soon as its process ends.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut sessions = Vec::new();
    while Instant::now() < deadline {
        sessions = client.list().unwrap();
        if sessions.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        sessions.is_empty(),
        "a session whose process ended was still offered: {sessions:?}"
    );

    // And attaching to it must fail cleanly, not yield a dead terminal that
    // looks like a hung window.
    assert!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some(TEST_SECRET.to_owned()),
            )
            .is_err(),
        "attaching to an ended session must be refused"
    );
}

#[test]
fn upgrading_twice_does_not_duplicate_a_session() {
    // Each upgrade hands the sessions to the next image. If that image adopts
    // them without accounting for what it already has — or restores the
    // identifier counters in the wrong order — the same session comes back
    // twice, listed identically and impossible to tell apart.
    let daemon = TestDaemon::start();
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

    for round in 1..=2 {
        let client = wait_for_multiplexer(&daemon);
        client.upgrade().unwrap();
        let client = wait_for_multiplexer(&daemon);
        let sessions = client.list().unwrap();
        assert_eq!(
            sessions.len(),
            1,
            "after upgrade {round} the multiplexer offered {sessions:?}"
        );
    }
}

/// Waits until a multiplexer is answering, which after an upgrade means the
/// replacement has rebound rather than merely that the socket accepts.
fn wait_for_multiplexer(daemon: &TestDaemon) -> Client {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(Some(client)) = Client::connect_ready_at(&daemon.sessions_dir())
            && client.list().is_ok()
        {
            return client;
        }
        assert!(
            Instant::now() < deadline,
            "the multiplexer never came back (process: {}); log:\n{}",
            daemon.process.id(),
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A second client attaching to a held pane takes it over: the daemon asks
/// the holder to hand its screen back, and from then on every client reads the
/// pane together — shared mode.
#[test]
fn attaching_to_a_held_pane_handsover_and_makes_it_shared() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; while true; do sleep 0.2; printf 'tick\\n'; done",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // The holder answers the revoke: it stops reading, hands its screen back
    // as a snapshot, and re-attaches, which joins the pane's shared set.
    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask the holder to hand the pane over");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .expect("handing the screen back");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => Some(pane),
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second_pid = second_process.id();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_pid,
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder
        .join()
        .expect("the holder.s handover failed")
        .expect("the holder never re-attached");

    // Both clients are on the same data plane: each sees what the pane
    // produces, including what the other one typed.
    let mut holder_reader = holder_pane.reader();
    let mut second_reader = second.reader();
    read_until_reader(&mut holder_reader, "tick");
    read_until_reader(&mut second_reader, "tick");

    holder_pane
        .send_input(b"echo from-holder\n")
        .expect("sending input as the holder");
    read_until_reader(&mut second_reader, "from-holder");
    read_until_reader(&mut holder_reader, "from-holder");
    reap(second_process);
}

/// A third client attaches to a shared pane without disturbing anyone: there
/// is nothing to take over, so no revoke is sent.
#[test]
fn a_shared_pane_welcomes_further_clients_without_a_revoke() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; while true; do sleep 0.2; printf 'tick\\n'; done",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            let pane = match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            };
            (pane, revoke_rx)
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second_pid = second_process.id();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_pid,
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };

    // The holder has rejoined; the third client joins without any revoke
    // being sent — nobody has anything left to hand over.
    let (holder_pane, revoke_rx) = holder.join().expect("the holder's handover failed");
    let third_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let third = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            third_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a shared pane must welcome a third client"),
    };
    assert!(
        recv_timeout(&revoke_rx, Duration::from_millis(300)).is_none(),
        "joining a shared pane must not revoke anyone"
    );

    // All three read the same pane.
    let mut holder_reader = holder_pane.reader();
    let mut second_reader = second.reader();
    let mut third_reader = third.reader();
    read_until_reader(&mut second_reader, "tick");
    read_until_reader(&mut third_reader, "tick");
    read_until_reader(&mut holder_reader, "tick");

    // A client leaving the relay drops only itself; the others keep reading.
    drop(third);
    drop(third_reader);
    second
        .send_input(b"echo still-sharing\n")
        .expect("sending input through the remaining client");
    read_until_reader(&mut holder_reader, "still-sharing");
    read_until_reader(&mut second_reader, "still-sharing");
    reap(second_process);
    reap(third_process);
}

/// Shared clients are sized to the smallest of them, and a size report that
/// stops being the smallest lets the pane grow again.
#[test]
fn shared_clients_are_sized_to_the_smallest_of_them() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    // `stty size` prints the terminal's size, which is the pane's truth.
    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; while true; do sleep 0.3; stty size; done",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    read_until(&descriptor, "24 80");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder.s handover failed");
    let mut holder_reader = holder_pane.reader();
    let mut second_reader = second.reader();
    read_until_reader(&mut holder_reader, "24 80");

    // The smaller client's size wins, and is announced to everyone.
    second
        .send_resize(40, 10)
        .expect("reporting a smaller size");
    read_until_reader(&mut holder_reader, "10 40");
    read_until_reader(&mut second_reader, "10 40");
    assert_eq!(holder_pane.take_sizes().last(), Some(&(40, 10)));
    assert_eq!(second.take_sizes().last(), Some(&(40, 10)));

    // The largest client no longer sets the smallest size; the pane grows
    // back to the other client's.
    second
        .send_resize(120, 40)
        .expect("reporting a larger size");
    read_until_reader(&mut holder_reader, "24 80");
    read_until_reader(&mut second_reader, "24 80");
    reap(second_process);
}

/// Input a shared client sends reaches the pane, and the pane's exit reports
/// that it was typed into — which is the multiplexer's own attribution.
#[test]
fn shared_input_is_attributed_when_the_pane_exits() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; read line; printf 'got:%s\\n' \"$line\"; exit 0",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder.s handover failed");
    let mut holder_reader = holder_pane.reader();

    let (exit_tx, exit_rx) = async_channel::unbounded();
    reporters.register_shared(pane.pane_id, exit_tx);

    // The second client types; the pane answers, then exits.
    second.send_input(b"hello shared\n").unwrap();
    read_until_reader(&mut holder_reader, "got:hello shared");

    match recv_timeout(&exit_rx, Duration::from_secs(15)) {
        Some(report) => {
            assert_eq!(report.raw_status, Some(0));
            assert!(report.input_sent, "the shared input must be attributed");
            assert!(!report.disconnected);
        }
        None => panic!("the pane's exit was never reported"),
    }
    reap(second_process);
}

/// A pane that exits without any shared client having typed into it is
/// reported as such.
#[test]
fn a_shared_pane_that_received_no_input_reports_so() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    // Do not make the handover race a one-second sleep. Under a loaded or
    // virtualized scheduler the child can finish that sleep before the second
    // attach has made it through the daemon, which tests an already-ended pane
    // rather than shared exit reporting.
    let release_path = daemon.config.join("release-shared-exit");
    let mut request = spawn_request(
        None,
        "printf ready; while [ ! -e \"$ZMUX_TEST_RELEASE\" ]; do sleep 0.05; done; exit 3",
    );
    request.env.insert(
        "ZMUX_TEST_RELEASE".to_owned(),
        release_path.to_string_lossy().into_owned(),
    );
    let pane = client.spawn(request).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let _second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder.s handover failed");

    let (exit_tx, exit_rx) = async_channel::unbounded();
    reporters.register_shared(pane.pane_id, exit_tx);
    std::fs::write(&release_path, []).expect("releasing the pane to exit");

    match recv_timeout(&exit_rx, Duration::from_secs(15)) {
        Some(report) => {
            // The raw wait status: exit code 3 lives in the upper byte.
            assert_eq!(report.raw_status, Some(768));
            assert!(!report.input_sent, "nobody typed into the pane");
            assert!(!report.disconnected);
        }
        None => panic!("the pane's exit was never reported"),
    }
    drop(holder_pane);
    reap(second_process);
}

/// Sharing is a workflow of its own: a session on screen becomes joinable
/// without being detached first.
///
/// The daemon has always *allowed* attaching a live pane — that is what the
/// revoke handover is for — but it would not *offer* one, because availability
/// required `keep`, and only detaching sets that. So the sole route to a shared
/// session was to dismiss the tab and immediately take it back.
#[test]
fn a_live_session_is_only_offered_once_its_window_shares_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    assert!(
        client.list().unwrap().is_empty(),
        "a session nobody shared or detached must not be offered"
    );

    let mut offered = summary(pane.session_id, pane.pane_id);
    offered.title = "shared live".to_owned();
    client
        .share(
            pane.session_id,
            offered,
            serde_json::json!({"tab": 1}),
            None,
            true,
        )
        .unwrap();

    let sessions = client.list().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "sharing must offer the session: {sessions:?}"
    );
    assert_eq!(sessions[0].id, pane.session_id);
    assert_eq!(sessions[0].title, "shared live");
    // Still on screen, so joining it means a revoke handover rather than an
    // ordinary reconnect. This flag is what lets a picker say so.
    assert!(
        sessions[0].held,
        "a session its window is still showing must be listed as held"
    );

    // Withdrawing takes it off offer and touches nothing else: the pane is
    // still this client's, and its process is still running.
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            false,
        )
        .unwrap();
    assert!(
        client.list().unwrap().is_empty(),
        "a withdrawn session must stop being offered"
    );
    assert!(
        process_is_alive(pane.child_pid),
        "withdrawing an offer must not end the session"
    );
    assert_terminal_echoes(&descriptor, "still-here");
}

/// A session that is both kept and shared stays shared when its window hands
/// the terminal back. This is the daemon-side contract for Zetta's
/// `Ctrl-Shift-B`: closing that window must leave a session another process can
/// reconnect to, rather than narrowing it back to the old process.
#[test]
fn a_shared_session_stays_shared_when_it_is_detached() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .unwrap();
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

    let listed = client.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].scoped_to, None);

    let stranger = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    match client
        .attach_as(pane.session_id, pane.pane_id, stranger.id(), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("a shared kept session must attach from another process"),
    }
    reap(stranger);
}

/// Unsharing scopes a session back to one window, so it is only accepted while one
/// window has it.
///
/// There is no way to take a pane away from a viewer that is still relaying it — a
/// grant goes to the last viewer, not to a chosen one — so withdrawing the offer
/// while others are attached would leave the session listed as private while other
/// windows carried on driving it. Refusing says which, and leaves the tab shared.
#[test]
fn a_session_is_only_unshared_while_one_window_has_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let revokes = subscription.revokes.clone();
    let grants = subscription.grants.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let offer = |offered: bool| {
        client.share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"panes": 1}),
            None,
            offered,
        )
    };
    offer(true).expect("sharing a live session");

    // One window has it, so it can be scoped straight back.
    offer(false).expect("unsharing while this window alone has it");
    assert!(
        client.list().unwrap().is_empty(),
        "an unshared session must stop being offered"
    );
    offer(true).expect("sharing it again");

    // A second window joins, and now it cannot.
    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let (grant_tx, grant_rx) = async_channel::unbounded();
    grants.register(pane.pane_id, grant_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx.recv_blocking().expect("the revoke must arrive");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, Vec::new(), 80, 24)
                .expect("handing the screen back");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });
    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder's handover failed");

    let refused = offer(false).expect_err("two windows cannot be scoped back to one");
    assert!(
        refused.to_string().contains("still open in 2 windows"),
        "the refusal has to say why: {refused:#}"
    );
    assert_eq!(
        client.list().unwrap().len(),
        1,
        "a refused unshare must leave the session shared"
    );

    // Once the other window goes, it is accepted — and the pane is offered back,
    // so the session stops being relayed as well as stopping being listed.
    drop(second);
    reap(second_process);
    grant_rx
        .recv_blocking()
        .expect("the last viewer is offered the terminal");
    offer(false).expect("unsharing once this window alone has it again");
    drop(holder_pane);
}

/// Sharing says who may see the session now. It does not ask for the session to
/// outlive the window showing it — that is what detaching is for, and conflating
/// the two would turn every shared tab into a session left behind on a crash.
#[test]
fn sharing_a_session_does_not_ask_for_it_to_outlive_its_window() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .unwrap();

    // Closing the shared tab. Nobody asked for this session to be kept, so it
    // ends with the pane rather than being left behind holding a stray shell.
    drop(descriptor);
    client.close_pane(pane.session_id, pane.pane_id).unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    while process_is_alive(pane.child_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !process_is_alive(pane.child_pid),
        "a shared session nobody detached kept running after its tab was closed"
    );
    assert!(
        client.list().unwrap().is_empty(),
        "a shared session nobody detached was left behind as a background session"
    );
}

/// Only a client that is showing a session may offer it.
///
/// Not merely tidiness: the request republishes the session's verifier, so
/// accepting it from anybody would be a way to reprotect — or unprotect — a
/// session without presenting the secret it already has.
#[test]
fn only_a_client_showing_a_session_may_share_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let other_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let mut request = spawn_request(None, "printf ready; sleep 60");
    request.client_process_id = other_process.id();
    let pane = client.spawn(request).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    let error = client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .expect_err("a client that is not showing the session must be refused");
    assert!(
        error.to_string().contains("not showing it"),
        "the refusal must say why: {error:#}"
    );
    assert!(
        client.list().unwrap().is_empty(),
        "a refused share must not offer the session"
    );
    reap(other_process);
}

/// A client joining a shared session gets the layout as it is at that moment,
/// not as it was when sharing was switched on.
///
/// A session shared while it is on screen keeps changing: panes are split and
/// closed, tabs are renamed. The holder republishes as part of answering the
/// revoke, and the daemon re-reads the session's state once the handover
/// completes — reusing the copy read before the revoke handed the joining client
/// a layout that could be arbitrarily old.
#[test]
fn a_client_joining_a_shared_session_gets_its_layout_as_it_is_now() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; while true; do sleep 0.2; printf 'tick\\n'; done",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    client
        .share(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({"panes": 1}),
            None,
            true,
        )
        .unwrap();

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask the holder to hand the pane over");
            drop(descriptor);
            // The tab has been split since it was shared. Republished before
            // the snapshot, because the snapshot is what releases the waiting
            // attach.
            client
                .share(
                    session_id,
                    summary(session_id, pane_id),
                    serde_json::json!({"panes": 2}),
                    Some(&test_verifier()),
                    true,
                )
                .expect("refreshing the shared session");
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .expect("handing the screen back");
            client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .expect("the holder must re-attach");
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let state = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { state, .. } => state,
        _other => panic!("a held pane must attach as shared"),
    };
    holder.join().expect("the holder's handover failed");

    assert_eq!(
        state,
        serde_json::json!({"panes": 2}),
        "the joining client was given the layout as of when sharing was switched on"
    );
    reap(second_process);
}

/// A shared pane's round trip is bounded by how fast its terminal answers, not
/// by a timer inside the multiplexer.
///
/// The drain thread used to sleep a fixed twenty milliseconds whenever it found
/// nothing to do, so a shared keystroke waited on average half of that before
/// its echo was even read from the pty — measured at a ten millisecond median,
/// against ten microseconds for a client reading the pty itself. Worse, the
/// wake channel could not shorten it: it was drained *before* the sleep rather
/// than waited on, so `wake_drain` did nothing whatsoever.
///
/// The bound is deliberately loose. It is thirty-five times the measured figure
/// and still less than the old code's *best* case, so it says "no timer in the
/// path" without depending on how loaded the machine is.
#[test]
fn a_shared_panes_round_trip_is_not_paced_by_a_timer() {
    const ROUND_TRIP_BOUND: Duration = Duration::from_millis(5);

    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    // `cat` echoes a line as soon as it reads one, so the round trip measures
    // the multiplexer's relay rather than a shell's prompt handling.
    let pane = client.spawn(spawn_request(None, "exec cat")).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx.recv_blocking().expect("the revoke must arrive");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, Vec::new(), 80, 24)
                .expect("handing the screen back");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder's handover failed");

    // Both viewers are read throughout: a relay write that nobody drains blocks
    // the daemon, which would be measuring the wrong thing entirely.
    let mut holder_reader = holder_pane.reader();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drain_second = std::thread::spawn({
        let stop = stop.clone();
        move || {
            let mut reader = second.reader();
            let mut buffer = [0; 4096];
            // Blocks on the shared connection's read timeout between bursts, so
            // this waits rather than spins.
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if matches!(reader.read(&mut buffer), Ok(0)) {
                    break;
                }
            }
        }
    });

    let mut best = Duration::from_secs(60);
    for round in 0..15 {
        let probe = format!("probe{round}");
        let start = Instant::now();
        holder_pane
            .send_input(format!("{probe}\n").as_bytes())
            .expect("sending input as a shared client");
        read_until_reader(&mut holder_reader, &probe);
        best = best.min(start.elapsed());
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    drop(holder_pane);
    drain_second.join().ok();
    reap(second_process);

    assert!(
        best < ROUND_TRIP_BOUND,
        "a shared round trip took at least {best:?}, which is a timer rather than a terminal"
    );
}

/// A pane shared with one viewer is handed back to it, and reads its own terminal
/// from then on.
///
/// The reverse of the revoke handover, and the reason it exists: relaying to a
/// single viewer is the daemon reading a terminal that viewer could read itself,
/// at about a quarter more cost for sustained output. The ordering is the hard
/// part — everything already relayed must not be replayed, and everything still
/// queued has to arrive *before* the descriptor does.
#[test]
fn a_pane_shared_with_one_viewer_is_handed_back_to_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let revokes = subscription.revokes.clone();
    let grants = subscription.grants.clone();

    let pane = client.spawn(spawn_request(None, "exec cat")).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);

    // Into shared mode the usual way: a second client attaches and the holder
    // answers the revoke.
    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let (grant_tx, grant_rx) = async_channel::unbounded();
    grants.register(pane.pane_id, grant_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx.recv_blocking().expect("the revoke must arrive");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, Vec::new(), 80, 24)
                .expect("handing the screen back");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });
    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder's handover failed");
    let mut relayed = holder_pane.reader();

    // Something to see on both sides of the switch, so nothing can be lost or
    // repeated across it without showing up.
    holder_pane
        .send_input(
            b"while-shared
",
        )
        .expect("typing while shared");
    read_until_reader(&mut relayed, "while-shared");

    // The second viewer leaves, which is what makes the pane worth handing back.
    drop(second);
    reap(second_process);
    grant_rx
        .recv_blocking()
        .expect("the last viewer must be offered the terminal");

    let taken = client
        .take_exclusive(pane.session_id, pane.pane_id)
        .expect("taking the pane back");
    assert_eq!(taken.child_pid, pane.child_pid, "the same process, still");
    assert!(
        taken.replay.is_empty(),
        "a granted pane must carry no replay: the viewer already has that output"
    );
    // The relay is finished with, and its end is closed by the multiplexer — which
    // is how a client knows it has everything before it starts reading the pty.
    //
    // And it has to end *cleanly* — a plain end of stream, not an error. A
    // retirement reported as an error made the terminal's byte-stream worker print
    // one into the grid, which shifted a full-screen program's display by the lines
    // it took and left the message there for good.
    let mut buffer = [0; 4096];
    let deadline = Instant::now() + Duration::from_secs(10);
    let ended = loop {
        if Instant::now() >= deadline {
            break Err("the relay never ended".to_owned());
        }
        match relayed.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => break Err(format!("the relay ended as a failure: {error}")),
        }
    };
    assert_eq!(
        ended,
        Ok(()),
        "the relay has to end, and end cleanly: {ended:?}"
    );

    // And the descriptor is live: this is the same terminal, still running the
    // same program, now read directly.
    let descriptor = std::fs::File::from(taken.descriptor);
    assert_terminal_echoes(&descriptor, "after-the-handover");

    // Exclusive again, so a fresh attach revokes rather than joining a relay.
    drop(holder_pane);
    drop(descriptor);
}

/// A viewer slower than the program must be waited for, not dropped.
///
/// A terminal parses and renders; a program only writes. So a viewer is *always*
/// slower than sustained output, and dropping one for having a backlog meant
/// dropping every viewer of any real workload — `zetta benchmark output --size
/// 1000` cut it off a few megabytes in, every time, leaving the pane attached to a
/// closed connection with no way to exit it. Leaving the bytes in the terminal
/// instead makes the program wait, which is what an exclusive client's own reading
/// does for free.
#[test]
fn a_viewer_slower_than_the_program_is_waited_for() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client.spawn(spawn_request(None, "exec sh")).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx.recv_blocking().expect("the revoke must arrive");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, Vec::new(), 80, 24)
                .expect("handing the screen back");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });
    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder's handover failed");
    drop(second);
    reap(second_process);

    // Far more than any backlog bound would tolerate, read by a viewer that is
    // deliberately slower than the shell producing it.
    let mut reader = holder_pane.reader();
    holder_pane
        .send_input(b"seq 1 900000; echo bur''st-done\n")
        .expect("asking the shell for sustained output");

    let mut seen = String::new();
    let mut buffer = [0; 4096];
    let mut received = 0usize;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !seen.contains("burst-done") && Instant::now() < deadline {
        match reader.read(&mut buffer) {
            Ok(0) => panic!("the viewer was disconnected after {received} bytes"),
            Ok(read) => {
                received += read;
                seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
                if seen.len() > 4096 {
                    seen.drain(..seen.len() - 4096);
                }
                // Slower than the shell, which is what a terminal always is.
                std::thread::sleep(Duration::from_micros(500));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("the viewer was cut off after {received} bytes: {error}"),
        }
    }
    assert!(
        seen.contains("burst-done"),
        "a viewer slower than the program received only {received} bytes before it was \
         abandoned; the program has to wait for it instead"
    );
    drop(holder_pane);
}

/// A viewer that has gone stops holding the pane, promptly.
///
/// A viewer that is merely slow is *waited for*: the pane is left unread so the
/// program is throttled to what the slowest viewer can take, exactly as a single
/// window's own reading throttles it. That is deliberate, and it is why slowness is
/// not grounds for dropping anyone — a window laying out a full-screen redraw stops
/// draining its socket for a moment as a matter of course, and dropping it there
/// left it frozen mid-repaint with no way to know why.
///
/// What must not persist is a viewer that is *gone*. Its connection closing is the
/// signal, and it has to release the pane for whoever is left.
#[test]
fn a_viewer_that_goes_away_stops_holding_the_pane() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let revokes = subscription.revokes.clone();

    let pane = client.spawn(spawn_request(None, "exec sh")).unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx.recv_blocking().expect("the revoke must arrive");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, Vec::new(), 80, 24)
                .expect("handing the screen back");
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });
    let gone_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let gone = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            gone_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder's handover failed");
    let mut reader = holder_pane.reader();

    // Enough to outrun any buffer, with one viewer never reading a byte of it.
    holder_pane
        .send_input(b"seq 1 400000; echo bur''st-done\n")
        .expect("asking the shell for sustained output");

    // The pane is held for the viewer that is not reading, which is the intended
    // behaviour — so drop it, and the pane must come back for the one that is.
    std::thread::sleep(Duration::from_millis(300));
    drop(gone);
    reap(gone_process);

    // Promptly, not eventually: the daemon does have a last-resort timeout for a
    // window that has hung, and recovering on *that* instead would mean a viewer
    // closing its tab left the others stalled for half a minute.
    const RECOVERS_WITHIN: Duration = Duration::from_secs(10);
    let start = Instant::now();
    let mut seen = String::new();
    let mut buffer = [0; 8192];
    let deadline = start + RECOVERS_WITHIN;
    while !seen.contains("burst-done") && Instant::now() < deadline {
        match reader.read(&mut buffer) {
            Ok(0) => panic!("the reading viewer was disconnected"),
            Ok(read) => {
                seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
                if seen.len() > 4096 {
                    seen.drain(..seen.len() - 4096);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("the reading viewer was cut off: {error}"),
        }
    }
    assert!(
        seen.contains("burst-done"),
        "the pane did not come back for the viewer that was reading within {:?} of \
         the other one going",
        start.elapsed()
    );
    drop(holder_pane);
}

/// The screen a window hands over is what every *other* window joins on, and is
/// not sent back to the window that is still showing it.
///
/// Both halves were one bug: the first shared attach consumed the retained
/// buffer. That attach is the sharing window's own, so it redrew the screen it
/// already had — over wherever the program had left the cursor — and every
/// window that joined afterwards got only what the program had redrawn since,
/// which for a full-screen program is whatever changed and nothing else.
#[test]
fn a_handed_over_screen_reaches_the_windows_that_do_not_have_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    // Quiet after the handover, so what a client is sent is the screen and not
    // a race with the pane's own output.
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx.recv_blocking().expect("the revoke");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            }
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let holder_pane = holder.join().expect("the holder's handover failed");

    assert!(
        !String::from_utf8_lossy(&holder_pane.replay).contains("ready-screen"),
        "the window that handed the screen over must not be sent it back: {:?}",
        String::from_utf8_lossy(&holder_pane.replay)
    );
    assert!(
        String::from_utf8_lossy(&second.replay).contains("ready-screen"),
        "a joining window must be sent the screen: {:?}",
        String::from_utf8_lossy(&second.replay)
    );

    // And the buffer survives that join too: the third window is as entitled to
    // the screen as the second.
    let third_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let third = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            third_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a shared pane must welcome a third client"),
    };
    assert!(
        String::from_utf8_lossy(&third.replay).contains("ready-screen"),
        "every later window joins on the same screen: {:?}",
        String::from_utf8_lossy(&third.replay)
    );

    drop(holder_pane);
    drop(second);
    drop(third);
    reap(second_process);
    reap(third_process);
}

/// When the last shared client lets go, the pane is nobody's again: a fresh
/// attach takes it exclusively, exactly as though it had never been shared.
#[test]
fn a_pane_returns_to_exclusive_after_its_last_shared_client_leaves() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();
    let revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; while true; do sleep 0.2; printf 'tick\\n'; done",
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // Shared first, as `Ctrl-Shift-K` does: a session belongs to the process
    // that made it, and the multiplexer refuses an attach from anywhere else.
    share_session(&client, pane.session_id, pane.pane_id);
    let (revoke_tx, revoke_rx) = async_channel::unbounded();
    revokes.register(pane.pane_id, revoke_tx);
    let holder = std::thread::spawn({
        let client = daemon.client();
        let session_id = pane.session_id;
        let pane_id = pane.pane_id;
        let descriptor = descriptor;
        move || {
            revoke_rx
                .recv_blocking()
                .expect("the daemon must ask for the pane");
            drop(descriptor);
            client
                .send_snapshot(session_id, pane_id, b"ready-screen".to_vec(), 80, 24)
                .unwrap();
            let pane = match client
                .attach_as(session_id, pane_id, std::process::id(), None)
                .unwrap()
            {
                AttachOutcome::SharedAttached { pane, .. } => pane,
                _other => panic!("the holder must re-attach in shared mode"),
            };
            (pane, revoke_rx)
        }
    });

    let second_process = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let second = match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            second_process.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a held pane must attach as shared"),
    };
    let (holder_pane, revoke_rx) = holder.join().expect("the holder's handover failed");

    // The holder leaves; the second client keeps the pane shared.
    drop(holder_pane);
    std::thread::sleep(Duration::from_millis(300));
    let mut second_reader = second.reader();
    read_until_reader(&mut second_reader, "tick");

    // The last shared client leaves; the pane is nobody's again.
    drop(second);
    drop(second_reader);
    std::thread::sleep(Duration::from_millis(300));

    // A fresh attach is exclusive: it takes the descriptor, no revoke, no
    // shared set. With the secret, because sharing the session set one.
    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane: taken, .. } => {
            let descriptor = std::fs::File::from(taken.descriptor);
            read_until(&descriptor, "tick");
        }
        _other => panic!("a pane nobody holds must attach exclusively"),
    }
    assert!(
        recv_timeout(&revoke_rx, Duration::from_millis(300)).is_none(),
        "an exclusive attach must not revoke a shared pane's clients — there are none"
    );
    reap(second_process);
}

/// Reaps a test's stand-in second client, which is otherwise left to die on
/// its own timer.
fn reap(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads from a shared reader until `expected` appears, or gives up.
fn read_until_reader(reader: &mut impl Read, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = String::new();
    let mut buffer = [0; 4096];
    while Instant::now() < deadline {
        match reader.read(&mut buffer) {
            Ok(0) => {}
            Ok(read) => seen.push_str(&String::from_utf8_lossy(&buffer[..read])),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("reading the shared pane failed: {error}"),
        }
        if seen.contains(expected) {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never saw {expected:?}; read {seen:?}");
}

/// Starts a daemon from a freshly copied binary, retrying `ETXTBSY`.
///
/// Copying a file and immediately executing it races the other tests: these run
/// as threads of one process, so a `Command::spawn` on any other thread forks a
/// child that inherits this copy's still-open write descriptor, and `execve`
/// refuses to run a file anyone holds open for writing. The child closes it on
/// its own exec moments later, so retrying is the fix; the alternative is a test
/// that fails for a reason that has nothing to do with what it checks.
fn spawn_copied_daemon(binary: &Path, config: &Path) -> Child {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match Command::new(binary)
            .arg("--daemon")
            .env("XDG_CONFIG_HOME", config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(config.join("zmux.log")).unwrap())
            .spawn()
        {
            Ok(process) => return process,
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("starting the copied daemon failed: {error}"),
        }
    }
}

/// Writes to a pane's terminal and reads the line discipline's echo back.
///
/// Proves the pane is still *usable* rather than merely un-errored. A master that
/// had hung up could not echo, and a hung-up master is the precondition for both
/// halves of the reported bug: it is what the event loop waits on for an exit
/// report, and what it eventually gives up on. Asserting "no exit was reported"
/// alone would pass just as well for a pane that had quietly gone dead.
fn assert_terminal_echoes(descriptor: &std::fs::File, probe: &str) {
    let mut file = descriptor;
    file.write_all(format!("{probe}\n").as_bytes())
        .expect("the terminal would not accept input after the upgrade");
    read_until(descriptor, probe);
}

/// Whether a process has ended but not been reaped.
///
/// `kill(pid, 0)` succeeds for a zombie, so it cannot tell "still running" from
/// "ended and never waited for" — which is exactly the distinction that matters
/// when checking that the daemon still reaps its own children after replacing
/// itself.
fn process_is_zombie(pid: u32) -> bool {
    let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // The state is the field after the parenthesised command name, which may
    // itself contain spaces and parentheses.
    let Some(after_name) = status.rsplit_once(") ") else {
        return false;
    };
    after_name.1.starts_with('Z')
}

/// An upgrade while a client is still holding a pane's terminal.
///
/// The upgrade tests above all detach first, which meant the daemon only ever
/// re-adopted panes nobody was holding — and so never exercised the case that
/// broke: an adopted pane whose child the replacement treated as somebody
/// else's, never reaping it and reading its dead signal pipe as an exit.
#[test]
fn an_upgrade_keeps_a_pane_that_is_still_attached() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let session_id = pane.session_id;
    let pane_id = pane.pane_id;
    let child_pid = pane.child_pid;
    let (pty, events) = alacritty_terminal::tty::attach(pane.descriptor, pane.child_pid).unwrap();
    reporters.register(pane_id, events);
    // Kept, so the session is the daemon's to hold across the upgrade while
    // this client still holds the pane exclusively.
    client
        .detach(
            session_id,
            summary(session_id, pane_id),
            serde_json::json!({}),
            None,
            Vec::new(),
        )
        .unwrap();
    match client
        .attach(session_id, Some(pane_id), Some(TEST_SECRET.to_owned()))
        .unwrap()
    {
        AttachOutcome::Attached { .. } => {}
        _ => panic!("expected an exclusive attach"),
    }

    client.upgrade().unwrap();
    let client = wait_for_multiplexer(&daemon);

    assert!(process_is_alive(child_pid), "the pane's process was ended");
    // The terminal this client is holding still works. Its descriptor was never
    // the multiplexer's to touch, and an upgrade that left it hung up would be
    // indistinguishable to the user from the pane having died.
    assert_terminal_echoes(pty.file(), "still-alive-after-upgrade");
    // Asked about directly rather than through the catalog: a session a window
    // is showing is not listed there, because it is on screen rather than
    // waiting to be picked up.
    let states = client.pane_states(vec![pane_id]).unwrap();
    assert_eq!(
        states.iter().filter(|state| !state.unknown).count(),
        1,
        "the session was dropped; log:\n{}",
        daemon.log()
    );

    // The replacement must not have decided the pane ended. Reading a carried
    // signal pipe whose writer the exec had closed reported end-of-file, which
    // the reaper took for an exit — so a shell that was running perfectly well
    // was marked dead and pruned. Give the reaper something to wake on first:
    // it only re-examines panes when a child ends.
    reap(
        Command::new(system_executable("true"))
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(300));
    let states = client.pane_states(vec![pane_id]).unwrap();
    assert_eq!(states.len(), 1);
    assert!(
        !states[0].unknown && !states[0].exited,
        "the replacement reported a running pane as ended: {:?}; log:\n{}",
        states[0],
        daemon.log()
    );

    // And it must still be able to reap it, which it can only do because an
    // `execv` keeps the parent/child relationship. Treating the child as
    // foreign left it a zombie whose exit was never reported to anybody.
    unsafe { libc::kill(child_pid as i32, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let states = client.pane_states(vec![pane_id]).unwrap();
        if states[0].exited || states[0].unknown {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the replacement never reaped the pane's process (zombie: {}); log:\n{}",
            process_is_zombie(child_pid),
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !process_is_zombie(child_pid),
        "the replacement noticed the exit but never reaped the process"
    );
}

/// A client whose subscription an upgrade tore down still learns about exits.
///
/// This is the regression that mattered most: a lost subscription used to be
/// reported to every attached pane as its own process having ended — so a
/// `zmux --upgrade` showed an error in panes that were running fine — and the
/// subscription was never re-established, so no pane opened afterwards could
/// ever be told it had exited.
#[test]
fn a_subscription_survives_the_multiplexer_replacing_itself() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();

    let held = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let session_id = held.session_id;
    let (mut pty, events) =
        alacritty_terminal::tty::attach(held.descriptor, held.child_pid).unwrap();
    reporters.register(held.pane_id, events);
    client
        .detach(
            session_id,
            summary(session_id, held.pane_id),
            serde_json::json!({}),
            None,
            Vec::new(),
        )
        .unwrap();
    match client
        .attach(session_id, Some(held.pane_id), Some(TEST_SECRET.to_owned()))
        .unwrap()
    {
        AttachOutcome::Attached { .. } => {}
        _ => panic!("expected an exclusive attach"),
    }

    client.upgrade().unwrap();
    let client = wait_for_multiplexer(&daemon);

    // Nothing may have been reported for the held pane. Its process is running,
    // its terminal is this test's to read, and the only thing that changed is a
    // connection that carried no data for it.
    use alacritty_terminal::tty::EventedPty as _;
    assert!(
        pty.next_child_event().is_none(),
        "losing the subscription was reported as the pane's process ending"
    );
    assert_terminal_echoes(pty.file(), "held-pane-still-usable");

    // The subscription reconnects on its own, so a pane spawned after the
    // upgrade is still told when it ends.
    let deadline = Instant::now() + Duration::from_secs(30);
    let fresh = loop {
        match client.spawn(spawn_request(None, "exit 5")) {
            Ok(pane) => break pane,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("could not spawn after the upgrade: {error:#}"),
        }
    };
    let (mut fresh_pty, fresh_events) =
        alacritty_terminal::tty::attach(fresh.descriptor, fresh.child_pid).unwrap();
    reporters.register(fresh.pane_id, fresh_events);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match fresh_pty.next_child_event() {
            Some(alacritty_terminal::tty::ChildEvent::Exited(status)) => {
                assert_eq!(status.code(), Some(5));
                break;
            }
            Some(other) => panic!("expected an exit status, got {other:?}"),
            None => {}
        }
        assert!(
            Instant::now() < deadline,
            "a pane spawned after the upgrade was never told it exited; log:\n{}",
            daemon.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// An exit that happens while nobody is subscribed is still reported.
///
/// Events are broadcast to whoever is listening at that instant, so an exit
/// during the gap between a daemon going away and its replacement being
/// subscribed to has already been and gone. Without reconciling on reconnect,
/// the pane showing it would wait for a notification that no longer exists.
#[test]
fn an_exit_missed_while_disconnected_is_reported_on_reconnect() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client.spawn(spawn_request(None, "sleep 60")).unwrap();
    let (_pty, events) = alacritty_terminal::tty::attach(pane.descriptor, pane.child_pid).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::json!({}),
            None,
            Vec::new(),
        )
        .unwrap();
    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { .. } => {}
        _ => panic!("expected an exclusive attach"),
    }

    // Ends while there is deliberately no subscriber at all, so the broadcast
    // reaches nobody.
    unsafe { libc::kill(pane.child_pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let states = client.pane_states(vec![pane.pane_id]).unwrap();
        if states[0].exited || states[0].unknown {
            break;
        }
        assert!(Instant::now() < deadline, "the process never ended");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Subscribing now and registering the reporter must deliver the exit that
    // has already happened.
    let subscription = client.subscribe().unwrap();
    let reporters = subscription.exits.clone();
    let _revokes = subscription.revokes.clone();
    let missed = client.pane_states(vec![pane.pane_id]).unwrap();
    reporters.register(pane.pane_id, events);
    for report in missed {
        assert!(
            report.exited || report.unknown,
            "the multiplexer forgot that the pane had ended"
        );
    }
}

/// Stopping the multiplexer without hunting for its process.
///
/// The alternative is `pkill zmux`, which matches every multiplexer this user
/// is running and ends their sessions without asking. So the refusal is the
/// feature: a daemon holding a session says how many and stays up, and --force
/// is the caller saying they meant to end what it was holding.
#[test]
fn the_multiplexer_can_be_asked_to_stop() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    let error = format!(
        "{:#}",
        zmux::stop(&daemon.sessions_dir(), false)
            .expect_err("a multiplexer holding a session must not be stopped by accident")
    );
    assert!(error.contains("holding 1 session"), "{error}");
    assert!(error.contains("--force"), "{error}");
    assert!(
        Client::connect_ready_at(&daemon.sessions_dir())
            .unwrap()
            .is_some(),
        "a refused stop must leave the multiplexer running"
    );

    // Forced, it goes, and what it was holding goes with it.
    drop(descriptor);
    drop(client);
    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), true).expect("stopping the multiplexer"),
        zmux::StopOutcome::Signalled {
            process_id: daemon.process_id(),
        },
    );
    assert!(
        Client::connect_ready_at(&daemon.sessions_dir())
            .unwrap()
            .is_none(),
        "the multiplexer must be gone once it says it stopped: {}",
        daemon.log()
    );

    // And stopping what is not running is not a failure: the request is that it
    // not be running, and it is not.
    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), false).expect("stopping nothing"),
        zmux::StopOutcome::NotRunning,
    );
}

/// An idle multiplexer is asked, not signalled: it removes its own socket and
/// endpoint on the way out, which a signal cannot make it do.
#[test]
fn an_idle_multiplexer_stops_when_asked() {
    let daemon = TestDaemon::start();

    assert_eq!(
        zmux::stop(&daemon.sessions_dir(), false).expect("stopping the multiplexer"),
        zmux::StopOutcome::Stopped,
    );
    assert!(
        !daemon.sessions_dir().join("zmux.sock").exists(),
        "a multiplexer that stopped when asked cleans up after itself: {}",
        daemon.log()
    );
}

/// A privately backgrounded session belongs to the window that backgrounded it.
///
/// Before the multiplexer held these sessions they lived inside the process
/// that made them, and no other Zetta could see or take one. Moving them out
/// published every one of them to every process, so a privately kept tab put
/// away in one window turned up in another window's reconnect picker.
/// `Ctrl-Shift-K` and the shared keep-running path are the explicit exceptions.
#[test]
fn a_backgrounded_session_is_scoped_to_the_process_that_backgrounded_it() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
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

    // Published as this process's, so another window's picker can leave it out.
    let listed = client.list().unwrap();
    assert_eq!(
        listed.first().map(|session| session.scoped_to),
        Some(Some(std::process::id())),
        "a backgrounded session says whose it is: {listed:?}"
    );

    // And refused to anybody else, whatever their picker happens to show.
    let stranger = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let error = match client.attach_as(
        pane.session_id,
        pane.pane_id,
        stranger.id(),
        Some(TEST_SECRET.to_owned()),
    ) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("another process must not attach a scoped session"),
    };
    assert!(error.contains("scoped to"), "{error}");

    // Its own window still may, which is what reconnecting is.
    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("the owning process must attach"),
    }
    reap(stranger);
}

/// The CLI's half of the sharing toggle, for a session with no window to
/// toggle it from.
#[test]
fn a_backgrounded_session_can_be_shared_and_scoped_back() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
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

    // Already protected — the detach above had to give it a secret — so sharing
    // it needs no new one. `a_session_cannot_be_shared_without_a_secret` covers
    // the session that has none.
    client
        .set_session_scope(pane.session_id, true, None)
        .unwrap();
    let listed = client.list().unwrap();
    assert_eq!(
        listed.first().map(|session| session.scoped_to),
        Some(None),
        "a shared session is nobody's in particular: {listed:?}"
    );

    // Which is what another process attaching it means.
    let stranger = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    match client
        .attach_as(
            pane.session_id,
            pane.pane_id,
            stranger.id(),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("a shared session must attach from anywhere"),
    }
    // Off screen again — the multiplexer reclaims a pane whose window has gone —
    // because scoping a session back is refused while a window still has it.
    reap(stranger);
    let deadline = Instant::now() + Duration::from_secs(15);
    while client
        .list()
        .unwrap()
        .first()
        .is_some_and(|session| session.held)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(100));
    }

    client
        .set_session_scope(pane.session_id, false, None)
        .unwrap();
    let listed = client.list().unwrap();
    assert_eq!(
        listed.first().map(|session| session.scoped_to),
        Some(Some(std::process::id())),
        "scoping back returns the session to the window that backgrounded it: {listed:?}"
    );
}

/// Protecting a session is the user's choice, and the same choice everywhere.
///
/// Detaching a tab has always taken an empty dialog to mean "leave it
/// unprotected", and keeping one running the same. Sharing is the same choice:
/// the dialog asks, and the multiplexer takes what it is given rather than
/// insisting. What a secret buys is identical in all three cases — nothing
/// attaches the session without it.
#[test]
fn protecting_a_session_is_optional_whichever_way_it_is_made_reachable() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    let unprotected = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(unprotected.descriptor);
    read_until(&descriptor, "ready");

    // Shared with no secret, as the dialog's empty pair asks for.
    client
        .share(
            unprotected.session_id,
            summary(unprotected.session_id, unprotected.pane_id),
            serde_json::Value::Null,
            None,
            true,
        )
        .expect("sharing without a secret is a choice, not an error");
    // And kept with no secret, which is what detaching an unprotected tab does.
    drop(descriptor);
    client
        .detach(
            unprotected.session_id,
            summary(unprotected.session_id, unprotected.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .expect("keeping without a secret is a choice, not an error");
    let stranger = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    match client
        .attach_as(
            unprotected.session_id,
            unprotected.pane_id,
            stranger.id(),
            None,
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("an unprotected shared session attaches without a secret"),
    }

    // The other choice, on a second session: a secret, and nothing joins without
    // it.
    let protected = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(protected.descriptor);
    read_until(&descriptor, "ready");
    share_session(&client, protected.session_id, protected.pane_id);
    assert!(
        matches!(
            client
                .attach_as(protected.session_id, protected.pane_id, stranger.id(), None)
                .unwrap(),
            AttachOutcome::AuthenticationRequired
        ),
        "joining a protected session must need its secret"
    );
    assert!(
        matches!(
            client
                .attach_as(
                    protected.session_id,
                    protected.pane_id,
                    stranger.id(),
                    Some("not-the-secret".to_owned())
                )
                .unwrap(),
            AttachOutcome::AuthenticationFailed
        ),
        "a wrong secret must not join a protected session"
    );
    reap(stranger);
}

/// A protected session's administrative boundary is tied to the local socket
/// peer, not to the process id a client writes into its JSON envelope.
#[test]
fn protected_controls_reject_a_claimed_owner_without_peer_authority() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let owner = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let owner_pid = owner.id();
    let pane = client
        .spawn(spawn_request_as(None, "printf ready; sleep 120", owner_pid))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);

    // The session starts unprotected, so this test can set up an owner label
    // that differs from the actual peer without bypassing the boundary it is
    // testing. Once protected, changing only the claimed process id must not
    // make the current test process an administrator.
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            owner_pid,
        )
        .unwrap();
    client
        .set_session_scope(pane.session_id, true, Some(&test_verifier()))
        .unwrap();

    for error in [
        client.kill(pane.session_id).unwrap_err().to_string(),
        client.forget(pane.session_id).unwrap_err().to_string(),
        client
            .resize(pane.session_id, pane.pane_id, 100, 30)
            .unwrap_err()
            .to_string(),
        client
            .close_pane(pane.session_id, pane.pane_id)
            .unwrap_err()
            .to_string(),
        client
            .set_session_scope(pane.session_id, false, None)
            .unwrap_err()
            .to_string(),
    ] {
        assert!(
            error.contains("protected"),
            "unexpected authorization error: {error}"
        );
    }
    let state = client.pane_states(vec![pane.pane_id]).unwrap();
    assert!(
        state[0].unknown,
        "protected pane state leaked to a foreign peer"
    );
    assert_eq!(
        client.list().unwrap().len(),
        1,
        "unauthorized controls changed the session"
    );

    reap(owner);
}

/// A backgrounded session stays its window's, even after that window is gone.
///
/// Releasing the scope when the process exited made backgrounding a slow way of
/// sharing: leave the window, and the session another Zetta could not see became
/// one it could attach. Widening a session is an explicit request, so the scope
/// outlives the window and `zmux share` is the way back in.
#[test]
fn a_backgrounded_session_stays_scoped_after_its_window_exits() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    // Backgrounded by a window that then exits, which is what a crash looks
    // like from here.
    let window = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let window_pid = window.id();
    let pane = client
        .spawn(spawn_request_as(
            None,
            "printf ready; sleep 120",
            window_pid,
        ))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            window_pid,
        )
        .unwrap();
    reap(window);

    // Long enough for the liveness sweep to have noticed, which is what used to
    // hand the session to everybody.
    std::thread::sleep(Duration::from_secs(3));
    let listed = client.list().unwrap();
    assert_eq!(
        listed.first().map(|session| session.scoped_to),
        Some(Some(window_pid)),
        "the scope outlives the window: {listed:?}"
    );
    let error = match client.attach(pane.session_id, Some(pane.pane_id), None) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a departed window's session must not attach elsewhere"),
    };
    assert!(error.contains("has exited"), "{error}");

    // Explicitly shared, it attaches — the one way in, and a deliberate one.
    client
        .set_session_scope(pane.session_id, true, None)
        .unwrap();
    match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => drop(std::fs::File::from(pane.descriptor)),
        _ => panic!("an explicitly shared session attaches from anywhere"),
    }
}

/// A session a window is showing is not something to pick up.
///
/// `keep` is sticky, so reattaching leaves it set — a window that then crashes
/// must still find the session. Listing a session on that basis put the tab the
/// user was looking at into their own reconnect picker, with the reconnect button
/// beside it permanently lit.
#[test]
fn a_reattached_session_stops_being_listed() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
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
    assert_eq!(
        client.list().unwrap().len(),
        1,
        "a detached session is waiting to be picked up"
    );

    let taken = match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => pane,
        _ => panic!("its own window must attach it"),
    };
    assert!(
        client.list().unwrap().is_empty(),
        "a session on screen is not offered back to the window showing it"
    );
    // Still held, and still kept: detaching it again lists it again rather than
    // needing the request to be made twice.
    drop(std::fs::File::from(taken.descriptor));
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    assert_eq!(client.list().unwrap().len(), 1);

    // Sharing is the one reason to list a session a window is showing: another
    // window may then join it, which is what "in use" means.
    match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => {
            std::mem::forget(std::fs::File::from(pane.descriptor))
        }
        _ => panic!("its own window must attach it"),
    }
    share_session(&client, pane.session_id, pane.pane_id);
    let listed = client.list().unwrap();
    assert_eq!(
        listed.len(),
        1,
        "a shared session is listed while on screen"
    );
    assert!(
        listed[0].held,
        "and says a window is showing it: {listed:?}"
    );
}

/// A full-screen program comes back as its screen, however long it has run.
///
/// The multiplexer used to keep bytes: a bounded buffer of what a pane printed
/// most recently. For a program that repaints parts of a screen that is a pile of
/// fragments describing a screen the buffer has already dropped to make room for
/// them, so a reattached `htop` came back as pieces of itself over a blank
/// terminal. What is kept now is a grid — the screen those fragments were
/// painting on — so the parts nothing has repainted are still there.
#[test]
fn a_full_screen_program_comes_back_as_its_screen() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    // A program that draws a screen and then repaints one field of it, tens of
    // thousands of times: far more bytes than any buffer of recent output holds,
    // and every one of them addressed to a line rather than appended.
    let pane = client
        .spawn(spawn_request(
            None,
            "printf ready; sleep 1; \
             printf '\\033[?1049h\\033[HHEADER\\r\\nbody\\r\\nFOOTER'; \
             i=0; while [ $i -lt 20000 ]; do printf '\\033[2;1Hbody %d' \"$i\"; i=$((i+1)); done; \
             sleep 120",
        ))
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

    // Read by the multiplexer while nobody is showing it, which is when what it
    // keeps is all there is.
    std::thread::sleep(Duration::from_secs(6));
    let taken = match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => pane,
        _ => panic!("its own window must attach it"),
    };
    let replay = String::from_utf8_lossy(&taken.replay).into_owned();
    assert!(
        replay.contains("body 19999"),
        "the last repaint must be there: {replay:?}"
    );
    // The parts a repainting program does not repaint are exactly what a buffer
    // of recent bytes loses.
    assert!(
        replay.contains("HEADER") && replay.contains("FOOTER"),
        "the screen under the repaints must be there: {replay:?}"
    );
    // And it comes back into the screen it was drawn in.
    assert!(
        replay.starts_with("\x1b[?1049h"),
        "restored into the alternate screen: {:?}",
        &replay[..20.min(replay.len())]
    );
    drop(std::fs::File::from(taken.descriptor));
}

/// A handed-over screen is kept at the width it was drawn at.
///
/// A window spawns a pane before it has laid it out, so the size the multiplexer
/// is told is a stand-in — 80x24 — and on Unix the resize that follows goes
/// straight to the descriptor that window holds. The multiplexer therefore knows
/// nothing about the real geometry until it is asked, and a 98x51 screen seeded
/// into an 80x24 grid comes back wrapped and interleaved: the scrambled `htop` a
/// second window joined to.
#[test]
fn a_handed_over_screen_keeps_the_width_it_was_drawn_at() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let subscription = client.subscribe().unwrap();
    let _reporters = subscription.exits.clone();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 120"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // The window lays the pane out and resizes the terminal itself, which is what
    // a client on Unix does: the multiplexer is not told.
    let resized = libc::winsize {
        ws_row: 51,
        ws_col: 98,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::ioctl(
                std::os::fd::AsRawFd::as_raw_fd(&descriptor),
                libc::TIOCSWINSZ,
                &raw const resized,
            )
        },
        0,
        "resizing the terminal the way a window does"
    );

    // A screen as wide as the window: one line of ninety columns, and a marker on
    // a row an 80x24 grid could not hold.
    let wide = "W".repeat(90);
    let mut screen = format!("\x1b[?1049h\x1b[H{wide}");
    for row in 2..=40 {
        screen.push_str(&format!("\x1b[{row};1Hrow {row}"));
    }
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, screen.into_bytes())],
        )
        .unwrap();

    let taken = match client
        .attach(pane.session_id, Some(pane.pane_id), None)
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => pane,
        _ => panic!("its own window must attach it"),
    };
    let replay = String::from_utf8_lossy(&taken.replay).into_owned();
    assert!(
        replay.contains(&wide),
        "a ninety-column line must come back unwrapped: {replay:?}"
    );
    assert!(
        replay.contains("row 40"),
        "and a row past the twenty-fourth must come back at all: {replay:?}"
    );
    drop(std::fs::File::from(taken.descriptor));
}

/// `--upgrade` is usable across a protocol boundary, not merely accepted at one.
///
/// The daemon exempts `Request::Upgrade` from its version check, and the command
/// still failed: the *client* refused to connect to a multiplexer whose version
/// it disagreed with, so the one command that exists to cross that boundary
/// reported the boundary instead of crossing it. Rebuilding Zetta creates exactly
/// this situation, which is when the command matters.
#[test]
fn an_upgrade_connects_to_a_multiplexer_of_another_protocol_version() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let child_pid = pane.child_pid;
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    drop(descriptor);

    // What a rebuild leaves behind: a running multiplexer advertising a protocol
    // this build does not speak.
    let endpoint_path = daemon.sessions_dir().join("zmux.json");
    let mut endpoint: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&endpoint_path).unwrap()).unwrap();
    endpoint["protocol_version"] = serde_json::json!(zmux::messages::PROTOCOL_VERSION - 1);
    std::fs::write(&endpoint_path, serde_json::to_vec(&endpoint).unwrap()).unwrap();

    // Every other command says so, and says what to do about it.
    let error = match Client::connect_existing_at(&daemon.sessions_dir()) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("an ordinary connection must refuse the version"),
    };
    assert!(
        error.contains(&format!(
            "protocol version {}",
            zmux::messages::PROTOCOL_VERSION - 1
        )),
        "{error}"
    );
    assert!(error.contains("--upgrade"), "{error}");

    // The upgrade goes through, and the session it was holding survives it.
    let upgrading = Client::connect_for_upgrade_at(&daemon.sessions_dir())
        .expect("connecting across the version boundary")
        .expect("the multiplexer is running");
    upgrading.upgrade().expect("replacing the multiplexer");

    let client = wait_for_multiplexer(&daemon);
    assert!(
        process_is_alive(child_pid),
        "the session's process was ended by the upgrade"
    );
    assert_eq!(
        client.list().unwrap().len(),
        1,
        "the session was dropped; log:\n{}",
        daemon.log()
    );
}

/// A session protected with a key sealed to the user's recipients is attached by
/// opening that key, with nothing typed. The daemon is unchanged by this: it
/// checks an Argon2id verifier exactly as it does for a secret someone chose.
#[cfg(feature = "session-persistence")]
#[test]
fn an_automatically_protected_session_is_attached_by_opening_its_sealed_key() {
    let identity = age::x25519::Identity::generate();
    let recipients =
        zmux::persistence::RecipientSet::parse(&[identity.to_public().to_string()]).unwrap();
    let daemon = TestDaemon::start();
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
            serde_json::json!({"sealed": true}),
            Some(&sealed.authentication),
            Vec::new(),
        )
        .unwrap();

    // The envelope is published, because it is public ciphertext and the party
    // it helps is the one holding the private key.
    let listed = client
        .list()
        .unwrap()
        .into_iter()
        .find(|session| session.id == pane.session_id)
        .expect("the detached session is listed");
    assert!(listed.authentication_required);
    let envelope = listed
        .key_envelope
        .expect("an automatically protected session publishes its sealed key");

    // Nothing typed: the key comes out of the envelope with the identity.
    let identity_path = daemon.config.join("identity.txt");
    std::fs::write(
        &identity_path,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .unwrap();
    let identities =
        zmux::persistence::IdentitySet::from_paths(std::slice::from_ref(&identity_path)).unwrap();
    let secret = zmux::auto_protect::open(&envelope, &identities).unwrap();

    assert!(
        matches!(
            client
                .attach(
                    pane.session_id,
                    Some(pane.pane_id),
                    Some(secret.expose().to_owned())
                )
                .unwrap(),
            AttachOutcome::Attached { .. }
        ),
        "the recovered key should attach the session"
    );
}

/// Without the sealed key the session is exactly as closed as any other
/// protected one — the envelope being published does not weaken it.
#[cfg(feature = "session-persistence")]
#[test]
fn a_published_envelope_does_not_let_an_unopened_session_be_attached() {
    let identity = age::x25519::Identity::generate();
    let recipients =
        zmux::persistence::RecipientSet::parse(&[identity.to_public().to_string()]).unwrap();
    let daemon = TestDaemon::start();
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
            serde_json::json!({}),
            Some(&sealed.authentication),
            Vec::new(),
        )
        .unwrap();

    let listed = client
        .list()
        .unwrap()
        .into_iter()
        .find(|session| session.id == pane.session_id)
        .unwrap();
    let envelope = listed.key_envelope.unwrap();

    // A stranger holds the envelope but not the key it was sealed to.
    let other = age::x25519::Identity::generate();
    let stranger_path = daemon.config.join("stranger.txt");
    std::fs::write(
        &stranger_path,
        format!("{}\n", other.to_string().expose_secret()),
    )
    .unwrap();
    let strangers =
        zmux::persistence::IdentitySet::from_paths(std::slice::from_ref(&stranger_path)).unwrap();
    assert!(zmux::auto_protect::open(&envelope, &strangers).is_err());

    // And the envelope itself is not a secret the daemon will take.
    assert!(
        matches!(
            client
                .attach(pane.session_id, Some(pane.pane_id), Some(envelope))
                .unwrap(),
            AttachOutcome::AuthenticationFailed
        ),
        "the envelope must not authenticate"
    );
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

/// The deadlock automatic protection created: a protected session whose window
/// has exited could not be reached at all.
///
/// Attaching it refuses because it is still scoped to that window and says to
/// share it — which is what `attach` tells the user to do — and sharing refused
/// in turn because the owner it named was gone. Before every backgrounded
/// session was protected this was rare; afterwards it was the ordinary outcome
/// of closing a window, and the session and its processes were stranded.
#[test]
fn a_protected_session_whose_window_exited_can_still_be_offered() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    // A real process, so the daemon's liveness check is exercised rather than a
    // pid that never existed; it is reaped before the session is shared.
    let owner = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let owner_pid = owner.id();
    let pane = client
        .spawn(spawn_request_as(None, "printf ready; sleep 120", owner_pid))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            Some(&test_verifier()),
            owner_pid,
        )
        .unwrap();

    // While that window is alive its scope is its own business, and a stranger
    // must not override it.
    let refused = client
        .set_session_scope(pane.session_id, true, None)
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("protected"),
        "a live owner's scope must stand: {refused}"
    );

    reap(owner);

    // With the window gone, offering it is the documented way out and now works.
    client
        .set_session_scope(pane.session_id, true, None)
        .unwrap();

    // Offering changed no secret: the session is still gated by the one it had.
    assert!(
        matches!(
            client
                .attach(pane.session_id, Some(pane.pane_id), None)
                .unwrap(),
            AttachOutcome::AuthenticationRequired
        ),
        "sharing a stranded session must not unprotect it"
    );
    assert!(
        matches!(
            client
                .attach(
                    pane.session_id,
                    Some(pane.pane_id),
                    Some(TEST_SECRET.to_owned())
                )
                .unwrap(),
            AttachOutcome::Attached { .. }
        ),
        "the session's own secret still opens it"
    );
}

/// The half that stays with the owner: offering a stranded session may not carry
/// a new verifier, or anyone could seize a protected session by replacing the
/// secret that gates it and then attaching with their own.
#[test]
fn a_stranded_session_cannot_be_seized_by_replacing_its_verifier() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let owner = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let owner_pid = owner.id();
    let pane = client
        .spawn(spawn_request_as(None, "printf ready; sleep 120", owner_pid))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    drop(descriptor);
    client
        .detach_as(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            Some(&test_verifier()),
            owner_pid,
        )
        .unwrap();
    reap(owner);

    let seized = zmux::auth::SessionAuthentication::create("attacker's secret").unwrap();
    let refused = client
        .set_session_scope(pane.session_id, true, Some(&seized))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("protected"),
        "replacing the verifier is not part of the escape hatch: {refused}"
    );

    // And the original secret is untouched.
    client
        .set_session_scope(pane.session_id, true, None)
        .unwrap();
    assert!(matches!(
        client
            .attach(
                pane.session_id,
                Some(pane.pane_id),
                Some("attacker's secret".to_owned())
            )
            .unwrap(),
        AttachOutcome::AuthenticationFailed
    ));
}

/// A pane's remembered size is a stand-in on Unix: a client resizes through the
/// descriptor it holds and tells the multiplexer nothing, so only the pty knows
/// the real geometry. `--upgrade` recorded the stand-in and rebuilt the retained
/// screen from it in the next image, re-wrapping a full-screen program's screen
/// at a width it was never drawn at — htop came back as fragments of itself.
#[cfg(all(unix, feature = "scrollback-buffer"))]
#[test]
fn an_upgrade_keeps_the_retained_screen_at_the_width_it_was_drawn_at() {
    use std::os::fd::AsRawFd as _;

    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");

    // What a window does after its first layout, and what the multiplexer never
    // hears about: the spawn asked for the stand-in geometry, this is the real
    // one.
    let size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe { libc::ioctl(descriptor.as_raw_fd(), libc::TIOCSWINSZ, &size) },
        0,
        "resizing the pty"
    );

    // Wider than the stand-in, so a grid rebuilt at the wrong width has to wrap
    // it and the damage is visible in the replay.
    let line = "W".repeat(100);
    drop(descriptor);
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            vec![(pane.pane_id, line.clone().into_bytes())],
        )
        .unwrap();

    client.upgrade().unwrap();
    // macOS rebinds the listener after exec so its peer-credential state is
    // rebuilt; wait for an answered request rather than only a successful
    // connect during that short gap.
    let client = wait_for_multiplexer(&daemon);

    match client
        .attach(
            pane.session_id,
            Some(pane.pane_id),
            Some(TEST_SECRET.to_owned()),
        )
        .unwrap()
    {
        AttachOutcome::Attached { pane, .. } => {
            let replay = String::from_utf8_lossy(&pane.replay).into_owned();
            assert!(
                replay.contains(&line),
                "the retained screen was re-wrapped by the upgrade: {replay:?}"
            );
        }
        _ => panic!("attach failed"),
    }
}
