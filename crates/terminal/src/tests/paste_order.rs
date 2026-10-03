use super::*;
use crate::{
    TerminalBuilder,
    terminal_settings::{AlternateScroll, CursorShape, TerminalSettings},
};
use gpui::{AppContext as _, ClipboardItem, Entity, TestAppContext};
use util::paths::PathStyle;

#[test]
fn input_passes_straight_through_when_no_paste_is_pending() {
    let mut order = PasteOrder::default();
    assert_eq!(order.admit("typed"), Some("typed"));
    assert!(!order.is_pending());
}

#[test]
fn input_after_a_pending_paste_is_released_after_the_paste() {
    let mut order = PasteOrder::default();
    let paste = order.begin();
    assert_eq!(order.admit("a"), None);
    assert_eq!(order.admit("b"), None);
    order.capture();
    assert_eq!(order.admit("pasted"), None);
    assert_eq!(order.resolve(paste), ["pasted", "a", "b"]);
    assert!(!order.is_pending());
    assert_eq!(order.admit("c"), Some("c"));
}

#[test]
fn a_later_paste_that_resolves_first_waits_for_the_earlier_one() {
    let mut order = PasteOrder::default();
    let first = order.begin();
    assert_eq!(order.admit("between"), None);
    let second = order.begin();
    assert_eq!(order.admit("after"), None);

    order.capture();
    assert_eq!(order.admit("second paste"), None);
    assert!(
        order.resolve(second).is_empty(),
        "the first paste is pending"
    );
    assert!(order.is_pending());

    order.capture();
    assert_eq!(order.admit("first paste"), None);
    assert_eq!(
        order.resolve(first),
        ["first paste", "between", "second paste", "after"]
    );
}

#[test]
fn an_empty_or_failed_read_still_releases_the_input_behind_it() {
    let mut order = PasteOrder::default();
    let paste = order.begin();
    assert_eq!(order.admit("typed"), None);
    order.capture();
    assert_eq!(order.resolve(paste), ["typed"]);

    // Resolving without starting a capture is also an empty paste.
    let paste = order.begin();
    assert_eq!(order.admit("again"), None);
    assert_eq!(order.resolve(paste), ["again"]);
}

fn display_only_terminal(cx: &mut TestAppContext) -> Entity<crate::Terminal> {
    cx.update(TerminalSettings::init);
    cx.new(|cx| {
        TerminalBuilder::new_display_only(
            CursorShape::default(),
            AlternateScroll::On,
            None,
            0,
            cx.background_executor(),
            PathStyle::local(),
        )
        .subscribe(cx)
    })
}

#[gpui::test]
async fn keystrokes_typed_while_a_paste_reads_are_written_after_it(cx: &mut TestAppContext) {
    let terminal = display_only_terminal(cx);
    let ticket = terminal.update(cx, |terminal, _| {
        let ticket = terminal.begin_paste();
        terminal.input(b"typed".to_vec());
        assert!(
            terminal.take_pty_write_log().is_empty(),
            "input overtook a pending paste"
        );
        assert!(terminal.paste_pending());
        ticket
    });
    terminal.update(cx, |terminal, _| {
        terminal.finish_paste(ticket, |terminal| terminal.paste("pasted"));
        assert_eq!(
            terminal.take_pty_write_log(),
            [b"pasted".to_vec(), b"typed".to_vec()]
        );
        assert!(!terminal.paste_pending());
    });
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[gpui::test]
async fn a_middle_click_paste_from_a_slow_owner_keeps_its_place(cx: &mut TestAppContext) {
    let terminal = display_only_terminal(cx);
    cx.update(|cx| cx.write_to_primary(ClipboardItem::new_string("selected".into())));
    cx.defer_clipboard_reads(true);
    terminal.update(cx, |terminal, cx| {
        terminal.paste_selection_clipboard(cx);
        terminal.input(b"typed".to_vec());
    });
    cx.run_until_parked();
    terminal.update(cx, |terminal, _| {
        assert!(terminal.take_pty_write_log().is_empty());
    });

    assert_eq!(cx.complete_clipboard_reads(), 1);
    cx.run_until_parked();
    terminal.update(cx, |terminal, _| {
        assert_eq!(
            terminal.take_pty_write_log(),
            [b"selected".to_vec(), b"typed".to_vec()]
        );
    });
}
