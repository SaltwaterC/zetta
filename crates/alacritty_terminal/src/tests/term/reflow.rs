use super::*;
use crate::snapshot::ansi_snapshot;
use crate::vte::ansi::{Processor, StdSyncHandler};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<Event>>>);

impl EventListener for Events {
    fn send_event(&self, event: Event) {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(event);
    }
}

fn populated() -> Term<Events> {
    let mut term = Term::new(
        Config { scrolling_history: 1_000_000, ..Config::default() },
        &test::TermSize::new(80, 24),
        Events::default(),
    );
    let mut parser = Processor::<StdSyncHandler>::new();
    for line in 0..16_000 {
        parser.advance(
            &mut term,
            format!("{line:05}: {}\r\n", "wide 界e\u{301} text ".repeat(9)).as_bytes(),
        );
    }
    parser.advance(&mut term, b"PS> partially typed command");
    term
}

fn finish(term: &mut Term<Events>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !term.finish_pending_reflow() {
        assert!(Instant::now() < deadline, "history reflow did not finish");
        thread::yield_now();
    }
}

#[test]
fn history_reflow_does_not_block_live_output_or_discard_old_cells() {
    let mut live = populated();
    let mut expected = live.clone();
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(31, 12), move || gate.recv().unwrap()).unwrap();
    // The worker is stopped before doing any reflow. Visible content and input
    // must already be usable; this assertion cannot pass by finishing the job.
    assert_eq!((live.columns(), live.screen_lines()), (31, 12));
    assert!(live.history_size() < 4_000);
    assert!(!live.finish_pending_reflow());
    expected.resize(test::TermSize::new(31, 12));
    let mut parser = Processor::<StdSyncHandler>::new();
    let mut reference_parser = Processor::<StdSyncHandler>::new();
    for (columns, lines, output) in [
        (97, 30, &b"ABCDEFGHIJ\r\n"[..]),
        (19, 8, &b"\x1b[31mcolored\x1b[m\r\n"[..]),
        (53, 24, &b"\x1b[2K\rPS> final input"[..]),
    ] {
        live.resize_with_async_history(test::TermSize::new(columns, lines)).unwrap();
        expected.resize(test::TermSize::new(columns, lines));
        parser.advance(&mut live, output);
        reference_parser.advance(&mut expected, output);
    }
    assert!(live.grid()[live.grid.cursor.point.line][live.grid.cursor.point.column - 1].c == 't');
    release.send(()).unwrap();
    let snapshot = live.reflow_reader().unwrap().snapshot().unwrap();
    assert_eq!(ansi_snapshot(&snapshot, 1_000_000), ansi_snapshot(&expected, 1_000_000));
    finish(&mut live);
    assert_eq!(ansi_snapshot(&live, 1_000_000), ansi_snapshot(&expected, 1_000_000));
    assert_eq!(live.grid.cursor, expected.grid.cursor);
    assert!(ansi_snapshot(&live, 1_000_000).windows(b"00000:".len()).any(|text| text == b"00000:"));
}

#[test]
fn superseded_resizes_coalesce_without_crossing_output() {
    let mut live = populated();
    let mut expected = live.clone();
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(40, 24), move || gate.recv().unwrap()).unwrap();
    expected.resize(test::TermSize::new(40, 24));
    live.resize_with_async_history(test::TermSize::new(5, 24)).unwrap();
    live.resize_with_async_history(test::TermSize::new(80, 24)).unwrap();
    expected.resize(test::TermSize::new(80, 24));
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(&mut live, b"OUTPUT BOUNDARY");
    parser.advance(&mut expected, b"OUTPUT BOUNDARY");
    live.resize_with_async_history(test::TermSize::new(53, 16)).unwrap();
    expected.resize(test::TermSize::new(53, 16));
    release.send(()).unwrap();
    finish(&mut live);
    assert_eq!(ansi_snapshot(&live, 1_000_000), ansi_snapshot(&expected, 1_000_000));
}

#[test]
fn closing_a_terminal_cancels_pending_reflow_readers() {
    let mut live = populated();
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(40, 24), move || {
        gate.recv().ok();
    })
    .unwrap();
    let reader = live.reflow_reader().unwrap();
    drop(live);
    assert!(reader.read(|_| ()).is_none());
    release.send(()).unwrap();
}

#[test]
fn mixed_binary_controls_during_reflow_match_the_live_parser() {
    let mut live = populated();
    let mut expected = live.clone();
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(37, 15), move || gate.recv().unwrap()).unwrap();
    expected.resize(test::TermSize::new(37, 15));
    let mut seed = 0x5117_u64;
    let bytes = (0..64 * 1024)
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 56) as u8
        })
        .collect::<Vec<_>>();
    let mut parser = Processor::<StdSyncHandler>::new();
    let mut reference = Processor::<StdSyncHandler>::new();
    for bytes in bytes.chunks(257) {
        parser.advance(&mut live, bytes);
        reference.advance(&mut expected, bytes);
    }
    release.send(()).unwrap();
    finish(&mut live);
    assert_eq!(ansi_snapshot(&live, 1_000_000), ansi_snapshot(&expected, 1_000_000));
    assert_eq!(live.grid.cursor, expected.grid.cursor);
}

#[test]
fn an_alternate_screen_also_reflows_its_hidden_primary_history() {
    let mut live = populated();
    let history = live.history_size();
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(&mut live, b"\x1b[?1049h\x1b[2JALT SCREEN");
    assert_eq!(live.history_size(), 0);
    assert_eq!(live.retained_history_size(), history);
    let mut expected = live.clone();
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(31, 12), move || gate.recv().unwrap()).unwrap();
    expected.resize(test::TermSize::new(31, 12));
    assert_eq!((live.columns(), live.screen_lines()), (31, 12));
    let bytes = b"\x1b[?1049l\r\nreturned primary";
    parser.advance(&mut live, bytes);
    parser.advance(&mut expected, bytes);
    release.send(()).unwrap();
    finish(&mut live);
    assert_eq!(ansi_snapshot(&live, 1_000_000), ansi_snapshot(&expected, 1_000_000));
    assert!(ansi_snapshot(&live, 1_000_000).windows(b"00000:".len()).any(|text| text == b"00000:"));
}

#[test]
fn a_wrapped_line_crossing_the_history_boundary_and_split_escapes_survive() {
    let mut live = populated();
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(&mut live, "界abc ".repeat(15_000).as_bytes());
    // The parser, rather than a second byte parser in the worker, owns this
    // partial escape. The worker receives its completed semantic operation.
    parser.advance(&mut live, b"\x1b[3");
    let mut expected = live.clone();
    let mut reference_parser = Processor::<StdSyncHandler>::new();
    reference_parser.advance(&mut expected, b"\x1b[3");
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(17, 24), move || gate.recv().unwrap()).unwrap();
    expected.resize(test::TermSize::new(17, 24));
    for bytes in [&b"2mTAIL"[..], &b"\x1b[?1049hALT\x1b[?1049l"[..], &b"\r\nPS> accepted\r\n"[..]] {
        parser.advance(&mut live, bytes);
        reference_parser.advance(&mut expected, bytes);
    }
    release.send(()).unwrap();
    finish(&mut live);
    assert_eq!(ansi_snapshot(&live, 1_000_000), ansi_snapshot(&expected, 1_000_000));
}

#[test]
fn clear_during_reflow_cannot_resurrect_history_or_duplicate_events() {
    let mut live = populated();
    let mut expected = live.clone();
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(40, 24), move || gate.recv().unwrap()).unwrap();
    expected.resize(test::TermSize::new(40, 24));
    let bytes = b"\x1b[3J\x1b[2J\x1b[H\x1b]0;new title\x07fresh\x1b[6n";
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(&mut live, bytes);
    parser.advance(&mut expected, bytes);
    release.send(()).unwrap();
    finish(&mut live);
    assert_eq!(ansi_snapshot(&live, 1_000_000), ansi_snapshot(&expected, 1_000_000));
    let events = live.event_proxy.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // The reference above shares this listener; each parser gets one response.
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::Title(title) if title == "new title"))
            .count(),
        2
    );
    assert_eq!(events.iter().filter(|event| matches!(event, Event::PtyWrite(_))).count(), 2);
}

#[test]
#[ignore = "manual optimized-build foreground resize timing"]
fn async_history_reflow_latency_benchmark() {
    for columns in [31, 120, 17] {
        let mut live = populated();
        let mut synchronous = live.clone();
        let initial_history = live.history_size();
        let started = Instant::now();
        synchronous.resize(test::TermSize::new(columns, 24));
        let synchronous_ms = started.elapsed().as_secs_f64() * 1000.;
        let (release, gate) = mpsc::channel();
        let started = Instant::now();
        live.start_reflow(test::TermSize::new(columns, 24), move || gate.recv().unwrap()).unwrap();
        let foreground_ms = started.elapsed().as_secs_f64() * 1000.;
        release.send(()).unwrap();
        finish(&mut live);
        let complete_ms = started.elapsed().as_secs_f64() * 1000.;
        assert_eq!(ansi_snapshot(&live, 1_000_000), ansi_snapshot(&synchronous, 1_000_000));
        eprintln!(
            "reflow columns={columns} history={initial_history} synchronous_ms={synchronous_ms:.3} foreground_ms={foreground_ms:.3} complete_ms={complete_ms:.3}"
        );
    }
}

#[test]
fn scrolling_can_reach_old_history_while_full_reflow_is_paused() {
    let mut live = populated();
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(17, 24), move || gate.recv().unwrap()).unwrap();
    live.scroll_display(Scroll::Top);
    let (view, offset, total) = live.pending_reflow_view().unwrap();
    let visible: String = view.renderable_content().display_iter.map(|cell| cell.c).collect();
    assert!(visible.starts_with("00000:"), "{visible:?}");
    assert_eq!(offset + 24, total);
    assert!(view.grid().display_iter().all(|cell| cell.point.column.0 < 17));
    live.scroll_display(Scroll::Delta(-12));
    let (_, next_offset, _) = live.pending_reflow_view().unwrap();
    assert_eq!(next_offset, offset - 12);
    live.scroll_display(Scroll::Bottom);
    assert!(live.pending_reflow_view().is_none());
    assert!(!live.finish_pending_reflow());
    release.send(()).unwrap();
    finish(&mut live);
    assert_eq!(live.grid.display_offset(), 0);
}

#[test]
fn cursor_and_size_queries_are_answered_before_history_reflow_runs() {
    let mut live = populated();
    let (release, gate) = mpsc::channel();
    live.start_reflow(test::TermSize::new(31, 24), move || gate.recv().unwrap()).unwrap();
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(&mut live, b"\x1b[6n\x1b[18t");
    {
        let events = live.event_proxy.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(events.iter().filter(|event| matches!(event, Event::PtyWrite(_))).count(), 2);
    }
    assert!(!live.finish_pending_reflow());
    release.send(()).unwrap();
    finish(&mut live);
    let events = live.event_proxy.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(events.iter().filter(|event| matches!(event, Event::PtyWrite(_))).count(), 2);
}

/// Matches `zetta benchmark output --output-type unique --size 1000` without
/// allocating a second GiB for the producer. A PTY maps LF to CRLF before the
/// emulator receives it. Run separately and monitor this process's peak memory.
#[test]
#[ignore = "manual optimized-build 1000 MiB resize and scrolling stress test"]
fn unique_1000_mib_resize_and_scroll_stress() {
    assert!(!std::hint::black_box(cfg!(debug_assertions)), "run this workload with --release");
    let bytes = std::env::var("ZETTA_REFLOW_STRESS_MIB")
        .ok()
        .map(|value| value.parse::<usize>().unwrap())
        .unwrap_or(1000)
        * 1024
        * 1024;
    let mut term = Term::new(
        Config { scrolling_history: i32::MAX as usize - 1024, ..Config::default() },
        &test::TermSize::new(100, 24),
        Events::default(),
    );
    let mut parser = Processor::<StdSyncHandler>::new();
    let started = Instant::now();
    for line_index in 0..bytes.div_ceil(80) {
        parser.advance(&mut term, &unique_benchmark_line(line_index as u64));
    }
    let history = term.history_size();
    eprintln!(
        "stress populated bytes={bytes} rows={history} elapsed_s={:.3}",
        started.elapsed().as_secs_f64()
    );
    for columns in [31, 120, 17] {
        let started = Instant::now();
        term.resize_with_async_history(test::TermSize::new(columns, 24)).unwrap();
        let resize_ms = started.elapsed().as_secs_f64() * 1000.;
        let mut max_scroll_ms = 0f64;
        let mut frames = 0usize;
        let mut oldest_seen = false;
        while !term.finish_pending_reflow() {
            assert!(started.elapsed() < Duration::from_secs(180), "reflow timed out");
            for scroll in [Scroll::Top, Scroll::Delta(-10), Scroll::PageDown, Scroll::Bottom] {
                let tick = Instant::now();
                term.scroll_display(scroll);
                if let Some((view, _, _)) = term.pending_reflow_view() {
                    let visible: String =
                        view.renderable_content().display_iter.map(|cell| cell.c).collect();
                    if matches!(scroll, Scroll::Top) {
                        oldest_seen |= visible.starts_with("line 0000000000000000");
                    }
                    assert!(view.grid.display_iter().all(|cell| cell.point.column.0 < columns));
                }
                max_scroll_ms = max_scroll_ms.max(tick.elapsed().as_secs_f64() * 1000.);
            }
            // Model a real frame interval; continuously flooding semantic
            // scroll requests would deliberately prevent a caught-up publish.
            thread::sleep(Duration::from_millis(16));
            frames += 1;
        }
        assert!(oldest_seen, "oldest history was inaccessible during reflow");
        assert!(frames > 0);
        assert_eq!(term.grid.display_offset(), 0);
        eprintln!(
            "stress columns={columns} resize_ms={resize_ms:.3} max_scroll_ms={max_scroll_ms:.3} frames={frames} complete_s={:.3}",
            started.elapsed().as_secs_f64()
        );
        assert_unique_history(&term, bytes.div_ceil(80));
    }
}

fn unique_benchmark_line(line_index: u64) -> [u8; 81] {
    let text = b"0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut line = [b' '; 81];
    line[..5].copy_from_slice(b"line ");
    for (index, value) in line[5..21].iter_mut().enumerate() {
        *value = b"0123456789abcdef"[((line_index >> (4 * (15 - index))) & 15) as usize];
    }
    for (index, value) in line[22..79].iter_mut().enumerate() {
        *value = text[(line_index as usize + index) % text.len()];
    }
    line[79..].copy_from_slice(b"\r\n");
    line
}

fn assert_unique_history(term: &Term<Events>, expected_lines: usize) {
    let mut row = crate::grid::RowText::default();
    let mut logical_line = String::new();
    let mut line_index = 0;
    for line in term.topmost_line().0..=term.bottommost_line().0 {
        row.clear();
        term.grid.row_text(Line(line), &mut row);
        logical_line.push_str(row.as_str());
        if !term.grid.row_wraps(Line(line)) && !logical_line.is_empty() {
            let expected = unique_benchmark_line(line_index as u64);
            assert_eq!(
                logical_line.trim_end().as_bytes(),
                expected[..79].trim_ascii_end(),
                "logical line {line_index}"
            );
            logical_line.clear();
            line_index += 1;
        }
    }
    assert_eq!(line_index, expected_lines);
    assert!(logical_line.is_empty());
}
