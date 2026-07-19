use super::*;

#[test]
fn only_plain_control_c_stops_the_http_server() {
    let input =
        |keystroke: &str| TerminalInput::Keystroke(gpui::Keystroke::parse(keystroke).unwrap());

    assert!(http_input_stops_server(&input("ctrl-c")));
    assert!(!http_input_stops_server(&input("c")));
    assert!(!http_input_stops_server(&input("ctrl-shift-c")));
    assert!(http_input_stops_server(&TerminalInput::Text(
        "\u{3}".to_owned()
    )));
    assert!(!http_input_stops_server(&TerminalInput::Paste(
        "\u{3}".to_owned()
    )));
}
