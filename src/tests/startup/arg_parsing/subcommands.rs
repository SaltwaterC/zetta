use super::*;
#[cfg(notify_cleanup_enabled)]
use crate::cli_services::NotifyCleanupCommand;
#[cfg(feature = "serial-console")]
use crate::cli_services::SerialCommand;
#[cfg(feature = "worktree")]
use zwt::WorktreeCommand;

#[test]
fn pane_wait_preserves_dependencies_and_exact_command_arguments() {
    let arguments = parse_args_from([
        OsString::from("pane"),
        OsString::from("wait"),
        OsString::from("api,db"),
        OsString::from("-a"),
        OsString::from("--"),
        OsString::from("python"),
        OsString::from("-c"),
        OsString::from("print('hello world')"),
        OsString::from("--literal-option"),
        OsString::new(),
    ])
    .unwrap();

    assert_eq!(
        arguments.mode,
        StartupMode::PaneWait(crate::run_command::PaneWaitCommand {
            dependencies: vec!["api".to_owned(), "db".to_owned()],
            allow_failure: true,
            command: vec![
                "python".to_owned(),
                "-c".to_owned(),
                "print('hello world')".to_owned(),
                "--literal-option".to_owned(),
                String::new(),
            ],
        })
    );
}

#[test]
fn pane_wait_rejects_malformed_syntax() {
    for arguments in [
        vec!["pane", "wait", "--", "echo"],
        vec!["pane", "wait", "", "--", "echo"],
        vec!["pane", "wait", "api,,db", "--", "echo"],
        vec!["pane", "wait", "api,api", "--", "echo"],
        vec!["pane", "wait", "api", "echo"],
        vec!["pane", "wait", "api", "--"],
        vec!["pane", "wait", "api", "--wait", "db", "--", "echo"],
        vec!["pane", "wait", "--wait", "api", "--", "echo"],
        vec!["pane", "wait", "api", "--allow-failure", "-a", "--", "echo"],
    ] {
        assert!(
            parse_args_from(arguments.iter().map(|argument| OsString::from(*argument))).is_err(),
            "expected pane wait arguments to be rejected: {arguments:?}"
        );
    }
    assert!(parse_args_from([OsString::from("run")]).is_err());
}

#[test]
fn pane_wait_accepts_flags_before_or_after_the_dependency_list() {
    for arguments in [
        ["pane", "wait", "-a", "api", "--", "echo"],
        ["pane", "wait", "api", "-a", "--", "echo"],
    ] {
        assert_eq!(
            parse_args_from(arguments.iter().map(|argument| OsString::from(*argument)))
                .unwrap()
                .mode,
            StartupMode::PaneWait(crate::run_command::PaneWaitCommand {
                dependencies: vec!["api".to_owned()],
                allow_failure: true,
                command: vec!["echo".to_owned()],
            })
        );
    }
}

#[cfg(feature = "worktree")]
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
    assert_eq!(
        parse_args_from([
            OsString::from("wt"),
            OsString::from("abort"),
            OsString::from("-P"),
        ])
        .unwrap()
        .mode,
        StartupMode::Worktree(WorktreeCommand::Abort { path_only: true })
    );
    assert_eq!(
        parse_args_from([
            OsString::from("wt"),
            OsString::from("sync"),
            OsString::from("main~2"),
        ])
        .unwrap()
        .mode,
        StartupMode::Worktree(WorktreeCommand::Sync {
            commit: Some("main~2".to_owned()),
        })
    );
    assert_eq!(
        parse_args_from([OsString::from("wt"), OsString::from("config")])
            .unwrap()
            .mode,
        StartupMode::Worktree(WorktreeCommand::Config)
    );
}

#[test]
fn project_subcommand_is_available_without_cli_services() {
    let arguments = parse_args_from([
        OsString::from("project"),
        OsString::from("open"),
        OsString::from("--path"),
        OsString::from("workspace"),
    ])
    .unwrap();

    assert_eq!(
        arguments.mode,
        StartupMode::Project(crate::project_cli::ProjectCommand::Open {
            path: Some(PathBuf::from("workspace")),
        })
    );
}

#[test]
fn project_command_subcommand_lists_and_preserves_delimited_arguments() {
    assert_eq!(
        parse_args_from([OsString::from("cmd"), OsString::from("--list")])
            .unwrap()
            .mode,
        StartupMode::ProjectCommand(crate::project_commands::ProjectCommandInvocation::List)
    );
    assert_eq!(
        parse_args_from([
            OsString::from("cmd"),
            OsString::from("test:unit"),
            OsString::from("--"),
            OsString::from("--release"),
            OsString::from("two words"),
        ])
        .unwrap()
        .mode,
        StartupMode::ProjectCommand(crate::project_commands::ProjectCommandInvocation::Run {
            name: "test:unit".to_owned(),
            arguments: vec!["--release".to_owned(), "two words".to_owned()],
        })
    );
    for arguments in [
        vec!["cmd"],
        vec!["cmd", "build", "--release"],
        vec!["cmd", "-l", "build"],
        vec!["cmd", "--unknown"],
    ] {
        assert!(
            parse_args_from(arguments.iter().map(|argument| OsString::from(*argument))).is_err(),
            "expected project command arguments to be rejected: {arguments:?}"
        );
    }
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
fn theme_subcommand_requires_scope_and_parses_names_resets_and_dynamic_listing() {
    assert_eq!(
        parse_args_from([
            OsString::from("theme"),
            OsString::from("pane"),
            OsString::from("Dracula"),
        ])
        .unwrap()
        .mode,
        StartupMode::SetTheme {
            scope: ThemeScope::Pane,
            theme: Some("Dracula".to_owned())
        }
    );
    assert_eq!(
        parse_args_from([
            OsString::from("theme"),
            OsString::from("tab"),
            OsString::from("--theme"),
            OsString::from("One Light"),
        ])
        .unwrap()
        .mode,
        StartupMode::SetTheme {
            scope: ThemeScope::Tab,
            theme: Some("One Light".to_owned())
        }
    );
    assert_eq!(
        parse_args_from([
            OsString::from("theme"),
            OsString::from("pane"),
            OsString::from("--reset"),
        ])
        .unwrap()
        .mode,
        StartupMode::SetTheme {
            scope: ThemeScope::Pane,
            theme: None
        }
    );
    assert_eq!(
        parse_args_from([
            OsString::from("theme"),
            OsString::from("tab"),
            OsString::from("--list"),
        ])
        .unwrap()
        .mode,
        StartupMode::ListThemes
    );
    assert!(parse_args_from([OsString::from("theme")]).is_err());
    assert!(parse_args_from([OsString::from("theme"), OsString::from("window")]).is_err());
    assert!(
        parse_args_from([
            OsString::from("theme"),
            OsString::from("pane"),
            OsString::from("--list"),
            OsString::from("--reset"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("theme"),
            OsString::from("tab"),
            OsString::from("--reset"),
            OsString::from("Dracula"),
        ])
        .is_err()
    );
    assert!(parse_args_from([OsString::from("panetheme"), OsString::from("Dracula")]).is_err());
}

#[test]
fn pane_subcommand_preserves_exact_command_arguments_and_modes() {
    let args = parse_args_from([
        OsString::from("pane"),
        OsString::from("--direction"),
        OsString::from("right"),
        OsString::from("-l"),
        OsString::from("api"),
        OsString::from("--"),
        OsString::from("npm"),
        OsString::from("run dev"),
        OsString::from("--host=127.0.0.1"),
    ])
    .unwrap();
    assert_eq!(
        args.mode,
        StartupMode::Pane(PaneCommand {
            direction: Some(PaneDirection::Right),
            label: Some("api".to_owned()),
            pane: None,
            overlay: None,
            stack: false,
            list: false,
            command: vec![
                "npm".to_owned(),
                "run dev".to_owned(),
                "--host=127.0.0.1".to_owned()
            ],
        })
    );

    assert_eq!(
        parse_args_from([
            OsString::from("pane"),
            OsString::from("-p"),
            OsString::from("api"),
            OsString::from("-s"),
            OsString::from("--"),
            OsString::from("tail"),
            OsString::from("-f"),
            OsString::from("server log"),
        ])
        .unwrap()
        .mode,
        StartupMode::Pane(PaneCommand {
            direction: None,
            label: None,
            pane: Some("api".to_owned()),
            overlay: None,
            stack: true,
            list: false,
            command: vec!["tail".to_owned(), "-f".to_owned(), "server log".to_owned()],
        })
    );
}

#[test]
fn pane_subcommand_supports_short_options_and_listing() {
    assert_eq!(
        parse_args_from([OsString::from("pane"), OsString::from("-L")])
            .unwrap()
            .mode,
        StartupMode::Pane(PaneCommand {
            direction: None,
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            list: true,
            command: Vec::new(),
        })
    );
    assert_eq!(
        parse_args_from([
            OsString::from("pane"),
            OsString::from("-d"),
            OsString::from("up"),
            OsString::from("--"),
            OsString::from("cargo"),
            OsString::from("test"),
        ])
        .unwrap()
        .mode,
        StartupMode::Pane(PaneCommand {
            direction: Some(PaneDirection::Up),
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            list: false,
            command: vec!["cargo".to_owned(), "test".to_owned()],
        })
    );
}

#[test]
fn pane_subcommand_parses_new_split_overlays_and_styles() {
    assert_eq!(
        parse_args_from([
            OsString::from("pane"),
            OsString::from("-d"),
            OsString::from("right"),
            OsString::from("-o"),
            OsString::from("API"),
            OsString::from("--overlay-size"),
            OsString::from("2xl"),
            OsString::from("-O"),
            OsString::from("70"),
            OsString::from("--overlay-color"),
            OsString::from("cyan"),
            OsString::from("--"),
            OsString::from("npm"),
            OsString::from("run"),
        ])
        .unwrap()
        .mode,
        StartupMode::Pane(PaneCommand {
            direction: Some(PaneDirection::Right),
            label: None,
            pane: None,
            overlay: Some(PaneOverlayRequest {
                text: Some("API".to_owned()),
                font_size: Some(OverlayFontSize::ExtraExtraLarge),
                opacity: Some(70),
                color: Some("cyan".to_owned()),
            }),
            stack: false,
            list: false,
            command: vec!["npm".to_owned(), "run".to_owned()],
        })
    );

    for args in [
        vec!["pane", "--overlay", "API", "--", "echo"],
        vec![
            "pane",
            "--direction",
            "right",
            "--overlay-size",
            "xl",
            "--",
            "echo",
        ],
        vec![
            "pane",
            "--direction",
            "right",
            "--overlay",
            "API",
            "--overlay",
            "Other",
            "--",
            "echo",
        ],
        vec![
            "pane",
            "--direction",
            "right",
            "--overlay",
            "API",
            "--overlay-opacity",
            "101",
            "--",
            "echo",
        ],
        vec![
            "pane",
            "--direction",
            "right",
            "--overlay",
            "API",
            "--overlay-color",
            "nope",
            "--",
            "echo",
        ],
    ] {
        assert!(
            parse_args_from(args.clone().into_iter().map(OsString::from)).is_err(),
            "expected pane overlay arguments to be rejected: {args:?}"
        );
    }
}

#[test]
fn pane_subcommand_preserves_labels_starting_with_a_dash() {
    assert_eq!(
        parse_args_from([
            OsString::from("pane"),
            OsString::from("--direction"),
            OsString::from("right"),
            OsString::from("--label"),
            OsString::from("-api"),
            OsString::from("--"),
            OsString::from("echo"),
        ])
        .unwrap()
        .mode,
        StartupMode::Pane(PaneCommand {
            direction: Some(PaneDirection::Right),
            label: Some("-api".to_owned()),
            pane: None,
            overlay: None,
            stack: false,
            list: false,
            command: vec!["echo".to_owned()],
        })
    );
    assert_eq!(
        parse_args_from([
            OsString::from("pane"),
            OsString::from("--pane"),
            OsString::from("-api"),
            OsString::from("--"),
            OsString::from("echo"),
        ])
        .unwrap()
        .mode,
        StartupMode::Pane(PaneCommand {
            direction: None,
            label: None,
            pane: Some("-api".to_owned()),
            overlay: None,
            stack: false,
            list: false,
            command: vec!["echo".to_owned()],
        })
    );
}

#[test]
fn pane_subcommand_rejects_missing_commands_invalid_directions_and_conflicts() {
    for args in [
        vec!["pane"],
        vec!["pane", "--direction", "diagonal", "--", "echo"],
        vec!["pane", "--label", "api", "--", "echo"],
        vec!["pane", "--direction", "right", "--stack", "--", "echo"],
        vec![
            "pane",
            "--direction",
            "right",
            "--pane",
            "api",
            "--",
            "echo",
        ],
        vec!["pane", "--list", "--", "echo"],
        vec!["pane", "echo"],
    ] {
        assert!(
            parse_args_from(args.clone().into_iter().map(OsString::from)).is_err(),
            "expected pane arguments to be rejected: {args:?}"
        );
    }
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

#[cfg(notify_cleanup_enabled)]
#[test]
fn notify_cleanup_subcommand_bypasses_application_startup() {
    let args = parse_args_from([OsString::from("notify"), OsString::from("cleanup")]).unwrap();

    assert!(matches!(
        args.mode,
        StartupMode::CliService(CliServiceCommand::NotifyCleanup(_))
    ));
    assert!(!should_handoff_to_existing_process(&args));

    let args = parse_args_from([
        OsString::from("notify"),
        OsString::from("cleanup"),
        OsString::from("--dry-run"),
    ])
    .unwrap();
    assert!(matches!(
        args.mode,
        StartupMode::CliService(CliServiceCommand::NotifyCleanup(NotifyCleanupCommand {
            dry_run: true
        }))
    ));

    assert!(
        parse_args_from([
            OsString::from("notify"),
            OsString::from("cleanup"),
            OsString::from("--unknown")
        ])
        .is_err()
    );
    assert!(parse_args_from([OsString::from("notify-cleanup")]).is_err());
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
        no_mux: false,
        mode: StartupMode::Attention(command),
        profile_report: None,
        profile_duration: None,
        profile_pane_stress: false,
        profile_workload: PerformanceWorkload::Standard,
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
fn mux_subcommand_forwards_its_arguments_verbatim() {
    // The multiplexer's own argument parsing lives in `zmux`, so `zetta mux`
    // must hand everything after the subcommand over untouched rather than
    // interpreting it — otherwise `zmux X` and `zetta mux X` could diverge.
    let bare = parse_args_from([OsString::from("mux")]).unwrap();
    assert_eq!(bare.mode, StartupMode::Mux(Vec::new()));

    let forwarded = parse_args_from([
        OsString::from("mux"),
        OsString::from("list"),
        OsString::from("--json"),
    ])
    .unwrap();
    assert_eq!(
        forwarded.mode,
        StartupMode::Mux(vec![OsString::from("list"), OsString::from("--json")])
    );

    // Including arguments Zetta itself would otherwise claim.
    let shadowing = parse_args_from([OsString::from("mux"), OsString::from("--profile")]).unwrap();
    assert_eq!(
        shadowing.mode,
        StartupMode::Mux(vec![OsString::from("--profile")])
    );
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
                    "layout": { "horizontal": [{}, {}] }
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
fn mux_reconnect_subcommand_is_forwarded_with_its_stable_id() {
    let args = parse_args_from([
        OsString::from("mux"),
        OsString::from("reconnect"),
        OsString::from("123:7:42"),
    ])
    .unwrap();
    assert_eq!(
        args.mode,
        StartupMode::Mux(vec![
            OsString::from("reconnect"),
            OsString::from("123:7:42"),
        ])
    );
    assert!(!should_handoff_to_existing_process(&args));
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
