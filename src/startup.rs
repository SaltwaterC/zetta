use super::*;
#[cfg(any(
    feature = "serial-console",
    feature = "http-server",
    feature = "tftp-server",
    feature = "notifications"
))]
use crate::cli_services::CliServiceCommand;
#[cfg(feature = "clipboard")]
use crate::cli_services::{copy_help, parse_copy_args, parse_paste_args, paste_help};
#[cfg(feature = "http-server")]
use crate::cli_services::{http_server_help, parse_http_args};
#[cfg(feature = "notifications")]
use crate::cli_services::{notify_help, parse_notify_args};
#[cfg(feature = "serial-console")]
use crate::cli_services::{parse_serial_args, serial_help};
#[cfg(feature = "tftp-server")]
use crate::cli_services::{parse_tftp_server_args, tftp_server_help};

#[cfg(not(feature = "tftp-client"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TftpCommand;

#[cfg(not(feature = "tftp-client"))]
impl TftpCommand {
    pub(crate) fn run(&self) -> Result<()> {
        anyhow::bail!("TFTP support is disabled in this build")
    }
}

#[cfg(not(feature = "tftp-client"))]
pub(crate) fn tftp_help() -> &'static str {
    #[cfg(feature = "tftp-server")]
    {
        "Zetta TFTP server\n\nUsage: zetta tftp server [OPTIONS]\n\nRun `zetta tftp server --help` for server options."
    }
    #[cfg(not(feature = "tftp-server"))]
    {
        "TFTP support is disabled in this build"
    }
}

#[cfg(not(feature = "tftp-client"))]
pub(crate) fn parse_tftp_args(_: impl IntoIterator<Item = OsString>) -> Result<TftpCommand> {
    anyhow::bail!("TFTP support is disabled in this build")
}

const DEFAULT_PERFORMANCE_REPORT_DURATION: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartupMode {
    Application,
    #[cfg(any(
        feature = "serial-console",
        feature = "http-server",
        feature = "tftp-server",
        feature = "notifications",
        feature = "clipboard"
    ))]
    CliService(CliServiceCommand),
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
    #[cfg(windows)]
    RegisterWindowsShell(PathBuf),
    TerminalRenderingProfile,
    TerminalRenderingWorkload,
    TerminalCheckerboardWorkload,
    TerminalSparseUpdateWorkload,
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
    pub(crate) mode: StartupMode,
    pub(crate) profile_report: Option<PathBuf>,
    pub(crate) profile_duration: Option<Duration>,
    pub(crate) profile_pane_stress: bool,
    pub(crate) profile_background_stress: bool,
    pub(crate) profile_sparse_updates: bool,
    pub(crate) profile_external_terminal: bool,
    pub(crate) tftp_command: Option<TftpCommand>,
}

pub(crate) fn version_text() -> String {
    format!("Zetta {}", env!("CARGO_PKG_VERSION"))
}

pub(crate) fn help_text(profiles: &[Profile]) -> String {
    let mut features = vec!["Terminal emulator"];
    #[cfg(feature = "wayland")]
    features.push("Wayland backend");
    #[cfg(feature = "x11")]
    features.push("X11 backend");
    #[cfg(feature = "serial-console")]
    features.push("Serial console");
    #[cfg(feature = "http-server")]
    features.push("HTTP server");
    #[cfg(feature = "tftp-server")]
    features.push("TFTP server");
    #[cfg(feature = "tftp-client")]
    features.push("TFTP client");
    #[cfg(feature = "notifications")]
    features.push("Desktop notifications");
    #[cfg(feature = "clipboard")]
    features.push("Clipboard access");

    let serial_usage = if cfg!(feature = "serial-console") {
        "\n       zetta serial <COMMAND>"
    } else {
        ""
    };
    let http_usage = if cfg!(feature = "http-server") {
        "\n       zetta http server [OPTIONS]"
    } else {
        ""
    };
    let tftp_usage = if cfg!(any(feature = "tftp-client", feature = "tftp-server")) {
        "\n       zetta tftp <COMMAND>"
    } else {
        ""
    };
    let notify_usage = if cfg!(feature = "notifications") {
        "\n       zetta notify [OPTIONS] SUMMARY [BODY]"
    } else {
        ""
    };
    let clipboard_usage = if cfg!(feature = "clipboard") {
        "\n       zetta copy [OPTIONS]\n       zetta paste [OPTIONS]"
    } else {
        ""
    };
    let serial_command = if cfg!(feature = "serial-console") {
        "\n  serial                              List or connect to serial devices"
    } else {
        ""
    };
    let http_command = if cfg!(feature = "http-server") {
        "\n  http server                         Serve static files over HTTP"
    } else {
        ""
    };
    let tftp_command = if cfg!(any(feature = "tftp-client", feature = "tftp-server")) {
        "\n  tftp                                Transfer files or serve them with TFTP"
    } else {
        ""
    };
    let notify_command = if cfg!(feature = "notifications") {
        "\n  notify                              Show a desktop notification"
    } else {
        ""
    };
    let clipboard_command = if cfg!(feature = "clipboard") {
        "\n  copy                                Copy standard input to the clipboard\n  paste                                Print the clipboard's contents"
    } else {
        ""
    };
    let profiles = profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<Vec<_>>()
        .join("\n  ");
    format!(
        "Zetta Terminal\n\nUsage: zetta [OPTIONS]\n       zetta benchmark [OPTIONS]\n       zetta benchmark-output [OPTIONS]\n       zetta terminal-size [--json | --resize [--columns COLUMNS] [--rows ROWS]]\n       zetta sessions [--json]\n       zetta init [SHELL]{serial_usage}{http_usage}{tftp_usage}{notify_usage}{clipboard_usage}\n\nCommands:\n  benchmark                           Profile terminal rendering\n  benchmark-output                    Write and time a text payload (default: 10 MiB)\n  terminal-size                       Print or resize the current terminal pane\n  sessions                            List detached background sessions\n  init                                Configure or generate shell integration{serial_command}{http_command}{tftp_command}{notify_command}{clipboard_command}\n\nBuilt-in features:\n  {}\n\nProfiles accepted by --profile NAME (case-insensitive):\n  {profiles}\n\nOptions:\n  -h, --help                          Print help\n  -v, --version                       Print version\n  -c, --config PATH                   Use a configuration file\n  -k, --keymap PATH                   Use a keymap file\n  -p, --profile NAME                  Select one of the profiles listed above",
        features.join("\n  "),
    )
}

fn is_version_argument(argument: &str) -> bool {
    matches!(argument, "--version" | "-v")
}

fn parse_terminal_resize_dimension(argument: &OsString, option: &str) -> Result<usize> {
    let value = argument
        .to_string_lossy()
        .parse::<usize>()
        .with_context(|| format!("{option} must be a positive whole number"))?;
    anyhow::ensure!(value > 0, "{option} must be greater than zero");
    anyhow::ensure!(
        value <= usize::from(u16::MAX),
        "{option} must not exceed {}",
        u16::MAX
    );
    Ok(value)
}

pub(crate) fn parse_args_from(args: impl IntoIterator<Item = OsString>) -> Result<StartupArgs> {
    let arguments = args.into_iter().collect::<Vec<_>>();
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
            mode: StartupMode::OutputBenchmark {
                size_mib: size_mib.unwrap_or(DEFAULT_OUTPUT_BENCHMARK_MIB),
                output_type,
            },
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_background_stress: false,
            profile_sparse_updates: false,
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
            mode: StartupMode::PrintTerminalSize {
                json,
                resize: resize.then_some(TerminalResize { columns, rows }),
            },
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_background_stress: false,
            profile_sparse_updates: false,
            profile_external_terminal: false,
            tftp_command: None,
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "sessions")
    {
        let mut json = false;
        for argument in &arguments[1..] {
            match argument.to_string_lossy().as_ref() {
                "--json" | "-j" => json = true,
                "--help" | "-h" => {
                    println!(
                        "List detached Zetta sessions\n\nUsage: zetta sessions [--json]\n\nOptions:\n  -j, --json  Print machine-readable JSON\n  -h, --help  Print help"
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
            mode: StartupMode::ListBackgroundSessions { json },
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_background_stress: false,
            profile_sparse_updates: false,
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
                mode: StartupMode::ConfigureCurrentShellIntegration,
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_background_stress: false,
                profile_sparse_updates: false,
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
            mode: StartupMode::PrintShellIntegration(ShellIntegration::parse(shell)?),
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_background_stress: false,
            profile_sparse_updates: false,
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
                mode: StartupMode::CliService(parse_serial_args(serial_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_background_stress: false,
                profile_sparse_updates: false,
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
                mode: StartupMode::CliService(parse_http_args(http_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_background_stress: false,
                profile_sparse_updates: false,
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
                    mode: StartupMode::CliService(parse_tftp_server_args(
                        server_arguments.iter().cloned(),
                    )?),
                    profile_report: None,
                    profile_duration: None,
                    profile_pane_stress: false,
                    profile_background_stress: false,
                    profile_sparse_updates: false,
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
            mode: StartupMode::Application,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_background_stress: false,
            profile_sparse_updates: false,
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
                mode: StartupMode::CliService(parse_notify_args(notify_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_background_stress: false,
                profile_sparse_updates: false,
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
                mode: StartupMode::CliService(parse_copy_args(copy_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_background_stress: false,
                profile_sparse_updates: false,
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
                mode: StartupMode::CliService(parse_paste_args(paste_arguments.iter().cloned())?),
                profile_report: None,
                profile_duration: None,
                profile_pane_stress: false,
                profile_background_stress: false,
                profile_sparse_updates: false,
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
            #[cfg(windows)]
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
    Ok(StartupArgs {
        config_path: config,
        keymap_path: keymap,
        profile,
        mode,
        profile_report: None,
        profile_duration: None,
        profile_pane_stress: false,
        profile_background_stress: false,
        profile_sparse_updates: false,
        profile_external_terminal: false,
        tftp_command: None,
    })
}

fn parse_benchmark_args(arguments: &[OsString]) -> Result<StartupArgs> {
    let mut mode = StartupMode::TerminalRenderingProfile;
    let mut profile_report = None;
    let mut profile_duration = None;
    let mut profile_pane_stress = false;
    let mut profile_background_stress = false;
    let mut profile_sparse_updates = false;
    let mut profile_external_terminal = false;
    let mut args = arguments.iter();
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--profile-pane-stress" | "-s" => profile_pane_stress = true,
            "--profile-background-stress" | "-b" => profile_background_stress = true,
            "--profile-sparse-updates" | "-u" => profile_sparse_updates = true,
            "--profile-external-terminal" | "-x" => profile_external_terminal = true,
            "--terminal-render-workload" => mode = StartupMode::TerminalRenderingWorkload,
            "--terminal-checkerboard-workload" => mode = StartupMode::TerminalCheckerboardWorkload,
            "--terminal-sparse-update-workload" => mode = StartupMode::TerminalSparseUpdateWorkload,
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
                    "Benchmark terminal rendering\n\nUsage: zetta benchmark [OPTIONS]\n\nOptions:\n  -s, --profile-pane-stress           Use four visible producer panes\n  -b, --profile-background-stress     Render alternating cell backgrounds\n  -u, --profile-sparse-updates        Update a dense terminal at 40 Hz\n  -x, --profile-external-terminal     Run the workload in the current terminal\n  -r, --profile-report PATH           Write a profiling report\n  -d, --profile-duration SECONDS      Set the profiling duration\n  -h, --help                          Print help"
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
    anyhow::ensure!(
        !(profile_background_stress && profile_sparse_updates),
        "--profile-background-stress and --profile-sparse-updates cannot be combined"
    );
    if profile_report.is_some() && profile_duration.is_none() {
        profile_duration = Some(DEFAULT_PERFORMANCE_REPORT_DURATION);
    }
    Ok(StartupArgs {
        config_path: None,
        keymap_path: None,
        profile: None,
        mode,
        profile_report,
        profile_duration,
        profile_pane_stress,
        profile_background_stress,
        profile_sparse_updates,
        profile_external_terminal,
        tftp_command: None,
    })
}

fn select_launch_profile(config: &Config, requested: Option<&str>) -> Result<Option<Profile>> {
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

pub(crate) fn parse_args() -> Result<StartupArgs> {
    parse_args_from(env::args_os().skip(1))
}

fn should_handoff_to_existing_process(args: &StartupArgs) -> bool {
    args.mode == StartupMode::Application
        && args.config_path.is_none()
        && args.keymap_path.is_none()
        && args.profile.is_none()
}

#[cfg(windows)]
fn path_with_entry_first(path: Option<&std::ffi::OsStr>, entry: &Path) -> Option<OsString> {
    let inherited = path.map(env::split_paths).into_iter().flatten();
    let entries = inherited.collect::<Vec<_>>();
    let entry_text = entry.to_string_lossy();
    if entries.iter().any(|candidate| {
        candidate
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .eq_ignore_ascii_case(entry_text.trim_end_matches(['\\', '/']))
    }) {
        return None;
    }
    env::join_paths(std::iter::once(entry.to_path_buf()).chain(entries)).ok()
}

#[cfg(windows)]
pub(crate) fn native_terminal_environment() -> Vec<(String, String)> {
    let Some(executable_directory) = env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
    else {
        return Vec::new();
    };
    let Some(path) = path_with_entry_first(env::var_os("PATH").as_deref(), &executable_directory)
    else {
        return Vec::new();
    };
    vec![("PATH".to_owned(), path.to_string_lossy().into_owned())]
}

#[cfg(not(windows))]
pub(crate) fn native_terminal_environment() -> Vec<(String, String)> {
    Vec::new()
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

pub(crate) fn profile_keybindings(
    slot: usize,
    keyboard_mapper: &dyn PlatformKeyboardMapper,
) -> [KeyBinding; 1] {
    const SHIFTED_DIGITS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];
    let action = OpenProfile { slot };
    let context = Some(
        KeyBindingContextPredicate::parse("Zetta > Terminal")
            .expect("built-in keybinding context must be valid")
            .into(),
    );
    let binding = |keystroke: &str, action: OpenProfile| {
        KeyBinding::load(
            keystroke,
            Box::new(action),
            context.clone(),
            true,
            None,
            keyboard_mapper,
        )
        .expect("built-in profile keystroke must be valid")
    };
    [binding(
        &format!("ctrl-{}", SHIFTED_DIGITS[slot - 1]),
        action,
    )]
}

/// Returns the user-facing alias for a built-in number-row profile shortcut.
///
/// GPUI represents a shifted number key by its produced symbol (for example,
/// `ctrl-!`), while users type the more familiar `Ctrl+Shift+1` chord. A
/// remapped binding has no alias, so callers display the effective binding.
pub(crate) fn profile_shortcut_label(slot: usize, binding: &KeyBinding) -> Option<String> {
    const SHIFTED_DIGITS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];
    let symbol = SHIFTED_DIGITS.get(slot.checked_sub(1)?)?;
    let expected = gpui::Keystroke::parse(&format!("ctrl-{symbol}")).ok()?;
    let [keystroke] = binding.keystrokes() else {
        return None;
    };
    (keystroke.inner() == &expected).then(|| format!("Ctrl+Shift+{slot}"))
}

pub(crate) fn pane_template_keybindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new(
            "alt-shift-o",
            ApplyPaneSplitTemplate {
                name: "three-right".to_owned(),
            },
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "alt-shift-e",
            ApplyPaneSplitTemplate {
                name: "quarters".to_owned(),
            },
            Some("Zetta > Terminal"),
        ),
    ]
}

pub(crate) fn pane_font_size_keybindings() -> [KeyBinding; 4] {
    [
        KeyBinding::new(
            "alt-shift-=",
            IncreasePaneFontSize,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "alt-shift-+",
            IncreasePaneFontSize,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "alt-shift--",
            DecreasePaneFontSize,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("alt-shift-0", ResetPaneFontSize, Some("Zetta > Terminal")),
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ThemeFileStamp {
    pub(crate) modified: Option<SystemTime>,
    pub(crate) len: u64,
}

pub(crate) fn changed_theme_files(
    themes_dir: &Path,
    cache: &mut HashMap<PathBuf, ThemeFileStamp>,
) -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    let mut present = std::collections::HashSet::new();
    for entry in fs::read_dir(themes_dir)
        .with_context(|| format!("reading theme directory {}", themes_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry.metadata()?;
        let stamp = ThemeFileStamp {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        };
        present.insert(path.clone());
        if cache.get(&path) != Some(&stamp) {
            cache.insert(path.clone(), stamp);
            changed.push(path);
        }
    }
    cache.retain(|path, _| present.contains(path));
    Ok(changed)
}

pub(crate) fn load_user_themes(cx: &mut App) -> Result<()> {
    static THEME_FILE_CACHE: OnceLock<Mutex<HashMap<PathBuf, ThemeFileStamp>>> = OnceLock::new();
    let themes_dir = config::themes_dir();
    fs::create_dir_all(&themes_dir)
        .with_context(|| format!("creating theme directory {}", themes_dir.display()))?;
    let registry = ThemeRegistry::global(cx);
    let paths = changed_theme_files(
        &themes_dir,
        &mut THEME_FILE_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )?;
    for path in paths {
        let bytes = fs::read(&path).with_context(|| format!("reading theme {}", path.display()))?;
        theme_settings::load_user_theme(&registry, &bytes)
            .with_context(|| format!("loading theme {}", path.display()))?;
    }
    Ok(())
}

pub(crate) fn with_zetta_theme_overrides(theme: Arc<Theme>) -> Arc<Theme> {
    let mut theme = theme.as_ref().clone();
    let colors = &mut theme.styles.colors;
    colors.scrollbar_thumb_background = colors.text_muted.opacity(0.7);
    colors.scrollbar_thumb_hover_background = colors.text.opacity(0.85);
    colors.scrollbar_thumb_active_background = colors.text_accent.opacity(0.95);
    Arc::new(theme)
}

pub(crate) fn apply_zetta_theme_overrides(cx: &mut App) {
    GlobalTheme::update_theme(cx, with_zetta_theme_overrides(cx.theme().clone()));
}

pub(crate) fn resolve_profile_theme(profile: &Profile, cx: &App) -> Result<Option<Arc<Theme>>> {
    profile
        .theme
        .as_deref()
        .map(|name| {
            ThemeRegistry::global(cx)
                .get(name)
                .map(with_zetta_theme_overrides)
                .with_context(|| format!("using theme {name:?} for profile {:?}", profile.name))
        })
        .transpose()
}

pub(crate) fn is_wsl_shell(shell: &Shell) -> bool {
    let program = match shell {
        Shell::System => return false,
        Shell::Program(program) | Shell::WithArguments { program, .. } => program,
    };
    program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("wsl.exe"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Msys2Shell {
    Bash,
    Zsh,
}

pub(crate) fn msys2_profile(shell: &Shell) -> Option<(PathBuf, Msys2Shell)> {
    let Shell::WithArguments { program, args, .. } = shell else {
        return None;
    };
    if !program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd.exe"))
    {
        return None;
    }
    let command = args.last()?.strip_prefix("\"\"")?;
    let launcher_end = command.find("\" -defterm")?;
    let launcher = PathBuf::from(&command[..launcher_end]);
    if !launcher
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("msys2_shell.cmd"))
    {
        return None;
    }
    let shell = command[launcher_end..]
        .split_once(" -shell ")?
        .1
        .strip_suffix('"')?;
    let shell = match shell {
        "bash" => Msys2Shell::Bash,
        "zsh" => Msys2Shell::Zsh,
        _ => return None,
    };
    Some((launcher.parent()?.to_path_buf(), shell))
}

pub(crate) fn msys2_path_to_windows(root: &Path, directory: &str) -> Option<PathBuf> {
    if !directory.starts_with('/') || directory.chars().any(char::is_control) {
        return None;
    }
    let parts = directory
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.iter().any(|part| matches!(*part, "." | "..")) {
        return None;
    }
    if directory.starts_with("//") {
        return (parts.len() >= 2)
            .then(|| PathBuf::from(format!(r"\\{}\{}", parts[0], parts[1..].join(r"\"))));
    }
    if parts
        .first()
        .is_some_and(|drive| drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic())
    {
        let drive = parts[0].to_ascii_uppercase();
        let mut path = PathBuf::from(format!("{drive}:\\"));
        path.extend(&parts[1..]);
        return Some(path);
    }
    let mut path = root.to_path_buf();
    path.extend(parts);
    Some(path)
}

#[cfg(windows)]
fn windows_path_to_msys(path: &Path) -> Option<String> {
    let path = path.to_string_lossy().replace('\\', "/");
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1..3] == *b":/" {
        return Some(format!(
            "/{}/{}",
            (bytes[0] as char).to_ascii_lowercase(),
            &path[3..]
        ));
    }
    path.strip_prefix("//")
        .map(|path| format!("//{path}"))
        .or_else(|| path.starts_with('/').then_some(path))
}

#[cfg(windows)]
const MSYS2_BASH_CWD_TRACKER: &str = r#"printf '\033]2;zetta-cwd:%s\033\\' "$PWD""#;

#[cfg(windows)]
const MSYS2_ZSH_CWD_TRACKER: &str = r#"if [[ -n ${ZETTA_ORIGINAL_ZDOTDIR+x} ]]; then
    ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
    export ZDOTDIR
else
    unset ZDOTDIR
fi
original_zdotdir="${ZDOTDIR:-$HOME}"
[[ -r "$original_zdotdir/.zshenv" ]] && source "$original_zdotdir/.zshenv"

function __zetta_report_cwd() {
    [[ "$PWD" == /* ]] && printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_report_cwd
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR original_zdotdir
"#;

#[cfg(windows)]
pub(crate) fn msys2_cwd_tracking_environment(
    shell: &Shell,
    pane_id: u64,
    temporary_directory: &Path,
) -> Result<Vec<(String, String)>> {
    let Some((_, shell)) = msys2_profile(shell) else {
        return Ok(Vec::new());
    };
    match shell {
        Msys2Shell::Bash => {
            let existing = env::var("PROMPT_COMMAND").ok();
            Ok(vec![(
                "PROMPT_COMMAND".to_owned(),
                format!(
                    "{MSYS2_BASH_CWD_TRACKER}{}",
                    existing
                        .filter(|command| !command.is_empty())
                        .map(|command| format!(";{command}"))
                        .unwrap_or_default()
                ),
            )])
        }
        Msys2Shell::Zsh => {
            let directory = temporary_directory
                .join(format!("zetta-msys2-zsh-{}-{pane_id}", std::process::id()));
            fs::create_dir_all(&directory)
                .with_context(|| format!("creating {}", directory.display()))?;
            fs::write(directory.join(".zshenv"), MSYS2_ZSH_CWD_TRACKER).with_context(|| {
                format!(
                    "writing MSYS2 Zsh CWD integration in {}",
                    directory.display()
                )
            })?;
            let msys_directory = windows_path_to_msys(&directory)
                .context("temporary directory cannot be represented as an MSYS2 path")?;
            let mut environment = vec![
                ("ZDOTDIR".to_owned(), msys_directory.clone()),
                ("ZETTA_INTEGRATION_ZDOTDIR".to_owned(), msys_directory),
            ];
            if let Some(original) = env::var_os("ZDOTDIR") {
                let original = PathBuf::from(original);
                let original = if original.is_absolute() {
                    windows_path_to_msys(&original)
                        .context("ZDOTDIR cannot be represented as an MSYS2 path")?
                } else {
                    original.to_string_lossy().into_owned()
                };
                environment.push(("ZETTA_ORIGINAL_ZDOTDIR".to_owned(), original));
            }
            Ok(environment)
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn msys2_cwd_tracking_environment(
    _shell: &Shell,
    _pane_id: u64,
    _temporary_directory: &Path,
) -> Result<Vec<(String, String)>> {
    Ok(Vec::new())
}

pub(crate) fn launch_working_directory(
    profile: &Profile,
    inherited: Option<PathBuf>,
    inherited_wsl: Option<String>,
    fallback: Option<PathBuf>,
    fallback_is_configured: bool,
) -> (Option<PathBuf>, Option<String>) {
    // Windows process inspection sees the cwd of wsl.exe, not of its Linux shell.
    // Passing that value to a new WSL session leaks Zetta's own launch directory.
    let is_wsl = is_wsl_shell(&profile.command);
    let has_inherited_wsl = inherited_wsl.is_some();
    let working_directory = if is_wsl && has_inherited_wsl {
        None
    } else if is_wsl {
        fallback_is_configured.then_some(fallback).flatten()
    } else {
        inherited.or(fallback)
    };
    let wsl_directory = if is_wsl && has_inherited_wsl {
        inherited_wsl
    } else {
        (is_wsl && !fallback_is_configured).then(|| "~".to_owned())
    };
    (working_directory, wsl_directory)
}

pub(crate) fn wsl_cwd_tracking_file(profile: &Profile, pane_id: u64) -> Option<PathBuf> {
    (cfg!(windows) && is_wsl_shell(&profile.command)).then(|| {
        let path = env::temp_dir().join(format!("zetta-wsl-cwd-{}-{pane_id}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    })
}

pub(crate) const WSL_CWD_TRACKER: &str = r#"marker="$(wslpath -u "$1" 2>/dev/null || true)"
shell="${SHELL:-}"
if [ ! -x "$shell" ]; then
    shell="$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f7)"
fi
[ -x "$shell" ] || shell=/bin/sh
# Windows-side process inspection can't see into the WSL VM's own process
# namespace, so the tab title can't be derived from the host process tree the
# way it is for native Windows/MSYS2 shells. Report it explicitly instead: a
# `zetta-cmd:<value>` title marker carrying the shell name at idle, or the
# command about to run, mirrored by `reported_foreground_command_from_title`
# in crates/terminal/src/terminal.rs.
export ZETTA_SHELL_NAME="${shell##*/}"

case "${shell##*/}" in
    bash)
        zetta_full_prompt_command="$(cat <<'ZETTA_BASH_PROMPT'
__zetta_preexec() {
    case "$BASH_COMMAND" in
        __zetta_precmd) return ;;
    esac
    printf '\033]2;zetta-cmd:%s\033\\' "$BASH_COMMAND"
}
__zetta_precmd() {
    case "$PWD" in
        /*) printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD" ;;
    esac
    printf '\033]2;zetta-cmd:%s\033\\' "$ZETTA_SHELL_NAME"
}
trap '__zetta_preexec' DEBUG
PROMPT_COMMAND="__zetta_precmd${ZETTA_ORIGINAL_PROMPT_COMMAND:+;${ZETTA_ORIGINAL_PROMPT_COMMAND}}"
__zetta_precmd
ZETTA_BASH_PROMPT
)"
        export ZETTA_ORIGINAL_PROMPT_COMMAND="$PROMPT_COMMAND"
        PROMPT_COMMAND="$zetta_full_prompt_command"
        export PROMPT_COMMAND
        exec "$shell" -l
        ;;
    fish)
        exec "$shell" -l -C 'function __zetta_report_cwd --on-event fish_prompt; if string match -qr "^/" -- "$PWD"; printf "\033]7;file://localhost%s\033\\" "$PWD"; printf "\033]2;zetta-cwd:%s\033\\" "$PWD"; end; printf "\033]2;zetta-cmd:%s\033\\" "$ZETTA_SHELL_NAME"; end; function __zetta_report_preexec --on-event fish_preexec; printf "\033]2;zetta-cmd:%s\033\\" "$argv[1]"; end'
        ;;
    zsh)
        integration_zdotdir="$(mktemp -d "${TMPDIR:-/tmp}/zetta-zsh-XXXXXX" 2>/dev/null || true)"
        if [ -n "$integration_zdotdir" ]; then
            export ZETTA_ORIGINAL_ZDOTDIR="${ZDOTDIR:-$HOME}"
            export ZETTA_INTEGRATION_ZDOTDIR="$integration_zdotdir"
            cat > "$integration_zdotdir/.zshenv" <<'ZETTA_ZSHENV'
ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"

function __zetta_report_cwd() {
    [[ "$PWD" == /* ]] && printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD"
    printf '\033]2;zetta-cmd:%s\033\\' "$ZETTA_SHELL_NAME"
}
function __zetta_report_preexec() {
    printf '\033]2;zetta-cmd:%s\033\\' "$1"
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_report_cwd
add-zsh-hook preexec __zetta_report_preexec
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR
ZETTA_ZSHENV
            ZDOTDIR="$integration_zdotdir"
            export ZDOTDIR
            exec "$shell" -l
        fi
        ;;
esac

# Shells without an injection mechanism retain the legacy tracker.
parent=$$
if [ -n "$marker" ]; then
    (
        previous=
        while kill -0 "$parent" 2>/dev/null; do
            cwd="$(readlink "/proc/$parent/cwd" 2>/dev/null)" || break
            if [ "$cwd" != "$previous" ]; then
                printf '%s\n' "$cwd" > "${marker}.tmp" && mv -f "${marker}.tmp" "$marker"
                previous="$cwd"
            fi
            sleep 0.1
        done
        rm -f "$marker" "${marker}.tmp"
    ) </dev/null >/dev/null 2>&1 &
fi
exec "$shell" -l"#;

pub(crate) fn wsl_shell_with_tracking(
    shell: Shell,
    directory: Option<&str>,
    cwd_file: Option<&Path>,
) -> Shell {
    match shell {
        Shell::Program(program) => {
            wsl_command_with_tracking(program, Vec::new(), None, directory, cwd_file)
        }
        Shell::WithArguments {
            program,
            args,
            title_override,
        } => wsl_command_with_tracking(program, args, title_override, directory, cwd_file),
        Shell::System => Shell::System,
    }
}

pub(crate) fn wsl_command_with_tracking(
    program: String,
    mut args: Vec<String>,
    title_override: Option<String>,
    directory: Option<&str>,
    cwd_file: Option<&Path>,
) -> Shell {
    let exec_index = args.iter().position(|arg| arg == "--exec" || arg == "-e");
    if let Some(directory) = directory
        && !args
            .iter()
            .take(exec_index.unwrap_or(args.len()))
            .any(|arg| arg == "--cd" || arg.starts_with("--cd="))
    {
        args.splice(
            exec_index.unwrap_or(args.len())..exec_index.unwrap_or(args.len()),
            ["--cd".to_owned(), directory.to_owned()],
        );
    }
    if exec_index.is_none()
        && let Some(cwd_file) = cwd_file
    {
        args.extend([
            "--exec".to_owned(),
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            WSL_CWD_TRACKER.to_owned(),
            "zetta-wsl-cwd".to_owned(),
            cwd_file.to_string_lossy().into_owned(),
        ]);
    }
    Shell::WithArguments {
        program,
        args,
        title_override,
    }
}

pub(crate) fn apply_config_settings(config: &Config, cx: &mut App) -> Result<()> {
    let theme_name = selected_theme_name(config.theme.as_deref());
    let theme = ThemeRegistry::global(cx)
        .get(theme_name)
        .with_context(|| format!("using Zed theme {theme_name:?}"))?;
    GlobalTheme::update_theme(cx, theme);
    apply_zetta_theme_overrides(cx);

    let mut terminal_settings = TerminalSettings::get_global(cx).clone();
    terminal_settings.font_family = Some(theme_settings::FontFamilyName(
        config.terminal_font_family.clone().into(),
    ));
    terminal_settings.font_size = config.terminal_font_size.map(px);
    terminal_settings.copy_on_select = true;
    terminal_settings.max_scroll_history_lines = Some(config.max_scroll_history_lines);
    TerminalSettings::override_global(terminal_settings, cx);
    Ok(())
}

pub(crate) fn selected_theme_name(configured_theme: Option<&str>) -> &str {
    configured_theme.unwrap_or(ZETTA_DEFAULT_THEME)
}

pub(crate) fn normalize_keymap_key_names(content: &str) -> String {
    content
        .replace("page-up", "pageup")
        .replace("page-down", "pagedown")
}

pub(crate) fn validate_keymap_contents(content: &str, cx: &mut App) -> Result<()> {
    let content = normalize_keymap_key_names(content);
    match KeymapFile::load(&content, cx) {
        KeymapFileLoadResult::Success { .. } => Ok(()),
        KeymapFileLoadResult::SomeFailedToLoad { error_message, .. } => {
            anyhow::bail!("some key bindings are invalid: {error_message}")
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            Err(error).context("parsing keymap JSON")
        }
    }
}

pub(crate) const RENAME_TAB_KEYBINDING: &str = "ctrl-shift-r";
pub(crate) const RELOAD_CONFIGURATION_KEYBINDING: &str = "ctrl-alt-r";
pub(crate) const RENAME_PANE_KEYBINDING: &str = "alt-shift-r";
pub(crate) const TOGGLE_PANE_CONTROLS_KEYBINDING: &str = "alt-shift-h";
pub(crate) const TOGGLE_TAB_PANE_CONTROLS_KEYBINDING: &str = "ctrl-shift-h";
pub(crate) const CLOSE_PANE_KEYBINDING: &str = "alt-shift-x";
pub(crate) const SAVE_PANE_OUTPUT_KEYBINDING: &str = "alt-shift-s";
pub(crate) const SELECT_ALL_KEYBINDING: &str = "alt-shift-a";
pub(crate) const RECONNECT_SESSION_KEYBINDING: &str = "ctrl-shift-a";
pub(crate) const DETACH_TAB_KEYBINDING: &str = "ctrl-shift-d";
pub(crate) const CLOSE_WINDOW_KEYBINDING: &str = "ctrl-shift-q";
pub(crate) const CLOSE_ALL_WINDOWS_KEYBINDING: &str = "ctrl-shift-x";
#[cfg(feature = "serial-console")]
pub(crate) const SERIAL_CONSOLE_KEYBINDING: &str = "ctrl-shift-s";
pub(crate) const AUTO_BACKGROUND_TAB_KEYBINDING: &str = "ctrl-shift-b";
pub(crate) const ROTATE_PANE_LAYOUT_KEYBINDING: &str = "alt-shift-l";
pub(crate) const TOGGLE_PANE_RESIZE_MODE_KEYBINDING: &str = "ctrl-shift-j";
pub(crate) const APPLICATION_MENU_KEYBINDING: &str = "alt-space";

pub(crate) fn pane_output_keybinding() -> KeyBinding {
    KeyBinding::new(
        SAVE_PANE_OUTPUT_KEYBINDING,
        SavePaneOutput,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn select_all_keybinding() -> KeyBinding {
    KeyBinding::new(SELECT_ALL_KEYBINDING, SelectAll, Some("Zetta > Terminal"))
}

pub(crate) fn reconnect_session_keybinding() -> KeyBinding {
    KeyBinding::new(
        RECONNECT_SESSION_KEYBINDING,
        ReconnectSession,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn application_menu_keybinding() -> Option<KeyBinding> {
    cfg!(any(target_os = "windows", target_os = "linux")).then(|| {
        KeyBinding::new(
            APPLICATION_MENU_KEYBINDING,
            OpenApplicationMenu,
            // The menu is an application-level control. Binding it at the Zetta
            // context keeps it available even when the focused terminal view does
            // not contribute its `Terminal` key context.
            Some("Zetta"),
        )
    })
}

pub(crate) fn application_menu_navigation_keybindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new("left", ActivateApplicationMenuLeft, Some("Zetta > menu")),
        KeyBinding::new("right", ActivateApplicationMenuRight, Some("Zetta > menu")),
    ]
}

pub(crate) fn detach_tab_keybinding() -> KeyBinding {
    KeyBinding::new(DETACH_TAB_KEYBINDING, DetachTab, Some("Zetta > Terminal"))
}

pub(crate) fn close_window_keybinding() -> KeyBinding {
    KeyBinding::new(
        CLOSE_WINDOW_KEYBINDING,
        CloseWindow,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn close_all_windows_keybinding() -> KeyBinding {
    KeyBinding::new(
        CLOSE_ALL_WINDOWS_KEYBINDING,
        CloseAllWindows,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn close_pane_keybinding() -> KeyBinding {
    KeyBinding::new(CLOSE_PANE_KEYBINDING, ClosePane, Some("Zetta > Terminal"))
}

#[cfg(feature = "serial-console")]
pub(crate) fn serial_console_keybinding() -> KeyBinding {
    KeyBinding::new(
        SERIAL_CONSOLE_KEYBINDING,
        ToggleSerialConsole,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn auto_background_tab_keybinding() -> KeyBinding {
    KeyBinding::new(
        AUTO_BACKGROUND_TAB_KEYBINDING,
        ToggleAutoBackgroundTab,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn rotate_pane_layout_keybinding() -> KeyBinding {
    KeyBinding::new(
        ROTATE_PANE_LAYOUT_KEYBINDING,
        RotatePaneLayout,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn pane_resize_mode_keybinding() -> KeyBinding {
    KeyBinding::new(
        TOGGLE_PANE_RESIZE_MODE_KEYBINDING,
        TogglePaneResizeMode,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn pane_resize_keybindings() -> [KeyBinding; 4] {
    [
        KeyBinding::new(
            "left",
            ResizePaneLeft,
            Some("Zetta > PaneResize > Terminal"),
        ),
        KeyBinding::new(
            "right",
            ResizePaneRight,
            Some("Zetta > PaneResize > Terminal"),
        ),
        KeyBinding::new("up", ResizePaneUp, Some("Zetta > PaneResize > Terminal")),
        KeyBinding::new(
            "down",
            ResizePaneDown,
            Some("Zetta > PaneResize > Terminal"),
        ),
    ]
}

pub(crate) fn minimized_pane_keybindings() -> [KeyBinding; 4] {
    [
        KeyBinding::new("alt-shift-down", MinimizePane, Some("Zetta > Terminal")),
        KeyBinding::new(
            "alt-shift-up",
            RestoreMinimizedPane,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "alt-shift-left",
            SelectPreviousMinimizedPane,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "alt-shift-right",
            SelectNextMinimizedPane,
            Some("Zetta > Terminal"),
        ),
    ]
}

pub(crate) fn load_keybindings(path: &PathBuf, profile_count: usize, cx: &mut App) {
    cx.clear_key_bindings();
    match KeymapFile::load_asset_allow_partial_failure(settings::DEFAULT_KEYMAP_PATH, cx) {
        Ok(bindings) => cx.bind_keys(bindings),
        Err(error) => eprintln!("Could not load the default terminal keymap: {error:#}"),
    }
    let mut bindings = vec![
        KeyBinding::new("ctrl-shift-t", NewTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-n", NewWindow, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Zetta > Terminal")),
        close_window_keybinding(),
        close_all_windows_keybinding(),
        detach_tab_keybinding(),
        reconnect_session_keybinding(),
        auto_background_tab_keybinding(),
        close_pane_keybinding(),
        KeyBinding::new(
            "ctrl-shift-o",
            SplitHorizontalDown,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-shift-e", SplitVerticalRight, Some("Zetta > Terminal")),
        rotate_pane_layout_keybinding(),
        pane_resize_mode_keybinding(),
        select_all_keybinding(),
        KeyBinding::new(
            "ctrl-shift-backspace",
            ClearClipboard,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("alt-left", FocusPaneLeft, Some("Zetta > Terminal")),
        KeyBinding::new("alt-right", FocusPaneRight, Some("Zetta > Terminal")),
        KeyBinding::new("alt-up", FocusPaneUp, Some("Zetta > Terminal")),
        KeyBinding::new("alt-down", FocusPaneDown, Some("Zetta > Terminal")),
        KeyBinding::new("shift-escape", ToggleMaximizePane, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-i",
            ToggleBroadcastInput,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-shift-m", ToggleMultiCommand, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-tab", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-pageup", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-pagedown", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-c",
            CopyAndClearSelection,
            Some("Zetta > Terminal && selection"),
        ),
        KeyBinding::new("ctrl-v", Paste, Some("Zetta > Terminal")),
        KeyBinding::new("alt-shift-f", SearchScrollback, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-f",
            SearchTabScrollback,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "enter",
            SearchNextMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "shift-enter",
            SearchPreviousMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "f3",
            SearchNextMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "shift-f3",
            SearchPreviousMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "escape",
            DismissSearch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "ctrl-a",
            SelectAllSearchText,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new("ctrl-alt-v", PasteTrimmed, Some("Zetta > Terminal")),
        pane_output_keybinding(),
        KeyBinding::new(
            "ctrl-shift-p",
            ToggleCommandPalette,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-,", ToggleSettings, Some("Zetta > Terminal")),
        KeyBinding::new(RENAME_TAB_KEYBINDING, RenameTab, Some("Zetta > Terminal")),
        KeyBinding::new(RENAME_PANE_KEYBINDING, RenamePane, Some("Zetta > Terminal")),
        KeyBinding::new(
            TOGGLE_PANE_CONTROLS_KEYBINDING,
            TogglePaneControls,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            TOGGLE_TAB_PANE_CONTROLS_KEYBINDING,
            ToggleTabPaneControls,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-=", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-+", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl--", DecreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-0", ResetTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new(
            RELOAD_CONFIGURATION_KEYBINDING,
            ReloadConfiguration,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "ctrl-shift-f12",
            TogglePerformanceOverlay,
            Some("Zetta > Terminal"),
        ),
        // Override Zed's inherited `pane::CloseActiveItem` binding in terminal focus.
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Terminal")),
    ];
    #[cfg(feature = "serial-console")]
    bindings.push(serial_console_keybinding());
    bindings.extend(application_menu_keybinding());
    bindings.extend(application_menu_navigation_keybindings());
    bindings.extend(minimized_pane_keybindings());
    bindings.extend(pane_resize_keybindings());
    bindings.extend(pane_template_keybindings());
    bindings.extend(pane_font_size_keybindings());
    let keyboard_mapper = cx.keyboard_mapper().clone();
    bindings.extend(
        (1..=profile_count.min(9))
            .flat_map(|slot| profile_keybindings(slot, keyboard_mapper.as_ref())),
    );
    cx.bind_keys(bindings);
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let content = normalize_keymap_key_names(&content);
    match KeymapFile::load(&content, cx) {
        KeymapFileLoadResult::Success { key_bindings } => cx.bind_keys(key_bindings),
        KeymapFileLoadResult::SomeFailedToLoad {
            key_bindings,
            error_message,
        } => {
            eprintln!(
                "Some key bindings in {} were ignored: {error_message}",
                path.display()
            );
            cx.bind_keys(key_bindings);
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            eprintln!("Could not load {}: {error:#}", path.display());
        }
    }
}

pub(crate) fn open_zetta_window(
    config: Config,
    configuration_error: Option<String>,
    initial_profile: Option<Profile>,
    enable_performance_overlay: bool,
    performance_report: Option<(PerformanceReportOptions, PerformanceReportStatus)>,
    profile_pane_stress: bool,
    cx: &mut App,
) -> Result<()> {
    let options = zetta_window_options(cx);
    cx.open_window(options, move |window, cx| {
        window.set_window_title("Zetta");
        let zetta =
            cx.new(|cx| Zetta::new(config, configuration_error, initial_profile, window, cx));
        track_zetta_window(&zetta, window, cx);
        prepare_background_tabs_before_window_close(&zetta, window, cx);
        if profile_pane_stress {
            zetta.update(cx, |zetta, cx| {
                zetta.configure_pane_profile_stress(window, cx)
            });
        }
        if enable_performance_overlay {
            zetta.update(cx, |zetta, cx| {
                zetta.toggle_performance_overlay(&TogglePerformanceOverlay, window, cx)
            });
        }
        if let Some((options, status)) = performance_report {
            zetta.update(cx, |zetta, cx| {
                zetta.start_performance_report(options, status, cx)
            });
        }
        zetta
    })
    .context("opening Zetta window")?;
    cx.activate(true);
    Ok(())
}

fn zetta_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(ZETTA_MINIMUM_WINDOW_SIZE),
        app_id: Some(ZETTA_APP_ID.to_owned()),
        titlebar: Some(TitlebarOptions {
            title: Some("Zetta".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.), px(9.))),
        }),
        app_owns_titlebar_drag: true,
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    }
}

fn track_zetta_window(zetta: &Entity<Zetta>, window: &Window, cx: &mut App) {
    if cx.has_global::<ZettaProcessState>() {
        let runner_id = zetta.read(cx).background_sessions.runner_id();
        let process = cx.global_mut::<ZettaProcessState>();
        process
            .windows
            .insert(window.window_handle().window_id(), zetta.clone());
        process.runners.insert(runner_id, zetta.clone());
    }
}

fn prepare_background_tabs_before_window_close(
    zetta: &Entity<Zetta>,
    window: &mut Window,
    cx: &mut App,
) {
    let zetta = zetta.downgrade();
    window.on_window_should_close(cx, move |_, cx| {
        zetta
            .update(cx, |zetta, cx| {
                zetta.prepare_for_background_window_close(cx)
            })
            .ok();
        true
    });
}

pub(crate) fn process_zetta_entities(cx: &App) -> Vec<Entity<Zetta>> {
    if !cx.has_global::<ZettaProcessState>() {
        return Vec::new();
    }
    let process = cx.global::<ZettaProcessState>();
    process
        .windows
        .values()
        .chain(process.dormant.iter())
        .cloned()
        .collect()
}

pub(crate) fn zetta_for_runner(runner_id: u64, cx: &App) -> Option<Entity<Zetta>> {
    if !cx.has_global::<ZettaProcessState>() {
        return None;
    }
    cx.global::<ZettaProcessState>()
        .runners
        .get(&runner_id)
        .cloned()
}

pub(crate) fn refresh_process_background_sessions(cx: &mut App) {
    let entities = process_zetta_entities(cx);
    let mut entries = Vec::new();
    for zetta in &entities {
        let zetta = zetta.read(cx);
        let runner_id = zetta.background_sessions.runner_id();
        entries.extend(zetta.background_session_picker_entries.iter().map(
            |(session_id, title, details)| (runner_id, *session_id, title.clone(), details.clone()),
        ));
    }
    if cx.has_global::<ZettaProcessState>() {
        cx.global_mut::<ZettaProcessState>()
            .background_session_entries = entries.into();
    }
    for zetta in entities {
        zetta.update(cx, |_, cx| cx.notify());
    }
}

pub(crate) fn prune_empty_dormant_runners(cx: &mut App) {
    if !cx.has_global::<ZettaProcessState>() {
        return;
    }
    let dormant = std::mem::take(&mut cx.global_mut::<ZettaProcessState>().dormant);
    let mut retained = Vec::with_capacity(dormant.len());
    let mut removed_runner_ids = Vec::new();
    for zetta in dormant {
        let (is_empty, runner_id) = {
            let state = zetta.read(cx);
            (
                state.background_sessions.is_empty(),
                state.background_sessions.runner_id(),
            )
        };
        if is_empty {
            removed_runner_ids.push(runner_id);
        } else {
            retained.push(zetta);
        }
    }
    let process = cx.global_mut::<ZettaProcessState>();
    process.dormant = retained;
    for runner_id in removed_runner_ids {
        process.runners.remove(&runner_id);
    }
    if should_quit_after_window_closed(process.windows.len(), process.dormant.len()) {
        quit_zetta_process(cx);
    }
}

fn should_quit_after_window_closed(window_count: usize, dormant_runner_count: usize) -> bool {
    window_count == 0 && dormant_runner_count == 0
}

fn zetta_quit_mode() -> gpui::QuitMode {
    gpui::QuitMode::Explicit
}

pub(crate) fn quit_zetta_process(cx: &mut App) {
    cx.global::<ZettaProcessState>()
        .control_server
        .begin_shutdown();
    cx.quit();
}

fn open_dormant_or_new_window(cx: &mut App) -> Result<()> {
    let (existing, dormant, config, configuration_error) = {
        let process = cx.global_mut::<ZettaProcessState>();
        (
            process
                .windows
                .iter()
                .next()
                .map(|(window_id, entity)| (*window_id, entity.clone())),
            process.dormant.pop(),
            process.config.clone(),
            process.configuration_error.clone(),
        )
    };
    if let Some((window_id, _)) = existing {
        gpui::WindowHandle::<Zetta>::new(window_id).update(cx, |zetta, window, cx| {
            zetta.resume_hidden_window(window, cx)
        })?;
        cx.activate(true);
        return Ok(());
    }
    if let Some(zetta) = dormant {
        let zetta_for_window = zetta.clone();
        cx.open_window(zetta_window_options(cx), move |window, cx| {
            window.set_window_title("Zetta");
            zetta_for_window.update(cx, |zetta, cx| zetta.attach_to_reopened_window(window, cx));
            track_zetta_window(&zetta_for_window, window, cx);
            prepare_background_tabs_before_window_close(&zetta_for_window, window, cx);
            zetta_for_window
        })?;
        cx.activate(true);
        Ok(())
    } else {
        open_zetta_window(config, configuration_error, None, false, None, false, cx)
    }
}

fn handle_zetta_window_closed(cx: &mut App, window_id: WindowId) {
    let entity = cx
        .global_mut::<ZettaProcessState>()
        .windows
        .remove(&window_id);
    if let Some(entity) = entity {
        entity.update(cx, |zetta, cx| {
            zetta.prepare_for_background_window_close(cx)
        });
        let (has_background_sessions, runner_id) = {
            let entity_state = entity.read(cx);
            (
                !entity_state.background_sessions.is_empty(),
                entity_state.background_sessions.runner_id(),
            )
        };
        if has_background_sessions {
            cx.global_mut::<ZettaProcessState>().dormant.push(entity);
        } else {
            cx.global_mut::<ZettaProcessState>()
                .runners
                .remove(&runner_id);
        }
    }
    let process = cx.global::<ZettaProcessState>();
    if should_quit_after_window_closed(process.windows.len(), process.dormant.len()) {
        quit_zetta_process(cx);
    }
}

fn terminal_rendering_profile_config(executable: &Path, workload: PerformanceWorkload) -> Config {
    let mut config = Config::defaults(None, None);
    let workload_argument = match workload {
        PerformanceWorkload::Standard => "--terminal-render-workload",
        PerformanceWorkload::CheckerboardBackground => "--terminal-checkerboard-workload",
        PerformanceWorkload::SparseUpdates => "--terminal-sparse-update-workload",
    };
    config.profiles = vec![Profile {
        name: "Terminal rendering profiler".to_owned(),
        command: Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec!["benchmark".to_owned(), workload_argument.to_owned()],
            title_override: Some("Terminal rendering profiler".to_owned()),
        },
        theme: None,
    }];
    config.default_profile = 0;
    config
}

fn checkerboard_background(row: usize, column: usize, frame: u64) -> u8 {
    if (row + column + frame as usize).is_multiple_of(2) {
        41
    } else {
        44
    }
}

struct TerminalStateRestore;

impl Drop for TerminalStateRestore {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(b"\x1b[0m\x1b[?25h\r\n");
        let _ = stdout.flush();
    }
}

fn run_terminal_rendering_workload(
    workload: PerformanceWorkload,
    duration: Option<Duration>,
) -> Result<()> {
    const FRAME_INTERVAL: Duration = Duration::from_nanos(4_166_667);
    const SPARSE_UPDATE_INTERVAL: Duration = Duration::from_millis(25);
    const ROW: &str = "0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ │─╭╮╰╯ ✓ rendered cell workload";

    let _restore_terminal_state = TerminalStateRestore;
    let stdout = std::io::stdout();
    let mut output = std::io::BufWriter::new(stdout.lock());
    output.write_all(b"\x1b[2J\x1b[?25l")?;
    if workload == PerformanceWorkload::SparseUpdates {
        output.write_all(
            b"\x1b[H\x1b[1;36mZetta sparse terminal update profiler\x1b[0m\r\n\
              40 Hz producer updating only this status line\r\n\
              Dense unchanged content below models a full-screen TUI.\r\n\r\n",
        )?;
        for row in 0..34 {
            writeln!(output, "{row:02} {ROW}\r")?;
        }
        output.flush()?;
    }
    let mut frame = 0_u64;
    let mut next_frame = Instant::now();
    let deadline = duration.map(|duration| next_frame + duration);
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        if workload == PerformanceWorkload::SparseUpdates {
            let spinner = ['|', '/', '-', '\\'][(frame as usize) % 4];
            write!(
                output,
                "\x1b[2;1H40 Hz sparse producer · processing {spinner} · frame {frame:010}"
            )?;
            output.flush()?;
            frame = frame.wrapping_add(1);

            next_frame += SPARSE_UPDATE_INTERVAL;
            let now = Instant::now();
            let wake_at = deadline.map_or(next_frame, |deadline| next_frame.min(deadline));
            if wake_at > now {
                std::thread::sleep(wake_at - now);
            } else {
                next_frame = now;
            }
            continue;
        }

        let workload_description = match workload {
            PerformanceWorkload::Standard => "text and line-drawing cells",
            PerformanceWorkload::CheckerboardBackground => "alternating cell backgrounds",
            PerformanceWorkload::SparseUpdates => unreachable!(),
        };
        if write!(
            output,
            "\x1b[H\x1b[1;36mZetta terminal rendering profiler\x1b[0m\r\n\
             240 Hz producer · {workload_description} · frame {frame:010}\r\n\
             This deterministic workload is identical on Linux, macOS, and Windows.\r\n\r\n"
        )
        .is_err()
        {
            return Ok(());
        }
        for row in 0..34 {
            match workload {
                PerformanceWorkload::Standard => {
                    writeln!(output, "{row:02} {ROW} {frame:010}\r")?;
                }
                PerformanceWorkload::CheckerboardBackground => {
                    write!(output, "{row:02} ")?;
                    for column in 0..96 {
                        let background = checkerboard_background(row, column, frame);
                        write!(output, "\x1b[{background}m ")?;
                    }
                    write!(output, "\x1b[0m\r\n")?;
                }
                PerformanceWorkload::SparseUpdates => unreachable!(),
            }
        }
        output.flush()?;
        frame = frame.wrapping_add(1);

        next_frame += FRAME_INTERVAL;
        let now = Instant::now();
        let wake_at = deadline.map_or(next_frame, |deadline| next_frame.min(deadline));
        if wake_at > now {
            std::thread::sleep(wake_at - now);
        } else {
            next_frame = now;
        }
    }
    Ok(())
}

fn selected_performance_workload(args: &StartupArgs) -> PerformanceWorkload {
    if args.profile_background_stress {
        PerformanceWorkload::CheckerboardBackground
    } else if args.profile_sparse_updates {
        PerformanceWorkload::SparseUpdates
    } else {
        PerformanceWorkload::Standard
    }
}

pub(crate) fn run() -> Result<()> {
    let args = parse_args()?;
    if let StartupMode::OutputBenchmark {
        size_mib,
        output_type,
    } = &args.mode
    {
        return run_output_benchmark(*size_mib, *output_type);
    }
    if let StartupMode::PrintTerminalSize { json, resize } = args.mode {
        if let Some(resize) = resize {
            return request_terminal_resize(resize.columns, resize.rows);
        }
        print_terminal_size(json);
        return Ok(());
    }
    if let StartupMode::PrintShellIntegration(shell) = args.mode {
        let (config, _) = load_startup_config(None, None);
        print!("{}", shell.script(&config.profiles));
        return Ok(());
    }
    if args.mode == StartupMode::ConfigureCurrentShellIntegration {
        println!(
            "{}",
            shell_integration_configuration_message(&configure_current_shell_integration()?)
        );
        return Ok(());
    }
    if let StartupMode::ListBackgroundSessions { json } = &args.mode {
        return print_session_catalogs(*json);
    }
    #[cfg(any(
        feature = "serial-console",
        feature = "http-server",
        feature = "tftp-server",
        feature = "notifications",
        feature = "clipboard"
    ))]
    if let StartupMode::CliService(command) = &args.mode {
        return command.run();
    }
    if should_handoff_to_existing_process(&args) && request_existing_process_window()? {
        return Ok(());
    }
    #[cfg(windows)]
    if let StartupMode::RegisterWindowsShell(shortcut_path) = &args.mode {
        let (config, _) =
            load_startup_config(args.config_path.as_deref(), args.keymap_path.clone());
        return windows_integration::register_shell_integration(shortcut_path, &config.profiles);
    }
    if let Some(command) = &args.tftp_command {
        return command.run();
    }
    if args.mode == StartupMode::TerminalRenderingWorkload {
        return run_terminal_rendering_workload(PerformanceWorkload::Standard, None);
    }
    if args.mode == StartupMode::TerminalCheckerboardWorkload {
        return run_terminal_rendering_workload(PerformanceWorkload::CheckerboardBackground, None);
    }
    if args.mode == StartupMode::TerminalSparseUpdateWorkload {
        return run_terminal_rendering_workload(PerformanceWorkload::SparseUpdates, None);
    }

    let profiling = args.mode == StartupMode::TerminalRenderingProfile;
    let workload = selected_performance_workload(&args);
    if profiling && args.profile_external_terminal {
        return run_terminal_rendering_workload(workload, args.profile_duration);
    }
    let report_options = args
        .profile_report
        .zip(args.profile_duration)
        .map(|(path, duration)| PerformanceReportOptions {
            path,
            duration,
            workload,
        });
    let report_requested = report_options.is_some();
    let report_status = Arc::new(Mutex::new(None));
    let (config, configuration_error) = if profiling {
        (
            terminal_rendering_profile_config(&env::current_exe()?, workload),
            None,
        )
    } else {
        load_startup_config(args.config_path.as_deref(), args.keymap_path)
    };
    let initial_profile = select_launch_profile(&config, args.profile.as_deref())?;
    let keymap_path = config.keymap_path.clone();
    let profile_count = config.profiles.len();
    let http_client = Arc::new(
        reqwest_client::ReqwestClient::user_agent(concat!("Zetta/", env!("CARGO_PKG_VERSION")))
            .context("initializing HTTP client")?,
    );
    let report_status_for_app = report_status.clone();
    gpui_platform::application()
        .with_quit_mode(zetta_quit_mode())
        .with_assets(ZettaAssets)
        .run(move |cx: &mut App| {
            #[cfg(windows)]
            {
                cx.set_app_identity(ZETTA_APP_ID, "Zetta");
                windows_integration::update_profile_jump_list(config.profiles.clone());
            }
            cx.set_http_client(http_client);
            menu::init();
            zed_actions::init();
            release_channel::init(semver::Version::new(0, 1, 0), cx);
            settings::init(cx);
            theme_settings::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
            load_user_themes(cx).log_err();
            ZettaAssets.load_fonts(cx).log_err();
            apply_config_settings(&config, cx).expect("failed to apply Zetta configuration");
            load_keybindings(&keymap_path, profile_count, cx);
            let (control_tx, mut control_rx) = futures::channel::mpsc::unbounded();
            let control_server = ProcessControlServer::start(control_tx)
                .expect("failed to start Zetta process control");
            let quit_subscription = cx.on_app_quit(|cx| {
                if cx.has_global::<ZettaProcessState>() {
                    cx.global::<ZettaProcessState>()
                        .control_server
                        .begin_shutdown();
                }
                async {}
            });
            cx.set_global(ZettaProcessState {
                windows: HashMap::new(),
                dormant: Vec::new(),
                runners: HashMap::new(),
                background_session_entries: Arc::from([]),
                config: config.clone(),
                configuration_error: configuration_error.clone(),
                control_server,
                _quit_subscription: quit_subscription,
            });
            cx.spawn(async move |cx| {
                while let Some(ProcessControlCommand::OpenWindow { completion }) =
                    control_rx.next().await
                {
                    let opened = cx.update(|cx| {
                        if !cx
                            .global::<ZettaProcessState>()
                            .control_server
                            .is_accepting()
                        {
                            return false;
                        }

                        match open_dormant_or_new_window(cx) {
                            Ok(()) => true,
                            Err(error) => {
                                eprintln!("Could not open the requested Zetta window: {error:#}");
                                false
                            }
                        }
                    });
                    let _ = completion.send(opened);
                }
            })
            .detach();
            let layout_keymap_path = keymap_path.clone();
            cx.on_keyboard_layout_change(move |cx| {
                load_keybindings(&layout_keymap_path, profile_count, cx);
            })
            .detach();
            cx.on_window_closed(handle_zetta_window_closed).detach();

            open_zetta_window(
                config,
                configuration_error,
                initial_profile,
                profiling,
                report_options.map(|options| (options, report_status_for_app)),
                args.profile_pane_stress,
                cx,
            )
            .expect("failed to open Zetta window");
        });
    if report_requested {
        let result = report_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .context("profiling window closed before the performance report completed")?;
        result.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

fn shell_integration_configuration_message(
    configuration: &ShellIntegrationConfiguration,
) -> String {
    match configuration {
        ShellIntegrationConfiguration::Written(path) => format!(
            "Added Zetta shell integration to {}. Start a new shell or reload this file to enable it.",
            path.display()
        ),
        ShellIntegrationConfiguration::AlreadyPresent(path) => format!(
            "Zetta shell integration is already present in {}; no changes made.",
            path.display()
        ),
    }
}

#[cfg(test)]
#[path = "tests/startup.rs"]
mod tests;
