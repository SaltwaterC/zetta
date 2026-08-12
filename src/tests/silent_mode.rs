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
fn unknown_detector_results_fall_back_to_manual_silence() {
    let mut state = SilentModeState::default();
    assert!(!state.observe_system(SystemSilentState::Unknown));
    assert!(state.observe_system(SystemSilentState::Active));
    assert!(state.observe_system(SystemSilentState::Unknown));
    assert!(!state.effective());
    assert!(state.observe_system(SystemSilentState::Inactive));
    assert!(!state.effective());
}

#[test]
fn focus_status_access_requires_explicit_authorization_and_explains_fallbacks() {
    assert_eq!(FocusStatusAccess::default(), FocusStatusAccess::Unknown);
    assert!(
        FocusStatusAccess::Denied
            .tooltip()
            .contains("System Settings")
    );
    assert!(
        FocusStatusAccess::Restricted
            .tooltip()
            .contains("manual Silent Mode")
    );
    assert!(
        FocusStatusAccess::AuthorizedButUnavailable
            .tooltip()
            .contains("Communication Notifications")
    );
}

#[test]
fn focus_status_access_clears_system_silence_when_authorization_is_lost() {
    let mut state = SilentModeState::default();
    assert!(state.observe_focus_status_access(FocusStatusAccess::Authorized));
    assert!(state.observe_system(SystemSilentState::Active));
    assert!(state.effective());

    assert!(state.observe_focus_status_access(FocusStatusAccess::Denied));
    assert!(!state.system_active());
    assert!(!state.effective());

    assert!(state.observe_focus_status_access(FocusStatusAccess::Authorized));
    assert!(state.observe_system(SystemSilentState::Active));
    assert!(state.observe_focus_status_access(FocusStatusAccess::AuthorizedButUnavailable));
    assert!(!state.system_active());
    assert!(!state.effective());
}

// Real HKCU\...\CloudStore\...\$$windows.data.notifications.quiethourssettings
// `Data` blobs captured from Windows 10/11 machines in each Focus Assist
// state, so the parser is exercised against actual Windows-produced bytes
// rather than a guessed layout.
const QUIET_HOURS_PRIORITY_ONLY_HEX: &str = "02,00,00,00,15,7c,93,f1,9a,7c,d8,01,00,00,00,00,43,42,01,00,c2,0a,01,d2,14,28,4d,00,69,00,63,00,72,00,6f,00,73,00,6f,00,66,00,74,00,2e,00,51,00,75,00,69,00,65,00,74,00,48,00,6f,00,75,00,72,00,73,00,50,00,72,00,6f,00,66,00,69,00,6c,00,65,00,2e,00,50,00,72,00,69,00,6f,00,72,00,69,00,74,00,79,00,4f,00,6e,00,6c,00,79,00,ca,28,d0,14,02,00,00";
const QUIET_HOURS_UNRESTRICTED_HEX: &str = "02,00,00,00,B4,67,2B,68,F0,0B,D8,01,00,00,00,00,43,42,01,00,C2,0A,01,D2,14,28,4D,00,69,00,63,00,72,00,6F,00,73,00,6F,00,66,00,74,00,2E,00,51,00,75,00,69,00,65,00,74,00,48,00,6F,00,75,00,72,00,73,00,50,00,72,00,6F,00,66,00,69,00,6C,00,65,00,2E,00,55,00,6E,00,72,00,65,00,73,00,74,00,72,00,69,00,63,00,74,00,65,00,64,00,CA,28,D0,14,02,00,00";
const QUIET_HOURS_ALARMS_ONLY_HEX: &str = "020000002bc05e5d177dd4010000000043420100c20a01d214264d006900630072006f0073006f00660074002e005100750069006500740048006f00750072007300500072006f00660069006c0065002e0041006c00610072006d0073004f006e006c00790000";

fn hex_bytes(spec: &str) -> Vec<u8> {
    let cleaned = spec
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>();
    cleaned
        .as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn quiet_hours_profile_maps_real_registry_blobs() {
    assert_eq!(
        parse_quiet_hours_profile(&hex_bytes(QUIET_HOURS_UNRESTRICTED_HEX)),
        SystemSilentState::Inactive
    );
    assert_eq!(
        parse_quiet_hours_profile(&hex_bytes(QUIET_HOURS_PRIORITY_ONLY_HEX)),
        SystemSilentState::Active
    );
    assert_eq!(
        parse_quiet_hours_profile(&hex_bytes(QUIET_HOURS_ALARMS_ONLY_HEX)),
        SystemSilentState::Active
    );
    assert_eq!(parse_quiet_hours_profile(&[]), SystemSilentState::Unknown);
    assert_eq!(
        parse_quiet_hours_profile(b"garbage"),
        SystemSilentState::Unknown
    );
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

/// The observer only re-reads the setting when dconf says it changed, so this
/// filter decides whether a wake happens at all. Both of dconf's shapes have to
/// match, and unrelated writes must not wake it.
#[test]
fn dconf_notifications_match_only_the_show_banners_key() {
    const KEY: &str = "/org/gnome/desktop/notifications/show-banners";

    // Single-key write: the whole path arrives as the prefix with one empty
    // change. This is what GNOME's Do Not Disturb switch produces.
    assert!(dconf_notify_affects(KEY, &["".to_owned()], KEY));

    // Directory write: keys arrive relative to the prefix.
    assert!(dconf_notify_affects(
        "/org/gnome/desktop/notifications/",
        &["show-banners".to_owned(), "show-in-lock-screen".to_owned()],
        KEY
    ));

    // A reset of an enclosing directory also covers the key.
    assert!(dconf_notify_affects(
        "/org/gnome/desktop/",
        &["notifications/show-banners".to_owned()],
        KEY
    ));
    assert!(dconf_notify_affects("/org/gnome/desktop/", &[], KEY));

    // Unrelated keys must not wake the observer.
    assert!(!dconf_notify_affects(
        "/org/gnome/desktop/interface/gtk-theme",
        &["".to_owned()],
        KEY
    ));
    assert!(!dconf_notify_affects(
        "/org/gnome/desktop/notifications/",
        &["show-in-lock-screen".to_owned()],
        KEY
    ));
    assert!(!dconf_notify_affects(
        "/org/gnome/shell/",
        &["favorite-apps".to_owned()],
        KEY
    ));
}

#[test]
fn macos_focus_status_requires_authorization_and_a_known_focus_value() {
    assert_eq!(
        macos_focus_status(FocusStatusAccess::Authorized, Some(true)),
        SystemSilentState::Active
    );
    assert_eq!(
        macos_focus_status(FocusStatusAccess::Authorized, Some(false)),
        SystemSilentState::Inactive
    );
    assert_eq!(
        macos_focus_status(FocusStatusAccess::Denied, Some(true)),
        SystemSilentState::Unknown
    );
    assert_eq!(
        macos_focus_status(FocusStatusAccess::Authorized, None),
        SystemSilentState::Unknown
    );
}
