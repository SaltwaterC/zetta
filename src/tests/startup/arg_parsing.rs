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

#[cfg(not(windows))]
#[test]
fn executable_directory_is_prepended_to_native_terminal_path() {
    let executable_directory = Path::new("/Applications/Zetta.app/Contents/MacOS");
    let inherited = std::ffi::OsStr::new("/usr/bin:/bin");
    let path = path_with_entry_first(Some(inherited), executable_directory).unwrap();
    let entries = env::split_paths(&path).collect::<Vec<_>>();

    assert_eq!(entries[0], executable_directory);
    assert_eq!(entries[1], Path::new("/usr/bin"));
    assert_eq!(entries[2], Path::new("/bin"));
    assert!(path_with_entry_first(Some(path.as_os_str()), executable_directory).is_none());
}

#[cfg(not(windows))]
#[test]
fn native_terminal_points_shell_integration_at_this_executable() {
    let executable = env::current_exe().unwrap();
    assert_eq!(
        native_terminal_environment()
            .iter()
            .find(|(name, _)| name == "ZETTA_HOST_EXECUTABLE")
            .map(|(_, value)| value.as_str()),
        Some(executable.to_string_lossy().as_ref())
    );
}

#[test]
fn ordinary_and_explicit_new_window_launches_are_handoff_eligible() {
    let plain = parse_args_from(Vec::<OsString>::new()).unwrap();
    let new_window = parse_args_from([OsString::from("--new-window")]).unwrap();
    let short_new_window = parse_args_from([OsString::from("-w")]).unwrap();
    let profile_new_window = parse_args_from([
        OsString::from("--new-window"),
        OsString::from("--profile"),
        OsString::from("System"),
    ])
    .unwrap();
    let short_profile_new_window = parse_args_from([
        OsString::from("-w"),
        OsString::from("-p"),
        OsString::from("System"),
    ])
    .unwrap();
    let reverse_profile_new_window = parse_args_from([
        OsString::from("--profile"),
        OsString::from("System"),
        OsString::from("--new-window"),
    ])
    .unwrap();
    let profile = parse_args_from([OsString::from("--profile"), OsString::from("System")]).unwrap();
    let split = parse_args_from([OsString::from("--split"), OsString::from("quarters")]).unwrap();
    let mux = parse_args_from([OsString::from("mux")]).unwrap();
    let no_mux = parse_args_from([OsString::from("--no-mux")]).unwrap();
    let short_no_mux = parse_args_from([OsString::from("-n")]).unwrap();

    assert!(should_handoff_to_existing_process(&plain));
    assert_eq!(new_window, short_new_window);
    assert_eq!(new_window.mode, StartupMode::NewWindow);
    assert!(should_handoff_to_existing_process(&new_window));
    assert_eq!(profile_new_window, short_profile_new_window);
    assert_eq!(profile_new_window, reverse_profile_new_window);
    assert_eq!(profile_new_window.mode, StartupMode::NewWindow);
    assert_eq!(profile_new_window.profile.as_deref(), Some("System"));
    assert!(should_handoff_to_existing_process(&profile_new_window));
    assert!(!should_handoff_to_existing_process(&profile));
    assert!(!should_handoff_to_existing_process(&split));
    assert!(!should_handoff_to_existing_process(&mux));
    assert!(no_mux.no_mux);
    assert_eq!(no_mux, short_no_mux);
    assert!(!should_handoff_to_existing_process(&no_mux));
    assert!(parse_args_from([OsString::from("--no-mux"), OsString::from("--no-mux")]).is_err());
    assert!(parse_args_from([OsString::from("--no-mux"), OsString::from("sessions")]).is_err());
}

#[test]
fn profile_action_generation_preserves_normal_application_handoff() {
    let args = parse_args_from([
        OsString::from("--zetta-profile-actions-generation"),
        OsString::from("123"),
    ])
    .unwrap();

    assert_eq!(args.mode, StartupMode::Application);
    assert!(should_handoff_to_existing_process(&args));
}

#[test]
fn explicit_new_window_rejects_unrelated_options() {
    for arguments in [
        vec!["--config", "config.json"],
        vec!["--keymap", "keymap.json"],
        vec!["--split", "quarters"],
        vec!["--replace-pane"],
        vec!["--theme", "Dracula"],
        vec!["--no-mux"],
        vec!["--command", "sh"],
    ] {
        let mut combined = vec![OsString::from("--new-window")];
        combined.extend(arguments.into_iter().map(OsString::from));
        assert!(
            parse_args_from(combined).is_err(),
            "accepted invalid --new-window combination"
        );
    }

    assert!(
        parse_args_from([
            OsString::from("--new-window"),
            OsString::from("--new-window")
        ])
        .is_err()
    );
    let profile_new_window = parse_args_from([
        OsString::from("-w"),
        OsString::from("--profile"),
        OsString::from("System"),
    ])
    .unwrap();
    assert_eq!(profile_new_window.mode, StartupMode::NewWindow);
    assert_eq!(profile_new_window.profile.as_deref(), Some("System"));
}

#[test]
fn command_launch_consumes_the_remaining_arguments() {
    let long = parse_args_from([
        OsString::from("--profile"),
        OsString::from("System"),
        OsString::from("--command"),
        OsString::from("python"),
        OsString::from("-c"),
        OsString::from("print('hello')"),
        OsString::from("--help"),
    ])
    .unwrap();
    let short = parse_args_from([
        OsString::from("-e"),
        OsString::from("python"),
        OsString::from("-c"),
        OsString::from("print('hello')"),
        OsString::from("--help"),
    ])
    .unwrap();

    assert_eq!(
        long.mode,
        StartupMode::Command(vec![
            "python".to_owned(),
            "-c".to_owned(),
            "print('hello')".to_owned(),
            "--help".to_owned(),
        ])
    );
    assert_eq!(short.profile, None);
    assert!(should_handoff_to_existing_process(&short));
    assert!(parse_args_from([OsString::from("-e")]).is_err());
}

#[test]
fn root_split_option_accepts_configured_names_and_combines_with_profile() {
    for name in [
        "quarters",
        "four-vertical",
        "three-left",
        "three-right",
        "custom-layout",
    ] {
        let long = parse_args_from([OsString::from("--split"), OsString::from(name)]).unwrap();
        assert_eq!(long.split.as_deref(), Some(name));
        assert_eq!(long.mode, StartupMode::Application);

        let short = parse_args_from([OsString::from("-s"), OsString::from(name)]).unwrap();
        assert_eq!(short, long);

        let profile_before = parse_args_from([
            OsString::from("--profile"),
            OsString::from("System"),
            OsString::from("--split"),
            OsString::from(name),
        ])
        .unwrap();
        let profile_after = parse_args_from([
            OsString::from("--split"),
            OsString::from(name),
            OsString::from("--profile"),
            OsString::from("System"),
        ])
        .unwrap();
        assert_eq!(profile_before, profile_after);
        assert_eq!(profile_before.profile.as_deref(), Some("System"));
        assert_eq!(profile_before.split.as_deref(), Some(name));
        assert!(!should_handoff_to_existing_process(&profile_before));
    }

    assert!(parse_args_from([OsString::from("--split")]).is_err());
    assert!(parse_args_from([OsString::from("--split"), OsString::from("")]).is_err());
    assert!(parse_args_from([OsString::from("--split"), OsString::from("--profile")]).is_err());

    let config = Config::defaults(None, None);
    for invalid in ["Quarters", "two", "three-right "] {
        assert!(validate_launch_split(&config, Some(invalid)).is_err());
    }
    assert!(validate_launch_split(&config, Some("quarters")).is_ok());
}

#[test]
fn replace_pane_accepts_long_and_short_forms_in_any_option_order() {
    let long = parse_args_from([
        OsString::from("--replace-pane"),
        OsString::from("--split"),
        OsString::from("quarters"),
        OsString::from("--profile"),
        OsString::from("System"),
        OsString::from("--theme"),
        OsString::from("Dracula"),
    ])
    .unwrap();
    let short = parse_args_from([
        OsString::from("-p"),
        OsString::from("System"),
        OsString::from("-t"),
        OsString::from("Dracula"),
        OsString::from("-s"),
        OsString::from("quarters"),
        OsString::from("-r"),
    ])
    .unwrap();

    assert_eq!(short, long);
    assert!(long.replace_pane);
    assert!(should_replace_pane_in_existing_process(&long));
    assert!(!should_handoff_to_existing_process(&long));

    let profile_only = parse_args_from([
        OsString::from("--profile"),
        OsString::from("System"),
        OsString::from("--replace-pane"),
    ])
    .unwrap();
    assert_eq!(profile_only.split, None);
    assert_eq!(profile_only.profile.as_deref(), Some("System"));
    assert!(should_replace_pane_in_existing_process(&profile_only));
}

#[test]
fn replace_pane_requires_a_split_or_profile_and_preserves_launch_fallback_options() {
    assert!(parse_args_from([OsString::from("--replace-pane")]).is_err());
    assert!(
        parse_args_from([
            OsString::from("--replace-pane"),
            OsString::from("--theme"),
            OsString::from("Dracula"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("--replace-pane"),
            OsString::from("--split"),
            OsString::from("quarters"),
            OsString::from("--replace-pane"),
        ])
        .is_err()
    );

    let with_config = parse_args_from([
        OsString::from("--replace-pane"),
        OsString::from("--profile"),
        OsString::from("System"),
        OsString::from("--config"),
        OsString::from("config.json"),
    ])
    .unwrap();
    assert!(!should_replace_pane_in_existing_process(&with_config));
    assert!(!should_handoff_to_existing_process(&with_config));

    let with_keymap = parse_args_from([
        OsString::from("-r"),
        OsString::from("-s"),
        OsString::from("quarters"),
        OsString::from("-k"),
        OsString::from("keymap.json"),
    ])
    .unwrap();
    assert!(!should_replace_pane_in_existing_process(&with_keymap));
    assert!(!should_handoff_to_existing_process(&with_keymap));
}

#[test]
fn profile_commands_are_typed_and_accept_config_paths_in_each_position() {
    let root_form = parse_args_from([
        OsString::from("-c"),
        OsString::from("profiles.json"),
        OsString::from("profile"),
        OsString::from("list"),
    ])
    .unwrap();
    let after_profile = parse_args_from([
        OsString::from("profile"),
        OsString::from("list"),
        OsString::from("--config"),
        OsString::from("profiles.json"),
    ])
    .unwrap();
    assert_eq!(root_form, after_profile);
    assert_eq!(root_form.config_path, Some(PathBuf::from("profiles.json")));
    assert_eq!(root_form.mode, StartupMode::Profile(ProfileCommand::List));

    let add = parse_args_from([
        OsString::from("profile"),
        OsString::from("add"),
        OsString::from("Dev Shell"),
        OsString::from("--program"),
        OsString::from("bash"),
        OsString::from("--arg"),
        OsString::from("-l"),
        OsString::from("-a"),
        OsString::from("one two"),
    ])
    .unwrap();
    assert_eq!(
        add.mode,
        StartupMode::Profile(ProfileCommand::Add {
            name: "Dev Shell".to_owned(),
            program: "bash".to_owned(),
            args: vec!["-l".to_owned(), "one two".to_owned()],
            theme: None,
            dark_theme: None,
            icon: None,
        })
    );
}

#[test]
fn launch_profile_selects_an_available_profile_without_changing_the_configured_default() {
    let mut config = Config::defaults(None, None);
    config.profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::Program("wsl.exe".to_owned()),
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Bash,
        },
    ];

    config.hidden_profiles.insert("wsl: ubuntu".to_owned());

    let profile = select_launch_profile(&config, Some("wsl: ubuntu"))
        .unwrap()
        .unwrap();
    assert_eq!(profile.name, "WSL: Ubuntu");
    assert_eq!(profile.theme, None);
    assert_eq!(config.default_profile, 0);

    let error = select_launch_profile(&config, Some("Missing")).unwrap_err();
    assert!(error.to_string().contains("is not available"));
    assert!(
        error
            .to_string()
            .contains("available profiles: System, WSL: Ubuntu")
    );
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
