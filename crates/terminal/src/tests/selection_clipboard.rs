use super::*;
use crate::{Content, TerminalBounds, alacritty::*, terminal_settings::*};
use alacritty_terminal::{
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    vte::ansi::{Processor, StdSyncHandler},
};
use futures::channel::{mpsc::unbounded, oneshot};
use gpui::TestAppContext;
use gpui::{AppContext as _, VisualContext as _};

fn selected_term(text: &[u8], ty: SelectionType) -> AlacrittyTerm {
    let (events, _) = unbounded();
    let term = new_term(
        &display_only_term_config(100_000, CursorShape::Block),
        TerminalBounds::default(),
        ZedListener::new(events, WakeupGate::new()),
        AlternateScroll::On,
    );
    let mut term = term.lock().clone();
    Processor::<StdSyncHandler>::new().advance(&mut term, text);
    let mut selection = Selection::new(ty, Point::new(Line(0), Column(0)), Side::Left);
    selection.update(Point::new(Line(1), Column(5)), Side::Right);
    term.selection = Some(selection);
    term
}

#[gpui::test]
async fn copy_captures_selection_before_output_and_selection_changes(cx: &mut TestAppContext) {
    for ty in [
        SelectionType::Simple,
        SelectionType::Block,
        SelectionType::Lines,
        SelectionType::Semantic,
    ] {
        let mut term = selected_term("a\t界e\u{301} text\r\nsecond line".as_bytes(), ty);
        let expected = term.selection_to_string();
        let read = cx.update(|cx| {
            assert!(copy(&term, cx));
            // This happens before the background task is polled. Neither the
            // mutable screen nor the selection may leak into the saved copy.
            Processor::<StdSyncHandler>::new().advance(&mut term, b"\x1b[2J\x1b[Hreplacement");
            term.selection = None;
            read(cx)
        });
        assert_eq!(read.await.and_then(|item| item.text()), expected);
    }
}

fn delayed_copy(cx: &mut App) -> (oneshot::Sender<Option<String>>, Shared<Task<()>>) {
    let version = invalidate(0, cx);
    let (sender, receiver) = oneshot::channel();
    let extraction = cx
        .background_executor()
        .spawn(async move { receiver.await.unwrap() });
    publish_when_ready(0, version, extraction, App::write_to_clipboard, cx);
    (sender, pending(0, cx).unwrap())
}

#[gpui::test]
async fn superseded_waiting_requests_do_not_extract_text(cx: &mut TestAppContext) {
    let gate = cx.update(|cx| {
        cx.default_global::<SelectionClipboard>().0[0]
            .extraction_gate
            .clone()
    });
    let guard = gate.lock().await;
    let term = selected_term(b"first\r\nsecond", SelectionType::Simple);
    let mut publications = Vec::new();
    for _ in 0..5 {
        publications.push(cx.update(|cx| {
            assert!(copy(&term, cx));
            pending(0, cx).unwrap()
        }));
    }
    drop(guard);
    for publication in publications {
        publication.await;
    }
    assert_eq!(
        cx.update(|cx| cx.global::<SelectionClipboard>().0[0]
            .extractions
            .load(Ordering::Relaxed)),
        1
    );
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text())),
        term.selection_to_string()
    );
}

#[gpui::test]
async fn older_completion_cannot_overwrite_a_newer_copy(cx: &mut TestAppContext) {
    let (old, old_done) = cx.update(delayed_copy);
    let (new, new_done) = cx.update(delayed_copy);
    new.send(Some("new selection".into())).unwrap();
    new_done.await;
    old.send(Some("old selection".into())).unwrap();
    old_done.await;
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().unwrap().text())
            .as_deref(),
        Some("new selection")
    );
}

#[gpui::test]
async fn reads_wait_for_the_newest_copy_and_direct_writes_supersede_pending_copies(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| write(ClipboardItem::new_string("previous clipboard".into()), cx));
    let (old, old_done) = cx.update(delayed_copy);
    let read = cx.update(|cx| read(cx));
    let (new, new_done) = cx.update(delayed_copy);
    old.send(Some("old selection".into())).unwrap();
    old_done.await;
    new.send(Some("new selection".into())).unwrap();
    new_done.await;
    assert_eq!(read.await.unwrap().text().as_deref(), Some("new selection"));

    let (old, old_done) = cx.update(delayed_copy);
    cx.update(|cx| write(ClipboardItem::new_string("OSC 52 or clear".into()), cx));
    old.send(Some("old selection".into())).unwrap();
    old_done.await;
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().unwrap().text())
            .as_deref(),
        Some("OSC 52 or clear")
    );
}

#[gpui::test]
async fn rendering_and_empty_selections_do_not_start_clipboard_work(cx: &mut TestAppContext) {
    let mut term = selected_term(b"first\r\nsecond", SelectionType::Simple);
    cx.update(|cx| {
        let mut content = Content::default();
        for _ in 0..10 {
            content = make_content(&term, &mut content);
            assert!(content.selection.is_some());
        }
        assert!(cx.try_global::<SelectionClipboard>().is_none());
        term.selection = None;
        assert!(!copy(&term, cx));
        assert!(cx.try_global::<SelectionClipboard>().is_none());
    });
}

#[gpui::test]
async fn large_selection_stays_visual_and_copies_its_capture_while_output_continues(
    cx: &mut TestAppContext,
) {
    cx.update(TerminalSettings::init);
    for rows in [1_000, 10_000, 100_000] {
        let window = cx.add_empty_window();
        let terminal = window.new(|cx| {
            crate::TerminalBuilder::new_display_only(
                CursorShape::Block,
                AlternateScroll::On,
                Some(rows + 100),
                0,
                cx.background_executor(),
                util::paths::PathStyle::local(),
            )
            .subscribe(cx)
        });
        let text = (0..rows)
            .map(|line| format!("row {line:06}\r\n"))
            .collect::<String>();
        let (expected, clipboard) =
            window.update_window_entity(&terminal, |terminal, window, cx| {
                terminal.write_output(text.as_bytes(), cx);
                terminal.select_all();
                terminal.sync(window, cx);
                // Highlighting is ready before any clipboard worker runs.
                assert!(terminal.last_content.selection.is_some());
                let expected = terminal.term.lock().selection_to_string();
                terminal.copy(Some(true));
                terminal.sync(window, cx);
                let version = cx.global::<SelectionClipboard>().0[0]
                    .version
                    .load(Ordering::Relaxed);
                for _ in 0..12 {
                    terminal.write_output(b"ongoing output\r\n", cx);
                    terminal.sync(window, cx);
                    assert!(terminal.last_content.selection.is_some());
                    assert_eq!(
                        cx.global::<SelectionClipboard>().0[0]
                            .version
                            .load(Ordering::Relaxed),
                        version
                    );
                }
                (expected, read(cx))
            });
        assert_eq!(clipboard.await.and_then(|item| item.text()), expected);
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let primary = cx.update(|cx| primary::read(cx));
            assert_eq!(primary.await.and_then(|item| item.text()), expected);
        }
    }
}

#[gpui::test]
async fn copy_and_clear_removes_highlight_before_extraction_finishes(cx: &mut TestAppContext) {
    cx.update(TerminalSettings::init);
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| {
        crate::TerminalBuilder::new_display_only(
            CursorShape::Block,
            AlternateScroll::On,
            Some(100),
            0,
            cx.background_executor(),
            util::paths::PathStyle::local(),
        )
        .subscribe(cx)
    });
    let (expected, clipboard) = window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.write_output(b"copy and clear\r\n", cx);
        let expected = terminal.get_content();
        terminal.select_all();
        terminal.copy(Some(false));
        terminal.sync(window, cx);
        assert!(terminal.last_content.selection.is_none());
        assert!(pending(0, cx).is_some());
        (expected, read(cx))
    });
    assert_eq!(
        clipboard.await.and_then(|item| item.text()),
        Some(expected.clone())
    );
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        let primary = cx.update(|cx| primary::read(cx));
        assert_eq!(primary.await.and_then(|item| item.text()), Some(expected));
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[gpui::test]
async fn primary_and_copy_have_independent_versions_and_clear_keeps_primary(
    cx: &mut TestAppContext,
) {
    let mut term = selected_term(b"first\r\nsecond", SelectionType::Simple);
    let expected = term.selection_to_string();
    let (primary, clipboard) = cx.update(|cx| {
        primary::copy(&term, cx);
        assert!(copy(&term, cx));
        term.selection = None;
        primary::copy(&term, cx);
        (primary::read(cx), read(cx))
    });
    assert_eq!(primary.await.and_then(|item| item.text()), expected);
    assert_eq!(clipboard.await.and_then(|item| item.text()), expected);
}
