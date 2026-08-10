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
        config.pane_split_templates["three-left"],
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

    assert_all_splits_are_vertical(four_vertical);
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
fn pane_split_templates_parse_labeled_and_legacy_leaves_in_traversal_order() {
    let config = Config::parse(
        r#"{
            "pane_split_templates": {
                "labeled": {
                    "horizontal": [
                        { "pane": "top" },
                        { "vertical": [
                            { "pane": "bottom-left" },
                            { "pane": "bottom-right" }
                        ] }
                    ]
                },
                "legacy": {
                    "vertical": ["pane", "pane"]
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
    assert_eq!(
        config.pane_split_templates["legacy"].pane_labels(),
        vec![None, None]
    );
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
fn pane_split_templates_reject_invalid_labels_and_label_types() {
    for label in ["", "Top-left", "top_left", "top--left", "-top", "top-"] {
        let document = serde_json::json!({
            "pane_split_templates": {
                "bad": {
                    "vertical": [
                        { "pane": label },
                        "pane"
                    ]
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
                    "vertical": [
                        { "pane": pane_value },
                        "pane"
                    ]
                }
            }
        });
        let error = Config::parse(&document.to_string(), None, None).unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("string label"),
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
        },
        Profile {
            name: "Hidden".to_owned(),
            command: Shell::Program("hidden-shell".to_owned()),
            theme: None,
        },
        Profile {
            name: "Visible".to_owned(),
            command: Shell::Program("visible-shell".to_owned()),
            theme: None,
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
