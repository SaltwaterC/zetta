use super::*;

#[test]
fn only_plain_control_c_stops_the_tftp_server() {
    let input =
        |keystroke: &str| TerminalInput::Keystroke(gpui::Keystroke::parse(keystroke).unwrap());

    assert!(tftp_input_stops_server(&input("ctrl-c")));
    assert!(!tftp_input_stops_server(&input("c")));
    assert!(!tftp_input_stops_server(&input("ctrl-shift-c")));
    assert!(tftp_input_stops_server(&TerminalInput::Text(
        "\u{3}".to_owned()
    )));
    assert!(!tftp_input_stops_server(&TerminalInput::Paste(
        "\u{3}".to_owned()
    )));
}
