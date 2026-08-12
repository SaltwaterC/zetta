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
fn system_silence_locks_manual_toggling_until_it_clears() {
    let mut state = SilentModeState::default();
    assert!(state.observe_system(SystemSilentState::Active));
    assert!(!state.toggle_manual());
    assert!(state.effective());

    assert!(state.observe_system(SystemSilentState::Inactive));
    assert!(state.toggle_manual());
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

#[test]
fn windows_dnd_query_classifies_live_profiles() {
    assert_eq!(
        classify_windows_dnd_query(Some((0, 4, 0))),
        SystemSilentState::Inactive
    );
    assert_eq!(
        classify_windows_dnd_query(Some((0, 4, 1))),
        SystemSilentState::Active
    );
    assert_eq!(
        classify_windows_dnd_query(Some((0, 4, 2))),
        SystemSilentState::Active
    );
}

#[test]
fn windows_dnd_query_fails_open_for_unreliable_results() {
    assert_eq!(classify_windows_dnd_query(None), SystemSilentState::Unknown);
    assert_eq!(
        classify_windows_dnd_query(Some((-1, 4, 1))),
        SystemSilentState::Unknown
    );
    assert_eq!(
        classify_windows_dnd_query(Some((0, 0, 1))),
        SystemSilentState::Unknown
    );
    assert_eq!(
        classify_windows_dnd_query(Some((0, 8, 1))),
        SystemSilentState::Unknown
    );
    assert_eq!(
        classify_windows_dnd_query(Some((0, 4, 3))),
        SystemSilentState::Unknown
    );
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
