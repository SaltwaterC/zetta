use super::*;

#[test]
fn keymap_round_trip_preserves_parameterized_actions_and_section_metadata() {
    let root = std::env::temp_dir().join(format!(
        "zetta-keymap-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"[{"context":"Zetta","use_key_equivalents":true,"bindings":{"ctrl-!":["zetta::OpenProfile",{"slot":1}]}}]"#,
    )
    .unwrap();
    let form = KeymapForm::load(&root).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();
    assert_eq!(output[0]["use_key_equivalents"], true);
    assert_eq!(output[0]["bindings"]["ctrl-!"][1]["slot"], 1);
}

#[test]
fn binding_form_exposes_string_action_parameters() {
    let binding = BindingForm {
        keystroke: TextField::new("alt-shift-o"),
        action: json!([
            "zetta::ApplyPaneSplitTemplate",
            { "name": "three-right" }
        ]),
    };

    assert_eq!(
        binding.action_parameter("name").as_deref(),
        Some("three-right")
    );
}

#[test]
fn missing_keymap_starts_with_the_structured_template() {
    let path = std::env::temp_dir().join(format!(
        "zetta-missing-keymap-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let form = KeymapForm::load(&path).unwrap();
    assert!(
        form.sections
            .iter()
            .any(|section| !section.bindings.is_empty())
    );
}

#[test]
fn configuration_form_round_trip_uses_typed_values_and_profiles() {
    let root = std::env::temp_dir().join(format!(
        "zetta-configuration-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"{
            "default_profile": "System",
            "terminal_font_size": 13,
            "profiles": [{
                "name": "Login shell",
                "program": "/bin/sh",
                "args": ["-l"],
                "theme": "One Dark"
            }]
        }"#,
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let mut form = ConfigurationForm::load(&root, &config).unwrap();
    form.terminal_font_size.text = "16".to_owned();
    form.max_scroll_history_lines.text = "123456789".to_owned();
    form.inactive_pane_opacity = 0.65;
    form.pane_controls_position = PaneControlsPosition::Left;
    form.http_server_port.text = "8080".to_owned();
    form.profiles
        .iter_mut()
        .find(|profile| !profile.detected)
        .unwrap()
        .arguments
        .text = "-l, -i".to_owned();

    let text = form.to_json().unwrap();
    let output: Value = serde_json::from_str(&text).unwrap();
    Config::parse(&text, Some(&root), None).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["terminal_font_size"], 16.);
    assert_eq!(output["max_scroll_history_lines"], 123_456_789);
    assert_eq!(output["inactive_pane_opacity"], 0.65);
    assert_eq!(output["pane_controls_position"], "left");
    assert_eq!(output["http_server_port"], 8080);
    assert_eq!(output["profiles"][0]["args"], json!(["-l", "-i"]));
}

#[test]
fn max_scrollback_is_displayed_symbolically_but_serialized_numerically() {
    let config = Config::defaults(None, None);
    let missing = std::env::temp_dir().join(format!(
        "zetta-max-scrollback-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let form = ConfigurationForm::load(&missing, &config).unwrap();
    assert_eq!(form.max_scroll_history_lines.text, "Max");
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    assert_eq!(
        output["max_scroll_history_lines"],
        terminal::MAX_SCROLL_HISTORY_LINES as u64
    );
}

#[test]
fn detected_profile_theme_overrides_are_the_only_detected_profiles_serialized() {
    let root = std::env::temp_dir().join(format!(
        "zetta-detected-profile-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"{"profiles":[{"name":"System","theme":"One Dark"}]}"#,
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let mut form = ConfigurationForm::load(&root, &config).unwrap();
    let system_index = form
        .profiles
        .iter()
        .position(|profile| profile.name.text == "System")
        .unwrap();
    assert!(form.profiles[system_index].detected);
    assert_eq!(
        form.profiles[system_index].theme.as_deref(),
        Some("One Dark")
    );

    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    assert_eq!(
        output["profiles"],
        json!([{"name": "System", "theme": "One Dark"}])
    );

    form.profiles[system_index].theme = None;
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();
    assert_eq!(output["profiles"], json!([]));
}

#[test]
fn text_field_edits_unicode_and_replaces_selection() {
    let mut field = TextField::new("héllo");
    field.move_left();
    field.backspace();
    assert_eq!(field.text, "hélo");
    field.select_all();
    field.insert("Zetta");
    assert_eq!(field.text, "Zetta");
}

#[test]
fn save_creates_parent_directories() {
    let root = std::env::temp_dir().join(format!("zetta-settings-save-{}", std::process::id()));
    let path = root.join("nested/config.json");
    save(&path, "{}").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    fs::remove_dir_all(root).unwrap();
}
