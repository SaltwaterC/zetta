use super::cli_help::{
    attention_help, benchmark_help, benchmark_output_help, edit_help, help_text,
    is_version_argument, pane_help, pane_splits_help, parse_overlay_args, parse_tab_icon_args,
    parse_terminal_resize_dimension, parse_theme_args, terminal_size_help, theme_help,
    version_text,
};
use super::*;
use crate::cli_services::{NotificationRequest, parse_notification_timeout};
use crate::command_panes::{
    MAX_PANE_COMMAND_BYTES, PaneCommand, pane_command_byte_len, parse_pane_direction,
};
use crate::profile_cli::{ProfileCommand, parse_profile_args};
use crate::project_cli::{ProjectCommand, parse_project_args};
use crate::project_commands::{ProjectCommandInvocation, parse_project_command_args};
use crate::run_command::PaneWaitCommand;
#[cfg(feature = "worktree")]
use zwt::{WorktreeCommand, WorktreeInvocation, parse_worktree_args_for};

const DEFAULT_PERFORMANCE_REPORT_DURATION: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartupMode {
    Application,
    /// Always open a fresh OS window, without activating or consuming an
    /// existing process's dormant sessions.
    NewWindow,
    /// Start a terminal and send the command to its shell after it is ready.
    /// The command owns the rest of argv, so arguments beginning with `-` are
    /// preserved instead of being interpreted as Zetta options.
    Command(Vec<String>),
    Pane(PaneCommand),
    PaneWait(PaneWaitCommand),
    Attention(AttentionCommand),
    #[cfg(feature = "worktree")]
    Worktree(WorktreeCommand),
    Project(ProjectCommand),
    ProjectCommand(ProjectCommandInvocation),
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
    /// `zetta mux ...`, forwarded to the multiplexer so the subcommand and the
    /// `zmux` binary cannot accept different arguments. Startup supplies the
    /// effective identity as a default for the commands that can need one; an
    /// explicit `-i/--identity` adds to it rather than replacing it.
    Mux(Vec<OsString>),
    SetTabIcon {
        icon: Option<IconName>,
    },
    ListTabIcons,
    SetTheme {
        scope: ThemeScope,
        theme: Option<String>,
    },
    ListThemes,
    ListPaneSplits,
    SetPaneOverlay(PaneOverlayRequest),
    #[cfg(windows)]
    RegisterWindowsShell(PathBuf),
    #[cfg(windows)]
    UnregisterWindowsShell,
    #[cfg(windows)]
    WindowsEmbedding,
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
    /// Explicitly retain the legacy in-process session owner instead of
    /// requiring the daemon-backed multiplexer.
    pub(crate) no_mux: bool,
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

impl StartupArgs {
    /// A launch that carries `mode` and nothing else.
    ///
    /// Every subcommand parses into exactly one mode and leaves the rest of the
    /// launch at its default, so this is what a `StartupArgs` literal in this
    /// module should be built from: spelling out the other thirteen fields as
    /// `None`/`false` at each of the two dozen sites made adding a field an
    /// edit of all of them, and hid the two branches that do set another field
    /// (`zetta profile`, which carries its own `--config`, and `zetta tftp
    /// get|put`, which carries a transfer for a window to run).
    pub(crate) fn for_mode(mode: StartupMode) -> Self {
        Self {
            config_path: None,
            keymap_path: None,
            profile: None,
            split: None,
            replace_pane: false,
            theme_override: None,
            no_mux: false,
            mode,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            profile_workload: PerformanceWorkload::Standard,
            profile_external_terminal: false,
            tftp_command: None,
        }
    }
}

mod benchmark;
mod subcommands;

use benchmark::parse_benchmark_subcommand;
use subcommands::{
    parse_attention_subcommand, parse_copy_subcommand, parse_edit_subcommand,
    parse_http_subcommand, parse_notify_subcommand, parse_pane_subcommand, parse_paste_subcommand,
    parse_serial_subcommand, parse_shell_integration_subcommand, parse_splits_subcommand,
    parse_terminal_size_subcommand, parse_tftp_subcommand, parse_theme_subcommand,
};

pub(crate) use subcommands::parse_attention_target;

pub(crate) fn parse_args_from(args: impl IntoIterator<Item = OsString>) -> Result<StartupArgs> {
    let arguments = args.into_iter().collect::<Vec<_>>();
    if let Some(parsed) = parse_subcommand(&arguments)? {
        return Ok(parsed);
    }
    // Only the plain launch reaches the global help: a subcommand's own
    // `--help` is that subcommand's, and `-e`'s belongs to the child command.
    if has_global_help_argument(&arguments) {
        let config_path = arguments
            .windows(2)
            .find(|arguments| matches!(arguments[0].to_string_lossy().as_ref(), "--config" | "-c"))
            .map(|arguments| PathBuf::from(&arguments[1]));
        let (config, _) = load_startup_config(config_path.as_deref(), None);
        println!("{}", help_text(&config.profiles));
        std::process::exit(0);
    }
    parse_application_args(arguments)
}

/// The launch a `zetta <subcommand>` describes, or `None` when the first
/// argument names no subcommand and this is a plain launch.
fn parse_subcommand(arguments: &[OsString]) -> Result<Option<StartupArgs>> {
    // `zetta profile` is the one subcommand that need not be the first
    // argument: a root `--config` may precede it, so it is matched by position.
    // `profile_subcommand_index` returns a position only when everything before
    // it is a global option, so testing it first cannot shadow the subcommands
    // matched below.
    if let Some(profile_index) = profile_subcommand_index(arguments) {
        let config_path = parse_profile_root_config(&arguments[..profile_index])?;
        let parsed = parse_profile_args(&arguments[profile_index + 1..], config_path)?;
        return Ok(Some(StartupArgs {
            config_path: parsed.config_path,
            ..StartupArgs::for_mode(StartupMode::Profile(parsed.command))
        }));
    }
    let Some(name) = arguments.first().map(|name| name.to_string_lossy()) else {
        return Ok(None);
    };
    let rest = &arguments[1..];
    let parsed = match name.as_ref() {
        "project" => StartupArgs::for_mode(StartupMode::Project(parse_project_args(rest)?)),
        "cmd" => StartupArgs::for_mode(StartupMode::ProjectCommand(parse_project_command_args(
            rest,
        )?)),
        "pane" => parse_pane_subcommand(rest)?,
        #[cfg(feature = "worktree")]
        "wt" => StartupArgs::for_mode(StartupMode::Worktree(parse_worktree_args_for(
            rest,
            WorktreeInvocation::Zetta,
        )?)),
        "tabicon" => StartupArgs::for_mode(parse_tab_icon_args(rest)?),
        "theme" => parse_theme_subcommand(rest)?,
        "splits" => parse_splits_subcommand(rest)?,
        "overlay" => StartupArgs::for_mode(parse_overlay_args(rest)?),
        "attention" => parse_attention_subcommand(rest)?,
        "benchmark" => parse_benchmark_subcommand(rest)?,
        "terminal-size" => parse_terminal_size_subcommand(rest)?,
        "mux" => StartupArgs::for_mode(StartupMode::Mux(rest.to_vec())),
        "edit" => parse_edit_subcommand(rest)?,
        "vi" => StartupArgs::for_mode(StartupMode::Vi(
            rest.iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
        )),
        "init" => parse_shell_integration_subcommand(rest)?,
        "serial" => parse_serial_subcommand(rest)?,
        "http" => parse_http_subcommand(rest)?,
        "tftp" => parse_tftp_subcommand(rest)?,
        "notify" => parse_notify_subcommand(rest)?,
        "copy" => parse_copy_subcommand(rest)?,
        "paste" => parse_paste_subcommand(rest)?,
        // Former spellings that now live under a parent subcommand. They are
        // rejected by name rather than falling through to the plain launch,
        // which would report them as an unknown *argument*.
        "panetheme" | "benchmark-output" | "notify-cleanup" => {
            anyhow::bail!("unknown command {:?}", arguments[0])
        }
        _ => return Ok(None),
    };
    Ok(Some(parsed))
}

/// The plain `zetta [OPTIONS] [-e COMMAND …]` launch: the only form that takes
/// the global options, and the only one that ends in a window.
fn parse_application_args(arguments: Vec<OsString>) -> Result<StartupArgs> {
    let mut config = None;
    let mut keymap = None;
    let mut profile = None;
    let mut split = None;
    let mut replace_pane = false;
    let mut theme_override = None;
    let mut no_mux = false;
    #[cfg(windows)]
    let mut mode = StartupMode::Application;
    #[cfg(not(windows))]
    let mut mode = StartupMode::Application;
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
            "--no-mux" | "-n" => {
                anyhow::ensure!(!no_mux, "--no-mux may only be specified once");
                no_mux = true;
            }
            "--new-window" | "-w" => {
                anyhow::ensure!(
                    mode == StartupMode::Application,
                    "--new-window cannot be combined with another startup mode"
                );
                mode = StartupMode::NewWindow;
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
            #[cfg(windows)]
            "--unregister-windows-shell" => {
                mode = StartupMode::UnregisterWindowsShell;
            }
            #[cfg(windows)]
            // COM launches the GUI executable with this switch. It is kept
            // out of the public help because it is an implementation detail of
            // Windows' out-of-process activation protocol.
            "-Embedding" | "--embedding" => {
                anyhow::ensure!(
                    mode == StartupMode::Application,
                    "Windows embedding cannot be combined with another startup mode"
                );
                mode = StartupMode::WindowsEmbedding;
            }
            // GNOME Shell uses this private no-op argument to notice changes
            // to the dynamically generated desktop action list. It is
            // written by the Linux desktop integration and is not user CLI.
            // It deliberately does not change the startup mode: the primary
            // desktop command must retain normal application activation
            // semantics.
            "--zetta-profile-actions-generation" => {
                args.next()
                    .context("--zetta-profile-actions-generation requires a value")?
                    .to_string_lossy()
                    .parse::<u64>()
                    .context("--zetta-profile-actions-generation requires a number")?;
            }
            "--command" | "-e" => {
                anyhow::ensure!(
                    mode == StartupMode::Application,
                    "-e/--command cannot be combined with another startup mode"
                );
                let command = args
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                anyhow::ensure!(
                    !command.is_empty(),
                    "-e/--command requires a command and optional arguments"
                );
                mode = StartupMode::Command(command);
                break;
            }
            "--help" | "-h" => unreachable!("help arguments return before parsing options"),
            unknown => anyhow::bail!("unknown argument {unknown:?}"),
        }
    }
    anyhow::ensure!(
        profile.is_none()
            || matches!(
                mode,
                StartupMode::Application | StartupMode::NewWindow | StartupMode::Command(_)
            ),
        "--profile cannot be combined with another startup mode"
    );
    anyhow::ensure!(
        split.is_none() || matches!(mode, StartupMode::Application | StartupMode::Command(_)),
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
    anyhow::ensure!(
        !no_mux || matches!(mode, StartupMode::Application | StartupMode::Command(_)),
        "--no-mux cannot be combined with another startup mode"
    );
    if mode == StartupMode::NewWindow {
        anyhow::ensure!(
            config.is_none(),
            "--new-window cannot be combined with --config"
        );
        anyhow::ensure!(
            keymap.is_none(),
            "--new-window cannot be combined with --keymap"
        );
        anyhow::ensure!(
            split.is_none(),
            "--new-window cannot be combined with --split"
        );
        anyhow::ensure!(
            !replace_pane,
            "--new-window cannot be combined with --replace-pane"
        );
        anyhow::ensure!(
            theme_override.is_none(),
            "--new-window cannot be combined with --theme"
        );
        anyhow::ensure!(!no_mux, "--new-window cannot be combined with --no-mux");
    }
    Ok(StartupArgs {
        config_path: config,
        keymap_path: keymap,
        profile,
        split,
        replace_pane,
        theme_override,
        no_mux,
        mode,
        profile_report: None,
        profile_duration: None,
        profile_pane_stress: false,
        profile_workload: PerformanceWorkload::Standard,
        profile_external_terminal: false,
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
            "--replace-pane" | "-r" | "--no-mux" | "-n" => index += 1,
            "--command" | "-e" => return None,
            "--help" | "-h" | "--version" | "-v" => index += 1,
            "profile" => return Some(index),
            _ => return None,
        }
    }
    None
}

fn has_global_help_argument(arguments: &[OsString]) -> bool {
    for argument in arguments {
        match argument.to_string_lossy().as_ref() {
            // Everything after -e/--command belongs to the child command,
            // including a literal `--help`.
            "--command" | "-e" => return false,
            "--help" | "-h" => return true,
            _ => {}
        }
    }
    false
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
    // Plain `--profile` launches intentionally remain independent processes;
    // only the explicit fresh-window mode can carry a profile to an existing
    // process.
    matches!(
        args.mode,
        StartupMode::Application | StartupMode::NewWindow | StartupMode::Command(_)
    ) && args.config_path.is_none()
        && args.keymap_path.is_none()
        && !args.replace_pane
        && !args.no_mux
        && (args.profile.is_none() || args.mode == StartupMode::NewWindow)
        && args.split.is_none()
}

pub(crate) fn should_replace_pane_in_existing_process(args: &StartupArgs) -> bool {
    args.mode == StartupMode::Application
        && args.replace_pane
        && !args.no_mux
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
    // A native Zetta can be launched beside another installation (for example
    // a debug build beside the installed application). Point shell
    // integration at the executable that owns this terminal instead of
    // relying on the shell's eventual PATH ordering. On Linux this also
    // replaces a Windows-host routing marker inherited by the application with
    // the native executable's path.
    let executable = env::current_exe().ok();
    environment.push((
        "ZETTA_HOST_EXECUTABLE".to_owned(),
        executable
            .as_ref()
            .map(|executable| executable.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ));

    let Some(executable_directory) =
        executable.and_then(|executable| executable.parent().map(Path::to_path_buf))
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
