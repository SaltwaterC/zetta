use super::*;
use std::time::{Duration, Instant};

/// Waits for the next PTY event, failing after `limit` rather than hanging.
fn next_within(pty: &mut PtyIo, limit: Duration) -> Option<PtyEvent> {
    let started = Instant::now();
    loop {
        pty.flush();
        if let Some(event) = pty.next_event() {
            return Some(event);
        }
        if started.elapsed() >= limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct ControlledWriter {
    entered: SyncSender<()>,
    release: Receiver<()>,
}

impl Write for ControlledWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.entered.send(()).unwrap();
        self.release.recv().unwrap();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The threaded end: an input frame is reported once the writer has really
/// written what came before it, not when it was queued, and output that
/// arrives meanwhile does not stand in for it.
#[test]
fn input_completion_follows_actual_write_not_enqueue_or_unrelated_output() {
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let (writes, write_rx) = mpsc::sync_channel(4);
    let (event_tx, events) = mpsc::sync_channel(4);
    spawn_pty_writer(
        Box::new(ControlledWriter {
            entered: entered_tx,
            release: release_rx,
        }),
        write_rx,
        WakingSender::to_current(event_tx.clone()),
    );
    let mut pty = PtyIo::Threads { events, writes };
    pty.queue(PtyWrite::Bytes(b"x".to_vec())).unwrap();
    pty.queue(PtyWrite::InputFrame(9)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    event_tx
        .send(PtyEvent::Output(b"old output".to_vec()))
        .unwrap();
    assert!(matches!(pty.next_event(), Some(PtyEvent::Output(_))));
    assert!(pty.next_event().is_none());
    release_tx.send(()).unwrap();
    assert!(matches!(
        next_within(&mut pty, Duration::from_secs(5)),
        Some(PtyEvent::InputWritten(9))
    ));
}

#[cfg(unix)]
mod direct {
    use super::*;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    struct Program {
        child: Box<dyn portable_pty::Child + Send + Sync>,
        /// Held only to keep the descriptor the `PtyIo` borrows open.
        _master: Box<dyn MasterPty + Send>,
    }

    impl Drop for Program {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn spawn(script: &str) -> (Program, PtyIo) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", script]);
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let io = PtyIo::open(pair.master.as_ref(), &Waker::current_thread()).unwrap();
        assert!(
            matches!(io, PtyIo::Direct(_)),
            "a native Unix PTY is read directly"
        );
        (
            Program {
                child,
                _master: pair.master,
            },
            io,
        )
    }

    fn output_until(pty: &mut PtyIo, wanted: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !output.windows(wanted.len()).any(|window| window == wanted) {
            assert!(
                Instant::now() < deadline,
                "never saw {wanted:?} in {output:?}"
            );
            match next_within(pty, Duration::from_millis(50)) {
                Some(PtyEvent::Output(bytes)) => output.extend(bytes),
                Some(PtyEvent::Eof) => panic!("ended before {wanted:?}: {output:?}"),
                _ => {}
            }
        }
        output
    }

    #[test]
    fn the_loop_reads_what_the_program_writes_and_sees_it_end() {
        let (_program, mut pty) = spawn("printf direct-output");
        output_until(&mut pty, b"direct-output");
        let ended = (0..250).any(|_| {
            matches!(
                next_within(&mut pty, Duration::from_millis(20)),
                Some(PtyEvent::Eof | PtyEvent::Error(_))
            )
        });
        assert!(ended, "the end of the program was not reported");
        // An ended PTY is neither read again nor watched.
        assert!(pty.next_event().is_none());
        let PtyIo::Direct(direct) = &pty else {
            unreachable!()
        };
        assert!(direct.interest(true).is_none());
    }

    #[test]
    fn an_input_frame_is_reported_once_its_bytes_have_reached_the_program() {
        let (_program, mut pty) = spawn("stty raw -echo; cat");
        std::thread::sleep(Duration::from_millis(100));
        pty.queue(PtyWrite::Bytes(b"typed\n".to_vec())).unwrap();
        pty.queue(PtyWrite::InputFrame(4)).unwrap();
        let mut saw_written = false;
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(saw_written && output.windows(5).any(|window| window == b"typed")) {
            assert!(
                Instant::now() < deadline,
                "output {output:?}, written {saw_written}"
            );
            match next_within(&mut pty, Duration::from_millis(50)) {
                Some(PtyEvent::InputWritten(4)) => saw_written = true,
                Some(PtyEvent::Output(bytes)) => output.extend(bytes),
                _ => {}
            }
        }
    }

    #[test]
    fn a_program_that_does_not_read_holds_its_input_frame_back() {
        // The PTY's input queue is finite and `sleep` never empties it, so
        // the bytes cannot all go out and the frame after them must wait.
        let (_program, mut pty) = spawn("stty raw -echo; sleep 30");
        std::thread::sleep(Duration::from_millis(100));
        pty.queue(PtyWrite::Bytes(vec![b'x'; 1 << 20])).unwrap();
        pty.queue(PtyWrite::InputFrame(5)).unwrap();
        for _ in 0..20 {
            assert!(
                !matches!(
                    next_within(&mut pty, Duration::from_millis(5)),
                    Some(PtyEvent::InputWritten(_))
                ),
                "the frame was reported before its bytes were written"
            );
        }
        // And the loop is told to wait for the PTY to take more.
        let PtyIo::Direct(direct) = &pty else {
            unreachable!()
        };
        let interest = direct.interest(false).expect("writes are pending");
        assert_ne!(interest.events & libc::POLLOUT, 0);
    }

    #[test]
    fn a_full_queue_is_reported_rather_than_grown() {
        let (_program, mut pty) = spawn("stty raw -echo; sleep 30");
        std::thread::sleep(Duration::from_millis(100));
        let mut refused = false;
        for _ in 0..=PTY_QUEUE_DEPTH + 1 {
            if pty.queue(PtyWrite::Bytes(vec![b'x'; 64 * 1024])).is_err() {
                refused = true;
                break;
            }
        }
        assert!(refused, "the queue grew past its depth");
    }

    #[test]
    fn output_ends_the_loops_wait_and_a_held_program_does_not() {
        let (_program, pty) = spawn("sleep 0.3; printf late; sleep 30");
        let mut wait = LoopWait::new().unwrap();
        let udp = UdpIo::new(UdpSocket::bind("127.0.0.1:0").unwrap(), &wait.waker()).unwrap();
        let far = {
            let mut deadline = WakeDeadline::default();
            deadline.at(Some(Instant::now() + Duration::from_secs(10)));
            deadline
        };
        // Held: the output arrives, but the wait runs until the deadline.
        let short = {
            let mut deadline = WakeDeadline::default();
            deadline.at(Some(Instant::now() + Duration::from_millis(600)));
            deadline
        };
        let started = Instant::now();
        wait.wait(short, &udp, Some(&pty), false).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(550));
        // Reading: the output that is already there ends the wait at once.
        let started = Instant::now();
        wait.wait(far, &udp, Some(&pty), true).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

#[cfg(unix)]
#[test]
fn a_waker_on_another_thread_ends_the_loops_wait() {
    let mut wait = LoopWait::new().unwrap();
    let udp = UdpIo::new(UdpSocket::bind("127.0.0.1:0").unwrap(), &wait.waker()).unwrap();
    let waker = wait.waker();
    let raiser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        waker.wake();
    });
    let mut deadline = WakeDeadline::default();
    deadline.at(Some(Instant::now() + Duration::from_secs(10)));
    let started = Instant::now();
    wait.wait(deadline, &udp, None, true).unwrap();
    raiser.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));

    // Drained by that wait, so the next one runs to its deadline.
    let mut deadline = WakeDeadline::default();
    deadline.at(Some(Instant::now() + Duration::from_millis(50)));
    let started = Instant::now();
    wait.wait(deadline, &udp, None, true).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(40));
}

#[test]
fn a_datagram_ends_the_wait_and_is_read_without_blocking() {
    let mut wait = LoopWait::new().unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    let mut udp = UdpIo::new(socket, &wait.waker()).unwrap();
    let mut buffer = [0_u8; 64];
    assert!(udp.recv(&mut buffer).unwrap().is_none());

    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    sender.send_to(b"frame", address).unwrap();
    let mut deadline = WakeDeadline::default();
    deadline.at(Some(Instant::now() + Duration::from_secs(10)));
    let started = Instant::now();
    let received = loop {
        if let Some(received) = udp.recv(&mut buffer).unwrap() {
            break received;
        }
        wait.wait(deadline, &udp, None, true).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "never woken");
    };
    assert_eq!(&buffer[..received.0], b"frame");
    assert_eq!(received.1, sender.local_addr().unwrap());
}
