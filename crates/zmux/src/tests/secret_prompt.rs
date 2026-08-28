use super::*;

#[test]
fn a_typed_secret_has_to_be_confirmed() {
    let error = confirmed(Zeroizing::new("hunter2".to_owned()), "hunter3")
        .expect_err("a mistyped secret must be refused")
        .to_string();
    assert!(error.contains("do not match"), "{error}");

    let secret = confirmed(Zeroizing::new("hunter2".to_owned()), "hunter2")
        .expect("a confirmed secret is the session's");
    assert_eq!(secret.expose(), "hunter2");
}

#[test]
fn a_typed_secret_keeps_whatever_is_not_the_line_ending() {
    for (typed, expected) in [
        ("hunter2\n", "hunter2"),
        ("hunter2\r\n", "hunter2"),
        // Only the ending: a secret may legitimately end in a space.
        ("hunter2 \n", "hunter2 "),
        ("", ""),
    ] {
        let mut line = typed.to_owned();
        strip_line_ending(&mut line);
        assert_eq!(line, expected, "{typed:?}");
    }
}

#[cfg(windows)]
fn key_down(unicode_char: u16, virtual_key_code: u16) -> ConsoleKeyEvent {
    ConsoleKeyEvent {
        key_down: true,
        repeat_count: 1,
        virtual_key_code,
        unicode_char,
        control_key_state: 0,
    }
}

#[cfg(windows)]
#[test]
fn windows_empty_enter_completes_immediately() {
    let mut line = SecretLine::new();

    assert_eq!(
        line.handle(key_down(0, VK_RETURN)),
        SecretInputAction::Complete
    );
    assert_eq!(line.finish().as_str(), "");
}

#[cfg(windows)]
#[test]
fn windows_secret_reducer_masks_repeats_and_deletes_unicode_characters() {
    let mut line = SecretLine::new();

    assert_eq!(
        line.handle(key_down('a' as u16, b'A' as u16)),
        SecretInputAction::Echo(1)
    );
    assert_eq!(
        line.handle(key_down('b' as u16, b'B' as u16)),
        SecretInputAction::Echo(1)
    );

    let repeated = ConsoleKeyEvent {
        repeat_count: 3,
        ..key_down('x' as u16, b'X' as u16)
    };
    assert_eq!(line.handle(repeated), SecretInputAction::Echo(3));

    let surrogate_pair: Vec<u16> = "😀".encode_utf16().collect();
    assert_eq!(surrogate_pair.len(), 2);
    assert_eq!(
        line.handle(key_down(surrogate_pair[0], 0)),
        SecretInputAction::Ignore
    );
    assert_eq!(
        line.handle(key_down(surrogate_pair[1], 0)),
        SecretInputAction::Echo(1)
    );

    let repeated_backspace = ConsoleKeyEvent {
        repeat_count: 2,
        ..key_down(0, VK_BACK)
    };
    assert_eq!(line.handle(repeated_backspace), SecretInputAction::Erase(2));
    assert_eq!(line.finish().as_str(), "abxx");
}

#[cfg(windows)]
#[test]
fn windows_secret_reducer_ignores_key_up_and_modifier_events() {
    let mut line = SecretLine::new();

    let key_up = ConsoleKeyEvent {
        key_down: false,
        repeat_count: 4,
        ..key_down('x' as u16, b'X' as u16)
    };
    assert_eq!(line.handle(key_up), SecretInputAction::Ignore);

    let modifier = key_down(0, 0x10); // VK_SHIFT
    assert_eq!(line.handle(modifier), SecretInputAction::Ignore);
    assert_eq!(line.finish().as_str(), "");
}

#[cfg(windows)]
#[test]
fn windows_ctrl_c_cancels_without_producing_a_secret() {
    let mut line = SecretLine::new();
    assert_eq!(
        line.handle(key_down('x' as u16, b'X' as u16)),
        SecretInputAction::Echo(1)
    );

    let ctrl_c = ConsoleKeyEvent {
        virtual_key_code: VK_C,
        unicode_char: CTRL_C,
        control_key_state: 0x0008, // LEFT_CTRL_PRESSED
        ..key_down(CTRL_C, VK_C)
    };
    assert_eq!(line.handle(ctrl_c), SecretInputAction::Cancel);
}

#[cfg(windows)]
#[test]
fn windows_secret_mode_preserves_unrelated_flags() {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
    };

    let unrelated = CONSOLE_MODE(0x8000);
    let original = unrelated | ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT;
    assert_eq!(secret_console_mode(original), unrelated);
}

#[cfg(unix)]
#[test]
fn unix_masked_reader_echoes_unicode_and_backspace() {
    use std::io::Cursor;

    let mut input = "ab😀".as_bytes().to_vec();
    input.push(0x7f); // DEL, the usual Unix erase character.
    input.extend_from_slice(b"x\n");
    let mut output = Vec::new();

    let secret = read_masked_secret(&mut Cursor::new(input), &mut output)
        .expect("a complete Unix input stream should be read");
    assert_eq!(secret.as_str(), "abx");
    assert_eq!(output, b"***\x08 \x08*");
}

#[cfg(unix)]
#[test]
fn unix_masked_reader_completes_empty_enter_and_cancels_on_ctrl_c() {
    use std::io::Cursor;

    let mut empty_output = Vec::new();
    let empty = read_masked_secret(&mut Cursor::new(b"\n"), &mut empty_output)
        .expect("empty Enter should complete");
    assert!(empty.is_empty());
    assert!(empty_output.is_empty());

    let mut cancelled_output = Vec::new();
    let error = read_masked_secret(&mut Cursor::new(b"abc\x03"), &mut cancelled_output)
        .expect_err("Ctrl-C should cancel the prompt");
    assert!(error.to_string().contains("secret prompt cancelled"));
    assert_eq!(cancelled_output, b"***");
}
