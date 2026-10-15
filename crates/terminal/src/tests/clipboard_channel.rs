use super::*;
use crate::{TerminalBuilder, terminal_settings::AlternateScroll};
use gpui::{AppContext as _, TestAppContext};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use util::paths::PathStyle;

struct NoPtyControl;

impl crate::PtyControl for NoPtyControl {
    fn resize(&self, _: u16, _: u16) {}

    fn set_console_palette(&self, _: crate::ConsolePalette) {}
}

fn served_terminal(cx: &mut TestAppContext, helpers: Helpers) -> gpui::Entity<Terminal> {
    cx.new(|cx| {
        let mut terminal = TerminalBuilder::new_display_only(
            Default::default(),
            AlternateScroll::On,
            None,
            0,
            cx.background_executor(),
            PathStyle::local(),
        )
        .with_pty_control(Arc::new(NoPtyControl))
        .subscribe(cx);
        terminal.remote_clipboard.helpers = helpers;
        terminal.set_remote_clipboard_paste_allowed(true);
        terminal
    })
}

static PASTES: AtomicUsize = AtomicUsize::new(0);

/// The helper used to run, and be waited for, inside the terminal's event
/// handling, so a slow one froze the pane and the window with it.
#[gpui::test]
async fn a_clipboard_request_is_served_off_the_terminal_event_path(cx: &mut TestAppContext) {
    let terminal = served_terminal(
        cx,
        Helpers {
            copy: |_| Ok(()),
            paste: || {
                PASTES.fetch_add(1, Ordering::SeqCst);
                Ok(Some("pasted".into()))
            },
        },
    );
    let request = Frame {
        id: [9; 16],
        message: Message::Paste,
    };
    terminal.update(cx, |terminal, cx| {
        terminal.write_output(&request.encode(), cx);
        assert_eq!(PASTES.load(Ordering::SeqCst), 0, "served synchronously");
        assert!(terminal.take_pty_write_log().is_empty());
    });
    cx.run_until_parked();
    terminal.update(cx, |terminal, _| {
        assert_eq!(PASTES.load(Ordering::SeqCst), 1);
        assert_eq!(
            terminal.take_pty_write_log(),
            vec![
                Frame {
                    id: request.id,
                    message: Message::Data {
                        sequence: 0,
                        bytes: b"pasted".to_vec(),
                    },
                }
                .encode()
            ]
        );
    });
}

/// Frames are answered in the order the pane printed them, and a flood beyond
/// the queue is turned away at once rather than held.
#[gpui::test]
async fn queued_requests_are_answered_in_order_and_bounded(cx: &mut TestAppContext) {
    let terminal = served_terminal(cx, Helpers::default());
    let probes = (0..QUEUED_REQUESTS + 3)
        .map(|index| Frame {
            id: [index as u8; 16],
            message: Message::Probe,
        })
        .collect::<Vec<_>>();
    terminal.update(cx, |terminal, cx| {
        for probe in &probes {
            terminal.write_output(&probe.encode(), cx);
        }
        // The worker has not run yet, so the three past the queue are refused.
        let refused = terminal.take_pty_write_log();
        assert_eq!(refused.len(), 3);
        for (answer, probe) in refused.iter().zip(&probes[QUEUED_REQUESTS..]) {
            let answer = Frame::parse(answer).unwrap();
            assert_eq!(answer.id, probe.id);
            assert!(matches!(answer.message, Message::Error(_)));
        }
    });
    cx.run_until_parked();
    terminal.update(cx, |terminal, _| {
        let answers = terminal.take_pty_write_log();
        let expected = probes[..QUEUED_REQUESTS]
            .iter()
            .map(|probe| {
                Frame {
                    id: probe.id,
                    message: Message::Ready,
                }
                .encode()
            })
            .collect::<Vec<_>>();
        assert_eq!(answers, expected);
    });
}

fn long_paste() -> Result<Option<String>> {
    Ok(Some("p".repeat(zclip::protocol::CHUNK_SIZE + 1)))
}

fn long_paste_terminal(cx: &mut TestAppContext) -> gpui::Entity<Terminal> {
    served_terminal(
        cx,
        Helpers {
            copy: |_| Ok(()),
            paste: long_paste,
        },
    )
}

fn answers(terminal: &gpui::Entity<Terminal>, cx: &mut TestAppContext) -> Vec<Message> {
    terminal.update(cx, |terminal, _| {
        terminal
            .take_pty_write_log()
            .iter()
            .map(|answer| Frame::parse(answer).unwrap().message)
            .collect()
    })
}

fn print(terminal: &gpui::Entity<Terminal>, message: Message, cx: &mut TestAppContext) {
    terminal.update(cx, |terminal, cx| {
        terminal.write_output(
            &Frame {
                id: [7; 16],
                message,
            }
            .encode(),
            cx,
        );
    });
    cx.run_until_parked();
}

/// Withdrawing the permission ends the paste at once, not at the helper's next
/// acknowledgement: allowing paste again before that acknowledgement arrives
/// does not let it fetch the rest of the clipboard captured earlier.
#[gpui::test]
async fn withdrawing_paste_ends_a_paste_already_in_flight(cx: &mut TestAppContext) {
    let terminal = long_paste_terminal(cx);
    print(&terminal, Message::Paste, cx);
    assert!(matches!(
        answers(&terminal, cx).as_slice(),
        [Message::Data { sequence: 0, .. }]
    ));
    terminal.update(cx, |terminal, _| {
        terminal.set_remote_clipboard_paste_allowed(false);
    });
    cx.run_until_parked();
    terminal.update(cx, |terminal, _| {
        terminal.set_remote_clipboard_paste_allowed(true);
    });
    print(&terminal, Message::Ack { next_sequence: 1 }, cx);
    assert!(matches!(
        answers(&terminal, cx).as_slice(),
        [Message::Error(_)]
    ));
}

/// The permission is read when a frame is served, not when it was printed.
#[gpui::test]
async fn a_paste_queued_before_revocation_is_refused(cx: &mut TestAppContext) {
    let terminal = long_paste_terminal(cx);
    terminal.update(cx, |terminal, cx| {
        let paste = Frame {
            id: [7; 16],
            message: Message::Paste,
        };
        terminal.write_output(&paste.encode(), cx);
        terminal.set_remote_clipboard_paste_allowed(false);
    });
    cx.run_until_parked();
    assert_eq!(
        answers(&terminal, cx),
        vec![Message::Error(zclip::host::PASTE_DISABLED.into())]
    );
}

static REVOKED_DURING_PASTE: std::sync::OnceLock<Arc<std::sync::atomic::AtomicBool>> =
    std::sync::OnceLock::new();

/// A revocation that lands while the helper is reading the clipboard holds for
/// the chunk that read produced.
#[gpui::test]
async fn a_revocation_during_the_clipboard_read_withholds_its_chunk(cx: &mut TestAppContext) {
    let terminal = served_terminal(
        cx,
        Helpers {
            copy: |_| Ok(()),
            paste: || {
                REVOKED_DURING_PASTE
                    .get()
                    .unwrap()
                    .store(false, Ordering::SeqCst);
                long_paste()
            },
        },
    );
    terminal.update(cx, |terminal, _| {
        REVOKED_DURING_PASTE
            .set(terminal.remote_clipboard.permissions.paste.clone())
            .unwrap();
    });
    print(&terminal, Message::Paste, cx);
    assert_eq!(
        answers(&terminal, cx),
        vec![Message::Error(zclip::host::PASTE_DISABLED.into())]
    );
}

static COPIES: AtomicUsize = AtomicUsize::new(0);

fn counting_copy_terminal(cx: &mut TestAppContext) -> gpui::Entity<Terminal> {
    served_terminal(
        cx,
        Helpers {
            copy: |_| {
                COPIES.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            paste: || Ok(None),
        },
    )
}

fn print_as(terminal: &gpui::Entity<Terminal>, id: u8, message: Message, cx: &mut TestAppContext) {
    terminal.update(cx, |terminal, cx| {
        terminal.write_output(
            &Frame {
                id: [id; 16],
                message,
            }
            .encode(),
            cx,
        );
    });
    cx.run_until_parked();
}

/// Withdrawing copy ends the copy at once: allowing it again before the
/// helper's end arrives does not let what it spooled reach the clipboard.
#[gpui::test]
async fn withdrawing_copy_ends_a_copy_already_in_flight(cx: &mut TestAppContext) {
    let terminal = counting_copy_terminal(cx);
    print_as(&terminal, 21, Message::Copy, cx);
    print_as(
        &terminal,
        21,
        Message::Data {
            sequence: 0,
            bytes: b"late".to_vec(),
        },
        cx,
    );
    answers(&terminal, cx);
    terminal.update(cx, |terminal, _| {
        terminal.set_remote_clipboard_copy_allowed(false);
    });
    cx.run_until_parked();
    terminal.update(cx, |terminal, _| {
        terminal.set_remote_clipboard_copy_allowed(true);
    });
    print_as(&terminal, 21, Message::End, cx);
    assert!(matches!(
        answers(&terminal, cx).as_slice(),
        [Message::Error(_)]
    ));
    assert_eq!(COPIES.load(Ordering::SeqCst), 0);
}

/// The copy permission is read when the end is served, not when it was
/// printed.
#[gpui::test]
async fn a_copy_ended_before_revocation_is_served_does_not_land(cx: &mut TestAppContext) {
    let terminal = counting_copy_terminal(cx);
    print_as(&terminal, 22, Message::Copy, cx);
    answers(&terminal, cx);
    terminal.update(cx, |terminal, cx| {
        let end = Frame {
            id: [22; 16],
            message: Message::End,
        };
        terminal.write_output(&end.encode(), cx);
        terminal.set_remote_clipboard_copy_allowed(false);
    });
    cx.run_until_parked();
    assert_eq!(
        answers(&terminal, cx),
        vec![Message::Error(zclip::host::COPY_DISABLED.into())]
    );
    assert_eq!(COPIES.load(Ordering::SeqCst), 0);
}

fn clipboard_text(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

/// OSC 52 is the other way pane output writes the clipboard, so the copy
/// permission governs it too.
#[gpui::test]
async fn osc52_copies_follow_the_copy_permission(cx: &mut TestAppContext) {
    cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("original".into())));
    let terminal = served_terminal(cx, Helpers::default());
    terminal.update(cx, |terminal, cx| {
        terminal.set_remote_clipboard_copy_allowed(false);
        // "refused"
        terminal.write_output(b"\x1b]52;c;cmVmdXNlZA==\x07", cx);
    });
    cx.run_until_parked();
    assert_eq!(clipboard_text(cx).as_deref(), Some("original"));
    terminal.update(cx, |terminal, cx| {
        terminal.set_remote_clipboard_copy_allowed(true);
        // "allowed"
        terminal.write_output(b"\x1b]52;c;YWxsb3dlZA==\x07", cx);
    });
    cx.run_until_parked();
    assert_eq!(clipboard_text(cx).as_deref(), Some("allowed"));
}

#[cfg(unix)]
mod helper_processes {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn a_helper_that_finishes_returns_its_output() {
        let output = run_helper(
            shell("cat"),
            Some(b"round trip".to_vec()),
            Some(64),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(output, b"round trip");
    }

    #[test]
    fn a_slow_helper_is_stopped_at_its_deadline() {
        let started = Instant::now();
        let error = run_helper(
            shell("sleep 30"),
            None,
            Some(64),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(format!("{error:#}").contains("in time"), "{error:#}");
    }

    /// A helper that never reads its input fills the pipe; the write must not
    /// hold the caller past the deadline either.
    #[test]
    fn a_helper_that_never_reads_its_input_is_stopped_at_its_deadline() {
        let started = Instant::now();
        let result = run_helper(
            shell("sleep 30"),
            Some(vec![b'x'; 8 * 1024 * 1024]),
            None,
            Duration::from_millis(200),
        );
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn helper_output_beyond_the_limit_is_an_error() {
        let error = run_helper(
            shell("head -c 100 /dev/zero"),
            None,
            Some(10),
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("too large"), "{error:#}");
        // Nor does a helper that writes forever outlast the deadline.
        assert!(run_helper(shell("yes"), None, Some(10), Duration::from_secs(10)).is_err());
    }

    #[test]
    fn a_failing_helper_is_an_error() {
        assert!(run_helper(shell("exit 3"), None, Some(10), Duration::from_secs(10)).is_err());
    }
}
