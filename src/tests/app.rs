use super::*;

#[test]
fn launch_theme_override_applies_case_insensitively_by_name_only() {
    let mut profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    apply_launch_theme_override(
        &mut profile,
        Some(&("system".to_owned(), "Override Theme".to_owned())),
    );
    assert_eq!(profile.theme, Some("Override Theme".to_owned()));
    assert_eq!(profile.dark_theme, Some("Override Theme".to_owned()));

    let mut other_profile = Profile {
        name: "Other".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    apply_launch_theme_override(
        &mut other_profile,
        Some(&("system".to_owned(), "Override Theme".to_owned())),
    );
    assert_eq!(other_profile.theme, Some("Configured Theme".to_owned()));
    assert_eq!(other_profile.dark_theme, None);

    let mut unaffected_profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Zetta,
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
fn exited_terminal_is_not_backgrounded_by_the_tab_pin() {
    let pinned = TabClosePolicy::Background {
        authentication: None,
    };

    assert!(background_authentication_for_close(&pinned, false, true, false).is_some());
    assert!(background_authentication_for_close(&pinned, false, false, false).is_none());
    assert!(background_authentication_for_close(&pinned, false, true, true).is_none());
}

#[test]
fn a_shared_tab_is_backgrounded_when_it_closes() {
    assert!(matches!(
        background_authentication_for_close(&TabClosePolicy::Close, true, true, false),
        Some(None)
    ));
    assert!(
        background_authentication_for_close(&TabClosePolicy::Close, false, true, false).is_none()
    );
    assert!(
        background_authentication_for_close(&TabClosePolicy::Close, true, true, true).is_none()
    );
}

#[test]
fn new_tab_inherits_the_active_profile_after_an_explicit_profile_tab_closes() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
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
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
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
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
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

#[test]
fn opening_a_project_starts_in_the_project_directory_regardless_of_scope() {
    for scope in [
        WorkingDirectoryScope::None,
        WorkingDirectoryScope::Pane,
        WorkingDirectoryScope::Tab,
    ] {
        assert!(
            !NewTabOrigin::ProjectEntry.inherits_working_directory(scope),
            "entering a project must not inherit the session directory with scope {scope:?}"
        );
    }
}

#[test]
fn a_new_tab_in_the_current_session_follows_the_configured_scope() {
    assert!(!NewTabOrigin::CurrentSession.inherits_working_directory(WorkingDirectoryScope::None));
    assert!(NewTabOrigin::CurrentSession.inherits_working_directory(WorkingDirectoryScope::Tab));
}

#[test]
fn a_transient_notice_is_taken_away_by_its_own_timer_only() {
    // The banner it replaces stayed on screen until something else overwrote
    // it, which made a sentence of advice read as an unresolved error with no
    // way to clear it.
    let mut notice = TransientNotice::default();
    let first = notice.show("attach it from another window".to_owned());
    assert_eq!(notice.message(), Some("attach it from another window"));

    // A later notice takes over, and the earlier one's timer must not then
    // remove it: the generation is what keeps the two apart.
    let second = notice.show("this tab can now be joined".to_owned());
    assert!(!notice.dismiss_if_current(first));
    assert_eq!(notice.message(), Some("this tab can now be joined"));

    assert!(notice.dismiss_if_current(second));
    assert_eq!(notice.message(), None);
    // Dismissing twice is not an error, and does not report a second change.
    assert!(!notice.dismiss_if_current(second));
}
