use super::*;
use std::sync::mpsc;
#[cfg(unix)]
use std::thread;

#[cfg(windows)]
#[test]
fn default_command_uses_openssh_shell_when_configured() {
    let Some(shell) = windows_ssh_default_shell() else {
        return;
    };
    let command = build_command(&Config::default());
    assert_eq!(command.get_argv(), &[shell]);
    let explicit = Config {
        command: vec!["custom-shell.exe".into()],
        ..Config::default()
    };
    assert_eq!(build_command(&explicit).get_argv(), &explicit.command);
}

#[test]
fn udp_port_candidates_start_randomly_and_wrap_the_complete_range() {
    let ports = (0..4)
        .map(|attempt| candidate_port(60_000, 4, 2, attempt))
        .collect::<Vec<_>>();
    assert_eq!(ports, [60_002, 60_003, 60_000, 60_001]);
}

#[test]
fn the_server_keeps_its_half_alive_without_hearing_anything() {
    let interval = Duration::from_millis(500);
    let armed = Some(interval);

    // A client that never announced one leaves the server on Mosh's own
    // heartbeat, however long it has been.
    assert!(!keep_alive_due(
        None,
        Duration::from_secs(60),
        Duration::ZERO
    ));

    // Armed: due on the interval since the last SEND. What has been heard
    // does not enter into it, which is the whole point — a server that
    // only replies goes quiet exactly when the client's packets are the
    // ones being delayed.
    assert!(!keep_alive_due(
        armed,
        interval - Duration::from_millis(1),
        Duration::ZERO
    ));
    assert!(keep_alive_due(armed, interval, Duration::from_secs(3)));

    // But not forever: once nothing has been heard for the linger, a
    // detached session stops transmitting into the void.
    assert!(keep_alive_due(
        armed,
        interval,
        KEEP_ALIVE_LINGER - Duration::from_millis(1)
    ));
    assert!(!keep_alive_due(armed, interval, KEEP_ALIVE_LINGER));
}

#[test]
fn the_keep_alive_deadline_is_when_the_keep_alive_falls_due() {
    let interval = Duration::from_millis(500);
    let armed = Some(interval);
    let start = Instant::now();

    assert_eq!(keep_alive_deadline(None, start, start), None);
    let due = keep_alive_deadline(armed, start, start).expect("armed");
    assert_eq!(due, start + interval);
    // The deadline and the predicate agree on either side of it.
    assert!(!keep_alive_due(
        armed,
        due - start - Duration::from_millis(1),
        due - start
    ));
    assert!(keep_alive_due(armed, due - start, due - start));

    // Nothing is scheduled once the linger would have lapsed by then.
    let heard = start - KEEP_ALIVE_LINGER;
    assert_eq!(keep_alive_deadline(armed, start, heard), None);
    let heard = start + interval - KEEP_ALIVE_LINGER + Duration::from_millis(1);
    assert_eq!(keep_alive_deadline(armed, start, heard), Some(due));
}

#[test]
fn an_announced_keep_alive_interval_is_clamped_before_it_is_believed() {
    // The interval arrives over the network, so it is clamped rather than
    // trusted: a peer must not be able to ask this server for a datagram
    // every microsecond, nor for one so rare it is not a keep-alive.
    let clamp = |announced: u32| {
        Duration::from_millis(u64::from(announced)).clamp(KEEP_ALIVE_MIN, KEEP_ALIVE_MAX)
    };
    assert_eq!(clamp(0), KEEP_ALIVE_MIN);
    assert_eq!(clamp(1), KEEP_ALIVE_MIN);
    assert_eq!(clamp(500), Duration::from_millis(500));
    assert_eq!(clamp(u32::MAX), KEEP_ALIVE_MAX);
}

#[test]
fn bounded_pty_drain_reports_remaining_work() {
    let (sender, events) = mpsc::sync_channel(128);
    let (writes, _write_rx) = mpsc::sync_channel(1);
    for frame in 1..=128 {
        sender.send(PtyEvent::InputWritten(frame)).unwrap();
    }
    let mut pty = PtyIo::Threads { events, writes };
    let mut terminal = TerminalState::new(24, 80);
    let mut responder = QueryResponder::new();
    let mut echo = EchoAcknowledgements::default();
    let progress = drain_pty_events(
        &mut pty,
        &mut terminal,
        &mut responder,
        true,
        &mut echo,
        false,
    )
    .unwrap();
    assert!(progress.budget_exhausted);
    assert!(pty.next_event().is_some());
}
use moshcatty::transport::Transport;

#[test]
fn rejects_pathological_terminal_sizes() {
    assert!(validate_terminal_size(24, 80).is_ok());
    assert!(validate_terminal_size(1024, 1024).is_err());
    assert!(validate_terminal_size(1, 1024).is_ok());
}

#[test]
fn late_ack_waits_for_grace_and_new_input_does_not_postpone_old_input() {
    let now = Instant::now();
    let mut echo = EchoAcknowledgements::default();
    echo.written(7, now);
    echo.written(8, now + Duration::from_millis(30));
    assert_eq!(echo.advance(now + Duration::from_millis(49)), 0);
    assert_eq!(echo.advance(now + Duration::from_millis(50)), 7);
    assert_eq!(echo.advance(now + Duration::from_millis(79)), 7);
    assert_eq!(echo.advance(now + Duration::from_millis(80)), 8);
    assert_eq!(echo.advance(now + Duration::from_secs(1)), 8);
    assert!(echo.pending.is_empty());
}

#[test]
fn unrelated_or_partial_output_does_not_confirm_recent_input() {
    let (event_tx, events) = mpsc::sync_channel(4);
    let (writes, _write_rx) = mpsc::sync_channel(4);
    let mut pty = PtyIo::Threads { events, writes };
    let mut terminal = TerminalState::new(24, 80);
    let mut responder = QueryResponder::new();
    let mut echo = EchoAcknowledgements::default();
    // A future timestamp makes this test independent of scheduler pauses.
    let written_at = Instant::now() + Duration::from_secs(60);
    echo.written(9, written_at);
    for bytes in [b"old output".as_slice(), b"\x1b[", b"K"] {
        event_tx.send(PtyEvent::Output(bytes.to_vec())).unwrap();
        let progress = drain_pty_events(
            &mut pty,
            &mut terminal,
            &mut responder,
            true,
            &mut echo,
            false,
        )
        .unwrap();
        assert!(progress.dirty);
        assert!(!progress.ended);
        assert_eq!(echo.advance(written_at), 0);
    }
    assert_eq!(echo.advance(written_at + ECHO_DELAY), 9);
}

#[test]
fn coalesced_screen_update_retains_echo_ack() {
    let key = [0x36; 16];
    let mut server = ServerTransport::new(Ocb::new(&key).unwrap());
    let mut client = Transport::new_client(Ocb::new(&key).unwrap());
    let mut terminal = TerminalState::new(24, 80);
    terminal.process(b"x");
    server.set_pending(host_update(&terminal, None, 7, &[]).unwrap());
    // A second PTY chunk arrives before tick sends the first update. The SSP
    // transport discards that unsent state, including its echo instruction.
    terminal.process(b"y");
    server.set_pending(host_update(&terminal, None, 7, &[]).unwrap());
    let datagrams = server.tick();
    assert!(!datagrams.is_empty());
    let payload = datagrams
        .iter()
        .find_map(|packet| client.recv(packet))
        .unwrap();
    let instructions = HostInstruction::decode_message(&payload).unwrap();
    assert!(
        instructions
            .iter()
            .any(|instruction| instruction.echo_ack_num == 7)
    );
    let mut screen = vt100::Parser::new(24, 80, 0);
    for instruction in instructions {
        screen.process(&instruction.hoststring);
    }
    assert_eq!(screen.screen().contents(), "xy");
}

#[test]
fn colour_configuration_preserves_inherited_locales_and_explicit_overrides() {
    const CHILD: &str = "ZOSH_SERVER_LOCALE_CHILD";
    const LOCALES: &[(&str, &str)] = &[
        ("LANG", "remote.UTF-8"),
        ("LANGUAGE", "remote: fallback"),
        ("LC_ALL", "all.UTF-8"),
        ("LC_CTYPE", "ctype.UTF-8"),
        ("LC_NUMERIC", ""),
        ("LC_TIME", "time"),
        ("LC_COLLATE", "collate"),
        ("LC_MONETARY", "money"),
        ("LC_MESSAGES", "messages"),
        ("LC_PAPER", "paper"),
        ("LC_NAME", "name"),
        ("LC_ADDRESS", "address"),
        ("LC_TELEPHONE", "telephone"),
        ("LC_MEASUREMENT", "measurement"),
        ("LC_IDENTIFICATION", "identity"),
        ("LC_ZOSH_TEST", "extension"),
    ];
    if std::env::var_os(CHILD).is_none() {
        for mode in ["seeded", "empty", "unset"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", "server::tests::colour_configuration_preserves_inherited_locales_and_explicit_overrides", "--nocapture"])
                .env_clear().env(CHILD, mode);
            if mode != "unset" {
                child.envs(
                    LOCALES
                        .iter()
                        .map(|(name, value)| (*name, if mode == "empty" { "" } else { *value })),
                );
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return;
    }
    let mode = std::env::var(CHILD).unwrap();
    for colors in [256, 32768] {
        let cfg = Config {
            colors,
            ..Config::default()
        };
        let mut command = CommandBuilder::new("fixture");
        configure_child_environment(&mut command, &cfg, None);
        for (name, value) in LOCALES {
            assert_eq!(
                command.get_env(name),
                match mode.as_str() {
                    "seeded" => Some(std::ffi::OsStr::new(value)),
                    "empty" => Some(std::ffi::OsStr::new("")),
                    "unset" => None,
                    _ => unreachable!(),
                },
                "{mode}: {name}"
            );
        }
        assert_eq!(command.get_env("LC_UNSET_TEST"), None);
        assert_eq!(
            command.get_env("TERM"),
            Some(std::ffi::OsStr::new("xterm-256color"))
        );
        if colors == 32768 {
            assert_eq!(
                command.get_env("COLORTERM"),
                Some(std::ffi::OsStr::new("truecolor"))
            );
        }
    }
    let crate::args::ParseOutcome::Run(cfg) = crate::args::parse(
        [
            "new",
            "-c",
            "32768",
            "-l",
            "LANG=explicit.UTF-8",
            "-l",
            "LC_ALL=",
            "-l",
            "LC_NUMERIC=C",
        ]
        .into_iter()
        .map(Into::into)
        .collect(),
    )
    .unwrap() else {
        panic!("expected server configuration")
    };
    let mut command = CommandBuilder::new("fixture");
    configure_child_environment(&mut command, &cfg, None);
    for (name, value) in [
        ("LANG", "explicit.UTF-8"),
        ("LC_ALL", ""),
        ("LC_NUMERIC", "C"),
    ] {
        assert_eq!(command.get_env(name), Some(std::ffi::OsStr::new(value)));
    }
}

#[test]
fn openssh_handle_state_is_recognised_by_its_suffix() {
    use std::ffi::OsStr;

    assert!(is_openssh_handle_state(OsStr::new(
        "c28fc6f98a2c44abbbd89d6a3037d0d9_POSIX_FD_STATE"
    )));
    assert!(is_openssh_handle_state(OsStr::new("other_posix_fd_state")));
    assert!(!is_openssh_handle_state(OsStr::new("SSH_AUTH_SOCK")));
    assert!(!is_openssh_handle_state(OsStr::new("POSIX_FD_STATE_EXTRA")));
}

fn test_transport() -> ServerTransport {
    ServerTransport::new(Ocb::new(&[0x5a; 16]).unwrap())
}

fn wake_sources<'a>(
    transport: &'a ServerTransport,
    terminal: &'a TerminalState,
    echo: &'a EchoAcknowledgements,
) -> WakeSources<'a> {
    let now = Instant::now();
    WakeSources {
        now,
        associated: true,
        association_deadline: now + ASSOCIATION_TIMEOUT,
        network_timeout: None,
        transport,
        keep_alive: None,
        last_send: now,
        send_failed_at: None,
        terminal,
        echo,
        child_poll: None,
        frame_due: None,
    }
}

#[test]
fn an_unattached_session_sleeps_until_its_association_deadline() {
    let (transport, terminal, echo) = (
        test_transport(),
        TerminalState::new(24, 80),
        EchoAcknowledgements::default(),
    );
    let sources = WakeSources {
        associated: false,
        ..wake_sources(&transport, &terminal, &echo)
    };
    assert_eq!(
        next_wake(sources).earliest(),
        Some(sources.association_deadline)
    );
}

#[test]
fn an_idle_attached_session_sleeps_until_its_transport_is_due() {
    let (transport, terminal, echo) = (
        test_transport(),
        TerminalState::new(24, 80),
        EchoAcknowledgements::default(),
    );
    let sources = wake_sources(&transport, &terminal, &echo);
    let due = transport
        .next_deadline()
        .expect("an idle transport has a heartbeat");
    assert!(
        due > sources.now + Duration::from_secs(1),
        "the heartbeat is seconds away"
    );
    assert_eq!(next_wake(sources).earliest(), Some(due));
}

#[test]
fn the_loop_wakes_for_echo_acknowledgements_and_keep_alives() {
    let (transport, terminal) = (test_transport(), TerminalState::new(24, 80));
    let mut echo = EchoAcknowledgements::default();
    let written = Instant::now();
    echo.written(1, written);
    let sources = wake_sources(&transport, &terminal, &echo);
    assert_eq!(next_wake(sources).earliest(), Some(written + ECHO_DELAY));

    let echo = EchoAcknowledgements::default();
    let sources = WakeSources {
        keep_alive: Some(Duration::from_millis(500)),
        ..wake_sources(&transport, &terminal, &echo)
    };
    assert_eq!(
        next_wake(sources).earliest(),
        Some(sources.last_send + Duration::from_millis(500))
    );
}

#[test]
fn a_keep_alive_that_cannot_be_sent_is_retried_on_an_interval_not_continuously() {
    let (transport, terminal, echo) = (
        test_transport(),
        TerminalState::new(24, 80),
        EchoAcknowledgements::default(),
    );
    let base = wake_sources(&transport, &terminal, &echo);
    let overdue = WakeSources {
        keep_alive: Some(KEEP_ALIVE_MIN),
        last_send: base.now - Duration::from_secs(1),
        ..base
    };
    assert!(
        next_wake(overdue).earliest().unwrap() <= base.now,
        "an overdue keep-alive is due at once"
    );

    let failing = WakeSources {
        send_failed_at: Some(base.now),
        ..overdue
    };
    assert_eq!(
        next_wake(failing).earliest(),
        Some(base.now + KEEP_ALIVE_MIN)
    );
}

#[test]
fn timers_that_have_already_fired_do_not_wake_the_loop_again() {
    // Past the presence linger and the scrollback stall, both of which a pass
    // acts on once and then has nothing more to do for. A deadline left in
    // the past would make every park return at once.
    let mut transport = test_transport();
    transport.start_shutdown();
    for _ in 0..64 {
        transport.force_next_send();
        transport.tick();
    }
    assert!(transport.shutdown_timed_out());
    assert_eq!(transport.next_deadline(), None);

    let mut terminal = TerminalState::new(24, 80);
    terminal.set_scrollback_budget(0);
    for _ in 0..20_000 {
        terminal.process(b"a line of output that scrolls off the top\r\n");
    }
    assert!(terminal.scrollback_over_budget());
    let echo = EchoAcknowledgements::default();
    let later = Instant::now() + sleep_guard::PRESENCE_LINGER * 2;

    let attached = WakeSources {
        now: later,
        ..wake_sources(&transport, &terminal, &echo)
    };
    assert_eq!(next_wake(attached).earliest(), None);

    let unattached = WakeSources {
        associated: false,
        association_deadline: later + ASSOCIATION_TIMEOUT,
        ..attached
    };
    assert_eq!(
        next_wake(unattached).earliest(),
        Some(unattached.association_deadline)
    );
}

#[test]
fn a_pty_without_an_exit_watch_is_polled_on_its_fallback_timer() {
    let (transport, terminal, echo) = (
        test_transport(),
        TerminalState::new(24, 80),
        EchoAcknowledgements::default(),
    );
    let poll = Instant::now() + child_exit::FALLBACK_POLL;
    let sources = WakeSources {
        child_poll: Some(poll),
        ..wake_sources(&transport, &terminal, &echo)
    };
    assert_eq!(next_wake(sources).earliest(), Some(poll));
}

/// Exchanges datagrams between a test client and the session for `period`,
/// returning when the first datagram from the session arrived, if one did.
#[cfg(unix)]
fn pump(
    client: &mut Transport,
    socket: &UdpSocket,
    server: SocketAddr,
    period: Duration,
) -> Option<Instant> {
    let until = Instant::now() + period;
    let mut first = None;
    let mut buf = vec![0u8; UDP_BUFFER];
    while Instant::now() < until {
        if let Ok((n, _)) = socket.recv_from(&mut buf) {
            first.get_or_insert_with(Instant::now);
            client.recv(&buf[..n]);
        }
        for datagram in client.tick() {
            socket.send_to(&datagram, server).unwrap();
        }
    }
    first
}

// Unix only because it echoes through `cat`; the loop it measures is the same
// code on every platform.
#[cfg(unix)]
#[test]
fn an_idle_session_sleeps_instead_of_polling_and_still_answers_at_once() {
    use moshcatty::pb::UserInstruction;

    let key = [0x24u8; 16];
    let server_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let server_addr = server_socket.local_addr().unwrap();
    let cfg = Config {
        command: vec!["cat".into()],
        ..Config::default()
    };
    let session = thread::spawn(move || {
        serve_session(
            cfg,
            server_socket,
            ServerTransport::new(Ocb::new(&key).unwrap()),
        )
    });

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(5)))
        .unwrap();
    let mut client = Transport::new_client(Ocb::new(&key).unwrap());
    client.set_pending(UserInstruction::encode_message(&[UserInstruction::resize(
        80, 24,
    )]));
    assert!(
        pump(
            &mut client,
            &socket,
            server_addr,
            Duration::from_millis(1500)
        )
        .is_some(),
        "the session never answered its client"
    );

    // Attached, acknowledged and quiet: the old loop made about two hundred
    // passes a second here.
    let before = LOOP_PASSES.load(Ordering::Relaxed);
    pump(&mut client, &socket, server_addr, Duration::from_secs(1));
    let passes = LOOP_PASSES.load(Ordering::Relaxed) - before;
    assert!(
        passes < 25,
        "an idle session made {passes} passes in a second"
    );

    // Sleeping must not cost responsiveness: a keystroke is read, echoed by
    // `cat` and the screen sent back without waiting for any timer.
    client.set_pending(UserInstruction::encode_message(&[
        UserInstruction::keystroke(b"x".to_vec()),
    ]));
    client.force_next_send();
    let sent = Instant::now();
    let answered = pump(&mut client, &socket, server_addr, Duration::from_secs(1))
        .expect("the keystroke was answered");
    assert!(
        answered - sent < Duration::from_millis(250),
        "the session took {:?} to answer a keystroke",
        answered - sent
    );

    client.start_shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !session.is_finished() {
        assert!(Instant::now() < deadline, "the session did not shut down");
        pump(&mut client, &socket, server_addr, Duration::from_millis(50));
    }
    session.join().unwrap().unwrap();
}

const SCREEN: Owed = Owed {
    screen: true,
    echo: false,
};
const ECHO: Owed = Owed {
    screen: false,
    echo: true,
};
const BOTH: Owed = Owed {
    screen: true,
    echo: true,
};
const NOTHING: Owed = Owed {
    screen: false,
    echo: false,
};

#[test]
fn a_burst_of_output_waits_for_the_collect_window_and_becomes_one_frame() {
    let mut frames = FramePacer::default();
    let interval = Duration::from_millis(20);
    let start = Instant::now();
    assert_eq!(frames.due(NOTHING, start, interval), None);

    // The first byte of a redraw starts the window; later ones do not move it.
    let due = frames.due(SCREEN, start, interval).unwrap();
    assert_eq!(due, start + FRAME_MINDELAY);
    let later = start + Duration::from_millis(5);
    assert_eq!(frames.due(SCREEN, later, interval), Some(due));
}

#[test]
fn frames_after_the_first_are_held_to_the_frame_interval() {
    let mut frames = FramePacer::default();
    let interval = Duration::from_millis(40);
    let start = Instant::now();
    frames.due(SCREEN, start, interval);
    frames.sent(start + FRAME_MINDELAY);

    // A program still writing is sent at the link's pace, not its own.
    let next = start + Duration::from_millis(10);
    assert_eq!(
        frames.due(SCREEN, next, interval),
        Some(start + FRAME_MINDELAY + interval)
    );
    // After a quiet spell the collect window is all that is left.
    let quiet = start + Duration::from_secs(1);
    frames.settled();
    assert_eq!(frames.due(NOTHING, quiet, interval), None);
    assert_eq!(
        frames.due(SCREEN, quiet, interval),
        Some(quiet + FRAME_MINDELAY)
    );
}

#[test]
fn an_echo_acknowledgement_waits_for_a_screen_change_to_ride_with() {
    let mut frames = FramePacer::default();
    let interval = Duration::from_millis(20);
    let start = Instant::now();
    // Due on its own, it would wait the whole piggyback window...
    assert_eq!(
        frames.due(ECHO, start, interval),
        Some(start + ECHO_PIGGYBACK)
    );
    // ...but the screen changing inside it brings the frame forward, and the
    // acknowledgement goes with that frame.
    let typed = start + Duration::from_millis(10);
    assert_eq!(
        frames.due(BOTH, typed, interval),
        Some(typed + FRAME_MINDELAY)
    );
}

#[test]
fn an_echo_acknowledgement_alone_still_goes_out() {
    let mut frames = FramePacer::default();
    let interval = Duration::from_millis(20);
    let start = Instant::now();
    let due = frames.due(ECHO, start, interval).unwrap();
    // Waiting does not move it: when typing has stopped, it is sent.
    assert_eq!(
        frames.due(ECHO, start + Duration::from_millis(29), interval),
        Some(due)
    );
    assert!(due <= start + ECHO_PIGGYBACK);
}

#[test]
fn an_owed_frame_is_a_deadline_the_loop_wakes_for() {
    let (transport, terminal, echo) = (
        test_transport(),
        TerminalState::new(24, 80),
        EchoAcknowledgements::default(),
    );
    let due = Instant::now() + FRAME_MINDELAY;
    let sources = WakeSources {
        frame_due: Some(due),
        ..wake_sources(&transport, &terminal, &echo)
    };
    assert_eq!(next_wake(sources).earliest(), Some(due));
}

/// A master that records the sizes it is set to, for checking which resizes
/// reach the program.
struct RecordingMaster(std::sync::Mutex<Vec<(u16, u16)>>);

impl portable_pty::MasterPty for RecordingMaster {
    fn resize(&self, size: PtySize) -> Result<(), anyhow::Error> {
        self.0.lock().unwrap().push((size.rows, size.cols));
        Ok(())
    }
    fn get_size(&self) -> Result<PtySize, anyhow::Error> {
        Ok(PtySize::default())
    }
    fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, anyhow::Error> {
        unimplemented!("not read in this test")
    }
    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, anyhow::Error> {
        unimplemented!("not written in this test")
    }
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<libc::pid_t> {
        None
    }
    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> {
        None
    }
    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

#[test]
fn a_run_of_resizes_reaches_the_program_as_the_last_one() {
    let master = RecordingMaster(std::sync::Mutex::new(Vec::new()));
    let (writes, written) = mpsc::sync_channel(16);
    let (_events_tx, events) = mpsc::sync_channel(1);
    let mut pty = PtyIo::Threads { events, writes };
    let mut terminal = TerminalState::new(24, 80);
    let mut dirty = false;
    let resize = |cols, rows| UserEvent::Resize { cols, rows };
    apply_user_events(
        vec![
            UserEvent::Byte(b'a'),
            resize(80, 24),
            resize(90, 30),
            resize(100, 40),
            UserEvent::Byte(b'b'),
            resize(70, 20),
        ],
        3,
        &master,
        &mut terminal,
        &mut pty,
        &mut dirty,
    )
    .unwrap();
    // A dragged edge: the sizes in between were gone before any byte could
    // see them. The one a byte follows, and the last, are what the program
    // gets — in order with that byte.
    assert_eq!(*master.0.lock().unwrap(), [(40, 100), (20, 70)]);
    assert_eq!(terminal.size(), (20, 70));
    assert!(dirty);
    let order: Vec<_> = written
        .try_iter()
        .map(|write| match write {
            PtyWrite::Bytes(bytes) => String::from_utf8(bytes).unwrap(),
            PtyWrite::InputFrame(frame) => format!("frame {frame}"),
        })
        .collect();
    assert_eq!(order, ["a", "b", "frame 3"]);
}

#[test]
fn a_superseded_resize_is_still_checked() {
    let master = RecordingMaster(std::sync::Mutex::new(Vec::new()));
    let (writes, _written) = mpsc::sync_channel(16);
    let (_events_tx, events) = mpsc::sync_channel(1);
    let mut pty = PtyIo::Threads { events, writes };
    let mut terminal = TerminalState::new(24, 80);
    let mut dirty = false;
    let result = apply_user_events(
        vec![
            UserEvent::Resize { cols: 0, rows: 0 },
            UserEvent::Resize { cols: 80, rows: 24 },
        ],
        1,
        &master,
        &mut terminal,
        &mut pty,
        &mut dirty,
    );
    assert!(
        result.is_err(),
        "an unreasonable size is refused wherever it is"
    );
}

/// End to end through the transport: with the first frame sent but not yet
/// acknowledged, the next is diffed from it, and the client applying it to
/// that state sees the whole screen.
#[test]
fn a_frame_after_an_unacknowledged_one_builds_on_it() {
    let key = [0x47; 16];
    let mut server = ServerTransport::new(Ocb::new(&key).unwrap());
    let mut client = Transport::new_client(Ocb::new(&key).unwrap());
    let mut terminal = TerminalState::new(5, 20);
    let mut peer_states = std::collections::HashMap::from([(0, vt100::Parser::new(5, 20, 0))]);
    let mut deliver = |datagrams: Vec<Vec<u8>>, terminal: &TerminalState| -> (u64, u64) {
        let mut accepted = None;
        for datagram in datagrams {
            if let Some(state) = client.recv_state(&datagram) {
                accepted = Some(state);
            }
        }
        let state = accepted.expect("a complete state");
        let host = moshcatty::pb::HostInstruction::decode_message(&state.diff).unwrap();
        let mut screen = vt100::Parser::from_screen(peer_states[&state.old_num].screen().clone());
        for instruction in host {
            screen.process(&instruction.hoststring);
        }
        assert_eq!(screen.screen().contents(), terminal.screen_contents());
        peer_states.insert(state.new_num, screen);
        (state.old_num, state.new_num)
    };

    terminal.process(b"first");
    let one = queue_frame(&mut server, &terminal, None, 0).expect("a frame");
    terminal.snapshot_for_state(one);
    let (base, num) = deliver(server.tick(), &terminal);
    assert_eq!((base, num), (0, one));

    terminal.process(b"\r\nsecond");
    let two = queue_frame(&mut server, &terminal, None, 0).expect("a frame");
    terminal.snapshot_for_state(two);
    let (base, num) = deliver(server.tick(), &terminal);
    assert_eq!(num, two);
    assert_eq!(
        base, one,
        "the second frame builds on the unacknowledged first"
    );
}

/// `datagram` re-sealed under `seq`, as a peer holding the key would have
/// sent it: the shape of a captured datagram once the sequence numbers
/// around it have moved on.
fn resealed(key: &[u8; 16], datagram: &[u8], seq: u64) -> Vec<u8> {
    let ocb = Ocb::new(key).unwrap();
    let (dir_seq, plaintext) = ocb.open_datagram(datagram).unwrap();
    ocb.seal_datagram((dir_seq & !moshcatty::crypto::SEQ_MASK) | seq, &plaintext)
}

#[test]
fn a_replayed_datagram_older_than_the_window_neither_roams_nor_refreshes_liveness() {
    let key = [0x23; 16];
    let mut server = ServerTransport::new(Ocb::new(&key).unwrap());
    let mut client = moshcatty::transport::Transport::new_client(Ocb::new(&key).unwrap());
    client.set_pending(b"x".to_vec());
    let captured = client.tick().remove(0);
    let home: SocketAddr = "192.0.2.1:60001".parse().unwrap();
    let attacker: SocketAddr = "198.51.100.7:4444".parse().unwrap();
    let mut peer = None;

    let first = server.receive(&captured).unwrap();
    roam(&mut peer, &first, home);
    assert_eq!(peer, Some(home));
    // The client goes on talking from home long enough for the captured
    // datagram's sequence number to leave any replay memory.
    let captured_seq =
        u64::from_be_bytes(captured[..8].try_into().unwrap()) & moshcatty::crypto::SEQ_MASK;
    for seq in captured_seq + 1..=captured_seq + 1100 {
        let outcome = server.receive(&resealed(&key, &captured, seq)).unwrap();
        assert!(outcome.authenticated && outcome.in_order);
        roam(&mut peer, &outcome, home);
    }
    let heard = server.last_recv();
    std::thread::sleep(Duration::from_millis(5));

    let replay = server.receive(&captured).unwrap();
    roam(&mut peer, &replay, attacker);

    assert!(!replay.in_order, "a replay must never count as in order");
    assert_eq!(peer, Some(home), "a replayed datagram moved the peer");
    assert_eq!(server.last_recv(), heard, "a replay refreshed liveness");
}

#[test]
fn a_reordered_datagram_is_accepted_but_neither_roams_nor_refreshes_liveness() {
    let key = [0x24; 16];
    let mut server = ServerTransport::new(Ocb::new(&key).unwrap());
    let mut client = moshcatty::transport::Transport::new_client(Ocb::new(&key).unwrap());
    client.set_pending(b"x".to_vec());
    let datagram = client.tick().remove(0);
    let home: SocketAddr = "192.0.2.1:60001".parse().unwrap();
    let elsewhere: SocketAddr = "198.51.100.7:4444".parse().unwrap();
    let mut peer = None;

    // Sequence 10 overtakes sequence 5.
    let newer = server.receive(&resealed(&key, &datagram, 10)).unwrap();
    roam(&mut peer, &newer, home);
    assert!(newer.state.is_some());
    let heard = server.last_recv();
    std::thread::sleep(Duration::from_millis(5));

    let older = server.receive(&resealed(&key, &datagram, 5)).unwrap();
    roam(&mut peer, &older, elsewhere);

    assert!(older.authenticated, "legitimate reordering is still opened");
    assert!(!older.in_order);
    assert_eq!(peer, Some(home));
    assert_eq!(server.last_recv(), heard);
}

#[test]
fn an_in_order_datagram_from_a_new_address_roams() {
    let key = [0x25; 16];
    let mut server = ServerTransport::new(Ocb::new(&key).unwrap());
    let mut client = moshcatty::transport::Transport::new_client(Ocb::new(&key).unwrap());
    client.set_pending(b"x".to_vec());
    let datagram = client.tick().remove(0);
    let home: SocketAddr = "192.0.2.1:60001".parse().unwrap();
    let roamed: SocketAddr = "203.0.113.9:60001".parse().unwrap();
    let mut peer = None;

    roam(
        &mut peer,
        &server.receive(&resealed(&key, &datagram, 1)).unwrap(),
        home,
    );
    // A datagram that completes no new state still roams, as in stock.
    let next = server.receive(&resealed(&key, &datagram, 2)).unwrap();
    assert!(next.state.is_none());
    roam(&mut peer, &next, roamed);

    assert_eq!(peer, Some(roamed));
}
