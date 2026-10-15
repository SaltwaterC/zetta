//! End-to-end tests of what keeps the daemon singular and bounded: the
//! lifetime lock that serializes startup, and the aggregate limits on
//! connections, request deadlines and snapshot bytes.
//!
//! Kept apart from `daemon.rs` because these start several daemons in one
//! directory, or deliberately exhaust one, rather than drive a session. Each
//! test has its own configuration directory, so nothing here can reach the
//! developer's running multiplexer.

#![cfg(unix)]

use std::{
    io::Write as _,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use zmux::{
    client::Client,
    messages::{ClientId, DetachRequest, Envelope, PaneSnapshot, Request, Response},
    protocol::{BackgroundPaneLayout, BackgroundSessionSummary},
    transport::{Connection, Endpoint, Stream},
};

fn daemon_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let binary = path.join("zmux");
    assert!(
        binary.is_file(),
        "{} is missing; run `cargo build --bin zmux` first",
        binary.display()
    );
    // A stale binary would test the previous implementation; see `daemon.rs`.
    let built = std::fs::metadata(&binary).unwrap().modified().unwrap();
    for source in [
        "src/server.rs",
        "src/server/lifetime_lock.rs",
        "src/server/ipc_limits.rs",
    ] {
        let edited = std::fs::metadata(Path::new(env!("CARGO_MANIFEST_DIR")).join(source))
            .unwrap()
            .modified()
            .unwrap();
        assert!(
            built >= edited,
            "{} is older than {source}; run `cargo build --bin zmux` first",
            binary.display()
        );
    }
    binary
}

fn sessions_dir(config: &Path) -> PathBuf {
    let name = if cfg!(debug_assertions) {
        format!("sessions-debug-v{}", zmux::messages::PROTOCOL_VERSION)
    } else {
        "sessions".to_owned()
    };
    config.join("zetta").join(name)
}

/// Daemons started in one configuration directory, all killed when dropped.
struct Daemons {
    config: tempfile::TempDir,
    processes: Vec<Child>,
}

impl Daemons {
    fn new() -> Self {
        Self {
            config: tempfile::tempdir().unwrap(),
            processes: Vec::new(),
        }
    }

    fn sessions(&self) -> PathBuf {
        sessions_dir(self.config.path())
    }

    fn spawn(&mut self) -> usize {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.config.path().join("zmux.log"))
            .unwrap();
        let process = Command::new(daemon_binary())
            .arg("--daemon")
            .env("XDG_CONFIG_HOME", self.config.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("starting zmux");
        self.processes.push(process);
        self.processes.len() - 1
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.config.path().join("zmux.log")).unwrap_or_default()
    }

    fn wait_for_ready(&self) -> Client {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(Some(client)) = Client::connect_ready_at(&self.sessions()) {
                return client;
            }
            assert!(
                Instant::now() < deadline,
                "no daemon became ready\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint::read(&self.sessions().join("zmux.json")).unwrap()
    }

    /// Which of the started daemons are still running.
    fn running(&mut self) -> Vec<u32> {
        self.processes
            .iter_mut()
            .filter_map(|process| process.try_wait().unwrap().is_none().then_some(&*process))
            .map(|process| process.id())
            .collect()
    }

    /// Waits for every daemon but `survivors` to exit.
    fn settle_to(&mut self, survivors: usize) -> Vec<u32> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let running = self.running();
            if running.len() <= survivors || Instant::now() >= deadline {
                return running;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Daemons {
    fn drop(&mut self) {
        for process in &mut self.processes {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

fn answers(socket: &Path) -> bool {
    Stream::connect(socket).is_ok()
}

#[test]
fn simultaneous_starts_leave_exactly_one_daemon_that_answers() {
    let mut daemons = Daemons::new();
    for _ in 0..8 {
        daemons.spawn();
    }
    // Ready means a ping was answered.
    daemons.wait_for_ready();

    let running = daemons.settle_to(1);
    assert_eq!(
        running.len(),
        1,
        "more than one daemon is serving the directory: {running:?}\n{}",
        daemons.log()
    );
    let endpoint = daemons.endpoint();
    assert_eq!(endpoint.process_id, running[0]);
    assert!(answers(&endpoint.socket_path));
    // And every client finds that one: a fresh client reaches the same daemon.
    let client = Client::connect_ready_at(&daemons.sessions())
        .unwrap()
        .expect("the surviving daemon answers");
    assert_eq!(client.process_id(), running[0]);
}

#[test]
fn a_second_start_never_removes_a_live_daemons_socket() {
    let mut daemons = Daemons::new();
    let first = daemons.spawn();
    daemons.wait_for_ready();
    let endpoint = daemons.endpoint();
    // Without its endpoint the live daemon is invisible to a check that reads
    // the endpoint first, which is exactly the window a concurrent start used
    // to fall into before the first daemon had published.
    std::fs::remove_file(daemons.sessions().join("zmux.json")).unwrap();

    let second = daemons.spawn();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = daemons.processes[second].try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the second daemon kept running");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success());
    assert!(daemons.processes[first].try_wait().unwrap().is_none());
    assert!(
        answers(&endpoint.socket_path),
        "the live daemon's socket was removed\n{}",
        daemons.log()
    );
}

#[test]
fn a_stale_socket_left_by_a_dead_daemon_is_recovered() {
    let mut daemons = Daemons::new();
    let first = daemons.spawn();
    daemons.wait_for_ready();
    let socket = daemons.endpoint().socket_path;
    daemons.processes[first].kill().unwrap();
    daemons.processes[first].wait().unwrap();
    assert!(socket.exists() && !answers(&socket));

    let second = daemons.spawn();
    daemons.wait_for_ready();
    assert_eq!(
        daemons.endpoint().process_id,
        daemons.processes[second].id()
    );
}

/// An upgrade replaces the image in place, and the lock goes with it: the
/// descriptor survives the `execv` and the replacement adopts it, so it is
/// never free for a starting daemon to take in between.
#[test]
fn an_upgrade_carries_the_lock_across_the_exec() {
    let mut daemons = Daemons::new();
    let first = daemons.spawn();
    daemons.wait_for_ready().upgrade().unwrap();
    daemons.wait_for_ready();
    assert_eq!(daemons.endpoint().process_id, daemons.processes[first].id());
    // Adopted, not taken again: the replacement says when it had to.
    let log = std::fs::read_to_string(daemons.sessions().join("daemon.log")).unwrap_or_default();
    assert!(
        !log.contains("multiplexer lock"),
        "the replacement did not take over the lock:\n{log}"
    );

    let second = daemons.spawn();
    let deadline = Instant::now() + Duration::from_secs(10);
    while daemons.processes[second].try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "a second daemon started beside the upgraded one"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(answers(&daemons.endpoint().socket_path));
}

fn raw_request(endpoint: &Endpoint, request: Request) -> Connection {
    let mut connection = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
    connection
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    connection
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token.clone(),
            client_process_id: std::process::id(),
            client_id: ClientId::new("limits-test"),
            stream_only: false,
            session_secret: None,
            request,
        })
        .unwrap();
    connection
}

fn detach_claiming(snapshots: Vec<PaneSnapshot>) -> Request {
    Request::Detach(DetachRequest {
        session_id: 1,
        summary: BackgroundSessionSummary {
            id: 1,
            title: "limits".to_owned(),
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
        snapshots,
    })
}

/// A detach declares its snapshots' lengths before sending them. One whose
/// declarations exceed the limits is refused on what it declared, before the
/// daemon reads, or reserves room for, a single byte of them.
#[test]
fn a_detach_declaring_too_many_snapshot_bytes_is_refused_before_they_are_read() {
    let mut daemons = Daemons::new();
    daemons.spawn();
    daemons.wait_for_ready();
    let endpoint = daemons.endpoint();
    let eight_mebibytes = zmux::retention::MAX_SNAPSHOT_BYTES;

    for snapshots in [
        // Each within the per-pane limit, together far beyond any request's.
        (0..64)
            .map(|pane_id| PaneSnapshot {
                pane_id,
                length: eight_mebibytes,
            })
            .collect::<Vec<_>>(),
        // Small, but more of them than a session has panes.
        (0..1000)
            .map(|pane_id| PaneSnapshot { pane_id, length: 1 })
            .collect(),
    ] {
        let started = Instant::now();
        let mut connection = raw_request(&endpoint, detach_claiming(snapshots));
        let (response, _) = connection
            .receive::<Response>()
            .expect("the refusal arrives without the snapshot bytes being sent");
        assert!(
            matches!(&response, Response::Error { message } if message.contains("snapshot")),
            "{response:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    // Refusals hold nothing: the daemon still serves.
    daemons.wait_for_ready();
}

/// A request trickled a byte at a time used to keep its connection, and its
/// thread, for as long as the peer cared to; it now has a deadline from accept.
#[test]
fn a_request_trickled_past_its_deadline_is_dropped() {
    let mut daemons = Daemons::new();
    daemons.spawn();
    daemons.wait_for_ready();
    let endpoint = daemons.endpoint();

    let mut stream = UnixStream::connect(&endpoint.socket_path).unwrap();
    // A frame header promising a large message, then one byte a second.
    stream.write_all(&60_000_u32.to_be_bytes()).unwrap();
    let started = Instant::now();
    let closed_after = loop {
        if stream.write_all(b" ").is_err() {
            break started.elapsed();
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the daemon kept a trickled request open past its deadline"
        );
        std::thread::sleep(Duration::from_millis(500));
    };
    // The deadline is ten seconds from accept; a write only fails once the
    // daemon has closed its end, which it does on answering the refusal.
    assert!(closed_after >= Duration::from_secs(9), "{closed_after:?}");
}

/// Past the connection limit a connection is answered and closed at once,
/// and the places come back when the connections holding them go.
#[test]
fn connections_past_the_limit_are_refused_and_places_return() {
    const LIMIT: usize = 1024;
    if !raise_descriptor_limit(LIMIT as u64 + 256) {
        eprintln!("skipping: cannot open enough descriptors to reach the connection limit");
        return;
    }
    let mut daemons = Daemons::new();
    daemons.spawn();
    daemons.wait_for_ready();
    let endpoint = daemons.endpoint();

    // Idle connections, each holding a place until its request deadline.
    let held = (0..LIMIT)
        .map(|_| UnixStream::connect(&endpoint.socket_path).unwrap())
        .collect::<Vec<_>>();
    let mut refused = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while refused.is_none() && Instant::now() < deadline {
        let mut connection = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        // The daemon answers before reading anything, so receive without sending.
        if let Ok((Response::Error { message }, _)) = connection.receive::<Response>() {
            refused = Some(message);
        }
    }
    let refused = refused.expect("a connection past the limit was not refused");
    assert!(refused.contains("too many connections"), "{refused}");

    drop(held);
    daemons.wait_for_ready();
}

fn raise_descriptor_limit(wanted: u64) -> bool {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid out-parameter for the call.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return false;
    }
    if limit.rlim_cur >= wanted {
        return true;
    }
    if limit.rlim_max < wanted {
        return false;
    }
    limit.rlim_cur = wanted;
    // SAFETY: as above; only the soft limit is raised, within the hard one.
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0 }
}
