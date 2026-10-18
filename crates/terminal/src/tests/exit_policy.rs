use super::*;
use gpui::TestAppContext;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt as _;
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt as _;

#[gpui::test(iterations = 10)]
fn a_failed_shell_stays_open_unless_the_user_sent_input(cx: &mut TestAppContext) {
    for input_sent in [false, true] {
        let builder = cx.update(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::default(),
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
        });
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let (events_tx, events_rx) = async_channel::unbounded();
        cx.update(|cx| {
            cx.subscribe(&terminal, move |_, event, _| {
                events_tx.try_send(event.clone()).unwrap();
            })
            .detach();
        });
        let (completion_tx, completion_rx) = async_channel::unbounded();
        terminal.update(cx, |terminal, cx| {
            terminal.completion_tx = Some(completion_tx);
            let status = ExitStatus::from_raw(if cfg!(windows) { 1 } else { 1 << 8 });
            terminal.report_child_exit(status, input_sent, cx);
        });
        cx.run_until_parked();
        assert_eq!(completion_rx.try_recv().unwrap().unwrap().code(), Some(1));
        let events = std::iter::from_fn(|| events_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::TerminalExited(_)))
        );
        assert_eq!(events.contains(&Event::CloseTerminal), input_sent);
    }
}
