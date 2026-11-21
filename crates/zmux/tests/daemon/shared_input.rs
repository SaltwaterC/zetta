//! Input to a shared pane: the drain that writes it, and viewers that read
//! slowly, stop reading or go away.

use super::*;

/// A shared pane whose program holds its input back until `gate` is written.
///
/// Returns once the terminal is raw, which the program reports by creating a
/// file beside the gate rather than by printing: output written before the
/// attach completes is not replayed to it. The terminal is raw so that a full input buffer throttles the multiplexer's
/// writes rather than having the line discipline discard what does not fit, as
/// canonical mode does. Once released, the program reads exactly `length` bytes
/// and prints their checksum, so the one line it prints says whether every byte
/// arrived, in order.
#[cfg(target_os = "linux")]
fn gated_reader_pane(
    client: &Client,
    gate: &Path,
    length: usize,
) -> (u64, zmux::client::SharedPane) {
    let status = Command::new("mkfifo").arg(gate).status().unwrap();
    assert!(status.success(), "creating the gate fifo");
    let ready = gate.with_extension("ready");
    let command = format!(
        "stty raw -echo; : > '{}'; read _ < '{}'; head -c {length} | cksum",
        ready.display(),
        gate.display()
    );
    let created = client
        .create_shared(one_pane_shared_request(client, &command))
        .unwrap();
    let pane_id = created.state.operation_receipts[0].draft_mappings[0].pane_id;
    let pane = match client
        .attach_shared_with_secret(created.session_id, pane_id, None)
        .unwrap()
    {
        AttachOutcome::SharedAttached { pane, .. } => pane,
        _other => panic!("a shared session's pane must attach shared"),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(Instant::now() < deadline, "the gated program never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    (created.session_id, pane)
}

/// Printable, so nothing in it is a control character to the line discipline,
/// and not periodic in the chunk size, so a chunk delivered twice or out of
/// order changes the checksum.
#[cfg(target_os = "linux")]
fn paste_payload(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| b'!' + ((index * 7 + index / 4093) % 94) as u8)
        .collect()
}

#[cfg(target_os = "linux")]
fn expected_cksum(payload: &[u8]) -> String {
    let mut cksum = Command::new("cksum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    cksum.stdin.take().unwrap().write_all(payload).unwrap();
    let output = cksum.wait_with_output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Sends a paste the way a window does, in frames the transport accepts.
#[cfg(target_os = "linux")]
fn send_paste(pane: &zmux::client::SharedPane, payload: &[u8]) {
    for chunk in payload.chunks(16 * 1024) {
        pane.send_input(chunk).expect("queueing shared input");
    }
}

/// CPU time a process has used, user and system, in clock ticks.
#[cfg(target_os = "linux")]
fn process_cpu_ticks(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // Counted from after the parenthesised command name, which may itself
    // contain spaces: utime and stime are the 14th and 15th fields of the line.
    let (_, after_name) = status.rsplit_once(") ").unwrap();
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
}

/// The share of one CPU the daemon uses over `window`.
#[cfg(target_os = "linux")]
fn daemon_cpu_share(daemon: &TestDaemon, window: Duration) -> f64 {
    // SAFETY: `sysconf` has no preconditions.
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let pid = daemon.process.id();
    let before = process_cpu_ticks(pid);
    std::thread::sleep(window);
    let used = process_cpu_ticks(pid) - before;
    used as f64 / ticks_per_second / window.as_secs_f64()
}

/// The most of one CPU a daemon holding input nobody reads may use. A drain
/// retrying the unwritable terminal without waiting uses all of one.
#[cfg(target_os = "linux")]
const BLOCKED_INPUT_CPU_CEILING: f64 = 0.1;

/// Another shared pane answers promptly, so the daemon is still serving.
#[cfg(target_os = "linux")]
fn assert_echoes_promptly(client: &Client, marker: &str) {
    let created = client
        .create_shared(one_pane_shared_request(client, "exec cat"))
        .unwrap();
    let pane_id = created.state.operation_receipts[0].draft_mappings[0].pane_id;
    let AttachOutcome::SharedAttached { pane, .. } = client
        .attach_shared_with_secret(created.session_id, pane_id, None)
        .unwrap()
    else {
        panic!("a shared session's pane must attach shared")
    };
    let mut reader = pane.reader();
    let started = Instant::now();
    pane.send_input(format!("{marker}\n").as_bytes()).unwrap();
    read_until_reader(&mut reader, marker);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an unrelated pane took {:?} to echo",
        started.elapsed()
    );
    drop(reader);
    drop(pane);
    client.kill(created.session_id).unwrap();
}

/// A paste into a program that has stopped reading leaves the drain waiting for
/// the terminal to take more, not retrying it as fast as it can — and the rest
/// arrives as soon as the program reads again, rather than on a timer.
#[cfg(target_os = "linux")]
#[test]
fn shared_input_the_child_does_not_read_leaves_the_drain_idle() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let gate = daemon._directory.path().join("gate");
    let payload = paste_payload(1024 * 1024);
    let (session_id, pane) = gated_reader_pane(&client, &gate, payload.len());
    let mut reader = pane.reader();

    send_paste(&pane, &payload);
    // Long enough for the terminal to fill and the drain to settle.
    std::thread::sleep(Duration::from_millis(200));
    let share = daemon_cpu_share(&daemon, Duration::from_secs(1));
    assert!(
        share < BLOCKED_INPUT_CPU_CEILING,
        "the daemon used {:.0}% of a CPU holding input the pane would not take",
        share * 100.0
    );
    assert_echoes_promptly(&client, "unrelated-pane-still-live");

    // Each chunk the terminal takes from here has to wake the drain at once. A
    // drain that only noticed room on its liveness timer would need seconds per
    // chunk, which this deadline does not allow.
    let released = Instant::now();
    std::fs::write(&gate, "\n").unwrap();
    read_until_reader(&mut reader, &expected_cksum(&payload));
    assert!(
        released.elapsed() < Duration::from_secs(2),
        "the held paste took {:?} to arrive once the program read again",
        released.elapsed()
    );
    drop(reader);
    drop(pane);
    // The program exits once it has printed, which may already have ended
    // the session.
    let _ = client.kill(session_id);
}

/// Closing a pane with input still queued for it takes the input with it, and
/// leaves the drain nothing to retry.
#[cfg(target_os = "linux")]
#[test]
fn closing_a_pane_with_pending_input_frees_the_drain() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let gate = daemon._directory.path().join("gate");
    let payload = paste_payload(512 * 1024);
    let (session_id, pane) = gated_reader_pane(&client, &gate, payload.len());
    send_paste(&pane, &payload);
    std::thread::sleep(Duration::from_millis(200));

    client.kill(session_id).unwrap();
    drop(pane);
    std::thread::sleep(Duration::from_millis(200));
    let share = daemon_cpu_share(&daemon, Duration::from_secs(1));
    assert!(
        share < BLOCKED_INPUT_CPU_CEILING,
        "the daemon used {:.0}% of a CPU after the pane holding input closed",
        share * 100.0
    );
    assert_echoes_promptly(&client, "after-close-still-live");
}

/// Input queued for a pane belongs to the pane, not to the client that sent it:
/// the sender leaving neither drops it nor turns the wait for room into a spin,
/// and it still arrives whole once the program reads.
#[cfg(target_os = "linux")]
#[test]
fn a_departing_sender_leaves_pending_input_queued_without_spinning() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let gate = daemon._directory.path().join("gate");
    let payload = paste_payload(512 * 1024);
    let (session_id, sender) = gated_reader_pane(&client, &gate, payload.len());
    let AttachOutcome::SharedAttached { pane: viewer, .. } = client
        .attach_shared_with_secret(session_id, sender.pane_id(), None)
        .unwrap()
    else {
        panic!("a second attachment must be shared")
    };
    let mut reader = viewer.reader();
    send_paste(&sender, &payload);
    std::thread::sleep(Duration::from_millis(200));

    drop(sender);
    std::thread::sleep(Duration::from_millis(200));
    let share = daemon_cpu_share(&daemon, Duration::from_secs(1));
    assert!(
        share < BLOCKED_INPUT_CPU_CEILING,
        "the daemon used {:.0}% of a CPU after the sender left",
        share * 100.0
    );
    assert_echoes_promptly(&client, "after-sender-left-still-live");

    std::fs::write(&gate, "\n").unwrap();
    read_until_reader(&mut reader, &expected_cksum(&payload));
    drop(reader);
    drop(viewer);
    // The program exits once it has printed, which may already have ended
    // the session.
    let _ = client.kill(session_id);
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

/// A subscription crossing an SSH forward can stop reading while Wi-Fi
/// recovers. Its event writer used to hold `Daemon::subscribers`; a concurrent
/// revoke then held `Daemon::sessions` while it waited for that writer, which
/// stopped an unrelated shared pane from accepting input.
#[test]
fn a_stalled_subscription_cannot_block_another_shared_panes_input() {
    let daemon = TestDaemon::start();
    let holder = daemon.client();
    let _holder_subscription = holder.subscribe().unwrap();
    let held = holder
        .spawn(spawn_request(None, "sleep 60"))
        .expect("spawning the exclusive pane");
    let held_descriptor = std::fs::File::from(held.descriptor);
    let offered = summary(held.session_id, held.pane_id);
    holder
        .share(
            held.session_id,
            offered,
            serde_json::Value::Null,
            None,
            true,
        )
        .expect("offering the session");

    let request = spawn_request(Some(held.session_id), "exec cat");
    let spawned = holder
        .spawn_shared(SharedSpawnRequest {
            session_id: held.session_id,
            base_revision: SessionRevision::INITIAL,
            operation_id: holder.next_shared_operation_id(),
            program: request.program,
            args: request.args,
            env: request.env,
            working_directory: request.working_directory,
            size: request.size,
            console_palette: request.console_palette,
            console_backend: Default::default(),
        })
        .expect("spawning the healthy shared pane");
    let mut healthy_reader = spawned.pane.reader();

    let endpoint: zmux::transport::Endpoint =
        serde_json::from_slice(&std::fs::read(daemon.sessions_dir().join("zmux.json")).unwrap())
            .unwrap();
    let mut stalled = Connection::new(Stream::connect(&endpoint.socket_path).unwrap());
    stalled
        .send(&Envelope {
            version: zmux::messages::PROTOCOL_VERSION,
            token: endpoint.token,
            client_process_id: std::process::id(),
            client_id: ClientId::new("stalled-subscription"),
            stream_only: true,
            session_secret: None,
            request: Request::Subscribe,
        })
        .expect("opening the subscription that intentionally never reads");
    std::thread::sleep(Duration::from_millis(100));

    let updates_started = Arc::new(AtomicUsize::new(0));
    let update_client = daemon.client();
    let update_started = Arc::clone(&updates_started);
    let session_id = held.session_id;
    let initial_revision = spawned.state.revision;
    std::thread::spawn(move || {
        let mut revision = initial_revision;
        let payload = "x".repeat(48 * 1024);
        for round in 0..24 {
            update_started.fetch_add(1, Ordering::Release);
            let result = update_client.apply_shared(
                session_id,
                revision,
                SharedSessionOperation::SetTabState {
                    state: serde_json::json!({ "round": round, "payload": payload }),
                },
            );
            let Ok(zmux::client::SharedOperationResult::Applied(state)) = result else {
                return;
            };
            revision = state.revision;
        }
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while updates_started.load(Ordering::Acquire) < 4 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    // The exclusive pane's revoke needs the subscriber registry. On the old
    // path it waits behind the stalled event write while holding sessions.
    let contender_process = Command::new("/bin/sleep").arg("60").spawn().unwrap();
    let contender_pid = contender_process.id();
    let contender = daemon.client();
    std::thread::spawn(move || {
        let _ = contender.attach_as(session_id, held.pane_id, contender_pid, None);
    });
    std::thread::sleep(Duration::from_millis(100));

    let marker = "subscription-control-plane-still-live";
    spawned
        .pane
        .send_input(format!("{marker}\n").as_bytes())
        .expect("sending input through the healthy shared pane");
    read_until_reader(&mut healthy_reader, marker);

    drop(stalled);
    drop(held_descriptor);
    reap(contender_process);
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

/// Does one viewer that stops reading wedge the pane for everybody?
///
/// The daemon broadcasts a pane's output to every attached viewer. If a write
/// to one of them can block, the pane's pty stops being drained, the shell
/// blocks on its own write, and it stops reading input — which looks exactly
/// like "one character went in and then nothing did, on either side".
#[test]
fn a_viewer_that_stops_reading_does_not_wedge_the_pane_for_the_others() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client
        .spawn(spawn_request(None, "printf ready; cat"))
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

    let request = spawn_request(Some(pane.session_id), "printf second-ready; cat");
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
    let shared_pane_id = spawned.pane.pane_id();

    // A second viewer that attaches and never reads a byte.
    let secret = SessionSecret::new(TEST_SECRET.to_owned());
    let AttachOutcome::SharedAttached { pane: silent, .. } = client
        .attach_shared_with_secret(pane.session_id, shared_pane_id, Some(&secret))
        .unwrap()
    else {
        panic!("the silent viewer did not attach as a stream")
    };

    // The viewer that is reading, and typing.
    let AttachOutcome::SharedAttached { pane: active, .. } = client
        .attach_shared_with_secret(pane.session_id, shared_pane_id, Some(&secret))
        .unwrap()
    else {
        panic!("the active viewer did not attach as a stream")
    };
    let seen = collect_output(active.reader());

    // Enough output to overflow any socket buffer the silent viewer is not
    // draining, several times over.
    active
        .send_input(b"yes zetta-wedge-probe | head -c 2000000\r")
        .unwrap();
    std::thread::sleep(Duration::from_millis(750));

    active.send_input(b"echo still-typing\r").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if seen.lock().unwrap().contains("still-typing") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the pane stopped taking input while one viewer was not reading; \
             read {} bytes",
            seen.lock().unwrap().len()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(silent);
}
