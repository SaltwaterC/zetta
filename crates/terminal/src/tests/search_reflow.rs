use super::*;
use gpui::{TestAppContext, VisualContext as _, bounds, size};

#[gpui::test]
async fn search_and_navigation_finish_while_history_reflow_is_stopped(cx: &mut TestAppContext) {
    exercise_paused_reflow_search(cx, 16_000).await;
}

#[gpui::test]
#[ignore = "manual optimized-build 1000 MiB search while history reflow is stopped"]
async fn large_unique_search_during_paused_reflow(cx: &mut TestAppContext) {
    assert!(
        !std::hint::black_box(cfg!(debug_assertions)),
        "run with --release"
    );
    let mib: usize = std::env::var("ZETTA_SEARCH_STRESS_MIB")
        .ok()
        .map(|value| value.parse().unwrap())
        .unwrap_or(1000);
    exercise_paused_reflow_search(cx, mib * 1024 * 1024 / 80).await;
}

async fn exercise_paused_reflow_search(cx: &mut TestAppContext, rows: usize) {
    let builder = cx.update(|cx| {
        TerminalBuilder::new_display_only(
            SettingsCursorShape::Block,
            AlternateScroll::On,
            Some(i32::MAX as usize - 1024),
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
    let (release, gate) = std::sync::mpsc::channel();
    let expired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_expired = expired.clone();
    let job = window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(geometry(80.));
        terminal.sync(window, cx);
        {
            let mut term = terminal.term.lock();
            let mut parser = Processor::<StdSyncHandler>::new();
            parser.advance(&mut *term, b"OLDEST needle with wide \xe7\x95\x8c\r\n");
            parser.advance(
                &mut *term,
                format!("{}edgehit\r\n", "x".repeat(72)).as_bytes(),
            );
            let mut batch = Vec::with_capacity(128 * 1024);
            for line in 0..rows {
                batch.extend_from_slice(
                    format!(
                        "line {line:016x} {}\r\n",
                        "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTU"
                    )
                    .as_bytes(),
                );
                if batch.len() >= 128 * 1024 {
                    parser.advance(&mut *term, &batch);
                    batch.clear();
                }
            }
            parser.advance(&mut *term, &batch);
            term.resize_with_async_history_for_test(geometry(17.), move || {
                if matches!(
                    gate.recv_timeout(std::time::Duration::from_secs(30)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    worker_expired.store(true, std::sync::atomic::Ordering::Release);
                }
            })
            .unwrap();
        }
        terminal.set_size(geometry(17.));
        terminal.sync(window, cx);
        terminal.find_matches(Search::new_literal("needle").unwrap(), cx)
    });
    // The reflow gate is still closed. A complete_snapshot/Condvar wait here
    // deadlocks this test instead of allowing an engine-only benchmark to pass.
    let started = std::time::Instant::now();
    let result = job.finish().await;
    assert!(
        !expired.load(std::sync::atomic::Ordering::Acquire),
        "search waited for the history reflow gate"
    );
    eprintln!(
        "paused_reflow_search rows={rows} complete_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.
    );
    assert!(result.complete);
    assert_eq!(result.total_count, 1);
    let navigate = std::time::Instant::now();
    let clipboard = window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.matches = Arc::new(result.ranges);
        terminal.activate_match(0);
        terminal.sync(window, cx);
        assert!(terminal.term.lock().reflow_reader().is_some());
        assert_eq!(terminal.last_content.columns, 17);
        let text: String = terminal
            .last_content
            .cells
            .iter()
            .map(|cell| cell.cell.character())
            .collect();
        assert!(text.contains("OLDEST needle"), "{text:?}");
        assert!(!terminal.renderable_search_matches().is_empty());
        assert!(terminal.last_content.selection.is_some());
        terminal.copy(Some(true));
        terminal.sync(window, cx);
        selection_clipboard::read(cx)
    });
    eprintln!(
        "paused_reflow_navigation rows={rows} visible_and_highlighted_ms={:.3}",
        navigate.elapsed().as_secs_f64() * 1000.
    );
    assert_eq!(
        clipboard.await.and_then(|item| item.text()).as_deref(),
        Some("needle")
    );
    for columns in [2., 31., 97.] {
        let job = window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal
                .term
                .lock()
                .resize_with_async_history(geometry(columns))
                .unwrap();
            terminal.set_size(geometry(columns));
            terminal.sync(window, cx);
            terminal.find_matches(Search::new_literal("edgehit").unwrap(), cx)
        });
        let result = job.finish().await;
        assert_eq!(result.total_count, 1);
        window.update_window_entity(&terminal, |terminal, window, cx| {
            terminal.matches = Arc::new(result.ranges);
            terminal.activate_match(0);
            terminal.sync(window, cx);
            let text: String = terminal
                .last_content
                .cells
                .iter()
                .map(|cell| cell.cell.character())
                .collect();
            assert!(text.contains("edgehit"), "{columns} columns: {text:?}");
            assert!(terminal.last_content.selection.is_some());
        });
    }
    if rows > 100_000 {
        // The stress measurement ends with the worker still blocked. Stop its
        // job before opening the gate, avoiding a second full-history workload.
        window.update_window_entity(&terminal, |terminal, _, _| {
            terminal.wakeup_gate.set_enabled(false);
            terminal.term.lock().cancel_pending_reflow_for_test();
        });
        drop(release);
        return;
    }
    let reader = window.update_window_entity(&terminal, |terminal, _, _| {
        // Publication is pumped explicitly below; a native worker must not
        // schedule GPUI's deterministic test executor from an OS thread.
        terminal.wakeup_gate.set_enabled(false);
        terminal.term.lock().reflow_reader().unwrap()
    });
    release.send(()).unwrap();
    reader.snapshot().unwrap();
    let job = window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.sync(window, cx);
        assert!(terminal.reflow_search.source().is_none());
        terminal.find_matches(Search::new_literal("needle").unwrap(), cx)
    });
    let result = job.finish().await;
    assert_eq!(result.total_count, 1);
    assert!(terminal.read_with(cx, |terminal, _| terminal.reflow_search.source().is_none()));
}
