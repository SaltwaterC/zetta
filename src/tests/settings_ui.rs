use super::*;

#[test]
fn profile_creation_requires_a_name_and_program() {
    assert!(!Zetta::profile_draft_has_required_fields("", "bash"));
    assert!(!Zetta::profile_draft_has_required_fields("Profile", ""));
    assert!(!Zetta::profile_draft_has_required_fields("  ", "bash"));
    assert!(!Zetta::profile_draft_has_required_fields("Profile", "  "));
    assert!(Zetta::profile_draft_has_required_fields("Profile", "bash"));
}

#[test]
fn font_filter_uses_pre_normalized_names_and_preserves_indices() {
    let fonts = vec![
        "jetbrains mono".to_owned(),
        "fira code".to_owned(),
        "fira mono".to_owned(),
    ];
    assert_eq!(&*matching_font_indices(&fonts, "FIRA"), &[1, 2]);
    assert_eq!(&*matching_font_indices(&fonts, "code"), &[1]);
}

#[test]
fn font_picker_positions_filtered_rows_by_source_index() {
    let fonts = vec![
        "jetbrains mono".to_owned(),
        "fira code".to_owned(),
        "fira mono".to_owned(),
    ];

    assert_eq!(matching_font_position(&fonts, "FIRA", 1), Some(0));
    assert_eq!(matching_font_position(&fonts, "FIRA", 2), Some(1));
    assert_eq!(matching_font_position(&fonts, "FIRA", 0), None);
}

#[test]
fn dropdown_matching_is_case_insensitive_and_supports_subsequences() {
    let options = vec![
        "Use application theme".to_owned(),
        "One Dark".to_owned(),
        "Solarized Light".to_owned(),
    ];

    assert_eq!(fuzzy_match_index(&options, "ond"), Some(1));
    assert_eq!(fuzzy_match_index(&options, "SL"), Some(2));
    assert_eq!(fuzzy_match_index(&options, "missing"), None);
    assert_eq!(fuzzy_match_indices(&options, "ond"), [1]);
    assert_eq!(
        fuzzy_match_indices(&options, "missing"),
        Vec::<usize>::new()
    );
}

#[test]
fn dropdown_matching_prefers_the_strongest_match() {
    let options = vec![
        "Open profile".to_owned(),
        "Open pane profile".to_owned(),
        "Profile".to_owned(),
    ];

    assert_eq!(fuzzy_match_index(&options, "profile"), Some(2));
}

#[test]
fn settings_options_are_shared_without_copying_the_collection() {
    let options: Arc<[String]> = vec!["One".into(), "Two".into()].into();
    let menu_options = options.clone();
    assert!(Arc::ptr_eq(&options, &menu_options));
    assert_eq!(&*menu_options, &["One", "Two"]);
}

#[test]
fn scroll_history_steps_cover_the_full_range_without_jumping_to_max() {
    let maximum = i32::MAX as u64;
    assert_eq!(adjusted_scroll_history(100_000, 1, maximum), 200_000);
    assert_eq!(adjusted_scroll_history(100_000, -1, maximum), 99_000);
    assert_eq!(
        adjusted_scroll_history(maximum, -1, maximum),
        maximum - 100_000_000
    );
    assert_eq!(adjusted_scroll_history(maximum - 1, 1, maximum), maximum);
}
