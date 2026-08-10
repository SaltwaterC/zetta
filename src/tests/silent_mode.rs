use super::*;

#[test]
fn global_and_tab_silence_are_combined() {
    assert!(!combined_silent_mode(false, false));
    assert!(combined_silent_mode(true, false));
    assert!(combined_silent_mode(false, true));
    assert!(combined_silent_mode(true, true));
}

#[test]
fn tab_silent_mode_toggles_locally() {
    let mut enabled = false;
    assert!(toggle_tab_silent_mode_value(&mut enabled));
    assert!(enabled);
    assert!(!toggle_tab_silent_mode_value(&mut enabled));
    assert!(!enabled);
}

#[test]
fn silent_mode_uses_manual_or_system_state() {
    let mut state = SilentModeState::default();
    assert!(!state.effective());

    assert!(state.toggle_manual());
    assert!(state.effective());

    assert!(state.observe_system(SystemSilentState::Inactive));
    assert!(state.effective());
    assert!(state.observe_system(SystemSilentState::Active));
    assert!(state.effective());
}

#[test]
fn system_silence_locks_manual_toggling() {
    let mut state = SilentModeState::default();
    assert!(state.observe_system(SystemSilentState::Active));
    assert!(!state.toggle_manual());
    assert!(state.effective());
}

#[test]
fn manual_preference_returns_after_system_silence_clears() {
    let mut state = SilentModeState::default();
    assert!(state.toggle_manual());
    assert!(state.observe_system(SystemSilentState::Active));
    assert!(state.observe_system(SystemSilentState::Inactive));
    assert!(state.effective());
}

#[test]
fn unknown_detector_results_retain_the_last_known_state() {
    let mut state = SilentModeState::default();
    assert!(!state.observe_system(SystemSilentState::Unknown));
    assert!(state.observe_system(SystemSilentState::Active));
    assert!(!state.observe_system(SystemSilentState::Unknown));
    assert!(state.effective());
    assert!(state.observe_system(SystemSilentState::Inactive));
    assert!(!state.effective());
}

#[test]
fn windows_notification_states_ignore_fullscreen_only() {
    assert_eq!(windows_notification_state(3), SystemSilentState::Inactive);
    assert_eq!(windows_notification_state(4), SystemSilentState::Active);
    assert_eq!(windows_notification_state(6), SystemSilentState::Active);
    assert_eq!(windows_notification_state(99), SystemSilentState::Unknown);
}

#[test]
fn gnome_banner_values_map_to_silence_states() {
    assert_eq!(
        parse_gnome_show_banners("true\n"),
        Some(SystemSilentState::Inactive)
    );
    assert_eq!(
        parse_gnome_show_banners("false\n"),
        Some(SystemSilentState::Active)
    );
    assert_eq!(parse_gnome_show_banners("nothing"), None);
}

#[test]
fn macos_focus_status_requires_authorization_and_a_known_focus_value() {
    assert_eq!(macos_focus_status(3, Some(true)), SystemSilentState::Active);
    assert_eq!(
        macos_focus_status(3, Some(false)),
        SystemSilentState::Inactive
    );
    assert_eq!(
        macos_focus_status(2, Some(true)),
        SystemSilentState::Unknown
    );
    assert_eq!(macos_focus_status(3, None), SystemSilentState::Unknown);
}
