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
