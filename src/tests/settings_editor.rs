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
    let mut form = KeymapForm::load(&root).unwrap();
    let section_index = form
        .sections
        .iter()
        .position(|section| section.context.text == "Zetta")
        .unwrap();
    assert_eq!(
        form.sections[section_index].bindings[0].keystroke.text,
        "Ctrl+Shift+1"
    );
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let output_section = output
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["context"] == "Zetta")
        .unwrap();
    assert_eq!(output_section["use_key_equivalents"], true);
    assert_eq!(output_section["bindings"]["Ctrl+Shift+1"][1]["slot"], 1);

    form.sections[section_index].bindings[0].keystroke.text = "Ctrl+Shift+3".to_owned();
    let alias_output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let alias_section = alias_output
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["context"] == "Zetta")
        .unwrap();
    form.sections[section_index].bindings[0].keystroke.text = "Ctrl+Shift+0".to_owned();
    let tenth_alias_output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let tenth_section = tenth_alias_output
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["context"] == "Zetta")
        .unwrap();
    fs::remove_file(root).unwrap();
    assert_eq!(alias_section["bindings"]["Ctrl+Shift+3"][1]["slot"], 1);
    assert_eq!(tenth_section["bindings"]["Ctrl+Shift+0"][1]["slot"], 1);
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
fn binding_form_exposes_numeric_action_parameters() {
    let binding = BindingForm {
        keystroke: TextField::new("ctrl-!"),
        action: json!(["zetta::OpenProfile", { "slot": 1 }]),
    };

    assert_eq!(binding.action_usize_parameter("slot"), Some(1));
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
    form.default_tab_icon = Some(IconName::Folder);
    form.max_scroll_history_lines.text = "123456789".to_owned();
    form.inactive_pane_opacity = 0.65;
    form.compact_mode = true;
    form.hide_pane_size = false;
    form.hide_title_bar_labels = true;
    form.hide_title_bar_buttons = true;
    #[cfg(target_os = "macos")]
    {
        form.hide_title_bar_menus = false;
    }
    form.pane_controls_position = PaneControlsPosition::Left;
    form.pane_controls_hidden_by_default = true;
    form.working_directory_scope = WorkingDirectoryScope::Pane;
    form.new_tab_profile = NewTabProfile::Inherit;
    #[cfg(feature = "http-server")]
    {
        form.http_server_port.text = "8080".to_owned();
    }
    #[cfg(feature = "tftp-server")]
    {
        form.tftp_server_port.text = "1069".to_owned();
    }
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
    assert_eq!(output["default_tab_icon"], "folder");
    assert_eq!(output["max_scroll_history_lines"], 123_456_789);
    assert_eq!(output["inactive_pane_opacity"], 0.65);
    assert_eq!(output["compact_mode"], true);
    assert_eq!(output["hide_pane_size"], false);
    assert_eq!(output["hide_title_bar_labels"], true);
    assert_eq!(output["hide_title_bar_buttons"], true);
    #[cfg(target_os = "macos")]
    assert_eq!(output["hide_title_bar_menus"], false);
    assert_eq!(output["pane_controls_position"], "left");
    assert_eq!(output["pane_controls_hidden_by_default"], true);
    assert_eq!(output["working_directory_scope"], "pane");
    assert_eq!(output["new_tab_profile"], "inherit");
    #[cfg(feature = "http-server")]
    assert_eq!(output["http_server_port"], 8080);
    #[cfg(feature = "tftp-server")]
    assert_eq!(output["tftp_server_port"], 1069);
    assert_eq!(output["profiles"][0]["args"], json!(["-l", "-i"]));
}

#[test]
fn configuration_form_round_trip_preserves_hidden_detected_profiles() {
    let root = std::env::temp_dir().join(format!(
        "zetta-hidden-profile-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&root, r#"{"profiles":[{"name":"System","hidden":true}]}"#).unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(
        output["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|profile| profile["name"] == "System")
            .and_then(|profile| profile.get("hidden")),
        Some(&json!(true))
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_configuration_preserves_macos_title_bar_setting() {
    let root = std::env::temp_dir().join(format!(
        "zetta-macos-title-bar-setting-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&root, r#"{"hide_title_bar_menus":true}"#).unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["hide_title_bar_menus"], true);
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
    // "Max" is the built-in default, so it's omitted from the saved file rather
    // than being pinned as an explicit override.
    assert!(
        !output
            .as_object()
            .unwrap()
            .contains_key("max_scroll_history_lines")
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
fn configuration_defaults_round_trip_produces_minimal_output() {
    let config = Config::defaults(None, None);
    let missing = std::env::temp_dir().join(format!(
        "zetta-default-config-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let form = ConfigurationForm::load(&missing, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let object = output.as_object().unwrap();
    for key in object.keys() {
        // `terminal_font_size` has no fixed default (it falls back to a
        // theme-dependent size at runtime), so it's intentionally always
        // written rather than filtered — see the design notes in to_json.
        assert!(
            matches!(key.as_str(), "profiles" | "terminal_font_size"),
            "unexpected default-valued key {key:?}"
        );
    }
    if let Some(profiles) = object.get("profiles") {
        assert_eq!(profiles, &json!([]));
    }
}

#[test]
fn keymap_defaults_round_trip_produces_empty_array() {
    let missing = std::env::temp_dir().join(format!(
        "zetta-default-keymap-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let form = KeymapForm::load(&missing).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    assert_eq!(output, json!([]));
}

#[test]
fn keymap_single_rebind_is_preserved_others_dropped() {
    let missing = std::env::temp_dir().join(format!(
        "zetta-keymap-rebind-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut form = KeymapForm::load(&missing).unwrap();
    let section = form
        .sections
        .iter_mut()
        .find(|section| section.context.text == "Zetta > Terminal")
        .unwrap();
    let binding = section
        .bindings
        .iter_mut()
        .find(|binding| binding.keystroke.text == "ctrl-shift-t")
        .unwrap();
    binding.keystroke.text = "ctrl-shift-x".to_owned();

    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let sections = output.as_array().unwrap();
    assert_eq!(sections.len(), 1);
    let bindings = sections[0]["bindings"].as_object().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings["ctrl-shift-x"], "zetta::NewTab");
}

#[test]
fn keymap_template_matches_hardcoded_default_constant() {
    let template: Vec<Value> = serde_json::from_str(include_str!("../../keymap.example.json"))
        .expect("parsing bundled keymap template");
    let terminal = template
        .iter()
        .find(|section| section["context"] == "Zetta > Terminal")
        .expect("bundled template must define the Zetta > Terminal context");
    assert_eq!(
        terminal["bindings"][crate::startup::RENAME_TAB_KEYBINDING],
        "zetta::RenameTab"
    );
}

#[test]
fn save_creates_parent_directories() {
    let root = std::env::temp_dir().join(format!("zetta-settings-save-{}", std::process::id()));
    let path = root.join("nested/config.json");
    save(&path, "{}").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_keymap_file_loads_default_template() {
    let root = std::env::temp_dir().join(format!(
        "zetta-empty-keymap-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&root, "[]").unwrap();
    let form = KeymapForm::load(&root).unwrap();
    // Should have sections from default template
    assert!(!form.sections.is_empty());
    let terminal_section = form
        .sections
        .iter()
        .find(|s| s.context.text == "Zetta > Terminal")
        .expect("should have Zetta > Terminal section");
    // Should have default bindings
    assert!(!terminal_section.bindings.is_empty());
    fs::remove_file(root).unwrap();
}

#[test]
fn keymap_user_customization_overrides_default() {
    let root = std::env::temp_dir().join(format!(
        "zetta-keymap-custom-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // User overrides ctrl-shift-t to do something else
    fs::write(
        &root,
        r#"[{"context":"Zetta > Terminal","bindings":{"ctrl-shift-t":"zetta::CloseTab"}}]"#,
    )
    .unwrap();
    let form = KeymapForm::load(&root).unwrap();
    let terminal_section = form
        .sections
        .iter()
        .find(|s| s.context.text == "Zetta > Terminal")
        .expect("should have Zetta > Terminal section");
    // Find the customized binding (stored in lowercase format)
    let binding = terminal_section
        .bindings
        .iter()
        .find(|b| b.keystroke.text == "ctrl-shift-t")
        .expect("should have ctrl-shift-t binding");
    // Should have user's action, not default
    assert_eq!(binding.action, json!("zetta::CloseTab"));
    // Other default bindings should still exist
    assert!(terminal_section.bindings.len() > 1);
    fs::remove_file(root).unwrap();
}

#[test]
fn keymap_new_user_section_is_preserved() {
    let root = std::env::temp_dir().join(format!(
        "zetta-keymap-new-section-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // User adds a completely new section
    fs::write(
        &root,
        r#"[{"context":"Custom Context","bindings":{"ctrl-x":"custom::Action"}}]"#,
    )
    .unwrap();
    let form = KeymapForm::load(&root).unwrap();
    // Should have default sections plus the new one
    let custom_section = form
        .sections
        .iter()
        .find(|s| s.context.text == "Custom Context")
        .expect("should have Custom Context section");
    assert!(!custom_section.bindings.is_empty());
    let binding = &custom_section.bindings[0];
    assert_eq!(binding.keystroke.text, "ctrl-x");
    assert_eq!(binding.action, json!("custom::Action"));
    // Default sections should still exist
    assert!(
        form.sections
            .iter()
            .any(|s| s.context.text == "Zetta > Terminal")
    );
    fs::remove_file(root).unwrap();
}

#[test]
fn keymap_rebinding_action_removes_old_default_binding() {
    let root = std::env::temp_dir().join(format!(
        "zetta-keymap-rebind-action-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // User rebinds NewTab from ctrl-shift-t to ctrl-?
    fs::write(
        &root,
        r#"[{"context":"Zetta > Terminal","bindings":{"ctrl-?":"zetta::NewTab"}}]"#,
    )
    .unwrap();
    let form = KeymapForm::load(&root).unwrap();
    let terminal_section = form
        .sections
        .iter()
        .find(|s| s.context.text == "Zetta > Terminal")
        .expect("should have Zetta > Terminal section");
    // Should have the new binding
    let new_binding = terminal_section
        .bindings
        .iter()
        .find(|b| b.keystroke.text == "ctrl-?")
        .expect("should have ctrl-? binding");
    assert_eq!(new_binding.action, json!("zetta::NewTab"));
    // Should NOT have the old default binding for NewTab
    let old_binding = terminal_section
        .bindings
        .iter()
        .find(|b| b.keystroke.text == "ctrl-shift-t");
    assert!(
        old_binding.is_none(),
        "old default binding for NewTab should be removed when rebound"
    );
    // Other default bindings should still exist
    assert!(terminal_section.bindings.len() > 1);
    fs::remove_file(root).unwrap();
}
