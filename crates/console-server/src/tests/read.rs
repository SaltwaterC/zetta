use super::*;
use crate::keys::key;

fn w(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn reads_stop_at_character_boundaries_in_any_encoding() {
    let text = w("λ🙂\r\n");
    assert_eq!(
        encode_prefix(&text, 3, false, codepage::UTF8),
        ("λ".as_bytes().to_vec(), 1)
    );
    assert_eq!(
        encode_prefix(&text[1..], 4, false, codepage::UTF8),
        ("🙂".as_bytes().to_vec(), 2)
    );
    assert_eq!(
        encode_prefix(&text[3..], 4, true, codepage::UTF8),
        (b"\r\0\n\0".to_vec(), 2)
    );
    assert_eq!(encode_prefix(&w("éa"), 1, false, 437), (vec![0x82], 1));
    // Characters the code page lacks take its best fit, as WideCharToMultiByte.
    assert_eq!(encode_prefix(&w("あ"), 4, false, 437), (b"?".to_vec(), 1));
    // Shift-JIS "あ" needs two bytes; one byte of room returns nothing.
    assert_eq!(encode_prefix(&w("あ"), 1, false, 932), (Vec::new(), 0));
    assert_eq!(
        encode_prefix(&w("あ"), 2, false, 932),
        (vec![0x82, 0xa0], 1)
    );
}

#[test]
fn get_char_skips_releases_escape_and_linefeed_outside_vt_mode() {
    let a = key(true, 1, 0x41, 30, u16::from(b'a'), 0);
    assert!(matches!(
        get_char(&a, Keys::Plain, false),
        Some(Key::Char(97, 0))
    ));
    let release = key(false, 1, 0x41, 30, u16::from(b'a'), 0);
    assert!(get_char(&release, Keys::Plain, false).is_none());
    let escape = key(true, 1, 0x1b, 1, 0x1b, 0);
    assert!(get_char(&escape, Keys::Plain, false).is_none());
    assert!(matches!(
        get_char(&escape, Keys::Plain, true),
        Some(Key::Char(0x1b, _))
    ));
    let linefeed = key(true, 1, 0x0d, 28, 10, 0x8);
    assert!(get_char(&linefeed, Keys::Plain, false).is_none());
}

#[test]
fn editing_and_popup_keys_are_keys_only_where_the_editor_wants_them() {
    let up = key(true, 1, 0x26, 72, 0, 0x100);
    assert!(get_char(&up, Keys::Plain, false).is_none());
    assert!(matches!(
        get_char(&up, Keys::Editing, false),
        Some(Key::Vkey(0x26, _))
    ));
    // F3 edits the line but means nothing to an open popup.
    let f3 = key(true, 1, 0x72, 61, 0, 0);
    assert!(matches!(
        get_char(&f3, Keys::Editing, false),
        Some(Key::Vkey(0x72, _))
    ));
    assert!(get_char(&f3, Keys::Popup, false).is_none());
    // Ctrl+Left is an editing key; Ctrl+Up is not.
    let ctrl_left = key(true, 1, 0x25, 75, 0, 0x108);
    assert!(matches!(
        get_char(&ctrl_left, Keys::Editing, false),
        Some(Key::Vkey(0x25, _))
    ));
    let ctrl_up = key(true, 1, 0x26, 72, 0, 0x108);
    assert!(get_char(&ctrl_up, Keys::Editing, false).is_none());
    // Escape is the editor's key, never its character.
    let escape = key(true, 1, 0x1b, 1, 0x1b, 0);
    assert!(matches!(
        get_char(&escape, Keys::Editing, false),
        Some(Key::Vkey(0x1b, _))
    ));
}

#[test]
fn multi_line_alias_text_is_returned_one_line_per_read() {
    let mut pending = Pending {
        text: w("echo one\r\necho two\r\n"),
        multiline: true,
    };
    assert_eq!(
        take_pending(&mut pending, 100, false, codepage::UTF8),
        b"echo one\r\n"
    );
    assert_eq!(
        take_pending(&mut pending, 100, false, codepage::UTF8),
        b"echo two\r\n"
    );
    assert!(pending.text.is_empty());
    let mut long = Pending {
        text: w("abcdef"),
        multiline: false,
    };
    assert_eq!(take_pending(&mut long, 4, false, codepage::UTF8), b"abcd");
    assert_eq!(take_pending(&mut long, 4, false, codepage::UTF8), b"ef");
}
