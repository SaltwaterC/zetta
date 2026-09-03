use super::*;

#[test]
fn parses_profile_with_arguments() {
    let profile = parse_profile(&serde_json::json!({
        "name": "WSL Ubuntu",
        "program": "wsl.exe",
        "args": ["-d", "Ubuntu"]
    }))
    .unwrap();
    assert_eq!(profile.name, "WSL Ubuntu");
    assert!(matches!(
        profile.command,
        Some(Shell::WithArguments { ref program, ref args, .. })
            if program == "wsl.exe" && args == &["-d", "Ubuntu"]
    ));
}

#[test]
fn parses_dark_themes_for_global_profiles_and_pane_template_leaves() {
    let config = Config::parse(
        r##"{
            "theme": "Solarized Light",
            "dark_theme": "Solarized Dark",
            "profiles": [
                {
                    "name": "Dark Shell",
                    "program": "dark-shell",
                    "theme": "One Light",
                    "dark_theme": "Dracula"
                }
            ],
            "pane_split_templates": {
                "custom": {
                    "layout": {
                        "vertical": [
                            { "profile": "Dark Shell", "dark_theme": "Gruvbox Dark" },
                            {}
                        ]
                    }
                }
            }
        }"##,
        None,
        None,
    )
    .unwrap();

    assert_eq!(config.theme.as_deref(), Some("Solarized Light"));
    assert_eq!(config.dark_theme.as_deref(), Some("Solarized Dark"));
    let profile = config
        .profiles
        .iter()
        .find(|profile| profile.name == "Dark Shell")
        .unwrap();
    assert_eq!(profile.theme.as_deref(), Some("One Light"));
    assert_eq!(profile.dark_theme.as_deref(), Some("Dracula"));
    let leaf = &config.pane_split_templates["custom"].pane_specifications()[0];
    assert_eq!(leaf.dark_theme.as_deref(), Some("Gruvbox Dark"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn homebrew_shells_are_profiles_with_their_installed_program_paths() {
    let prefix = tempfile::tempdir().unwrap();
    let bin = prefix.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("brew"), "").unwrap();
    fs::write(bin.join("fish"), "").unwrap();
    fs::write(bin.join("bash"), "").unwrap();

    let profiles = homebrew_shell_profiles([prefix.path().to_path_buf()]);

    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].name, "Bash (Homebrew)");
    assert_eq!(
        profiles[0].command,
        Shell::Program(bin.join("bash").to_string_lossy().into_owned())
    );
    assert_eq!(profiles[1].name, "Fish (Homebrew)");
    assert_eq!(
        profiles[1].command,
        Shell::Program(bin.join("fish").to_string_lossy().into_owned())
    );
    assert_eq!(profiles[0].icon, ProfileIcon::Bash);
    assert_eq!(profiles[1].icon, ProfileIcon::Fish);
}

#[test]
fn profile_icon_configuration_accepts_automatic_and_explicit_values() {
    let config = Config::parse(
        r#"{
            "profiles": [
                { "name": "Auto Fish", "program": "/custom/fish", "icon": "auto" },
                { "name": "Explicit Shell", "program": "/custom/fish", "icon": "bash" },
                { "name": "Null Shell", "program": "/custom/zsh", "icon": null }
            ]
        }"#,
        None,
        None,
    )
    .unwrap();

    let profile = |name: &str| {
        config
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .unwrap()
    };
    assert_eq!(profile("Auto Fish").icon, ProfileIcon::Fish);
    assert_eq!(profile("Explicit Shell").icon, ProfileIcon::Bash);
    assert_eq!(profile("Null Shell").icon, ProfileIcon::Zsh);
    assert!(
        Config::parse(
            r#"{"profiles":[{"name":"Invalid","program":"bash","icon":"terminal"}]}"#,
            None,
            None,
        )
        .is_err()
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn path_resolved_homebrew_shells_match_direct_discovery() {
    let prefix = tempfile::tempdir().unwrap();
    let bin = prefix.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("brew"), "").unwrap();
    fs::write(bin.join("bash"), "").unwrap();
    fs::write(bin.join("fish"), "").unwrap();

    let prefix_path = prefix.path().to_path_buf();
    let direct_profiles = homebrew_shell_profiles([prefix_path.clone()]);
    for program in ["bash", "fish"] {
        let path = command_path_in(program, bin.as_os_str()).unwrap();
        let path_profile =
            homebrew_profile_for_path(&path, std::slice::from_ref(&prefix_path)).unwrap();
        let direct_profile = direct_profiles
            .iter()
            .find(|profile| profile.command == Shell::Program(path.to_string_lossy().into_owned()))
            .cloned()
            .unwrap();

        assert_eq!(path_profile, direct_profile);
    }
}

#[test]
fn configuration_uses_profile_terminology() {
    assert!(
        validate_config_fields(&serde_json::json!({
            "default_profile": "System",
            "new_tab_profile": "default",
            "profiles": []
        }))
        .is_ok()
    );

    let error = validate_config_fields(&serde_json::json!({
        "default_shell": "System",
        "shells": []
    }))
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unrecognized configuration field")
    );

    let keymap_error = validate_config_fields(&serde_json::json!({
        "keymap": "custom-keymap.json"
    }))
    .unwrap_err();
    assert!(
        keymap_error
            .to_string()
            .contains("unrecognized configuration field")
    );
}

#[test]
fn default_working_directory_is_the_user_home() {
    let config = Config::defaults(None, None);
    assert_eq!(config.working_directory, Some(home_dir()));
    assert!(!config.working_directory_configured);
    assert_eq!(config.http_server_port, DEFAULT_HTTP_PORT);
    assert_eq!(config.tftp_server_port, DEFAULT_TFTP_SERVER_PORT);
    assert_eq!(config.default_tab_icon, Some(IconName::Terminal));
    assert_eq!(config.pane_controls_position, PaneControlsPosition::Right);
    assert!(!config.pane_controls_hidden_by_default);
    assert_eq!(config.working_directory_scope, WorkingDirectoryScope::Tab);
    assert_eq!(config.new_tab_profile, NewTabProfile::Default);
    assert!(!config.compact_mode);
    assert!(config.hide_pane_size);
    assert!(!config.hide_title_bar_labels);
    assert!(!config.hide_title_bar_buttons);
    assert_eq!(config.hide_title_bar_menus, cfg!(target_os = "macos"));
}

#[test]
fn default_tab_icon_accepts_an_icon_or_null() {
    assert_eq!(
        Config::parse(r#"{"default_tab_icon":"star"}"#, None, None)
            .unwrap()
            .default_tab_icon,
        Some(IconName::Star)
    );
    assert_eq!(
        Config::parse(r#"{"default_tab_icon":null}"#, None, None)
            .unwrap()
            .default_tab_icon,
        None
    );
    assert!(Config::parse(r#"{"default_tab_icon":"missing"}"#, None, None).is_err());
}

#[test]
fn validates_working_directory_scope() {
    for (value, expected) in [
        ("none", WorkingDirectoryScope::None),
        ("pane", WorkingDirectoryScope::Pane),
        ("tab", WorkingDirectoryScope::Tab),
    ] {
        assert_eq!(
            Config::parse(
                &format!(r#"{{"working_directory_scope":"{value}"}}"#),
                None,
                None,
            )
            .unwrap()
            .working_directory_scope,
            expected
        );
    }
    for value in [r#""#, "true", "null", r#""window""#] {
        assert!(
            Config::parse(
                &format!(r#"{{"working_directory_scope":{value}}}"#),
                None,
                None,
            )
            .is_err(),
            "accepted invalid working directory scope {value}"
        );
    }
}

#[test]
fn validates_new_tab_profile() {
    for (value, expected) in [
        ("default", NewTabProfile::Default),
        ("inherit", NewTabProfile::Inherit),
    ] {
        assert_eq!(
            Config::parse(&format!(r#"{{"new_tab_profile":"{value}"}}"#), None, None,)
                .unwrap()
                .new_tab_profile,
            expected
        );
    }
    for value in [r#""#, "true", "null", r#""current""#] {
        assert!(
            Config::parse(&format!(r#"{{"new_tab_profile":{value}}}"#), None, None,).is_err(),
            "accepted invalid new tab profile {value}"
        );
    }
}

#[test]
fn working_directory_scope_controls_inheritance_boundaries() {
    assert!(!WorkingDirectoryScope::None.inherits_for_new_tab());
    assert!(!WorkingDirectoryScope::None.inherits_for_new_pane());
    assert!(!WorkingDirectoryScope::Pane.inherits_for_new_tab());
    assert!(WorkingDirectoryScope::Pane.inherits_for_new_pane());
    assert!(WorkingDirectoryScope::Tab.inherits_for_new_tab());
    assert!(WorkingDirectoryScope::Tab.inherits_for_new_pane());
}

#[test]
fn validates_title_bar_visibility_settings() {
    let config = Config::parse(
        r#"{
            "hide_pane_size": false,
            "compact_mode": true,
            "hide_title_bar_labels": true,
            "hide_title_bar_buttons": true,
            "hide_title_bar_menus": false
        }"#,
        None,
        None,
    )
    .unwrap();
    assert!(config.compact_mode);
    assert!(!config.hide_pane_size);
    assert!(config.hide_title_bar_labels);
    assert!(config.hide_title_bar_buttons);
    assert!(!config.hide_title_bar_menus);

    for field in [
        "compact_mode",
        "hide_pane_size",
        "hide_title_bar_labels",
        "hide_title_bar_buttons",
        "hide_title_bar_menus",
    ] {
        assert!(
            Config::parse(&format!(r#"{{"{field}":"yes"}}"#), None, None).is_err(),
            "accepted invalid title bar setting {field}"
        );
    }
}

#[test]
fn validates_pane_controls_default_visibility() {
    assert!(
        Config::parse(r#"{"pane_controls_hidden_by_default":true}"#, None, None)
            .unwrap()
            .pane_controls_hidden_by_default
    );
    for value in [r#""hidden""#, "1", "null"] {
        assert!(
            Config::parse(
                &format!(r#"{{"pane_controls_hidden_by_default":{value}}}"#),
                None,
                None
            )
            .is_err(),
            "accepted invalid pane controls default visibility {value}"
        );
    }
}

#[test]
fn validates_pane_controls_position() {
    assert_eq!(
        Config::parse(r#"{"pane_controls_position":"left"}"#, None, None)
            .unwrap()
            .pane_controls_position,
        PaneControlsPosition::Left
    );
    for value in [r#""top""#, "true", "null"] {
        assert!(
            Config::parse(
                &format!(r#"{{"pane_controls_position":{value}}}"#),
                None,
                None
            )
            .is_err(),
            "accepted invalid pane controls position {value}"
        );
    }
}

#[test]
fn validates_http_server_port() {
    assert_eq!(
        Config::parse(r#"{"http_server_port":8080}"#, None, None)
            .unwrap()
            .http_server_port,
        8080
    );
    for value in ["0", "65536", "-1", "1.5", "\"8000\""] {
        assert!(
            Config::parse(&format!(r#"{{"http_server_port":{value}}}"#), None, None).is_err(),
            "accepted invalid HTTP server port {value}"
        );
    }
}

// The configuration directory's resolution, and the assertion that it never
// falls back to the working directory, moved to `crates/zmux/src/tests/paths.rs`
// with the code.

#[test]
fn validates_tftp_server_port() {
    assert_eq!(
        Config::parse(r#"{"tftp_server_port":1069}"#, None, None)
            .unwrap()
            .tftp_server_port,
        1069
    );
    for value in ["0", "65536", "-1", "1.5", "\"69\""] {
        assert!(
            Config::parse(&format!(r#"{{"tftp_server_port":{value}}}"#), None, None).is_err(),
            "accepted invalid TFTP server port {value}"
        );
    }
}

#[test]
fn session_authentication_is_not_a_mutable_global_configuration_value() {
    let error =
        Config::parse(r#"{"session_authentication":"replacement"}"#, None, None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unrecognized configuration field")
    );
}

#[test]
fn session_retention_is_typed_and_bounded() {
    let config = Config::parse(
        r#"{"sessions":{"retention":"memory","ring_bytes":4096}}"#,
        None,
        None,
    )
    .unwrap();
    assert_eq!(config.sessions.retention, SessionRetention::Memory);
    assert_eq!(config.sessions.ring_bytes, 4096);

    let none = Config::parse(
        r#"{"sessions":{"retention":"none","ring_bytes":65536}}"#,
        None,
        None,
    )
    .unwrap();
    assert_eq!(none.sessions.retention, SessionRetention::None);

    let migration =
        Config::parse(r#"{"sessions":{"retention":"persist"}}"#, None, None).unwrap_err();
    assert!(migration.to_string().contains("use \"disk\""));

    for document in [
        r#"{"sessions":{"ring_bytes":4095}}"#,
        r#"{"sessions":{"ring_bytes":67108865}}"#,
    ] {
        let error = Config::parse(document, None, None).unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}

#[cfg(feature = "session-persistence")]
#[test]
fn disk_session_persistence_round_trips_as_an_overlay() {
    let config = Config::parse(
        r#"{
            "sessions": {
                "retention": "disk",
                "persistence": {
                    "recipients": ["age1example", "github:zetta"],
                    "identity": "~/.config/age/identity.txt"
                }
            }
        }"#,
        None,
        None,
    )
    .unwrap();
    assert_eq!(config.sessions.retention, SessionRetention::Disk);
    assert_eq!(
        config.sessions.persistence.recipients,
        vec!["age1example", "github:zetta"]
    );
    assert_eq!(
        config.sessions.persistence.identity,
        Some(PathBuf::from("~/.config/age/identity.txt"))
    );
    assert!(!config.sessions.persistence.auto_protect);
}

#[cfg(feature = "session-persistence")]
#[test]
fn automatic_session_protection_parses_and_is_off_by_default() {
    let with_flag = Config::parse(
        r#"{"sessions":{"persistence":{"recipients":["age1example"],"auto_protect":true}}}"#,
        None,
        None,
    )
    .unwrap();
    assert!(with_flag.sessions.persistence.auto_protect);

    let without = Config::parse(r#"{"sessions":{"persistence":{}}}"#, None, None).unwrap();
    assert!(!without.sessions.persistence.auto_protect);

    let error = Config::parse(
        r#"{"sessions":{"persistence":{"auto_protect":"yes"}}}"#,
        None,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("auto_protect"));
}

/// The flag on its own is never enough: without a recipient there is nothing to
/// seal a key to, and without an effective identity there is nothing to open it
/// again. The conventional SSH identity is effective when it exists.
#[cfg(feature = "session-persistence")]
#[test]
fn automatic_protection_is_only_configured_with_a_recipient_and_an_identity() {
    let configured = |document| {
        Config::parse(document, None, None)
            .unwrap()
            .sessions
            .persistence
            .auto_protect_is_configured()
    };
    assert!(configured(
        r#"{"sessions":{"persistence":{"recipients":["age1example"],"identity":"/keys/id.txt","auto_protect":true}}}"#
    ));
    assert!(!configured(
        r#"{"sessions":{"persistence":{"recipients":["age1example"],"identity":"/keys/id.txt"}}}"#
    ));
    assert!(!configured(
        r#"{"sessions":{"persistence":{"identity":"/keys/id.txt","auto_protect":true}}}"#
    ));
    assert_eq!(
        configured(
            r#"{"sessions":{"persistence":{"recipients":["age1example"],"auto_protect":true}}}"#
        ),
        crate::config::default_session_identity_path().is_some()
    );
}

#[cfg(feature = "session-persistence")]
#[test]
fn an_unrecognized_persistence_field_is_still_rejected() {
    let error = Config::parse(
        r#"{"sessions":{"persistence":{"auto_protct":true}}}"#,
        None,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("auto_protct"));
}

#[cfg(not(feature = "session-persistence"))]
#[test]
fn disk_retention_reports_a_feature_error_in_constrained_builds() {
    let error = Config::parse(r#"{"sessions":{"retention":"disk"}}"#, None, None).unwrap_err();
    assert!(error.to_string().contains("session-persistence"));
}

#[test]
fn configured_home_alias_is_equivalent_to_the_default_directory() {
    let config_path = env::temp_dir().join(format!(
        "zetta-working-directory-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&config_path, r#"{"working_directory":"~"}"#).unwrap();

    let config = Config::load(Some(&config_path), None).unwrap();

    fs::remove_file(config_path).unwrap();
    assert_eq!(config.working_directory, Some(home_dir()));
    assert!(!config.working_directory_configured);

    let trailing_slash = Config::parse(r#"{"working_directory":"~/"}"#, None, None).unwrap();
    assert_eq!(trailing_slash.working_directory, Some(home_dir()));
    assert!(!trailing_slash.working_directory_configured);
}

#[test]
fn configured_non_default_working_directory_is_marked_explicit() {
    let config = Config::parse(r#"{"working_directory":"~/source"}"#, None, None).unwrap();

    assert_eq!(config.working_directory, Some(home_dir().join("source")));
    assert!(config.working_directory_configured);
}

#[test]
fn pane_split_templates_include_built_ins_and_custom_layouts() {
    let config = Config::parse(
        r#"{
            "pane_split_templates": {
                "custom": {
                    "layout": {
                        "horizontal": [
                            {},
                            { "vertical": [{}, {}] }
                        ]
                    }
                }
            }
        }"#,
        None,
        None,
    )
    .unwrap();

    assert_eq!(config.pane_split_templates["three-right"].pane_count(), 3);
    assert_eq!(
        config.pane_split_templates["three-right"].pane_labels(),
        vec![
            Some("left".to_owned()),
            Some("top-right".to_owned()),
            Some("bottom-right".to_owned()),
        ]
    );
    assert_eq!(config.pane_split_templates["three-left"].pane_count(), 3);
    assert_eq!(
        config.pane_split_templates["three-left"].pane_labels(),
        vec![
            Some("top-left".to_owned()),
            Some("bottom-left".to_owned()),
            Some("right".to_owned()),
        ]
    );
    assert!(matches!(
        config.pane_split_templates["three-left"].layout,
        PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Vertical,
            ref first,
            ref second,
        } if matches!(first.as_ref(), PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Horizontal,
            ..
        }) && matches!(second.as_ref(), PaneSplitTemplate::Pane(_))
    ));
    assert_eq!(config.pane_split_templates["quarters"].pane_count(), 4);
    assert_eq!(
        config.pane_split_templates["quarters"].pane_labels(),
        vec![
            Some("top-left".to_owned()),
            Some("bottom-left".to_owned()),
            Some("top-right".to_owned()),
            Some("bottom-right".to_owned()),
        ]
    );
    let four_vertical = &config.pane_split_templates["four-vertical"];
    assert_eq!(four_vertical.pane_count(), 4);
    assert_eq!(
        four_vertical.pane_labels(),
        vec![
            Some("left".to_owned()),
            Some("left-center".to_owned()),
            Some("right-center".to_owned()),
            Some("right".to_owned()),
        ]
    );

    fn assert_all_splits_are_vertical(template: &PaneSplitTemplate) {
        match template {
            PaneSplitTemplate::Pane(_) => {}
            PaneSplitTemplate::Split {
                axis,
                first,
                second,
            } => {
                assert_eq!(*axis, PaneSplitAxis::Vertical);
                assert_all_splits_are_vertical(first);
                assert_all_splits_are_vertical(second);
            }
        }
    }

    assert_all_splits_are_vertical(&four_vertical.layout);
    assert_eq!(config.pane_split_templates["custom"].pane_count(), 3);
    assert!(matches!(
        config.pane_split_templates["custom"].layout,
        PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Horizontal,
            ..
        }
    ));
}

#[test]
fn pane_split_templates_parse_labeled_leaves_in_traversal_order() {
    let config = Config::parse(
        r#"{
            "pane_split_templates": {
                "labeled": {
                    "layout": {
                        "horizontal": [
                            { "label": "top" },
                            { "vertical": [
                                { "label": "bottom-left" },
                                { "label": "bottom-right" }
                            ] }
                        ]
                    }
                }
            }
        }"#,
        None,
        None,
    )
    .unwrap();

    assert_eq!(
        config.pane_split_templates["labeled"].pane_labels(),
        vec![
            Some("top".to_owned()),
            Some("bottom-left".to_owned()),
            Some("bottom-right".to_owned()),
        ]
    );
}

#[test]
fn pane_split_templates_parse_fully_customized_leaves_and_same_file_profiles() {
    let config = Config::parse(
        r##"{
            "profiles": [
                { "name": "Server Shell", "program": "/bin/bash", "theme": "One Dark" }
            ],
            "pane_split_templates": {
                "custom": {
                    "env": { "SHARED": "yes", "ROLE": "default" },
                    "layout": {
                        "vertical": [
                            {
                                "label": "server",
                                "profile": "sErVeR sHeLl",
                                "theme": "One Light",
                                "env": { "ROLE": "server", "EMPTY": "" },
                                "overlay": {
                                    "text": "SERVER",
                                    "size": "xl",
                                    "opacity": 85,
                                    "color": "cyan"
                                }
                            },
                            {
                                "label": "client",
                                "command": { "program": "ssh", "args": ["host", "-p", "22"] }
                            }
                        ]
                    }
                }
            }
        }"##,
        None,
        None,
    )
    .unwrap();

    let leaves = config.pane_split_templates["custom"].pane_specifications();
    assert_eq!(leaves.len(), 2);
    assert_eq!(leaves[0].label.as_deref(), Some("server"));
    assert_eq!(leaves[0].profile.as_ref().unwrap().name, "Server Shell");
    assert_eq!(leaves[0].theme.as_deref(), Some("One Light"));
    assert_eq!(leaves[0].env["ROLE"], "server");
    assert_eq!(leaves[0].env["EMPTY"], "");
    assert_eq!(leaves[0].env["SHARED"], "yes");
    assert_eq!(
        leaves[0].overlay,
        Some(PaneSplitOverlay {
            text: Some("SERVER".to_owned()),
            size: Some(PaneSplitOverlaySize::ExtraLarge),
            opacity: Some(85),
            color: Some("cyan".to_owned()),
        })
    );
    assert_eq!(leaves[1].label.as_deref(), Some("client"));
    assert_eq!(leaves[1].env["SHARED"], "yes");
    assert_eq!(leaves[1].env["ROLE"], "default");
    assert_eq!(
        leaves[1].command,
        Some(PaneSplitCommand {
            program: "ssh".to_owned(),
            args: vec!["host".to_owned(), "-p".to_owned(), "22".to_owned()],
        })
    );
}

#[test]
fn pane_split_template_leaves_parse_stacked_commands_in_order() {
    let config = Config::parse(
        r##"{
            "pane_split_templates": {
                "custom": {
                    "layout": {
                        "vertical": [
                            {
                                "label": "server",
                                "stack": [
                                    { "program": "cargo", "args": ["watch", "-x", "test"] },
                                    { "program": "tail", "args": ["-f", "logs/app.log"] }
                                ]
                            },
                            { "label": "client" }
                        ]
                    }
                }
            }
        }"##,
        None,
        None,
    )
    .unwrap();

    let template = &config.pane_split_templates["custom"];
    assert_eq!(template.pane_count(), 2);
    assert_eq!(template.layout.stacked_command_count(), 2);
    let leaves = template.pane_specifications();
    assert_eq!(
        leaves[0].stack,
        vec![
            PaneSplitCommand {
                program: "cargo".to_owned(),
                args: vec!["watch".to_owned(), "-x".to_owned(), "test".to_owned()],
            },
            PaneSplitCommand {
                program: "tail".to_owned(),
                args: vec!["-f".to_owned(), "logs/app.log".to_owned()],
            },
        ]
    );
    assert!(leaves[1].stack.is_empty());
}

#[test]
fn pane_split_templates_reject_invalid_stacked_commands() {
    for stack in [
        serde_json::json!({ "program": "cargo" }),
        serde_json::json!(["cargo watch"]),
        serde_json::json!([{ "program": "" }]),
        serde_json::json!([{ "args": ["watch"] }]),
        serde_json::json!([{ "program": "cargo", "shell": true }]),
        serde_json::json!([{ "program": "cargo", "args": [7] }]),
    ] {
        let document = serde_json::json!({
            "pane_split_templates": {
                "bad": { "layout": { "vertical": [{ "stack": stack }, {}] } }
            }
        });
        assert!(
            Config::parse(&document.to_string(), None, None).is_err(),
            "expected an invalid stacked command: {document}"
        );
    }
}

#[test]
fn pane_split_templates_reject_stacked_commands_beyond_the_tab_budget() {
    let entry = serde_json::json!({ "program": "true" });
    let over_pane_limit = serde_json::json!({
        "pane_split_templates": {
            "bad": {
                "layout": {
                    "vertical": [
                        { "stack": vec![entry.clone(); 64] },
                        {}
                    ]
                }
            }
        }
    });
    let error = Config::parse(&over_pane_limit.to_string(), None, None).unwrap_err();
    assert!(
        format!("{error:#}").contains("more than 63 commands"),
        "unexpected per-pane stack error: {error:#}"
    );

    let over_combined_limit = serde_json::json!({
        "pane_split_templates": {
            "bad": {
                "layout": {
                    "vertical": [
                        { "stack": vec![entry.clone(); 63] },
                        {}
                    ]
                }
            }
        }
    });
    let error = Config::parse(&over_combined_limit.to_string(), None, None).unwrap_err();
    assert!(
        format!("{error:#}").contains("panes and stacked commands combined"),
        "unexpected combined budget error: {error:#}"
    );

    // 62 stacked commands beside two panes is exactly the 64-terminal budget.
    let at_limit = serde_json::json!({
        "pane_split_templates": {
            "fits": {
                "layout": {
                    "vertical": [
                        { "stack": vec![entry; 62] },
                        {}
                    ]
                }
            }
        }
    });
    Config::parse(&at_limit.to_string(), None, None).unwrap();
}

#[test]
fn pane_split_templates_reject_invalid_leaf_fields() {
    let invalid_documents = [
        serde_json::json!({
            "profile": "System",
            "command": { "program": "ssh" }
        }),
        serde_json::json!({ "command": { "args": ["host"] } }),
        serde_json::json!({ "command": { "program": "ssh", "args": [1] } }),
        serde_json::json!({ "env": { "ROLE": true } }),
        serde_json::json!({ "env": { "": "value" } }),
        serde_json::json!({ "overlay": { "size": "huge" } }),
        serde_json::json!({ "overlay": { "opacity": 101 } }),
        serde_json::json!({ "overlay": { "opacity": 12.5 } }),
        serde_json::json!({ "overlay": { "color": "not-a-color" } }),
        serde_json::json!({ "unknown": true }),
    ];

    for leaf in invalid_documents {
        let document = serde_json::json!({
            "pane_split_templates": {
                "bad": { "layout": { "vertical": [leaf, {}] } }
            }
        });
        assert!(
            Config::parse(&document.to_string(), None, None).is_err(),
            "expected invalid pane leaf: {document}"
        );
    }
}

#[test]
fn pane_split_templates_reject_unavailable_profiles_and_more_than_64_panes() {
    let unavailable = serde_json::json!({
        "pane_split_templates": {
            "bad": { "layout": { "vertical": [{ "profile": "missing" }, {}] } }
        }
    });
    let error = Config::parse(&unavailable.to_string(), None, None).unwrap_err();
    assert!(format!("{error:#}").contains("is not available"));

    fn balanced_tree(leaves: usize) -> serde_json::Value {
        if leaves == 1 {
            return serde_json::json!({});
        }
        let first = leaves / 2;
        serde_json::json!({
            "vertical": [balanced_tree(first), balanced_tree(leaves - first)]
        })
    }
    let tree = balanced_tree(65);
    let document = serde_json::json!({
        "pane_split_templates": { "too-many": { "layout": tree } }
    });
    let error = Config::parse(&document.to_string(), None, None).unwrap_err();
    assert!(
        format!("{error:#}").contains("between 2 and 64 panes"),
        "unexpected pane limit error: {error:#}"
    );
}

#[test]
fn pane_split_templates_reject_legacy_leaf_syntax() {
    for leaf in [
        serde_json::json!("pane"),
        serde_json::json!({"pane": "label"}),
    ] {
        let document = serde_json::json!({
            "pane_split_templates": {
                "legacy": { "layout": { "vertical": [leaf, {}] } }
            }
        });
        assert!(Config::parse(&document.to_string(), None, None).is_err());
    }
}

#[test]
fn pane_split_templates_reject_malformed_and_single_pane_layouts() {
    let malformed = Config::parse(
        r#"{"pane_split_templates":{"bad":{"layout":{"diagonal":["pane","pane"]}}}}"#,
        None,
        None,
    )
    .unwrap_err();
    assert!(
        malformed
            .to_string()
            .contains("parsing pane split template")
    );

    let single = Config::parse(
        r#"{"pane_split_templates":{"bad":{"layout":{}}}}"#,
        None,
        None,
    )
    .unwrap_err();
    assert!(single.to_string().contains("between 2 and 64 panes"));
}

#[test]
fn pane_split_templates_require_the_explicit_layout_envelope_and_validate_global_env() {
    let old_format = Config::parse(
        r#"{"pane_split_templates":{"bad":{"vertical":[{},{}]}}}"#,
        None,
        None,
    )
    .unwrap_err();
    assert!(format!("{old_format:#}").contains("unrecognized pane split template field"));

    for env in [
        serde_json::json!({"": "value"}),
        serde_json::json!({"VALID": true}),
    ] {
        let document = serde_json::json!({
            "pane_split_templates": {
                "bad": {
                    "env": env,
                    "layout": { "vertical": [{}, {}] }
                }
            }
        });
        assert!(Config::parse(&document.to_string(), None, None).is_err());
    }
}

#[test]
fn pane_split_templates_reject_invalid_labels_and_label_types() {
    for label in ["", "Top-left", "top_left", "top--left", "-top", "top-"] {
        let document = serde_json::json!({
            "pane_split_templates": {
                "bad": {
                    "layout": {
                        "vertical": [
                        { "label": label },
                        {}
                    ]
                    }
                }
            }
        });
        let error = Config::parse(&document.to_string(), None, None).unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("lowercase kebab-case"),
            "unexpected error for {label:?}: {error:#}"
        );
    }

    for value in ["null", "true", "1", "[]", "{}"] {
        let pane_value: serde_json::Value = serde_json::from_str(value).unwrap();
        let document = serde_json::json!({
            "pane_split_templates": {
                "bad": {
                    "layout": {
                        "vertical": [
                        { "label": pane_value },
                        {}
                    ]
                    }
                }
            }
        });
        let error = Config::parse(&document.to_string(), None, None).unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("pane template label must be a string"),
            "unexpected error for {value}: {error:#}"
        );
    }
}

#[test]
fn configured_profiles_extend_detected_profiles() {
    let mut profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "Zsh".to_owned(),
            command: Shell::Program("zsh".to_owned()),
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zsh,
        },
    ];

    merge_profiles(
        &mut profiles,
        vec![ProfileConfig {
            name: "Login Zsh".to_owned(),
            command: Some(Shell::Program("/bin/zsh".to_owned())),
            theme: None,
            dark_theme: None,
            icon: None,
            hidden: None,
        }],
    )
    .unwrap();

    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["System", "Zsh", "Login Zsh"]
    );
    assert_eq!(resolve_default_profile(&profiles, "system").unwrap(), 0);
    assert_eq!(resolve_default_profile(&profiles, "ZSH").unwrap(), 1);
}

#[test]
fn configured_profiles_override_detected_profiles_by_name() {
    let mut profiles = vec![Profile {
        name: "Zsh".to_owned(),
        command: Shell::Program("zsh".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zsh,
    }];

    merge_profiles(
        &mut profiles,
        vec![ProfileConfig {
            name: "zsh".to_owned(),
            command: Some(Shell::WithArguments {
                program: "/bin/zsh".to_owned(),
                args: vec!["-l".to_owned()],
                title_override: Some("zsh".to_owned()),
            }),
            theme: Some("Solarized Dark".to_owned()),
            dark_theme: Some("Dracula".to_owned()),
            icon: None,
            hidden: None,
        }],
    )
    .unwrap();

    assert_eq!(profiles.len(), 1);
    assert!(matches!(
        profiles[0].command,
        Shell::WithArguments { ref args, .. } if args == &["-l"]
    ));
    assert_eq!(profiles[0].theme.as_deref(), Some("Solarized Dark"));
    assert_eq!(profiles[0].dark_theme.as_deref(), Some("Dracula"));
}

#[test]
fn profile_theme_override_does_not_require_a_program() {
    let mut profiles = vec![Profile {
        name: "Zsh".to_owned(),
        command: Shell::Program("zsh".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zsh,
    }];
    let profile = parse_profile(&serde_json::json!({
        "name": "Zsh",
        "theme": "Solarized Dark"
    }))
    .unwrap();

    merge_profiles(&mut profiles, vec![profile]).unwrap();

    assert!(matches!(profiles[0].command, Shell::Program(ref program) if program == "zsh"));
    assert_eq!(profiles[0].theme.as_deref(), Some("Solarized Dark"));
}

#[test]
fn configured_profiles_can_hide_detected_profiles_by_name() {
    let config = Config::parse(
        r#"{
            "profiles": [
                { "name": "system", "hidden": true }
            ]
        }"#,
        None,
        None,
    )
    .unwrap();

    assert!(profile_is_hidden(
        &config.profiles[0],
        &config.hidden_profiles
    ));
}

#[test]
fn hidden_profiles_do_not_consume_visible_profile_slots() {
    let profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "Hidden".to_owned(),
            command: Shell::Program("hidden-shell".to_owned()),
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "Visible".to_owned(),
            command: Shell::Program("visible-shell".to_owned()),
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
    ];
    let hidden = HashSet::from(["hidden".to_owned()]);

    assert_eq!(visible_profile_count(&profiles, &hidden), 2);
    assert_eq!(visible_profile_index(&profiles, &hidden, 1), Some(0));
    assert_eq!(visible_profile_index(&profiles, &hidden, 2), Some(2));
    assert_eq!(visible_profile_index(&profiles, &hidden, 3), None);
}

#[test]
fn profile_hidden_must_be_boolean() {
    let error = Config::parse(
        r#"{"profiles":[{"name":"System","hidden":"yes"}]}"#,
        None,
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("profile.hidden must be a boolean")
    );
}

#[test]
fn parses_utf8_wsl_distribution_names() {
    assert_eq!(
        parse_wsl_distribution_names(b"Ubuntu\r\nDocker-Desktop\r\nDebian\r\nubuntu\r\n\r\n"),
        ["Ubuntu", "Debian"]
    );
}

#[test]
fn parses_utf16_wsl_distribution_names() {
    let output = "Ubuntu-24.04\r\nopenSUSE Tumbleweed\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();

    assert_eq!(
        parse_wsl_distribution_names(&output),
        ["Ubuntu-24.04", "openSUSE Tumbleweed"]
    );
}

#[test]
fn parses_big_endian_utf16_wsl_distribution_names() {
    let mut output = vec![0xfe, 0xff];
    output.extend("Debian\r\n".encode_utf16().flat_map(u16::to_be_bytes));

    assert_eq!(parse_wsl_distribution_names(&output), ["Debian"]);
}

#[test]
fn creates_a_profile_for_each_wsl_distribution() {
    let profiles = wsl_profiles_from_output("wsl.exe", b"Ubuntu\r\nDebian\r\n");

    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].name, "WSL: Ubuntu");
    assert_eq!(profiles[0].icon, ProfileIcon::Tux);
    assert!(matches!(
        profiles[0].command,
        Shell::WithArguments {
            ref program,
            ref args,
            ref title_override,
        } if program == "wsl.exe"
            && args == &["--distribution", "Ubuntu"]
            && title_override.as_deref() == Some("WSL: Ubuntu")
    ));
}

#[test]
fn creates_msys2_profiles_for_installed_shells_using_the_launcher() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("msys2_shell.cmd"), "").unwrap();
    fs::create_dir_all(root.path().join("usr/bin")).unwrap();
    fs::write(root.path().join("usr/bin/bash.exe"), "").unwrap();
    fs::write(root.path().join("usr/bin/zsh.exe"), "").unwrap();

    let profiles = msys2_profiles(root.path());

    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["MSYS2", "MSYS2: Zsh"]
    );
    for (profile, shell) in profiles.iter().zip(["bash", "zsh"]) {
        assert_eq!(
            profile.icon,
            if shell == "bash" {
                ProfileIcon::Bash
            } else {
                ProfileIcon::Zsh
            }
        );
        assert!(matches!(
            profile.command,
            Shell::WithArguments {
                ref program,
                ref args,
                ..
            } if program == "cmd.exe"
                && args[..3] == ["/d", "/s", "/c"]
                && args[3].starts_with("\"\"")
                && args[3].contains("msys2_shell.cmd\" -defterm")
                && args[3].contains("-defterm -here -no-start -msys -use-full-path")
                && args[3].ends_with(&format!("-shell {shell}\""))
        ));
    }
}

#[test]
fn omits_msys2_zsh_profile_when_zsh_is_not_installed() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("usr/bin")).unwrap();
    fs::write(root.path().join("usr/bin/bash.exe"), "").unwrap();

    let profiles = msys2_profiles(root.path());

    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].name, "MSYS2");
}

#[test]
fn creates_cygwin_profiles_for_installed_shells_with_direct_commands() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("bin")).unwrap();
    fs::write(root.path().join("bin/cygwin1.dll"), "").unwrap();
    for shell in ["bash", "zsh", "fish", "nu"] {
        fs::write(root.path().join("bin").join(format!("{shell}.exe")), "").unwrap();
    }

    let profiles = cygwin_profiles(root.path());

    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["Cygwin", "Cygwin: Zsh", "Cygwin: Fish", "Cygwin: Nushell"]
    );
    for (profile, (shell, icon)) in profiles.iter().zip([
        ("bash", ProfileIcon::Bash),
        ("zsh", ProfileIcon::Zsh),
        ("fish", ProfileIcon::Fish),
        ("nu", ProfileIcon::Zetta),
    ]) {
        assert_eq!(profile.icon, icon);
        assert!(matches!(
            &profile.command,
            Shell::WithArguments {
                program,
                args,
                title_override,
            } if program == &root.path().join("bin").join(format!("{shell}.exe")).display().to_string()
                && args == &["-l"]
                && title_override.as_deref() == Some(profile.name.as_str())
        ));
    }
}

#[test]
fn omits_cygwin_profiles_for_missing_shell_executables() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("bin")).unwrap();
    fs::write(root.path().join("bin/cygwin1.dll"), "").unwrap();
    fs::write(root.path().join("bin/bash.exe"), "").unwrap();
    fs::write(root.path().join("bin/nu.exe"), "").unwrap();

    let profiles = cygwin_profiles(root.path());

    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["Cygwin", "Cygwin: Nushell"]
    );
}

#[test]
fn skips_incomplete_cygwin_roots_when_a_later_root_has_shells() {
    let incomplete = tempfile::tempdir().unwrap();
    fs::create_dir_all(incomplete.path().join("bin")).unwrap();
    fs::write(incomplete.path().join("bin/cygwin1.dll"), "").unwrap();

    let complete = tempfile::tempdir().unwrap();
    fs::create_dir_all(complete.path().join("bin")).unwrap();
    fs::write(complete.path().join("bin/cygwin1.dll"), "").unwrap();
    fs::write(complete.path().join("bin/bash.exe"), "").unwrap();

    assert_eq!(
        select_cygwin_installation_root([
            incomplete.path().to_path_buf(),
            complete.path().to_path_buf(),
        ]),
        Some(complete.path().to_path_buf())
    );
}

#[test]
fn normalizes_cygwin_registry_installation_paths() {
    assert_eq!(
        normalize_cygwin_registry_path(r"\??\C:\cygwin64"),
        Some(PathBuf::from(r"C:\cygwin64"))
    );
    assert_eq!(
        normalize_cygwin_registry_path(r"\\??\D:\Tools\cygwin"),
        Some(PathBuf::from(r"D:\Tools\cygwin"))
    );
    assert_eq!(
        normalize_cygwin_registry_path(r"\??\UNC\server\share\cygwin"),
        Some(PathBuf::from(r"\\server\share\cygwin"))
    );
    assert_eq!(normalize_cygwin_registry_path("relative\npath"), None);
}

#[cfg(windows)]
#[test]
fn msys2_launcher_command_supports_custom_paths_with_spaces() {
    use std::os::windows::process::CommandExt as _;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("custom MSYS2 installation");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::write(
        root.join("msys2_shell.cmd"),
        "@echo off\r\nif \"%7\"==\"zsh\" exit /b 0\r\nexit /b 1\r\n",
    )
    .unwrap();
    fs::write(root.join("usr/bin/zsh.exe"), "").unwrap();
    let profile = msys2_profiles(&root).pop().unwrap();
    let Shell::WithArguments { program, args, .. } = profile.command else {
        panic!("MSYS2 profile did not include launcher arguments");
    };

    let status = Command::new(program)
        .raw_arg(args.join(" "))
        .status()
        .unwrap();

    assert!(status.success());
}

#[cfg(windows)]
#[test]
fn reads_custom_msys2_root_from_an_installer_shortcut() {
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, IPersistFile,
            },
            UI::Shell::{IShellLinkW, ShellLink},
        },
        core::{HSTRING, Interface},
    };

    let temporary = tempfile::tempdir().unwrap();
    let shortcut = temporary.path().join("MSYS2 MSYS.lnk");
    let root = temporary.path().join("custom MSYS2 installation");
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(&HSTRING::from(root.join("msys2.exe").as_os_str()))
                .unwrap();
            link.SetWorkingDirectory(&HSTRING::from(root.as_os_str()))
                .unwrap();
            let persist: IPersistFile = link.cast().unwrap();
            persist
                .Save(&HSTRING::from(shortcut.as_os_str()), true)
                .unwrap();

            assert_eq!(shortcut_working_directory(&shortcut), Some(root));
        }
        CoUninitialize();
    }
}

#[test]
fn validates_max_scroll_history_lines() {
    assert_eq!(
        parse_max_scroll_history_lines(&serde_json::json!(0)).unwrap(),
        0
    );
    assert_eq!(
        parse_max_scroll_history_lines(&serde_json::json!(2_147_483_647)).unwrap(),
        2_147_483_647
    );
    assert!(parse_max_scroll_history_lines(&serde_json::json!(-1)).is_err());
    assert!(parse_max_scroll_history_lines(&serde_json::json!(2_147_483_648_u64)).is_err());
    assert!(parse_max_scroll_history_lines(&serde_json::json!(1.5)).is_err());
}

#[test]
fn validates_inactive_pane_opacity() {
    assert_eq!(DEFAULT_INACTIVE_PANE_OPACITY, 0.8);
    assert_eq!(
        parse_inactive_pane_opacity(&serde_json::json!(0.8)).unwrap(),
        0.8
    );
    assert!(parse_inactive_pane_opacity(&serde_json::json!(-0.1)).is_err());
    assert!(parse_inactive_pane_opacity(&serde_json::json!(1.1)).is_err());
    assert!(parse_inactive_pane_opacity(&serde_json::json!("dim")).is_err());
}

/// `src/mux_identity.rs` reads `sessions.persistence.identity` on its own so the
/// `zmux` binary need not parse a whole configuration. This is what keeps the
/// two readers agreeing about what that field means, including the `~/` shorthand
/// they both have to expand. Both readers also use the conventional SSH identity
/// when the field is absent and that file exists.
#[cfg(feature = "session-persistence")]
#[test]
fn the_command_line_identity_reader_agrees_with_the_configuration_parser() {
    let directory = tempfile::tempdir().unwrap();
    for identity in ["/keys/zetta.txt", "~/keys/zetta.txt"] {
        let path = directory.path().join("config.json");
        let document = format!(r#"{{"sessions":{{"persistence":{{"identity":"{identity}"}}}}}}"#);
        std::fs::write(&path, &document).unwrap();

        let parsed = Config::parse(&document, None, None)
            .unwrap()
            .sessions
            .persistence
            .resolved_identity();
        let read = crate::mux_identity::configured_identity_paths(Some(path));

        assert_eq!(read, parsed.into_iter().collect::<Vec<_>>(), "{identity}");
    }
}

/// An absent field resolves to the same conventional identity in both readers
/// when `~/.ssh/id_ed25519` exists, and to no identity otherwise.
#[cfg(feature = "session-persistence")]
#[test]
fn both_identity_readers_treat_an_unset_identity_the_same_way() {
    let directory = tempfile::tempdir().unwrap();
    for document in [
        r#"{"sessions":{"persistence":{}}}"#,
        r#"{"sessions":{"persistence":{"identity":null}}}"#,
        r#"{"sessions":{"persistence":{"identity":""}}}"#,
        r#"{"sessions":{"persistence":{"identity":"  "}}}"#,
        r#"{}"#,
    ] {
        let path = directory.path().join("config.json");
        std::fs::write(&path, document).unwrap();

        let parsed = Config::parse(document, None, None)
            .unwrap()
            .sessions
            .persistence
            .resolved_identity();
        let read = crate::mux_identity::configured_identity_paths(Some(path));
        assert_eq!(read, parsed.into_iter().collect::<Vec<_>>(), "{document}");
    }
}
