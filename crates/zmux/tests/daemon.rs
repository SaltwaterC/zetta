//! End-to-end tests against a real daemon process.
//!
//! Each test gets its own configuration directory, so the daemon it starts is
//! its own and cannot find, or be found by, the developer's running sessions.

#![cfg(unix)]

#[path = "daemon/exclusive_panes.rs"]
mod exclusive_panes;

#[path = "daemon/persistence.rs"]
mod persistence;

#[path = "daemon/sessions.rs"]
mod sessions;

#[path = "daemon/upgrade.rs"]
mod upgrade;

#[path = "daemon/sharing.rs"]
mod sharing;

#[path = "daemon/shared_layout.rs"]
mod shared_layout;

#[path = "daemon/shared_input.rs"]
mod shared_input;

#[path = "daemon/handover.rs"]
mod handover;

#[path = "daemon/protection.rs"]
mod protection;

#[path = "daemon/relay.rs"]
mod relay;

#[path = "daemon/viewer_requests.rs"]
mod viewer_requests;

use std::{
    collections::HashMap,
    io::{Read, Write as _},
    net::Shutdown,
    os::{
        fd::{AsRawFd as _, FromRawFd as _},
        unix::{
            net::{UnixListener, UnixStream},
            process::CommandExt as _,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use zetta_sync::RecoverPoison as _;

use alacritty_terminal::tty::ConsolePalette;

#[cfg(feature = "session-persistence")]
use age::secrecy::ExposeSecret as _;

#[cfg(feature = "session-persistence")]
use zmux::persistence::{PersistedSession, PersistedSnapshot, PersistenceStore};

use zmux::{
    auth::SessionSecret,
    client::{AttachOutcome, Client},
    messages::{
        ClientId, CreateSharedRequest, Envelope, Event, Request, Response, SessionRevision,
        SharedDraftLayout, SharedPaneDirection, SharedPaneRef, SharedRotationDirection,
        SharedSessionOperation, SharedSpawnBatchRequest, SharedSpawnRequest, SpawnRequest,
        TerminalSize,
    },
    protocol::{
        BackgroundPaneLayout, BackgroundPaneState, BackgroundPaneSummary, BackgroundSessionSummary,
    },
    retention::Retention,
    transport::{Connection, Stream},
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
        Self::start_in(extra, None)
    }

    /// A daemon whose own working directory is `working_directory`, which is
    /// how a detached one starts: a window that has since closed, a login
    /// item, `zmux --daemon` run from anywhere. On macOS that is `/`.
    fn start_in(extra: &[&str], working_directory: Option<&Path>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().to_path_buf();
        let mut command = Command::new(daemon_binary());
        if let Some(working_directory) = working_directory {
            command.current_dir(working_directory);
        }
        let process = command
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

    /// A client of the image that replaced this daemon after `upgrade`.
    fn wait_for_replacement(&self) -> Client {
        // Waiting for a connection to succeed is not enough: until the
        // replacement has rebound, a connection can still land in the old
        // listener's backlog and be reset when the exec tears it down.
        // Readiness is a request that answers, not a socket that accepts.
        //
        // Generous, because an upgrade is several steps — a pre-flight
        // subprocess, the exec, re-adopting the sessions, rebinding — and these
        // tests are about whether they happen at all, not how quickly. A machine
        // busy compiling was enough to make a ten-second bound report a failure
        // that was not one.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(Some(connected)) = Client::connect_ready_at(&self.sessions_dir())
                && connected.list().is_ok()
            {
                return connected;
            }
            assert!(
                Instant::now() < deadline,
                "the replacement never came back (process: {}); daemon log:\n{}",
                self.process.id(),
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
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
        console_backend: Default::default(),
    }
}

fn summary(session_id: u64, pane_id: u64) -> BackgroundSessionSummary {
    BackgroundSessionSummary {
        id: session_id,
        title: "test".to_owned(),
        authentication_required: false,
        active_pane: pane_id,
        layout: BackgroundPaneLayout::Pane { pane_id },
        panes: vec![pane_summary(pane_id)],
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

fn pane_summary(pane_id: u64) -> BackgroundPaneSummary {
    BackgroundPaneSummary {
        id: pane_id,
        label: format!("pane-{pane_id}"),
        profile: String::new(),
        configured_command: String::new(),
        application: "/bin/sh".to_owned(),
        foreground_command: None,
        terminal_title: None,
        working_directory: None,
        state: BackgroundPaneState::Running,
        exit: None,
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

/// The secret every shared session in these tests is protected with.
///
/// Sharing one is what makes it joinable from another process, so the
/// multiplexer refuses to offer a session that has no secret: joining a session
/// is being handed whatever its terminals can already do.
const TEST_SECRET: &str = "test-secret";

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
    // SAFETY: signal 0 delivers nothing, and `kill` takes no pointers.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// A headless shared session of one pane running `command`.
fn one_pane_shared_request(client: &Client, command: &str) -> CreateSharedRequest {
    CreateSharedRequest {
        operation_id: client.next_shared_operation_id(),
        title: "one pane".to_owned(),
        replacement: SharedDraftLayout::Draft { draft_id: 1 },
        panes: vec![zmux::messages::SharedPaneDraft {
            draft_id: 1,
            profile: "System".to_owned(),
            command: Some(zetta_profiles::ProfileCommand::with_args(
                "sh",
                vec!["-c".to_owned(), command.to_owned()],
            )),
            env: HashMap::new(),
            working_directory: None,
            inherit_working_directory_from: None,
            load_shell_integration: false,
            size: TerminalSize {
                columns: 80,
                lines: 24,
                cell_width: 0,
                cell_height: 0,
            },
            console_palette: ConsolePalette::default(),
            metadata: BackgroundPaneSummary {
                id: 0,
                label: "pane".to_owned(),
                profile: "System".to_owned(),
                configured_command: String::new(),
                application: "sh".to_owned(),
                foreground_command: None,
                terminal_title: None,
                working_directory: None,
                state: BackgroundPaneState::Starting,
                exit: None,
            },
        }],
        active_pane: Some(SharedPaneRef::Draft { draft_id: 1 }),
        verifier: None,
    }
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

/// Reads a shared stream until its reader has recorded the requested size.
/// Size events are intentionally not bytes, so the ordinary output helper
/// cannot observe them without losing the assertion's useful history.
fn wait_for_shared_size(
    reader: &mut impl Read,
    pane: &zmux::client::SharedPane,
    expected: (u16, u16),
) -> Vec<(u16, u16)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut sizes = Vec::new();
    let mut buffer = [0; 4096];
    while Instant::now() < deadline {
        match reader.read(&mut buffer) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("reading the shared pane failed: {error}"),
        }
        let new_sizes = pane.take_revisioned_sizes();
        sizes.extend(
            new_sizes
                .iter()
                .map(|(_, columns, lines)| (*columns, *lines)),
        );
        for (revision, columns, lines) in &new_sizes {
            pane.finish_size_application((*revision, *columns, *lines));
        }
        if sizes.contains(&expected) {
            return sizes;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never saw shared size {expected:?}; read {sizes:?}");
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

fn winsize(columns: u16, lines: u16) -> libc::winsize {
    libc::winsize {
        ws_row: lines,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn collect_output(mut output: impl Read + Send + 'static) -> Arc<Mutex<String>> {
    let collected = Arc::new(Mutex::new(String::new()));
    let thread_collected = Arc::clone(&collected);
    std::thread::spawn(move || {
        let mut bytes = [0_u8; 4096];
        loop {
            match output.read(&mut bytes) {
                Ok(0) => return,
                Ok(read) => thread_collected
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&bytes[..read])),
                // A shared pane's reader reports its own read timeout this
                // way, which is "nothing yet" rather than an end.
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    collected
}
