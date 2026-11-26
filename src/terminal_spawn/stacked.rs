//! Spawning the terminal of a stacked command entry, which shares its pane's
//! layout region with the entries above it.

use super::*;

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
            .by_id(tab_id)
            .and_then(|tab| {
                tab.pane(pane_id)
                    .and_then(|pane| pane.stack.entry(entry_id).map(|entry| entry.routing_id))
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
        // Console backend selection must preserve daemon session ownership.
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
        // WSL skips the palette helper, but still names its console backend.
        let initial_console_palette = (!is_wsl
            || profile.console_backend == crate::config::ConsoleBackend::ConsoleServer)
            .then(|| terminal::ConsoleLaunchOptions {
                palette: terminal::console_palette_for_theme(effective_theme.as_ref()),
                backend: profile.console_backend,
            });
        let image_paste_handler = Arc::new(crate::ssh_image_paste::SshImagePasteHandler::new(
            shell.clone(),
            environment.clone(),
            working_directory.clone(),
        ));
        let builder = TerminalBuilder::spawn(
            terminal::TerminalSpawnOptions {
                working_directory,
                task: Some(task_state),
                env: environment,
                cursor_shape: settings.cursor_shape,
                alternate_scroll: settings.alternate_scroll,
                max_scroll_history_lines: settings.max_scroll_history_lines,
                path_hyperlink_regexes: settings.path_hyperlink_regexes(final_spawn),
                path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
                completion_tx: Some(completion_tx),
                pty_provider: mux_provider
                    .clone()
                    .map(|provider| provider as Arc<dyn terminal::PtyProvider>),
                initial_console_palette,
                ..terminal::TerminalSpawnOptions::new(
                    shell,
                    cx.entity_id().as_u64(),
                    PathStyle::local(),
                )
            },
            cx,
        );

        let this = cx.entity().downgrade();
        let spawned = SpawnedStackedTerminal {
            tab_id,
            pane_id,
            entry_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            mux_provider,
            image_paste_handler,
        };
        window
            .spawn(cx, async move |cx| match builder.await {
                Ok(builder) => {
                    this.update_in(cx, |this, window, cx| {
                        this.finish_stacked_terminal_spawn(builder, spawned, window, cx);
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, _window, cx| {
                        this.stacked_terminal_failed(
                            spawned.tab_id,
                            spawned.pane_id,
                            spawned.entry_id,
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

    /// Wires a stacked command's terminal into the pane stack that asked for it.
    ///
    /// Unlike an interactive pane, a stacked entry routes its exit and its focus
    /// through the stack's selection, which is why this does not reuse
    /// `finish_terminal_spawn`.
    pub(super) fn finish_stacked_terminal_spawn(
        &mut self,
        mut builder: TerminalBuilder,
        spawned: SpawnedStackedTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SpawnedStackedTerminal {
            tab_id,
            pane_id,
            entry_id,
            attention_id,
            pane_routing_id,
            terminal_theme,
            mux_provider,
            image_paste_handler,
        } = spawned;
        builder = builder.with_image_paste_handler(image_paste_handler);
        let this = self;
        this.adopt_mux_pane(
            tab_id,
            entry_id,
            mux_provider.as_deref(),
            &mut builder,
            window,
            cx,
        );
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view =
            cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), terminal_theme, window, cx));
        this.configure_terminal_view_silent_mode(tab_id, &view, cx);
        let run_registry = crate::run_command::process_run_registry();
        let run_identity = crate::run_command::RunPaneIdentity::new(attention_id, pane_routing_id);
        run_registry.pane_reopened(run_identity);
        this.subscribe_stacked_entry(
            crate::stacked_panes::StackEntryAddress {
                tab_id,
                pane_id,
                entry_id,
            },
            &terminal,
            &view,
            Some(run_identity),
            window,
            cx,
        );
        let tab_index = this.tab_index(tab_id);
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
                let entry = pane.stack.entry_mut(entry_id)?;
                entry.terminal = Some(terminal.clone());
                entry.view = Some(view.clone());
                entry.state = StackedPaneState::Running;
                Some(())
            })
            .is_some();
        if !inserted {
            let stored_in_background = this
                .background_sessions
                .pane_in_mut(tab_id, pane_id)
                .and_then(|pane| {
                    let entry = pane.stack.entry_mut(entry_id)?;
                    entry.terminal = Some(terminal.clone());
                    entry.state = StackedPaneState::Running;
                    Some(())
                })
                .is_some();
            if stored_in_background {
                terminal.update(cx, |terminal, cx| {
                    terminal.set_ui_visible(false, cx);
                });
                this.observe_background_stacked_terminal(pane_id, entry_id, terminal.clone(), cx);
                this.publish_background_session_catalog(cx);
            }
        }
        if should_focus {
            let focus_handle = view.focus_handle(cx);
            this.focus_terminal_if_allowed(&focus_handle, window, cx);
        }
        this.sync_visible_terminals(cx);
        this.schedule_terminal_spawn_notify(cx);
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
            .pane_in_mut(tab_id, pane_id)
            .and_then(|pane| pane.stack.entry_mut(entry_id));
        if let Some(entry) = entry {
            entry.state = StackedPaneState::Failed;
            entry.error = Some(error);
            cx.notify();
            return;
        }
        let updated_background = self
            .background_sessions
            .pane_in_mut(tab_id, pane_id)
            .and_then(|pane| pane.stack.entry_mut(entry_id))
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
