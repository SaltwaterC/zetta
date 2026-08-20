use super::*;

#[test]
fn settings_control_navigation_wraps_and_starts_at_the_expected_end() {
    assert_eq!(adjacent_settings_control_index(0, None, false), None);
    assert_eq!(adjacent_settings_control_index(3, Some(0), true), Some(2));
    assert_eq!(adjacent_settings_control_index(3, Some(2), false), Some(0));
    assert_eq!(adjacent_settings_control_index(3, None, false), Some(0));
    assert_eq!(adjacent_settings_control_index(3, None, true), Some(2));
}

#[test]
fn dropdown_snapshot_lists_every_option_when_no_query_is_typed() {
    let options = vec![
        "Alpha".to_owned(),
        "A much longer option label".to_owned(),
        "Beta".to_owned(),
    ];

    let (rows, widest_row) = dropdown_snapshot_rows(&options, "");

    assert_eq!(rows.as_ref(), [0, 1, 2]);
    // The measured row is an index into `rows`, not into `options`.
    assert_eq!(widest_row, Some(1));
}

#[test]
fn dropdown_snapshot_keeps_fuzzy_matches_and_remeasures_them() {
    let options = vec![
        "zetta::NewTab".to_owned(),
        "zetta::CloseTab".to_owned(),
        "zetta::SplitHorizontalDown".to_owned(),
    ];

    let (rows, widest_row) = dropdown_snapshot_rows(&options, "tab");

    assert_eq!(rows.as_ref(), fuzzy_match_indices(&options, "tab"));
    assert_eq!(rows.as_ref(), [0, 1]);
    // Widest among the *matching* rows, so the popover is not sized for a
    // filtered-out option.
    assert_eq!(widest_row, Some(1));
}

#[test]
fn dropdown_snapshot_is_empty_when_nothing_matches() {
    let options = vec!["Alpha".to_owned(), "Beta".to_owned()];

    let (rows, widest_row) = dropdown_snapshot_rows(&options, "gamma");

    assert!(rows.is_empty());
    assert_eq!(widest_row, None);
}

#[test]
fn the_form_scroll_mapping_skips_the_controls_that_live_in_the_dialog_header() {
    let controls = vec![
        SettingsControl::Tab(SettingsPage::Configuration),
        SettingsControl::Tab(SettingsPage::Themes),
        SettingsControl::Tab(SettingsPage::Keymap),
        SettingsControl::Tab(SettingsPage::PaneTemplates),
        SettingsControl::Tab(SettingsPage::Projects),
        SettingsControl::Close,
        SettingsControl::Save,
        SettingsControl::SelectPaneTemplate(0),
        SettingsControl::NewPaneTemplate,
    ];

    // The tab strip, Close, and Save are painted in the fixed header, so the
    // first form control has to map to the top of the scroll range: counting
    // them as form controls scrolls every page too far.
    assert_eq!(leading_header_controls(&controls), 7);
    assert_eq!(leading_header_controls(&controls[..7]), 7);
    assert_eq!(leading_header_controls(&[]), 0);
}

#[test]
fn custom_profiles_can_reach_their_visibility_control() {
    let controls = profile_settings_controls(3, false);

    assert!(
        controls.contains(&SettingsControl::Toggle(SettingsToggle::ProfileVisibility(
            3
        )))
    );
}
