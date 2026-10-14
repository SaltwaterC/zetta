use super::*;
use crate::alacritty::{WakeupGate, ZedListener, new_term};
use crate::{PtyEvent, TerminalBackendEvent, TerminalBounds};
use alacritty_terminal::event_loop::{EventLoop, Msg, ReplayBarrier};
use alacritty_terminal::index::{Column, Line};
use std::time::{Duration, Instant};

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
