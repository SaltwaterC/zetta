use super::*;

#[test]
fn publishing_pty_event_preserves_wakeup_before_park() {
    // Use a fresh thread so another test cannot leave a park token behind.
    thread::spawn(|| {
        let (sender, receiver) = mpsc::sync_channel(1);
        let events = PtyEventSender {
            sender,
            consumer: thread::current(),
        };
        events.send(PtyEvent::InputWritten(1)).unwrap();
        let started = Instant::now();
        thread::park_timeout(Duration::from_secs(2));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(receiver.try_recv(), Ok(PtyEvent::InputWritten(1))));
    })
    .join()
    .unwrap();
}

#[test]
fn bounded_pty_drain_reports_remaining_work() {
    let (sender, receiver) = mpsc::sync_channel(128);
    let (writes, _write_rx) = mpsc::sync_channel(1);
    for frame in 1..=128 {
        sender.send(PtyEvent::InputWritten(frame)).unwrap();
    }
    let mut terminal = TerminalState::new(24, 80);
    let mut responder = QueryResponder::new();
    let mut echo = EchoAcknowledgements::default();
    let progress = drain_pty_events(
        &receiver,
        &mut terminal,
        &mut responder,
        &writes,
        &mut echo,
        false,
    )
    .unwrap();
    assert!(progress.budget_exhausted);
    assert!(receiver.try_recv().is_ok());
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

struct ControlledWriter {
    entered: SyncSender<()>,
    release: Receiver<()>,
}

#[test]
fn unrelated_or_partial_output_does_not_confirm_recent_input() {
    let (event_tx, event_rx) = mpsc::sync_channel(4);
    let (write_tx, _write_rx) = mpsc::sync_channel(4);
    let mut terminal = TerminalState::new(24, 80);
    let mut responder = QueryResponder::new();
    let mut echo = EchoAcknowledgements::default();
    // A future timestamp makes this test independent of scheduler pauses.
    let written_at = Instant::now() + Duration::from_secs(60);
    echo.written(9, written_at);
    for bytes in [b"old output".as_slice(), b"\x1b[", b"K"] {
        event_tx.send(PtyEvent::Output(bytes.to_vec())).unwrap();
        let progress = drain_pty_events(
            &event_rx,
            &mut terminal,
            &mut responder,
            &write_tx,
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

impl Write for ControlledWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.entered.send(()).unwrap();
        self.release.recv().unwrap();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn input_completion_follows_actual_write_not_enqueue_or_unrelated_output() {
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let (write_tx, write_rx) = mpsc::sync_channel(4);
    let (event_tx, event_rx) = mpsc::sync_channel(4);
    spawn_pty_writer(
        Box::new(ControlledWriter {
            entered: entered_tx,
            release: release_rx,
        }),
        write_rx,
        PtyEventSender {
            sender: event_tx.clone(),
            consumer: thread::current(),
        },
    );
    queue_pty_write(&write_tx, b"x".to_vec()).unwrap();
    queue_pty_request(&write_tx, PtyWrite::InputFrame(9)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    event_tx
        .send(PtyEvent::Output(b"old output".to_vec()))
        .unwrap();
    assert!(matches!(event_rx.try_recv(), Ok(PtyEvent::Output(_))));
    assert!(matches!(event_rx.try_recv(), Err(TryRecvError::Empty)));
    release_tx.send(()).unwrap();
    assert!(matches!(
        event_rx.recv_timeout(Duration::from_secs(5)),
        Ok(PtyEvent::InputWritten(9))
    ));
}

#[test]
fn coalesced_screen_update_retains_echo_ack() {
    let key = [0x36; 16];
    let mut server = ServerTransport::new(Ocb::new(&key).unwrap());
    let mut client = Transport::new_client(Ocb::new(&key).unwrap());
    let mut terminal = TerminalState::new(24, 80);
    terminal.process(b"x");
    server.set_pending(host_update(&terminal, 7).unwrap());
    // A second PTY chunk arrives before tick sends the first update. The SSP
    // transport discards that unsent state, including its echo instruction.
    terminal.process(b"y");
    server.set_pending(host_update(&terminal, 7).unwrap());
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
