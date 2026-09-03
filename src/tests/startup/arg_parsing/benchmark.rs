use super::*;

#[test]
fn output_benchmark_subcommand_bypasses_application_startup() {
    let args = parse_args_from([OsString::from("benchmark"), OsString::from("output")]).unwrap();

    assert_eq!(
        args.mode,
        StartupMode::OutputBenchmark {
            size_mib: DEFAULT_OUTPUT_BENCHMARK_MIB,
            output_type: OutputBenchmarkType::RepeatedLines,
        }
    );
    assert!(!should_handoff_to_existing_process(&args));

    let sized = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("output"),
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
        OsString::from("benchmark"),
        OsString::from("output"),
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
        OsString::from("benchmark"),
        OsString::from("output"),
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
        OsString::from("benchmark"),
        OsString::from("output"),
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
                OsString::from("benchmark"),
                OsString::from("output"),
                OsString::from("--size"),
                OsString::from(invalid),
            ])
            .is_err()
        );
    }
    assert!(
        parse_args_from([
            OsString::from("benchmark"),
            OsString::from("output"),
            OsString::from("--size"),
        ])
        .is_err()
    );
    assert!(
        parse_args_from([
            OsString::from("benchmark"),
            OsString::from("output"),
            OsString::from("--unknown"),
        ])
        .is_err()
    );
    for invalid in ["different", ""] {
        assert!(
            parse_args_from([
                OsString::from("benchmark"),
                OsString::from("output"),
                OsString::from("--output-type"),
                OsString::from(invalid),
            ])
            .is_err()
        );
    }
    assert!(
        parse_args_from([
            OsString::from("benchmark"),
            OsString::from("output"),
            OsString::from("--output-type"),
        ])
        .is_err()
    );
    assert!(parse_args_from([OsString::from("benchmark-output")]).is_err());
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
            no_mux: false,
            mode: StartupMode::TerminalRenderingProfile,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
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
            no_mux: false,
            mode: StartupMode::TerminalRenderingWorkload,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
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
    assert_eq!(
        args.profile_workload,
        PerformanceWorkload::CheckerboardBackground
    );

    assert!(parse_args_from([OsString::from("--profile-background-stress")]).is_err());
}

#[test]
fn sparse_updates_are_a_benchmark_option() {
    let args = parse_args_from([
        OsString::from("benchmark"),
        OsString::from("--profile-sparse-updates"),
    ])
    .unwrap();
    assert_eq!(args.profile_workload, PerformanceWorkload::SparseUpdates);

    assert!(parse_args_from([OsString::from("--profile-sparse-updates")]).is_err());
}

#[test]
fn alt_screen_scroll_is_a_benchmark_option() {
    for flag in ["--profile-alt-screen-scroll", "-a"] {
        let args = parse_args_from([OsString::from("benchmark"), OsString::from(flag)]).unwrap();
        assert_eq!(args.profile_workload, PerformanceWorkload::AltScreenScroll);
    }

    assert!(parse_args_from([OsString::from("--profile-alt-screen-scroll")]).is_err());

    assert_eq!(
        parse_args_from([
            OsString::from("benchmark"),
            OsString::from("--terminal-alt-screen-scroll-workload"),
        ])
        .unwrap()
        .mode,
        StartupMode::TerminalAltScreenScrollWorkload
    );
}

#[test]
fn benchmark_workload_options_are_mutually_exclusive() {
    // Every pair, so adding a fourth workload cannot quietly go unguarded.
    let flags = [
        "--profile-background-stress",
        "--profile-sparse-updates",
        "--profile-alt-screen-scroll",
    ];
    for (index, first) in flags.iter().enumerate() {
        for second in flags.iter().skip(index + 1) {
            let error = parse_args_from([
                OsString::from("benchmark"),
                OsString::from(*first),
                OsString::from(*second),
            ])
            .unwrap_err();
            assert!(
                error.to_string().contains("cannot be combined"),
                "{first} with {second}: {error}"
            );
        }
        // The same workload asked for twice is not a conflict.
        parse_args_from([
            OsString::from("benchmark"),
            OsString::from(*first),
            OsString::from(*first),
        ])
        .unwrap();
    }
}

#[test]
fn benchmark_defaults_to_the_standard_workload() {
    let args = parse_args_from([OsString::from("benchmark")]).unwrap();
    assert_eq!(args.profile_workload, PerformanceWorkload::Standard);
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
