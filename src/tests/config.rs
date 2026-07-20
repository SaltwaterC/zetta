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
fn configuration_uses_profile_terminology() {
    assert!(
        validate_config_fields(&serde_json::json!({
            "default_profile": "System",
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
                    "horizontal": [
                        "pane",
                        { "vertical": ["pane", "pane"] }
                    ]
                }
            }
        }"#,
        None,
        None,
    )
    .unwrap();

    assert_eq!(config.pane_split_templates["three-right"].pane_count(), 3);
    assert_eq!(config.pane_split_templates["three-left"].pane_count(), 3);
    assert!(matches!(
        config.pane_split_templates["three-left"],
        PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Vertical,
            ref first,
            ref second,
        } if matches!(first.as_ref(), PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Horizontal,
            ..
        }) && matches!(second.as_ref(), PaneSplitTemplate::Pane)
    ));
    assert_eq!(config.pane_split_templates["quarters"].pane_count(), 4);
    assert_eq!(config.pane_split_templates["custom"].pane_count(), 3);
    assert!(matches!(
        config.pane_split_templates["custom"],
        PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Horizontal,
            ..
        }
    ));
}

#[test]
fn pane_split_templates_reject_malformed_and_single_pane_layouts() {
    let malformed = Config::parse(
        r#"{"pane_split_templates":{"bad":{"diagonal":["pane","pane"]}}}"#,
        None,
        None,
    )
    .unwrap_err();
    assert!(
        malformed
            .to_string()
            .contains("parsing pane split template")
    );

    let single =
        Config::parse(r#"{"pane_split_templates":{"bad":"pane"}}"#, None, None).unwrap_err();
    assert!(single.to_string().contains("between 2 and 64 panes"));
}

#[test]
fn configured_profiles_extend_detected_profiles() {
    let mut profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
        },
        Profile {
            name: "Zsh".to_owned(),
            command: Shell::Program("zsh".to_owned()),
            theme: None,
        },
    ];

    merge_profiles(
        &mut profiles,
        vec![ProfileConfig {
            name: "Login Zsh".to_owned(),
            command: Some(Shell::Program("/bin/zsh".to_owned())),
            theme: None,
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
        }],
    )
    .unwrap();

    assert_eq!(profiles.len(), 1);
    assert!(matches!(
        profiles[0].command,
        Shell::WithArguments { ref args, .. } if args == &["-l"]
    ));
    assert_eq!(profiles[0].theme.as_deref(), Some("Solarized Dark"));
}

#[test]
fn profile_theme_override_does_not_require_a_program() {
    let mut profiles = vec![Profile {
        name: "Zsh".to_owned(),
        command: Shell::Program("zsh".to_owned()),
        theme: None,
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
