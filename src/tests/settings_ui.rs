use super::*;

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
fn settings_control_navigation_wraps_and_starts_at_the_expected_end() {
    assert_eq!(adjacent_settings_control_index(0, None, false), None);
    assert_eq!(adjacent_settings_control_index(3, Some(0), true), Some(2));
    assert_eq!(adjacent_settings_control_index(3, Some(2), false), Some(0));
    assert_eq!(adjacent_settings_control_index(3, None, false), Some(0));
    assert_eq!(adjacent_settings_control_index(3, None, true), Some(2));
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

#[test]
fn keymap_capture_keeps_modified_escape_and_return_available() {
    assert!(is_unmodified_capture_control(
        "escape",
        &gpui::Modifiers::none()
    ));
    assert!(is_unmodified_capture_control(
        "enter",
        &gpui::Modifiers::none()
    ));
    assert!(!is_unmodified_capture_control(
        "escape",
        &gpui::Modifiers::shift()
    ));
    assert!(!is_unmodified_capture_control(
        "enter",
        &gpui::Modifiers::control()
    ));
}

#[test]
fn keymap_capture_ignores_modifier_only_events() {
    assert!(is_modifier_key("shift"));
    assert!(is_modifier_key("control"));
    assert!(!is_modifier_key("escape"));
    assert!(!is_modifier_key("f12"));
}

#[test]
fn captured_keymap_shortcuts_use_keymap_text_syntax() {
    let keystroke = gpui::Keystroke::parse("shift-escape").unwrap();
    assert_eq!(keystroke.unparse(), "shift-escape");
}

fn binding(keystroke: &str, action: &str) -> BindingForm {
    BindingForm {
        keystroke: TextField::new(keystroke),
        action: serde_json::Value::String(action.to_owned()),
    }
}

fn section(context: &str, bindings: Vec<BindingForm>) -> KeymapSectionForm {
    let mut section = KeymapSectionForm::new(context);
    section.bindings = bindings;
    section
}

#[test]
fn keymap_search_matches_empty_query_keeps_every_section_and_binding() {
    let sections = vec![
        section("Zetta > Terminal", vec![binding("ctrl-t", "zetta::NewTab")]),
        section("Zetta > Pane", vec![]),
    ];

    let (matched_sections, matched_bindings) = keymap_search_matches(&sections, "");

    assert_eq!(matched_sections, vec![0, 1]);
    assert_eq!(matched_bindings.get(&0), Some(&vec![0]));
    assert_eq!(matched_bindings.get(&1), Some(&Vec::new()));
}

#[test]
fn keymap_search_matches_binding_by_keystroke_or_action_name() {
    let sections = vec![section(
        "Zetta > Terminal",
        vec![
            binding("ctrl-t", "zetta::NewTab"),
            binding("ctrl-w", "zetta::CloseTab"),
        ],
    )];

    let (matched_sections, matched_bindings) = keymap_search_matches(&sections, "closetab");
    assert_eq!(matched_sections, vec![0]);
    assert_eq!(matched_bindings.get(&0), Some(&vec![1]));

    let (matched_sections, matched_bindings) = keymap_search_matches(&sections, "ctrl-t");
    assert_eq!(matched_sections, vec![0]);
    assert_eq!(matched_bindings.get(&0), Some(&vec![0]));
}

#[test]
fn keymap_search_matching_context_surfaces_every_binding_in_it() {
    let sections = vec![
        section("Zetta > Terminal", vec![binding("ctrl-t", "zetta::NewTab")]),
        section("Zetta > Pane", vec![binding("ctrl-w", "zetta::CloseTab")]),
    ];

    let (matched_sections, matched_bindings) = keymap_search_matches(&sections, "terminal");

    assert_eq!(matched_sections, vec![0]);
    assert_eq!(matched_bindings.get(&0), Some(&vec![0]));
    assert_eq!(matched_bindings.get(&1), None);
}

#[test]
fn keymap_search_drops_sections_with_no_matching_bindings() {
    let sections = vec![section(
        "Zetta > Terminal",
        vec![binding("ctrl-t", "zetta::NewTab")],
    )];

    let (matched_sections, matched_bindings) = keymap_search_matches(&sections, "nonexistent");

    assert!(matched_sections.is_empty());
    assert!(matched_bindings.is_empty());
}

#[test]
fn captured_shifted_number_row_uses_gpui_keymap_normalization() {
    let keystroke = gpui::Keystroke::parse("ctrl-!").unwrap();
    let keyboard_mapper = gpui::DummyKeyboardMapper;
    let captured = keybinding_for_capture(&keystroke, &keyboard_mapper);
    assert_eq!(captured.unparse(), "ctrl-!");
    assert_eq!(
        keymap_keystroke_display(&captured.unparse()),
        "Ctrl+Shift+1"
    );

    assert_eq!(keymap_keystroke_display("ctrl-)"), "Ctrl+Shift+0");
    assert_eq!(keymap_keystroke_display("ctrl-shift-0"), "Ctrl+Shift+0");
    assert_eq!(
        keymap_keystroke_alias("ctrl-shift-0"),
        Some("Ctrl+Shift+0".to_owned())
    );
    assert_eq!(keymap_keystroke_display("ctrl-shift-10"), "ctrl-shift-10");
}
