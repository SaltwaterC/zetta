use super::*;

#[test]
fn only_plain_control_c_interrupts_the_byte_stream() {
    let input =
        |keystroke: &str| TerminalInput::Keystroke(gpui::Keystroke::parse(keystroke).unwrap());

    assert!(ctrl_c_interrupts_byte_stream(&input("ctrl-c")));
    assert!(!ctrl_c_interrupts_byte_stream(&input("c")));
    assert!(!ctrl_c_interrupts_byte_stream(&input("ctrl-shift-c")));
    assert!(ctrl_c_interrupts_byte_stream(&TerminalInput::Text(
        "\u{3}".to_owned()
    )));
    assert!(!ctrl_c_interrupts_byte_stream(&TerminalInput::Paste(
        "\u{3}".to_owned()
    )));
}
