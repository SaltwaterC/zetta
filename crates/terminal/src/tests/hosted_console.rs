use super::*;
use crate::alacritty::{WakeupGate, ZedListener, new_term};
use crate::{PtyEvent, TerminalBackendEvent, TerminalBounds};
use alacritty_terminal::event_loop::{EventLoop, Msg, ReplayBarrier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use std::time::{Duration, Instant};

#[test]
fn native_screen_queries_do_not_wait_for_the_history_worker() {
    let (events, _) = futures::channel::mpsc::unbounded();
    let listener = ZedListener::new(events, WakeupGate::new());
    let term = new_term(
        &alacritty_terminal::term::Config::default(),
        TerminalBounds::default(),
        listener,
        crate::terminal_settings::AlternateScroll::On,
    );
    let (release, gate) = std::sync::mpsc::channel();
    term.lock()
        .resize_with_async_history_for_test(
            alacritty_terminal::term::test::TermSize::new(17, 24),
            move || gate.recv().unwrap(),
        )
        .unwrap();
    let (writer, _reader) = queue::channel();
    let output = Output {
        writer,
        term: Arc::clone(&term),
    };
    let (send, receive) = std::sync::mpsc::channel();
    let query = std::thread::spawn(move || {
        send.send((
            output.screen(),
            output.cursor(),
            output.size(),
            output.read_cells(0..24, 0..17),
        ))
        .unwrap();
    });
    let result = receive.recv_timeout(Duration::from_secs(2));
    // Release before joining even on failure, so a regression fails cleanly.
    release.send(()).unwrap();
    query.join().unwrap();
    let (screen, cursor, size, cells) =
        result.expect("visible console queries waited for off-screen history");
    assert_eq!((screen.columns, screen.rows), (17, 24));
    assert_eq!(cursor, screen.cursor);
    assert_eq!(size, (17, 24));
    assert_eq!(cells.unwrap().len(), 17 * 24);
}

#[test]
fn powershell_accepts_input_after_repeated_local_resizes() {
    assert_powershell_accepts_input_after_resizes("powershell.exe");
}

#[test]
fn powershell_7_accepts_input_after_repeated_local_resizes() {
    if std::process::Command::new("pwsh.exe")
        .args(["-NoProfile", "-Command", "exit"])
        .status()
        .is_err()
    {
        eprintln!("skipped: PowerShell 7 is not installed");
        return;
    }
    assert_powershell_accepts_input_after_resizes("pwsh.exe");
}

fn assert_powershell_accepts_input_after_resizes(program: &str) {
    assert_shell_accepts_input_after_resizes(
        program,
        vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NoExit".into(),
            "-Command".into(),
            "function prompt { 'RESIZE> ' }".into(),
        ],
        b"Write-Output ('INPUT:' + 'abcdefghijklm",
        b"nopqrstuvwxyz0123456789'); exit\r",
    );
}

#[test]
fn cmd_accepts_input_after_repeated_local_resizes() {
    assert_shell_accepts_input_after_resizes(
        "cmd.exe",
        vec!["/d".into(), "/k".into(), "prompt RESIZE$G$S".into()],
        b"echo INPUT:abcdefghijklm",
        b"nopqrstuvwxyz0123456789&exit\r",
    );
}

fn assert_shell_accepts_input_after_resizes(
    program: &str,
    args: Vec<String>,
    before_resize: &[u8],
    after_resize: &[u8],
) {
    let (events, mut received) = futures::channel::mpsc::unbounded();
    let listener = ZedListener::new(events, WakeupGate::new());
    let term = new_term(
        &alacritty_terminal::term::Config::default(),
        TerminalBounds::default(),
        listener.clone(),
        crate::terminal_settings::AlternateScroll::On,
    );
    let options = tty::Options {
        shell: Some(tty::Shell::new(program.into(), args)),
        console_backend: tty::ConsoleBackend::Hosted,
        escape_args: true,
        ..Default::default()
    };
    let pty = open(&options, &term).unwrap();
    let event_loop = EventLoop::new(
        term.clone(),
        listener,
        pty,
        true,
        false,
        ReplayBarrier::open(),
    )
    .unwrap();
    let sender = event_loop.channel();
    let thread = event_loop.spawn();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stage = 0;
    let mut exited = false;
    let (release_reflow, gate_reflow) = std::sync::mpsc::channel();
    let mut gate_reflow = Some(gate_reflow);
    while Instant::now() < deadline {
        while let Ok(PtyEvent::Event(event)) = received.try_recv() {
            if matches!(event, TerminalBackendEvent::ChildExit(_)) {
                exited = true;
            }
        }
        let text = crate::alacritty::content_text(&term.lock());
        if stage == 0 && text.contains("RESIZE>") {
            sender
                .send(Msg::Input(before_resize.to_vec().into()))
                .unwrap();
            stage = 1;
        } else if stage == 1 && text.contains("abcdefghijklm") {
            for columns in [40, 20, 60, 30, 40] {
                let size = alacritty_terminal::term::test::TermSize::new(columns, 24);
                if let Some(gate) = gate_reflow.take() {
                    term.lock()
                        .resize_with_async_history_for_test(size, move || gate.recv().unwrap())
                        .unwrap();
                } else {
                    term.lock().resize_with_async_history(size).unwrap();
                }
                sender
                    .send(Msg::Resize(WindowSize {
                        num_cols: columns as u16,
                        num_lines: 24,
                        cell_width: 8,
                        cell_height: 16,
                    }))
                    .unwrap();
            }
            sender
                .send(Msg::Input(after_resize.to_vec().into()))
                .unwrap();
            stage = 2;
        }
        if exited {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // `exited` was observed with the history worker stopped. Release even on
    // failure so shutdown cannot leave a console service waiting indefinitely.
    release_reflow.send(()).ok();
    if !exited {
        sender.send(Msg::Shutdown).unwrap();
    }
    let (event_loop, _) = thread.join().unwrap();
    drop(event_loop);
    assert!(exited, "{program} did not accept the command and exit");
    let reader = term.lock().reflow_reader();
    if let Some(reader) = reader {
        // This test thread plays the UI's publication role after the reader
        // exits. Native console queries exercised the worker while it ran.
        reader.read(|_| ());
        assert!(term.lock().finish_pending_reflow());
    }
    let text = crate::alacritty::content_text(&term.lock());
    assert!(
        text.contains("INPUT:abcdefghijklmnopqrstuvwxyz0123456789"),
        "{text:?}"
    );
}

#[test]
fn embedded_adapter_uses_the_parser_thread_and_publishes_native_titles() {
    let (events, mut received) = futures::channel::mpsc::unbounded();
    let listener = ZedListener::new(events, WakeupGate::new());
    let term = new_term(
        &alacritty_terminal::term::Config::default(),
        TerminalBounds::default(),
        listener.clone(),
        crate::terminal_settings::AlternateScroll::On,
    );
    let options = tty::Options {
        shell: Some(tty::Shell::new(
            "cmd.exe".into(),
            vec![
                "/d".into(),
                "/c".into(),
                "title Pipeline title&echo READY&set /p answer=&echo FINAL".into(),
            ],
        )),
        console_backend: tty::ConsoleBackend::Hosted,
        escape_args: true,
        ..Default::default()
    };
    let pty = open(&options, &term).unwrap();
    assert!(pty.handover_handles().is_none());
    let event_loop = EventLoop::new(
        term.clone(),
        listener,
        pty,
        true,
        false,
        ReplayBarrier::open(),
    )
    .unwrap();
    let sender = event_loop.channel();
    let thread = event_loop.spawn();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut title = None;
    let mut exited = false;
    let mut input = false;
    while Instant::now() < deadline {
        while let Ok(event) = received.try_recv() {
            let PtyEvent::Event(event) = event;
            match event {
                TerminalBackendEvent::Title(value) => title = Some(value),
                TerminalBackendEvent::ChildExit(_) => exited = true,
                _ => {}
            }
        }
        if !input && title.is_some() {
            sender.send(Msg::Input(b"hello\r".to_vec().into())).unwrap();
            input = true;
        }
        if exited {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if !exited {
        sender.send(Msg::Shutdown).unwrap();
    }
    let (event_loop, _) = thread.join().unwrap();
    drop(event_loop);
    assert!(exited, "embedded child did not exit");
    // cmd prefixes "Administrator: " to the titles it sets when elevated.
    assert_eq!(
        title
            .as_deref()
            .map(|title| title.trim_start_matches("Administrator:  ")),
        Some("Pipeline title")
    );
    let terminal = term.lock();
    let mut text = String::new();
    for row in 0..terminal.screen_lines() {
        for column in 0..terminal.columns() {
            text.push(terminal.grid()[Line(row as i32)][Column(column)].c);
        }
    }
    assert!(text.contains("READY"), "{text}");
    assert!(text.contains("FINAL"), "{text}");
}
