//! A remote pane carried over Mosh, end to end, without the remote.
//!
//! Everything between the multiplexer and the window is exercised here: a live
//! `zmux` daemon holding a shared pane, the `zmux relay-pane` that copies it to
//! its own stdio, a real `zosh-server` emulating that terminal, and the
//! headless [`zosh::PaneSession`] Zetta builds a pane's byte stream from. Only
//! the SSH bootstrap is left out — that is `zosh`'s own, unchanged, and it
//! needs a host to log in to.
//!
//! It drives separately built executables, so build them first:
//!
//! ```sh
//! cargo build --bin zmux --bin zosh-server
//! cargo test --test zosh_pane -- --ignored
//! ```
//!
//! Ignored by default for that reason, in the same way `crates/zosh`'s interop
//! tests are.

#![cfg(all(unix, feature = "zmux", feature = "zosh-client"))]

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use zmux::{
    auth::SessionAuthentication,
    client::Client,
    messages::{SessionRevision, SharedSpawnRequest, SpawnRequest, TerminalSize},
    protocol::{BackgroundPaneLayout, BackgroundPaneSummary, BackgroundSessionSummary},
};

const TEST_SECRET: &str = "relayed-pane-secret";

/// The whole path a Zosh pane's bytes take: the multiplexer's shared stream,
/// the relay's stdio, the Mosh server's emulator, the link, and the session
/// this side renders it with. Input goes back the same way.
#[test]
#[ignore = "drives separately built zmux and zosh-server binaries; see the module docs"]
fn a_pane_carried_over_mosh_shows_its_output_and_takes_its_input() {
    let daemon = TestDaemon::start();
    let client = daemon.client();

    // A session with one pane, shared and protected, which is the shape a
    // remote session has by the time Zetta attaches it.
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .expect("spawning the session's first pane");
    let mut offered = summary(pane.session_id, pane.pane_id);
    offered.panes.push(pane_summary(pane.pane_id));
    client
        .share(
            pane.session_id,
            offered,
            serde_json::Value::Null,
            Some(&SessionAuthentication::create(TEST_SECRET).expect("a verifier")),
            true,
        )
        .expect("sharing the session");
    // The pane reports the size its own pty is at whenever it is sent a line,
    // which is how the resize below is checked all the way through rather than
    // only as far as the frame it produces.
    let request = spawn_request(
        Some(pane.session_id),
        concat!(
            "printf 'relayed-pane-is-live\n'; ",
            "while IFS= read -r line; do stty size; ",
            // The marker Zetta's shell integration reports a directory with,
            // built from what was typed so the echo of the input cannot be
            // mistaken for the title itself.
            r#"printf '\033]2;zetta-cwd:%s\033\\' "$line"; done"#,
        ),
    );
    let spawned = client
        .spawn_shared(SharedSpawnRequest {
            session_id: pane.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: client.next_shared_operation_id(),
            program: request.program,
            args: request.args,
            env: request.env,
            working_directory: request.working_directory,
            size: request.size,
            console_palette: request.console_palette,
        })
        .expect("spawning the pane to relay");
    let relayed_pane = spawned.pane.pane_id();
    // Nothing in this process reads the multiplexer's own stream for that pane
    // any more; the relay is its viewer, exactly as it is for Zetta.
    drop(spawned);

    let mut server = MoshServer::start(
        &daemon.config,
        &[
            &binary("zmux").to_string_lossy(),
            "relay-pane",
            &pane.session_id.to_string(),
            &relayed_pane.to_string(),
            "--secret-stdin",
        ],
    );

    let mut session = zosh::PaneSession::connect(
        "127.0.0.1",
        server.port,
        &server.key,
        80,
        24,
        zosh::PaneSessionSettings {
            keep_alive: Some(250),
            ..zosh::PaneSessionSettings::default()
        },
    )
    .expect("connecting to the Mosh endpoint");
    let frames = Frames::collect(session.take_reader().expect("the session's reader"));

    // The secret goes in first and in band, which is the only way the relay
    // ever learns it.
    let mut writer = session.writer();
    use std::io::Write as _;
    writeln!(writer, "{TEST_SECRET}").expect("sending the session secret");

    let screen = frames.wait_for("relayed-pane-is-live");
    assert!(
        screen.contains("relayed-pane-is-live"),
        "the pane's output has to reach the session: {screen:?}"
    );

    // Input reaches the pane's terminal: the line is echoed by its pty, and
    // the program behind it answers with the size that pty is at.
    write!(writer, "typed-into-the-pane\r").expect("sending pane input");
    let echoed = frames.wait_for("typed-into-the-pane");
    assert!(
        echoed.contains("typed-into-the-pane"),
        "input has to reach the pane's terminal: {echoed:?}"
    );
    frames.wait_for("24 80");

    // A resize has to survive everything between here and the pane: the
    // session sizes the Mosh server's pty, the relay's SIGWINCH reports that
    // size to the multiplexer, and the multiplexer resizes the pane itself.
    session.resize(100, 30);
    // Asked repeatedly rather than once: the size has four hops to make, and
    // a question that arrives before it has made them is answered with the
    // old one.
    let resized = frames.wait_for_answer(&mut writer, "30 100");
    assert!(
        resized.contains("30 100"),
        "the pane's own pty has to follow the window's size: {resized:?}"
    );

    // Zetta learns a pane's working directory from a `zetta-cwd:` window
    // title its shell integration reports. Mosh carries a title, so the marker
    // survives the crossing — and it arrives unprefixed, which is the part
    // that matters: a `[mosh] ` prefix like the standalone client's would make
    // every directory report unreadable.
    // The line typed here is what the pane puts in the marker, so the echo of
    // the input cannot be mistaken for the title the pane reported.
    write!(writer, "/tmp/zosh-pane\r").expect("asking the pane to report a directory");
    let reported = frames.wait_for("zetta-cwd:/tmp/zosh-pane");
    let marker = reported
        .find("zetta-cwd:/tmp/zosh-pane")
        .expect("the reported directory");
    assert!(
        reported[..marker].ends_with("\u{1b}]0;") || reported[..marker].ends_with("\u{1b}]2;"),
        "the directory has to arrive as a title, and unprefixed: {:?}",
        &reported[marker.saturating_sub(16)..marker]
    );

    // A pane added to the session afterwards — which is what a split here or
    // in any other viewer's window amounts to — gets a link of its own, live
    // at the same time as the first. Mosh carries one terminal, so this is the
    // only way a tab can have more than one pane on it.
    let mut second = spawn_relayed_pane(
        &client,
        pane.session_id,
        "printf second-pane-is-live; cat",
        &daemon.config,
    );
    let second_frames = Frames::collect(
        second
            .session
            .take_reader()
            .expect("the second session's reader"),
    );
    second_frames.wait_for("second-pane-is-live");
    frames.wait_for_answer(&mut writer, "30 100");

    drop(second);
    drop(session);
    server.stop();
}

/// A pane added to a session that is already open, carried over a link of its
/// own — the shape a split takes, from whichever side asks for it.
struct RelayedPane {
    session: zosh::PaneSession,
    _server: MoshServer,
}

fn spawn_relayed_pane(
    client: &Client,
    session_id: u64,
    command: &str,
    config: &Path,
) -> RelayedPane {
    let request = spawn_request(Some(session_id), command);
    let spawned = client
        .spawn_shared(SharedSpawnRequest {
            session_id,
            base_revision: client
                .shared_snapshot(session_id)
                .expect("the session's current revision")
                .revision,
            operation_id: client.next_shared_operation_id(),
            program: request.program,
            args: request.args,
            env: request.env,
            working_directory: request.working_directory,
            size: request.size,
            console_palette: request.console_palette,
        })
        .expect("spawning another pane in the session");
    let mux_pane_id = spawned.pane.pane_id();
    drop(spawned);

    let server = MoshServer::start(
        config,
        &[
            &binary("zmux").to_string_lossy(),
            "relay-pane",
            &session_id.to_string(),
            &mux_pane_id.to_string(),
            "--secret-stdin",
        ],
    );
    let session = zosh::PaneSession::connect(
        "127.0.0.1",
        server.port,
        &server.key,
        80,
        24,
        zosh::PaneSessionSettings::default(),
    )
    .expect("connecting the added pane's Mosh endpoint");
    let mut writer = session.writer();
    use std::io::Write as _;
    writeln!(writer, "{TEST_SECRET}").expect("sending the session secret");
    RelayedPane {
        session,
        _server: server,
    }
}

/// The frames the session has painted so far.
///
/// Collected on a thread of its own because the session's reader blocks: a
/// deadline around a blocking read is not a deadline, and a test that hangs
/// says nothing about what went wrong.
struct Frames {
    seen: Arc<Mutex<String>>,
}

impl Frames {
    fn collect(mut reader: impl Read + Send + 'static) -> Self {
        let seen = Arc::new(Mutex::new(String::new()));
        let collected = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut bytes = [0_u8; 8192];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) | Err(_) => return,
                    Ok(read) => collected
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&bytes[..read])),
                }
            }
        });
        Self { seen }
    }

    /// Keeps asking the pane a question until its answer contains `expected`.
    ///
    /// The question is a bare newline, which the program in the pane answers
    /// with the size of its own pty.
    fn wait_for_answer(&self, writer: &mut impl std::io::Write, expected: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            write!(writer, "\r").expect("asking the pane");
            std::thread::sleep(Duration::from_millis(250));
            let seen = self.seen.lock().unwrap().clone();
            if seen.contains(expected) {
                return seen;
            }
            assert!(
                Instant::now() < deadline,
                "never saw {expected:?}; painted {seen:?}"
            );
        }
    }

    /// Waits for `expected` to appear in what has been painted. Mosh paints a
    /// screen rather than a byte stream, so the text arrives surrounded by
    /// cursor movement and attribute sequences.
    fn wait_for(&self, expected: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let seen = self.seen.lock().unwrap().clone();
            if seen.contains(expected) {
                return seen;
            }
            assert!(
                Instant::now() < deadline,
                "never saw {expected:?}; painted {seen:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A `zosh-server` running one command on a pty of its own, which is what the
/// SSH bootstrap would have started on the remote host.
struct MoshServer {
    process: Child,
    port: u16,
    key: zosh::Base64Key,
}

impl MoshServer {
    fn start(config: &Path, command: &[&str]) -> Self {
        let mut arguments = vec!["new", "-i", "127.0.0.1", "-c", "256", "--foreground", "--"];
        arguments.extend_from_slice(command);
        let mut process = Command::new(binary("zosh-server"))
            .args(&arguments)
            .env("XDG_CONFIG_HOME", config)
            .env("LANG", "en_US.UTF-8")
            .env("MOSH_SERVER_NETWORK_TMOUT", "60")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("starting zosh-server");
        let stdout = process.stdout.take().expect("the server's stdout");
        let connect = read_connect_line(stdout);
        let mut fields = connect.split_whitespace();
        let port = fields
            .next()
            .and_then(|port| port.parse().ok())
            .expect("the server's UDP port");
        let key = zosh::Base64Key::from_printable(fields.next().expect("the session key"))
            .expect("a valid session key");
        Self { process, port, key }
    }

    fn stop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Drop for MoshServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_connect_line(stdout: impl Read) -> String {
    let mut reader = std::io::BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut line = String::new();
    while Instant::now() < deadline {
        use std::io::BufRead as _;
        line.clear();
        let read = reader.read_line(&mut line).expect("reading the server");
        assert!(read != 0, "zosh-server ended without a MOSH CONNECT line");
        if let Some(connect) = line.trim().strip_prefix("MOSH CONNECT ") {
            return connect.to_owned();
        }
    }
    panic!("zosh-server printed no MOSH CONNECT line");
}

/// A multiplexer of this build, in a configuration directory of its own.
struct TestDaemon {
    process: Child,
    _directory: tempfile::TempDir,
    config: PathBuf,
}

impl TestDaemon {
    fn start() -> Self {
        let directory = tempfile::tempdir().expect("a temporary configuration directory");
        let config = directory.path().to_path_buf();
        let process = Command::new(binary("zmux"))
            .arg("--daemon")
            .env("XDG_CONFIG_HOME", &config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("starting zmux");
        let daemon = Self {
            process,
            _directory: directory,
            config,
        };
        let _ = daemon.client();
        daemon
    }

    fn client(&self) -> Client {
        let sessions = self.sessions_dir();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(Some(client)) = Client::connect_ready_at(&sessions) {
                return client;
            }
            assert!(
                Instant::now() < deadline,
                "the multiplexer was not ready within 20s"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn sessions_dir(&self) -> PathBuf {
        let name = if cfg!(debug_assertions) {
            format!("sessions-debug-v{}", zmux::messages::PROTOCOL_VERSION)
        } else {
            "sessions".to_owned()
        };
        self.config.join("zetta").join(name)
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// A binary of this build, beside the test executable. These tests drive
/// separately built executables, so a stale one would quietly test the
/// previous implementation.
fn binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("the test executable's path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let binary = path.join(name);
    assert!(
        binary.is_file(),
        "{} is missing; run `cargo build --bin zmux --bin zosh-server` first",
        binary.display()
    );
    binary
}

fn spawn_request(session_id: Option<u64>, command: &str) -> SpawnRequest {
    let mut env = std::collections::HashMap::new();
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
        title: "relayed".to_owned(),
        authentication_required: true,
        active_pane: pane_id,
        layout: BackgroundPaneLayout::Pane { pane_id },
        panes: Vec::new(),
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

fn pane_summary(pane_id: u64) -> BackgroundPaneSummary {
    BackgroundPaneSummary {
        id: pane_id,
        label: "relayed".to_owned(),
        profile: "relayed".to_owned(),
        configured_command: "/bin/sh".to_owned(),
        application: "sh".to_owned(),
        foreground_command: None,
        terminal_title: None,
        working_directory: None,
        state: zmux::protocol::BackgroundPaneState::Running,
        exit: None,
    }
}
