//! Real ConPTY output checks and optimized benchmarks, independent of GPUI rendering.

use super::*;
use crate::index::Column;
use crate::term::{Config, test::TermSize};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

static NEXT_PAYLOAD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[derive(Clone)]
struct Listener(Sender<event::Event>);

impl EventListener for Listener {
    fn send_event(&self, event: event::Event) {
        if !matches!(event, event::Event::Wakeup | event::Event::Bell) {
            let _ = self.0.send(event);
        }
    }
}

fn run_payload(payload: &[u8], hold_grid: bool) -> (String, State, Duration) {
    let sequence = NEXT_PAYLOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory =
        std::env::temp_dir().join(format!("zetta-conpty-{}-{sequence}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let script = directory.join("producer.ps1");
    let path = directory.join("payload.bin");
    std::fs::write(&script, include_str!("event_loop_windows/producer.ps1")).unwrap();
    std::fs::write(&path, payload).unwrap();
    let (sent, received) = mpsc::channel();
    let listener = Listener(sent);
    let config = Config { scrolling_history: 500, ..Config::default() };
    let terminal =
        Arc::new(FairMutex::new(Term::new(config, &TermSize::new(80, 24), listener.clone())));
    let options = tty::Options {
        shell: Some(tty::Shell::new(
            "powershell.exe".into(),
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-File".into(),
                script.to_string_lossy().into_owned(),
                path.to_string_lossy().into_owned(),
            ],
        )),
        escape_args: true,
        ..tty::Options::default()
    };
    let size = WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 };
    let started = Instant::now();
    let pty = tty::new(&options, size, 0).unwrap();
    let child = hold_grid.then(|| {
        // SAFETY: the PID belongs to the child this test just started. The
        // synchronization-only handle is owned and closed exactly once.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pty.child_pid()) };
        assert!(!handle.is_null());
        unsafe { OwnedHandle::from_raw_handle(handle.cast()) }
    });
    let event_loop =
        EventLoop::new(terminal.clone(), listener, pty, true, false, ReplayBarrier::open())
            .unwrap();
    let sender = event_loop.channel();
    let grid = terminal.lock();
    let worker = event_loop.spawn();
    let mut pending = VecDeque::new();
    if let Some(child) = child {
        // Prove the child has actually exited while parsing is blocked. A
        // fixed sleep can expire before PowerShell finishes compiling its
        // producer, leaving the final-drain assertion unexercised.
        let status = unsafe { WaitForSingleObject(child.as_raw_handle().cast(), 10_000) };
        assert_eq!(status, WAIT_OBJECT_0, "producer did not exit while the grid was locked");
        std::thread::sleep(Duration::from_millis(100));
        pending.extend(received.try_iter());
        assert!(
            !pending.iter().any(|event| matches!(event, event::Event::ChildExit(_))),
            "child exit was reported before queued output could reach the grid"
        );
    }
    drop(grid);
    loop {
        let event = pending.pop_front().unwrap_or_else(|| {
            received.recv_timeout(Duration::from_secs(30)).expect("ConPTY child did not exit")
        });
        match event {
            event::Event::ChildExit(status) => {
                assert!(status.success());
                break;
            },
            event::Event::BackendShutdown | event::Event::ChildExitStatusUnavailable => {
                panic!("ConPTY failed")
            },
            event::Event::PtyWrite(reply) => {
                sender.send(Msg::Input(reply.into_bytes().into())).unwrap();
            },
            event::Event::TextAreaSizeRequest(reply) => {
                sender.send(Msg::Input(reply(size).into_bytes().into())).unwrap();
            },
            _ => {},
        }
    }
    // Capture the grid before joining: exit consumers can drop their PTY as
    // soon as the report arrives, so joining first could conceal a late drain.
    let text = {
        let terminal = terminal.lock();
        let line = terminal.grid().cursor.point.line;
        (0..80).map(|column| terminal.grid()[line][Column(column)].c).collect()
    };
    let elapsed = started.elapsed();
    let (event_loop, state) = worker.join().unwrap();
    drop(event_loop);
    std::fs::remove_file(script).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
    (text, state, elapsed)
}

#[test]
#[ignore = "manual optimized-build ConPTY throughput check"]
fn ascii_conpty_throughput_benchmark() {
    let line = (0..80u8).map(|index| b' ' + index % 95).chain(*b"\n").collect::<Vec<_>>();
    let payload = line.repeat(256 * 1024 * 1024 / line.len());
    let (_, state, elapsed) = run_payload(&payload, false);
    eprintln!(
        "ASCII: {:.1} MiB/s, {} reads, {:.3}s parsing",
        256.0 / elapsed.as_secs_f64(),
        state.profile.read_calls,
        state.profile.parse_ns as f64 / 1e9
    );
}

#[test]
#[ignore = "manual optimized-build ConPTY throughput check"]
fn random_conpty_throughput_benchmark() {
    let mut seed = 0x5117_u64;
    let payload = (0..32 * 1024 * 1024)
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 56) as u8
        })
        .collect::<Vec<_>>();
    let (_, state, elapsed) = run_payload(&payload, false);
    eprintln!(
        "random: {:.1} MiB/s, {} reads, {:.3}s parsing",
        32.0 / elapsed.as_secs_f64(),
        state.profile.read_calls,
        state.profile.parse_ns as f64 / 1e9
    );
}

#[test]
fn final_conpty_output_is_drained_before_child_exit() {
    let mut payload = b"x\r\n".repeat(256 * 1024);
    payload.extend_from_slice(b"the end");
    let (text, _, _) = run_payload(&payload, true);
    assert_eq!(text.trim_end(), "the end");
}

#[test]
fn a_surviving_writer_cannot_extend_the_final_drain_indefinitely() {
    struct WritingPty {
        output: io::Take<io::Repeat>,
        input: io::Sink,
    }
    impl event::OnResize for WritingPty {
        fn on_resize(&mut self, _: WindowSize) {}
    }
    impl tty::EventedReadWrite for WritingPty {
        type Reader = io::Take<io::Repeat>;
        type Writer = io::Sink;
        unsafe fn register(
            &mut self,
            _: &Arc<Poller>,
            _: PollingEvent,
            _: polling::PollMode,
        ) -> io::Result<()> {
            Ok(())
        }
        fn reregister(
            &mut self,
            _: &Arc<Poller>,
            _: PollingEvent,
            _: polling::PollMode,
        ) -> io::Result<()> {
            Ok(())
        }
        fn deregister(&mut self, _: &Arc<Poller>) -> io::Result<()> {
            Ok(())
        }
        fn reader(&mut self) -> &mut Self::Reader {
            &mut self.output
        }
        fn writer(&mut self) -> &mut Self::Writer {
            &mut self.input
        }
    }
    impl tty::EventedPty for WritingPty {
        fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
            None
        }
    }
    let (sent, _) = mpsc::channel();
    let listener = Listener(sent);
    let terminal = Arc::new(FairMutex::new(Term::new(
        Config { scrolling_history: 2, ..Config::default() },
        &TermSize::new(80, 24),
        listener.clone(),
    )));
    // Twice the final-drain cap, finite so removing the cap fails an assertion
    // instead of hanging the test suite.
    let pty = WritingPty {
        output: io::repeat(b'x').take(8 * READ_BUFFER_SIZE as u64),
        input: io::sink(),
    };
    let mut event_loop =
        EventLoop::new(terminal.clone(), listener.clone(), pty, true, false, ReplayBarrier::open())
            .unwrap();
    let mut parser =
        ParserThread::spawn(terminal, listener, ReplayBarrier::open(), ansi::Processor::new());
    let mut state = State::default();
    event_loop.drain_final_pty(&mut state, &mut parser, None::<&mut io::Sink>);
    state.finish_parsing(&mut parser);
    assert!(
        (4 * READ_BUFFER_SIZE as u64..4 * READ_BUFFER_SIZE as u64 + READ_CHUNK_SIZE as u64)
            .contains(&state.profile.bytes)
    );
}

/// A PTY whose finite output, once read, is known to have ended.
struct EndingPty {
    output: io::Cursor<Vec<u8>>,
    input: io::Sink,
}

impl event::OnResize for EndingPty {
    fn on_resize(&mut self, _: WindowSize) {}
}

impl tty::EventedReadWrite for EndingPty {
    type Reader = io::Cursor<Vec<u8>>;
    type Writer = io::Sink;
    unsafe fn register(
        &mut self,
        _: &Arc<Poller>,
        _: PollingEvent,
        _: polling::PollMode,
    ) -> io::Result<()> {
        Ok(())
    }
    fn reregister(
        &mut self,
        _: &Arc<Poller>,
        _: PollingEvent,
        _: polling::PollMode,
    ) -> io::Result<()> {
        Ok(())
    }
    fn deregister(&mut self, _: &Arc<Poller>) -> io::Result<()> {
        Ok(())
    }
    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.output
    }
    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.input
    }
}

impl tty::EventedPty for EndingPty {
    fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
        None
    }

    fn output_ended(&self) -> bool {
        self.output.position() == self.output.get_ref().len() as u64
    }
}

#[test]
fn the_final_drain_stops_at_the_end_of_output_without_a_quiet_period() {
    let (sent, _) = mpsc::channel();
    let listener = Listener(sent);
    let terminal = Arc::new(FairMutex::new(Term::new(
        Config::default(),
        &TermSize::new(80, 24),
        listener.clone(),
    )));
    let pty = EndingPty { output: io::Cursor::new(b"the end".to_vec()), input: io::sink() };
    let mut event_loop =
        EventLoop::new(terminal.clone(), listener.clone(), pty, true, false, ReplayBarrier::open())
            .unwrap();
    let mut parser =
        ParserThread::spawn(terminal, listener, ReplayBarrier::open(), ansi::Processor::new());
    let mut state = State::default();
    let started = Instant::now();
    event_loop.drain_final_pty(&mut state, &mut parser, None::<&mut io::Sink>);
    let elapsed = started.elapsed();
    state.finish_parsing(&mut parser);
    assert_eq!(state.profile.bytes, 7);
    // Without the end-of-output check the drain always waits out the quiet period.
    assert!(elapsed < Duration::from_millis(20), "the drain waited {elapsed:?} after the end");
}

#[test]
fn a_released_console_ends_its_output_when_the_child_exits() {
    if !tty::windows::releases_consoles() {
        eprintln!("skipped: this ConPTY cannot release its console");
        return;
    }
    let options = tty::Options {
        shell: Some(tty::Shell::new(
            "cmd.exe".into(),
            vec!["/d".into(), "/q".into(), "/c".into(), "echo released".into()],
        )),
        escape_args: true,
        ..tty::Options::default()
    };
    let size = WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 };
    let mut pty = tty::new(&options, size, 0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    let mut answered = false;
    let mut buffer = [0; 4096];
    while !tty::EventedPty::output_ended(&pty) {
        assert!(Instant::now() < deadline, "the console outlived its child: {output:?}");
        let count = tty::EventedReadWrite::reader(&mut pty).read(&mut buffer).unwrap();
        output.extend_from_slice(&buffer[..count]);
        // ConPTY waits for the terminal's first device-attributes reply.
        if !answered && output.windows(3).any(|window| window == b"\x1b[c") {
            answered = true;
            tty::EventedReadWrite::writer(&mut pty).write_all(b"\x1b[?6c").unwrap();
        }
        if count == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert!(output.windows(8).any(|window| window == b"released"), "{output:?}");
}
