//! `zmux relay-pane`: copying a shared pane between the daemon and a relay's
//! own stdio, and the size it reports.

use super::*;

/// The far side of a pane carried over Mosh.
///
/// `zmux relay-pane` is what runs on the PTY `zosh-server` creates, so the
/// pane has to come out of that process's stdout, its stdin has to reach the
/// program, and a protected session's secret has to be taken from stdin
/// rather than from a command line every account on the host can read.
#[test]
fn relay_pane_copies_a_shared_pane_between_the_daemon_and_its_own_stdio() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let offered = summary(pane.session_id, pane.pane_id);
    client
        .share(
            pane.session_id,
            offered,
            serde_json::Value::Null,
            Some(&test_verifier()),
            true,
        )
        .unwrap();

    // A shared spawn, so the pane the relay attaches is one no other process
    // is holding a descriptor for — which is the shape a remote pane has.
    let request = spawn_request(Some(pane.session_id), "printf relayed; cat");
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
            console_backend: Default::default(),
        })
        .unwrap();
    let relayed_pane = spawned.pane.pane_id();
    drop(spawned);

    let mut relay = Command::new(daemon_binary())
        .args([
            "relay-pane",
            &pane.session_id.to_string(),
            &relayed_pane.to_string(),
            "--secret-stdin",
        ])
        .env("XDG_CONFIG_HOME", &daemon.config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starting the relay");
    let mut relay_input = relay.stdin.take().expect("the relay's stdin");
    let relayed = collect_output(relay.stdout.take().expect("the relay's stdout"));

    writeln!(relay_input, "{TEST_SECRET}").expect("sending the session secret");
    wait_for_output(&relayed, "relayed", &relay);

    // The prelude reaches the PTY over a Mosh link established before this
    // process existed, so the line discipline can echo it before raw mode is
    // on. That echo is not the pane's, and blanking the screen before the
    // pane's first byte is what keeps it from being the first line a viewer
    // sees — which is where the viewer's client ID used to appear.
    assert!(
        relayed
            .lock()
            .unwrap()
            .starts_with("\u{1b}[2J\u{1b}[3J\u{1b}[H"),
        "the relay must blank its screen before writing the pane: {:?}",
        relayed.lock().unwrap()
    );

    write!(relay_input, "typed\r").expect("sending pane input");
    wait_for_output(&relayed, "typed", &relay);

    let _ = relay.kill();
    let _ = relay.wait();
}

/// A relay's size outlives edits to the session that do not resize it.
///
/// Every edit moves the session's revision on, and the daemon forgets the sizes
/// reported against the old one. A window reports again after each edit; a
/// relay reports only when Mosh resizes its terminal, so a focus change left it
/// unmeasured — and when the smaller window then left, the pane stayed at that
/// window's size instead of growing to the relay's.
#[test]
fn a_relay_keeps_its_size_when_a_smaller_viewer_leaves_after_the_session_moves_on() {
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

    // Every line typed into the pane prints the size its terminal is at.
    let request = spawn_request(
        Some(pane.session_id),
        "printf relayed; while read -r _; do stty size; done",
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
            console_backend: Default::default(),
        })
        .unwrap();
    let relayed_pane = spawned.pane.pane_id();
    drop(spawned);

    let small = match client
        .attach_shared_as(pane.session_id, relayed_pane, std::process::id(), None)
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("the small viewer must attach as shared"),
    };
    let mut small_reader = small.reader();

    // Started the way Zetta starts one: naming the window it relays for.
    let terminal = TestPty::open(120, 40);
    let mut relay = relay_command(&daemon, pane.session_id, relayed_pane, &terminal);
    relay.arg("--viewer-stdin");
    let mut relay = relay.spawn().expect("starting the relay");
    let mut relay_input = terminal.master().expect("the terminal's master side");
    let relayed = collect_output(terminal.master().expect("the terminal's master side"));
    writeln!(relay_input, "relayed-window").expect("naming the relayed viewer");
    wait_for_output(&relayed, "relayed", &relay);
    wait_for_shared_size(&mut small_reader, &small, (120, 40));

    let revision = client.shared_snapshot(pane.session_id).unwrap().revision;
    small
        .send_resize_for_revision(revision, 60, 20)
        .expect("reporting the small viewer's size");
    wait_for_shared_size(&mut small_reader, &small, (60, 20));

    // A focus change moves the session on; the window reports again for the
    // new revision, as Zetta does after every edit, and the relay does not.
    client
        .apply_shared(
            pane.session_id,
            revision,
            SharedSessionOperation::SetFocus {
                pane_id: relayed_pane,
            },
        )
        .unwrap();
    let revision = client.shared_snapshot(pane.session_id).unwrap().revision;
    small
        .send_resize_for_revision(revision, 60, 20)
        .expect("reporting the small viewer's size again");

    drop(small_reader);
    drop(small);

    let deadline = Instant::now() + Duration::from_secs(10);
    while !relayed.lock().unwrap().contains("40 120") {
        assert!(
            Instant::now() < deadline,
            "the pane never grew back to the relay's size; it printed {:?}\nDAEMONLOG\n{}",
            relayed.lock().unwrap(),
            std::fs::read_to_string(daemon.sessions_dir().join("daemon.log")).unwrap_or_default()
        );
        write!(relay_input, "\r").expect("asking the pane for its size");
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = relay.kill();
    let _ = relay.wait();
}

/// A relay's size report is taken whatever revision it names.
///
/// A relay stamps its report with the revision it reads just before sending,
/// and an edit landing in between used to get the report dropped. A window
/// reports again after every edit; a relay does not, so it stayed unmeasured.
#[test]
fn a_relays_size_report_survives_losing_a_race_with_an_edit() {
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
    let request = spawn_request(Some(pane.session_id), "printf relayed; sleep 60");
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
            console_backend: Default::default(),
        })
        .unwrap();
    let relayed_pane = spawned.pane.pane_id();
    drop(spawned);

    let observer = match client
        .attach_shared_as(pane.session_id, relayed_pane, std::process::id(), None)
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("the observer must attach as shared"),
    };
    let mut observer_reader = observer.reader();
    let relay = match daemon
        .client()
        .attach_shared_relaying_for(
            pane.session_id,
            relayed_pane,
            None,
            ClientId::new("relayed-window"),
        )
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("the relay must attach as shared"),
    };

    let stale = client.shared_snapshot(pane.session_id).unwrap().revision;
    client
        .apply_shared(
            pane.session_id,
            stale,
            SharedSessionOperation::SetFocus {
                pane_id: relayed_pane,
            },
        )
        .unwrap();
    assert_ne!(
        client.shared_snapshot(pane.session_id).unwrap().revision,
        stale,
        "the edit has to have moved the revision on for this test to mean anything"
    );
    relay
        .send_resize_for_revision(stale, 120, 40)
        .expect("reporting the relay's size at the revision it read");
    wait_for_shared_size(&mut observer_reader, &observer, (120, 40));
}

/// A relay reports its size at the revision the session is on *now*.
///
/// The daemon drops a size report naming any other revision, and says nothing
/// about having done so. A relay learns a revision only from an arbitrated size
/// broadcast to it, which never happens for a pane whose one viewer it is — so
/// the revision it attached at was the only one it ever reported at. Adding a
/// pane to the session moves the revision on, which used to leave the pane the
/// relay carries frozen at its pre-split size for good: the window showed 98
/// columns while the shell inside it went on wrapping at 198.
#[test]
fn a_relay_reports_its_size_after_the_session_revision_moves_on() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; sleep 60"))
        .unwrap();
    let descriptor = std::fs::File::from(pane.descriptor);
    read_until(&descriptor, "ready");
    let offered = summary(pane.session_id, pane.pane_id);
    client
        .share(
            pane.session_id,
            offered,
            serde_json::Value::Null,
            None,
            true,
        )
        .unwrap();

    let request = spawn_request(Some(pane.session_id), "printf relayed; cat");
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
            console_backend: Default::default(),
        })
        .unwrap();
    let relayed_pane = spawned.pane.pane_id();
    drop(spawned);

    // A second viewer of the same pane: the arbitrated size is broadcast to
    // every shared client, and this one reports no size of its own, so what it
    // is told is what the relay asked for.
    // Give the observer its own logical identity. The spawning stream above
    // closes asynchronously; if this attachment reused its client ID, that
    // stream's delayed cleanup could remove the new observer along with the
    // old connection and leave this test waiting on a dead relay.
    let observer = match client
        .attach_shared_as(pane.session_id, relayed_pane, std::process::id(), None)
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("the observer must attach as shared"),
    };
    let mut observer_reader = observer.reader();

    let terminal = TestPty::open(120, 40);
    let mut relay = relay_command(&daemon, pane.session_id, relayed_pane, &terminal)
        .spawn()
        .expect("starting the relay");
    // Drained so a PTY that fills cannot block the relay's own writes.
    let _relayed = collect_output(terminal.master().expect("the terminal's master side"));

    assert_eq!(
        wait_for_shared_size(&mut observer_reader, &observer, (120, 40)).last(),
        Some(&(120, 40)),
        "a relay's first report is at the revision it attached at"
    );

    // Anything that edits the session moves it on, exactly as adding a pane
    // does. The relay is told nothing about it.
    let revision = client.shared_snapshot(pane.session_id).unwrap().revision;
    client
        .apply_shared(
            pane.session_id,
            revision,
            SharedSessionOperation::SetFocus {
                pane_id: relayed_pane,
            },
        )
        .unwrap();
    assert_ne!(
        client.shared_snapshot(pane.session_id).unwrap().revision,
        revision,
        "the edit has to have moved the revision on for this test to mean anything"
    );

    terminal.resize(60, 20);
    assert_eq!(
        wait_for_shared_size(&mut observer_reader, &observer, (60, 20)).last(),
        Some(&(60, 20)),
        "a resize after the session moved on must still reach the pane"
    );

    let _ = relay.kill();
    let _ = relay.wait();
}

/// `zmux relay-pane` reading the pane whose viewer is `terminal`.
fn relay_command(
    daemon: &TestDaemon,
    session_id: u64,
    pane_id: u64,
    terminal: &TestPty,
) -> Command {
    let mut command = Command::new(daemon_binary());
    command
        .args(["relay-pane", &session_id.to_string(), &pane_id.to_string()])
        .env("XDG_CONFIG_HOME", &daemon.config)
        .stdin(terminal.stdio())
        .stdout(terminal.stdio())
        .stderr(terminal.stdio());
    // SAFETY: the closure runs between `fork` and `exec` and calls only
    // async-signal-safe functions.
    unsafe {
        command.pre_exec(|| {
            // A terminal's size change is signalled to the foreground process
            // group of the session that owns it, so a relay that is neither
            // never hears about one.
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

/// A PTY pair for a child that has to be under a terminal rather than a pipe:
/// a relay reads its size from one and is signalled by it, and neither happens
/// through a pipe.
struct TestPty {
    master: std::fs::File,
    slave: std::fs::File,
}

impl TestPty {
    fn open(columns: u16, lines: u16) -> Self {
        let mut master = 0;
        let mut slave = 0;
        let mut size = winsize(columns, lines);
        // SAFETY: `openpty` writes both descriptors and reads `size` while the
        // pointer remains valid. Apple declares the input pointer mutable.
        let opened = unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut size,
            )
        };
        assert_eq!(
            opened,
            0,
            "opening a PTY: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: `openpty` succeeded, so both descriptors are open and ours.
        unsafe {
            Self {
                master: std::fs::File::from_raw_fd(master),
                slave: std::fs::File::from_raw_fd(slave),
            }
        }
    }

    /// Stdio for the child: one handle per stream, because `Stdio` consumes it.
    fn stdio(&self) -> Stdio {
        Stdio::from(self.slave.try_clone().expect("cloning the PTY slave"))
    }

    fn master(&self) -> std::io::Result<std::fs::File> {
        self.master.try_clone()
    }

    fn resize(&self, columns: u16, lines: u16) {
        let size = winsize(columns, lines);
        // SAFETY: the descriptor is this pair's master side and `size` is the
        // window it is being given.
        let resized = unsafe {
            libc::ioctl(
                self.master.as_raw_fd(),
                libc::TIOCSWINSZ as _,
                &raw const size,
            )
        };
        assert_eq!(
            resized,
            0,
            "resizing the PTY: {}",
            std::io::Error::last_os_error()
        );
    }
}

fn wait_for_output(collected: &Arc<Mutex<String>>, expected: &str, child: &Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let seen = collected.lock().unwrap().clone();
        if seen.contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never saw {expected:?} from the relay (process {}); read {seen:?}",
            child.id()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
