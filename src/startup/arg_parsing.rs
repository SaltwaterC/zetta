use super::cli_help::{
    attention_help, help_text, is_version_argument, pane_help, pane_splits_help,
    parse_overlay_args, parse_pane_theme_args, parse_tab_icon_args,
    parse_terminal_resize_dimension, version_text,
};
use super::*;
use crate::cli_services::{NotificationRequest, parse_notification_timeout};
use crate::command_panes::{
    MAX_PANE_COMMAND_BYTES, PaneCommand, pane_command_byte_len, parse_pane_direction,
};
use crate::profile_cli::{ProfileCommand, parse_profile_args};
use crate::project_cli::{ProjectCommand, parse_project_args};
use crate::worktree_cli::{WorktreeCommand, parse_worktree_args};

const DEFAULT_PERFORMANCE_REPORT_DURATION: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartupMode {
    Application,
    Pane(PaneCommand),
    Attention(AttentionCommand),
    Worktree(WorktreeCommand),
    Project(ProjectCommand),
    #[cfg(cli_services)]
    CliService(CliServiceCommand),
    Profile(ProfileCommand),
    PrintShellIntegration(ShellIntegration),
    ConfigureCurrentShellIntegration,
    OutputBenchmark {
        size_mib: usize,
        output_type: OutputBenchmarkType,
    },
    PrintTerminalSize {
        json: bool,
        resize: Option<TerminalResize>,
    },
    ListBackgroundSessions {
        json: bool,
    },
    /// `zetta mux ...`, forwarded verbatim to the multiplexer so the subcommand
    /// and the `zmux` binary cannot accept different arguments.
    Mux(Vec<OsString>),
    ReconnectBackgroundSession {
        identifier: String,
    },
    SetTabIcon {
        icon: Option<IconName>,
    },
    ListTabIcons,
    SetPaneTheme {
        theme: Option<String>,
    },
    ListPaneThemes,
    ListPaneSplits,
    SetPaneOverlay(PaneOverlayRequest),
    #[cfg(windows)]
    RegisterWindowsShell(PathBuf),
    TerminalRenderingProfile,
    TerminalRenderingWorkload,
    TerminalCheckerboardWorkload,
    Edit {
        arguments: Vec<String>,
        delete_after: bool,
    },
    Vi(Vec<String>),
    TerminalSparseUpdateWorkload,
    TerminalAltScreenScrollWorkload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttentionCommand {
    pub(crate) notify: bool,
    pub(crate) notification: NotificationRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalResize {
    pub(crate) columns: Option<usize>,
    pub(crate) rows: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartupArgs {
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) keymap_path: Option<PathBuf>,
    pub(crate) profile: Option<String>,
    pub(crate) split: Option<String>,
    pub(crate) replace_pane: bool,
    /// Non-persistently overrides `profile`'s configured theme for this launch only.
    pub(crate) theme_override: Option<String>,
    pub(crate) mode: StartupMode,
    pub(crate) profile_report: Option<PathBuf>,
    pub(crate) profile_duration: Option<Duration>,
    pub(crate) profile_pane_stress: bool,
    /// The producer pattern the benchmark drives the renderer with. One
    /// pattern per run, so the mutually exclusive workload flags cannot be
    /// combined by construction.
    pub(crate) profile_workload: PerformanceWorkload,
    pub(crate) profile_external_terminal: bool,
    pub(crate) tftp_command: Option<TftpCommand>,
}

pub(crate) fn parse_attention_args(args: &[OsString]) -> Result<StartupArgs> {
    let mut notify = false;
    let mut app_name = None;
    let mut icon = None;
    let mut sound = None;
    let mut timeout = None;
    let mut positional = Vec::new();
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--notify" | "-n" => {
                anyhow::ensure!(!notify, "--notify may only be specified once");
                notify = true;
            }
            "--app-name" | "-a" => {
                anyhow::ensure!(app_name.is_none(), "--app-name may only be specified once");
                app_name = Some(
                    arguments
                        .next()
                        .context("--app-name requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--icon" | "-i" => {
                anyhow::ensure!(icon.is_none(), "--icon may only be specified once");
                icon = Some(
                    arguments
                        .next()
                        .context("--icon requires a path")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--sound" | "-s" => {
                anyhow::ensure!(sound.is_none(), "--sound may only be specified once");
                sound = Some(
                    arguments
                        .next()
                        .context("--sound requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--timeout" | "-t" => {
                anyhow::ensure!(timeout.is_none(), "--timeout may only be specified once");
                let value = arguments
                    .next()
                    .context("--timeout requires default, never, or a number of milliseconds")?
                    .to_string_lossy()
                    .into_owned();
                timeout = Some(parse_notification_timeout(&value)?);
            }
            "--help" | "-h" => anyhow::bail!("{}", attention_help()),
            option if option.starts_with('-') => {
                anyhow::bail!("unknown attention option {option:?}")
            }
            _ => positional.push(argument),
        }
    }

    anyhow::ensure!(
        (0..=2).contains(&positional.len()),
        "usage: zetta attention [OPTIONS] [SUMMARY] [BODY]; run `zetta attention --help` for details"
    );
    anyhow::ensure!(
        notify || (app_name.is_none() && icon.is_none() && sound.is_none() && timeout.is_none()),
        "--app-name, --icon, --sound, and --timeout require --notify"
    );
    if notify && !cfg!(feature = "notifications") {
        anyhow::bail!("desktop notifications are disabled in this build")
    }

    let summary = positional
        .first()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Attention required".to_owned());
    anyhow::ensure!(!summary.is_empty(), "SUMMARY must not be empty");
    let body = positional
        .get(1)
        .map(|value| value.to_string_lossy().into_owned());

    Ok(StartupArgs {
        config_path: None,
        keymap_path: None,
        profile: None,
        split: None,
        replace_pane: false,
        theme_override: None,
        mode: StartupMode::Attention(AttentionCommand {
            notify,
            notification: NotificationRequest {
                summary,
                body,
                app_name,
                icon,
                sound,
                timeout,
            },
        }),
        profile_report: None,
        profile_duration: None,
        profile_pane_stress: false,
        profile_workload: PerformanceWorkload::Standard,
        profile_external_terminal: false,
        tftp_command: None,
    })
}

pub(crate) fn parse_attention_target(process_id: &str, attention_id: &str) -> Result<(u32, u64)> {
    let process_id = process_id
        .parse::<u32>()
        .context("ZETTA_PROCESS_ID must be a positive process ID")?;
    anyhow::ensure!(
        process_id != 0,
        "ZETTA_PROCESS_ID must be a positive process ID"
    );
    let attention_id = attention_id
        .parse::<u64>()
        .context("ZETTA_ATTENTION_ID must be a positive attention ID")?;
    anyhow::ensure!(
        attention_id != 0,
        "ZETTA_ATTENTION_ID must be a positive attention ID"
    );
    Ok((process_id, attention_id))
}

pub(crate) fn parse_pane_args(args: &[OsString]) -> Result<PaneCommand> {
    let mut direction = None;
    let mut label = None;
    let mut pane = None;
    let mut overlay = None;
    let mut stack = false;
    let mut list = false;
    let mut command = Vec::new();
    let mut after_delimiter = false;
    let mut arguments = args.iter();

    while let Some(argument) = arguments.next() {
        let value = argument.to_string_lossy();
        if after_delimiter {
            command.push(value.into_owned());
            continue;
        }
        match value.as_ref() {
            "--" => after_delimiter = true,
            "--help" | "-h" => {
                println!("{}", pane_help());
                std::process::exit(0);
            }
            "--direction" | "-d" => {
                anyhow::ensure!(
                    direction.is_none(),
                    "--direction may only be specified once"
                );
                let value = arguments
                    .next()
                    .context("--direction requires left, right, up, or down")?
                    .to_string_lossy()
                    .into_owned();
                direction = Some(parse_pane_direction(&value).with_context(|| {
                    format!("unknown pane direction {value:?}; use left, right, up, or down")
                })?);
            }
            "--label" | "-l" => {
                anyhow::ensure!(label.is_none(), "--label may only be specified once");
                let value = arguments
                    .next()
                    .context("--label requires a non-empty pane label")?
                    .to_string_lossy()
                    .into_owned();
                anyhow::ensure!(!value.is_empty(), "--label requires a non-empty pane label");
                label = Some(value);
            }
            "--pane" | "-p" => {
                anyhow::ensure!(pane.is_none(), "--pane may only be specified once");
                let value = arguments
                    .next()
                    .context("--pane requires an existing pane label")?
                    .to_string_lossy()
                    .into_owned();
                anyhow::ensure!(!value.is_empty(), "--pane requires a non-empty pane label");
                pane = Some(value);
            }
            "--overlay" | "-o" => {
                let overlay = overlay.get_or_insert(PaneOverlayRequest {
                    text: None,
                    font_size: None,
                    opacity: None,
                    color: None,
                });
                anyhow::ensure!(
                    overlay.text.is_none(),
                    "--overlay may only be specified once"
                );
                overlay.text = Some(
                    arguments
                        .next()
                        .context("--overlay requires overlay text")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--overlay-size" | "-S" => {
                let overlay = overlay.get_or_insert(PaneOverlayRequest {
                    text: None,
                    font_size: None,
                    opacity: None,
                    color: None,
                });
                anyhow::ensure!(
                    overlay.font_size.is_none(),
                    "--overlay-size may only be specified once"
                );
                let value = arguments
                    .next()
                    .context("--overlay-size requires sm, base, lg, xl, 2xl, or 3xl")?
                    .to_string_lossy()
                    .into_owned();
                overlay.font_size = Some(OverlayFontSize::parse(&value).with_context(|| {
                    format!(
                        "unknown overlay size {value:?}; expected one of {}",
                        OverlayFontSize::CLI_NAMES.join(", ")
                    )
                })?);
            }
            "--overlay-opacity" | "-O" => {
                let overlay = overlay.get_or_insert(PaneOverlayRequest {
                    text: None,
                    font_size: None,
                    opacity: None,
                    color: None,
                });
                anyhow::ensure!(
                    overlay.opacity.is_none(),
                    "--overlay-opacity may only be specified once"
                );
                let value = arguments
                    .next()
                    .context("--overlay-opacity requires a percentage from 0 to 100")?
                    .to_string_lossy()
                    .into_owned();
                let percent = value.parse::<u8>().with_context(|| {
                    format!("--overlay-opacity {value:?} must be a whole number")
                })?;
                anyhow::ensure!(
                    percent <= 100,
                    "--overlay-opacity must be between 0 and 100"
                );
                overlay.opacity = Some(percent);
            }
            "--overlay-color" | "-c" => {
                let overlay = overlay.get_or_insert(PaneOverlayRequest {
                    text: None,
                    font_size: None,
                    opacity: None,
                    color: None,
                });
                anyhow::ensure!(
                    overlay.color.is_none(),
                    "--overlay-color may only be specified once"
                );
                let value = arguments
                    .next()
                    .context("--overlay-color requires a color name or hex color")?
                    .to_string_lossy()
                    .into_owned();
                anyhow::ensure!(
                    overlay_color_from_value(&value).is_some(),
                    "invalid overlay color {value:?}"
                );
                overlay.color = Some(value);
            }
            "--stack" | "-s" => {
                anyhow::ensure!(!stack, "--stack may only be specified once");
                stack = true;
            }
            "--list" | "-L" => {
                anyhow::ensure!(!list, "--list may only be specified once");
                list = true;
            }
            option if option.starts_with('-') => anyhow::bail!("unknown pane option {option:?}"),
            positional => anyhow::bail!(
                "pane command arguments must follow --; found positional argument {positional:?}"
            ),
        }
    }

    if list {
        anyhow::ensure!(
            direction.is_none(),
            "--list cannot be combined with --direction"
        );
        anyhow::ensure!(label.is_none(), "--list cannot be combined with --label");
        anyhow::ensure!(pane.is_none(), "--list cannot be combined with --pane");
        anyhow::ensure!(!stack, "--list cannot be combined with --stack");
        anyhow::ensure!(
            !after_delimiter && command.is_empty(),
            "--list cannot be combined with a command"
        );
    } else {
        anyhow::ensure!(after_delimiter, "pane commands require arguments after --");
        anyhow::ensure!(
            !command.is_empty(),
            "pane commands require a command after --"
        );
        anyhow::ensure!(
            pane_command_byte_len(&command) <= MAX_PANE_COMMAND_BYTES,
            "pane command exceeds the {} KiB limit",
            MAX_PANE_COMMAND_BYTES / 1024
        );
        anyhow::ensure!(
            label.is_none() || direction.is_some(),
            "--label requires --direction"
        );
        anyhow::ensure!(
            direction.is_none() || (pane.is_none() && !stack),
            "--direction cannot be combined with --pane or --stack"
        );
        anyhow::ensure!(
            overlay.is_none() || direction.is_some(),
            "--overlay options require --direction"
        );
        anyhow::ensure!(
            overlay
                .as_ref()
                .is_none_or(|overlay| overlay.text.is_some()),
            "an overlay requires --overlay TEXT"
        );
    }

    Ok(PaneCommand {
        direction,
        label,
        pane,
        overlay,
        stack,
        list,
        command,
    })
}

pub(crate) fn parse_args_from(args: impl IntoIterator<Item = OsString>) -> Result<StartupArgs> {
    let arguments = args.into_iter().collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "project")
    {
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::Project(parse_project_args(&arguments[1..])?),
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments.first().is_some_and(|argument| argument == "pane") {
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::Pane(parse_pane_args(&arguments[1..])?),
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments.first().is_some_and(|argument| argument == "wt") {
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::Worktree(parse_worktree_args(&arguments[1..])?),
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if let Some(profile_index) = profile_subcommand_index(&arguments) {
        let config_path = parse_profile_root_config(&arguments[..profile_index])?;
        let parsed = parse_profile_args(&arguments[profile_index + 1..], config_path)?;
        return Ok(StartupArgs {
            config_path: parsed.config_path,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::Profile(parsed.command),
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "tabicon")
    {
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: parse_tab_icon_args(&arguments[1..])?,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "panetheme")
    {
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: parse_pane_theme_args(&arguments[1..])?,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "splits")
    {
        if arguments[1..]
            .iter()
            .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
        {
            println!("{}", pane_splits_help());
            std::process::exit(0);
        }
        if let Some(argument) = arguments.get(1) {
            anyhow::bail!("unknown splits argument {argument:?}");
        }
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::ListPaneSplits,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "overlay")
    {
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: parse_overlay_args(&arguments[1..])?,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "attention")
    {
        if arguments[1..]
            .iter()
            .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
        {
            println!("{}", attention_help());
            std::process::exit(0);
        }
        return parse_attention_args(&arguments[1..]);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "benchmark-output")
    {
        let mut size_mib = None;
        let mut output_type = OutputBenchmarkType::RepeatedLines;
        let mut benchmark_arguments = arguments[1..].iter();
        while let Some(argument) = benchmark_arguments.next() {
            match argument.to_string_lossy().as_ref() {
                "--help" | "-h" => {
                    println!(
                        "Benchmark terminal output throughput\n\nUsage: zetta benchmark-output [OPTIONS]\n\nWrites deterministic text to standard output and prints the elapsed time to standard error.\n\nOptions:\n  -s, --size MIB                 Set the output size in MiB [default: 10]\n  -t, --output-type TYPE         Select repeated or unique lines [default: repeated]\n  -h, --help                     Print help"
                    );
                    std::process::exit(0);
                }
                "--size" | "-s" => {
                    anyhow::ensure!(size_mib.is_none(), "--size may only be specified once");
                    let value = benchmark_arguments
                        .next()
                        .context("--size requires a number of MiB")?
                        .to_string_lossy()
                        .parse::<usize>()
                        .context("--size must be a whole number of MiB")?;
                    anyhow::ensure!(value > 0, "--size must be greater than zero");
                    anyhow::ensure!(
                        value.checked_mul(MIB_BYTES).is_some(),
                        "--size is too large"
                    );
                    size_mib = Some(value);
                }
                "--output-type" | "-t" => {
                    let value = benchmark_arguments
                        .next()
                        .context("--output-type requires repeated or unique")?
                        .to_string_lossy();
                    output_type = match value.as_ref() {
                        "repeated" => OutputBenchmarkType::RepeatedLines,
                        "unique" => OutputBenchmarkType::UniqueLines,
                        _ => anyhow::bail!(
                            "--output-type must be either repeated or unique, got {value:?}"
                        ),
                    };
                }
                unknown => anyhow::bail!("unknown benchmark-output argument {unknown:?}"),
            }
        }
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::OutputBenchmark {
                size_mib: size_mib.unwrap_or(DEFAULT_OUTPUT_BENCHMARK_MIB),
                output_type,
            },
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "benchmark")
    {
        return parse_benchmark_args(&arguments[1..]);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "terminal-size")
    {
        let mut json = false;
        let mut resize = false;
        let mut columns = None;
        let mut rows = None;
        let mut terminal_size_arguments = arguments[1..].iter();
        while let Some(argument) = terminal_size_arguments.next() {
            match argument.to_string_lossy().as_ref() {
                "--json" | "-j" => json = true,
                "--resize" | "-r" => resize = true,
                "--columns" | "-c" => {
                    anyhow::ensure!(columns.is_none(), "--columns may only be specified once");
                    columns = Some(parse_terminal_resize_dimension(
                        terminal_size_arguments
                            .next()
                            .context("--columns requires a positive whole number")?,
                        "--columns",
                    )?);
                }
                "--rows" | "-R" => {
                    anyhow::ensure!(rows.is_none(), "--rows may only be specified once");
                    rows = Some(parse_terminal_resize_dimension(
                        terminal_size_arguments
                            .next()
                            .context("--rows requires a positive whole number")?,
                        "--rows",
                    )?);
                }
                "--help" | "-h" => {
                    println!(
                        "Print or resize the current terminal pane\n\nUsage: zetta terminal-size [--json | --resize [--columns COLUMNS] [--rows ROWS]]\n\nWithout --resize, prints the terminal width in columns and height in rows. With --resize, emits the xterm CSI 8 resize request for the current pane; an omitted dimension is kept unchanged.\n\nOptions:\n  -j, --json           Print machine-readable JSON\n  -r, --resize         Resize the current pane\n  -c, --columns COLUMNS Set the pane width in columns\n  -R, --rows ROWS       Set the pane height in rows\n  -h, --help           Print help"
                    );
                    std::process::exit(0);
                }
                unknown => anyhow::bail!("unknown terminal-size argument {unknown:?}"),
            }
        }
        anyhow::ensure!(!json || !resize, "--json cannot be used with --resize");
        anyhow::ensure!(
            resize || (columns.is_none() && rows.is_none()),
            "--columns and --rows require --resize"
        );
        anyhow::ensure!(
            !resize || columns.is_some() || rows.is_some(),
            "--resize requires --columns and/or --rows"
        );
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::PrintTerminalSize {
                json,
                resize: resize.then_some(TerminalResize { columns, rows }),
            },
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments.first().is_some_and(|argument| argument == "mux") {
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::Mux(arguments[1..].to_vec()),
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "sessions")
    {
        if arguments
            .get(1)
            .is_some_and(|argument| argument == "reconnect")
        {
            let mut identifier = None;
            let mut session_arguments = arguments[2..].iter();
            while let Some(argument) = session_arguments.next() {
                match argument.to_string_lossy().as_ref() {
                    "--session" | "-s" => {
                        anyhow::ensure!(
                            identifier.is_none(),
                            "--session may only be specified once"
                        );
                        identifier = Some(
                            session_arguments
                                .next()
                                .context("--session requires a session ID")?
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                    "--help" | "-h" => {
                        println!(
                            "Reconnect a detached Zetta session\n\nUsage: zetta sessions reconnect SESSION_ID\n\nSESSION_ID is the PROCESS:RUNNER:SESSION identifier printed by `zetta sessions`. Protected sessions prompt for their secret without echoing it or placing it in shell history. A bare SESSION value is accepted only when it is unique.\n\nOptions:\n  -s, --session SESSION_ID  Specify the session ID as an option\n  -h, --help                Print help"
                        );
                        std::process::exit(0);
                    }
                    value if !value.starts_with('-') => {
                        anyhow::ensure!(
                            identifier.is_none(),
                            "only one session ID may be specified"
                        );
                        identifier = Some(value.to_owned());
                    }
                    unknown => anyhow::bail!("unknown sessions reconnect argument {unknown:?}"),
                }
            }
            return Ok(StartupArgs {
                config_path: None,
                keymap_path: None,
                profile: None,
                split: None,
                replace_pane: false,
                theme_override: None,
                mode: StartupMode::ReconnectBackgroundSession {
                    identifier: identifier.context(
                        "sessions reconnect requires a session ID; run `zetta sessions reconnect --help` for usage",
                    )?,
                },
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_workload: PerformanceWorkload::Standard,
                profile_external_terminal: false,
                tftp_command: None,
            });
        }
        let mut json = false;
        for argument in &arguments[1..] {
            match argument.to_string_lossy().as_ref() {
                "--json" | "-j" => json = true,
                "--help" | "-h" => {
                    println!(
                        "List or reconnect detached Zetta sessions\n\nUsage: zetta sessions [--json]\n       zetta sessions reconnect SESSION_ID\n\nOptions:\n  -j, --json  Print machine-readable JSON\n  -h, --help  Print help\n\nRun `zetta sessions reconnect --help` for reconnect options."
                    );
                    std::process::exit(0);
                }
                unknown => anyhow::bail!("unknown sessions argument {unknown:?}"),
            }
        }
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::ListBackgroundSessions { json },
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments.first().is_some_and(|argument| argument == "edit") {
        let editor_arguments = arguments[1..].iter();
        let mut delete_after = false;
        let mut paths = Vec::new();
        let mut options = true;
        for argument in editor_arguments {
            match argument.to_string_lossy().as_ref() {
                "--" if options => options = false,
                "--help" | "-h" if options => {
                    println!(
                        "Edit files with the pane's configured editor\n\nUsage: zetta edit [OPTIONS] [--] FILE ...\n\nUses EDITOR from the current environment. If EDITOR is unset or empty, Zetta's built-in vi is used.\n\nOptions:\n  -d, --delete-after             Delete a managed scrollback file after editing\n  -h, --help                     Print help"
                    );
                    std::process::exit(0);
                }
                "--delete-after" | "-d" if options => {
                    anyhow::ensure!(!delete_after, "--delete-after may only be specified once");
                    delete_after = true;
                }
                option if options && option.starts_with('-') => {
                    anyhow::bail!("unknown edit option {option:?}")
                }
                _ => paths.push(argument.to_string_lossy().into_owned()),
            }
        }
        anyhow::ensure!(!paths.is_empty(), "zetta edit requires at least one file");
        anyhow::ensure!(
            !delete_after || paths.len() == 1,
            "--delete-after requires exactly one managed scrollback file"
        );
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::Edit {
                arguments: paths,
                delete_after,
            },
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments.first().is_some_and(|argument| argument == "vi") {
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::Vi(
                arguments[1..]
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect(),
            ),
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments.first().is_some_and(|argument| argument == "init") {
        let integration_arguments = &arguments[1..];
        if integration_arguments
            .iter()
            .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
        {
            println!("{}", shell_integration_help());
            std::process::exit(0);
        }
        anyhow::ensure!(
            integration_arguments.len() <= 1,
            "usage: zetta init [SHELL]; run `zetta init --help` for supported shells"
        );
        if integration_arguments.is_empty() {
            return Ok(StartupArgs {
                config_path: None,
                keymap_path: None,
                profile: None,
                split: None,
                replace_pane: false,
                theme_override: None,
                mode: StartupMode::ConfigureCurrentShellIntegration,
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_workload: PerformanceWorkload::Standard,
                profile_external_terminal: false,
                tftp_command: None,
            });
        }
        let shell = integration_arguments[0]
            .to_str()
            .context("SHELL must be valid UTF-8")?;
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::PrintShellIntegration(ShellIntegration::parse(shell)?),
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "serial")
    {
        #[cfg(feature = "serial-console")]
        {
            let serial_arguments = &arguments[1..];
            if serial_arguments
                .iter()
                .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
            {
                println!("{}", serial_help());
                std::process::exit(0);
            }
            return Ok(StartupArgs {
                config_path: None,
                keymap_path: None,
                profile: None,
                split: None,
                replace_pane: false,
                theme_override: None,
                mode: StartupMode::CliService(parse_serial_args(serial_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_workload: PerformanceWorkload::Standard,
                profile_external_terminal: false,
                tftp_command: None,
            });
        }
        #[cfg(not(feature = "serial-console"))]
        anyhow::bail!("Serial console support is disabled in this build");
    }
    if arguments.first().is_some_and(|argument| argument == "http") {
        #[cfg(feature = "http-server")]
        {
            let http_arguments = &arguments[1..];
            if http_arguments
                .iter()
                .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
            {
                println!("{}", http_server_help());
                std::process::exit(0);
            }
            return Ok(StartupArgs {
                config_path: None,
                keymap_path: None,
                profile: None,
                split: None,
                replace_pane: false,
                theme_override: None,
                mode: StartupMode::CliService(parse_http_args(http_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_workload: PerformanceWorkload::Standard,
                profile_external_terminal: false,
                tftp_command: None,
            });
        }
        #[cfg(not(feature = "http-server"))]
        anyhow::bail!("HTTP server support is disabled in this build");
    }
    if arguments.first().is_some_and(|argument| argument == "tftp") {
        let tftp_arguments = &arguments[1..];
        if tftp_arguments
            .first()
            .is_some_and(|argument| argument == "server")
        {
            #[cfg(feature = "tftp-server")]
            {
                let server_arguments = &tftp_arguments[1..];
                if server_arguments
                    .iter()
                    .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
                {
                    println!("{}", tftp_server_help());
                    std::process::exit(0);
                }
                return Ok(StartupArgs {
                    config_path: None,
                    keymap_path: None,
                    profile: None,
                    split: None,
                    replace_pane: false,
                    theme_override: None,
                    mode: StartupMode::CliService(parse_tftp_server_args(
                        server_arguments.iter().cloned(),
                    )?),
                    profile_report: None,
                    profile_duration: None,
                    profile_pane_stress: false,
                    profile_workload: PerformanceWorkload::Standard,
                    profile_external_terminal: false,
                    tftp_command: None,
                });
            }
            #[cfg(not(feature = "tftp-server"))]
            anyhow::bail!("TFTP server support is disabled in this build");
        }
        if tftp_arguments
            .iter()
            .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
        {
            println!("{}", tftp_help());
            std::process::exit(0);
        }
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            mode: StartupMode::Application,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: Some(parse_tftp_args(tftp_arguments.iter().cloned())?),
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "notify")
    {
        #[cfg(feature = "notifications")]
        {
            let notify_arguments = &arguments[1..];
            if notify_arguments
                .iter()
                .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
            {
                println!("{}", notify_help());
                std::process::exit(0);
            }
            return Ok(StartupArgs {
                config_path: None,
                keymap_path: None,
                profile: None,
                split: None,
                replace_pane: false,
                theme_override: None,
                mode: StartupMode::CliService(parse_notify_args(notify_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_workload: PerformanceWorkload::Standard,
                profile_external_terminal: false,
                tftp_command: None,
            });
        }
        #[cfg(not(feature = "notifications"))]
        anyhow::bail!("Desktop notification support is disabled in this build");
    }
    if arguments.first().is_some_and(|argument| argument == "copy") {
        #[cfg(feature = "clipboard")]
        {
            let copy_arguments = &arguments[1..];
            if copy_arguments.iter().any(|argument| {
                matches!(
                    argument.to_string_lossy().as_ref(),
                    "--help" | "-h" | "-help"
                )
            }) {
                println!("{}", copy_help());
                std::process::exit(0);
            }
            return Ok(StartupArgs {
                config_path: None,
                keymap_path: None,
                profile: None,
                split: None,
                replace_pane: false,
                theme_override: None,
                mode: StartupMode::CliService(parse_copy_args(copy_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_workload: PerformanceWorkload::Standard,
                profile_external_terminal: false,
                tftp_command: None,
            });
        }
        #[cfg(not(feature = "clipboard"))]
        anyhow::bail!("Clipboard support is disabled in this build");
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "paste")
    {
        #[cfg(feature = "clipboard")]
        {
            let paste_arguments = &arguments[1..];
            if paste_arguments.iter().any(|argument| {
                matches!(
                    argument.to_string_lossy().as_ref(),
                    "--help" | "-h" | "-help"
                )
            }) {
                println!("{}", paste_help());
                std::process::exit(0);
            }
            return Ok(StartupArgs {
                config_path: None,
                keymap_path: None,
                profile: None,
                split: None,
                replace_pane: false,
                theme_override: None,
                mode: StartupMode::CliService(parse_paste_args(paste_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_workload: PerformanceWorkload::Standard,
                profile_external_terminal: false,
                tftp_command: None,
            });
        }
        #[cfg(not(feature = "clipboard"))]
        anyhow::bail!("Clipboard support is disabled in this build");
    }
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        let config_path = arguments
            .windows(2)
            .find(|arguments| matches!(arguments[0].to_string_lossy().as_ref(), "--config" | "-c"))
            .map(|arguments| PathBuf::from(&arguments[1]));
        let (config, _) = load_startup_config(config_path.as_deref(), None);
        println!("{}", help_text(&config.profiles));
        std::process::exit(0);
    }
    let mut config = None;
    let mut keymap = None;
    let mut profile = None;
    let mut split = None;
    let mut replace_pane = false;
    let mut theme_override = None;
    #[cfg(windows)]
    let mut mode = StartupMode::Application;
    #[cfg(not(windows))]
    let mode = StartupMode::Application;
    let mut args = arguments.into_iter();
    while let Some(argument) = args.next() {
        let argument = argument.to_string_lossy();
        if is_version_argument(&argument) {
            println!("{}", version_text());
            std::process::exit(0);
        }
        match argument.as_ref() {
            "--config" | "-c" => {
                config = Some(args.next().context("--config requires a path")?.into())
            }
            "--keymap" | "-k" => {
                keymap = Some(args.next().context("--keymap requires a path")?.into())
            }
            "--profile" | "-p" => {
                profile = Some(
                    args.next()
                        .context("--profile requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                )
            }
            "--split" | "-s" => {
                let name = args
                    .next()
                    .context("--split requires a template name")?
                    .to_string_lossy()
                    .into_owned();
                anyhow::ensure!(
                    !name.is_empty() && !name.starts_with('-'),
                    "--split requires a template name"
                );
                split = Some(name);
            }
            "--replace-pane" | "-r" => {
                anyhow::ensure!(!replace_pane, "--replace-pane may only be specified once");
                replace_pane = true;
            }
            "--theme" | "-t" => {
                theme_override = Some(
                    args.next()
                        .context("--theme requires a theme name")?
                        .to_string_lossy()
                        .into_owned(),
                )
            }
            #[cfg(windows)]
            // Hidden: written into the Start menu shortcut by the installer,
            // not something a user types.
            "--register-windows-shell" => {
                mode = StartupMode::RegisterWindowsShell(
                    args.next()
                        .context("--register-windows-shell requires a shortcut path")?
                        .into(),
                )
            }
            "--help" | "-h" => unreachable!("help arguments return before parsing options"),
            unknown => anyhow::bail!("unknown argument {unknown:?}"),
        }
    }
    anyhow::ensure!(
        profile.is_none() || mode == StartupMode::Application,
        "--profile cannot be combined with another startup mode"
    );
    anyhow::ensure!(
        split.is_none() || mode == StartupMode::Application,
        "--split cannot be combined with another startup mode"
    );
    anyhow::ensure!(
        !replace_pane || mode == StartupMode::Application,
        "--replace-pane cannot be combined with another startup mode"
    );
    anyhow::ensure!(
        !replace_pane || split.is_some() || profile.is_some(),
        "--replace-pane requires --split or --profile"
    );
    anyhow::ensure!(
        theme_override.is_none() || profile.is_some(),
        "--theme requires --profile"
    );
    Ok(StartupArgs {
        config_path: config,
        keymap_path: keymap,
        profile,
        split,
        replace_pane,
        theme_override,
        mode,
        profile_report: None,
        profile_duration: None,
        profile_pane_stress: false,
        profile_workload: PerformanceWorkload::Standard,
        profile_external_terminal: false,
        tftp_command: None,
    })
}

/// Records which producer pattern the benchmark should run. The workload flags
/// select one pattern between them, so this remembers which flag asked for
/// which and rejects a second, different request rather than letting the last
/// flag on the command line silently win.
fn select_benchmark_workload(
    selected: &mut Option<(String, PerformanceWorkload)>,
    flag: &str,
    workload: PerformanceWorkload,
) -> Result<()> {
    if let Some((selected_flag, selected_workload)) = selected
        && *selected_workload != workload
    {
        anyhow::bail!("{selected_flag} cannot be combined with {flag}");
    }
    *selected = Some((flag.to_owned(), workload));
    Ok(())
}

fn parse_benchmark_args(arguments: &[OsString]) -> Result<StartupArgs> {
    let mut mode = StartupMode::TerminalRenderingProfile;
    let mut profile_report = None;
    let mut profile_duration = None;
    let mut profile_pane_stress = false;
    let mut profile_workload = None;
    let mut profile_external_terminal = false;
    let mut args = arguments.iter();
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--profile-pane-stress" | "-s" => profile_pane_stress = true,
            flag @ ("--profile-background-stress" | "-b") => select_benchmark_workload(
                &mut profile_workload,
                flag,
                PerformanceWorkload::CheckerboardBackground,
            )?,
            flag @ ("--profile-sparse-updates" | "-u") => select_benchmark_workload(
                &mut profile_workload,
                flag,
                PerformanceWorkload::SparseUpdates,
            )?,
            flag @ ("--profile-alt-screen-scroll" | "-a") => select_benchmark_workload(
                &mut profile_workload,
                flag,
                PerformanceWorkload::AltScreenScroll,
            )?,
            "--profile-external-terminal" | "-x" => profile_external_terminal = true,
            // Hidden, and long-only for that reason: these are the child half of
            // `--profile-external-terminal`, which `startup.rs` spells out when
            // it launches the producer in the user's own terminal. The
            // documented way to choose a workload is `--profile-background-
            // stress` and its siblings, which select the same producers.
            "--terminal-render-workload" => mode = StartupMode::TerminalRenderingWorkload,
            "--terminal-checkerboard-workload" => mode = StartupMode::TerminalCheckerboardWorkload,
            "--terminal-sparse-update-workload" => mode = StartupMode::TerminalSparseUpdateWorkload,
            "--terminal-alt-screen-scroll-workload" => {
                mode = StartupMode::TerminalAltScreenScrollWorkload
            }
            "--profile-report" | "-r" => {
                profile_report = Some(
                    args.next()
                        .context("--profile-report requires a path")?
                        .into(),
                )
            }
            "--profile-duration" | "-d" => {
                let seconds = args
                    .next()
                    .context("--profile-duration requires seconds")?
                    .to_string_lossy()
                    .parse::<f64>()
                    .context("--profile-duration must be a number of seconds")?;
                anyhow::ensure!(
                    seconds.is_finite() && seconds > 0.0,
                    "--profile-duration must be greater than zero"
                );
                profile_duration = Some(Duration::from_secs_f64(seconds));
            }
            "--help" | "-h" => {
                println!(
                    "Benchmark terminal rendering\n\nUsage: zetta benchmark [OPTIONS]\n\nThe workload options select one producer pattern and cannot be combined with\neach other. Without one, the standard text and line-drawing workload runs.\n\nOptions:\n  -s, --profile-pane-stress           Use four visible producer panes\n  -b, --profile-background-stress     Render alternating cell backgrounds\n  -u, --profile-sparse-updates        Update a dense terminal at 40 Hz\n  -a, --profile-alt-screen-scroll     Scroll a colourised diff on the alternate screen\n  -x, --profile-external-terminal     Run the workload in the current terminal\n  -r, --profile-report PATH           Write a profiling report\n  -d, --profile-duration SECONDS      Set the profiling duration\n  -h, --help                          Print help"
                );
                std::process::exit(0);
            }
            unknown => anyhow::bail!("unknown benchmark argument {unknown:?}"),
        }
    }
    anyhow::ensure!(
        !(profile_external_terminal && profile_report.is_some()),
        "--profile-external-terminal cannot be combined with --profile-report"
    );
    anyhow::ensure!(
        !(profile_external_terminal && profile_pane_stress),
        "--profile-external-terminal cannot be combined with --profile-pane-stress"
    );
    anyhow::ensure!(
        !profile_external_terminal || profile_duration.is_some(),
        "--profile-external-terminal requires --profile-duration"
    );
    anyhow::ensure!(
        profile_duration.is_none() || profile_report.is_some() || profile_external_terminal,
        "--profile-duration requires --profile-report or --profile-external-terminal"
    );
    if profile_report.is_some() && profile_duration.is_none() {
        profile_duration = Some(DEFAULT_PERFORMANCE_REPORT_DURATION);
    }
    Ok(StartupArgs {
        config_path: None,
        keymap_path: None,
        profile: None,
        split: None,
        replace_pane: false,
        theme_override: None,
        mode,
        profile_report,
        profile_duration,
        profile_pane_stress,
        profile_workload: profile_workload
            .map(|(_, workload)| workload)
            .unwrap_or_default(),
        profile_external_terminal,
        tftp_command: None,
    })
}

pub(crate) fn select_launch_profile(
    config: &Config,
    requested: Option<&str>,
) -> Result<Option<Profile>> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    config
        .profiles
        .iter()
        .find(|profile| profile.name.eq_ignore_ascii_case(requested))
        .cloned()
        .map(Some)
        .with_context(|| {
            let available = config
                .profiles
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("profile {requested:?} is not available; available profiles: {available}")
        })
}

pub(crate) fn configured_split_names(config: &Config) -> Vec<String> {
    let mut names = config
        .pane_split_templates
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

pub(crate) fn validate_launch_split(config: &Config, requested: Option<&str>) -> Result<()> {
    let Some(requested) = requested else {
        return Ok(());
    };
    anyhow::ensure!(
        config.pane_split_templates.contains_key(requested),
        "pane split template {requested:?} is not configured; available pane split templates: {}",
        configured_split_names(config).join(", ")
    );
    Ok(())
}

pub(crate) fn parse_args() -> Result<StartupArgs> {
    parse_args_from(env::args_os().skip(1))
}

fn profile_subcommand_index(arguments: &[OsString]) -> Option<usize> {
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].to_string_lossy().as_ref() {
            "--config" | "-c" | "--keymap" | "-k" | "--profile" | "-p" | "--split" | "-s"
            | "--theme" | "-t" => {
                index = index.checked_add(2)?;
            }
            "--replace-pane" | "-r" => index += 1,
            "--help" | "-h" | "--version" | "-v" => index += 1,
            "profile" => return Some(index),
            _ => return None,
        }
    }
    None
}

fn parse_profile_root_config(arguments: &[OsString]) -> Result<Option<PathBuf>> {
    let mut config_path = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--config" | "-c" => {
                anyhow::ensure!(
                    config_path.is_none(),
                    "--config may only be specified once for zetta profile"
                );
                config_path = Some(arguments.next().context("--config requires a path")?.into());
            }
            "--help" | "-h" => {}
            unknown => anyhow::bail!(
                "unknown root argument {unknown:?} before zetta profile; use --config/-c"
            ),
        }
    }
    Ok(config_path)
}

pub(crate) fn should_handoff_to_existing_process(args: &StartupArgs) -> bool {
    args.mode == StartupMode::Application
        && args.config_path.is_none()
        && args.keymap_path.is_none()
        && !args.replace_pane
        && args.profile.is_none()
        && args.split.is_none()
}

pub(crate) fn should_replace_pane_in_existing_process(args: &StartupArgs) -> bool {
    args.mode == StartupMode::Application
        && args.replace_pane
        && args.config_path.is_none()
        && args.keymap_path.is_none()
}

fn path_with_entry_first(path: Option<&std::ffi::OsStr>, entry: &Path) -> Option<OsString> {
    let inherited = path.map(env::split_paths).into_iter().flatten();
    let entries = inherited.collect::<Vec<_>>();
    let entry_text = entry.to_string_lossy();
    if entries.iter().any(|candidate| {
        let candidate_text = candidate.to_string_lossy();
        let candidate = candidate_text.trim_end_matches(['\\', '/']);
        let entry = entry_text.trim_end_matches(['\\', '/']);
        if cfg!(windows) {
            candidate.eq_ignore_ascii_case(entry)
        } else {
            candidate == entry
        }
    }) {
        return None;
    }
    env::join_paths(std::iter::once(entry.to_path_buf()).chain(entries)).ok()
}

pub(crate) fn native_terminal_environment() -> Vec<(String, String)> {
    let mut environment = Vec::new();
    // A native Linux Zetta can be launched from a Windows-hosted WSL pane.
    // Do not let that pane's host-routing marker leak into terminals owned by
    // the native application; its executable directory below must win there.
    #[cfg(not(windows))]
    environment.push(("ZETTA_HOST_EXECUTABLE".to_owned(), String::new()));

    let Some(executable_directory) = env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
    else {
        return environment;
    };
    let Some(path) = path_with_entry_first(env::var_os("PATH").as_deref(), &executable_directory)
    else {
        return environment;
    };
    environment.push(("PATH".to_owned(), path.to_string_lossy().into_owned()));
    environment
}

pub(crate) fn load_startup_config(
    config_path: Option<&Path>,
    keymap_path: Option<PathBuf>,
) -> (Config, Option<String>) {
    match Config::load(config_path, keymap_path.clone()) {
        Ok(config) => (config, None),
        Err(error) => (
            Config::defaults(config_path, keymap_path),
            Some(format!("Could not load configuration: {error:#}")),
        ),
    }
}

#[cfg(test)]
#[path = "../tests/startup/arg_parsing.rs"]
mod tests;
