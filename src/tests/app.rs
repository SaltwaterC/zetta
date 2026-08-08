use super::*;

#[test]
fn launch_theme_override_applies_case_insensitively_by_name_only() {
    let mut profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
    };
    apply_launch_theme_override(
        &mut profile,
        Some(&("system".to_owned(), "Override Theme".to_owned())),
    );
    assert_eq!(profile.theme, Some("Override Theme".to_owned()));

    let mut other_profile = Profile {
        name: "Other".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
    };
    apply_launch_theme_override(
        &mut other_profile,
        Some(&("system".to_owned(), "Override Theme".to_owned())),
    );
    assert_eq!(other_profile.theme, Some("Configured Theme".to_owned()));

    let mut unaffected_profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
    };
    apply_launch_theme_override(&mut unaffected_profile, None);
    assert_eq!(
        unaffected_profile.theme,
        Some("Configured Theme".to_owned())
    );
}

#[test]
fn mouse_window_resize_clamps_each_dimension_to_the_minimum() {
    assert_eq!(
        clamp_window_size_to_minimum(size(px(400.), px(500.))),
        size(px(520.), px(500.))
    );
    assert_eq!(
        clamp_window_size_to_minimum(size(px(600.), px(200.))),
        size(px(600.), px(320.))
    );
}

#[test]
fn pane_resize_mode_pauses_terminal_input() {
    assert!(pane_input_enabled(false));
    assert!(!pane_input_enabled(true));
}

#[test]
fn application_menu_navigation_wraps_in_both_directions() {
    assert_eq!(
        adjacent_application_menu_index(2, 0, ApplicationMenuDirection::Left),
        1
    );
    assert_eq!(
        adjacent_application_menu_index(2, 1, ApplicationMenuDirection::Right),
        0
    );
    assert_eq!(
        adjacent_application_menu_index(3, 1, ApplicationMenuDirection::Left),
        0
    );
    assert_eq!(
        adjacent_application_menu_index(3, 1, ApplicationMenuDirection::Right),
        2
    );
}

#[test]
fn exited_terminal_is_not_backgrounded_by_the_tab_pin() {
    let pinned = TabClosePolicy::Background {
        authentication: None,
    };

    assert!(background_authentication_for_close(&pinned, true).is_some());
    assert!(background_authentication_for_close(&pinned, false).is_none());
}

#[test]
fn new_tab_inherits_the_active_profile_after_an_explicit_profile_tab_closes() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
    };

    let profile = new_tab_profile(
        Some(&system),
        &[system.clone(), alternate],
        0,
        NewTabProfile::Inherit,
    )
    .unwrap();

    assert_eq!(profile.name, "System");
}

#[test]
fn first_tab_uses_the_configured_default_profile() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
    };

    let profile = new_tab_profile(None, &[system, alternate], 1, NewTabProfile::Default).unwrap();

    assert_eq!(profile.name, "Alternate");
}

#[test]
fn default_new_tabs_ignore_the_active_profile() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
    };

    let profile = new_tab_profile(
        Some(&alternate),
        &[system, alternate.clone()],
        0,
        NewTabProfile::Default,
    )
    .unwrap();

    assert_eq!(profile.name, "System");
}
