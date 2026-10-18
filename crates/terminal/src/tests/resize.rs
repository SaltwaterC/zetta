use super::*;
use gpui::{TestAppContext, VisualContext as _, bounds, size};

#[gpui::test]
async fn layout_resize_reflows_text_and_preserves_the_input_cursor(cx: &mut TestAppContext) {
    use alacritty_terminal::index::{Column, Line, Point as AlacPoint};

    let builder = cx.update(|cx| {
        TerminalBuilder::new_display_only(
            SettingsCursorShape::Block,
            AlternateScroll::On,
            None,
            100,
            cx.background_executor(),
            PathStyle::local(),
        )
    });
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| builder.subscribe(cx));
    let make_bounds = |columns: f32| {
        TerminalBounds::new(
            px(10.),
            px(10.),
            bounds(GpuiPoint::default(), size(px(columns * 10.), px(100.))),
        )
    };
    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(make_bounds(20.));
        terminal.sync(window, cx);
        let mut parser = Processor::<StdSyncHandler>::new();
        parser.advance(
            &mut *terminal.term.lock(),
            b"abcdefghijklmnop\r\nPS> abcdefghij",
        );

        // A split, repeated gutter changes, and closing the split all
        // resize the local grid through set_size. No shell redraw should
        // be required to retain either output or the partially typed line.
        for columns in [10., 8., 20., 9., 20.] {
            terminal.set_size(make_bounds(columns));
            terminal.sync(window, cx);
            let term = terminal.term.lock();
            let text = content_text(&term);
            assert!(text.contains("abcdefghijklmnop"), "{columns}: {text:?}");
            assert!(text.contains("PS> abcdefghij"), "{columns}: {text:?}");
        }
        let mut term = terminal.term.lock();
        parser.advance(&mut *term, b"K");
        assert!(content_text(&term).contains("PS> abcdefghijK"));
        assert_eq!(
            term.grid().cursor.point,
            AlacPoint::new(Line(1), Column(15))
        );
    });
}

#[test]
fn synchronous_history_reflow_has_a_cell_budget() {
    assert!(synchronous_reflow_is_bounded(1_310, 100));
    assert!(!synchronous_reflow_is_bounded(1_311, 100));
    assert!(!synchronous_reflow_is_bounded(usize::MAX, 2));
}

#[gpui::test]
async fn a_large_layout_resize_scrolls_before_the_worker_is_allowed_to_run(
    cx: &mut TestAppContext,
) {
    let builder = cx.update(|cx| {
        TerminalBuilder::new_display_only(
            SettingsCursorShape::Block,
            AlternateScroll::On,
            None,
            0,
            cx.background_executor(),
            PathStyle::local(),
        )
    });
    let window = cx.add_empty_window();
    let terminal = window.new(|cx| builder.subscribe(cx));
    let geometry = |columns: f32| {
        TerminalBounds::new(
            px(10.),
            px(10.),
            bounds(GpuiPoint::default(), size(px(columns * 10.), px(240.))),
        )
    };
    let mut expected = window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(geometry(80.));
        terminal.sync(window, cx);
        let mut parser = Processor::<StdSyncHandler>::new();
        let mut term = terminal.term.lock();
        let line = format!("{}\r\n", "retained output ".repeat(5));
        parser.advance(&mut *term, b"OLDEST RETAINED LINE\r\n");
        for _ in 0..16_000 {
            parser.advance(&mut *term, line.as_bytes());
        }
        parser.advance(&mut *term, b"PS> partially typed");
        assert!(!synchronous_reflow_is_bounded(
            term.history_size(),
            term.columns()
        ));
        term.clone()
    });
    resize(&mut expected, geometry(31.), true);
    let (release, gate) = std::sync::mpsc::channel();
    let reader = window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal
            .term
            .lock()
            .resize_with_async_history_for_test(geometry(31.), move || gate.recv().unwrap())
            .unwrap();
        terminal.set_size(geometry(31.));
        terminal.sync(window, cx);
        assert_eq!(terminal.term.lock().columns(), 31);
        terminal.scroll_to_top();
        terminal.select_all();
        terminal.sync(window, cx);
        // This must be visible while the full-history worker is stopped. A
        // test that waits for publication before checking scroll misses the bug.
        assert!(terminal.term.lock().reflow_reader().is_some());
        assert!(terminal.last_content.scrolled_to_top);
        assert!(terminal.last_content.total_lines > 15_000);
        let visible: String = terminal
            .last_content
            .cells
            .iter()
            .map(|cell| cell.cell.character())
            .collect();
        assert!(visible.contains("OLDEST RETAINED LINE"), "{visible:?}");
        assert_eq!(terminal.last_content.columns, 31);
        for cell in &terminal.last_content.cells {
            assert!(
                (0..24).contains(&(cell.point.line + terminal.last_content.display_offset as i32))
            );
            assert!(cell.point.column < 31);
        }
        // A second frame, wheel movement, returning to the prompt and another
        // resize must all work before the worker receives its release signal.
        terminal.scroll_down_by(10);
        terminal.sync(window, cx);
        assert!(!terminal.last_content.scrolled_to_top);
        terminal.scroll_to_bottom();
        terminal.sync(window, cx);
        assert!(terminal.last_content.scrolled_to_bottom);
        terminal.set_size(geometry(17.));
        terminal.sync(window, cx);
        terminal.scroll_to_top();
        terminal.sync(window, cx);
        assert_eq!(terminal.last_content.columns, 17);
        assert!(terminal.last_content.scrolled_to_top);
        terminal.term.lock().reflow_reader()
    });
    resize(&mut expected, geometry(17.), true);
    release.send(()).unwrap();
    // Only the test waits here; production receives the worker's Wakeup.
    if let Some(reader) = reader {
        reader.read(|_| ());
    }
    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.sync(window, cx);
        let term = terminal.term.lock();
        let text = content_text(&term);
        assert!(text.contains("OLDEST RETAINED LINE"));
        assert_eq!(text, content_text(&expected));
        assert_eq!(term.grid().cursor.point, expected.grid().cursor.point);
        assert_eq!(term.grid().display_offset(), term.history_size());
        assert!(
            term.selection_to_string()
                .unwrap()
                .contains("OLDEST RETAINED LINE")
        );
    });
}
