//! Encoding a key event as the bytes a terminal program expects, where keys
//! arrive as events rather than as bytes from a TTY.

use super::*;

pub(crate) fn key_bytes(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    // Windows supplies decoded console events, so reconstruct the sequence
    // using the remote terminal's DECCKM state. Unix forwards these bytes
    // directly from the local terminal, whose mode follows the screen diff.
    let cursor_key = match key.code {
        KeyCode::Up => Some(b'A'),
        KeyCode::Down => Some(b'B'),
        KeyCode::Right => Some(b'C'),
        KeyCode::Left => Some(b'D'),
        KeyCode::Home => Some(b'H'),
        KeyCode::End => Some(b'F'),
        _ => None,
    };
    if let Some(final_byte) = cursor_key {
        return vec![
            0x1b,
            if application_cursor { b'O' } else { b'[' },
            final_byte,
        ];
    }
    match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            control_byte(character).into_iter().collect()
        }
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::ALT) => {
            let mut bytes = vec![0x1b];
            bytes.extend(character.to_string().as_bytes());
            bytes
        }
        KeyCode::Char(character) => character.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Null => vec![0],
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(number) if number <= 12 => function_key_bytes(number),
        _ => Vec::new(),
    }
}

pub(super) fn function_key_bytes(number: u8) -> Vec<u8> {
    match number {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => Vec::new(),
    }
}

pub(crate) fn control_byte(character: char) -> Option<u8> {
    let character = character.to_ascii_uppercase();
    if character.is_ascii() && character.is_ascii_control() {
        return Some(character as u8);
    }
    if character.is_ascii_alphabetic() {
        return Some((character as u8) & 0x1f);
    }
    match character {
        ' ' => Some(0),
        '@' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}
