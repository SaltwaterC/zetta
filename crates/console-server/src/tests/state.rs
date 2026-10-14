use super::*;

fn state() -> ConsoleState {
    ConsoleState::new(437, Palette::default())
}

#[test]
fn defaults_report_what_a_conpty_console_reports() {
    let state = state();
    assert_eq!(state.input_mode(), 0x1f7);
    assert_eq!(state.output_mode, 0x7);
    assert!(!state.wants_mouse(), "quick edit owns the mouse by default");
}

#[test]
fn invalid_input_modes_are_stored_and_still_rejected() {
    let mut state = state();
    // ECHO without LINE: OpenConsole keeps the mode but returns E_INVALIDARG.
    assert!(!state.set_input_mode(0x4));
    assert_eq!(state.input_mode(), 0x4);
    assert!(!state.set_input_mode(0x400));
    assert!(state.set_input_mode(0x2f7));
    assert_eq!(state.input_mode(), 0x2f7);
    assert!(state.set_input_mode(0x0));
    assert_eq!(state.input_mode(), 0x0, "private flags stop being reported");
}

#[test]
fn quick_edit_and_mouse_input_decide_who_owns_the_mouse() {
    let mut state = state();
    assert!(state.set_input_mode(mode::MOUSE_INPUT | mode::EXTENDED_FLAGS));
    assert!(state.wants_mouse());
    assert!(state.set_input_mode(mode::MOUSE_INPUT | mode::EXTENDED_FLAGS | mode::QUICK_EDIT_MODE));
    assert!(!state.wants_mouse());
}

#[test]
fn every_documented_output_mode_is_accepted_and_nothing_else() {
    let mut state = state();
    for value in [0x0, 0x1, 0x3, 0x5, 0x7, 0xf, 0x1f] {
        assert!(state.set_output_mode(value).is_some(), "{value:#x}");
    }
    assert_eq!(state.set_output_mode(0x20), None);
    assert_eq!(state.output_mode, 0x1f);
}

#[test]
fn buffer_size_must_cover_the_window_and_lapses_on_resize() {
    let mut state = state();
    assert!(!state.set_buffer_size((120, 30), (120, 29)));
    assert!(!state.set_buffer_size((120, 30), (120, i16::MAX)));
    assert!(state.set_buffer_size((120, 30), (120, 130)));
    assert_eq!(state.buffer_size((120, 30)), (120, 130));
    assert_eq!(state.buffer_size((100, 30)), (100, 30));
}

#[test]
fn only_the_first_client_names_the_console() {
    let mut state = state();
    state.connected(r"C:\Windows\System32\cmd.exe");
    state.connected(r"C:\Windows\System32\where.exe");
    assert_eq!(state.title, r"C:\Windows\System32\cmd.exe");
    assert_eq!(state.original_title, state.title);
}
