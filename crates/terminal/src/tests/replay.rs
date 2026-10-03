use super::*;
use gpui::{TestAppContext, VisualContext as _, bounds, size};

fn geometry(columns: usize, lines: usize) -> TerminalBounds {
    TerminalBounds::new(
        px(10.),
        px(8.),
        bounds(
            GpuiPoint::default(),
            size(px(columns as f32 * 8.), px(lines as f32 * 10.)),
        ),
    )
}

fn builder(cx: &mut TestAppContext, bytes: Vec<u8>) -> TerminalBuilder {
    cx.update(|cx| {
        TerminalSettings::init(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
    });
    TerminalBuilder::new_display_only(
        SettingsCursorShape::default(),
        AlternateScroll::On,
        Some(4000),
        0,
        &cx.background_executor,
        PathStyle::local(),
    )
    .with_replay(bytes)
}

#[gpui::test]
fn replay_keeps_the_live_grid_available_and_applies_resize_generations(cx: &mut TestAppContext) {
    let bytes = format!("{}\r\nsaved tail\r\n", "long wrapped line ".repeat(100)).into_bytes();
    let mut builder = builder(cx, bytes.clone());
    let (release, gate) = async_channel::bounded(1);
    builder.terminal.replay_job.gate = Some(gate);
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| builder.subscribe(cx));
    let mut expected = terminal.read_with(window, |terminal, _| terminal.term.lock().clone());
    resize(&mut expected, geometry(80, 24), true);
    let mut processor = Processor::<StdSyncHandler>::new();
    processor.advance(&mut expected, &bytes);

    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(geometry(80, 24));
        terminal.sync(window, cx);
        assert!(terminal.replay_job.is_running());
        assert!(!terminal.get_content().contains("saved tail"));
    });
    window.run_until_parked();
    for columns in [31, 140, 53] {
        window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.set_size(geometry(columns, 12));
            terminal.sync(window, cx);
            assert_eq!(terminal.term.lock().columns(), columns);
            assert!(!terminal.get_content().contains("saved tail"));
        });
        resize(&mut expected, geometry(columns, 12), true);
    }
    terminal.update(window, |terminal, cx| {
        terminal.write_output(b"live after replay", cx)
    });
    processor.advance(&mut expected, b"live after replay");
    // A size request not yet drained by sync is also a newer generation.
    terminal.update(window, |terminal, _| terminal.set_size(geometry(67, 15)));
    resize(&mut expected, geometry(67, 15), true);
    release.try_send(()).unwrap();
    window.run_until_parked();
    terminal.read_with(window, |terminal, _| {
        assert!(
            !terminal.replay_job.is_running(),
            "completion needs no timer or extra layout"
        );
        let term = terminal.term.lock();
        assert_eq!(term.columns(), 67);
        assert_eq!(term.screen_lines(), 15);
        assert_eq!(content_text(&term), content_text(&expected));
        assert_eq!(term.grid().cursor.point, expected.grid().cursor.point);
        assert_eq!(
            snapshot::ansi_snapshot(&term, 4000),
            snapshot::ansi_snapshot(&expected, 4000)
        );
    });
}

#[gpui::test]
fn alternate_screen_replay_and_live_output_publish_in_order(cx: &mut TestAppContext) {
    let mut builder = builder(
        cx,
        b"primary\x1b[?1049h\x1b[2J\x1b[Hsaved alternate".to_vec(),
    );
    let (release, gate) = async_channel::bounded(1);
    builder.terminal.replay_job.gate = Some(gate);
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| builder.subscribe(cx));
    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(geometry(80, 24));
        terminal.sync(window, cx);
        // Superseded sizes must not truncate the alternate screen on its way
        // to the final size. Output starts a new resize generation boundary.
        terminal.set_size(geometry(5, 24));
        terminal.set_size(geometry(80, 24));
        terminal.write_output(b" and live", cx);
    });
    window.run_until_parked();
    release.try_send(()).unwrap();
    window.run_until_parked();
    terminal.update(window, |terminal, cx| {
        assert!(
            terminal
                .term
                .lock()
                .mode()
                .contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
        );
        assert!(terminal.get_content().contains("saved alternate and live"));
        terminal.write_output(b"\x1b[?1049l", cx);
        assert!(terminal.get_content().contains("primary"));
    });
}

#[gpui::test]
fn maximum_accepted_snapshot_is_parsed_after_sync_returns(cx: &mut TestAppContext) {
    // zmux::retention::MAX_SNAPSHOT_BYTES. Keep the crate independent of zmux.
    let mut bytes = vec![b'x'; 8 * 1024 * 1024];
    let tail = b"\r\nmaximum replay tail\r\n";
    let start = bytes.len() - tail.len();
    bytes[start..].copy_from_slice(tail);
    let builder = builder(cx, bytes);
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| builder.subscribe(cx));
    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(geometry(160, 24));
        terminal.sync(window, cx);
        assert!(terminal.replay_job.is_running());
        assert_eq!(terminal.term.lock().history_size(), 0);
        assert!(!terminal.get_content().contains("maximum replay tail"));
    });
    window.run_until_parked();
    terminal.read_with(window, |terminal, _| {
        assert!(!terminal.replay_job.is_running());
        assert!(terminal.get_content().contains("maximum replay tail"));
    });
}

#[gpui::test]
fn fresh_shell_input_waits_for_publication_and_typing_cancels_prefill(cx: &mut TestAppContext) {
    for typed in [false, true] {
        let mut builder = builder(cx, b"\x1b[?1049hsaved screen".to_vec())
            .with_fresh_shell_restore()
            .with_restore_prefill(Some("saved command".into()));
        let (release, gate) = async_channel::bounded(1);
        builder.terminal.replay_job.gate = Some(gate);
        let window = cx.add_empty_window();
        let terminal = window.new(|cx| builder.subscribe(cx));
        window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.set_size(geometry(80, 24));
            terminal.sync(window, cx);
            assert!(!terminal.replay_job.is_running(), "startup is not ready");
            terminal.finish_fresh_shell_restore(cx);
            terminal.sync(window, cx);
            assert!(terminal.replay_job.is_running());
            terminal.finish_fresh_shell_restore(cx);
            assert!(terminal.pty_write_log.borrow().is_empty());
            if typed {
                terminal.input(b"typed command".as_slice());
            }
        });
        window.run_until_parked();
        release.try_send(()).unwrap();
        window.run_until_parked();
        terminal.read_with(window, |terminal, _| {
            assert!(!terminal.replay_job.is_running());
            assert!(terminal.get_content().contains("saved screen"));
            assert_eq!(
                *terminal.term.lock().mode(),
                alacritty_terminal::term::TermMode::default()
            );
            let expected = if typed {
                b"typed command".as_slice()
            } else {
                b"\rsaved command".as_slice()
            };
            assert_eq!(terminal.pty_write_log.borrow().concat(), expected);
        });
    }
}

#[gpui::test]
fn dropping_a_running_replay_does_not_keep_the_terminal_alive(cx: &mut TestAppContext) {
    let mut builder = builder(cx, b"saved screen".to_vec());
    let (release, gate) = async_channel::bounded(1);
    builder.terminal.replay_job.gate = Some(gate);
    let barrier = builder.terminal.replay_barrier.clone();
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| builder.subscribe(cx));
    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(geometry(80, 24));
        terminal.sync(window, cx);
    });
    window.run_until_parked();
    let weak = terminal.downgrade();
    drop(terminal);
    window.update(|_, _| {});
    assert!(weak.upgrade().is_none());
    assert!(!barrier.wait());
    release.try_send(()).unwrap();
    window.run_until_parked();
    assert!(
        !barrier.wait(),
        "completion cannot reopen an aborted reader"
    );
}

#[gpui::test]
fn live_reader_waits_until_the_private_replay_is_published(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut builder = builder(cx, b"saved\r\n".to_vec());
    builder.terminal.byte_stream = Some(spawn_byte_stream(
        Box::new(std::io::Cursor::new(b"live\r\n")),
        Box::new(std::io::sink()),
        builder.terminal.term.clone(),
        builder.events_tx.clone(),
        builder.terminal.wakeup_gate.clone(),
        builder.terminal.replay_barrier.clone(),
        true,
    ));
    let (release, gate) = async_channel::bounded(1);
    builder.terminal.replay_job.gate = Some(gate);
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| builder.subscribe(cx));
    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(geometry(80, 24));
        terminal.sync(window, cx);
    });
    window.run_until_parked();
    terminal.read_with(window, |terminal, _| {
        assert!(terminal.replay_job.is_running());
        assert!(!terminal.get_content().contains("live"));
        assert!(matches!(
            terminal.byte_stream.as_ref().unwrap().finished.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    });
    release.try_send(()).unwrap();
    window.run_until_parked();
    terminal.read_with(window, |terminal, _| {
        assert!(!matches!(
            terminal
                .byte_stream
                .as_ref()
                .unwrap()
                .finished
                .recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(terminal.get_content().starts_with("saved\nlive\n"));
    });
}

#[gpui::test]
fn replay_backend_events_wait_for_the_published_grid(cx: &mut TestAppContext) {
    let mut builder = builder(cx, b"saved screen".to_vec());
    let (release, gate) = async_channel::bounded(1);
    builder.terminal.replay_job.gate = Some(gate);
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| builder.subscribe(cx));
    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(geometry(80, 24));
        terminal.sync(window, cx);
        // A parser event can arrive before parsing the rest of the replay.
        // Title handlers may trigger application work; color and cursor events
        // also read the grid, which must be the restored one by then.
        terminal
            .events_tx
            .unbounded_send(PtyEvent::Event(TerminalBackendEvent::Title(
                "replayed title".into(),
            )))
            .unwrap();
    });
    window.run_until_parked();
    terminal.read_with(window, |terminal, _| {
        assert!(terminal.breadcrumb_text.is_empty())
    });
    release.try_send(()).unwrap();
    window.run_until_parked();
    terminal.read_with(window, |terminal, _| {
        assert_eq!(terminal.breadcrumb_text, "replayed title");
        assert!(terminal.get_content().contains("saved screen"));
    });
}

#[gpui::test]
#[ignore = "manual optimized-build replay latency probe"]
#[expect(
    clippy::assertions_on_constants,
    reason = "the ignored probe must compile in debug suites but reject debug measurements at runtime"
)]
fn replay_latency_probe(cx: &mut TestAppContext) {
    assert!(!cfg!(debug_assertions), "run with --release");
    for _ in 0..5 {
        let bytes = vec![b'x'; 8 * 1024 * 1024];
        let builder = builder(cx, bytes.clone());
        let mut reference = builder.terminal.term.lock().clone();
        resize(&mut reference, geometry(160, 24), true);
        let started = Instant::now();
        Processor::<StdSyncHandler>::new().advance(&mut reference, &bytes);
        let foreground_parse = started.elapsed();
        let window = cx.add_empty_window();
        let terminal = window.new(|cx| builder.subscribe(cx));
        let started = Instant::now();
        let sync = window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.set_size(geometry(160, 24));
            let started = Instant::now();
            terminal.sync(window, cx);
            started.elapsed()
        });
        window.run_until_parked();
        let completion = started.elapsed();
        terminal.read_with(window, |terminal, _| {
            assert!(!terminal.replay_job.is_running());
            assert_eq!(terminal.get_content(), content_text(&reference));
        });
        eprintln!(
            "replay_probe bytes={} foreground_parse_us={} sync_us={} completion_us={}",
            bytes.len(),
            foreground_parse.as_micros(),
            sync.as_micros(),
            completion.as_micros()
        );
    }
}
