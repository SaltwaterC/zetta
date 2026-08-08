use super::*;

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
