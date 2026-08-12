use super::*;

impl Zetta {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: Profile,
        working_directory: Option<PathBuf>,
        wsl_directory: Option<String>,
        wsl_cwd_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal_theme = match resolve_profile_theme(&profile, cx) {
            Ok(theme) => theme,
            Err(error) => {
                if let Some(pane) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.pane_mut(pane_id))
                {
                    pane.error = Some(format!("Could not apply profile theme: {error:#}"));
                }
                cx.notify();
                return;
            }
        };
        let mut terminal_settings = TerminalSpawnSettings::current(cx);
        let path_hyperlink_regexes = terminal_settings.path_hyperlink_regexes(true);
        self.spawn_terminal_with_theme(
            tab_id,
            pane_id,
            profile,
            working_directory,
            wsl_directory,
            wsl_cwd_file,
            terminal_theme,
            &terminal_settings,
            path_hyperlink_regexes,
            false,
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_terminal_with_theme(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: Profile,
        working_directory: Option<PathBuf>,
        wsl_directory: Option<String>,
        wsl_cwd_file: Option<PathBuf>,
        terminal_theme: Option<Arc<Theme>>,
        settings: &TerminalSpawnSettings,
        path_hyperlink_regexes: Vec<String>,
        tracked_multi_command_launch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.spawn_terminal_with_theme_and_environment(
            tab_id,
            pane_id,
            profile,
            working_directory,
            wsl_directory,
            wsl_cwd_file,
            terminal_theme,
            settings,
            path_hyperlink_regexes,
            HashMap::new(),
            tracked_multi_command_launch,
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_terminal_with_theme_and_environment(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: Profile,
        working_directory: Option<PathBuf>,
        wsl_directory: Option<String>,
        wsl_cwd_file: Option<PathBuf>,
        terminal_theme: Option<Arc<Theme>>,
        settings: &TerminalSpawnSettings,
        path_hyperlink_regexes: Vec<String>,
        environment_overrides: HashMap<String, String>,
        tracked_multi_command_launch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_wsl = is_wsl_shell(&profile.command);
        let Some(attention_id) = self.attention_id_for_tab(tab_id) else {
            if let Some(pane) = self
                .tabs
                .iter_mut()
                .find(|tab| tab.id == tab_id)
                .and_then(|tab| tab.pane_mut(pane_id))
            {
                pane.error = Some("Could not identify the terminal's Zetta tab".to_owned());
            }
            cx.notify();
            return;
        };
        let command = if is_wsl {
            wsl_shell_with_tracking(
                profile.command,
                wsl_directory.as_deref(),
                wsl_cwd_file.as_deref(),
            )
        } else {
            profile.command
        };
        let mut environment = if is_wsl {
            HashMap::default()
        } else {
            let msys2_environment =
                match msys2_cwd_tracking_environment(&command, pane_id, &env::temp_dir()) {
                    Ok(environment) => environment,
                    Err(error) => {
                        if let Some(pane) = self
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.error =
                                Some(format!("Could not configure MSYS2 CWD tracking: {error:#}"));
                        }
                        cx.notify();
                        return;
                    }
                };
            native_terminal_environment()
                .into_iter()
                .chain(msys2_environment)
                .collect()
        };
        if is_wsl {
            wsl_terminal_environment(&mut environment, wsl_cwd_file.as_deref());
        }
        apply_terminal_environment_overrides(
            &mut environment,
            &environment_overrides,
            std::process::id(),
            attention_id,
        );
        if is_wsl {
            add_wsl_environment_variables(&mut environment);
        }
        let builder = TerminalBuilder::new(
            working_directory,
            None,
            command,
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
        );

        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| match builder.await {
                Ok(builder) => {
                    this.update_in(cx, |this, window, cx| {
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
                        cx.subscribe_in(
                            &terminal,
                            window,
                            move |this, _, event: &TerminalEvent, window, cx| {
                                if let TerminalEvent::ResizeRequested { rows, columns } = event {
                                    this.resize_pane_to(
                                        tab_id,
                                        pane_id,
                                        Some(*columns),
                                        Some(*rows),
                                        window,
                                        cx,
                                    );
                                }
                            },
                        )
                        .detach();
                        cx.subscribe_in(
                            &view,
                            window,
                            move |this, _, event, window, cx| match event {
                                TerminalViewEvent::Close => {
                                    this.terminal_closed(tab_id, pane_id, window, cx);
                                }
                                TerminalViewEvent::TitleChanged => {
                                    this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                                    cx.notify();
                                }
                                TerminalViewEvent::Input(input) => {
                                    this.broadcast_input(tab_id, pane_id, input, cx);
                                }
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
                                .filter(|tab| {
                                    tab.pane(pane_id).is_some_and(|pane| !pane.base_exited)
                                })
                            {
                                tab.activate_stack_entry(pane_id, PaneStackSelection::Base);
                                cx.notify();
                            }
                            this.clear_active_tab_attention_if_focused(window, cx);
                        })
                        .detach();
                        let tab_index = this.tabs.iter().position(|tab| tab.id == tab_id);
                        let should_focus = tab_index.is_some_and(|index| {
                            index == this.active_tab
                                && this.tabs[index].active_pane == pane_id
                                && this.tabs[index]
                                    .pane(pane_id)
                                    .is_some_and(|pane| pane.stack.selected_is_base())
                        });
                        if let Some(pane) = tab_index
                            .and_then(|index| this.tabs.get_mut(index))
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.terminal = Some(terminal.clone());
                            pane.view = Some(view.clone());
                            pane.base_exited = false;
                            pane.error = None;
                            if let Some(command) = pane.pending_command.take() {
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
                                this.observe_background_terminal(tab_id, pane_id, terminal, cx);
                                this.publish_background_session_catalog(cx);
                            }
                        }
                        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                        if should_focus {
                            view.focus_handle(cx).focus(window, cx);
                        }
                        this.sync_visible_terminals(cx);
                        this.schedule_terminal_spawn_notify(cx);
                        if tracked_multi_command_launch {
                            this.finish_multi_command_launch(window, cx);
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
}

/// Builds the shell invocation used by a stacked command. Native profiles go
/// through the same shell-aware builder as Zed tasks. WSL and MSYS2 profiles
/// need their launcher arguments preserved so the command runs inside the
/// configured POSIX environment rather than in the Windows command shell.
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

impl Zetta {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_stacked_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
        command: String,
        profile: Profile,
        working_directory: Option<PathBuf>,
        wsl_directory: Option<String>,
        terminal_theme: Option<Arc<Theme>>,
        settings: TerminalSpawnSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let shell = stacked_task_shell(&profile.command, &command, wsl_directory.as_deref());
        let mut environment = if is_wsl {
            let mut environment = HashMap::default();
            wsl_terminal_environment(&mut environment, None);
            environment
        } else {
            let msys2_environment = match msys2_cwd_tracking_environment(
                &profile.command,
                entry_id,
                &env::temp_dir(),
            ) {
                Ok(environment) => environment,
                Err(error) => {
                    self.stacked_terminal_failed(
                        tab_id,
                        pane_id,
                        entry_id,
                        format!("Could not configure MSYS2 CWD tracking: {error:#}"),
                        cx,
                    );
                    return;
                }
            };
            native_terminal_environment()
                .into_iter()
                .chain(msys2_environment)
                .collect()
        };
        environment.insert(
            "ZETTA_PROCESS_ID".to_owned(),
            std::process::id().to_string(),
        );
        environment.insert("ZETTA_ATTENTION_ID".to_owned(), attention_id.to_string());
        if is_wsl {
            add_wsl_environment_variables(&mut environment);
        }

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
        let builder = TerminalBuilder::new(
            working_directory,
            Some(task_state),
            shell,
            environment,
            settings.cursor_shape,
            settings.alternate_scroll,
            settings.max_scroll_history_lines,
            settings.path_hyperlink_regexes,
            settings.path_hyperlink_timeout_ms,
            false,
            cx.entity_id().as_u64(),
            Some(completion_tx),
            cx,
            Vec::new(),
            PathStyle::local(),
        );

        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| match builder.await {
                Ok(builder) => {
                    this.update_in(cx, |this, window, cx| {
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
                        cx.subscribe_in(
                            &terminal,
                            window,
                            move |this, terminal, event: &TerminalEvent, _window, cx| match event {
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
