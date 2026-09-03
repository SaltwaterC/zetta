use super::*;
#[cfg(cli_services)]
use crate::cli_services::CliServiceCommand;
use crate::cli_services::NotificationTarget;
#[cfg(all(target_os = "macos", feature = "notifications"))]
use crate::cli_services::macos_notification_target_for_response;
#[cfg(feature = "clipboard")]
use crate::cli_services::{copy_help, parse_copy_args, parse_paste_args, paste_help};
#[cfg(feature = "http-server")]
use crate::cli_services::{http_server_help, parse_http_args};
#[cfg(notify_cleanup_enabled)]
use crate::cli_services::{notify_cleanup_help, parse_notify_cleanup_args};
#[cfg(feature = "notifications")]
use crate::cli_services::{notify_help, parse_notify_args, run_notification};
#[cfg(feature = "serial-console")]
use crate::cli_services::{parse_serial_args, serial_help};
#[cfg(feature = "tftp-server")]
use crate::cli_services::{parse_tftp_server_args, tftp_server_help};
use crate::process_control::{
    ReplacePaneRequest, TabAttentionRequest, request_existing_process_command,
    request_existing_process_new_window, request_existing_process_pane,
    request_existing_process_pane_labels, request_existing_process_pane_overlay,
    request_existing_process_project_with_working_directory,
    request_existing_process_projects_reload, request_existing_process_replace_pane,
    request_existing_process_shell_command, request_existing_process_tab_icon,
    request_existing_process_theme, request_existing_process_theme_list, request_process_run_wait,
    request_process_tab_attention,
};
use crate::project_commands::{ProjectCommandInvocation, merge_command_environment};
use crate::run_command::{PaneWaitCommand, RunWaitRequest, process_run_registry};
#[cfg(feature = "worktree")]
use zwt::{WorktreeInvocation, run_for as run_worktree};

use gpui::{KeyBindingContextPredicate, Unbind};
#[cfg(target_os = "macos")]
use gpui::{Menu, MenuItem};
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags};
use serde_json::Value;
use std::rc::Rc;

mod arg_parsing;
mod cli_help;
mod cli_modes;
mod keybindings;
mod process_control_loop;
mod theming;
mod watchers;
mod window;
mod workload;
mod wsl;

// `main.rs` pulls this module in with `use startup::*`, so what these three
// hold is still named `crate::…` exactly as it was before the split.
pub(crate) use theming::*;
pub(crate) use watchers::*;
pub(crate) use window::*;

use arg_parsing::{
    StartupArgs, configured_split_names, parse_attention_target, select_launch_profile,
    should_handoff_to_existing_process, should_replace_pane_in_existing_process,
    validate_launch_split,
};
pub(crate) use arg_parsing::{
    StartupMode, load_startup_config, native_terminal_environment, parse_args,
};
#[cfg(not(feature = "tftp-client"))]
pub(crate) use cli_help::{TftpCommand, parse_tftp_args, tftp_help};
pub(crate) use cli_help::{command_help, format_help_table};
pub(crate) use keybindings::{
    PROFILE_SHORTCUT_KEYS, keymap_keystroke_display, keymap_keystroke_storage, load_keybindings,
    profile_keybindings, profile_shortcut_label,
};
#[cfg(test)]
pub(crate) use keybindings::{RENAME_TAB_KEYBINDING, keymap_keystroke_alias};
#[cfg(target_os = "macos")]
pub(crate) use keybindings::{
    install_native_macos_menus, update_native_macos_dock_menu, update_native_macos_menus,
};
#[cfg(windows)]
pub(crate) use wsl::Msys2Shell;
use wsl::paths_for_external_editor;
pub(crate) use wsl::{
    add_wsl_environment_variable_names, add_wsl_environment_variables, cygwin_path_to_windows,
    cygwin_profile, is_wsl_shell, launch_working_directory, msys2_cwd_tracking_environment,
    msys2_path_to_windows, msys2_profile, wsl_cwd_tracking_file, wsl_shell_with_tracking,
    wsl_terminal_environment,
};
#[cfg(windows)]
pub(crate) use wsl::{
    cygwin_cwd_tracking_environment_with_path, cygwin_shell_with_tracking,
    ensure_cygwin_environment,
};

fn terminal_rendering_profile_config(executable: &Path, workload: PerformanceWorkload) -> Config {
    let mut config = Config::defaults(None, None);
    let workload_argument = match workload {
        PerformanceWorkload::Standard => "--terminal-render-workload",
        PerformanceWorkload::CheckerboardBackground => "--terminal-checkerboard-workload",
        PerformanceWorkload::SparseUpdates => "--terminal-sparse-update-workload",
        PerformanceWorkload::AltScreenScroll => "--terminal-alt-screen-scroll-workload",
    };
    config.profiles = vec![Profile {
        name: "Terminal rendering profiler".to_owned(),
        command: Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec!["benchmark".to_owned(), workload_argument.to_owned()],
            title_override: Some("Terminal rendering profiler".to_owned()),
        },
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    }];
    config.default_profile = 0;
    config
}

trait ApplicationOpenUrlsExt {
    fn with_open_url_handler(
        self,
        control_tx: futures::channel::mpsc::UnboundedSender<ProcessControlCommand>,
    ) -> Self;
}

impl ApplicationOpenUrlsExt for gpui::Application {
    fn with_open_url_handler(
        self,
        control_tx: futures::channel::mpsc::UnboundedSender<ProcessControlCommand>,
    ) -> Self {
        #[cfg(target_os = "macos")]
        self.on_open_urls(move |urls| {
            let _ = control_tx.unbounded_send(ProcessControlCommand::OpenUrls(urls));
        });
        #[cfg(not(target_os = "macos"))]
        let _ = control_tx;
        self
    }
}

pub(crate) fn run() -> Result<()> {
    let args = parse_args()?;
    // `zetta tftp get|put` parses as `StartupMode::Application` carrying a
    // client command rather than as a mode of its own, so it is dispatched
    // ahead of the mode match. It used to be dispatched after the
    // handoff-to-a-running-process checks below, where an Application-mode
    // launch beside a running Zetta was satisfied by raising that process's
    // window and the transfer never ran.
    if let Some(command) = &args.tftp_command {
        return command.run();
    }
    match dispatch_startup_mode(&args) {
        Some(result) => result,
        None => run_application(args),
    }
}

/// Runs the subcommand `args.mode` names, or returns `None` for the modes that
/// end in a window this process owns.
///
/// The match is exhaustive on purpose. This replaced a sequence of 36
/// `if let StartupMode::X(..)` and `args.mode == StartupMode::X` tests, in
/// which a new variant matched none of them and silently fell through to a GUI
/// launch; now it has to name the function that handles it, or say explicitly
/// that it launches a window.
fn dispatch_startup_mode(args: &StartupArgs) -> Option<Result<()>> {
    Some(match &args.mode {
        StartupMode::Application
        | StartupMode::NewWindow
        | StartupMode::Command(_)
        | StartupMode::Project(crate::project_cli::ProjectCommand::Open { .. })
        | StartupMode::TerminalRenderingProfile => return None,
        #[cfg(windows)]
        StartupMode::WindowsEmbedding => return None,
        StartupMode::PaneWait(command) => cli_modes::run_wait_command(command.clone()),
        StartupMode::ProjectCommand(invocation) => {
            cli_modes::run_registered_project_command(invocation)
        }
        StartupMode::Project(command) => cli_modes::run_project_registry_command(command),
        StartupMode::Edit {
            arguments,
            delete_after,
        } => cli_modes::run_editor(arguments, *delete_after),
        StartupMode::Vi(arguments) => cli_modes::run_vi(arguments.clone()),
        StartupMode::OutputBenchmark {
            size_mib,
            output_type,
        } => run_output_benchmark(*size_mib, *output_type),
        StartupMode::Pane(request) => cli_modes::run_pane_command(request),
        StartupMode::Attention(command) => cli_modes::run_attention_command(command),
        #[cfg(feature = "worktree")]
        StartupMode::Worktree(command) => run_worktree(command, WorktreeInvocation::Zetta),
        StartupMode::Profile(command) => {
            cli_modes::run_profile_command(command.clone(), args.config_path.as_deref())
        }
        StartupMode::PrintTerminalSize { json, resize } => {
            cli_modes::run_terminal_size_command(*json, *resize)
        }
        StartupMode::PrintShellIntegration(shell) => {
            print!("{}", shell.script());
            Ok(())
        }
        StartupMode::ConfigureCurrentShellIntegration => cli_modes::configure_shell_integration(),
        StartupMode::ListTabIcons => cli_modes::list_tab_icons(),
        StartupMode::SetTabIcon { icon } => cli_modes::set_tab_icon(*icon),
        StartupMode::SetTheme { scope, theme } => cli_modes::set_theme(*scope, theme.clone()),
        StartupMode::ListThemes => cli_modes::list_themes(),
        StartupMode::ListPaneSplits => cli_modes::list_pane_splits(),
        StartupMode::SetPaneOverlay(request) => cli_modes::set_pane_overlay(request.clone()),
        StartupMode::Mux(arguments) => {
            cli_modes::run_mux_command(arguments, args.config_path.clone())
        }
        #[cfg(cli_services)]
        StartupMode::CliService(command) => command.run(),
        #[cfg(windows)]
        StartupMode::RegisterWindowsShell(shortcut_path) => cli_modes::register_windows_shell(
            shortcut_path,
            args.config_path.as_deref(),
            args.keymap_path.clone(),
        ),
        #[cfg(windows)]
        StartupMode::UnregisterWindowsShell => windows_integration::unregister_shell_integration(),
        StartupMode::TerminalRenderingWorkload => {
            workload::run_terminal_rendering_workload(PerformanceWorkload::Standard, None)
        }
        StartupMode::TerminalCheckerboardWorkload => workload::run_terminal_rendering_workload(
            PerformanceWorkload::CheckerboardBackground,
            None,
        ),
        StartupMode::TerminalSparseUpdateWorkload => {
            workload::run_terminal_rendering_workload(PerformanceWorkload::SparseUpdates, None)
        }
        StartupMode::TerminalAltScreenScrollWorkload => {
            workload::run_terminal_rendering_workload(PerformanceWorkload::AltScreenScroll, None)
        }
    })
}

/// The project a GUI launch opens: named by `zetta project open`, or detected
/// from the directory a plain launch was started in.
#[derive(Debug, Default)]
struct StartupProject {
    root: Option<PathBuf>,
    working_directory: Option<PathBuf>,
}

/// A launch that ends in a window: the handoffs a running process can take,
/// then the GUI.
fn run_application(args: StartupArgs) -> Result<()> {
    let activation_token = (args.mode == StartupMode::NewWindow)
        .then(|| env::var("XDG_ACTIVATION_TOKEN").ok())
        .flatten();
    if args.mode == StartupMode::NewWindow
        && should_handoff_to_existing_process(&args)
        && request_existing_process_new_window(
            args.profile.as_deref(),
            activation_token.as_deref(),
        )?
    {
        return Ok(());
    }
    let Some(project) = resolve_startup_project(&args)? else {
        return Ok(());
    };
    if args.mode == StartupMode::Application
        || matches!(
            args.mode,
            StartupMode::Project(crate::project_cli::ProjectCommand::Open { .. })
        )
    {
        terminal_view::start_scrollback_cleanup_monitor();
    }
    if handed_off_to_existing_process(&args, project.root.as_deref())? {
        return Ok(());
    }
    launch_gui(args, project, activation_token)
}

/// Resolves the project a GUI launch opens, offering it to a running Zetta
/// process first.
///
/// `None` means a running process took the launch and this one has nothing
/// left to do. This runs before [`handed_off_to_existing_process`] because a
/// resolved project makes the plain window handoff there the wrong answer:
/// raising a window would drop the project, so that handoff is skipped once a
/// project resolves.
fn resolve_startup_project(args: &StartupArgs) -> Result<Option<StartupProject>> {
    if let StartupMode::Project(crate::project_cli::ProjectCommand::Open { path }) = &args.mode {
        let target = crate::project_cli::resolve_open_target(path.as_deref())?;
        if request_existing_process_project_with_working_directory(
            &target.root,
            target.working_directory.as_deref(),
        )? {
            return Ok(None);
        }
        return Ok(Some(StartupProject {
            root: Some(target.root),
            working_directory: target.working_directory,
        }));
    }
    // A plain launch adopts the project of the directory it started in, but
    // not when it carries configuration of its own or is replacing a pane in
    // another window.
    if args.mode != StartupMode::Application
        || args.config_path.is_some()
        || args.keymap_path.is_some()
        || args.replace_pane
    {
        return Ok(Some(StartupProject::default()));
    }
    let mut project = StartupProject::default();
    match crate::project_cli::current_project_target() {
        Ok(Some(target)) => {
            project.working_directory = target.working_directory;
            project.root = Some(target.root);
        }
        Ok(None) => {}
        Err(error) => eprintln!("Could not load the Zetta project registry: {error:#}"),
    }
    if let Some(root) = project.root.as_ref()
        && should_handoff_to_existing_process(args)
        && request_existing_process_project_with_working_directory(
            root,
            project.working_directory.as_deref(),
        )?
    {
        return Ok(None);
    }
    Ok(Some(project))
}

/// Offers this launch to a running Zetta process, returning whether one took
/// it.
///
/// The three are disjoint rather than ordered by precedence:
/// `should_replace_pane_in_existing_process` requires `--replace-pane`, the
/// command handoff requires [`StartupMode::Command`], and the window handoff
/// requires [`StartupMode::Application`] *without* `--replace-pane` — which
/// `should_handoff_to_existing_process` also demands. So at most one of them
/// can apply to any launch.
fn handed_off_to_existing_process(args: &StartupArgs, project_root: Option<&Path>) -> Result<bool> {
    if should_replace_pane_in_existing_process(args) {
        let request = ReplacePaneRequest {
            split: args.split.clone(),
            profile: args.profile.clone(),
            theme: args.theme_override.clone(),
        };
        if request_existing_process_replace_pane(request)? {
            return Ok(true);
        }
    }
    if should_handoff_to_existing_process(args)
        && let StartupMode::Command(command) = &args.mode
    {
        let request = crate::command_panes::PaneCommand {
            direction: None,
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            list: false,
            command: command.clone(),
        };
        if request_existing_process_command(request, Some(env::current_dir()?))? {
            return Ok(true);
        }
    }
    if project_root.is_none()
        && args.mode == StartupMode::Application
        && should_handoff_to_existing_process(args)
        && request_existing_process_window()?
    {
        return Ok(true);
    }
    Ok(false)
}

/// Everything a GUI launch resolves before the application loop starts.
///
/// Built by [`resolve_application_launch`] and consumed inside the application
/// closure, which keeps that closure a list of initialization steps rather
/// than a closure over two dozen locals.
struct ApplicationLaunch {
    config: Config,
    configuration_error: Option<String>,
    keymap_path: PathBuf,
    profile_count: usize,
    no_mux: bool,
    http_client: Arc<reqwest_client::ReqwestClient>,
    /// The terminal-rendering profiler, which supplies its own configuration
    /// and opens its window with the performance overlay on.
    profiling: bool,
    performance_report: Option<(PerformanceReportOptions, PerformanceReportStatus)>,
    /// Windows only: this process embeds a handed-off console, so it opens no
    /// window until the handoff arrives over the control channel.
    embedding: bool,
    /// `zetta --new-window` that no running process took. It opens a window
    /// from process configuration and resolves its own profile, so the
    /// resolved project, profile, command, split, and theme override below do
    /// not apply to it.
    fresh_window: bool,
    fresh_window_profile: Option<String>,
    activation_token: Option<String>,
    initial_profile: Option<Profile>,
    initial_project: Option<ProjectConfig>,
    launch_theme_override: Option<(String, String)>,
    launch_split: Option<String>,
    profile_pane_stress: bool,
    initial_command: Option<Vec<String>>,
    initial_working_directory: Option<PathBuf>,
}

fn launch_gui(
    args: StartupArgs,
    project: StartupProject,
    activation_token: Option<String>,
) -> Result<()> {
    let profiling = args.mode == StartupMode::TerminalRenderingProfile;
    if profiling && args.profile_external_terminal {
        return workload::run_terminal_rendering_workload(
            args.profile_workload,
            args.profile_duration,
        );
    }
    let report_status: PerformanceReportStatus = Arc::new(Mutex::new(None));
    let launch = resolve_application_launch(args, project, activation_token, &report_status)?;
    let report_requested = launch.performance_report.is_some();
    start_application(launch);
    // `Zetta::start_performance_report` writes the outcome into `report_status`
    // as the profiling window finishes, so a failed report becomes this
    // process's exit status rather than only a log line.
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

fn resolve_application_launch(
    args: StartupArgs,
    project: StartupProject,
    activation_token: Option<String>,
    report_status: &PerformanceReportStatus,
) -> Result<ApplicationLaunch> {
    let profiling = args.mode == StartupMode::TerminalRenderingProfile;
    let workload = args.profile_workload;
    let performance_report =
        args.profile_report
            .zip(args.profile_duration)
            .map(|(path, duration)| {
                (
                    PerformanceReportOptions {
                        path,
                        duration,
                        workload,
                    },
                    report_status.clone(),
                )
            });
    let (config, configuration_error) = if profiling {
        (
            terminal_rendering_profile_config(&env::current_exe()?, workload),
            None,
        )
    } else {
        load_startup_config(args.config_path.as_deref(), args.keymap_path)
    };
    let mut initial_project = project
        .root
        .as_deref()
        .map(|root| ProjectConfig::load(root, &config))
        .transpose()?;
    // An explicit `--split` replaces the project's own initial layout rather
    // than being applied on top of it.
    if args.split.is_some()
        && let Some(project) = initial_project.as_mut()
    {
        project.initial_split = None;
    }
    let effective_launch_config = initial_project
        .as_ref()
        .map(|project| &project.effective)
        .unwrap_or(&config);
    validate_launch_split(effective_launch_config, args.split.as_deref())?;
    let initial_profile = select_launch_profile(effective_launch_config, args.profile.as_deref())?;
    // Keyed by profile name (case-insensitive) rather than baked into
    // `initial_profile.theme`, so every tab opened with this profile for the
    // rest of the process gets the override too, not just the first one.
    // Applied in `Zetta::open_tab_with_profile`; never written back to
    // `config.profiles` or the settings UI.
    let launch_theme_override = initial_profile
        .as_ref()
        .zip(args.theme_override.as_ref())
        .map(|(profile, theme)| (profile.name.to_lowercase(), theme.clone()));
    let initial_command = match &args.mode {
        StartupMode::Command(command) => Some(command.clone()),
        _ => None,
    };
    let initial_working_directory = match project.working_directory {
        Some(directory) => Some(directory),
        None => initial_command
            .as_ref()
            .map(|_| env::current_dir())
            .transpose()?,
    };
    let profile_count = visible_profile_count(
        &effective_launch_config.profiles,
        &effective_launch_config.hidden_profiles,
    );
    #[cfg(windows)]
    let embedding = args.mode == StartupMode::WindowsEmbedding;
    #[cfg(not(windows))]
    let embedding = false;
    Ok(ApplicationLaunch {
        keymap_path: config.keymap_path.clone(),
        config,
        configuration_error,
        profile_count,
        no_mux: args.no_mux,
        http_client: Arc::new(
            reqwest_client::ReqwestClient::user_agent(concat!("Zetta/", env!("CARGO_PKG_VERSION")))
                .context("initializing HTTP client")?,
        ),
        profiling,
        performance_report,
        embedding,
        fresh_window: args.mode == StartupMode::NewWindow,
        fresh_window_profile: args.profile,
        activation_token,
        initial_profile,
        initial_project,
        launch_theme_override,
        launch_split: args.split,
        profile_pane_stress: args.profile_pane_stress,
        initial_command,
        initial_working_directory,
    })
}

/// Runs the GPUI application loop until the last window closes.
fn start_application(launch: ApplicationLaunch) {
    let (control_tx, control_rx) = futures::channel::mpsc::unbounded();
    gpui_platform::application()
        .with_quit_mode(zetta_quit_mode())
        .with_assets(ZettaAssets)
        .with_open_url_handler(control_tx.clone())
        .run(move |cx: &mut App| {
            #[cfg(windows)]
            {
                cx.set_app_identity(ZETTA_APP_ID, "Zetta");
                windows_integration::update_profile_jump_list(
                    launch.config.profiles.clone(),
                    launch.config.hidden_profiles.clone(),
                );
            }
            #[cfg(target_os = "linux")]
            let linux_desktop_entry_managed = {
                linux_desktop::update_profile_actions(
                    &launch.config.profiles,
                    &launch.config.hidden_profiles,
                )
                .log_err();
                // The desktop entry may have been replaced by the installer
                // immediately before this process started. In that case its
                // contents already match the current configuration and the
                // updater reports no change, but GNOME Shell may still be
                // holding the old cached app record.
                linux_desktop::is_managed_user_desktop_entry()
            };
            initialize_zetta_settings(&launch, cx);
            initialize_process_state(&launch, control_tx, cx);
            install_process_observers(&launch, cx);
            cx.spawn(async move |cx| process_control_loop::serve(control_rx, cx).await)
                .detach();
            open_launch_window(launch, cx);
            #[cfg(target_os = "linux")]
            if linux_desktop_entry_managed {
                schedule_linux_desktop_window_reassociation(cx);
            }
        });
}

/// Brings up the settings, theme, font, and keybinding state every window
/// reads from.
fn initialize_zetta_settings(launch: &ApplicationLaunch, cx: &mut App) {
    cx.set_http_client(launch.http_client.clone());
    menu::init();
    zed_actions::init();
    release_channel::init(semver::Version::new(0, 1, 0), cx);
    settings::init(cx);
    theme_settings::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
    load_user_themes(cx).log_err();
    ZettaAssets.load_fonts(cx).log_err();
    apply_config_settings(&launch.config, cx).expect("failed to apply Zetta configuration");
    load_keybindings(&launch.keymap_path, launch.profile_count, launch.no_mux, cx);
    #[cfg(target_os = "macos")]
    install_native_macos_menus(
        cx,
        &launch.config.profiles,
        &launch.config.hidden_profiles,
        launch.config.default_profile,
    );
    #[cfg(target_os = "macos")]
    update_native_macos_dock_menu(cx, &launch.config.profiles, &launch.config.hidden_profiles);
}

/// Starts the control server, publishes [`ZettaProcessState`], and starts the
/// watchers that keep it current.
fn initialize_process_state(
    launch: &ApplicationLaunch,
    control_tx: futures::channel::mpsc::UnboundedSender<ProcessControlCommand>,
    cx: &mut App,
) {
    #[cfg(windows)]
    let handoff_control_tx = control_tx.clone();
    let control_server =
        ProcessControlServer::start(control_tx).expect("failed to start Zetta process control");
    #[cfg(windows)]
    if launch.embedding {
        windows_integration::start_handoff_server(handoff_control_tx)
            .expect("failed to start the Windows terminal handoff server");
    }
    let quit_subscription = cx.on_app_quit(|cx| {
        if cx.has_global::<ZettaProcessState>() {
            cx.global::<ZettaProcessState>()
                .control_server
                .begin_shutdown();
        }
        shutdown_multiplexer_if_idle(cx);
        async {}
    });
    cx.set_global(ZettaProcessState {
        windows: HashMap::new(),
        dormant: Vec::new(),
        runners: HashMap::new(),
        next_attention_id: 1,
        silent_mode: SilentModeState::default(),
        background_session_entries: Arc::from([]),
        config: launch.config.clone(),
        config_file_stamp: config_file_stamp(&launch.config.config_path),
        configuration_error: launch.configuration_error.clone(),
        no_mux: launch.no_mux,
        #[cfg(target_os = "linux")]
        linux_desktop_reassociation_generation: 0,
        control_server,
        _quit_subscription: quit_subscription,
    });
    start_configuration_watcher(cx);
    silent_mode::start_observer(cx);
    if !launch.no_mux {
        start_multiplexer_session_watcher(cx);
    }
}

/// Installs the process-wide observers: notification responses, the tab
/// overflow menu's keystroke interception, the macOS window actions, keyboard
/// layout changes, and window teardown.
fn install_process_observers(launch: &ApplicationLaunch, cx: &mut App) {
    #[cfg(all(target_os = "macos", feature = "notifications"))]
    cx.on_system_notification_response(|response, cx| {
        let target = macos_notification_target_for_response(
            response.tag.as_ref(),
            response.action_id.as_deref(),
        );
        if let Some(target) = target {
            focus_visible_tab_by_attention_id(cx, target.attention_id);
        }
    });
    install_tab_overflow_keystroke_interception(cx);
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &NewWindow, cx| {
        open_fresh_zetta_window(cx).log_err();
    });
    #[cfg(target_os = "macos")]
    cx.on_action(|action: &OpenProfileWindow, cx| {
        open_fresh_zetta_window_with_profile_and_activation_token(
            cx,
            Some(action.profile.clone()),
            None,
        )
        .log_err();
    });
    let keymap_path = launch.keymap_path.clone();
    let profile_count = launch.profile_count;
    let no_mux = launch.no_mux;
    cx.on_keyboard_layout_change(move |cx| {
        let keymap_path = keymap_path.clone();
        cx.defer(move |cx| {
            load_keybindings(&keymap_path, profile_count, no_mux, cx);
        });
    })
    .detach();
    cx.on_window_closed(handle_zetta_window_closed).detach();
}

/// Lets the tab overflow menu cycle with tab and page keys.
///
/// `intercept_keystrokes` fires before action dispatch, which is what lets
/// `stop_propagation` here claim keys a keybinding would otherwise take. The
/// handler returns early unless the menu is open, so those keys keep their
/// normal meaning the rest of the time.
fn install_tab_overflow_keystroke_interception(cx: &mut App) {
    cx.intercept_keystrokes(|event, _window, cx| {
        let reverse = match event.keystroke.key.as_str() {
            "tab" => event.keystroke.modifiers.shift,
            "pageup" => false,
            "pagedown" => true,
            _ => return,
        };
        let Some(window_handle) = cx.active_window() else {
            return;
        };
        let should_cycle = window_handle
            .update(cx, |view, _window, cx| {
                view.downcast::<Zetta>()
                    .is_ok_and(|zetta| zetta.read(cx).tab_overflow_keyboard_menu_edge.is_some())
            })
            .unwrap_or(false);
        if !should_cycle {
            return;
        }

        cx.stop_propagation();
        cx.defer(move |cx| {
            let _ = window_handle.update(cx, |view, window, cx| {
                let Ok(zetta) = view.downcast::<Zetta>() else {
                    return;
                };
                zetta.update(cx, |zetta, cx| {
                    if reverse {
                        zetta.previous_tab(&PreviousTab, window, cx);
                    } else {
                        zetta.next_tab(&NextTab, window, cx);
                    }
                });
            });
        });
    })
    .detach();
}

/// Opens the window this launch asked for, if it asks for one at all.
fn open_launch_window(launch: ApplicationLaunch, cx: &mut App) {
    if launch.embedding {
        return;
    }
    let opened = if launch.fresh_window {
        open_fresh_zetta_window_with_profile_and_activation_token(
            cx,
            launch.fresh_window_profile,
            launch.activation_token,
        )
    } else {
        open_zetta_window(
            launch.config,
            launch.configuration_error,
            launch.initial_profile,
            launch.initial_project,
            launch.launch_theme_override,
            launch.launch_split,
            launch.profiling,
            launch.performance_report,
            launch.profile_pane_stress,
            launch.no_mux,
            launch.initial_command,
            launch.initial_working_directory,
            None,
            None,
            cx,
        )
    };
    opened.expect("failed to open Zetta window");
}

#[cfg(test)]
#[path = "tests/startup.rs"]
mod tests;
