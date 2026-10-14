use super::*;

fn put(message: &mut [u8], length_offset: usize, offset: usize, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    message[length_offset..length_offset + 2]
        .copy_from_slice(&((wide.len() * 2) as u16).to_le_bytes());
    for (index, unit) in wide.iter().enumerate() {
        message[offset + index * 2..offset + index * 2 + 2].copy_from_slice(&unit.to_le_bytes());
    }
}

#[test]
fn title_application_and_group_come_from_their_documented_offsets() {
    let mut message = vec![0u8; SIZE];
    message[28..32].copy_from_slice(&4242u32.to_le_bytes());
    message[32] = 1;
    put(&mut message, 34, 36, r"C:\Python314\python.exe");
    put(&mut message, 558, 560, "python.exe");
    let connect = parse(&message);
    assert_eq!(connect.process_group, 4242);
    assert!(connect.console_app);
    assert_eq!(connect.title, r"C:\Python314\python.exe");
    assert_eq!(connect.application, "python.exe");
}

#[test]
fn short_or_lying_messages_never_read_past_their_fields() {
    let connect = parse(&[0u8; 36]);
    assert_eq!(connect, Connect::default());
    let mut message = vec![0u8; SIZE];
    message[34..36].copy_from_slice(&u16::MAX.to_le_bytes());
    message[558..560].copy_from_slice(&u16::MAX.to_le_bytes());
    let connect = parse(&message);
    assert!(connect.title.is_empty() && connect.application.is_empty());
}
