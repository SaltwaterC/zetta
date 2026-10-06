use super::*;

#[test]
fn clipboard_writes_accept_both_terminators_and_default_targets() {
    for sequence in [
        &b"\x1b]52;c;dGV4dA==\x07"[..],
        b"\x1b]52;pc;dGV4dA==\x1b\\",
        b"\x1b]52;;dGV4dA==\x07",
        b"\x1b]52;c;\x07",
    ] {
        assert!(is_write(sequence), "{sequence:?}");
    }
}

#[test]
fn reads_and_malformed_sequences_are_not_clipboard_writes() {
    for sequence in [
        &b"\x1b]52;c;?\x07"[..],
        b"\x1b]52;;?\x1b\\",
        b"\x1b]52;c;dGV4dA==",
        b"\x1b]52;c\x07",
        b"\x1b]52;invalid;dGV4dA==\x07",
        b"\x1b]52;c;dGV4\x18dA==\x07",
        b"\x1b]52;c;dGV4;dA==\x07",
        b"\x1b]10;?\x07",
    ] {
        assert!(!is_write(sequence), "{sequence:?}");
    }
}

#[test]
fn clipboard_write_size_is_bounded() {
    let mut sequence = b"\x1b]52;c;".to_vec();
    sequence.resize(MAX_SEQUENCE_BYTES - 1, b'A');
    sequence.push(0x07);
    assert!(is_write(&sequence));
    sequence.insert(sequence.len() - 1, b'A');
    assert!(!is_write(&sequence));
}
