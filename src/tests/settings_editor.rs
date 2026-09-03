use super::*;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::config::Profile;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use task::Shell;

fn settings_test_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn configuration_form_with_empty_custom_template() -> (ConfigurationForm, usize) {
    let path = settings_test_path("zetta-empty-pane-template");
    let config = Config::defaults(None, None);
    let mut form = ConfigurationForm::load(&path, &config).unwrap();
    let index = form.pane_templates.create_empty();
    (form, index)
}

fn pane_for_test(
    form: &mut ConfigurationForm,
    template: usize,
    path: PaneTemplateNodePath,
) -> &mut PaneTemplatePaneForm {
    match form.pane_templates.templates[template]
        .node
        .node_at_mut(path)
        .unwrap()
    {
        PaneTemplateNodeForm::Pane(pane) => pane,
        PaneTemplateNodeForm::Split { .. } => unreachable!(),
    }
}

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
    let profile_binding_index = form.sections[section_index]
        .bindings
        .iter()
        .position(|binding| binding.keystroke.text == "ctrl-shift-1")
        .unwrap();
    assert_eq!(
        form.sections[section_index].bindings[profile_binding_index]
            .keystroke
            .text,
        "ctrl-shift-1"
    );
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let output_section = output
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["context"] == "Zetta")
        .unwrap();
    assert_eq!(output_section["use_key_equivalents"], true);
    assert_eq!(output_section["bindings"]["ctrl-shift-1"][1]["slot"], 1);

    form.sections[section_index].bindings[profile_binding_index]
        .keystroke
        .text = "ctrl-shift-3".to_owned();
    let alias_output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let alias_section = alias_output
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["context"] == "Zetta")
        .unwrap();
    form.sections[section_index].bindings[profile_binding_index]
        .keystroke
        .text = "ctrl-shift-0".to_owned();
    let tenth_alias_output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let tenth_section = tenth_alias_output
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["context"] == "Zetta")
        .unwrap();
    fs::remove_file(root).unwrap();
    assert_eq!(alias_section["bindings"]["ctrl-shift-3"][1]["slot"], 1);
    assert_eq!(tenth_section["bindings"]["ctrl-shift-0"][1]["slot"], 1);
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
            "dark_theme": "Solarized Dark",
            "profiles": [{
                "name": "Login shell",
                "program": "/bin/sh",
                "args": ["-l"],
                "theme": "One Dark",
                "dark_theme": "Solarized Dark",
                "icon": "fish"
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
    assert_eq!(output["dark_theme"], "Solarized Dark");
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
    let login_profile = output["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["name"] == "Login shell")
        .unwrap();
    assert_eq!(login_profile["icon"], "fish");
    assert_eq!(login_profile["dark_theme"], "Solarized Dark");
}

#[test]
fn configuration_form_round_trip_preserves_custom_pane_template_trees() {
    let root = std::env::temp_dir().join(format!(
        "zetta-pane-template-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let layout = json!({
        "vertical": [
            {
                "label": "server",
                "profile": "System",
                "env": { "ROLE": "server" },
                "overlay": { "text": "SERVER", "size": "xl", "opacity": 85, "color": "cyan" }
            },
            { "command": { "program": "ssh", "args": ["host"] } }
        ]
    });
    let template = json!({
        "env": { "PROJECT": "zetta", "ROLE": "default" },
        "layout": layout
    });
    fs::write(
        &root,
        serde_json::to_string(&json!({
            "pane_split_templates": { "custom": template.clone() }
        }))
        .unwrap(),
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["pane_split_templates"]["custom"], template);
}

#[test]
fn pane_template_form_round_trip_covers_nested_leaf_options() {
    let root = settings_test_path("zetta-pane-template-recursive-form");
    let defaults = Config::defaults(Some(&root), None);
    let profile = defaults.profiles.first().unwrap().name.clone();
    let layout = json!({
        "vertical": [
            {
                "label": "server",
                "profile": profile,
                "theme": "One Dark",
                "dark_theme": "Solarized Dark",
                "env": { "ROLE": "server", "EMPTY": "" },
                "overlay": {
                    "text": "SERVER",
                    "size": "2xl",
                    "opacity": 85,
                    "color": "cyan"
                }
            },
            {
                "horizontal": [
                    {
                        "command": {
                            "program": "ssh",
                            "args": ["-o", "StrictHostKeyChecking=no", "host"]
                        }
                    },
                    {
                        "label": "client",
                        "env": { "ROLE": "client" },
                        "stack": [
                            { "program": "cargo", "args": ["watch", "-x", "test"] },
                            { "program": "tail" }
                        ]
                    }
                ]
            }
        ]
    });
    let template = json!({
        "env": { "PROJECT": "zetta" },
        "layout": layout
    });
    fs::write(
        &root,
        serde_json::to_string(&json!({
            "pane_split_templates": { "recursive": template.clone() }
        }))
        .unwrap(),
    )
    .unwrap();

    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    Config::parse(&serde_json::to_string(&output).unwrap(), Some(&root), None).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["pane_split_templates"]["recursive"], template);
}

#[test]
fn pane_template_creation_and_duplication_are_immediate_and_unique() {
    let (mut form, _) = configuration_form_with_empty_custom_template();
    let second_empty = form.pane_templates.create_empty();
    assert_eq!(
        form.pane_templates.templates[second_empty].name.text,
        "custom-2"
    );
    assert_eq!(form.pane_templates.selected_template, second_empty);
    assert_eq!(
        form.pane_templates.templates[second_empty].node,
        PaneTemplateNodeForm::empty_two_pane()
    );

    for (name, pane_count) in [
        ("three-right", 3),
        ("three-left", 3),
        ("quarters", 4),
        ("four-vertical", 4),
    ] {
        let preset = form
            .pane_templates
            .templates
            .iter()
            .position(|template| template.name.text == name)
            .unwrap();
        form.pane_templates.select_template(preset);
        let expected = form.pane_templates.templates[preset].node.clone();
        let index = form.pane_templates.duplicate_selected().unwrap();
        assert_eq!(
            form.pane_templates.templates[index].node.pane_count(),
            pane_count
        );
        assert_eq!(form.pane_templates.templates[index].node, expected);
        assert!(form.pane_templates.templates[index].editable());
        assert_eq!(form.pane_templates.selected_template, index);
    }
    let duplicate = form.pane_templates.duplicate_selected().unwrap();
    assert!(
        form.pane_templates.templates[duplicate]
            .name
            .text
            .ends_with("-copy")
    );
    let mut names = form.pane_templates.names();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), form.pane_templates.templates.len());

    let root = settings_test_path("zetta-pane-template-override");
    fs::write(
        &root,
        serde_json::to_string(&json!({
            "pane_split_templates": {
                "three-right": { "layout": { "vertical": [{}, {}] } }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let mut override_form = ConfigurationForm::load(&root, &config).unwrap();
    assert!(override_form.pane_templates.templates[0].editable());
    assert!(
        override_form
            .pane_templates
            .to_value()
            .unwrap()
            .get("three-right")
            .is_some()
    );
    override_form.pane_templates.select_template(0);
    override_form.pane_templates.delete_selected(false).unwrap();
    assert!(
        override_form
            .pane_templates
            .to_value()
            .unwrap()
            .get("three-right")
            .is_none()
    );
    fs::remove_file(root).unwrap();
}

#[test]
fn pane_template_global_environment_duplicates_and_validates() {
    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.select_template(index);
    form.pane_templates.templates[index].environment = vec![PaneTemplateEnvironmentForm {
        name: TextField::new("PROJECT"),
        value: TextField::new("zetta"),
    }];

    let duplicate = form.pane_templates.duplicate_selected().unwrap();
    assert_eq!(
        form.pane_templates.templates[duplicate].environment,
        form.pane_templates.templates[index].environment
    );
    assert!(form.pane_templates.validate().is_ok());

    form.pane_templates.templates[duplicate]
        .environment
        .push(PaneTemplateEnvironmentForm {
            name: TextField::new("PROJECT"),
            value: TextField::new("duplicate"),
        });
    let error = form.pane_templates.validate().unwrap_err();
    assert!(format!("{error:#}").contains("duplicate environment key"));
}

#[test]
fn pane_template_tree_operations_preserve_bounds_and_collapse_parents() {
    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.select_template(index);
    let left = PaneTemplateNodePath::ROOT.child(false).unwrap();
    form.pane_templates.select_node(left);
    assert!(form.pane_templates.toggle_node_selection(left));
    assert_eq!(
        form.pane_templates.selected_node,
        Some(PaneTemplateNodePath::ROOT)
    );
    assert!(
        form.pane_templates
            .split_selected_leaf(PaneSplitAxis::Horizontal)
            .is_err()
    );
    assert!(form.pane_templates.toggle_node_selection(left));
    assert_eq!(form.pane_templates.selected_node, Some(left));
    form.pane_templates
        .split_selected_leaf(PaneSplitAxis::Horizontal)
        .unwrap();
    assert_eq!(form.pane_templates.selected().unwrap().node.pane_count(), 3);

    let nested_leaf = left.child(true).unwrap();
    form.pane_templates.select_node(nested_leaf);
    form.pane_templates.remove_selected_node().unwrap();
    assert_eq!(form.pane_templates.selected().unwrap().node.pane_count(), 2);
    assert_eq!(form.pane_templates.selected_node, Some(left));

    form.pane_templates.select_node(PaneTemplateNodePath::ROOT);
    form.pane_templates
        .set_selected_axis(PaneSplitAxis::Horizontal)
        .unwrap();
    form.pane_templates.swap_selected_children().unwrap();
    assert!(matches!(
        form.pane_templates.selected_node().unwrap(),
        PaneTemplateNodeForm::Split {
            axis: PaneSplitAxis::Horizontal,
            ..
        }
    ));

    let max_index = form.pane_templates.create_empty();
    form.pane_templates.select_template(max_index);
    let mut rightmost = PaneTemplateNodePath::ROOT.child(true).unwrap();
    for _ in 0..62 {
        form.pane_templates.select_node(rightmost);
        form.pane_templates
            .split_selected_leaf(PaneSplitAxis::Vertical)
            .unwrap();
        rightmost = rightmost.child(true).unwrap();
    }
    assert_eq!(
        form.pane_templates.selected().unwrap().node.pane_count(),
        64
    );
    form.pane_templates.select_node(rightmost);
    assert!(
        form.pane_templates
            .split_selected_leaf(PaneSplitAxis::Vertical)
            .is_err()
    );

    let min_index = form.pane_templates.create_empty();
    form.pane_templates.select_template(min_index);
    form.pane_templates.select_node(left);
    form.pane_templates
        .split_selected_leaf(PaneSplitAxis::Vertical)
        .unwrap();
    form.pane_templates.select_node(left);
    assert!(form.pane_templates.remove_selected_node().is_err());
    assert_eq!(form.pane_templates.selected().unwrap().node.pane_count(), 3);
}

#[test]
fn pane_template_validation_reports_leaf_field_errors() {
    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.select_template(index);
    let path = PaneTemplateNodePath::ROOT.child(false).unwrap();
    form.pane_templates.select_node(path);

    pane_for_test(&mut form, index, path).label = TextField::new("Bad Label");
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.label = TextField::new("good-label");
        pane.source = PaneTemplateSourceForm::Command(PaneTemplateCommandForm {
            program: TextField::default(),
            args: vec![TextField::new("--flag")],
        });
    }
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.source = PaneTemplateSourceForm::Inherit;
        pane.environment = vec![
            PaneTemplateEnvironmentForm {
                name: TextField::new("ROLE"),
                value: TextField::new("one"),
            },
            PaneTemplateEnvironmentForm {
                name: TextField::new("ROLE"),
                value: TextField::new("two"),
            },
        ];
    }
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.environment.clear();
        pane.overlay = Some(PaneTemplateOverlayForm {
            text: TextField::new("overlay"),
            size: Some(PaneSplitOverlaySize::Large),
            opacity: TextField::new("101"),
            color: TextField::new("cyan"),
        });
    }
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.overlay.as_mut().unwrap().opacity = TextField::new("80");
        pane.overlay.as_mut().unwrap().color = TextField::new("not-a-color");
    }
    assert!(form.pane_templates.validate().is_err());
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.overlay = None;
        pane.stack = vec![PaneTemplateCommandForm {
            program: TextField::new("  "),
            args: vec![TextField::new("watch")],
        }];
    }
    let error = form.pane_templates.validate().unwrap_err();
    assert!(
        format!("{error:#}").contains("stacked command 1 program is required"),
        "unexpected stacked command error: {error:#}"
    );
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.stack[0].program = TextField::new("cargo");
    }
    form.pane_templates.validate().unwrap();

    // Two panes plus 63 stacked commands is one terminal past what a tab holds.
    {
        let pane = pane_for_test(&mut form, index, path);
        pane.stack = vec![
            PaneTemplateCommandForm {
                program: TextField::new("true"),
                args: Vec::new(),
            };
            63
        ];
    }
    let error = form.pane_templates.validate().unwrap_err();
    assert!(
        format!("{error:#}").contains("panes and stacked commands combined"),
        "unexpected combined budget error: {error:#}"
    );
}

#[test]
fn pane_template_keybindings_follow_renames_and_block_pending_deletes() {
    let missing = settings_test_path("zetta-pane-template-keymap");
    let mut keymap = KeymapForm::load(&missing).unwrap();
    keymap.sections[0].bindings.push(BindingForm {
        keystroke: TextField::new("alt-shift-p"),
        action: json!(["zetta::ApplyPaneSplitTemplate", { "name": "custom" }]),
    });
    assert!(rename_pane_template_bindings(
        &mut keymap,
        &[("custom".to_owned(), "renamed".to_owned())]
    ));
    assert_eq!(
        keymap.sections[0].bindings.last().unwrap().action[1]["name"],
        "renamed"
    );

    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.templates[index].name = TextField::new("renamed");
    form.pane_templates.select_template(index);
    assert!(form.pane_templates.delete_selected(true).is_err());
}

#[test]
fn configuration_form_omits_automatic_profile_icons() {
    let root = std::env::temp_dir().join(format!(
        "zetta-automatic-profile-icon-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"{"profiles":[{"name":"Automatic fish","program":"/opt/bin/fish"}]}"#,
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let form = ConfigurationForm::load(&root, &config).unwrap();
    let automatic = form
        .profiles
        .iter()
        .find(|profile| profile.name.text == "Automatic fish")
        .unwrap();
    assert_eq!(automatic.icon, None);
    assert_eq!(automatic.automatic_icon, ProfileIcon::Fish);
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let serialized = output["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["name"] == "Automatic fish")
        .unwrap();
    assert!(serialized.get("icon").is_none());
    fs::remove_file(root).unwrap();
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn configuration_form_round_trip_serializes_the_resolved_homebrew_profile_name() {
    let root = std::env::temp_dir().join(format!(
        "zetta-homebrew-profile-form-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &root,
        r#"{
            "default_profile": "fish (homebrew)",
            "profiles": [{"name":"fish (homebrew)","theme":"One Dark"}]
        }"#,
    )
    .unwrap();

    let mut config = Config::defaults(Some(&root), None);
    config.profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "Fish (Homebrew)".to_owned(),
            command: Shell::Program("/opt/homebrew/bin/fish".to_owned()),
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Fish,
        },
    ];
    config.default_profile = 1;

    let form = ConfigurationForm::load(&root, &config).unwrap();
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(output["default_profile"], "Fish (Homebrew)");
    assert_eq!(
        output["profiles"],
        json!([{"name":"Fish (Homebrew)","theme":"One Dark"}])
    );
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
    binding.keystroke.text = "ctrl-shift-z".to_owned();

    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let sections = output.as_array().unwrap();
    assert_eq!(sections.len(), 1);
    let bindings = sections[0]["bindings"].as_object().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings["ctrl-shift-z"], "zetta::NewTab");
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
fn keymap_template_exposes_all_builtin_shortcuts() {
    let template = bundled_keymap_template().unwrap();
    let assert_binding = |context: &str, keystroke: &str, action: &str| {
        let section = template
            .iter()
            .find(|section| section["context"] == context)
            .unwrap_or_else(|| panic!("missing keymap context {context:?}"));
        let binding = section["bindings"]
            .as_object()
            .and_then(|bindings| bindings.get(keystroke))
            .unwrap_or_else(|| panic!("missing keymap binding {keystroke:?} in {context:?}"));
        assert_eq!(
            binding, action,
            "wrong action for {context:?} {keystroke:?}"
        );
    };

    for (keystroke, action) in [
        ("ctrl-shift-q", "zetta::CloseWindow"),
        ("ctrl-shift-x", "zetta::CloseAllWindows"),
        ("alt-shift-x", "zetta::ClosePane"),
        ("alt-shift-l", "zetta::RotatePaneLayout"),
        ("alt-shift-k", "zetta::RotatePaneLayoutCounterClockwise"),
        ("ctrl-shift-g", "zetta::ToggleTabMoveMode"),
        ("alt-shift-a", "terminal_view::SelectAll"),
        ("ctrl-shift-s", "zetta::ToggleSilentMode"),
        ("ctrl-shift-m", "zetta::ToggleMultiCommand"),
        ("ctrl-shift-l", "terminal::Clear"),
        ("shift-insert", "terminal::Paste"),
        ("alt-shift-f", "terminal_view::SearchScrollback"),
        ("ctrl-alt-v", "terminal::PasteTrimmed"),
        ("ctrl-cmd-v", "terminal::PasteTrimmed"),
        ("ctrl-alt-left", "zetta::PreviousTab"),
        ("ctrl-alt-right", "zetta::NextTab"),
        ("ctrl-cmd-left", "zetta::PreviousTab"),
        ("ctrl-cmd-right", "zetta::NextTab"),
        ("alt-shift-s", "zetta::SavePaneOutput"),
        ("ctrl-shift-y", "zetta::ChangeTabIcon"),
        ("alt-shift-t", "zetta::ChangePaneTheme"),
        ("alt-shift-r", "zetta::RenamePane"),
        ("ctrl-+", "zetta::IncreaseTerminalFontSize"),
        ("ctrl-alt-r", "zetta::ReloadConfiguration"),
        ("alt-left", "zetta::FocusPaneLeft"),
        ("alt-right", "zetta::FocusPaneRight"),
        ("alt-up", "zetta::FocusPaneUp"),
        ("alt-down", "zetta::FocusPaneDown"),
        ("alt-shift-down", "zetta::MinimizePane"),
        ("alt-shift-up", "zetta::RestoreMinimizedPane"),
        ("alt-shift-left", "zetta::SelectPreviousMinimizedPane"),
        ("alt-shift-right", "zetta::SelectNextMinimizedPane"),
        ("alt-shift-=", "zetta::IncreasePaneFontSize"),
        ("alt-shift-+", "zetta::IncreasePaneFontSize"),
        ("alt-shift--", "zetta::DecreasePaneFontSize"),
        ("alt-shift-0", "zetta::ResetPaneFontSize"),
        ("shift-f11", "zetta::ToggleFullscreen"),
    ] {
        assert_binding("Zetta > Terminal", keystroke, action);
    }

    assert_binding("Zetta", "alt-space", "zetta::OpenApplicationMenu");
    assert_binding("Zetta", "ctrl-shift-h", "zetta::HideWindow");

    for (keystroke, action) in [
        ("cmd-h", "zetta::HideWindow"),
        ("cmd-shift-h", "zetta::HideWindow"),
        ("ctrl-shift-h", "zetta::MinimizeWindow"),
    ] {
        assert_binding("", keystroke, action);
    }

    for (keystroke, action) in [
        ("left", "zetta::ResizePaneLeft"),
        ("right", "zetta::ResizePaneRight"),
        ("up", "zetta::ResizePaneUp"),
        ("down", "zetta::ResizePaneDown"),
    ] {
        assert_binding("Zetta > PaneResize > Terminal", keystroke, action);
    }
    for (keystroke, action) in [
        ("left", "zetta::MovePaneLeft"),
        ("right", "zetta::MovePaneRight"),
        ("up", "zetta::MovePaneUp"),
        ("down", "zetta::MovePaneDown"),
    ] {
        assert_binding("Zetta > PaneMove > Terminal", keystroke, action);
    }
    for (keystroke, action) in [
        ("left", "zetta::MoveTabLeft"),
        ("right", "zetta::MoveTabRight"),
    ] {
        assert_binding("Zetta > TabMove > Terminal", keystroke, action);
    }

    assert_binding(
        "Zetta > Terminal && selection",
        "cmd-c",
        "terminal_view::CopyAndClearSelection",
    );
    assert_binding(
        "Zetta > Terminal && selection",
        "ctrl-insert",
        "terminal_view::CopyAndClearSelection",
    );
    for (keystroke, action) in [
        ("left", "zetta::ActivateApplicationMenuLeft"),
        ("right", "zetta::ActivateApplicationMenuRight"),
        ("ctrl-tab", "zetta::NextTab"),
        ("ctrl-shift-tab", "zetta::PreviousTab"),
        ("ctrl-pageup", "zetta::NextTab"),
        ("ctrl-pagedown", "zetta::PreviousTab"),
        ("ctrl-alt-left", "zetta::PreviousTab"),
        ("ctrl-alt-right", "zetta::NextTab"),
        ("ctrl-cmd-left", "zetta::PreviousTab"),
        ("ctrl-cmd-right", "zetta::NextTab"),
    ] {
        assert_binding("Zetta > menu", keystroke, action);
    }
    assert_binding("Terminal", "ctrl-shift-w", "zetta::CloseTab");

    let terminal = template
        .iter()
        .find(|section| section["context"] == "Zetta > Terminal")
        .expect("bundled template must define the terminal context");
    assert!(terminal["bindings"].get("alt-shift-o").is_none());
    assert!(terminal["bindings"].get("alt-shift-e").is_none());
}

#[test]
fn keymap_template_displays_insert_clipboard_shortcuts_in_keymap_syntax() {
    let path = std::env::temp_dir().join(format!(
        "zetta-insert-keymap-template-{}",
        std::process::id()
    ));
    let form = KeymapForm::load(&path).unwrap();

    for (context, keystroke, action) in [
        (
            "Zetta > Terminal && selection",
            "ctrl-insert",
            "terminal_view::CopyAndClearSelection",
        ),
        ("Zetta > Terminal", "shift-insert", "terminal::Paste"),
    ] {
        let section = form
            .sections
            .iter()
            .find(|section| section.context.text == context)
            .unwrap_or_else(|| panic!("missing keymap context {context:?}"));
        let binding = section
            .bindings
            .iter()
            .find(|binding| binding.keystroke.text == keystroke)
            .unwrap_or_else(|| panic!("missing displayed keymap binding {keystroke:?}"));
        assert_eq!(binding.action, json!(action));
    }
}

#[test]
fn default_binding_lookup_keeps_plus_and_minus_shortcuts_distinct() {
    let defaults = default_bindings_by_context().unwrap();
    let terminal = defaults.get("Zetta > Terminal").unwrap();

    assert_eq!(
        terminal.get("ctrl-+"),
        Some(&json!("zetta::IncreaseTerminalFontSize"))
    );
    assert_eq!(
        terminal.get("ctrl--"),
        Some(&json!("zetta::DecreaseTerminalFontSize"))
    );
    assert_eq!(
        terminal.get("alt-shift-+"),
        Some(&json!("zetta::IncreasePaneFontSize"))
    );
    assert_eq!(
        terminal.get("alt-shift--"),
        Some(&json!("zetta::DecreasePaneFontSize"))
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

#[test]
fn keymap_explicit_unbind_is_loaded_and_serialized() {
    let root = std::env::temp_dir().join(format!(
        "zetta-keymap-explicit-unbind-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // User explicitly unbinds a default binding (e.g., unbind ctrl-shift-w which is CloseTab by default)
    fs::write(
        &root,
        r#"[{"context":"Zetta > Terminal","unbind":{"ctrl-shift-w":"zetta::CloseTab"}}]"#,
    )
    .unwrap();
    let form = KeymapForm::load(&root).unwrap();
    let terminal_section = form
        .sections
        .iter()
        .find(|s| s.context.text == "Zetta > Terminal")
        .expect("should have Zetta > Terminal section");
    // Should have the unbind entry
    assert!(terminal_section.unbind.contains_key("ctrl-shift-w"));
    assert_eq!(
        terminal_section.unbind.get("ctrl-shift-w"),
        Some(&"zetta::CloseTab".to_owned())
    );
    // Should NOT have the default binding for CloseTab in bindings list
    let close_tab_binding = terminal_section
        .bindings
        .iter()
        .find(|b| b.action_name() == "zetta::CloseTab");
    assert!(
        close_tab_binding.is_none(),
        "explicit unbind should remove default binding from bindings list"
    );
    // Serialize back and verify unbind is preserved
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let output_section = output
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["context"] == "Zetta > Terminal")
        .unwrap();
    assert!(output_section["unbind"].get("ctrl-shift-w").is_some());
    assert_eq!(output_section["unbind"]["ctrl-shift-w"], "zetta::CloseTab");
    fs::remove_file(root).unwrap();
}

#[test]
fn keymap_unbind_and_binding_both_preserved() {
    let root = std::env::temp_dir().join(format!(
        "zetta-keymap-unbind-and-binding-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // User unbinds one default and adds a custom binding
    fs::write(
        &root,
        r#"[{"context":"Zetta > Terminal","unbind":{"ctrl-shift-w":"zetta::CloseTab"},"bindings":{"ctrl-shift-x":"zetta::NewTab"}}]"#,
    )
    .unwrap();
    let form = KeymapForm::load(&root).unwrap();
    let terminal_section = form
        .sections
        .iter()
        .find(|s| s.context.text == "Zetta > Terminal")
        .expect("should have Zetta > Terminal section");
    // Should have the unbind entry
    assert!(terminal_section.unbind.contains_key("ctrl-shift-w"));
    // Should have the custom binding
    assert!(
        terminal_section
            .bindings
            .iter()
            .any(|b| b.keystroke.text == "ctrl-shift-x")
    );
    // Serialize back and verify both are preserved
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    let output_section = output
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["context"] == "Zetta > Terminal")
        .unwrap();
    assert!(output_section["unbind"].get("ctrl-shift-w").is_some());
    assert!(output_section["bindings"].get("ctrl-shift-x").is_some());
    fs::remove_file(root).unwrap();
}

#[test]
fn merge_keymap_with_defaults_merges_unbind() {
    let user_value = json!([{
        "context": "Zetta > Terminal",
        "unbind": {"ctrl-shift-w": "zetta::CloseTab"}
    }]);
    let default_template = bundled_keymap_template().unwrap();
    let merged = merge_keymap_with_defaults(user_value, default_template).unwrap();
    let sections = merged.as_array().unwrap();
    let terminal_section = sections
        .iter()
        .find(|s| s["context"] == "Zetta > Terminal")
        .unwrap();
    // Should have unbind entry from user
    assert!(terminal_section["unbind"].get("ctrl-shift-w").is_some());
    assert_eq!(
        terminal_section["unbind"]["ctrl-shift-w"],
        "zetta::CloseTab"
    );
    // Should still have default bindings (except the unbound one)
    let close_tab_binding = terminal_section["bindings"].get("ctrl-shift-w");
    assert!(
        close_tab_binding.is_none(),
        "unbind should remove default binding"
    );
}

#[test]
fn bundled_keymap_defaults_are_parsed_once_and_shared() {
    let first = bundled_keymap_template().unwrap();
    let second = bundled_keymap_template().unwrap();
    assert!(std::ptr::eq(first, second));

    let first_bindings = default_bindings_by_context().unwrap();
    let second_bindings = default_bindings_by_context().unwrap();
    assert!(std::ptr::eq(first_bindings, second_bindings));

    // The shared map still describes the shared template: every context in the
    // template is present, with the same number of bindings.
    for section in first {
        let Some(context) = section.get("context").and_then(Value::as_str) else {
            continue;
        };
        let bindings = section
            .get("bindings")
            .and_then(Value::as_object)
            .expect("each bundled section has bindings");
        assert_eq!(
            first_bindings.get(context).map(IndexMap::len),
            Some(bindings.len()),
            "context {context:?} lost bindings when the defaults were cached"
        );
    }
}

#[test]
fn the_root_pane_template_path_has_no_parent() {
    let root = PaneTemplateNodePath::ROOT;
    // Selecting the root split toggles its selection off, which asks the root
    // for its parent: computing `length - 1` there panicked in debug builds and
    // wrapped to a 255-deep path in release ones.
    assert_eq!(root.parent(), None);

    let left = root.child(false).unwrap();
    let left_right = left.child(true).unwrap();
    assert_eq!(left_right.parent(), Some(left));
    assert_eq!(left.parent(), Some(root));
    assert_eq!(left_right.depth(), 2);
}

#[test]
fn toggling_the_selected_root_node_clears_the_selection_without_panicking() {
    let (mut form, index) = configuration_form_with_empty_custom_template();
    form.pane_templates.select_template(index);
    assert_eq!(
        form.pane_templates.selected_node,
        Some(PaneTemplateNodePath::ROOT)
    );

    assert!(
        form.pane_templates
            .toggle_node_selection(PaneTemplateNodePath::ROOT)
    );
    assert_eq!(form.pane_templates.selected_node, None);

    // And selecting it again brings the split details back.
    assert!(
        form.pane_templates
            .toggle_node_selection(PaneTemplateNodePath::ROOT)
    );
    assert_eq!(
        form.pane_templates.selected_node,
        Some(PaneTemplateNodePath::ROOT)
    );
}

/// The automatic-protection toggle appears only once there is something to seal
/// a session key to and an effective identity to open it with. The conventional
/// SSH identity counts when the identity field is blank.
#[cfg(feature = "session-persistence")]
#[test]
fn the_automatic_protection_toggle_is_offered_only_with_a_recipient_and_an_identity() {
    let root = settings_test_path("zetta-auto-protect-offered");
    let config = Config::defaults(Some(&root), None);
    let mut form = ConfigurationForm::load(&root, &config).unwrap();
    assert!(!form.session_auto_protect_is_offered());

    form.session_persistence_recipients = TextField::new("age1example".to_owned());
    assert_eq!(
        form.session_auto_protect_is_offered(),
        crate::config::default_session_identity_path().is_some()
    );

    form.session_persistence_identity = TextField::new("   ".to_owned());
    assert_eq!(
        form.session_auto_protect_is_offered(),
        crate::config::default_session_identity_path().is_some()
    );

    form.session_persistence_identity = TextField::new("~/keys/zetta.txt".to_owned());
    assert!(form.session_auto_protect_is_offered());

    form.session_persistence_recipients = TextField::new(String::new());
    assert!(!form.session_auto_protect_is_offered());
}

#[cfg(feature = "session-persistence")]
#[test]
fn automatic_protection_round_trips_through_the_configuration_form() {
    let root = settings_test_path("zetta-auto-protect-round-trip");
    fs::write(
        &root,
        serde_json::to_string(&json!({
            "sessions": {
                "persistence": {
                    "recipients": ["age1example"],
                    "identity": "~/keys/zetta.txt",
                    "auto_protect": true,
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let config = Config::load(Some(&root), None).unwrap();
    let mut form = ConfigurationForm::load(&root, &config).unwrap();
    assert!(form.session_persistence_auto_protect);

    form.session_persistence_auto_protect = false;
    let output: Value = serde_json::from_str(&form.to_json().unwrap()).unwrap();
    fs::remove_file(root).unwrap();

    assert_eq!(
        output["sessions"]["persistence"]["auto_protect"],
        json!(false)
    );
    // Written back beside what it depends on, so a reload cannot end up with the
    // flag set and nothing to seal to.
    assert_eq!(
        output["sessions"]["persistence"]["recipients"],
        json!(["age1example"])
    );
}
