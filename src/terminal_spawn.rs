//! Spawning a pane's terminal: the environment and shell integration it starts
//! with, local and shared sessions, and wiring the result into the pane.

use super::*;
#[cfg(feature = "zmux")]
use crate::mux::MuxPaneIds;
#[cfg(feature = "zmux")]
use crate::mux::SharedSessionKey;
use crate::worktree_detection::terminal_event_requires_worktree_detection;

#[cfg(feature = "zmux")]
mod shared;
#[cfg(feature = "zmux")]
pub(crate) use shared::*;
mod stacked;
#[cfg(windows)]
mod windows_handoff;
pub(crate) use stacked::*;

/// PowerShell loads its integration from the command passed at process start,
/// before the first prompt. Sending a second line through its terminal makes
/// that line visible in the user's shell.
#[cfg(windows)]
fn shell_integration_startup_command(_shell: &Shell) -> Option<Vec<u8>> {
    None
}

/// Returns the shell command used to load this process's shell integration
/// into an interactive native shell. The command is sent after the shell's
/// startup files have completed so a stale `zetta` found earlier on PATH
/// cannot leave the pane with CWD-only tracking.
#[cfg(not(windows))]
fn shell_integration_startup_command(shell: &Shell) -> Option<Vec<u8>> {
    let (program, arguments) = shell.program_and_args();
    if zetta_profiles::runs_a_command(arguments) {
        return None;
    }

    let shell_name = Path::new(&program)
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    let command = match shell_name.as_str() {
        "bash" | "bash.exe" => {
            r#"if [[ ${__ZETTA_LIFECYCLE_TRACKING_INSTALLED:-0} != 1 || ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ]]; then eval "$(command zetta init bash)"; fi"#
        }
        "zsh" | "zsh.exe" => {
            r#"if [[ ${__ZETTA_LIFECYCLE_TRACKING_VERSION:-0} != 1 || ( -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} && ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ) ]]; then eval "$(command zetta init zsh)"; fi"#
        }
        "fish" | "fish.exe" => {
            r#"if not set -q __ZETTA_LIFECYCLE_TRACKING_INSTALLED; or test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" != 1; command zetta init fish | source; end"#
        }
        _ => return None,
    };
    let mut command = command.as_bytes().to_vec();
    command.push(b'\r');
    Some(command)
}

#[derive(Clone)]
pub(crate) struct RestoredTerminalOptions {
    replay: Option<Vec<u8>>,
    prefill: Option<String>,
}
/// What wiring a stacked command's terminal needs once its builder resolves.
///
/// The stacked equivalent of [`SpawnedTerminal`]: a stacked entry belongs to a
/// pane's stack rather than to the pane itself, so it carries the entry it is
/// becoming. The task state that reports the command's exit is consumed by the
/// builder and does not travel with it.
struct SpawnedStackedTerminal {
    tab_id: u64,
    pane_id: u64,
    entry_id: u64,
    attention_id: u64,
    pane_routing_id: u64,
    terminal_theme: Option<Arc<Theme>>,
    mux_provider: Option<Arc<crate::mux::MuxPtyProvider>>,
    image_paste_handler: Arc<crate::ssh_image_paste::SshImagePasteHandler>,
}

/// What the spawn callback still needs once the terminal builder resolves.
///
/// The builder is awaited off the render path, so everything the wiring after
/// it depends on has to cross into that callback. These are those values, as
/// one bundle rather than eleven captured locals — which is also what lets the
/// success and failure paths be named functions rather than two arms of a
/// closure.
struct SpawnedTerminal {
    tab_id: u64,
    pane_id: u64,
    /// Reported to shell integration so `zetta attention` can address the pane.
    attention_id: u64,
    pane_routing_id: u64,
    terminal_theme: Option<Arc<Theme>>,
    restore_options: Option<RestoredTerminalOptions>,
    /// Only set while restoring: the directory the stored session was in.
    restored_working_directory: Option<PathBuf>,
    mux_provider: Option<Arc<crate::mux::MuxPtyProvider>>,
    /// Set when the pane belongs to a shared session.
    #[cfg(feature = "zmux")]
    shared: Option<SharedSpawn>,
    shell_integration_startup_command: Option<Vec<u8>>,
    tracked_multi_command_launch: bool,
    image_paste_handler: Option<Arc<dyn terminal::ImagePasteHandler>>,
}

struct LocalTerminalLaunch {
    /// The grid the terminal starts at before its first layout, if known.
    initial_bounds: Option<terminal::InitialTerminalSize>,
    tab_id: u64,
    pane_id: u64,
    attention_id: u64,
    pane_routing_id: u64,
    terminal_theme: Option<Arc<Theme>>,
    shell: Shell,
    environment: HashMap<String, String>,
    working_directory: Option<PathBuf>,
    path_hyperlink_regexes: Vec<String>,
    restore_options: Option<RestoredTerminalOptions>,
    mux_provider: Option<Arc<crate::mux::MuxPtyProvider>>,
    initial_console_palette: Option<terminal::ConsoleLaunchOptions>,
    shell_integration_startup_command: Option<Vec<u8>>,
    tracked_multi_command_launch: bool,
}

/// Everything one interactive-terminal spawn needs. The tab, the pane and the
/// profile are always known; every other input has a default, so a caller
/// names only what it varies. This replaced a ladder of forwarding
/// constructors that differed from each other by one defaulted argument.
pub(crate) struct TerminalSpawnRequest {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
    pub(crate) profile: Profile,
    /// The shell to run. `None` derives it from `profile.command`, wrapping it
    /// in the WSL or Cygwin CWD-tracking launcher when the profile needs one.
    /// A caller that has already built a shell — a `zetta pane` command, say —
    /// passes it here, and both the derivation and `wsl_directory`, which only
    /// feeds it, are skipped.
    pub(crate) shell: Option<Shell>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
    pub(crate) wsl_cwd_file: Option<PathBuf>,
    pub(crate) terminal_theme: Option<Arc<Theme>>,
    pub(crate) path_hyperlink_regexes: Vec<String>,
    /// Environment overrides layered over the tab's project environment.
    pub(crate) environment: HashMap<String, String>,
    pub(crate) tracked_multi_command_launch: bool,
    /// Set through [`TerminalSpawnRequest::restored`]; only visible so that
    /// callers can use struct-update syntax against `new`.
    pub(crate) restore: Option<RestoredTerminalOptions>,
}

impl TerminalSpawnRequest {
    pub(crate) fn new(tab_id: u64, pane_id: u64, profile: Profile) -> Self {
        Self {
            tab_id,
            pane_id,
            profile,
            shell: None,
            working_directory: None,
            wsl_directory: None,
            wsl_cwd_file: None,
            terminal_theme: None,
            path_hyperlink_regexes: Vec::new(),
            environment: HashMap::new(),
            tracked_multi_command_launch: false,
            restore: None,
        }
    }

    /// Starts the shell in a daemon-created restore session. The saved screen
    /// is handed to the provider as a one-shot replay, while the shell itself
    /// is always created by the daemon from the saved profile and CWD.
    #[cfg(feature = "session-persistence")]
    pub(crate) fn restored(mut self, replay: Option<Vec<u8>>, prefill: Option<String>) -> Self {
        self.restore = Some(RestoredTerminalOptions { replay, prefill });
        self
    }
}

/// The inputs the environment of a spawned terminal is built from.
struct TerminalEnvironment<'a> {
    /// The profile's own command, not the shell derived from it: the CWD
    /// tracking a profile needs is chosen from how the user configured it.
    profile: &'a Shell,
    /// Overrides layered over the inherited environment. `ZETTA_`-prefixed
    /// names are ignored; see [`apply_terminal_environment_overrides`].
    overrides: &'a HashMap<String, String>,
    attention_id: u64,
    /// The terminal's identity for shell integration: the pane id for an
    /// interactive shell, the stack entry id for a command terminal.
    tracking_id: u64,
    routing_id: u64,
    wsl_cwd_file: Option<&'a Path>,
    theme_name: &'a str,
    no_mux: bool,
}

impl TerminalEnvironment<'_> {
    /// Builds the environment: the inherited native environment plus the
    /// profile's CWD-tracking variables, the caller's overrides, and the
    /// pane's routing identity. Errors already carry the "Could not configure
    /// … CWD tracking" context a pane's error field wants.
    ///
    /// The hasher is the caller's: the root package does not depend on Zed's
    /// `collections` crate, so the map's `FxBuildHasher` can only be inferred
    /// from the [`TerminalBuilder`] the environment is handed to.
    fn build<S>(self) -> Result<HashMap<String, String, S>>
    where
        S: std::hash::BuildHasher + Default,
    {
        let is_wsl = is_wsl_shell(self.profile);
        let mut environment = if is_wsl {
            HashMap::default()
        } else {
            let native_environment = native_terminal_environment();
            #[cfg(windows)]
            let inherited_path = native_environment
                .iter()
                .find(|(name, _)| name == "PATH")
                .map(|(_, value)| value.clone());
            let msys2_environment =
                msys2_cwd_tracking_environment(self.profile, self.tracking_id, &env::temp_dir())
                    .context("Could not configure MSYS2 CWD tracking")?;
            #[cfg(windows)]
            let cygwin_environment = cygwin_cwd_tracking_environment_with_path(
                self.profile,
                self.tracking_id,
                &env::temp_dir(),
                inherited_path.as_deref(),
            )
            .context("Could not configure Cygwin CWD tracking")?;
            #[cfg(not(windows))]
            let cygwin_environment = Vec::new();
            native_environment
                .into_iter()
                .chain(msys2_environment)
                .chain(cygwin_environment)
                .collect()
        };
        if is_wsl {
            wsl_terminal_environment(&mut environment, self.wsl_cwd_file);
        }
        apply_terminal_environment_overrides(
            &mut environment,
            self.overrides,
            std::process::id(),
            self.attention_id,
            self.tracking_id,
            self.routing_id,
            self.no_mux,
        );
        #[cfg(windows)]
        ensure_cygwin_environment(self.profile, &mut environment);
        environment.insert("ZETTA_THEME".to_owned(), self.theme_name.to_owned());
        if is_wsl {
            add_wsl_environment_variable_names(
                &mut environment,
                self.overrides.keys().map(String::as_str),
            );
            add_wsl_environment_variables(&mut environment);
        }
        Ok(environment)
    }
}

impl Zetta {
    /// Spawns the terminal for a pane, resolving the pane's theme and this
    /// process's current terminal settings first — so `terminal_theme` and
    /// `path_hyperlink_regexes` are filled in here and anything the caller put
    /// in them is replaced. A caller that spawns a batch of panes resolves both
    /// once for the batch and calls [`Zetta::spawn_terminal`] instead.
    pub(crate) fn spawn_terminal_for_pane(
        &mut self,
        mut request: TerminalSpawnRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal_theme) =
            self.resolve_pane_spawn_theme(request.tab_id, request.pane_id, &request.profile, cx)
        else {
            return;
        };
        let mut settings = TerminalSpawnSettings::current(cx);
        request.path_hyperlink_regexes = settings.path_hyperlink_regexes(true);
        request.terminal_theme = terminal_theme;
        self.spawn_terminal(request, &settings, window, cx);
    }

    /// Resolves the theme a pane's terminal starts with. `None` means the
    /// failure has already been reported on the pane and the spawn must stop.
    fn resolve_pane_spawn_theme(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: &Profile,
        cx: &mut Context<Self>,
    ) -> Option<Option<Arc<Theme>>> {
        let (pane_theme_override, tab_theme_override) =
            self.tabs.by_id(tab_id).map_or((None, None), |tab| {
                (
                    tab.pane(pane_id)
                        .and_then(|pane| pane.theme_override.as_deref()),
                    tab.theme_override.as_deref(),
                )
            });
        match self.pane_theme(
            tab_id,
            profile,
            self.project_config_for_tab(tab_id).map(Arc::as_ref),
            ThemeOverrides {
                pane: pane_theme_override,
                tab: tab_theme_override,
            },
            cx,
        ) {
            Ok(theme) => Some(theme),
            Err(error) => {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!("Could not apply profile theme: {error:#}"),
                    cx,
                );
                None
            }
        }
    }

    /// Reports a synchronous spawn failure on the pane the terminal was meant
    /// for. Failures raised once the spawn is asynchronous instead coalesce
    /// their redraw through [`Zetta::schedule_terminal_spawn_notify`].
    pub(crate) fn report_pane_spawn_error(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        error: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self.tabs.pane_in_mut(tab_id, pane_id) {
            pane.error = Some(error);
        }
        cx.notify();
    }

    /// Wires a freshly built terminal to the pane that owns it: the run
    /// registry's view of the pane, unexpected exits, resize requests, and the
    /// grid-size notify the cached title bar depends on.
    ///
    /// Shared with the Windows console handover, which builds its terminal from
    /// an inherited console rather than by spawning one but observes it
    /// identically. `run_identity` is absent only for a pane whose shell
    /// integration cannot report, which is what the handover starts as.
    fn subscribe_spawned_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        run_identity: Option<crate::run_command::RunPaneIdentity>,
        terminal: &Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remote_paste_allowed = self.remote_clipboard_paste_tabs.contains(&tab_id);
        let remote_copy_allowed = !self.remote_clipboard_copy_blocked_tabs.contains(&tab_id);
        terminal.update(cx, |terminal, _| {
            terminal.set_remote_clipboard_paste_allowed(remote_paste_allowed);
            terminal.set_remote_clipboard_copy_allowed(remote_copy_allowed);
        });
        let run_registry = crate::run_command::process_run_registry();
        if let Some(identity) = run_identity {
            run_registry.pane_reopened(identity);
        }
        cx.subscribe_in(
            terminal,
            window,
            move |this, _, event: &TerminalEvent, window, cx| {
                if let Some(identity) = run_identity {
                    run_registry.observe_terminal_event(identity, event);
                    if let Some(command) =
                        crate::background_session_ui::restorable_command_change(event)
                    {
                        this.update_active_command(tab_id, pane_id, command);
                    }
                }
                match event {
                    TerminalEvent::TerminalExited(exit)
                        if exit.is_unexpected()
                            && this.retain_unexpected_terminal_exit(tab_id, pane_id, exit, cx) =>
                    {
                        this.publish_background_session_catalog(cx);
                        this.sync_visible_terminals(cx);
                        this.focus_active(window, cx);
                    }
                    TerminalEvent::ResizeRequested { rows, columns } => {
                        this.request_pane_grid(tab_id, pane_id, *columns, *rows, window, cx);
                    }
                    #[cfg(feature = "notifications")]
                    TerminalEvent::DesktopNotification(notification) => {
                        this.show_desktop_notification(tab_id, pane_id, notification, window, cx);
                    }
                    // The title bar reports the active pane's grid size, and
                    // it renders inside a cached boundary that only a notify
                    // on `Zetta` busts. Terminal output must not reach here;
                    // only an actual change of the grid's dimensions does.
                    TerminalEvent::GridSizeChanged => {
                        this.continue_pane_grid_request(tab_id, pane_id, window, cx);
                        cx.notify();
                    }
                    event if terminal_event_requires_worktree_detection(event) => {
                        // A program can change the terminal's ordinary OSC
                        // title without changing its process metadata. Treat it
                        // as a worktree-detection trigger too, so that a title
                        // such as Codex's `switched-source` cannot become the
                        // tab title while the shell remains in a linked
                        // worktree.
                        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                        this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                        cx.notify();
                    }
                    _ => {}
                }
            },
        )
        .detach();
    }

    /// Wires the view built over that terminal: close, title changes,
    /// broadcast input, and the editor a pane opens.
    fn subscribe_spawned_terminal_view(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(
            view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => {
                    this.terminal_closed(tab_id, pane_id, window, cx);
                }
                TerminalViewEvent::TitleChanged => {
                    this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                    this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                    cx.notify();
                }
                TerminalViewEvent::PasteError(error) => this.show_error_notice(error.clone(), cx),
                TerminalViewEvent::Input(input) => {
                    this.broadcast_input(tab_id, pane_id, input, cx);
                }
                TerminalViewEvent::OpenEditor(request) => {
                    this.open_editor_in_new_pane(tab_id, pane_id, request.clone(), window, cx);
                }
            },
        )
        .detach();
    }

    /// Applies the tab's input state to a new view, routes its focus, and
    /// reports whether this pane is the one that should take focus now.
    ///
    /// Answered before the view is stored on the pane, because it asks what the
    /// tab looked like when the spawn was requested: a pane that is no longer
    /// active by the time its terminal arrives must not steal focus.
    fn configure_spawned_terminal_focus(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let this = self;
        let focus_handle = view.focus_handle(cx);
        let emit_input_events = this
            .tabs
            .by_id(tab_id)
            .is_some_and(|tab| tab.broadcast_input);
        let input_enabled = this.terminal_input_enabled();
        view.update(cx, |view, cx| {
            view.set_emit_input_events(emit_input_events);
            view.set_input_enabled(input_enabled, cx);
        });
        cx.on_focus_in(&focus_handle, window, move |this, window, cx| {
            if let Some(tab) = this
                .tabs
                .by_id_mut(tab_id)
                .filter(|tab| tab.pane(pane_id).is_some_and(|pane| !pane.base_exited))
            {
                tab.activate_stack_entry(pane_id, PaneStackSelection::Base);
                cx.notify();
            }
            this.activate_current_project(window, cx);
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
        this.tab_index(tab_id).is_some_and(|index| {
            index == this.active_tab
                && this.tabs[index].active_pane == pane_id
                && this.tabs[index]
                    .pane(pane_id)
                    .is_some_and(|pane| pane.stack.selected_is_base())
        })
    }

    /// Spawns the terminal a [`TerminalSpawnRequest`] describes. The single
    /// spawn path: every interactive pane, split, profile replacement, pane
    /// template and restored session reaches the PTY through here.
    pub(crate) fn spawn_terminal(
        &mut self,
        request: TerminalSpawnRequest,
        settings: &TerminalSpawnSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let TerminalSpawnRequest {
            tab_id,
            pane_id,
            profile,
            shell,
            working_directory,
            wsl_directory,
            wsl_cwd_file,
            terminal_theme,
            path_hyperlink_regexes,
            environment: environment_overrides,
            tracked_multi_command_launch,
            restore: restore_options,
        } = request;
        // The launch's `--geometry`: the window is sized for it before it is
        // shown (see `fit_window_to_pending_grid`), and the terminal starts at
        // it so that a program asking before the first layout already sees it.
        let launch_grid = self.launch_grid.take();
        if let Some(grid) = launch_grid {
            self.request_pane_grid(tab_id, pane_id, grid.columns, grid.rows, window, cx);
        }
        let Some(shell) = self.resolve_spawn_shell(
            &profile,
            shell,
            tab_id,
            pane_id,
            wsl_directory.as_deref(),
            wsl_cwd_file.as_deref(),
            cx,
        ) else {
            return;
        };
        let mut combined_environment = self.project_environment_for_tab(tab_id);
        combined_environment.extend(environment_overrides);
        let is_wsl = is_wsl_shell(&profile.command);
        let Some((attention_id, pane_routing_id)) = self.tabs.by_id(tab_id).and_then(|tab| {
            tab.pane(pane_id)
                .map(|pane| (tab.attention_id, pane.routing_id))
        }) else {
            self.report_pane_spawn_error(
                tab_id,
                pane_id,
                "Could not identify the terminal's Zetta tab".to_owned(),
                cx,
            );
            return;
        };
        let effective_theme = terminal_theme.clone().unwrap_or_else(|| cx.theme().clone());
        let Some(environment) = self.build_spawn_environment(
            TerminalEnvironment {
                profile: &profile.command,
                overrides: &combined_environment,
                attention_id,
                tracking_id: pane_id,
                routing_id: pane_routing_id,
                wsl_cwd_file: wsl_cwd_file.as_deref(),
                theme_name: &effective_theme.name,
                no_mux: self.no_mux,
            },
            &shell,
            tab_id,
            pane_id,
            cx,
        ) else {
            return;
        };
        let shell_integration_startup_command = (!is_wsl)
            .then(|| shell_integration_startup_command(&shell))
            .flatten();
        let restore_replay = restore_options
            .as_ref()
            .and_then(|options| options.replay.clone());
        // Console backend selection must preserve daemon session ownership.
        let mux_provider =
            match self.mux_provider_for_tab_with_restore_replay(tab_id, restore_replay, cx) {
                Ok(provider) => provider,
                Err(error) => {
                    let message = format!(
                        "Could not start the terminal through the session multiplexer: {error:#}"
                    );
                    self.report_pane_spawn_error(tab_id, pane_id, message, cx);
                    return;
                }
            };
        // WSL skips the palette helper, but still names its console backend.
        let initial_console_palette = (!is_wsl
            || profile.console_backend == crate::config::ConsoleBackend::ConsoleServer)
            .then(|| terminal::ConsoleLaunchOptions {
                palette: terminal::console_palette_for_theme(effective_theme.as_ref()),
                backend: profile.console_backend,
            });
        #[cfg(feature = "zmux")]
        if let Some(provider) = mux_provider.as_ref().filter(|provider| {
            provider
                .shared_key()
                .is_some_and(|key| self.shared_collaboration.is_bound(&key))
        }) {
            let Some(key) = provider.shared_key() else {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    "Could not identify the shared session".to_owned(),
                    cx,
                );
                return;
            };
            let base_revision = self
                .shared_collaboration
                .state(&key)
                .map_or(zmux::messages::SessionRevision::INITIAL, |state| {
                    state.revision
                });
            let title = self
                .tabs
                .by_id(tab_id)
                .and_then(|tab| tab.process_title.clone())
                .unwrap_or_else(|| profile.name.clone());
            self.spawn_shared_terminal(
                SharedTerminalLaunch {
                    tab_id,
                    pane_id,
                    attention_id,
                    pane_routing_id,
                    terminal_theme,
                    provider: provider.clone(),
                    shell,
                    environment,
                    marker_nonce: terminal::draw_command_marker_nonce(),
                    working_directory,
                    title,
                    profile_name: profile.name.clone(),
                    cursor_shape: settings.cursor_shape,
                    alternate_scroll: settings.alternate_scroll,
                    max_scroll_history_lines: settings.max_scroll_history_lines,
                    // The shell integration loads through the `zetta` on the
                    // pane's own `PATH` and reports to the routing ids in its
                    // environment. For a pane running on another machine both
                    // name the wrong side, so it runs without the integration
                    // rather than with one that answers nobody.
                    shell_integration_startup_command: shell_integration_startup_command
                        .filter(|_| !provider.runtime().is_remote()),
                    tracked_multi_command_launch,
                    base_revision,
                    console_palette: initial_console_palette
                        .map(|options| options.palette)
                        .unwrap_or_default(),
                    resynchronized: false,
                },
                window,
                cx,
            );
            return;
        }
        self.spawn_prepared_local_terminal(
            LocalTerminalLaunch {
                initial_bounds: launch_grid.map(|grid| terminal::InitialTerminalSize {
                    bounds: terminal_view::TerminalCellMetrics::standalone(
                        cx,
                        window.scale_factor(),
                    )
                    .bounds_for_grid(grid.columns, grid.rows),
                    scale_factor: window.scale_factor(),
                }),
                tab_id,
                pane_id,
                attention_id,
                pane_routing_id,
                terminal_theme,
                shell,
                environment,
                working_directory,
                path_hyperlink_regexes,
                restore_options,
                mux_provider,
                initial_console_palette,
                shell_integration_startup_command,
                tracked_multi_command_launch,
            },
            settings,
            window,
            cx,
        );
    }

    /// The environment a pane's shell starts with, or `None` once a failure to
    /// build it has been reported on the pane.
    #[cfg_attr(
        windows,
        expect(
            unused_variables,
            reason = "only Unix shells get the zsh history environment"
        )
    )]
    fn build_spawn_environment(
        &mut self,
        environment: TerminalEnvironment<'_>,
        shell: &Shell,
        tab_id: u64,
        pane_id: u64,
        cx: &mut Context<Self>,
    ) -> Option<HashMap<String, String>> {
        // The `mut` is for the zsh history step below, which is Unix-only.
        #[cfg_attr(
            windows,
            expect(
                unused_mut,
                reason = "only the Unix shell-history setup mutates this map"
            )
        )]
        let mut environment = match environment.build() {
            Ok(environment) => environment,
            Err(error) => {
                self.report_pane_spawn_error(tab_id, pane_id, format!("{error:#}"), cx);
                return None;
            }
        };
        #[cfg(not(windows))]
        if let Err(error) = configure_zsh_history_environment(shell, &mut environment, pane_id) {
            log::warn!("could not configure early zsh history filtering: {error:#}");
        }
        Some(environment)
    }

    fn spawn_prepared_local_terminal(
        &mut self,
        launch: LocalTerminalLaunch,
        settings: &TerminalSpawnSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let LocalTerminalLaunch {
            initial_bounds,
            tab_id,
            pane_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            shell,
            environment,
            working_directory,
            path_hyperlink_regexes,
            restore_options,
            mux_provider,
            initial_console_palette,
            shell_integration_startup_command,
            tracked_multi_command_launch,
        } = launch;
        let image_paste_handler = Arc::new(crate::ssh_image_paste::SshImagePasteHandler::new(
            shell.clone(),
            environment.clone(),
            working_directory.clone(),
        ));
        let restored_working_directory = restore_options
            .is_some()
            .then(|| working_directory.clone())
            .flatten();
        // A restored pane must reattach to its own multiplexer session; a local
        // fallback would start a different one.
        let pty_requirement = if restore_options.is_some() {
            terminal::PtyRequirement::ProviderRequired
        } else {
            terminal::PtyRequirement::LocalFallback
        };
        let builder = TerminalBuilder::spawn(
            terminal::TerminalSpawnOptions {
                working_directory,
                env: environment.into_iter().collect(),
                cursor_shape: settings.cursor_shape,
                alternate_scroll: settings.alternate_scroll,
                max_scroll_history_lines: settings.max_scroll_history_lines,
                path_hyperlink_regexes,
                path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
                pty_provider: mux_provider
                    .clone()
                    .map(|provider| provider as Arc<dyn terminal::PtyProvider>),
                initial_console_palette,
                initial_bounds,
                pty_requirement,
                ..terminal::TerminalSpawnOptions::new(
                    shell,
                    cx.entity_id().as_u64(),
                    PathStyle::local(),
                )
            },
            cx,
        );
        let this = cx.entity().downgrade();
        let spawned = SpawnedTerminal {
            tab_id,
            pane_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            restore_options,
            restored_working_directory,
            mux_provider,
            #[cfg(feature = "zmux")]
            shared: None,
            shell_integration_startup_command,
            tracked_multi_command_launch,
            image_paste_handler: Some(image_paste_handler),
        };
        window
            .spawn(cx, async move |cx| match builder.await {
                Ok(mut builder) => {
                    if let Some(options) = spawned.restore_options.as_ref() {
                        builder = builder
                            .with_fresh_shell_restore()
                            .with_restore_prefill(options.prefill.clone())
                            .with_working_directory(spawned.restored_working_directory.clone());
                    }
                    this.update_in(cx, |this, window, cx| {
                        this.finish_terminal_spawn(builder, spawned, window, cx);
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, window, cx| {
                        this.report_terminal_spawn_failure(&spawned, &error, window, cx);
                    })
                    .ok();
                }
            })
            .detach();
    }

    /// The shell a spawn runs, deriving it from the profile when the request did
    /// not name one.
    ///
    /// Returns `None` once it has already reported the failure on the pane —
    /// only the Cygwin CWD-tracking path can fail, and it does so with a message
    /// naming what could not be configured.
    #[expect(
        clippy::too_many_arguments,
        reason = "six values the spawn is resolved from, plus the GPUI context"
    )]
    #[cfg_attr(
        not(windows),
        expect(
            unused_variables,
            reason = "only the Windows-only Cygwin arm reads the tab, the pane and the context"
        )
    )]
    fn resolve_spawn_shell(
        &mut self,
        profile: &Profile,
        shell: Option<Shell>,
        tab_id: u64,
        pane_id: u64,
        wsl_directory: Option<&str>,
        wsl_cwd_file: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> Option<Shell> {
        Some(match shell {
            Some(shell) => shell,
            None if is_wsl_shell(&profile.command) => {
                wsl_shell_with_tracking(profile.command.clone(), wsl_directory, wsl_cwd_file)
            }
            None if cfg!(windows) && cygwin_profile(&profile.command).is_some() => {
                #[cfg(windows)]
                {
                    match cygwin_shell_with_tracking(
                        profile.command.clone(),
                        pane_id,
                        &env::temp_dir(),
                    ) {
                        Ok(shell) => shell,
                        Err(error) => {
                            self.report_pane_spawn_error(
                                tab_id,
                                pane_id,
                                format!("Could not configure Cygwin CWD tracking: {error:#}"),
                                cx,
                            );
                            return None;
                        }
                    }
                }
                #[cfg(not(windows))]
                unreachable!()
            }
            None => profile.command.clone(),
        })
    }
}

impl Zetta {
    /// Wires a terminal whose builder has resolved into the pane that asked for
    /// it, and runs whatever has to happen once it exists.
    ///
    /// The pane may have been closed or detached while the builder was in
    /// flight, which is why the install below falls back to the background
    /// session that now owns it.
    fn finish_terminal_spawn(
        &mut self,
        mut builder: TerminalBuilder,
        spawned: SpawnedTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SpawnedTerminal {
            tab_id,
            pane_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            restore_options,
            shell_integration_startup_command,
            tracked_multi_command_launch,
            mux_provider,
            #[cfg(feature = "zmux")]
            mut shared,
            image_paste_handler,
            ..
        } = spawned;
        if let Some(image_paste_handler) = image_paste_handler {
            builder = builder.with_image_paste_handler(image_paste_handler);
        }
        #[cfg(feature = "zmux")]
        if let Some(shared) = &mut shared {
            let mux_pane_id = shared.pane.pane_id();
            self.mux_panes.adopt_session_with_runtime(
                tab_id,
                shared.pane.session_id(),
                shared.runtime.clone(),
            );
            self.mux_panes.record(pane_id, mux_pane_id);
            let key = SharedSessionKey::new(&shared.runtime, shared.pane.session_id());
            self.shared_collaboration
                .record_pane(&key, mux_pane_id, pane_id);
            if let Some(state) = shared.state.take() {
                let _ = self.shared_collaboration.accept_pane_added(
                    &key,
                    state.clone(),
                    mux_pane_id,
                    pane_id,
                );
                self.apply_shared_snapshot(&key, state, window, cx);
            }
        }
        let this = self;
        this.adopt_mux_pane(
            tab_id,
            pane_id,
            mux_provider.as_deref(),
            &mut builder,
            window,
            cx,
        );
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view =
            cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), terminal_theme, window, cx));
        this.configure_terminal_view_silent_mode(tab_id, &view, cx);
        this.subscribe_spawned_terminal(
            tab_id,
            pane_id,
            Some(crate::run_command::RunPaneIdentity::new(
                attention_id,
                pane_routing_id,
            )),
            &terminal,
            window,
            cx,
        );
        this.subscribe_spawned_terminal_view(tab_id, pane_id, &view, window, cx);
        let should_focus =
            this.configure_spawned_terminal_focus(tab_id, pane_id, &view, window, cx);
        let tab_index = this.tab_index(tab_id);
        let mut deferred_pending_command = None;
        if let Some(pane) = tab_index
            .and_then(|index| this.tabs.get_mut(index))
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.terminal = Some(terminal.clone());
            pane.view = Some(view.clone());
            pane.base_exited = false;
            pane.error = None;
            pane.exit = None;
            if restore_options.is_none()
                && let Some(command) = pane.pending_command.take()
            {
                match pending_command_delivery(command, shell_integration_startup_command.is_some())
                {
                    PendingCommandDelivery::Now(command) => {
                        view.update(cx, |view, cx| {
                            view.apply_input(&TerminalInput::Text(format!("{command}\r")), cx);
                        });
                    }
                    PendingCommandDelivery::AfterIntegration(command) => {
                        deferred_pending_command = Some(command);
                    }
                }
            }
        } else {
            let stored_in_background = {
                let pane = this.background_sessions.pane_in_mut(tab_id, pane_id);
                if let Some(pane) = pane {
                    pane.terminal = Some(terminal.clone());
                    true
                } else {
                    false
                }
            };
            if stored_in_background {
                this.observe_background_terminal(tab_id, pane_id, terminal.clone(), cx);
                this.publish_background_session_catalog(cx);
            } else {
                // Either this pane, or its whole tab, closed while the spawn
                // above was still in flight. `adopt_mux_pane` just registered
                // it as held by this process, and nothing else will ever ask
                // the multiplexer to let it go — dropping `terminal` below
                // only closes this process's copy of the descriptor, not the
                // daemon's, so without this the pane stays wedged as ours
                // until the window exits. See `Zetta::release_mux_pane`.
                this.release_orphaned_terminal_spawn(tab_id, pane_id, tab_index, cx);
            }
        }
        #[cfg(feature = "zmux")]
        if let Some(shared) = shared {
            this.register_spawned_shared_pane(
                MuxPaneIds {
                    tab_id,
                    pane_id,
                    session_id: shared.pane.session_id(),
                    mux_pane_id: shared.pane.pane_id(),
                },
                SpawnedSharedPane {
                    shared_pane: &shared.pane,
                    zosh_session: shared.zosh_session,
                    transport_fallback: shared.transport_fallback,
                    runtime: &shared.runtime,
                    terminal: &terminal,
                },
                window,
                cx,
            );
        }
        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
        this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
        if should_focus {
            let focus_handle = view.focus_handle(cx);
            this.focus_terminal_if_allowed(&focus_handle, window, cx);
        }
        this.sync_visible_terminals(cx);
        this.schedule_terminal_spawn_notify(cx);
        if tracked_multi_command_launch {
            this.finish_multi_command_launch(window, cx);
        }
        this.deliver_spawn_startup_commands(
            &terminal,
            &view,
            restore_options.is_some(),
            shell_integration_startup_command,
            deferred_pending_command,
            cx,
        );
    }

    /// Writes what a new shell is started with once it is ready for it: the
    /// shell-integration line after the startup handshake, then a pending
    /// command that had to wait for that line. A restored pane also finishes
    /// its restore here.
    fn deliver_spawn_startup_commands(
        &mut self,
        terminal: &Entity<Terminal>,
        view: &Entity<TerminalView>,
        restoring: bool,
        shell_integration_startup_command: Option<Vec<u8>>,
        deferred_pending_command: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if restoring {
            let terminal_for_restore = terminal.clone();
            if let Some(command) = shell_integration_startup_command.as_ref() {
                let startup_handshake = terminal.update(cx, |terminal, _| {
                    terminal.start_init_command_startup_handshake()
                });
                let command = command.clone();
                cx.spawn(async move |_this, cx| {
                    startup_handshake.await;
                    terminal_for_restore.update(cx, |terminal, cx| {
                        terminal.write_init_command_after_startup(command, cx);
                        terminal.finish_fresh_shell_restore(cx);
                    });
                })
                .detach();
            } else {
                terminal_for_restore.update(cx, |terminal, cx| {
                    terminal.finish_fresh_shell_restore(cx);
                });
            }
        } else if let Some(command) = shell_integration_startup_command.as_ref() {
            let startup_handshake = terminal.update(cx, |terminal, _| {
                terminal.start_init_command_startup_handshake()
            });
            let command = command.clone();
            let terminal_for_startup = terminal.clone();
            let view_for_startup = view.clone();
            cx.spawn(async move |_this, cx| {
                startup_handshake.await;
                terminal_for_startup.update(cx, |terminal, cx| {
                    terminal.write_init_command_after_startup(command, cx);
                });
                if let Some(command) = deferred_pending_command {
                    view_for_startup.update(cx, |view, cx| {
                        view.apply_input(&TerminalInput::Text(format!("{command}\r")), cx);
                    });
                }
            })
            .detach();
        }
    }

    /// Releases the multiplexer pane after its terminal finishes spawning
    /// without a live pane to own it. A pane closed inside a still-live tab
    /// keeps that tab's session mapping; a closed tab does not.
    fn release_orphaned_terminal_spawn(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        tab_index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.release_mux_pane(tab_id, pane_id, cx);
        if tab_index.is_none() {
            self.mux_panes.forget_tab(tab_id);
        }
    }

    /// A spawn that never produced a terminal: the pane shows the error, and a
    /// multi-command batch waiting on this launch is released so the rest of it
    /// is not left pending.
    fn report_terminal_spawn_failure(
        &mut self,
        spawned: &SpawnedTerminal,
        error: &anyhow::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SpawnedTerminal {
            tab_id,
            pane_id,
            tracked_multi_command_launch,
            ..
        } = *spawned;
        let this = self;
        if let Some(pane) = this.tabs.pane_in_mut(tab_id, pane_id) {
            pane.error = Some(format!("{error:#}"));
        }
        this.schedule_terminal_spawn_notify(cx);
        if tracked_multi_command_launch {
            this.finish_multi_command_launch(window, cx);
        }
    }

    pub(crate) fn schedule_terminal_spawn_notify(&mut self, cx: &mut Context<Self>) {
        if !begin_coalesced_notification(&mut self.terminal_spawn_notify_pending) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_SPAWN_NOTIFY_INTERVAL)
                .await;
            this.update(cx, |this, cx| {
                this.terminal_spawn_notify_pending = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

pub(crate) fn apply_terminal_environment_overrides<S>(
    environment: &mut HashMap<String, String, S>,
    overrides: &HashMap<String, String>,
    process_id: u32,
    attention_id: u64,
    pane_id: u64,
    pane_routing_id: u64,
    no_mux: bool,
) where
    S: std::hash::BuildHasher,
{
    for (name, value) in overrides {
        if !name
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ZETTA_"))
        {
            environment.insert(name.clone(), value.clone());
        }
    }
    environment.insert("ZETTA_PROCESS_ID".to_owned(), process_id.to_string());
    environment.insert("ZETTA_ATTENTION_ID".to_owned(), attention_id.to_string());
    environment.insert("ZETTA_PANE_ID".to_owned(), pane_id.to_string());
    environment.insert(
        "ZETTA_PANE_ROUTING_ID".to_owned(),
        pane_routing_id.to_string(),
    );
    environment.insert(
        "ZETTA_NO_MUX".to_owned(),
        if no_mux { "1" } else { "0" }.to_owned(),
    );
}

/// Builds the shell invocation used by a stacked command. Native profiles go
/// through the same shell-aware builder as Zed tasks. WSL, MSYS2, and Cygwin
/// profiles preserve their launcher or executable so the command runs inside
/// the configured POSIX environment rather than in the Windows command shell.
pub(crate) fn stacked_task_shell(
    profile: &Shell,
    command: &str,
    wsl_directory: Option<&str>,
) -> Shell {
    if is_wsl_shell(profile) {
        return match wsl_shell_with_tracking(profile.clone(), wsl_directory, None) {
            Shell::WithArguments {
                program,
                mut args,
                title_override,
            } => {
                args.extend([
                    "--exec".to_owned(),
                    "/bin/sh".to_owned(),
                    "-i".to_owned(),
                    "-c".to_owned(),
                    command.to_owned(),
                ]);
                Shell::WithArguments {
                    program,
                    args,
                    title_override,
                }
            }
            shell => shell,
        };
    }

    #[cfg(windows)]
    if let Some((root, shell)) = msys2_profile(profile) {
        let shell_name = match shell {
            Msys2Shell::Bash => "bash.exe",
            Msys2Shell::Zsh => "zsh.exe",
        };
        let shell = Shell::Program(
            root.join("usr")
                .join("bin")
                .join(shell_name)
                .display()
                .to_string(),
        );
        let (program, args) =
            ShellBuilder::new(&shell, true).build_no_quote(Some(command.to_owned()), &[]);
        return Shell::WithArguments {
            program,
            args,
            title_override: None,
        };
    }

    #[cfg(windows)]
    if cygwin_profile(profile).is_some() {
        let (program, args) =
            ShellBuilder::new(profile, true).build_no_quote(Some(command.to_owned()), &[]);
        return Shell::WithArguments {
            program,
            args,
            title_override: None,
        };
    }

    let (program, args) =
        ShellBuilder::new(profile, cfg!(windows)).build_no_quote(Some(command.to_owned()), &[]);
    Shell::WithArguments {
        program,
        args,
        title_override: None,
    }
}

/// When a pane's pending command may be typed into its shell.
#[derive(Debug, PartialEq, Eq)]
enum PendingCommandDelivery {
    Now(String),
    AfterIntegration(String),
}

/// The startup handshake leaves a marker command reading its payload with echo
/// and rendering suppressed. A command typed before the integration line is
/// written would be consumed as that payload and run invisibly (an empty pane,
/// and a process the pane cannot kill), so it has to wait for the line.
fn pending_command_delivery(
    command: String,
    integration_line_pending: bool,
) -> PendingCommandDelivery {
    if integration_line_pending {
        PendingCommandDelivery::AfterIntegration(command)
    } else {
        PendingCommandDelivery::Now(command)
    }
}

#[cfg(test)]
#[path = "tests/terminal_spawn.rs"]
mod tests;

#[cfg(not(windows))]
fn configure_zsh_history_environment<S>(
    shell: &Shell,
    environment: &mut HashMap<String, String, S>,
    pane_id: u64,
) -> Result<()>
where
    S: std::hash::BuildHasher,
{
    let program = shell.program();
    let is_zsh = Path::new(&program)
        .file_stem()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("zsh"));
    if !is_zsh || shell_integration_startup_command(shell).is_none() {
        return Ok(());
    }

    let directory = tempfile::Builder::new()
        .prefix(&format!("zetta-zsh-history-{pane_id}-"))
        .tempdir()
        .context("creating temporary zsh history directory")?;
    let zshenv = directory.path().join(".zshenv");
    fs::write(&zshenv, ZSH_EARLY_HISTORY_INTEGRATION.as_bytes())
        .with_context(|| format!("writing {}", zshenv.display()))?;
    let directory = directory.keep();
    let directory = directory
        .to_str()
        .context("temporary zsh history directory is not valid UTF-8")?
        .to_owned();

    let original_zdotdir = environment
        .get("ZDOTDIR")
        .cloned()
        .filter(|value| !value.is_empty());
    environment.insert("ZETTA_ZSH_HISTORY_ZDOTDIR".to_owned(), directory.clone());
    environment.insert(
        "ZETTA_ZSH_ORIGINAL_ZDOTDIR_SET".to_owned(),
        u8::from(original_zdotdir.is_some()).to_string(),
    );
    if let Some(original_zdotdir) = original_zdotdir {
        environment.insert("ZETTA_ZSH_ORIGINAL_ZDOTDIR".to_owned(), original_zdotdir);
    } else {
        environment.remove("ZETTA_ZSH_ORIGINAL_ZDOTDIR");
    }
    environment.insert("ZDOTDIR".to_owned(), directory);
    Ok(())
}
