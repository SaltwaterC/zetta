use super::*;

fn records(events: Vec<Event>) -> Vec<Record> {
    events
        .into_iter()
        .flat_map(|event| match event {
            Event::Records(records) => records,
            Event::Key(record) => vec![record],
            Event::Raw(text) => panic!("unexpected raw text {text:?}"),
        })
        .collect()
}
fn field(record: &Record, offset: usize) -> u16 {
    u16::from_le_bytes([record[offset], record[offset + 1]])
}
fn state(record: &Record) -> u32 {
    u32::from_le_bytes(record[16..20].try_into().unwrap())
}
/// Key-down records as (vk, char, state).
fn presses(records: &[Record]) -> Vec<(u16, u16, u32)> {
    records
        .iter()
        .filter(|record| field(record, 0) == KEY_EVENT && record[4] != 0)
        .map(|record| (field(record, 10), field(record, 14), state(record)))
        .collect()
}

#[test]
fn win32_keys_pass_through_and_mark_cursor_reports() {
    let mut decoder = Decoder::default();
    let events = decoder.decode(b"\x1b[65;30;97;1;0;1_", false);
    assert_eq!(events, [Event::Key(key(true, 1, 65, 30, 97, 0))]);
    // After a win32 key, CSI R is a cursor position report for the app.
    assert_eq!(
        decoder.decode(b"\x1b[12;5R", false),
        [Event::Raw("\x1b[12;5R".into())]
    );
}

#[test]
fn vt_cursor_and_function_keys_become_enhanced_records() {
    let mut decoder = Decoder::default();
    let up = records(decoder.decode(b"\x1b[A", false));
    assert_eq!(presses(&up), [(VK_UP, 0, ENHANCED_KEY)]);
    // Ctrl+Right is modifier 5, wrapped in Ctrl presses.
    let right = records(decoder.decode(b"\x1b[1;5C", false));
    assert_eq!(
        presses(&right),
        [
            (VK_CONTROL, 0, LEFT_CTRL),
            (VK_RIGHT, 0, LEFT_CTRL | ENHANCED_KEY)
        ]
    );
    assert_eq!(right.len(), 4, "Ctrl down, key down, key up, Ctrl up");
    let delete = records(decoder.decode(b"\x1b[3~", false));
    assert_eq!(presses(&delete)[0].0, VK_DELETE);
    let f5 = records(decoder.decode(b"\x1b[15~", false));
    assert_eq!(presses(&f5)[0].0, VK_F1 + 4);
    let f1 = records(decoder.decode(b"\x1bOP", false));
    assert_eq!(presses(&f1)[0].0, VK_F1);
    // Bracketed paste markers are consumed without producing keys.
    assert!(records(decoder.decode(b"\x1b[200~\x1b[201~", false)).is_empty());
}

#[test]
fn controls_become_their_keys_and_etx_is_a_ctrl_c_key() {
    let mut decoder = Decoder::default();
    let events = decoder.decode(b"\x03", false);
    assert_eq!(
        events,
        [
            Event::Key(key(true, 1, u16::from(b'C'), scan_code(67), 3, LEFT_CTRL)),
            Event::Key(key(false, 1, u16::from(b'C'), scan_code(67), 3, LEFT_CTRL)),
        ]
    );
    let enter = records(decoder.decode(b"\r", false));
    assert_eq!(presses(&enter), [(0x0d, 0x0d, 0)]);
    let escape = records(decoder.decode(b"\x1b", false));
    assert_eq!(presses(&escape), [(VK_ESCAPE, 0x1b, 0)]);
    let delete = records(decoder.decode(b"\x7f", false));
    assert_eq!(presses(&delete), [(VK_BACK, 0x08, 0)]);
}

#[test]
fn vt_input_mode_passes_terminal_sequences_through_as_text() {
    let mut decoder = Decoder::default();
    assert_eq!(
        decoder.decode(b"\x1b[A", true),
        [Event::Raw("\x1b[A".into())]
    );
    assert_eq!(decoder.decode(b"ab", true), [Event::Raw("ab".into())]);
    assert_eq!(
        decoder.decode(b"\x1b[<0;3;4M", true),
        [Event::Raw("\x1b[<0;3;4M".into())]
    );
    // Focus reports are not the application's to see unless it asked.
    assert!(decoder.decode(b"\x1b[I", true).is_empty());
    // Ctrl+C keeps its control-key path even in VT input mode.
    assert!(matches!(decoder.decode(b"\x03", true)[0], Event::Key(_)));
}

#[test]
fn sgr_mouse_reports_track_buttons_wheel_and_motion() {
    let mut decoder = Decoder::default();
    let press = records(decoder.decode(b"\x1b[<0;10;5M", false));
    assert_eq!(field(&press[0], 0), MOUSE_EVENT);
    assert_eq!((field(&press[0], 4), field(&press[0], 6)), (9, 4));
    assert_eq!(u32::from_le_bytes(press[0][8..12].try_into().unwrap()), 1);
    let drag = records(decoder.decode(b"\x1b[<32;11;5M", false));
    assert_eq!(u32::from_le_bytes(drag[0][8..12].try_into().unwrap()), 1);
    assert_eq!(state(&drag[0]), MOUSE_MOVED);
    let release = records(decoder.decode(b"\x1b[<0;11;5m", false));
    assert_eq!(u32::from_le_bytes(release[0][8..12].try_into().unwrap()), 0);
    let wheel = records(decoder.decode(b"\x1b[<65;11;5M", false));
    assert_eq!(
        u32::from_le_bytes(wheel[0][8..12].try_into().unwrap()),
        SCROLL_DELTA_BACKWARD
    );
    assert_eq!(state(&wheel[0]), MOUSE_WHEELED);
    let ctrl_click = records(decoder.decode(b"\x1b[<16;1;1M", false));
    assert_eq!(
        u32::from_le_bytes(ctrl_click[0][12..16].try_into().unwrap()),
        LEFT_CTRL
    );
}

#[test]
fn focus_reports_become_focus_records() {
    let mut decoder = Decoder::default();
    assert_eq!(
        decoder.decode(b"\x1b[I\x1b[O", false),
        [
            Event::Records(vec![focus(true)]),
            Event::Records(vec![focus(false)])
        ]
    );
}

#[test]
fn sequences_split_across_writes_are_held_until_complete() {
    let mut decoder = Decoder::default();
    assert!(decoder.decode(b"\x1b[65;30;97;1", false).is_empty());
    assert_eq!(
        decoder.decode(b";0;1_", false),
        [Event::Key(key(true, 1, 65, 30, 97, 0))]
    );
    let smile = "🙂".as_bytes();
    assert!(decoder.decode(&smile[..2], true).is_empty());
    assert_eq!(decoder.decode(&smile[2..], true), [Event::Raw("🙂".into())]);
    // OSC replies pass through whole, whichever terminator they use.
    assert!(
        decoder
            .decode(b"\x1b]11;rgb:0000/0000/0000", true)
            .is_empty()
    );
    assert_eq!(
        decoder.decode(b"\x1b\\", true),
        [Event::Raw("\x1b]11;rgb:0000/0000/0000\x1b\\".into())]
    );
}

#[test]
fn win32_keys_arriving_in_vt_input_mode_are_encoded_as_vt() {
    assert_eq!(
        vt_text(&key(true, 1, VK_UP, 72, 0, 0)).as_deref(),
        Some("\x1b[A")
    );
    assert_eq!(
        vt_text(&key(true, 1, VK_UP, 72, 0, LEFT_CTRL)).as_deref(),
        Some("\x1b[1;5A")
    );
    assert_eq!(vt_text(&key(true, 2, 65, 30, 97, 0)).as_deref(), Some("aa"));
    assert_eq!(
        vt_text(&key(true, 1, 65, 30, 97, LEFT_ALT)).as_deref(),
        Some("\x1ba")
    );
    // AltGr (Ctrl+RightAlt) characters are typed, not Alt-prefixed.
    assert_eq!(
        vt_text(&key(
            true,
            1,
            0x51,
            16,
            u16::from(b'@'),
            LEFT_CTRL | RIGHT_ALT
        ))
        .as_deref(),
        Some("@")
    );
    assert_eq!(vt_text(&key(false, 1, 65, 30, 97, 0)), None);
    assert_eq!(
        vt_text(&key(true, 1, VK_F1 + 4, 63, 0, 0)).as_deref(),
        Some("\x1b[15~")
    );
}
