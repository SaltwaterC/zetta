use super::*;
use crate::output::OutputMode;
use crate::sink::Sink;
fn packet(api: u32, body: &[u8]) -> Packet {
    let mut packet = Packet::default();
    packet.descriptor.input_size = (8 + body.len()) as u32;
    packet.descriptor.output_size = body.len() as u32;
    set_u32(&mut packet.payload, 0, api);
    set_u32(&mut packet.payload, 4, body.len() as u32);
    packet.payload[8..8 + body.len()].copy_from_slice(body);
    packet
}
fn fixture() -> (Server, Handle) {
    let sink = Sink::new(false, None, 500).unwrap();
    let event = unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(
            std::ptr::null(),
            1,
            0,
            std::ptr::null(),
        )
    };
    (Server::new(sink, false), Handle::new(event).unwrap())
}
#[test]
fn input_event_query_rejects_output_handles_as_python_requires() {
    let (mut server, handle) = fixture();
    let mut packet = packet(0x01000003, &[0; 4]);
    packet.descriptor.object = server.object(Object::Output(0));
    assert_eq!(server.api(&handle, &packet, 12).unwrap().0, INVALID_HANDLE);
    server.reply.clear();
    packet.descriptor.object = server.object(Object::Input);
    assert_eq!(server.api(&handle, &packet, 12).unwrap().0, 0);
}
#[test]
fn malformed_api_sizes_never_index_outside_the_packet() {
    let (mut server, handle) = fixture();
    for (body_size, input_size, output_size, available) in [
        (4096, 4104, 4096, 4104),
        (92, 100, 92, 99),
        (92, 99, 92, 100),
        (92, 100, 91, 100),
        (91, 99, 91, 99),
        (u32::MAX, u32::MAX, u32::MAX, 4104),
    ] {
        let mut packet = packet(0x02000007, &[]);
        set_u32(&mut packet.payload, 4, body_size);
        packet.descriptor.input_size = input_size;
        packet.descriptor.output_size = output_size;
        assert_eq!(
            server.api(&handle, &packet, available).unwrap().0,
            INVALID_PARAMETER
        );
    }
}
#[test]
fn screen_info_includes_geometry_cursor_and_the_whole_color_table() {
    let (mut server, handle) = fixture();
    server
        .sink
        .write(b"abc", OutputMode::from_console_flags(7))
        .unwrap();
    let mut packet = packet(0x02000007, &[0; 92]);
    assert_eq!(server.api(&handle, &packet, 100).unwrap().0, INVALID_HANDLE);
    server.reply.clear();
    packet.descriptor.object = server.object(Object::Output(0));
    assert_eq!(server.api(&handle, &packet, 100).unwrap().0, 0);
    assert_eq!(server.reply.len(), 92);
    assert_eq!(&server.reply[..8], &[80, 0, 24, 0, 3, 0, 0, 0]);
    // Campbell's bright white, as a ConPTY console reports by default.
    assert_eq!(u32_at(&server.reply, 88), 0xf2f2f2);
}
#[test]
fn unsupported_api_returns_an_error_and_is_counted_once() {
    let (mut server, handle) = fixture();
    // GetConsoleFontInfo, deprecated in OpenConsole's table too.
    let packet = packet(0x03000002, &[0; 12]);
    for _ in 0..2 {
        assert_eq!(server.api(&handle, &packet, 20).unwrap().0, NOT_SUPPORTED);
    }
    assert_eq!(server.unsupported.len(), 1);
}
