use super::*;
use crate::worktree_detection::terminal_event_requires_worktree_detection;

/// Returns the shell command used to load this process's shell integration
/// into an interactive native shell.  The command is sent after the shell's
/// startup files have completed so a stale `zetta` found earlier on PATH
/// cannot leave the pane with CWD-only tracking.
fn shell_integration_startup_command(shell: &Shell) -> Option<Vec<u8>> {
    let (program, arguments) = shell.program_and_args();
    let has_command = arguments.iter().any(|argument| {
        let argument = argument.to_ascii_lowercase();
        matches!(
            argument.as_str(),
            "-c" | "--command"
                | "/c"
                | "/k"
                | "-command"
                | "-commandwithargs"
                | "-encodedcommand"
                | "-encodedarguments"
                | "-file"
        ) || (argument.starts_with('-') && argument[1..].contains('c'))
    });
    if has_command {
        return None;
    }

    let shell_name = Path::new(&program)
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    #[cfg(not(windows))]
    let command = match shell_name.as_str() {
        "bash" | "bash.exe" => {
            r#"if [[ ${__ZETTA_LIFECYCLE_TRACKING_INSTALLED:-0} != 1 || ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ]]; then eval "$("$ZETTA_HOST_EXECUTABLE" init bash)"; fi"#
        }
        "zsh" | "zsh.exe" => {
            r#"if [[ ${__ZETTA_LIFECYCLE_TRACKING_VERSION:-0} != 3 || ( -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} && ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ) ]]; then eval "$("$ZETTA_HOST_EXECUTABLE" init zsh)"; fi"#
        }
        "fish" | "fish.exe" => {
            r#"if not set -q __ZETTA_LIFECYCLE_TRACKING_INSTALLED; or test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" != 1; "$ZETTA_HOST_EXECUTABLE" init fish | source; end"#
        }
        _ => return None,
    };
    #[cfg(windows)]
    let command = match shell_name.as_str() {
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => {
            r#"if (-not $global:__ZettaLifecycleTrackerInstalled -or -not $global:__ZettaLifecycleTrackingEnabled) { & $env:ZETTA_HOST_EXECUTABLE init powershell | Out-String | Invoke-Expression }"#
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
        let (pane_theme_override, tab_theme_override) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| {
                (
                    tab.pane(pane_id)
                        .and_then(|pane| pane.theme_override.as_deref()),
                    tab.theme_override.as_deref(),
                )
            })
            .unwrap_or((None, None));
        match resolve_terminal_theme(
            pane_theme_override,
            tab_theme_override,
            profile,
            self.project_config_for_tab(tab_id).map(Arc::as_ref),
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
    fn report_pane_spawn_error(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        error: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.error = Some(error);
        }
        cx.notify();
    }

    #[cfg(windows)]
    pub(crate) fn spawn_windows_handoff_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: Profile,
        request: crate::windows_integration::WindowsHandoffRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal_theme) = self.resolve_pane_spawn_theme(tab_id, pane_id, &profile, cx)
        else {
            return;
        };
        let mut settings = TerminalSpawnSettings::current(cx);
        let path_hyperlink_regexes = settings.path_hyperlink_regexes(true);
        let child_handle = match request.duplicate_child_handle() {
            Ok(handle) => handle,
            Err(error) => {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!("Could not monitor the handed-over process: {error}"),
                    cx,
                );
                return;
            }
        };
        let options = terminal::AttachedOptions {
            shell: profile.command.clone(),
            env: native_terminal_environment().into_iter().collect(),
            cursor_shape: settings.cursor_shape,
            alternate_scroll: settings.alternate_scroll,
            max_scroll_history_lines: settings.max_scroll_history_lines,
            path_hyperlink_regexes,
            path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
            window_id: cx.entity_id().as_u64(),
        };
        let handover = request.into_handover();
        let run_identity = self.run_pane_identity(tab_id, pane_id);
        let build_executor = cx.background_executor().clone();
        let terminal_executor = build_executor.clone();
        let build = build_executor.spawn(async move {
            TerminalBuilder::new_attached(handover, options, &terminal_executor, PathStyle::local())
        });
        let this = cx.entity().downgrade();
        let terminal_theme_for_task = terminal_theme.clone();
        window
            .spawn(cx, async move |cx| match build.await {
                Ok(attached) => {
                    let terminal::AttachedTerminal {
                        builder,
                        child_events,
                    } = attached;
                    crate::windows_integration::monitor_handoff_child(child_handle, child_events);
                    this.update_in(cx, |this, window, cx| {
                        let terminal = cx.new(|cx| builder.subscribe(cx));
                        let view = cx.new(|cx| {
                            TerminalView::new_with_theme(
                                terminal.clone(),
                                terminal_theme_for_task,
                                window,
                                cx,
                            )
                        });
                        this.configure_terminal_view_silent_mode(tab_id, &view, cx);
                        this.subscribe_spawned_terminal(
                            tab_id,
                            pane_id,
                            run_identity,
                            &terminal,
                            window,
                            cx,
                        );
                        this.subscribe_spawned_terminal_view(tab_id, pane_id, &view, window, cx);
                        let should_focus = this
                            .configure_spawned_terminal_focus(tab_id, pane_id, &view, window, cx);
                        let tab_index = this.tabs.iter().position(|tab| tab.id == tab_id);
                        if let Some(pane) = tab_index
                            .and_then(|index| this.tabs.get_mut(index))
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.terminal = Some(terminal.clone());
                            pane.view = Some(view.clone());
                            pane.base_exited = false;
                            pane.error = None;
                            pane.exit = None;
                        }
                        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                        this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                        if should_focus {
                            view.focus_handle(cx).focus(window, cx);
                        }
                        this.sync_visible_terminals(cx);
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, _, cx| {
                        if let Some(pane) = this
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.error = Some(format!("{error:#}"));
                        }
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
            })
            .detach();
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
        let run_registry = crate::run_command::process_run_registry();
        if let Some(identity) = run_identity {
            run_registry.pane_reopened(identity);
        }
        cx.subscribe_in(
            terminal,
            window,
            move |this, _, event: &TerminalEvent, window, cx| {
                if let Some(identity) = run_identity {
                    match event {
                        TerminalEvent::TrackingReady => run_registry.tracking_ready(identity),
                        TerminalEvent::CommandStarted { command } => {
                            run_registry.command_started(identity, command.clone());
                            this.update_active_command(
                                tab_id,
                                pane_id,
                                crate::session_state::valid_restore_command(command),
                            );
                        }
                        TerminalEvent::CommandFinished { exit_code } => {
                            run_registry.command_finished(identity, *exit_code);
                            this.update_active_command(tab_id, pane_id, None);
                        }
                        TerminalEvent::TerminalExited(_) => {
                            run_registry.terminal_lost(identity);
                            this.update_active_command(tab_id, pane_id, None);
                        }
                        _ => {}
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
                        this.resize_pane_to(
                            tab_id,
                            pane_id,
                            Some(*columns),
                            Some(*rows),
                            window,
                            cx,
                        );
                    }
                    // The title bar reports the active pane's grid size, and
                    // it renders inside a cached boundary that only a notify
                    // on `Zetta` busts. Terminal output must not reach here;
                    // only an actual change of the grid's dimensions does.
                    TerminalEvent::GridSizeChanged => cx.notify(),
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
            .iter()
            .find(|tab| tab.id == tab_id)
            .is_some_and(|tab| tab.broadcast_input);
        let input_enabled = this.terminal_input_enabled();
        view.update(cx, |view, cx| {
            view.set_emit_input_events(emit_input_events);
            view.set_input_enabled(input_enabled, cx);
        });
        cx.on_focus_in(&focus_handle, window, move |this, window, cx| {
            if let Some(tab) = this
                .tabs
                .iter_mut()
                .find(|tab| tab.id == tab_id)
                .filter(|tab| tab.pane(pane_id).is_some_and(|pane| !pane.base_exited))
            {
                tab.activate_stack_entry(pane_id, PaneStackSelection::Base);
                cx.notify();
            }
            this.activate_current_project(window, cx);
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
        this.tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .is_some_and(|index| {
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
        let shell = match shell {
            Some(shell) => shell,
            None if is_wsl_shell(&profile.command) => wsl_shell_with_tracking(
                profile.command.clone(),
                wsl_directory.as_deref(),
                wsl_cwd_file.as_deref(),
            ),
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
                            return;
                        }
                    }
                }
                #[cfg(not(windows))]
                unreachable!()
            }
            None => profile.command.clone(),
        };
        let mut combined_environment = self.project_environment_for_tab(tab_id);
        combined_environment.extend(environment_overrides);
        let is_wsl = is_wsl_shell(&profile.command);
        let Some((attention_id, pane_routing_id)) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| {
                tab.pane(pane_id)
                    .map(|pane| (tab.attention_id, pane.routing_id))
            })
        else {
            self.report_pane_spawn_error(
                tab_id,
                pane_id,
                "Could not identify the terminal's Zetta tab".to_owned(),
                cx,
            );
            return;
        };
        let effective_theme = terminal_theme.clone().unwrap_or_else(|| cx.theme().clone());
        // The `mut` is for the zsh history step below, which is Unix-only.
        #[cfg_attr(windows, allow(unused_mut))]
        let mut environment = match (TerminalEnvironment {
            profile: &profile.command,
            overrides: &combined_environment,
            attention_id,
            tracking_id: pane_id,
            routing_id: pane_routing_id,
            wsl_cwd_file: wsl_cwd_file.as_deref(),
            theme_name: &effective_theme.name,
            no_mux: self.no_mux,
        })
        .build()
        {
            Ok(environment) => environment,
            Err(error) => {
                self.report_pane_spawn_error(tab_id, pane_id, format!("{error:#}"), cx);
                return;
            }
        };
        #[cfg(not(windows))]
        if let Err(error) = configure_zsh_history_environment(&shell, &mut environment, pane_id) {
            log::warn!("could not configure early zsh history filtering: {error:#}");
        }
        let shell_integration_startup_command = (!is_wsl)
            .then(|| shell_integration_startup_command(&shell))
            .flatten();
        let restore_replay = restore_options
            .as_ref()
            .and_then(|options| options.replay.clone());
        let mux_provider =
            match self.mux_provider_for_tab_with_restore_replay(tab_id, restore_replay, cx) {
                Ok(provider) => provider,
                Err(error) => {
                    self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!(
                        "Could not start the terminal through the session multiplexer: {error:#}"
                    ),
                    cx,
                );
                    return;
                }
            };
        let initial_console_palette =
            (!is_wsl).then(|| terminal::console_palette_for_theme(effective_theme.as_ref()));
        let restored_working_directory = restore_options
            .is_some()
            .then(|| working_directory.clone())
            .flatten();
        let builder = if restore_options.is_some() {
            TerminalBuilder::new_with_console_palette_for_restore(
                working_directory,
                None,
                shell,
                environment,
                settings.cursor_shape,
                settings.alternate_scroll,
                settings.max_scroll_history_lines,
                path_hyperlink_regexes,
                settings.path_hyperlink_timeout_ms,
                false,
                cx.entity_id().as_u64(),
                None,
                cx,
                Vec::new(),
                PathStyle::local(),
                mux_provider
                    .clone()
                    .map(|provider| provider as Arc<dyn terminal::PtyProvider>),
                initial_console_palette,
            )
        } else {
            TerminalBuilder::new_with_console_palette(
                working_directory,
                None,
                shell,
                environment,
                settings.cursor_shape,
                settings.alternate_scroll,
                settings.max_scroll_history_lines,
                path_hyperlink_regexes,
                settings.path_hyperlink_timeout_ms,
                false,
                cx.entity_id().as_u64(),
                None,
                cx,
                Vec::new(),
                PathStyle::local(),
                mux_provider
                    .clone()
                    .map(|provider| provider as Arc<dyn terminal::PtyProvider>),
                initial_console_palette,
            )
        };

        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| match builder.await {
                Ok(mut builder) => {
                    if let Some(options) = restore_options.as_ref() {
                        builder = builder
                            .with_fresh_shell_restore()
                            .with_restore_prefill(options.prefill.clone())
                            .with_working_directory(restored_working_directory);
                    }
                    this.update_in(cx, |this, window, cx| {
                        this.adopt_mux_pane(
                            tab_id,
                            pane_id,
                            mux_provider.as_deref(),
                            &mut builder,
                            window,
                            cx,
                        );
                        let terminal = cx.new(|cx| builder.subscribe(cx));
                        let view = cx.new(|cx| {
                            TerminalView::new_with_theme(
                                terminal.clone(),
                                terminal_theme,
                                window,
                                cx,
                            )
                        });
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
                        let should_focus = this
                            .configure_spawned_terminal_focus(tab_id, pane_id, &view, window, cx);
                        let tab_index = this.tabs.iter().position(|tab| tab.id == tab_id);
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
                                view.update(cx, |view, cx| {
                                    view.apply_input(
                                        &TerminalInput::Text(format!("{command}\r")),
                                        cx,
                                    )
                                });
                            }
                        } else {
                            let stored_in_background = {
                                let pane = this
                                    .background_sessions
                                    .iter_mut()
                                    .find(|tab| tab.id == tab_id)
                                    .and_then(|tab| tab.pane_mut(pane_id));
                                if let Some(pane) = pane {
                                    pane.terminal = Some(terminal.clone());
                                    true
                                } else {
                                    false
                                }
                            };
                            if stored_in_background {
                                this.observe_background_terminal(
                                    tab_id,
                                    pane_id,
                                    terminal.clone(),
                                    cx,
                                );
                                this.publish_background_session_catalog(cx);
                            }
                        }
                        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                        this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                        if should_focus {
                            view.focus_handle(cx).focus(window, cx);
                        }
                        this.sync_visible_terminals(cx);
                        this.schedule_terminal_spawn_notify(cx);
                        if tracked_multi_command_launch {
                            this.finish_multi_command_launch(window, cx);
                        }
                        if restore_options.is_some() {
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
                            cx.spawn(async move |_this, cx| {
                                startup_handshake.await;
                                terminal_for_startup.update(cx, |terminal, cx| {
                                    terminal.write_init_command_after_startup(command, cx);
                                });
                            })
                            .detach();
                        }
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, window, cx| {
                        if let Some(pane) = this
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.error = Some(format!("{error:#}"));
                        }
                        this.schedule_terminal_spawn_notify(cx);
                        if tracked_multi_command_launch {
                            this.finish_multi_command_launch(window, cx);
                        }
                    })
                    .ok();
                }
            })
            .detach();
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
        zmux::NO_MUX_ENVIRONMENT_VARIABLE.to_owned(),
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

#[cfg(test)]
#[path = "tests/terminal_spawn.rs"]
mod tests;

/// The inputs one stacked command terminal needs. Its `entry_id` addresses
/// the [`PaneStack`] entry that shares `pane_id`'s layout region.
pub(crate) struct StackedTerminalSpawnRequest {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
    pub(crate) entry_id: u64,
    pub(crate) command: String,
    pub(crate) profile: Profile,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
    pub(crate) terminal_theme: Option<Arc<Theme>>,
}

impl Zetta {
    pub(crate) fn spawn_stacked_terminal(
        &mut self,
        request: StackedTerminalSpawnRequest,
        settings: &mut TerminalSpawnSettings,
        // `final_spawn` lets the last terminal of a batch move the shared
        // hyperlink regexes instead of cloning them.
        final_spawn: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let StackedTerminalSpawnRequest {
            tab_id,
            pane_id,
            entry_id,
            command,
            profile,
            working_directory,
            wsl_directory,
            terminal_theme,
        } = request;
        let is_wsl = is_wsl_shell(&profile.command);
        let Some(attention_id) = self.attention_id_for_tab(tab_id) else {
            self.stacked_terminal_failed(
                tab_id,
                pane_id,
                entry_id,
                "Could not identify the terminal's Zetta tab".to_owned(),
                cx,
            );
            return;
        };
        let pane_routing_id = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| {
                tab.pane(pane_id).and_then(|pane| {
                    pane.stack
                        .entries
                        .iter()
                        .find(|entry| entry.id == entry_id)
                        .map(|entry| entry.routing_id)
                })
            })
            .unwrap_or(entry_id);
        let shell = stacked_task_shell(&profile.command, &command, wsl_directory.as_deref());
        let project_environment = self.project_environment_for_tab(tab_id);
        let effective_theme = terminal_theme.clone().unwrap_or_else(|| cx.theme().clone());
        let environment = match (TerminalEnvironment {
            profile: &profile.command,
            overrides: &project_environment,
            attention_id,
            tracking_id: entry_id,
            routing_id: pane_routing_id,
            wsl_cwd_file: None,
            theme_name: &effective_theme.name,
            no_mux: self.no_mux,
        })
        .build()
        {
            Ok(environment) => environment,
            Err(error) => {
                self.stacked_terminal_failed(tab_id, pane_id, entry_id, format!("{error:#}"), cx);
                return;
            }
        };

        let (completion_tx, completion_rx) = async_channel::unbounded();
        let task = SpawnInTerminal {
            id: TaskId(format!("zetta-stacked-{tab_id}-{entry_id}")),
            full_label: command.clone(),
            label: command.clone(),
            command: Some(command.clone()),
            args: Vec::new(),
            command_label: command.clone(),
            cwd: working_directory.clone(),
            env: SpawnInTerminal::default().env,
            use_new_terminal: false,
            allow_concurrent_runs: true,
            reveal: task::RevealStrategy::Never,
            reveal_target: task::RevealTarget::Dock,
            hide: task::HideStrategy::Never,
            shell: shell.clone(),
            show_summary: false,
            show_command: false,
            show_rerun: false,
            save: task::SaveStrategy::None,
        };
        let task_state = TaskState {
            status: TaskStatus::Running,
            completion_rx,
            spawned_task: task,
        };
        let mux_provider = match self.mux_provider_for_tab(tab_id, cx) {
            Ok(provider) => provider,
            Err(error) => {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!(
                        "Could not start the stacked terminal through the session multiplexer: {error:#}"
                    ),
                    cx,
                );
                return;
            }
        };
        let initial_console_palette =
            (!is_wsl).then(|| terminal::console_palette_for_theme(effective_theme.as_ref()));
        let builder = TerminalBuilder::new_with_console_palette(
            working_directory,
            Some(task_state),
            shell,
            environment,
            settings.cursor_shape,
            settings.alternate_scroll,
            settings.max_scroll_history_lines,
            settings.path_hyperlink_regexes(final_spawn),
            settings.path_hyperlink_timeout_ms,
            false,
            cx.entity_id().as_u64(),
            Some(completion_tx),
            cx,
            Vec::new(),
            PathStyle::local(),
            mux_provider
                .clone()
                .map(|provider| provider as Arc<dyn terminal::PtyProvider>),
            initial_console_palette,
        );

        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| match builder.await {
                Ok(mut builder) => {
                    this.update_in(cx, |this, window, cx| {
                        this.adopt_mux_pane(
                            tab_id,
                            entry_id,
                            mux_provider.as_deref(),
                            &mut builder,
                            window,
                            cx,
                        );
                        let terminal = cx.new(|cx| builder.subscribe(cx));
                        let view = cx.new(|cx| {
                            TerminalView::new_with_theme(
                                terminal.clone(),
                                terminal_theme,
                                window,
                                cx,
                            )
                        });
                        this.configure_terminal_view_silent_mode(tab_id, &view, cx);
                        let run_registry = crate::run_command::process_run_registry();
                        let run_identity =
                            crate::run_command::RunPaneIdentity::new(attention_id, pane_routing_id);
                        run_registry.pane_reopened(run_identity);
                        cx.subscribe_in(
                            &terminal,
                            window,
                            move |this, terminal, event: &TerminalEvent, _window, cx| match event {
                                TerminalEvent::TrackingReady => {
                                    run_registry.tracking_ready(run_identity)
                                }
                                TerminalEvent::CommandStarted { command } => {
                                    run_registry.command_started(run_identity, command.clone())
                                }
                                TerminalEvent::CommandFinished { exit_code } => {
                                    run_registry.command_finished(run_identity, *exit_code)
                                }
                                TerminalEvent::TerminalExited(_) => {
                                    run_registry.terminal_lost(run_identity)
                                }
                                TerminalEvent::TaskFinished { exit_code } => {
                                    this.stacked_task_finished(
                                        tab_id, pane_id, entry_id, *exit_code, cx,
                                    );
                                }
                                TerminalEvent::ResizeRequested { .. } => {
                                    terminal.update(cx, |terminal, _| {
                                        terminal.truncate_on_next_resize()
                                    });
                                }
                                _ => {}
                            },
                        )
                        .detach();
                        cx.subscribe_in(
                            &view,
                            window,
                            move |this, _, event, window, cx| match event {
                                TerminalViewEvent::Close => {
                                    this.stacked_terminal_closed(
                                        tab_id, pane_id, entry_id, window, cx,
                                    );
                                }
                                TerminalViewEvent::TitleChanged => cx.notify(),
                                TerminalViewEvent::Input(_) => {}
                                TerminalViewEvent::OpenEditor(request) => {
                                    this.open_editor_in_new_pane(
                                        tab_id,
                                        pane_id,
                                        request.clone(),
                                        window,
                                        cx,
                                    );
                                }
                            },
                        )
                        .detach();
                        let input_enabled = this.terminal_input_enabled();
                        view.update(cx, |view, cx| {
                            view.set_emit_input_events(false);
                            view.set_input_enabled(input_enabled, cx);
                        });
                        let focus_handle = view.focus_handle(cx);
                        cx.on_focus_in(&focus_handle, window, move |this, window, cx| {
                            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                                tab.activate_stack_entry(
                                    pane_id,
                                    PaneStackSelection::Stacked(entry_id),
                                );
                                cx.notify();
                            }
                            this.activate_current_project(window, cx);
                            this.clear_active_tab_attention_if_focused(window, cx);
                        })
                        .detach();
                        let tab_index = this.tabs.iter().position(|tab| tab.id == tab_id);
                        let should_focus = tab_index.is_some_and(|index| {
                            index == this.active_tab
                                && this.tabs[index].active_pane == pane_id
                                && this.tabs[index].pane(pane_id).is_some_and(|pane| {
                                    pane.stack.selected == PaneStackSelection::Stacked(entry_id)
                                })
                        });
                        let inserted = tab_index
                            .and_then(|index| this.tabs.get_mut(index))
                            .and_then(|tab| tab.pane_mut(pane_id))
                            .and_then(|pane| {
                                let entry = pane
                                    .stack
                                    .entries
                                    .iter_mut()
                                    .find(|entry| entry.id == entry_id)?;
                                entry.terminal = Some(terminal.clone());
                                entry.view = Some(view.clone());
                                entry.state = StackedPaneState::Running;
                                Some(())
                            })
                            .is_some();
                        if !inserted {
                            let stored_in_background = this
                                .background_sessions
                                .iter_mut()
                                .find(|tab| tab.id == tab_id)
                                .and_then(|tab| tab.pane_mut(pane_id))
                                .and_then(|pane| {
                                    let entry = pane
                                        .stack
                                        .entries
                                        .iter_mut()
                                        .find(|entry| entry.id == entry_id)?;
                                    entry.terminal = Some(terminal.clone());
                                    entry.state = StackedPaneState::Running;
                                    Some(())
                                })
                                .is_some();
                            if stored_in_background {
                                terminal.update(cx, |terminal, cx| {
                                    terminal.set_ui_visible(false, cx);
                                });
                                this.observe_background_stacked_terminal(
                                    pane_id,
                                    entry_id,
                                    terminal.clone(),
                                    cx,
                                );
                                this.publish_background_session_catalog(cx);
                            }
                        }
                        if should_focus {
                            view.focus_handle(cx).focus(window, cx);
                        }
                        this.sync_visible_terminals(cx);
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, _window, cx| {
                        this.stacked_terminal_failed(
                            tab_id,
                            pane_id,
                            entry_id,
                            format!("{error:#}"),
                            cx,
                        );
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
            })
            .detach();
    }

    pub(crate) fn stacked_terminal_failed(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let entry = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
            .and_then(|pane| {
                pane.stack
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == entry_id)
            });
        if let Some(entry) = entry {
            entry.state = StackedPaneState::Failed;
            entry.error = Some(error);
            cx.notify();
            return;
        }
        let updated_background = self
            .background_sessions
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
            .and_then(|pane| {
                pane.stack
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == entry_id)
            })
            .map(|entry| {
                entry.state = StackedPaneState::Failed;
                entry.error = Some(error);
            })
            .is_some();
        if updated_background {
            self.publish_background_session_catalog(cx);
        }
    }
}

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
