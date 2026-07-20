use super::*;

#[cfg(windows)]
#[test]
fn executable_directory_is_prepended_to_native_terminal_path() {
    let executable_directory = Path::new(r"C:\Program Files\Zetta");
    let inherited = std::ffi::OsStr::new(r"C:\Windows\System32;C:\Tools");
    let path = path_with_entry_first(Some(inherited), executable_directory).unwrap();
    let entries = env::split_paths(&path).collect::<Vec<_>>();

    assert_eq!(entries[0], executable_directory);
    assert_eq!(entries[1], Path::new(r"C:\Windows\System32"));
    assert_eq!(entries[2], Path::new(r"C:\Tools"));
    assert!(
        path_with_entry_first(
            Some(path.as_os_str()),
            Path::new(r"c:\program files\zetta\")
        )
        .is_none()
    );
}

#[test]
fn version_flags_and_output_are_defined() {
    assert!(is_version_argument("-v"));
    assert!(is_version_argument("--version"));
    assert!(!is_version_argument("-V"));
    assert_eq!(
        version_text(),
        format!("Zetta {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn sessions_subcommand_supports_human_and_json_output() {
    let human = parse_args_from([OsString::from("sessions")]).unwrap();
    assert_eq!(
        human.mode,
        StartupMode::ListBackgroundSessions { json: false }
    );

    let json = parse_args_from([OsString::from("sessions"), OsString::from("--json")]).unwrap();
    assert_eq!(
        json.mode,
        StartupMode::ListBackgroundSessions { json: true }
    );
    assert!(parse_args_from([OsString::from("sessions"), OsString::from("--unknown")]).is_err());
}

#[test]
fn only_plain_application_launches_handoff_to_the_session_runner() {
    let plain = parse_args_from(Vec::<OsString>::new()).unwrap();
    let profile = parse_args_from([OsString::from("--profile"), OsString::from("System")]).unwrap();
    let sessions = parse_args_from([OsString::from("sessions")]).unwrap();

    assert!(should_handoff_to_existing_process(&plain));
    assert!(!should_handoff_to_existing_process(&profile));
    assert!(!should_handoff_to_existing_process(&sessions));
}

#[test]
fn process_quits_only_without_windows_or_dormant_session_runners() {
    assert!(should_quit_after_window_closed(0, 0));
    assert!(!should_quit_after_window_closed(0, 1));
    assert!(!should_quit_after_window_closed(1, 0));
}

#[test]
fn application_shutdown_is_managed_by_the_session_runner() {
    assert_eq!(zetta_quit_mode(), gpui::QuitMode::Explicit);
}

#[test]
fn terminal_rendering_profiler_arguments_are_cross_platform() {
    assert_eq!(
        parse_args_from([OsString::from("--profile-terminal-rendering")]).unwrap(),
        StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            mode: StartupMode::TerminalRenderingProfile,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            tftp_command: None,
        }
    );
    assert_eq!(
        parse_args_from([OsString::from("--terminal-render-workload")]).unwrap(),
        StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            mode: StartupMode::TerminalRenderingWorkload,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            tftp_command: None,
        }
    );
}

#[test]
fn shorthand_options_match_long_options() {
    let shorthand = parse_args_from([
        OsString::from("-P"),
        OsString::from("-s"),
        OsString::from("-r"),
        OsString::from("profile.json"),
        OsString::from("-d"),
        OsString::from("2.5"),
    ])
    .unwrap();
    let longhand = parse_args_from([
        OsString::from("--profile-terminal-rendering"),
        OsString::from("--profile-pane-stress"),
        OsString::from("--profile-report"),
        OsString::from("profile.json"),
        OsString::from("--profile-duration"),
        OsString::from("2.5"),
    ])
    .unwrap();
    assert_eq!(shorthand, longhand);

    let shorthand = parse_args_from([OsString::from("-p"), OsString::from("WSL: Ubuntu")]).unwrap();
    let longhand =
        parse_args_from([OsString::from("--profile"), OsString::from("WSL: Ubuntu")]).unwrap();
    assert_eq!(shorthand, longhand);
    assert_eq!(shorthand.profile.as_deref(), Some("WSL: Ubuntu"));

    let shorthand = parse_args_from([
        OsString::from("-c"),
        OsString::from("config.json"),
        OsString::from("-k"),
        OsString::from("keymap.json"),
    ])
    .unwrap();
    let longhand = parse_args_from([
        OsString::from("--config"),
        OsString::from("config.json"),
        OsString::from("--keymap"),
        OsString::from("keymap.json"),
    ])
    .unwrap();
    assert_eq!(shorthand, longhand);
}

#[test]
fn launch_profile_selects_an_available_profile_case_insensitively() {
    let mut config = Config::defaults(None, None);
    config.profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
        },
        Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::Program("wsl.exe".to_owned()),
            theme: None,
        },
    ];

    select_launch_profile(&mut config, Some("wsl: ubuntu")).unwrap();
    assert_eq!(config.default_profile, 1);

    let error = select_launch_profile(&mut config, Some("Missing")).unwrap_err();
    assert!(error.to_string().contains("is not available"));
}

#[test]
fn tftp_subcommand_is_parsed_without_starting_the_application() {
    let args = parse_args_from([
        OsString::from("tftp"),
        OsString::from("get"),
        OsString::from("--port"),
        OsString::from("1069"),
        OsString::from("localhost"),
        OsString::from("boot.bin"),
        OsString::from("download.bin"),
    ])
    .unwrap();

    assert_eq!(
        args.tftp_command,
        Some(TftpCommand::Get {
            host: "localhost".to_owned(),
            remote: "boot.bin".to_owned(),
            local: PathBuf::from("download.bin"),
            port: 1069,
        })
    );
}

#[test]
fn terminal_rendering_report_defaults_to_ten_seconds() {
    let args = parse_args_from([
        OsString::from("--profile-terminal-rendering"),
        OsString::from("--profile-report"),
        OsString::from("profile.json"),
    ])
    .unwrap();

    assert_eq!(args.profile_report, Some(PathBuf::from("profile.json")));
    assert_eq!(
        args.profile_duration,
        Some(DEFAULT_PERFORMANCE_REPORT_DURATION)
    );
}

#[test]
fn pane_stress_requires_and_records_profiler_mode() {
    let args = parse_args_from([
        OsString::from("--profile-terminal-rendering"),
        OsString::from("--profile-pane-stress"),
    ])
    .unwrap();
    assert!(args.profile_pane_stress);

    let error = parse_args_from([OsString::from("--profile-pane-stress")]).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires --profile-terminal-rendering")
    );
}

#[test]
fn terminal_rendering_report_accepts_fractional_duration() {
    let args = parse_args_from([
        OsString::from("--profile-terminal-rendering"),
        OsString::from("--profile-report"),
        OsString::from("profile.json"),
        OsString::from("--profile-duration"),
        OsString::from("2.5"),
    ])
    .unwrap();

    assert_eq!(args.profile_duration, Some(Duration::from_secs_f64(2.5)));
}

#[test]
fn terminal_rendering_report_options_require_profiler_mode() {
    let error = parse_args_from([
        OsString::from("--profile-report"),
        OsString::from("profile.json"),
    ])
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("require --profile-terminal-rendering")
    );

    let error = parse_args_from([
        OsString::from("--profile-terminal-rendering"),
        OsString::from("--profile-duration"),
        OsString::from("1"),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("requires --profile-report"));
}

#[test]
fn terminal_rendering_profiler_rejects_user_configuration() {
    let error = parse_args_from([
        OsString::from("--profile-terminal-rendering"),
        OsString::from("--config"),
        OsString::from("config.json"),
    ])
    .unwrap_err();

    assert!(error.to_string().contains("cannot be combined"));
}

#[test]
fn terminal_rendering_profiler_launches_the_current_executable() {
    let executable = Path::new(if cfg!(windows) {
        r"C:\tools\zetta.exe"
    } else {
        "/usr/local/bin/zetta"
    });
    let config = terminal_rendering_profile_config(executable);

    assert_eq!(config.profiles.len(), 1);
    assert_eq!(config.default_profile, 0);
    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec!["--terminal-render-workload".to_owned()],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn unchanged_user_themes_are_not_reloaded() {
    let themes_dir = env::temp_dir().join(format!(
        "zetta-theme-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&themes_dir).unwrap();
    let theme_path = themes_dir.join("test.json");
    fs::write(&theme_path, "one").unwrap();
    let mut cache = HashMap::new();

    assert_eq!(
        changed_theme_files(&themes_dir, &mut cache).unwrap(),
        [theme_path.clone()]
    );
    assert!(
        changed_theme_files(&themes_dir, &mut cache)
            .unwrap()
            .is_empty()
    );

    fs::write(&theme_path, "a longer theme").unwrap();
    assert_eq!(
        changed_theme_files(&themes_dir, &mut cache).unwrap(),
        [theme_path]
    );
    fs::remove_dir_all(themes_dir).unwrap();
}

#[test]
fn invalid_startup_config_falls_back_and_reports_the_error() {
    let config_path = env::temp_dir().join(format!(
        "zetta-invalid-config-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&config_path, r#"{"theme": "One Light",}"#).unwrap();

    let (config, error) = load_startup_config(Some(&config_path), None);

    fs::remove_file(&config_path).unwrap();
    assert_eq!(config.config_path, config_path);
    assert_eq!(config.default_profile, 0);
    let error = error.expect("invalid JSON should be reported");
    assert!(error.contains("Could not load configuration"));
    assert!(error.contains("parsing"));
    assert!(error.contains("line 1 column"));
}

#[test]
fn defaults_to_light_theme_without_overriding_configuration() {
    assert_eq!(selected_theme_name(None), "One Light");
    assert_eq!(selected_theme_name(Some("One Dark")), "One Dark");
}

#[test]
fn linux_desktop_entry_matches_app_id() {
    let desktop_entry = include_str!("../../resources/linux/Zetta.desktop");
    assert!(desktop_entry.contains(&format!("\nIcon={ZETTA_APP_ID}\n")));
    assert!(desktop_entry.contains(&format!("\nStartupWMClass={ZETTA_APP_ID}\n")));
}

#[test]
fn profile_shortcuts_match_the_shifted_number_row() {
    const SHIFTED_DIGITS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];
    let keyboard_mapper = gpui::DummyKeyboardMapper;
    for (index, symbol) in SHIFTED_DIGITS.into_iter().enumerate() {
        let slot = index + 1;
        let bindings = profile_keybindings(slot, &keyboard_mapper);
        let shifted = gpui::Keystroke::parse(&format!("ctrl-{symbol}")).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].match_keystrokes(&[shifted]), Some(false));
    }
}

#[test]
fn pane_template_shortcuts_are_built_in() {
    let [three_right, quarters] = pane_template_keybindings();
    let three_right_key = gpui::Keystroke::parse("alt-shift-o").unwrap();
    let quarters_key = gpui::Keystroke::parse("alt-shift-e").unwrap();

    assert_eq!(
        three_right.match_keystrokes(&[three_right_key]),
        Some(false)
    );
    assert_eq!(quarters.match_keystrokes(&[quarters_key]), Some(false));
}

#[test]
fn profile_shortcut_labels_cover_the_number_row() {
    assert_eq!(profile_shortcut_label(1).as_deref(), Some("Ctrl+Shift+1"));
    assert_eq!(profile_shortcut_label(9).as_deref(), Some("Ctrl+Shift+9"));
    assert_eq!(profile_shortcut_label(10), None);
}

#[test]
fn wsl_home_is_applied_to_detected_wsl_commands() {
    let shell = Shell::WithArguments {
        program: "C:\\Windows\\System32\\wsl.exe".to_owned(),
        args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
        title_override: Some("WSL: Ubuntu".to_owned()),
    };

    assert!(is_wsl_shell(&shell));
    assert!(matches!(
        wsl_shell_with_tracking(shell, Some("~"), None),
        Shell::WithArguments { args, title_override, .. }
            if args == ["--distribution", "Ubuntu", "--cd", "~"]
                && title_override.as_deref() == Some("WSL: Ubuntu")
    ));
}

#[test]
fn native_shells_are_not_treated_as_wsl() {
    assert!(!is_wsl_shell(&Shell::Program("pwsh.exe".to_owned())));
}

#[test]
fn explicit_wsl_directory_is_not_overridden() {
    let shell = Shell::WithArguments {
        program: "wsl.exe".to_owned(),
        args: vec!["--cd".to_owned(), "/work".to_owned()],
        title_override: None,
    };

    assert!(matches!(
        wsl_shell_with_tracking(shell, Some("~"), None),
        Shell::WithArguments { args, .. } if args == ["--cd", "/work"]
    ));
}

#[test]
fn wsl_ignores_the_windows_side_inherited_directory() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::WithArguments {
            program: "wsl.exe".to_owned(),
            args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
            title_override: None,
        },
        theme: None,
    };

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(PathBuf::from(r"C:\source\zetta")),
        None,
        Some(PathBuf::from(r"C:\Users\stefan")),
        false,
    );

    assert_eq!(directory, None);
    assert_eq!(wsl_directory.as_deref(), Some("~"));
}

#[test]
fn explicitly_configured_home_alias_still_uses_the_wsl_home() {
    let config = Config::parse(r#"{"working_directory":"~"}"#, None, None).unwrap();
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::Program("wsl.exe".to_owned()),
        theme: None,
    };

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(PathBuf::from(r"C:\source\zetta")),
        None,
        config.working_directory,
        config.working_directory_configured,
    );

    assert_eq!(directory, None);
    assert_eq!(wsl_directory.as_deref(), Some("~"));
}

#[test]
fn native_profiles_still_inherit_the_active_directory() {
    let profile = Profile {
        name: "PowerShell".to_owned(),
        command: Shell::Program("pwsh.exe".to_owned()),
        theme: None,
    };
    let inherited = PathBuf::from(r"C:\source\zetta");

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(inherited.clone()),
        None,
        Some(PathBuf::from(r"C:\Users\stefan")),
        false,
    );

    assert_eq!(directory, Some(inherited));
    assert_eq!(wsl_directory, None);
}

#[test]
fn configured_directory_overrides_the_windows_side_wsl_directory() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::Program("wsl.exe".to_owned()),
        theme: None,
    };
    let configured = PathBuf::from(r"C:\Users\stefan");

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(PathBuf::from(r"C:\source\zetta")),
        None,
        Some(configured.clone()),
        true,
    );

    assert_eq!(directory, Some(configured));
    assert_eq!(wsl_directory, None);
}

#[test]
fn tracked_wsl_directory_takes_precedence_over_the_initial_configuration() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::Program("wsl.exe".to_owned()),
        theme: None,
    };

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        None,
        Some("/work".to_owned()),
        Some(PathBuf::from(r"C:\Users\stefan")),
        true,
    );

    assert_eq!(directory, None);
    assert_eq!(wsl_directory.as_deref(), Some("/work"));
}

#[test]
fn wsl_inherits_the_tracked_linux_directory() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::Program("wsl.exe".to_owned()),
        theme: None,
    };

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(PathBuf::from(r"C:\source\zetta")),
        Some("/home/stefan/source/zetta".to_owned()),
        Some(PathBuf::from(r"C:\Users\stefan")),
        false,
    );

    assert_eq!(directory, None);
    assert_eq!(wsl_directory.as_deref(), Some("/home/stefan/source/zetta"));
}

#[test]
fn wsl_tracker_wraps_the_default_login_shell() {
    let marker = Path::new(r"C:\Users\stefan\AppData\Local\Temp\zetta-cwd");
    let shell = wsl_shell_with_tracking(
        Shell::WithArguments {
            program: "wsl.exe".to_owned(),
            args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
            title_override: None,
        },
        Some("/work"),
        Some(marker),
    );

    assert!(matches!(
        shell,
        Shell::WithArguments { args, .. }
            if args[..4] == ["--distribution", "Ubuntu", "--cd", "/work"]
                && args[4..8] == ["--exec", "/bin/sh", "-c", WSL_CWD_TRACKER]
                && args.last().map(String::as_str) == marker.to_str()
    ));
}

#[test]
fn wsl_wrapper_prefers_prompt_cwd_reports_and_keeps_a_shell_fallback() {
    assert!(WSL_CWD_TRACKER.contains("PROMPT_COMMAND="));
    assert!(WSL_CWD_TRACKER.contains("--on-event fish_prompt"));
    assert!(WSL_CWD_TRACKER.contains("add-zsh-hook precmd __zetta_report_cwd"));
    assert!(WSL_CWD_TRACKER.contains("source \"$ZDOTDIR/.zshenv\""));
    assert!(WSL_CWD_TRACKER.contains("rm -rf -- \"$ZETTA_INTEGRATION_ZDOTDIR\""));
    assert!(!WSL_CWD_TRACKER.contains("source \"$ZDOTDIR/.zshrc\""));
    assert!(WSL_CWD_TRACKER.contains("]7;file://localhost"));
    assert!(WSL_CWD_TRACKER.contains("]2;zetta-cwd:"));
    assert!(WSL_CWD_TRACKER.contains("readlink \"/proc/$parent/cwd\""));
}

#[test]
fn normalizes_hyphenated_page_key_names() {
    let keymap = r#"{"ctrl-page-up":"zetta::NextTab","ctrl-page-down":"zetta::PreviousTab"}"#;
    assert_eq!(
        normalize_keymap_key_names(keymap),
        r#"{"ctrl-pageup":"zetta::NextTab","ctrl-pagedown":"zetta::PreviousTab"}"#
    );
}

#[test]
fn tab_rename_does_not_capture_an_unmodified_function_key() {
    assert!(RENAME_TAB_KEYBINDING.contains('-'));
    assert_ne!(RENAME_TAB_KEYBINDING, "f2");
}

#[test]
fn pane_label_uses_the_documented_shortcut() {
    assert_eq!(RENAME_PANE_KEYBINDING, "alt-shift-l");
}

#[test]
fn close_pane_uses_a_distinct_shortcut_from_close_tab() {
    assert_eq!(CLOSE_PANE_KEYBINDING, "ctrl-shift-x");
    let shortcut = gpui::Keystroke::parse(CLOSE_PANE_KEYBINDING).unwrap();
    assert_eq!(
        close_pane_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn pane_output_uses_the_standard_save_shortcut() {
    assert_eq!(SAVE_PANE_OUTPUT_KEYBINDING, "ctrl-shift-s");
    let shortcut = gpui::Keystroke::parse(SAVE_PANE_OUTPUT_KEYBINDING).unwrap();
    assert_eq!(
        pane_output_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn serial_console_avoids_the_linux_unicode_input_shortcut() {
    assert_eq!(SERIAL_CONSOLE_KEYBINDING, "ctrl-shift-d");
    assert_ne!(SERIAL_CONSOLE_KEYBINDING, "ctrl-shift-u");
    let shortcut = gpui::Keystroke::parse(SERIAL_CONSOLE_KEYBINDING).unwrap();
    assert_eq!(
        serial_console_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn minimized_pane_shortcuts_are_built_in() {
    let bindings = minimized_pane_keybindings();
    for (binding, shortcut) in bindings.into_iter().zip([
        "alt-shift-down",
        "alt-shift-up",
        "alt-shift-left",
        "alt-shift-right",
    ]) {
        let shortcut = gpui::Keystroke::parse(shortcut).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    }
}
