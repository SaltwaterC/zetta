use super::*;

#[test]
fn generation_survives_every_possible_chunk_boundary() {
    for terminator in ["\x07", "\x1b\\"] {
        let marker = format!("\x1b]777;zosh-clear-scrollback;18446744073709551615{terminator}");
        for split in 0..=marker.len() {
            let mut state = ScrollbackState::default();
            state.feed(&marker.as_bytes()[..split]);
            state.feed(&marker.as_bytes()[split..]);
            assert_eq!(state.generation, u64::MAX);
        }
    }
}

#[test]
fn only_a_complete_valid_marker_changes_the_generation() {
    for bytes in [
        &b"\x0c\x1b[H\x1b[2J\x1b[3J"[..],
        b"\x1b]0;777;zosh-clear-scrollback;1\x07",
        b"\x1b]777;zosh-clear-scrollback;-1\x07",
        b"\x1b]777;zosh-clear-scrollback;\x07",
        b"\x1b]777;zosh-clear-scrollback;18446744073709551616\x07",
        b"\x1b]777;zosh-clear-scrollback;1\x18\x07",
        b"\x1bP\x07\x1b]777;zosh-clear-scrollback;1\x07\x1b\\",
    ] {
        let mut state = ScrollbackState::default();
        state.feed(bytes);
        assert_eq!(state.generation, 0, "{bytes:?}");
        state.feed(b"\x1b]777;zosh-clear-scrollback;2\x07");
        assert_eq!(state.generation, 2);
    }
}

/// The framing, byte for byte, as `PROTOCOL.md` documents it. The writing
/// half carries the same vector in `zosh-server`'s `terminal_state.rs`.
#[test]
fn the_marker_is_read_exactly_as_the_protocol_writes_it() {
    let mut state = ScrollbackState::default();
    state.feed(b"\x1b]777;zosh-scrollback;7;AQAAAAJoaQ\x07");

    assert_eq!(state.rows_first, 7);
    assert_eq!(state.evicted, 8);
    assert_eq!(
        state.rows.as_slice(),
        [ScrollbackRow {
            contents: b"hi".to_vec(),
            wrapped: true,
        }]
    );
}

/// A count that moved with no rows attached is the server saying the history
/// it would have sent is gone. The screen still has to be placed.
#[test]
fn a_marker_with_no_rows_still_moves_the_count() {
    let mut state = ScrollbackState::default();
    state.feed(b"\x1b]777;zosh-scrollback;12;\x07");

    assert_eq!(state.evicted, 12);
    assert_eq!(state.rows_first, 12);
    assert!(state.rows.is_empty());
}

#[test]
fn rows_survive_every_possible_chunk_boundary() {
    let marker = b"\x1b]777;zosh-scrollback;3;AAAAAANvbmUBAAAAA3R3bw\x07";
    for split in 0..=marker.len() {
        let mut state = ScrollbackState::default();
        state.feed(&marker[..split]);
        state.feed(&marker[split..]);
        assert_eq!(state.rows_first, 3, "split at {split}");
        assert_eq!(state.evicted, 5, "split at {split}");
        assert_eq!(
            state
                .rows
                .iter()
                .map(|row| row.contents.clone())
                .collect::<Vec<_>>(),
            vec![b"one".to_vec(), b"two".to_vec()],
            "split at {split}"
        );
        assert_eq!(
            state.rows.iter().map(|row| row.wrapped).collect::<Vec<_>>(),
            vec![false, true],
            "split at {split}"
        );
    }
}

/// A payload longer than the fixed marker buffer is the ordinary case: the
/// buffer only ever holds the header, and the Base64 runs past it.
#[test]
fn a_payload_longer_than_the_marker_buffer_is_read_whole() {
    let rows: Vec<Vec<u8>> = (0..200)
        .map(|n| format!("row number {n}").into_bytes())
        .collect();
    let mut payload = Vec::new();
    for row in &rows {
        payload.push(0);
        payload.extend_from_slice(&(row.len() as u32).to_be_bytes());
        payload.extend_from_slice(row);
    }
    let encoded = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(&payload)
    };
    let mut marker = b"\x1b]777;zosh-scrollback;0;".to_vec();
    marker.extend_from_slice(encoded.as_bytes());
    marker.push(0x07);

    let mut state = ScrollbackState::default();
    // Fed in chunks, the way a PTY delivers anything this long.
    for chunk in marker.chunks(7) {
        state.feed(chunk);
    }
    assert_eq!(state.evicted, 200);
    assert_eq!(
        state
            .rows
            .iter()
            .map(|row| row.contents.clone())
            .collect::<Vec<_>>(),
        rows
    );
}

#[test]
fn only_a_complete_valid_rows_marker_is_accepted() {
    for bytes in [
        // Not at the start of the OSC.
        &b"\x1b]0;777;zosh-scrollback;1;AA\x07"[..],
        // No index.
        b"\x1b]777;zosh-scrollback;;AA\x07",
        b"\x1b]777;zosh-scrollback;-1;AA\x07",
        // No separator after the index.
        b"\x1b]777;zosh-scrollback;1\x07",
        // Base64 that is not.
        b"\x1b]777;zosh-scrollback;1;!!!!\x07",
        // A row header promising more bytes than the payload holds.
        b"\x1b]777;zosh-scrollback;1;AH////8A\x07",
        // Cancelled before it was terminated.
        b"\x1b]777;zosh-scrollback;1;AQAAAAJoaQ\x18\x07",
    ] {
        let mut state = ScrollbackState::default();
        state.feed(bytes);
        assert_eq!(state.evicted, 0, "{bytes:?}");
        assert!(state.rows.is_empty(), "{bytes:?}");
    }
}

/// The two markers share a prefix, so neither may be read as the other.
#[test]
fn the_two_markers_do_not_shadow_each_other() {
    let mut state = ScrollbackState::default();
    state.feed(b"\x1b]777;zosh-clear-scrollback;4\x07");
    state.feed(b"\x1b]777;zosh-scrollback;9;AQAAAAJoaQ\x07");

    assert_eq!(state.generation, 4);
    assert_eq!(state.evicted, 10);

    // And a clear arriving afterwards leaves the rows where they were.
    state.feed(b"\x1b]777;zosh-clear-scrollback;5\x07");
    assert_eq!(state.generation, 5);
    assert_eq!(state.evicted, 10);
}

/// `rows` framed and Base64-encoded into a rows marker, as `zosh-server`
/// writes one.
fn rows_marker(first: u64, rows: &[&[u8]]) -> Vec<u8> {
    use base64::Engine as _;
    let mut payload = Vec::new();
    for row in rows {
        payload.push(0);
        payload.extend_from_slice(&u32::try_from(row.len()).unwrap().to_be_bytes());
        payload.extend_from_slice(row);
    }
    let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&payload);
    format!("\x1b]777;zosh-scrollback;{first};{encoded}\x07").into_bytes()
}

/// The rows are written to the terminal raw, so a row is the one place a
/// server could slip in a query whose answer the terminal would type back
/// into the remote session. Every one of these must arrive as its text only.
#[test]
fn a_carried_row_keeps_its_text_and_loses_every_query_and_control() {
    let hostile: &[(&[u8], &[u8])] = &[
        (b"a\x1b]52;c;?\x07b", b"ab"),
        (b"a\x1b]52;c;?\x1b\\b", b"ab"),
        (b"a\xc2\x9d52;c;?\xc2\x9cb", b"ab"),
        (b"a\x1b[6nb", b"ab"),
        (b"a\x1b[cb", b"ab"),
        (b"a\x1b[>0cb", b"ab"),
        (b"a\x1b[?1049hb", b"ab"),
        (b"a\x1b[21tb", b"ab"),
        (b"a\x1b[2;1Hb", b"ab"),
        (b"a\xc2\x9b6nb", b"ab"),
        (b"a\x1bP$qm\x1b\\b", b"ab"),
        (b"a\x1bP+q544e\x07still-in-dcs\x1b\\b", b"ab"),
        (b"a\x1b_apc\x1b\\b", b"ab"),
        (b"a\x1bZb", b"ab"),
        (b"a\x1b#8b", b"ab"),
        (b"a\x05\x07\x08\r\n\tb\x7f", b"ab"),
        // A raw 8-bit CSI is not UTF-8: the byte goes, its tail is inert text.
        (b"a\x9b6nb", b"a6nb"),
        (b"a\x1b[1;31\x07mb", b"amb"),
        (b"a\x1b]52;c;?", b"a"),
    ];
    for (row, expected) in hostile {
        let mut state = ScrollbackState::default();
        state.feed(&rows_marker(0, &[row]));
        assert_eq!(
            String::from_utf8_lossy(&state.rows[0].contents),
            String::from_utf8_lossy(expected),
            "row {:?}",
            String::from_utf8_lossy(row)
        );
    }
}

/// What a row legitimately contains — text in any script, colours, attribute
/// resets, gaps and erased runs — passes through byte for byte. A row the
/// filter changed would also never compare equal to the same row on screen.
#[test]
fn every_row_vt100_formats_survives_the_filter_unchanged() {
    let mut parser = vt100::Parser::new(4, 40, 0);
    parser.screen_mut().set_capture_evicted_rows(true);
    parser.process(
        "\x1b[1;31mbold red\x1b[0m plain \x1b[38;5;208mindexed\x1b[48;2;1;2;3mtrue\x1b[m\r\n\
         wide 漢字 and é\x1b[10Cgap\x1b[7m   \x1b[m\r\n\
         \x1b[4;3mitalic underline\x1b[2m dim\x1b[22m \x1b[44m\x1b[5X\x1b[5Cerased\x1b[m\r\n"
            .as_bytes(),
    );
    parser.process(b"\r\n\r\n\r\n\r\n");
    let (_, rows) = parser.screen_mut().take_evicted_rows();
    assert!(rows.len() >= 3);
    let all: Vec<u8> = rows.iter().flat_map(|row| row.contents.clone()).collect();
    for kept in [&b"\x1b[10C"[..], b"\x1b[5X", b"\x1b[38;5;208m"] {
        assert!(
            all.windows(kept.len()).any(|window| window == kept),
            "the fixture no longer exercises {:?}: {:?}",
            String::from_utf8_lossy(kept),
            String::from_utf8_lossy(&all)
        );
    }
    for row in rows {
        assert_eq!(
            sanitize_row(&row.contents),
            row.contents,
            "{:?}",
            String::from_utf8_lossy(&row.contents)
        );
    }
}
