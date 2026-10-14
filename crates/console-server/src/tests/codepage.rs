use super::*;

#[test]
fn oem_code_pages_have_glyphs_for_controls() {
    assert_eq!(glyph(437, 0x1b), Some('←'));
    assert_eq!(glyph(437, 0x0d), Some('♪'));
    // UTF-8 has no glyphs: Windows hands the control back, exactly as it
    // does to OpenConsole making the same call.
    assert_eq!(glyph(UTF8, 0x1b), Some('\x1b'));
}

#[test]
fn conversions_round_trip_through_an_oem_code_page() {
    assert_eq!(
        to_utf16(437, &[0x82, b'a']),
        "éa".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(
        from_utf16(437, &"éa".encode_utf16().collect::<Vec<_>>()),
        [0x82, b'a']
    );
    assert_eq!(
        from_utf16(UTF8, &"é".encode_utf16().collect::<Vec<_>>()),
        "é".as_bytes()
    );
}

#[test]
fn ascii_shortcut_is_refused_for_code_pages_that_remap_ascii() {
    assert!(ascii_compatible(437));
    assert!(ascii_compatible(UTF8));
    // IBM EBCDIC US-Canada.
    assert!(!ascii_compatible(37));
}

#[test]
fn a_character_split_across_writes_is_completed_by_the_next_one() {
    let mut decoder = Decoder::default();
    // Shift-JIS "あ" is 0x82 0xa0.
    assert!(decoder.decode(932, &[b'x', 0x82]) == "x".encode_utf16().collect::<Vec<_>>());
    assert_eq!(
        decoder.decode(932, &[0xa0]),
        "あ".encode_utf16().collect::<Vec<_>>()
    );
    let smile = "🙂".as_bytes();
    assert!(decoder.decode(UTF8, &smile[..2]).is_empty());
    assert_eq!(
        decoder.decode(UTF8, &smile[2..]),
        "🙂".encode_utf16().collect::<Vec<_>>()
    );
    // Invalid bytes are not held back waiting for a completion.
    assert_eq!(
        decoder.decode(UTF8, &[0xff, b'a']),
        "\u{fffd}a".encode_utf16().collect::<Vec<_>>()
    );
}
