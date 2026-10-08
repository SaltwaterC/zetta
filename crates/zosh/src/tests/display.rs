use super::*;

/// The original row-by-row replay, kept as a byte oracle for bounded gaps.
fn reference_scrollback_replay(
    current: &DisplayScreen,
    previous: &DisplayScreen,
) -> Option<Vec<u8>> {
    let height = current.rows();
    let missing = current
        .scrollback
        .evicted
        .checked_sub(previous.scrollback.evicted)
        .filter(|missing| *missing > 0)?;
    if !current.scrollback.active || height < 1 {
        return None;
    }
    let mut output = b"\x1b[r\x1b[0m".to_vec();
    current.correct_stale_rows(previous, missing, &mut output);
    output.extend_from_slice(format!("\x1b[{height};1H").as_bytes());
    let mut wrapping = false;
    let mut dropped = 0;
    for index in previous
        .scrollback
        .evicted
        .saturating_add(u64::from(height))..current.scrollback.evicted
    {
        if let Some(row) = current.carried_row(index) {
            if dropped > 0 {
                DisplayScreen::write_gap(&mut output, dropped, &mut wrapping);
                dropped = 0;
            }
            if !wrapping {
                output.extend_from_slice(b"\r\n");
            }
            output.extend_from_slice(&row.contents);
            wrapping = row.wrapped;
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        DisplayScreen::write_gap(&mut output, dropped, &mut wrapping);
    }
    output.push(b'\r');
    output.extend(std::iter::repeat_n(
        b'\n',
        usize::try_from(missing.min(u64::from(height))).unwrap(),
    ));
    Some(output)
}

fn carried_rows(
    count: usize,
    wrap_mask: usize,
) -> std::sync::Arc<Vec<crate::scrollback::ScrollbackRow>> {
    std::sync::Arc::new(
        (0..count)
            .map(|index| crate::scrollback::ScrollbackRow {
                contents: format!("row{index}").into_bytes(),
                wrapped: wrap_mask & (1 << (index % usize::BITS as usize)) != 0,
            })
            .collect(),
    )
}

#[test]
fn arithmetic_replay_matches_row_by_row_bytes() {
    // Cover first contact and established sessions, both edges of every
    // intersection, disjoint/empty carriage, all wrapping combinations, and
    // absolute indexes whose carried end or screen height would overflow.
    for base in [0, 5, u64::MAX - 12] {
        for height in [1, 3] {
            let mut previous = DisplayScreen::new(height, 20);
            previous.scrollback.evicted = base;
            for missing in 0..=12 {
                let mut current = previous.clone();
                current.scrollback.active = true;
                current.scrollback.evicted = base + missing;
                for first in 0..=12 {
                    current.scrollback.rows_first = base + first;
                    for count in 0..=4 {
                        for wrap_mask in 0..(1 << count) {
                            current.scrollback.rows = carried_rows(count, wrap_mask);
                            assert_eq!(
                                current.scrollback_replay(&previous),
                                reference_scrollback_replay(&current, &previous),
                                "base={base}, height={height}, missing={missing}, \
                                 first={first}, count={count}, wrap_mask={wrap_mask}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn replay_without_new_history_stays_absent() {
    let mut previous = DisplayScreen::new(3, 20);
    previous.scrollback.evicted = 10;
    let mut current = previous.clone();
    current.scrollback.evicted = 20;
    assert_eq!(current.scrollback_replay(&previous), None);
    current.scrollback.active = true;
    for evicted in [0, 9, 10] {
        current.scrollback.evicted = evicted;
        assert_eq!(current.scrollback_replay(&previous), None);
    }
}

#[test]
fn replay_of_maximum_loss_emits_one_exact_gap() {
    let previous = DisplayScreen::new(3, 20);
    let mut current = previous.clone();
    current.scrollback.active = true;
    current.scrollback.evicted = u64::MAX;
    // Empty carriage before, inside, and after the unseen interval must all
    // collapse to one marker, without overflowing the gap count.
    for first in [0, 3, u64::MAX / 2, u64::MAX] {
        current.scrollback.rows_first = first;
        assert_eq!(
            current.scrollback_replay(&previous).unwrap(),
            b"\x1b[r\x1b[0m\x1b[3;1H\r\n\x1b[2m[zosh: 18446744073709551612 lines of scrollback dropped]\x1b[0m\r\n\n\n"
        );
    }
    current.scrollback.rows_first = u64::MAX - 2;
    current.scrollback.rows = carried_rows(4, 1);
    assert_eq!(
        current.scrollback_replay(&previous).unwrap(),
        b"\x1b[r\x1b[0m\x1b[3;1H\r\n\x1b[2m[zosh: 18446744073709551610 lines of scrollback dropped]\x1b[0m\r\nrow0row1\r\n\n\n"
    );
}

/// Run with `cargo test --release replay_cost_tracks_retained_rows -- --ignored --nocapture`.
#[test]
#[ignore = "release-mode scaling probe"]
fn replay_cost_tracks_retained_rows() {
    use std::{hint::black_box, time::Instant};

    let previous = DisplayScreen::new(24, 80);
    for retained in [0, 32, 4096] {
        let mut medians = Vec::new();
        for lost in [10_000, 100_000_000] {
            let mut current = previous.clone();
            current.scrollback.active = true;
            current.scrollback.rows_first = 24 + lost;
            current.scrollback.rows = carried_rows(retained, 0);
            current.scrollback.evicted = 24 + lost + retained as u64;
            let expected = reference_scrollback_replay(&current, &previous).unwrap();
            assert_eq!(current.scrollback_replay(&previous).unwrap(), expected);
            let mut reference_samples = Vec::new();
            for _ in 0..9 {
                let start = Instant::now();
                black_box(reference_scrollback_replay(
                    black_box(&current),
                    black_box(&previous),
                ));
                reference_samples.push(start.elapsed());
            }
            reference_samples.sort();
            let mut samples = Vec::new();
            for _ in 0..9 {
                let start = Instant::now();
                for _ in 0..100 {
                    black_box(black_box(&current).scrollback_replay(black_box(&previous)));
                }
                samples.push(start.elapsed() / 100);
            }
            samples.sort();
            medians.push(samples[4]);
            eprintln!(
                "lost={lost} retained={retained} bytes={} median={:?} original={:?}",
                expected.len(),
                samples[4],
                reference_samples[4]
            );
        }
        // A 10,000-fold growth in loss must not produce linear growth in
        // work. Allow substantial scheduler noise, especially for tiny runs.
        assert!(
            medians[1] < medians[0] * 20 + std::time::Duration::from_micros(100),
            "replay cost grew with discarded rows: {medians:?}"
        );
    }
}

#[test]
fn repeated_padding_preserves_columns_across_protocol_states() {
    let mut first = DisplayScreen::new(3, 40);
    first.feed(b"root ");
    let mut next = first.clone();
    next.feed(b"\x1b[5b20   0\x1b[1;11H99");
    assert_eq!(next.text().trim_end(), "root      99   0");
    let mut physical = DisplayScreen::new(3, 40);
    physical.feed(&first.repaint());
    physical.feed(&next.diff_from(&first));
    assert_eq!(physical.text(), next.text());
}

#[test]
fn a_clear_is_applied_once_when_its_protocol_state_is_displayed() {
    use mosh_rs::{ClientTerminal, screen::OverlayCursor};
    let mut terminal = ClientTerminal::new(DisplayScreen::new(5, 20));
    let marker = b"\x1b]777;zosh-clear-scrollback;1\x07";
    terminal.apply_diff(0, 1, b"old TUI");
    terminal.render(&[], OverlayCursor::Unchanged);
    let clear = [b"\x1b[H\x1b[2Jprompt".as_slice(), marker].concat();
    terminal.apply_diff(1, 3, &clear);
    let output = terminal.render(&[], OverlayCursor::Unchanged);
    assert!(output.windows(4).any(|bytes| bytes == CLEAR_SCROLLBACK));
    let mut physical = DisplayScreen::new(5, 20);
    physical.feed(&output);
    assert_eq!(physical.text().trim(), "prompt");
    assert!(!output.windows(3).any(|bytes| bytes == b"777"));

    // An old arrival and a new diff against the same unacknowledged base
    // must both preserve scrollback created after the first clear.
    terminal.apply_diff(1, 2, &clear);
    assert!(terminal.render(&[], OverlayCursor::Unchanged).is_empty());
    terminal.apply_diff(1, 4, &clear);
    assert!(terminal.render(&[], OverlayCursor::Unchanged).is_empty());
    terminal.apply_diff(1, 5, b"\x1b]777;zosh-clear-scrollback;2\x07");
    assert!(
        terminal
            .render(&[], OverlayCursor::Unchanged)
            .windows(4)
            .any(|b| b == CLEAR_SCROLLBACK)
    );
}

#[test]
fn an_ordinary_redraw_does_not_clear_scrollback() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"old");
    let mut next = previous.clone();
    next.feed(b"\x1b[H\x1b[2Jprompt\x0c");
    assert!(
        !next
            .diff_from(&previous)
            .windows(4)
            .any(|b| b == CLEAR_SCROLLBACK)
    );
}

#[test]
fn a_pending_clear_survives_a_resize_repaint_without_repeating() {
    use mosh_rs::ClientTerminal;
    let mut terminal = ClientTerminal::new(DisplayScreen::new(5, 20));
    terminal.apply_diff(0, 1, b"prompt\x1b]777;zosh-clear-scrollback;1\x07");
    terminal.resize(10, 40);
    let previous = terminal.displayed().clone();
    let repaint = terminal.repaint();
    assert!(terminal.displayed().clears_scrollback_since(&previous));
    let mut output = Vec::new();
    repaint_after_resize(&mut output, &repaint, b"", CLEAR_SCROLLBACK).unwrap();
    assert!(output.ends_with(CLEAR_SCROLLBACK));

    let previous = terminal.displayed().clone();
    terminal.resize(5, 20);
    terminal.repaint();
    assert!(!terminal.displayed().clears_scrollback_since(&previous));
}

#[test]
fn opening_clears_the_visible_page_without_entering_alternate_screen() {
    let mut output = Vec::new();
    open(&mut output).unwrap();
    assert_eq!(output, DISPLAY_OPEN);
    assert!(
        !output
            .windows(b"\x1b[?1049h".len())
            .any(|window| window == b"\x1b[?1049h")
    );
}

#[test]
fn resizing_clears_before_writing_the_full_repaint() {
    let mut output = Vec::new();
    repaint_after_resize(&mut output, b"screen", b"modes", b"scrollback").unwrap();
    assert_eq!(&output[..DISPLAY_RESIZE.len()], DISPLAY_RESIZE);
    assert_eq!(&output[DISPLAY_RESIZE.len()..], b"screenmodesscrollback");
}

#[test]
fn a_scrolled_screen_uses_physical_newlines() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"zero\r\none\r\ntwo\r\nthree\r\nfour");
    let mut current = DisplayScreen::new(5, 20);
    current.feed(b"two\r\nthree\r\nfour\r\nfive\r\nsix");

    assert_eq!(
        scroll_plan(&previous, &current),
        Some(ScrollPlan {
            lines_scrolled: 2,
            scroll_height: 3
        })
    );
    let output = current.diff_from(&previous);
    assert!(output.starts_with(b"\r\n\n"));

    let mut physical = previous.clone();
    physical.feed(&output);
    assert_eq!(physical.text(), current.text());
    assert_eq!(physical.cursor(), current.cursor());
}

#[test]
fn a_non_scrolling_redraw_does_not_scroll_the_terminal() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"one\r\ntwo");
    let mut current = previous.clone();
    current.feed(b"\rX");

    let output = current.diff_from(&previous);
    assert!(!output.starts_with(b"\r\n"));
    assert!(scroll_plan(&previous, &current).is_none());
}

#[test]
fn an_unchanged_top_row_is_not_mistaken_for_a_scroll() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"top\r\n");
    let mut current = previous.clone();
    current.feed(b"next");

    assert!(scroll_plan(&previous, &current).is_none());
    assert!(!current.diff_from(&previous).starts_with(b"\r\n"));
}

#[test]
fn the_first_frame_restores_the_protocol_cursor_state() {
    let previous = DisplayScreen::new(5, 20);
    let mut current = previous.clone();
    current.feed(b"hello");

    assert!(
        current
            .diff_from(&previous)
            .windows(6)
            .any(|window| { window == b"\x1b[?25h" })
    );
}

#[test]
fn a_resize_clears_and_repaints_the_new_geometry() {
    let mut previous = DisplayScreen::new(3, 10);
    previous.feed(b"old screen");
    let mut current = previous.clone();
    current.resize(5, 20);
    current.feed(b"new screen");

    let output = current.diff_from(&previous);
    assert!(output.starts_with(DISPLAY_RESIZE));

    // A real terminal has already applied the geometry change before this
    // output arrives. Replaying the output on a blank screen models that
    // terminal after the clear sequence.
    let mut physical = DisplayScreen::new(5, 20);
    physical.feed(&output);
    assert_eq!(physical.text(), current.text());
    assert_eq!(physical.cursor(), current.cursor());
}

#[test]
fn display_diff_carries_input_modes_from_the_protocol_screen() {
    let mut previous = DisplayScreen::new(5, 20);
    previous.feed(b"\x1b[?2004l");
    let mut current = previous.clone();
    current.feed(b"\x1b[?2004h");

    assert!(
        current
            .diff_from(&previous)
            .windows(8)
            .any(|window| { window == b"\x1b[?2004h" })
    );
}

#[test]
fn a_repaint_preserves_input_modes_already_set_on_the_terminal() {
    let mut screen = DisplayScreen::new(5, 20);
    screen.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1000hhello");

    let repaint = screen.repaint();

    assert!(!repaint.windows(5).any(|window| window == b"\x1b[?1l"));
    assert!(!repaint.windows(8).any(|window| window == b"\x1b[?2004l"));
    assert!(!repaint.windows(8).any(|window| window == b"\x1b[?1000l"));
    assert!(repaint.windows(5).any(|window| window == b"hello"));
}

#[test]
fn repeated_output_keeps_the_physical_screen_in_sync() {
    let mut previous = DisplayScreen::new(24, 80);
    previous.feed(b"banner one\r\nbanner two\r\nbanner three\r\n> ");
    let mut physical = previous.clone();
    for number in 1..=30 {
        let mut current = previous.clone();
        current.feed(format!("LINE{number}\r\n").as_bytes());
        let output = current.diff_from(&previous);
        physical.feed(&output);
        assert_eq!(physical.text(), current.text(), "line {number}");
        previous = current;
    }
}

// ---------------------------------------------------------------------- //
// Scrollback carriage: the rows the server sends, and where they land.
//
// These drive both halves against a model of a real terminal, because the
// claim is not about the bytes emitted but about what a terminal does with
// them. `PROTOCOL.md` is the specification.
// ---------------------------------------------------------------------- //

use mosh_rs::{ClientTerminal, screen::OverlayCursor};

/// Just enough of `zosh-server` to produce the diffs a client would see.
struct FakeServer {
    parser: vt100::Parser,
    base: vt100::Screen,
    carry_scrollback: bool,
}

impl FakeServer {
    fn new(rows: u16, cols: u16, carry_scrollback: bool) -> Self {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser
            .screen_mut()
            .set_capture_evicted_rows(carry_scrollback);
        let base = parser.screen().clone();
        Self {
            parser,
            base,
            carry_scrollback,
        }
    }

    fn run(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// The diff for everything since the last one, which the caller is taken
    /// to have acknowledged.
    fn diff(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.carry_scrollback {
            let (first, rows) = self.parser.screen_mut().take_evicted_rows();
            if !rows.is_empty() {
                let mut payload = Vec::new();
                for row in &rows {
                    payload.push(u8::from(row.wrapped));
                    payload.extend_from_slice(&(row.contents.len() as u32).to_be_bytes());
                    payload.extend_from_slice(&row.contents);
                }
                let encoded = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD_NO_PAD.encode(&payload)
                };
                out.extend_from_slice(format!("\x1b]777;zosh-scrollback;{first};").as_bytes());
                out.extend_from_slice(encoded.as_bytes());
                out.push(0x07);
            }
        }
        out.extend_from_slice(&self.parser.screen().state_diff(&self.base));
        self.base = self.parser.screen().clone();
        out
    }

    fn text(&self) -> String {
        self.parser.screen().contents()
    }
}

/// A real terminal, as far as this matters: a screen, and a history that only
/// ever gains the rows it is asked to scroll off the top.
struct ModelTerminal {
    parser: vt100::Parser,
    history: Vec<String>,
}

impl ModelTerminal {
    fn new(rows: u16, cols: u16) -> Self {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.screen_mut().set_capture_evicted_rows(true);
        let mut model = Self {
            parser,
            history: Vec::new(),
        };
        model.write(DISPLAY_OPEN);
        model
    }

    fn write(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        let (_, rows) = self.parser.screen_mut().take_evicted_rows();
        for row in rows {
            let mut text = String::new();
            let mut plain = vt100::Parser::new(1, self.parser.screen().size().1, 0);
            plain.process(&row.contents);
            text.push_str(plain.screen().contents().trim_end());
            self.history.push(text);
        }
    }

    fn text(&self) -> String {
        self.parser.screen().contents()
    }
}

/// Runs a program's output through both halves and reports what the terminal
/// ended up holding: everything in its history, then what is on its screen.
fn replay(rows: u16, cols: u16, carry_scrollback: bool, chunks: &[&[u8]]) -> (Vec<String>, String) {
    let mut server = FakeServer::new(rows, cols, carry_scrollback);
    let mut terminal = ClientTerminal::new(DisplayScreen::new(rows, cols));
    let mut model = ModelTerminal::new(rows, cols);
    for (index, chunk) in chunks.iter().enumerate() {
        server.run(chunk);
        let state = index as u64 + 1;
        assert!(terminal.apply_diff(state - 1, state, &server.diff()));
        model.write(&terminal.render(&[], OverlayCursor::Unchanged));
    }
    assert_eq!(
        model.text().trim_end(),
        server.text().trim_end(),
        "the screen the client painted is not the screen the server has"
    );
    (model.history.clone(), model.text())
}

fn lines(from: usize, to: usize) -> Vec<u8> {
    (from..to).fold(Vec::new(), |mut out, n| {
        out.extend_from_slice(format!("LINE{n}\r\n").as_bytes());
        out
    })
}

/// The regression this whole extension exists for: a burst that moves the
/// screen by more than a screenful between two states. Stock Mosh, and zosh
/// before the extension, kept only the last screen of it.
#[test]
fn a_burst_larger_than_the_screen_keeps_every_line() {
    let (history, screen) = replay(24, 80, true, &[b"START\r\n", &lines(0, 500)]);

    let kept: Vec<&String> = history.iter().filter(|line| !line.is_empty()).collect();
    assert_eq!(kept[0], "START");
    for n in 0..476 {
        assert_eq!(kept[n + 1], &format!("LINE{n}"), "line {n} went missing");
    }
    // And the rest is on the screen, which is where the 500 lines end.
    assert!(screen.contains("LINE499"));
}

/// Without the extension the client is back to inferring scrolls from the
/// screen, which is all stock Mosh has ever been able to do.
#[test]
fn without_the_extension_a_burst_keeps_only_the_last_screen() {
    let (history, _) = replay(24, 80, false, &[b"START\r\n", &lines(0, 500)]);

    assert!(
        history.iter().all(|line| !line.starts_with("LINE1")),
        "a stock session cannot have kept these: {history:?}"
    );
}

/// A paced session scrolls a row at a time, which the inference already
/// handled. The extension must not double it up.
#[test]
fn a_line_at_a_time_is_kept_exactly_once() {
    let chunks: Vec<Vec<u8>> = (0..60)
        .map(|n| format!("LINE{n}\r\n").into_bytes())
        .collect();
    let borrowed: Vec<&[u8]> = chunks.iter().map(Vec::as_slice).collect();
    let (history, _) = replay(24, 80, true, &borrowed);

    let kept: Vec<&String> = history.iter().filter(|line| !line.is_empty()).collect();
    // 60 lines plus the blank the cursor sits on, through a 24-row window.
    assert_eq!(kept.len(), 37, "37 of 60 lines have left a 24-row screen");
    for (n, line) in kept.iter().enumerate() {
        assert_eq!(*line, &format!("LINE{n}"));
    }
}

#[test]
fn colour_and_wrapping_survive_the_round_trip() {
    let mut program = b"\x1b[31mred line\x1b[m\r\n".to_vec();
    program.extend_from_slice(&"w".repeat(100).into_bytes());
    program.extend_from_slice(b"\r\n");
    program.extend_from_slice(&lines(0, 60));
    let mut server = FakeServer::new(24, 80, true);
    let mut terminal = ClientTerminal::new(DisplayScreen::new(24, 80));
    server.run(&program);
    assert!(terminal.apply_diff(0, 1, &server.diff()));
    let painted = terminal.render(&[], OverlayCursor::Unchanged);

    let text = String::from_utf8_lossy(&painted).into_owned();
    assert!(text.contains("\x1b[31mred line"), "the colour was dropped");
    // The wrapped line is written as its two physical rows, and the first of
    // them fills its last column so the terminal wraps it back together.
    assert!(
        text.contains(&"w".repeat(80)),
        "the wrapped row was truncated"
    );
}

/// A resize during a burst must not be the one way to lose output: the rows
/// go into the history before the repaint clears the screen.
#[test]
fn a_resize_mid_burst_still_keeps_the_history() {
    let mut server = FakeServer::new(24, 80, true);
    let mut terminal = ClientTerminal::new(DisplayScreen::new(24, 80));
    let mut model = ModelTerminal::new(24, 80);

    server.run(&lines(0, 200));
    assert!(terminal.apply_diff(0, 1, &server.diff()));
    model.write(&terminal.render(&[], OverlayCursor::Unchanged));

    // Now the same burst again, but the frame that carries it is a resize.
    server.run(&lines(200, 400));
    server.parser.screen_mut().set_size(30, 80);
    terminal.resize(30, 80);
    assert!(terminal.apply_diff(1, 2, &server.diff()));
    model.parser.screen_mut().set_size(30, 80);
    model.write(&terminal.render(&[], OverlayCursor::Unchanged));

    let kept: Vec<&String> = model
        .history
        .iter()
        .filter(|line| !line.is_empty())
        .collect();
    for n in 0..340 {
        assert!(
            kept.iter().any(|line| *line == &format!("LINE{n}")),
            "line {n} was lost across the resize"
        );
    }
}

/// Rows the server had to drop are reported rather than silently skipped, and
/// the screen still lands where it should.
#[test]
fn dropped_rows_are_reported_where_they_would_have_been() {
    let mut server = FakeServer::new(24, 80, true);
    let mut terminal = ClientTerminal::new(DisplayScreen::new(24, 80));
    let mut model = ModelTerminal::new(24, 80);

    server.run(&lines(0, 300));
    let mut diff = server.diff();
    // Rewrite the marker to claim a much later starting index: the shape of a
    // server that dropped its oldest unacknowledged rows.
    let marker_end = diff
        .iter()
        .position(|&byte| byte == 0x07)
        .expect("a marker");
    let mut trimmed = b"\x1b]777;zosh-scrollback;270;".to_vec();
    trimmed.push(0x07);
    trimmed.extend_from_slice(&diff[marker_end + 1..]);
    diff = trimmed;

    assert!(terminal.apply_diff(0, 1, &diff));
    model.write(&terminal.render(&[], OverlayCursor::Unchanged));

    // 270 rows left the screen and none were carried. The 24 the terminal was
    // showing are kept as it was showing them, so what is reported gone is the
    // 246 it never saw.
    assert!(
        model
            .history
            .iter()
            .any(|line| line.contains("246 lines of scrollback dropped")),
        "{:?}",
        model.history
    );
    assert_eq!(model.text().trim_end(), server.text().trim_end());
}

/// End to end: a server that hides a clipboard read and a cursor-position
/// query inside a carried row gets neither to the terminal.
#[test]
fn a_query_inside_a_carried_row_never_reaches_the_terminal() {
    let mut server = FakeServer::new(4, 40, true);
    let mut terminal = ClientTerminal::new(DisplayScreen::new(4, 40));
    server.run(&lines(0, 10));
    let diff = server.diff();
    // Swap the honest marker for one whose first row carries the queries.
    let marker_end = diff
        .iter()
        .position(|&byte| byte == 0x07)
        .expect("a marker");
    let mut hostile = {
        use base64::Engine as _;
        let row = b"LINE0\x1b]52;c;?\x07\x1b[6n\x1b[c\x1bP$qm\x1b\\";
        let mut payload = vec![0];
        payload.extend_from_slice(&(row.len() as u32).to_be_bytes());
        payload.extend_from_slice(row);
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&payload);
        format!("\x1b]777;zosh-scrollback;0;{encoded}\x07").into_bytes()
    };
    hostile.extend_from_slice(&diff[marker_end + 1..]);
    assert!(terminal.apply_diff(0, 1, &hostile));

    let painted = terminal.render(&[], OverlayCursor::Unchanged);

    let text = String::from_utf8_lossy(&painted);
    assert!(text.contains("LINE0"), "the row's text was lost: {text:?}");
    for query in ["\x1b]52", "\x1b[6n", "\x1b[c", "\x1bP"] {
        assert!(
            !text.contains(query),
            "{query:?} reached the terminal: {text:?}"
        );
    }
}
