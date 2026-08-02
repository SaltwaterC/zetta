use super::*;
#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server",
    feature = "notifications"
))]
use crate::cli_services::CliServiceCommand;
#[cfg(feature = "serial-console")]
use crate::cli_services::SerialCommand;

#[cfg(windows)]
fn msys2_shell(root: &Path, shell: &str) -> Shell {
    Shell::WithArguments {
        program: "cmd.exe".to_owned(),
        args: vec![
            "/d".to_owned(),
            "/s".to_owned(),
            "/c".to_owned(),
            format!(
                "\"\"{}\" -defterm -here -no-start -msys -use-full-path -shell {shell}\"",
                root.join("msys2_shell.cmd").display()
            ),
        ],
        title_override: None,
    }
}

#[cfg(windows)]
#[test]
fn recognizes_detected_msys2_profiles_and_their_custom_root() {
    let root = Path::new(r"D:\Applications with spaces\MSYS2");

    assert_eq!(
        msys2_profile(&msys2_shell(root, "bash")),
        Some((root.to_path_buf(), Msys2Shell::Bash))
    );
    assert_eq!(
        msys2_profile(&msys2_shell(root, "zsh")),
        Some((root.to_path_buf(), Msys2Shell::Zsh))
    );
}

#[cfg(windows)]
#[test]
fn converts_reported_msys2_directories_to_native_windows_paths() {
    let root = Path::new(r"D:\Applications\MSYS2");

    assert_eq!(
        msys2_path_to_windows(root, "/c/Users/saltw/source/zetta"),
        Some(PathBuf::from(r"C:\Users\saltw\source\zetta"))
    );
    assert_eq!(
        msys2_path_to_windows(root, "/home/saltw/project"),
        Some(root.join("home").join("saltw").join("project"))
    );
    assert_eq!(
        msys2_path_to_windows(root, "//server/share/project"),
        Some(PathBuf::from(r"\\server\share\project"))
    );
    assert_eq!(msys2_path_to_windows(root, "/c/../Windows"), None);
    assert_eq!(msys2_path_to_windows(root, "relative/path"), None);
}

#[cfg(windows)]
#[test]
fn configures_bash_to_report_prompt_directories_and_foreground_commands() {
    let environment = msys2_cwd_tracking_environment(
        &msys2_shell(Path::new(r"C:\msys64"), "bash"),
        7,
        Path::new(r"C:\Temp"),
    )
    .unwrap();

    assert_eq!(environment.len(), 1);
    assert_eq!(environment[0].0, "PROMPT_COMMAND");
    assert!(environment[0].1.contains("zetta-cwd:%s"));
    assert!(environment[0].1.contains("\"$PWD\""));
    assert!(environment[0].1.contains("trap '__zetta_preexec' DEBUG"));
    assert!(environment[0].1.contains("__zetta_at_prompt=0"));
    assert!(environment[0].1.contains("__zetta_at_prompt=1"));
    assert!(environment[0].1.contains("zetta-cmd:%s"));
    assert!(environment[0].1.contains("zetta-cmd:bash"));
}

#[cfg(windows)]
#[test]
fn configures_zsh_to_report_directories_and_commands_without_changing_user_files() {
    let temporary = tempfile::tempdir().unwrap();
    let environment = msys2_cwd_tracking_environment(
        &msys2_shell(Path::new(r"C:\msys64"), "zsh"),
        11,
        temporary.path(),
    )
    .unwrap();
    let integration_directory = environment
        .iter()
        .find_map(|(name, value)| (name == "ZDOTDIR").then_some(value))
        .unwrap();
    let native_directory =
        msys2_path_to_windows(Path::new(r"C:\msys64"), integration_directory).unwrap();
    let integration = fs::read_to_string(native_directory.join(".zshenv")).unwrap();

    assert!(integration.contains("add-zsh-hook precmd __zetta_report_cwd"));
    assert!(integration.contains("add-zsh-hook preexec __zetta_report_preexec"));
    assert!(integration.contains("zetta-cwd:%s"));
    assert!(integration.contains("zetta-cmd:%s"));
    assert!(integration.contains("zetta-cmd:zsh"));
    assert!(integration.contains("source \"$original_zdotdir/.zshenv\""));
}

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
fn help_text_uses_title_case_and_lists_built_in_features() {
    let profiles = [
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
        },
        Profile {
            name: "Operations".to_owned(),
            command: Shell::Program("zsh".to_owned()),
            theme: None,
        },
    ];
    let help = help_text(&profiles);
    assert!(help.starts_with("Zetta Terminal\n"));
    assert!(help.contains("Built-in features:\n  Terminal emulator"));
    assert!(help.contains("Profiles accepted by --profile NAME (case-insensitive):"));
    assert!(help.contains("  System\n  Operations"));
    assert!(help.contains("Select one of the profiles listed above"));
    assert!(help.contains("zetta terminal-size [--json | --resize"));
    assert!(
        help.contains(
            "terminal-size                       Print or resize the current terminal pane"
        )
    );
    assert!(help.contains("zetta init [SHELL]"));
    assert!(
        help.contains(
            "init                                Configure or generate shell integration"
        )
    );

    #[cfg(feature = "wayland")]
    assert!(help.contains("Wayland backend"));
    #[cfg(not(feature = "wayland"))]
    assert!(!help.contains("Wayland backend"));

    #[cfg(feature = "x11")]
    assert!(help.contains("X11 backend"));
    #[cfg(not(feature = "x11"))]
    assert!(!help.contains("X11 backend"));

    #[cfg(feature = "serial-console")]
    {
        assert!(help.contains("Serial console"));
        assert!(help.contains("zetta serial <COMMAND>"));
        assert!(
            help.contains("serial                              List or connect to serial devices")
        );
    }
    #[cfg(not(feature = "serial-console"))]
    assert!(!help.contains("Serial console"));

    #[cfg(feature = "http-server")]
    {
        assert!(help.contains("HTTP server"));
        assert!(help.contains("zetta http server [OPTIONS]"));
        assert!(help.contains("http server                         Serve static files over HTTP"));
    }
    #[cfg(not(feature = "http-server"))]
    assert!(!help.contains("HTTP server"));

    #[cfg(feature = "tftp-server")]
    {
        assert!(help.contains("TFTP server"));
        assert!(help.contains("zetta tftp <COMMAND>"));
    }
    #[cfg(not(feature = "tftp-server"))]
    assert!(!help.contains("TFTP server"));

    #[cfg(any(feature = "tftp-client", feature = "tftp-server"))]
    {
        #[cfg(feature = "tftp-client")]
        assert!(help.contains("TFTP client"));
        assert!(help.contains("zetta tftp <COMMAND>"));
    }
    #[cfg(not(any(feature = "tftp-client", feature = "tftp-server")))]
    {
        assert!(!help.contains("TFTP client"));
        assert!(!help.contains("zetta tftp <COMMAND>"));
    }

    #[cfg(feature = "notifications")]
    {
        assert!(help.contains("Desktop notifications"));
        assert!(help.contains("zetta notify [OPTIONS] SUMMARY [BODY]"));
        assert!(help.contains("notify                              Show a desktop notification"));
    }
    #[cfg(not(feature = "notifications"))]
    {
        assert!(!help.contains("Desktop notifications"));
        assert!(!help.contains("zetta notify"));
    }
}

#[cfg(feature = "serial-console")]
#[test]
fn serial_subcommands_bypass_application_startup() {
    let args = parse_args_from([OsString::from("serial"), OsString::from("list")]).unwrap();

    assert!(matches!(
        args.mode,
        StartupMode::CliService(CliServiceCommand::Serial(SerialCommand::List))
    ));
    assert!(!should_handoff_to_existing_process(&args));
}

#[cfg(feature = "http-server")]
#[test]
fn http_server_subcommand_bypasses_application_startup() {
    let args = parse_args_from([
        OsString::from("http"),
        OsString::from("server"),
        OsString::from("--port"),
        OsString::from("8080"),
    ])
    .unwrap();

    assert!(matches!(
        args.mode,
        StartupMode::CliService(CliServiceCommand::Http(_))
    ));
    assert!(!should_handoff_to_existing_process(&args));
}

#[cfg(feature = "notifications")]
#[test]
fn notify_subcommand_bypasses_application_startup() {
    let args = parse_args_from([
        OsString::from("notify"),
        OsString::from("--app-name"),
        OsString::from("zetta"),
        OsString::from("Build finished"),
    ])
    .unwrap();

    assert!(matches!(
        args.mode,
        StartupMode::CliService(CliServiceCommand::Notify(_))
    ));
    assert!(!should_handoff_to_existing_process(&args));

    assert!(parse_args_from([OsString::from("notify")]).is_err());
}

#[cfg(feature = "clipboard")]
#[test]
fn copy_and_paste_subcommands_bypass_application_startup() {
    let copy = parse_args_from([OsString::from("copy")]).unwrap();
    assert!(matches!(
        copy.mode,
        StartupMode::CliService(CliServiceCommand::Copy(_))
    ));
    assert!(!should_handoff_to_existing_process(&copy));

    let paste = parse_args_from([OsString::from("paste")]).unwrap();
    assert!(matches!(
        paste.mode,
        StartupMode::CliService(CliServiceCommand::Paste(_))
    ));
    assert!(!should_handoff_to_existing_process(&paste));

    assert!(parse_args_from([OsString::from("copy"), OsString::from("--unknown")]).is_err());
    assert!(parse_args_from([OsString::from("paste"), OsString::from("--unknown")]).is_err());
}

#[cfg(feature = "tftp-server")]
#[test]
fn tftp_server_subcommand_bypasses_application_startup() {
    let args = parse_args_from([
        OsString::from("tftp"),
        OsString::from("server"),
        OsString::from("--port"),
        OsString::from("1069"),
    ])
    .unwrap();

    assert!(matches!(
        args.mode,
        StartupMode::CliService(CliServiceCommand::Tftp(_))
    ));
    assert!(!should_handoff_to_existing_process(&args));
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
    let short_json = parse_args_from([OsString::from("sessions"), OsString::from("-j")]).unwrap();
    assert_eq!(short_json, json);
    assert!(parse_args_from([OsString::from("sessions"), OsString::from("--unknown")]).is_err());
}

#[test]
fn terminal_size_subcommand_bypasses_application_startup() {
    let args = parse_args_from([OsString::from("terminal-size")]).unwrap();

    assert_eq!(
        args.mode,
        StartupMode::PrintTerminalSize {
            json: false,
            resize: None,
        }
    );
    assert!(!should_handoff_to_existing_process(&args));
    let json =
        parse_args_from([OsString::from("terminal-size"), OsString::from("--json")]).unwrap();
    let short_json =
        parse_args_from([OsString::from("terminal-size"), OsString::from("-j")]).unwrap();
    assert_eq!(
        json.mode,
        StartupMode::PrintTerminalSize {
            json: true,
            resize: None,
        }
    );
    assert_eq!(short_json, json);
    assert!(
        parse_args_from([OsString::from("terminal-size"), OsString::from("--unknown")]).is_err()
    );
}

#[test]
fn terminal_size_resize_accepts_each_dimension_independently() {
    let columns = parse_args_from([
        OsString::from("terminal-size"),
        OsString::from("--resize"),
        OsString::from("--columns"),
        OsString::from("120"),
    ])
    .unwrap();
    assert_eq!(
        columns.mode,
        StartupMode::PrintTerminalSize {
            json: false,
            resize: Some(TerminalResize {
                columns: Some(120),
                rows: None,
            }),
        }
    );
    let short_columns = parse_args_from([
        OsString::from("terminal-size"),
        OsString::from("-r"),
        OsString::from("-c"),
        OsString::from("120"),
    ])
    .unwrap();
    assert_eq!(short_columns, columns);

    let rows = parse_args_from([
        OsString::from("terminal-size"),
        OsString::from("--resize"),
        OsString::from("--rows"),
        OsString::from("40"),
    ])
    .unwrap();
    assert_eq!(
        rows.mode,
        StartupMode::PrintTerminalSize {
            json: false,
            resize: Some(TerminalResize {
                columns: None,
                rows: Some(40),
            }),
        }
    );
    let short_rows = parse_args_from([
        OsString::from("terminal-size"),
        OsString::from("-r"),
        OsString::from("-R"),
        OsString::from("40"),
    ])
    .unwrap();
    assert_eq!(short_rows, rows);

    let dimensions = parse_args_from([
        OsString::from("terminal-size"),
        OsString::from("--resize"),
        OsString::from("--columns"),
        OsString::from("120"),
        OsString::from("--rows"),
        OsString::from("40"),
    ])
    .unwrap();
    assert_eq!(
        dimensions.mode,
        StartupMode::PrintTerminalSize {
            json: false,
            resize: Some(TerminalResize {
                columns: Some(120),
                rows: Some(40),
            }),
        }
    );
    let short_dimensions = parse_args_from([
        OsString::from("terminal-size"),
        OsString::from("-r"),
        OsString::from("-c"),
        OsString::from("120"),
        OsString::from("-R"),
        OsString::from("40"),
    ])
    .unwrap();
    assert_eq!(short_dimensions, dimensions);

    assert!(
        parse_args_from([
            OsString::from("terminal-size"),
            OsString::from("--columns"),
            OsString::from("120"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("terminal-size"),
            OsString::from("--resize"),
            OsString::from("--rows"),
            OsString::from("0"),
        ])
        .is_err()
    );
}

#[test]
fn init_subcommand_configures_the_current_shell_or_prints_an_explicit_integration() {
    let configured = parse_args_from([OsString::from("init")]).unwrap();
    assert_eq!(
        configured.mode,
        StartupMode::ConfigureCurrentShellIntegration
    );
    assert!(!should_handoff_to_existing_process(&configured));

    let args = parse_args_from([OsString::from("init"), OsString::from("zsh")]).unwrap();

    assert_eq!(
        args.mode,
        StartupMode::PrintShellIntegration(ShellIntegration::Zsh)
    );
    assert!(!should_handoff_to_existing_process(&args));
    assert!(parse_args_from([OsString::from("init"), OsString::from("sh")]).is_err());
}

#[test]
fn shell_integration_setup_message_explains_how_to_enable_a_new_configuration() {
    let message = shell_integration_configuration_message(&ShellIntegrationConfiguration::Written(
        PathBuf::from("/home/example/.zshrc"),
    ));

    assert!(message.contains("Start a new shell or reload this file"));
}

#[test]
fn output_benchmark_subcommand_bypasses_application_startup() {
    let args = parse_args_from([OsString::from("benchmark-output")]).unwrap();

    assert_eq!(
        args.mode,
        StartupMode::OutputBenchmark {
            size_mib: DEFAULT_OUTPUT_BENCHMARK_MIB,
            output_type: OutputBenchmarkType::RepeatedLines,
        }
    );
    assert!(!should_handoff_to_existing_process(&args));

    let sized = parse_args_from([
        OsString::from("benchmark-output"),
        OsString::from("--size"),
        OsString::from("64"),
    ])
    .unwrap();
    assert_eq!(
        sized.mode,
        StartupMode::OutputBenchmark {
            size_mib: 64,
            output_type: OutputBenchmarkType::RepeatedLines,
        }
    );

    let short_sized = parse_args_from([
        OsString::from("benchmark-output"),
        OsString::from("-s"),
        OsString::from("32"),
    ])
    .unwrap();
    assert_eq!(
        short_sized.mode,
        StartupMode::OutputBenchmark {
            size_mib: 32,
            output_type: OutputBenchmarkType::RepeatedLines,
        }
    );

    let unique = parse_args_from([
        OsString::from("benchmark-output"),
        OsString::from("--output-type"),
        OsString::from("unique"),
    ])
    .unwrap();
    assert_eq!(
        unique.mode,
        StartupMode::OutputBenchmark {
            size_mib: DEFAULT_OUTPUT_BENCHMARK_MIB,
            output_type: OutputBenchmarkType::UniqueLines,
        }
    );

    let short_unique = parse_args_from([
        OsString::from("benchmark-output"),
        OsString::from("-t"),
        OsString::from("unique"),
    ])
    .unwrap();
    assert_eq!(
        short_unique.mode,
        StartupMode::OutputBenchmark {
            size_mib: DEFAULT_OUTPUT_BENCHMARK_MIB,
            output_type: OutputBenchmarkType::UniqueLines,
        }
    );

    for invalid in ["0", "1.5", "not-a-number"] {
        assert!(
            parse_args_from([
                OsString::from("benchmark-output"),
                OsString::from("--size"),
                OsString::from(invalid),
            ])
            .is_err()
        );
    }
    assert!(
        parse_args_from([OsString::from("benchmark-output"), OsString::from("--size"),]).is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("benchmark-output"),
            OsString::from("--unknown"),
        ])
        .is_err()
    );
    for invalid in ["different", ""] {
        assert!(
            parse_args_from([
                OsString::from("benchmark-output"),
                OsString::from("--output-type"),
                OsString::from(invalid),
            ])
            .is_err()
        );
    }
    assert!(
        parse_args_from([
            OsString::from("benchmark-output"),
            OsString::from("--output-type"),
        ])
        .is_err()
    );
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
fn benchmark_subcommand_arguments_are_cross_platform() {
    assert_eq!(
        parse_args_from([OsString::from("benchmark")]).unwrap(),
        StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            mode: StartupMode::TerminalRenderingProfile,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_background_stress: false,
            profile_sparse_updates: false,
            profile_external_terminal: false,
            tftp_command: None,
        }
    );
    assert_eq!(
        parse_args_from([
            OsString::from("benchmark"),
            OsString::from("--terminal-render-workload"),
        ])
        .unwrap(),
        StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            mode: StartupMode::TerminalRenderingWorkload,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_background_stress: false,
            profile_sparse_updates: false,
            profile_external_terminal: false,
            tftp_command: None,
        }
    );
    assert_eq!(
        parse_args_from([
            OsString::from("benchmark"),
            OsString::from("--terminal-checkerboard-workload"),
        ])
        .unwrap()
        .mode,
        StartupMode::TerminalCheckerboardWorkload
    );
}

#[test]
fn shorthand_options_match_long_options() {
    let shorthand = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("-s"),
        OsString::from("-b"),
        OsString::from("-r"),
        OsString::from("profile.json"),
        OsString::from("-d"),
        OsString::from("2.5"),
    ])
    .unwrap();
    let longhand = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-pane-stress"),
        OsString::from("--profile-background-stress"),
        OsString::from("--profile-report"),
        OsString::from("profile.json"),
        OsString::from("--profile-duration"),
        OsString::from("2.5"),
    ])
    .unwrap();
    assert_eq!(shorthand, longhand);

    let shorthand = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("-u"),
        OsString::from("-x"),
        OsString::from("-d"),
        OsString::from("2.5"),
    ])
    .unwrap();
    let longhand = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-sparse-updates"),
        OsString::from("--profile-external-terminal"),
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
fn launch_profile_selects_an_available_profile_without_changing_the_configured_default() {
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

    let profile = select_launch_profile(&config, Some("wsl: ubuntu"))
        .unwrap()
        .unwrap();
    assert_eq!(profile.name, "WSL: Ubuntu");
    assert_eq!(config.default_profile, 0);

    let error = select_launch_profile(&config, Some("Missing")).unwrap_err();
    assert!(error.to_string().contains("is not available"));
    assert!(
        error
            .to_string()
            .contains("available profiles: System, WSL: Ubuntu")
    );
}

#[cfg(feature = "tftp-client")]
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
        OsString::from("benchmark"),
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
fn pane_stress_is_a_benchmark_option() {
    let args = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-pane-stress"),
    ])
    .unwrap();
    assert!(args.profile_pane_stress);

    assert!(parse_args_from([OsString::from("--profile-pane-stress")]).is_err());
}

#[test]
fn background_stress_is_a_benchmark_option() {
    let args = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-background-stress"),
    ])
    .unwrap();
    assert!(args.profile_background_stress);

    assert!(parse_args_from([OsString::from("--profile-background-stress")]).is_err());
}

#[test]
fn sparse_updates_are_a_benchmark_option() {
    let args = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-sparse-updates"),
    ])
    .unwrap();
    assert!(args.profile_sparse_updates);

    assert!(parse_args_from([OsString::from("--profile-sparse-updates")]).is_err());

    let error = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-background-stress"),
        OsString::from("--profile-sparse-updates"),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("cannot be combined"));
}

#[test]
fn external_terminal_mode_requires_a_bounded_compatible_workload() {
    let args = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-external-terminal"),
        OsString::from("--profile-duration"),
        OsString::from("2.5"),
    ])
    .unwrap();
    assert!(args.profile_external_terminal);
    assert_eq!(args.profile_duration, Some(Duration::from_secs_f64(2.5)));

    assert!(
        parse_args_from([
            OsString::from("--profile-external-terminal"),
            OsString::from("--profile-duration"),
            OsString::from("1"),
        ])
        .is_err()
    );

    let error = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-external-terminal"),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("requires --profile-duration"));

    let error = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-external-terminal"),
        OsString::from("--profile-duration"),
        OsString::from("1"),
        OsString::from("--profile-report"),
        OsString::from("profile.json"),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("cannot be combined"));

    let error = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-external-terminal"),
        OsString::from("--profile-duration"),
        OsString::from("1"),
        OsString::from("--profile-pane-stress"),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("cannot be combined"));
}

#[test]
fn terminal_rendering_report_accepts_fractional_duration() {
    let args = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-report"),
        OsString::from("profile.json"),
        OsString::from("--profile-duration"),
        OsString::from("2.5"),
    ])
    .unwrap();

    assert_eq!(args.profile_duration, Some(Duration::from_secs_f64(2.5)));
}

#[test]
fn terminal_rendering_report_options_require_a_benchmark_subcommand() {
    assert!(
        parse_args_from([
            OsString::from("--profile-report"),
            OsString::from("profile.json"),
        ])
        .is_err()
    );

    let error = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-duration"),
        OsString::from("1"),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("requires --profile-report"));
}

#[test]
fn benchmark_subcommand_rejects_application_options() {
    let error = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--config"),
        OsString::from("config.json"),
    ])
    .unwrap_err();

    assert!(error.to_string().contains("unknown benchmark argument"));
}

#[test]
fn terminal_rendering_profiler_launches_the_current_executable() {
    let executable = Path::new(if cfg!(windows) {
        r"C:\tools\zetta.exe"
    } else {
        "/usr/local/bin/zetta"
    });
    let config = terminal_rendering_profile_config(executable, PerformanceWorkload::Standard);

    assert_eq!(config.profiles.len(), 1);
    assert_eq!(config.default_profile, 0);
    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-render-workload".to_owned(),
            ],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn checkerboard_profiler_launches_the_background_workload() {
    let executable = Path::new("/path/to/zetta");
    let config =
        terminal_rendering_profile_config(executable, PerformanceWorkload::CheckerboardBackground);

    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-checkerboard-workload".to_owned(),
            ],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn checkerboard_background_changes_every_cell_on_each_frame() {
    assert_ne!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 0, 1)
    );
    assert_ne!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 1, 0)
    );
    assert_eq!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 0, 2)
    );
}

#[test]
fn sparse_update_profiler_launches_the_sparse_workload() {
    let executable = Path::new("/path/to/zetta");
    let config = terminal_rendering_profile_config(executable, PerformanceWorkload::SparseUpdates);

    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-sparse-update-workload".to_owned(),
            ],
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
fn profile_shortcut_labels_use_number_row_aliases() {
    let keyboard_mapper = gpui::DummyKeyboardMapper;
    let slot_one = profile_keybindings(1, &keyboard_mapper)[0].clone();
    let slot_nine = profile_keybindings(9, &keyboard_mapper)[0].clone();
    let remapped = KeyBinding::new("alt-p", OpenProfile { slot: 1 }, Some("Zetta > Terminal"));

    assert_eq!(
        profile_shortcut_label(1, &slot_one, &slot_one).as_deref(),
        Some("Ctrl+Shift+1")
    );
    assert_eq!(
        profile_shortcut_label(9, &slot_nine, &slot_nine).as_deref(),
        Some("Ctrl+Shift+9")
    );
    assert_eq!(profile_shortcut_label(1, &remapped, &slot_one), None);
}

#[test]
fn profile_shortcut_labels_survive_keyboard_layout_mapping() {
    // On the British layout, GPUI's mapper turns the shifted `#` source into
    // `£` before it reaches the menu renderer.
    let expected = KeyBinding::new("ctrl-£", OpenProfile { slot: 3 }, Some("Zetta > Terminal"));
    let mapped = KeyBinding::new("ctrl-£", OpenProfile { slot: 3 }, Some("Zetta > Terminal"));
    let raw = KeyBinding::new("ctrl-#", OpenProfile { slot: 3 }, Some("Zetta > Terminal"));

    assert_eq!(
        profile_shortcut_label(3, &mapped, &expected).as_deref(),
        Some("Ctrl+Shift+3")
    );
    assert_eq!(profile_shortcut_label(3, &raw, &expected), None);
}

#[test]
fn pane_template_shortcuts_are_built_in() {
    let [three_right, quarters] = pane_template_keybindings();
    let three_right_key = gpui::Keystroke::parse(&platform_keystroke("alt-shift-o")).unwrap();
    let quarters_key = gpui::Keystroke::parse(&platform_keystroke("alt-shift-e")).unwrap();

    assert_eq!(
        three_right.match_keystrokes(&[three_right_key]),
        Some(false)
    );
    assert_eq!(quarters.match_keystrokes(&[quarters_key]), Some(false));
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

    // Windows-side process inspection can't see into the WSL VM's own process
    // namespace, so the running command is reported explicitly by each shell's
    // preexec-equivalent hook via the `zetta-cmd:` title marker.
    assert!(WSL_CWD_TRACKER.contains("trap '__zetta_preexec' DEBUG"));
    assert!(WSL_CWD_TRACKER.contains("--on-event fish_preexec"));
    assert!(WSL_CWD_TRACKER.contains("add-zsh-hook preexec __zetta_report_preexec"));
    assert!(WSL_CWD_TRACKER.contains("]2;zetta-cmd:"));
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
fn tab_rename_and_configuration_reload_shortcuts_are_swapped() {
    assert_eq!(RENAME_TAB_KEYBINDING, "ctrl-shift-r");
    assert_eq!(
        RELOAD_CONFIGURATION_KEYBINDING,
        platform_keystroke("ctrl-alt-r")
    );
    assert_ne!(RENAME_TAB_KEYBINDING, RELOAD_CONFIGURATION_KEYBINDING);
}

#[test]
fn pane_label_uses_the_documented_shortcut() {
    assert_eq!(RENAME_PANE_KEYBINDING, platform_keystroke("alt-shift-r"));
}

#[test]
fn pane_controls_use_the_requested_shortcuts() {
    assert_eq!(
        TOGGLE_PANE_CONTROLS_KEYBINDING,
        platform_keystroke("alt-shift-h")
    );
    assert_eq!(TOGGLE_TAB_PANE_CONTROLS_KEYBINDING, "ctrl-shift-h");
    assert_ne!(
        TOGGLE_PANE_CONTROLS_KEYBINDING,
        TOGGLE_TAB_PANE_CONTROLS_KEYBINDING
    );
}

#[test]
fn pane_layout_rotation_uses_the_requested_shortcut() {
    assert_eq!(
        ROTATE_PANE_LAYOUT_KEYBINDING,
        platform_keystroke("alt-shift-l")
    );
    let shortcut = gpui::Keystroke::parse(ROTATE_PANE_LAYOUT_KEYBINDING).unwrap();
    assert_eq!(
        rotate_pane_layout_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn pane_resize_mode_uses_a_dedicated_ctrl_shift_shortcut() {
    assert_eq!(TOGGLE_PANE_RESIZE_MODE_KEYBINDING, "ctrl-shift-j");
    let shortcut = gpui::Keystroke::parse(TOGGLE_PANE_RESIZE_MODE_KEYBINDING).unwrap();
    assert_eq!(
        pane_resize_mode_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
    for (binding, shortcut) in pane_resize_keybindings()
        .into_iter()
        .zip(["left", "right", "up", "down"])
    {
        let shortcut = gpui::Keystroke::parse(shortcut).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
        assert!(
            binding
                .predicate()
                .expect("pane resize shortcut should be scoped to a terminal")
                .depth_of(&[
                    gpui::KeyContext::parse("Zetta").unwrap(),
                    gpui::KeyContext::parse("PaneResize").unwrap(),
                    gpui::KeyContext::parse("Terminal").unwrap(),
                ])
                .is_some()
        );
    }
}

#[test]
fn pane_focus_shortcuts_use_the_platform_modifier() {
    let shortcuts = ["alt-left", "alt-right", "alt-up", "alt-down"];

    for (binding, shortcut) in focus_pane_keybindings().into_iter().zip(shortcuts) {
        let shortcut = gpui::Keystroke::parse(&platform_keystroke(shortcut)).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    }
}

#[test]
fn alt_shortcuts_use_the_platform_equivalent() {
    for (shortcut, expected) in [
        ("alt-left", "cmd-left"),
        ("alt-shift-l", "cmd-shift-l"),
        ("alt-shift-o", "cmd-shift-o"),
        ("alt-shift-e", "cmd-shift-e"),
        ("alt-shift-a", "cmd-shift-a"),
        ("alt-shift-down", "cmd-shift-down"),
        ("alt-shift-up", "cmd-shift-up"),
        ("alt-shift-left", "cmd-shift-left"),
        ("alt-shift-right", "cmd-shift-right"),
        ("alt-shift-f", "cmd-shift-f"),
        ("ctrl-alt-v", "ctrl-cmd-v"),
        ("alt-shift-s", "cmd-shift-s"),
        ("alt-shift-r", "cmd-shift-r"),
        ("alt-shift-h", "cmd-shift-h"),
        ("alt-shift-x", "cmd-shift-x"),
        ("alt-shift-=", "cmd-shift-="),
        ("alt-shift-+", "cmd-shift-+"),
        ("alt-shift--", "cmd-shift--"),
        ("alt-shift-0", "cmd-shift-0"),
        ("ctrl-alt-r", "ctrl-cmd-r"),
        ("alt-space", "alt-space"),
    ] {
        let expected = if cfg!(target_os = "macos") {
            expected
        } else {
            shortcut
        };
        assert_eq!(platform_keystroke(shortcut), expected);
    }
}

#[test]
fn close_pane_uses_the_pane_control_modifiers() {
    assert_eq!(CLOSE_PANE_KEYBINDING, platform_keystroke("alt-shift-x"));
    let shortcut = gpui::Keystroke::parse(CLOSE_PANE_KEYBINDING).unwrap();
    assert_eq!(
        close_pane_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn close_all_windows_uses_the_documented_shortcut() {
    assert_eq!(CLOSE_ALL_WINDOWS_KEYBINDING, "ctrl-shift-x");
    let shortcut = gpui::Keystroke::parse(CLOSE_ALL_WINDOWS_KEYBINDING).unwrap();
    assert_eq!(
        close_all_windows_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn close_window_uses_the_documented_shortcut() {
    assert_eq!(CLOSE_WINDOW_KEYBINDING, "ctrl-shift-q");
    let shortcut = gpui::Keystroke::parse(CLOSE_WINDOW_KEYBINDING).unwrap();
    assert_eq!(
        close_window_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn terminal_clear_uses_ctrl_shift_l() {
    let shortcut = gpui::Keystroke::parse("ctrl-shift-l").unwrap();
    assert_eq!(
        terminal_clear_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
    assert_eq!(terminal_clear_keybinding().action().name(), Clear.name());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_shortcuts_are_additional_application_bindings() {
    let expected = [
        ("cmd-t", NewTab.name()),
        ("cmd-n", NewWindow.name()),
        ("cmd-,", ToggleSettings.name()),
        ("cmd-w", CloseTab.name()),
        ("cmd-q", CloseWindow.name()),
        ("cmd-x", CloseAllWindows.name()),
        ("cmd-c", CopyAndClearSelection.name()),
        ("cmd-l", Clear.name()),
        ("cmd-v", Paste.name()),
    ];

    for (binding, (shortcut, action)) in macos_keybindings().into_iter().zip(expected) {
        let shortcut = gpui::Keystroke::parse(shortcut).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
        assert_eq!(binding.action().name(), action);
    }

    assert_eq!(CLOSE_WINDOW_KEYBINDING, "ctrl-shift-q");
    assert_eq!(CLOSE_ALL_WINDOWS_KEYBINDING, "ctrl-shift-x");

    let unbinding = macos_terminal_clear_unbinding();
    let shortcut = gpui::Keystroke::parse("cmd-k").unwrap();
    assert_eq!(unbinding.match_keystrokes(&[shortcut]), Some(false));
    assert_eq!(
        unbinding
            .action()
            .as_any()
            .downcast_ref::<Unbind>()
            .expect("Cmd+K should use an unbind marker")
            .0
            .as_ref(),
        "terminal::Clear"
    );
}

#[test]
fn pane_output_uses_the_standard_save_shortcut() {
    assert_eq!(
        SAVE_PANE_OUTPUT_KEYBINDING,
        platform_keystroke("alt-shift-s")
    );
    let shortcut = gpui::Keystroke::parse(SAVE_PANE_OUTPUT_KEYBINDING).unwrap();
    assert_eq!(
        pane_output_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn select_all_and_reconnect_use_scope_based_shortcuts() {
    assert_eq!(SELECT_ALL_KEYBINDING, platform_keystroke("alt-shift-a"));
    assert_eq!(RECONNECT_SESSION_KEYBINDING, "ctrl-shift-a");
    assert_ne!(SELECT_ALL_KEYBINDING, RECONNECT_SESSION_KEYBINDING);

    let select_all = gpui::Keystroke::parse(SELECT_ALL_KEYBINDING).unwrap();
    assert_eq!(
        select_all_keybinding().match_keystrokes(&[select_all]),
        Some(false)
    );
    let reconnect = gpui::Keystroke::parse(RECONNECT_SESSION_KEYBINDING).unwrap();
    assert_eq!(
        reconnect_session_keybinding().match_keystrokes(&[reconnect]),
        Some(false)
    );
}

#[test]
fn application_menu_shortcut_uses_the_platform_modifier() {
    let binding = application_menu_keybinding();
    assert_eq!(APPLICATION_MENU_KEYBINDING, platform_keystroke("alt-space"));
    let shortcut = gpui::Keystroke::parse(APPLICATION_MENU_KEYBINDING).unwrap();
    let binding = binding.expect("all platforms should bind the application menu");
    assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    assert!(
        binding
            .predicate()
            .expect("application menu shortcut should be scoped to Zetta")
            .depth_of(&[
                gpui::KeyContext::parse("Zetta").unwrap(),
                gpui::KeyContext::parse("Terminal").unwrap(),
            ])
            .is_some()
    );
}

#[test]
fn application_menu_navigation_shortcuts_apply_while_a_menu_is_focused() {
    let shortcuts = ["left", "right"];
    for (binding, shortcut) in application_menu_navigation_keybindings()
        .into_iter()
        .zip(shortcuts)
    {
        assert_eq!(
            binding.match_keystrokes(&[gpui::Keystroke::parse(shortcut).unwrap()]),
            Some(false)
        );
        assert!(
            binding
                .predicate()
                .expect("application menu navigation should be scoped to menus")
                .depth_of(&[
                    gpui::KeyContext::parse("Zetta").unwrap(),
                    gpui::KeyContext::parse("Terminal").unwrap(),
                    gpui::KeyContext::parse("menu").unwrap(),
                ])
                .is_some()
        );
    }
}

#[test]
fn pane_font_size_shortcuts_use_pane_control_modifiers() {
    let bindings = pane_font_size_keybindings();
    for (binding, shortcut) in
        bindings
            .into_iter()
            .zip(["alt-shift-=", "alt-shift-+", "alt-shift--", "alt-shift-0"])
    {
        let shortcut = gpui::Keystroke::parse(&platform_keystroke(shortcut)).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    }
}

#[cfg(feature = "serial-console")]
#[test]
fn serial_console_avoids_the_linux_unicode_input_shortcut() {
    assert_eq!(SERIAL_CONSOLE_KEYBINDING, "ctrl-shift-s");
    assert_ne!(SERIAL_CONSOLE_KEYBINDING, "ctrl-shift-u");
    let shortcut = gpui::Keystroke::parse(SERIAL_CONSOLE_KEYBINDING).unwrap();
    assert_eq!(
        serial_console_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn auto_background_tab_uses_the_documented_shortcut() {
    assert_eq!(AUTO_BACKGROUND_TAB_KEYBINDING, "ctrl-shift-b");
    let shortcut = gpui::Keystroke::parse(AUTO_BACKGROUND_TAB_KEYBINDING).unwrap();
    assert_eq!(
        auto_background_tab_keybinding().match_keystrokes(std::slice::from_ref(&shortcut)),
        Some(false)
    );
}

#[test]
fn detach_tab_uses_the_tab_scoped_shortcut() {
    assert_eq!(DETACH_TAB_KEYBINDING, "ctrl-shift-d");
    let shortcut = gpui::Keystroke::parse(DETACH_TAB_KEYBINDING).unwrap();
    assert_eq!(
        detach_tab_keybinding().match_keystrokes(&[shortcut]),
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
        let shortcut = gpui::Keystroke::parse(&platform_keystroke(shortcut)).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn native_macos_menus_duplicate_the_title_bar_menus() {
    let profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
        },
        Profile {
            name: "Alternate".to_owned(),
            command: Shell::Program("alternate-shell".to_owned()),
            theme: None,
        },
    ];
    let [application_menu, profile_menu, window_menu] = native_macos_menus(&profiles, 1);
    assert_eq!(application_menu.name, "Zetta");
    assert_eq!(profile_menu.name, "Profile");
    assert_eq!(window_menu.name, "Window");

    let application_action_names = application_menu
        .items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action { action, .. } => Some(action.name()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        application_action_names,
        [
            NewTab.name(),
            NewWindow.name(),
            ToggleSettings.name(),
            OpenThemes.name(),
            OpenKeymap.name(),
            CloseTab.name(),
            CloseWindow.name(),
            CloseAllWindows.name(),
        ]
    );

    let profile_items = profile_menu
        .items
        .into_iter()
        .map(|item| match item {
            MenuItem::Action {
                name,
                action,
                checked,
                ..
            } => (name.to_string(), action.name(), checked),
            _ => panic!("profile menu contains a non-action item"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        profile_items,
        vec![
            ("System".to_owned(), OpenProfile::name_for_type(), false),
            ("Alternate".to_owned(), OpenProfile::name_for_type(), true)
        ]
    );

    let action_names = window_menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Action { action, .. } => Some(action.name()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(action_names, [MinimizeWindow.name(), ZoomWindow.name()]);
}
