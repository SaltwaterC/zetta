use super::*;

#[test]
fn pane_resize_carries_displaced_text_and_preserves_the_remaining_screen() {
    let mut terminal = TerminalState::new(4, 12);
    terminal.set_scrollback_budget(64 * 1024);
    terminal.process(b"abcdefghijkl\r\nsecond-line\r\nthird-line\r\nprompt> ");
    terminal.snapshot_for_state(1);
    terminal.acknowledge(1);
    terminal.resize(3, 6);
    let frame = terminal.frame_from(None).unwrap();
    assert_eq!(frame.resize, Some((3, 6)));
    let (_, rows) = read_scrollback(&frame.host_bytes).expect("resize must carry history");
    // Read every carried row with its own geometry, followed by the visible
    // screen: neither the right edge nor bottom rows may disappear.
    let mut contents = String::new();
    for row in rows {
        let mut parser = vt100::Parser::new(1, 6, 0);
        parser.process(&row.0);
        contents.push_str(&parser.screen().contents());
        if !row.1 {
            contents.push('\n');
        }
    }
    contents.push_str(&terminal.parser.screen().contents());
    assert_eq!(contents, "abcdefghijkl\nsecond-line\nthird-line\nprompt> ");

    let mut peer = vt100::Parser::new(3, 6, 0);
    peer.process(&frame.host_bytes);
    assert_eq!(
        peer.screen().contents(),
        terminal.parser.screen().contents()
    );
    terminal.snapshot_for_state(2);
    terminal.acknowledge(2);
    assert!(read_scrollback(&terminal.diff_from_ack()).is_none());
}

#[test]
fn uncollected_rows_keep_the_width_they_had_before_resize() {
    let mut terminal = TerminalState::new(2, 12);
    terminal.process(b"abcdefghijkl\r\nsecond\r\nthird");
    terminal.resize(2, 6);
    terminal.set_scrollback_budget(64 * 1024);
    let (_, rows) = read_scrollback(&terminal.diff_from_ack()).unwrap();
    assert!(rows.iter().any(|row| row.0 == b"abcdefghijkl"));
}

#[test]
fn htop_ascii_padding_survives_server_state_updates() {
    let mut terminal = TerminalState::new(3, 80);
    // ncurses emits REP for runs of spaces in htop's ASCII mode,
    // triggered when macOS forwards LC_CTYPE=UTF-8 to a Linux host.
    terminal.process(b"\x1b[2;5H275 \x1b[90;1mroot \x1b[");
    terminal.process(b"6b\x1b[0m 20   0 43360");
    let mut client = vt100::Parser::new(3, 80, 0);
    client.process(&terminal.diff_from_ack());
    assert_eq!(client.screen().cell(1, 20).unwrap().contents(), "2");
    assert_eq!(client.screen().cell(1, 30).unwrap().contents(), "6");
    terminal.snapshot_for_state(1);
    terminal.acknowledge(1);
    terminal.process(b"\x1b[2;31H9");
    client.process(&terminal.diff_from_ack());
    assert_eq!(client.screen().cell(1, 20).unwrap().contents(), "2");
    assert_eq!(client.screen().cell(1, 30).unwrap().contents(), "9");
}

#[test]
fn terminal_queries_follow_snapshots_and_acknowledgements() {
    let mut terminal = TerminalState::new(24, 80);
    assert_eq!(terminal.add_query(b"q1".to_vec()), 1);
    terminal.snapshot_for_state(1);
    assert_eq!(terminal.add_query(b"q2".to_vec()), 2);
    assert_eq!(terminal.pending_query_bytes, 4);
    terminal.snapshot_for_state(2);
    assert_eq!(
        terminal.queries_from_ack(),
        &[
            TerminalQuery {
                id: 1,
                bytes: b"q1".to_vec(),
            },
            TerminalQuery {
                id: 2,
                bytes: b"q2".to_vec(),
            },
        ]
    );

    // A retransmitted state still sees both IDs before the ACK. Once
    // state 1 is acknowledged, only the newer query remains to repeat.
    terminal.acknowledge(1);
    assert_eq!(terminal.pending_query_bytes, 2);
    assert_eq!(
        terminal.queries_from_ack(),
        &[TerminalQuery {
            id: 2,
            bytes: b"q2".to_vec(),
        }]
    );
    terminal.acknowledge(2);
    assert!(terminal.queries_from_ack().is_empty());
    assert_eq!(terminal.pending_query_bytes, 0);
}

/// A title is terminal state that `vt100::Screen::state_diff` knows
/// nothing about, so the server has to restate it or it never crosses.
/// Zetta reports a pane's working directory this way, which is what makes
/// this more than cosmetic.
#[test]
fn a_window_title_is_carried_in_the_state_the_client_is_sent() {
    let mut state = TerminalState::new(24, 80);
    state.process(b"\x1b]2;zetta-cwd:/tmp/project\x1b\\");

    let diff = state.diff_from_ack();
    let text = String::from_utf8_lossy(&diff);
    assert!(
        text.contains("\u{1b}]0;zetta-cwd:/tmp/project\u{7}"),
        "the title has to be restated in the diff: {text:?}"
    );
    // And in front of the contents, so a client that is still painting the
    // screen it belongs to has it by the time the frame is shown.
    assert!(text.starts_with("\u{1b}]0;"), "{text:?}");
}

/// Cumulative diffs are sent until one is acknowledged, so an unchanged
/// title must not be restated forever — and a changed one must be.
#[test]
fn an_acknowledged_title_is_not_restated_and_a_new_one_is() {
    let mut state = TerminalState::new(24, 80);
    state.process(b"\x1b]2;first\x07");
    state.snapshot_for_state(1);
    state.acknowledge(1);
    assert!(
        !String::from_utf8_lossy(&state.diff_from_ack()).contains("\u{1b}]0;"),
        "a title the peer already shows is not state it is missing"
    );

    state.process(b"\x1b]2;second\x07");
    assert!(
        String::from_utf8_lossy(&state.diff_from_ack()).contains("\u{1b}]0;second\u{7}"),
        "a title that changed since the acknowledged state has to be sent"
    );
}

/// OSC 1 is an icon name rather than a window title, and a sequence that
/// never terminates must not be able to make the server hold an unbounded
/// string.
#[test]
fn the_title_scanner_reads_titles_and_only_titles() {
    let mut scanner = TitleScanner::default();
    scanner.feed(b"\x1b]1;icon-name\x07");
    assert_eq!(scanner.title(), None);

    scanner.feed(b"\x1b]0;icon and title\x07");
    assert_eq!(scanner.title(), Some("icon and title"));

    scanner.feed(b"\x1b]2;title alone\x1b\\");
    assert_eq!(scanner.title(), Some("title alone"));

    let mut unbounded = TitleScanner::default();
    unbounded.feed(b"\x1b]2;");
    unbounded.feed(&vec![b'x'; MAX_TITLE_BYTES * 2]);
    unbounded.feed(b"\x07");
    assert_eq!(
        unbounded.title(),
        None,
        "an unbounded title is abandoned rather than buffered"
    );
}

#[test]
fn terminal_diff_reconstructs_visible_state() {
    let mut state = TerminalState::new(24, 80);
    state.process(b"hello");
    let diff = state.diff_from_ack();

    let mut peer = vt100::Parser::new(24, 80, 0);
    peer.process(&diff);
    assert_eq!(peer.screen().contents(), state.parser.screen().contents());
}

#[test]
fn clear_scrollback_marker_survives_chunk_boundaries() {
    let mut state = TerminalState::new(24, 80);
    state.process(b"before\x1b[3");
    state.process(b"Jafter");

    assert_eq!(state.scrollback_clear_count(), 1);
    assert_eq!(
        state.diff_from_ack(),
        b"\x1b]777;zosh-clear-scrollback;1\x07beforeafter"
    );
}

#[test]
fn clear_scrollback_marker_is_sent_when_the_screen_is_unchanged() {
    let mut state = TerminalState::new(24, 80);
    state.process(b"\x1b[3J");

    assert_eq!(
        state.diff_from_ack(),
        b"\x1b]777;zosh-clear-scrollback;1\x07"
    );
}

#[test]
fn clear_scrollback_detector_matches_moshs_first_parameter_rules() {
    for sequence in [b"\x1b[03J".as_slice(), b"\x1b[3;0J", b"\xc2\x9b3J"] {
        let mut state = TerminalState::new(24, 80);
        state.process(sequence);
        assert_eq!(state.scrollback_clear_count(), 1, "{sequence:?}");
    }

    for sequence in [b"\x1b[30J".as_slice(), b"\x1b[?3J", b"\x1b[3 J"] {
        let mut state = TerminalState::new(24, 80);
        state.process(sequence);
        assert_eq!(state.scrollback_clear_count(), 0, "{sequence:?}");
    }
}

#[test]
fn unicode_prompt_bytes_do_not_poison_clear_detection() {
    let mut state = TerminalState::new(24, 80);

    // This is present in the captured real zsh prompt. U+276F is encoded
    // as E2 9D AF; the old raw-byte detector interpreted 9D as C1 OSC.
    state.process("❯".as_bytes());
    state.process(b"\x1b[3J");

    assert_eq!(state.scrollback_clear_count(), 1);
}

#[test]
fn unicode_prompt_and_clear_in_same_chunk_are_detected() {
    let mut state = TerminalState::new(24, 80);

    state.process("prompt ❯ \x1b[3J".as_bytes());

    assert_eq!(state.scrollback_clear_count(), 1);
}

#[test]
fn utf8_c1_csi_is_still_supported() {
    let mut state = TerminalState::new(24, 80);

    // U+009B CSI encoded as UTF-8.
    state.process(b"\xc2\x9b3J");

    assert_eq!(state.scrollback_clear_count(), 1);
}

#[test]
fn utf8_c1_csi_survives_pty_chunk_boundary() {
    let mut state = TerminalState::new(24, 80);

    state.process(b"\xc2");
    state.process(b"\x9b3J");

    assert_eq!(state.scrollback_clear_count(), 1);
}

#[test]
fn raw_utf8_continuation_byte_is_not_a_c1_control() {
    let mut state = TerminalState::new(24, 80);

    // Standalone 9D is not a valid UTF-8 encoding of U+009D.
    state.process(b"\x9d");
    state.process(b"\x1b[3J");

    assert_eq!(state.scrollback_clear_count(), 1);
}

#[test]
fn clear_scrollback_marker_is_not_found_inside_a_string() {
    let mut state = TerminalState::new(24, 80);
    state.process(b"\x1b]title \x1b[3J still title\x07");
    state.process(b"\x1bPdata \x1b[3J still data\x1b\\");

    assert_eq!(state.scrollback_clear_count(), 0);
}

#[test]
fn acknowledged_snapshot_stops_repeating_the_clear_marker() {
    let mut state = TerminalState::new(24, 80);
    state.process(b"\x1b[3J");
    assert!(!state.diff_from_ack().is_empty());
    state.snapshot_for_state(1);
    state.acknowledge(1);

    assert!(state.diff_from_ack().is_empty());
}

// ------------------------------------------------------------------ //
// Scrollback carriage. `PROTOCOL.md` is the specification; the reading
// half is `crates/zosh/src/scrollback.rs`, which is tested against the
// same framing.
// ------------------------------------------------------------------ //

/// One row as the reading half recovers it: its contents, and whether
/// the logical line continued onto the row below.
type ReadRow = (Vec<u8>, bool);

/// The marker a diff carries, as `(first_index, rows)`. Deliberately a
/// second implementation of the client's reader: the two agreeing is the
/// claim being tested.
fn read_scrollback(diff: &[u8]) -> Option<(u64, Vec<ReadRow>)> {
    let start = diff
        .windows(SCROLLBACK_MARKER_PREFIX.len())
        .position(|window| window == SCROLLBACK_MARKER_PREFIX)?
        + SCROLLBACK_MARKER_PREFIX.len();
    let end = start + diff[start..].iter().position(|&byte| byte == 0x07)?;
    let body = &diff[start..end];
    let semicolon = body.iter().position(|&byte| byte == b';')?;
    let first: u64 = std::str::from_utf8(&body[..semicolon]).ok()?.parse().ok()?;
    let payload = STANDARD_NO_PAD.decode(&body[semicolon + 1..]).ok()?;

    let mut rows = Vec::new();
    let mut rest = payload.as_slice();
    while !rest.is_empty() {
        let flags = rest[0];
        let length = u32::from_be_bytes(rest[1..5].try_into().ok()?) as usize;
        rows.push((
            rest[SCROLLBACK_ROW_HEADER..SCROLLBACK_ROW_HEADER + length].to_vec(),
            flags & SCROLLBACK_FLAG_WRAPPED != 0,
        ));
        rest = &rest[SCROLLBACK_ROW_HEADER + length..];
    }
    Some((first, rows))
}

fn row_text(rows: &[ReadRow]) -> Vec<String> {
    rows.iter()
        .map(|(bytes, _)| String::from_utf8_lossy(bytes).into_owned())
        .collect()
}

/// A stock Mosh client has no way to ask, so it must get exactly what it
/// got before: a screen, and nothing wrapped around it. The rows are
/// collected on spec until the first state settles the question, and a
/// client that never asks never sees them.
#[test]
fn nothing_is_carried_until_a_client_asks_for_it() {
    let mut state = TerminalState::new(2, 20);
    state.process(b"one\r\ntwo\r\nthree\r\n");
    assert!(state.pending_scrollback_bytes() > 0, "held on spec");
    state.forget_scrollback();
    assert_eq!(state.pending_scrollback_bytes(), 0);
    state.process(b"four\r\n");

    let diff = state.diff_from_ack();
    assert!(read_scrollback(&diff).is_none(), "{diff:?}");
    assert!(
        !String::from_utf8_lossy(&diff).contains("zosh-scrollback"),
        "{diff:?}"
    );
}

#[test]
fn rows_that_scrolled_off_are_carried_oldest_first_from_zero() {
    let mut state = TerminalState::new(2, 20);
    // The program starts writing before the first datagram can arrive, so
    // what it wrote in the meantime is collected on spec and is there for
    // a client that turns out to want it.
    state.process(b"before\r\nalso before\r\n");
    state.set_scrollback_budget(64 * 1024);
    state.process(b"one\r\ntwo\r\nthree\r\n");

    let (first, rows) = read_scrollback(&state.diff_from_ack()).expect("a marker");
    assert_eq!(first, 0);
    assert_eq!(row_text(&rows), vec!["before", "also before", "one", "two"]);
}

#[test]
fn an_acknowledged_row_is_not_carried_again_and_the_index_continues() {
    let mut state = TerminalState::new(2, 20);
    state.set_scrollback_budget(64 * 1024);
    state.process(b"one\r\ntwo\r\nthree\r\n");
    state.snapshot_for_state(1);
    state.acknowledge(1);

    // Nothing has scrolled since, so there is nothing left to say.
    assert!(read_scrollback(&state.diff_from_ack()).is_none());
    assert_eq!(state.pending_scrollback_bytes(), 0);

    state.process(b"four\r\n");
    let (first, rows) = read_scrollback(&state.diff_from_ack()).expect("a marker");
    assert_eq!(first, 2, "the index is absolute, not per-diff");
    assert_eq!(row_text(&rows), vec!["three"]);
}

/// A cumulative diff is recomputed from the peer's acknowledged state on
/// every pass, so an unacknowledged row has to keep travelling.
#[test]
fn an_unacknowledged_row_is_carried_by_every_later_diff() {
    let mut state = TerminalState::new(2, 20);
    state.set_scrollback_budget(64 * 1024);
    state.process(b"one\r\ntwo\r\nthree\r\n");
    let first_diff = read_scrollback(&state.diff_from_ack()).expect("a marker");
    state.process(b"four\r\n");
    let second_diff = read_scrollback(&state.diff_from_ack()).expect("a marker");

    assert_eq!(first_diff.0, 0);
    assert_eq!(second_diff.0, 0);
    assert_eq!(row_text(&second_diff.1), vec!["one", "two", "three"]);
}

#[test]
fn a_wrapped_row_says_so_and_its_colour_travels_with_it() {
    let mut state = TerminalState::new(2, 4);
    state.set_scrollback_budget(64 * 1024);
    state.process(b"\x1b[31mabcdefgh\x1b[m\r\nx\r\ny\r\n");

    let (_, rows) = read_scrollback(&state.diff_from_ack()).expect("a marker");
    assert!(rows[0].1, "a full row that continued below");
    assert!(!rows[1].1, "the row it continued onto ended the line");
    assert!(
        String::from_utf8_lossy(&rows[0].0).contains("\x1b[31m"),
        "{:?}",
        rows[0].0
    );
}

#[test]
fn the_budget_is_clamped_and_reports_pressure_once_it_is_reached() {
    let mut state = TerminalState::new(2, 20);
    // Below the floor: a client cannot ask for a budget so small that a
    // single screen cannot fit through it.
    state.set_scrollback_budget(1);
    assert!(!state.scrollback_over_budget());

    for line in 0..4000 {
        state.process(format!("line {line}\r\n").as_bytes());
    }
    assert!(
        state.scrollback_over_budget(),
        "{} bytes pending against a {SCROLLBACK_BUDGET_MIN} byte floor",
        state.pending_scrollback_bytes()
    );
}

/// What a session does once its client has gone: a Mosh session outliving
/// its client is the point, so the history gives way rather than the
/// program. The skipped indices are what let the client say so.
#[test]
fn dropping_over_budget_skips_indices_rather_than_renumbering() {
    let mut state = TerminalState::new(2, 20);
    state.set_scrollback_budget(SCROLLBACK_BUDGET_MIN);
    for line in 0..4000 {
        state.process(format!("line {line}\r\n").as_bytes());
    }
    state.drop_scrollback_over_budget();
    assert!(state.pending_scrollback_bytes() <= SCROLLBACK_BUDGET_MIN);

    let (first, rows) = read_scrollback(&state.diff_from_ack()).expect("a marker");
    assert!(first > 0, "the oldest rows were dropped");
    assert_eq!(
        row_text(&rows)[0],
        format!("line {first}"),
        "the index still names the row it is attached to"
    );
}

/// A program that clears the history is asking for the rows above the
/// screen to be forgotten, including ones still in flight.
#[test]
fn a_clear_drops_the_rows_that_were_still_waiting() {
    let mut state = TerminalState::new(2, 20);
    state.set_scrollback_budget(64 * 1024);
    state.process(b"one\r\ntwo\r\nthree\r\n");
    assert!(state.pending_scrollback_bytes() > 0);

    state.process(b"\x1b[3J");
    assert_eq!(state.pending_scrollback_bytes(), 0);
    // The count still moved, so the marker still travels: the client has
    // to learn where the screen now sits even when it is given no rows.
    let diff = state.diff_from_ack();
    let (first, rows) = read_scrollback(&diff).expect("a marker");
    assert_eq!(first, 2);
    assert!(rows.is_empty());
    assert!(String::from_utf8_lossy(&diff).contains("zosh-clear-scrollback"));
}

/// The framing, byte for byte, as `PROTOCOL.md` documents it. The client
/// carries the same vector.
#[test]
fn the_marker_is_framed_exactly_as_the_protocol_says() {
    let mut rows = VecDeque::new();
    rows.push_back(PendingRow {
        index: 7,
        contents: b"hi".to_vec(),
        wrapped: true,
    });
    assert_eq!(
        scrollback_marker(rows.iter(), 8),
        b"\x1b]777;zosh-scrollback;7;AQAAAAJoaQ\x07".to_vec()
    );
    // No rows, but a count that moved: the first index is the count.
    assert_eq!(
        scrollback_marker(std::iter::empty(), 12),
        b"\x1b]777;zosh-scrollback;12;\x07".to_vec()
    );
}

// ------------------------------------------------------------------ //
// Frames diffed from a state the peer has probably received rather
// than from the acknowledged one.
// ------------------------------------------------------------------ //

fn host_bytes(state: &TerminalState, base: Option<u64>) -> Vec<u8> {
    state.frame_from(base).expect("a held base").host_bytes
}

#[test]
fn a_frame_from_a_sent_state_rebuilds_the_screen_on_top_of_it() {
    let mut state = TerminalState::new(5, 20);
    state.process(b"first");
    let first = host_bytes(&state, None);
    state.snapshot_for_state(1);
    state.process(b"\r\nsecond");
    let second = host_bytes(&state, Some(1));
    assert!(
        !String::from_utf8_lossy(&second).contains("first"),
        "what the base already shows is not sent again: {second:?}"
    );
    // The peer applies it to the state it was diffed from.
    let mut peer = vt100::Parser::new(5, 20, 0);
    peer.process(&first);
    peer.process(&second);
    assert_eq!(peer.screen().contents(), state.parser.screen().contents());
}

#[test]
fn rows_a_sent_state_carried_are_not_carried_again() {
    let mut state = TerminalState::new(3, 20);
    state.set_scrollback_budget(64 * 1024);
    state.process(b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
    state.snapshot_for_state(1);
    let carried = state.evicted_total();
    state.process(b"\r\nr5\r\nr6");

    let (first, rows) = read_scrollback(&host_bytes(&state, Some(1))).expect("a marker");
    assert_eq!(first, carried, "only what scrolled off after the base");
    assert_eq!(rows.len() as u64, state.evicted_total() - carried);
    // From the acknowledged state, everything still unacknowledged.
    let (first, rows) = read_scrollback(&host_bytes(&state, None)).expect("a marker");
    assert_eq!(first, 0);
    assert_eq!(rows.len() as u64, state.evicted_total());
}

#[test]
fn what_a_sent_state_carried_is_not_restated() {
    let mut state = TerminalState::new(5, 20);
    state.process(b"\x1b]0;a title\x07x");
    assert_eq!(state.add_query(b"\x1b]11;?\x07".to_vec()), 1);
    state.snapshot_for_state(1);
    state.process(b"y");
    let frame = state.frame_from(Some(1)).expect("a held base");
    assert!(!String::from_utf8_lossy(&frame.host_bytes).contains("\u{1b}]0;"));
    assert!(frame.queries.is_empty());

    assert_eq!(state.add_query(b"\x1b]10;?\x07".to_vec()), 2);
    let frame = state.frame_from(Some(1)).expect("a held base");
    assert_eq!(
        frame
            .queries
            .iter()
            .map(|query| query.id)
            .collect::<Vec<_>>(),
        [2]
    );
    // The acknowledged state carried none of it.
    assert_eq!(state.frame_from(None).unwrap().queries.len(), 2);
}

#[test]
fn a_base_with_no_snapshot_has_no_frame() {
    let state = TerminalState::new(5, 20);
    assert!(state.frame_from(Some(9)).is_none());
    assert!(!state.has_snapshot(9));
    assert!(state.frame_from(None).is_some());
}
