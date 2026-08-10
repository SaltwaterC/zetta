use super::*;
#[cfg(feature = "serial-console")]
use crate::cli_services::SerialCommand;
use crate::worktree_cli::WorktreeCommand;

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
fn worktree_subcommand_is_available_without_cli_services() {
    let arguments = parse_args_from([
        OsString::from("wt"),
        OsString::from("new"),
        OsString::from("--copy"),
        OsString::from(".zetta-local"),
        OsString::from("-P"),
        OsString::from("feature/api"),
    ])
    .unwrap();
    assert_eq!(
        arguments.mode,
        StartupMode::Worktree(WorktreeCommand::New {
            name: "feature/api".to_owned(),
            path_only: true,
            copy_paths: vec![PathBuf::from(".zetta-local")],
        })
    );
    assert!(
        parse_args_from([
            OsString::from("wt"),
            OsString::from("done"),
            OsString::from("--path-only"),
        ])
        .is_ok()
    );
}

#[test]
fn tabicon_subcommand_parses_icons_and_dynamic_listing() {
    assert_eq!(
        parse_args_from([OsString::from("tabicon"), OsString::from("terminal")])
            .unwrap()
            .mode,
        StartupMode::SetTabIcon {
            icon: Some(IconName::Terminal)
        }
    );
    assert_eq!(
        parse_args_from([
            OsString::from("tabicon"),
            OsString::from("--icon"),
            OsString::from("none")
        ])
        .unwrap()
        .mode,
        StartupMode::SetTabIcon { icon: None }
    );
    assert_eq!(
        parse_args_from([OsString::from("tabicon"), OsString::from("--list")])
            .unwrap()
            .mode,
        StartupMode::ListTabIcons
    );
    assert!(parse_args_from([OsString::from("tabicon")]).is_err());
    assert!(parse_args_from([OsString::from("tabicon"), OsString::from("not-an-icon")]).is_err());
}

#[test]
fn panetheme_subcommand_parses_names_resets_and_dynamic_listing() {
    assert_eq!(
        parse_args_from([OsString::from("panetheme"), OsString::from("Dracula")])
            .unwrap()
            .mode,
        StartupMode::SetPaneTheme {
            theme: Some("Dracula".to_owned())
        }
    );
    assert_eq!(
        parse_args_from([
            OsString::from("panetheme"),
            OsString::from("--theme"),
            OsString::from("One Light"),
        ])
        .unwrap()
        .mode,
        StartupMode::SetPaneTheme {
            theme: Some("One Light".to_owned())
        }
    );
    assert_eq!(
        parse_args_from([OsString::from("panetheme"), OsString::from("--reset")])
            .unwrap()
            .mode,
        StartupMode::SetPaneTheme { theme: None }
    );
    assert_eq!(
        parse_args_from([OsString::from("panetheme"), OsString::from("--list")])
            .unwrap()
            .mode,
        StartupMode::ListPaneThemes
    );
    assert!(parse_args_from([OsString::from("panetheme")]).is_err());
    assert!(
        parse_args_from([
            OsString::from("panetheme"),
            OsString::from("--list"),
            OsString::from("--reset"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("panetheme"),
            OsString::from("--reset"),
            OsString::from("Dracula"),
        ])
        .is_err()
    );
}

#[test]
fn overlay_subcommand_parses_text_and_reset() {
    assert_eq!(
        parse_args_from([OsString::from("overlay"), OsString::from("Prod")])
            .unwrap()
            .mode,
        StartupMode::SetPaneOverlay(PaneOverlayRequest {
            text: Some("Prod".to_owned()),
            font_size: None,
            opacity: None,
            color: None,
        })
    );
    assert_eq!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("--text"),
            OsString::from("Staging box"),
        ])
        .unwrap()
        .mode,
        StartupMode::SetPaneOverlay(PaneOverlayRequest {
            text: Some("Staging box".to_owned()),
            font_size: None,
            opacity: None,
            color: None,
        })
    );
    assert_eq!(
        parse_args_from([OsString::from("overlay"), OsString::from("--reset")])
            .unwrap()
            .mode,
        StartupMode::SetPaneOverlay(PaneOverlayRequest {
            text: None,
            font_size: None,
            opacity: None,
            color: None,
        })
    );
    assert!(parse_args_from([OsString::from("overlay")]).is_err());
    assert!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("--reset"),
            OsString::from("Prod"),
        ])
        .is_err()
    );
}

#[test]
fn overlay_subcommand_parses_style_options_and_rejects_invalid_values() {
    assert_eq!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("Prod"),
            OsString::from("--size"),
            OsString::from("2xl"),
            OsString::from("--opacity"),
            OsString::from("50"),
            OsString::from("--color"),
            OsString::from("ff8800"),
        ])
        .unwrap()
        .mode,
        StartupMode::SetPaneOverlay(PaneOverlayRequest {
            text: Some("Prod".to_owned()),
            font_size: Some(OverlayFontSize::ExtraExtraLarge),
            opacity: Some(50),
            color: Some("ff8800".to_owned()),
        })
    );
    assert!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("Prod"),
            OsString::from("--color"),
            OsString::from("#ff8800"),
        ])
        .is_ok(),
        "a leading # must still be accepted"
    );
    assert!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("Prod"),
            OsString::from("--size"),
            OsString::from("huge"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("Prod"),
            OsString::from("--opacity"),
            OsString::from("101"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("Prod"),
            OsString::from("--opacity"),
            OsString::from("not-a-number"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("Prod"),
            OsString::from("--color"),
            OsString::from("not-a-color"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("overlay"),
            OsString::from("--reset"),
            OsString::from("--size"),
            OsString::from("xl"),
        ])
        .is_err()
    );
}

#[test]
fn overlay_subcommand_accepts_named_colours_case_insensitively() {
    let mode = parse_args_from([
        OsString::from("overlay"),
        OsString::from("Prod"),
        OsString::from("--color"),
        OsString::from("  ReD  "),
    ])
    .unwrap()
    .mode;

    assert_eq!(
        mode,
        StartupMode::SetPaneOverlay(PaneOverlayRequest {
            text: Some("Prod".to_owned()),
            font_size: None,
            opacity: None,
            color: Some("  ReD  ".to_owned()),
        })
    );
}

#[test]
fn vi_subcommand_bypasses_application_startup_and_preserves_arguments() {
    let args = parse_args_from([
        OsString::from("vi"),
        OsString::from("-R"),
        OsString::from("notes.txt"),
    ])
    .unwrap();

    assert_eq!(
        args.mode,
        StartupMode::Vi(vec!["-R".into(), "notes.txt".into()])
    );
    assert!(!should_handoff_to_existing_process(&args));
}

#[test]
fn edit_subcommand_bypasses_application_startup_and_preserves_paths() {
    let args = parse_args_from([
        OsString::from("edit"),
        OsString::from("--"),
        OsString::from("notes with spaces.txt"),
    ])
    .unwrap();

    assert_eq!(
        args.mode,
        StartupMode::Edit {
            arguments: vec!["notes with spaces.txt".into()],
            delete_after: false,
        }
    );
    assert!(!should_handoff_to_existing_process(&args));
}

#[test]
fn edit_subcommand_accepts_managed_file_cleanup() {
    let args = parse_args_from([
        OsString::from("edit"),
        OsString::from("--delete-after"),
        OsString::from("--"),
        OsString::from("scrollback.txt"),
    ])
    .unwrap();

    assert_eq!(
        args.mode,
        StartupMode::Edit {
            arguments: vec!["scrollback.txt".into()],
            delete_after: true,
        }
    );
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

#[test]
fn attention_subcommand_defaults_to_a_badge_without_notification() {
    let args = parse_args_from([OsString::from("attention")]).unwrap();
    let StartupMode::Attention(command) = args.mode else {
        panic!("unexpected startup mode");
    };
    assert!(!command.notify);
    assert_eq!(command.notification.summary, "Attention required");
    assert_eq!(command.notification.body, None);
    assert_eq!(command.notification.timeout, None);
    assert!(!should_handoff_to_existing_process(&StartupArgs {
        config_path: None,
        keymap_path: None,
        profile: None,
        split: None,
        replace_pane: false,
        theme_override: None,
        mode: StartupMode::Attention(command),
        profile_report: None,
        profile_duration: None,
        profile_pane_stress: false,
        profile_background_stress: false,
        profile_sparse_updates: false,
        profile_external_terminal: false,
        tftp_command: None,
    }));
}

#[cfg(feature = "notifications")]
#[test]
fn attention_subcommand_parses_summary_body_and_notification_options() {
    let args = parse_args_from([
        OsString::from("attention"),
        OsString::from("--notify"),
        OsString::from("-a"),
        OsString::from("zetta-ci"),
        OsString::from("--icon"),
        OsString::from("icon.png"),
        OsString::from("-s"),
        OsString::from("zetta-ok"),
        OsString::from("--timeout"),
        OsString::from("5000"),
        OsString::from("Build finished"),
        OsString::from("All tests passed"),
    ])
    .unwrap();
    let StartupMode::Attention(command) = args.mode else {
        panic!("unexpected startup mode");
    };
    assert!(command.notify);
    assert_eq!(command.notification.summary, "Build finished");
    assert_eq!(
        command.notification.body.as_deref(),
        Some("All tests passed")
    );
    assert_eq!(command.notification.app_name.as_deref(), Some("zetta-ci"));
    assert_eq!(command.notification.icon.as_deref(), Some("icon.png"));
    assert_eq!(command.notification.sound.as_deref(), Some("zetta-ok"));
    assert_eq!(
        command.notification.timeout,
        Some(crate::cli_services::NotificationTimeout::Milliseconds(5000))
    );
}

#[test]
fn attention_subcommand_rejects_duplicate_invalid_and_unpaired_options() {
    assert!(
        parse_args_from([
            OsString::from("attention"),
            OsString::from("--notify"),
            OsString::from("-n"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("attention"),
            OsString::from("--timeout"),
            OsString::from("soon"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("attention"),
            OsString::from("--icon"),
            OsString::from("icon.png"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("attention"),
            OsString::from("summary"),
            OsString::from("body"),
            OsString::from("extra"),
        ])
        .is_err()
    );
    assert!(parse_args_from([OsString::from("attention"), OsString::from("--unknown"),]).is_err());
}

#[test]
fn attention_target_requires_positive_inherited_process_and_tab_ids() {
    assert_eq!(parse_attention_target("42", "7").unwrap(), (42, 7));
    for (process_id, attention_id) in [
        ("", "7"),
        ("not-a-process", "7"),
        ("0", "7"),
        ("42", ""),
        ("42", "not-an-attention"),
        ("42", "0"),
    ] {
        assert!(parse_attention_target(process_id, attention_id).is_err());
    }
}

#[cfg(not(feature = "notifications"))]
#[test]
fn attention_notification_is_rejected_when_notifications_are_disabled() {
    assert!(parse_args_from([OsString::from("attention"), OsString::from("--notify"),]).is_err());
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
fn splits_subcommand_lists_configured_templates_without_starting_the_application() {
    let args = parse_args_from([OsString::from("splits")]).unwrap();
    assert_eq!(args.mode, StartupMode::ListPaneSplits);
    assert!(!should_handoff_to_existing_process(&args));
    assert!(parse_args_from([OsString::from("splits"), OsString::from("--unknown")]).is_err());

    let config = Config::defaults(None, None);
    assert_eq!(
        configured_split_names(&config),
        [
            "four-vertical".to_owned(),
            "quarters".to_owned(),
            "three-left".to_owned(),
            "three-right".to_owned(),
        ]
    );

    let custom_config = Config::parse(
        r#"{
            "pane_split_templates": {
                "custom-layout": {
                    "horizontal": ["pane", "pane"]
                }
            }
        }"#,
        None,
        None,
    )
    .unwrap();
    assert!(configured_split_names(&custom_config).contains(&"custom-layout".to_owned()));
    assert!(validate_launch_split(&custom_config, Some("custom-layout")).is_ok());
}

#[test]
fn sessions_reconnect_subcommand_accepts_a_stable_id_without_a_secret_argument() {
    let args = parse_args_from([
        OsString::from("sessions"),
        OsString::from("reconnect"),
        OsString::from("123:7:42"),
    ])
    .unwrap();
    assert_eq!(
        args.mode,
        StartupMode::ReconnectBackgroundSession {
            identifier: "123:7:42".to_owned(),
        }
    );
    assert!(!should_handoff_to_existing_process(&args));

    let option = parse_args_from([
        OsString::from("sessions"),
        OsString::from("reconnect"),
        OsString::from("-s"),
        OsString::from("123:7:42"),
    ])
    .unwrap();
    assert_eq!(option, args);
    assert!(parse_args_from([OsString::from("sessions"), OsString::from("reconnect"),]).is_err());
    assert!(
        parse_args_from([
            OsString::from("sessions"),
            OsString::from("reconnect"),
            OsString::from("--secret"),
            OsString::from("not-private"),
        ])
        .is_err()
    );
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
    let split = parse_args_from([OsString::from("--split"), OsString::from("quarters")]).unwrap();
    let sessions = parse_args_from([OsString::from("sessions")]).unwrap();

    assert!(should_handoff_to_existing_process(&plain));
    assert!(!should_handoff_to_existing_process(&profile));
    assert!(!should_handoff_to_existing_process(&split));
    assert!(!should_handoff_to_existing_process(&sessions));
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
fn benchmark_subcommand_arguments_are_cross_platform() {
    assert_eq!(
        parse_args_from([OsString::from("benchmark")]).unwrap(),
        StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
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
            split: None,
            replace_pane: false,
            theme_override: None,
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
        OsString::from("-p"),
        OsString::from("WSL: Ubuntu"),
        OsString::from("-t"),
        OsString::from("Dracula"),
    ])
    .unwrap();
    let longhand = parse_args_from([
        OsString::from("--profile"),
        OsString::from("WSL: Ubuntu"),
        OsString::from("--theme"),
        OsString::from("Dracula"),
    ])
    .unwrap();
    assert_eq!(shorthand, longhand);
    assert_eq!(shorthand.theme_override.as_deref(), Some("Dracula"));

    assert!(
        parse_args_from([OsString::from("--theme"), OsString::from("Dracula")]).is_err(),
        "--theme without --profile must be rejected"
    );

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
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::Program("wsl.exe".to_owned()),
            theme: None,
            icon: ProfileIcon::Bash,
        },
    ];

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
