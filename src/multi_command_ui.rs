use super::*;

impl Zetta {
    pub(crate) fn load_multi_command_catalog(&mut self, cx: &mut Context<Self>) {
        let path = env::var_os("PATH");
        let home = util::paths::home_dir().clone();
        let task = cx.background_spawn(async move { load_completion_catalog(path, &home) });
        cx.spawn(async move |this, cx| {
            let catalog = task.await;
            this.update(cx, |this, cx| {
                this.multi_command_catalog = catalog.clone();
                if let Some(prompt) = this.multi_command.as_mut() {
                    prompt.set_catalog(catalog);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn toggle_multi_command(
        &mut self,
        _: &ToggleMultiCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.multi_command.is_some() {
            self.dismiss_multi_command(window, cx);
            return;
        }
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if self.command_palette.is_some() {
            self.command_palette = None;
        }
        self.multi_command = Some(MultiCommandPrompt::new(self.multi_command_catalog.clone()));
        self.multi_command_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_multi_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.multi_command = None;
        self.focus_active(window, cx);
    }

    pub(crate) fn submit_multi_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(template) = self
            .multi_command
            .as_ref()
            .map(|prompt| prompt.query.clone())
        else {
            return;
        };
        let expansions = match expand_multi_command_with_labels(&template, MAX_PANES_PER_TAB) {
            Ok(expansions) => expansions,
            Err(error) => {
                self.set_multi_command_error(error, cx);
                return;
            }
        };
        let expansions = match MultiCommandExecution::new(expansions) {
            MultiCommandExecution::Single(expansion) => {
                self.submit_single_multi_command(expansion.command, window, cx);
                return;
            }
            MultiCommandExecution::Tiled(expansions) => expansions,
        };

        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let additional = expansions.len() - 1;
        if !can_add_panes(tab.panes.len(), additional) {
            self.set_multi_command_error(
                format!(
                    "This tab has {} panes; the command would exceed the {MAX_PANES_PER_TAB}-pane limit",
                    tab.panes.len()
                ),
                cx,
            );
            return;
        }

        let tab_id = tab.id;
        let active_pane_id = tab.active_pane;
        let active_pane = tab.active_pane();
        let inherited_working_directory = active_pane
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.view.as_ref())
            .and_then(|view| view.read(cx).terminal().read(cx).working_directory());
        let Some(profile) = tab.active_profile().cloned() else {
            return;
        };
        let inherited_wsl_directory = active_pane.and_then(|pane| pane.wsl_working_directory(cx));
        let (working_directory, wsl_directory) = launch_working_directory(
            &profile,
            inherited_working_directory,
            inherited_wsl_directory,
            self.working_directory.clone(),
            self.launch_config.working_directory_configured,
        );
        let terminal_theme = match resolve_profile_theme(&profile, cx) {
            Ok(theme) => theme,
            Err(error) => {
                self.set_multi_command_error(
                    format!("Could not apply the active profile theme: {error:#}"),
                    cx,
                );
                return;
            }
        };
        let terminal_settings = Arc::new(TerminalSpawnSettings::current(cx));

        let new_pane_ids = (0..additional)
            .map(|_| {
                let pane_id = self.next_pane_id;
                self.next_pane_id += 1;
                pane_id
            })
            .collect::<Vec<_>>();
        let pane_ids = std::iter::once(active_pane_id)
            .chain(new_pane_ids.iter().copied())
            .collect::<Vec<_>>();
        let replacement = PaneLayout::tiled(&pane_ids)
            .expect("a multi-command always contains at least two expansions");

        let tab = &mut self.tabs[self.active_tab];
        tab.maximized_pane = None;
        if !tab.layout.replace(active_pane_id, replacement) {
            return;
        }
        let active_view = tab.pane(active_pane_id).and_then(|pane| pane.view.clone());
        tab.pane_mut(active_pane_id).unwrap().generated_label = Some(expansions[0].label.clone());
        if active_view.is_none() {
            tab.pane_mut(active_pane_id).unwrap().pending_command =
                Some(expansions[0].command.clone());
        }
        let mut launches = Vec::with_capacity(additional);
        for (pane_id, expansion) in new_pane_ids.iter().copied().zip(expansions.iter().skip(1)) {
            let wsl_cwd_file = wsl_cwd_tracking_file(&profile, pane_id);
            tab.push_pane(TerminalPane {
                id: pane_id,
                label_number: 0,
                generated_label: Some(expansion.label.clone()),
                custom_label: None,
                profile: profile.clone(),
                view: None,
                error: None,
                wsl_cwd_file: wsl_cwd_file.clone(),
                pending_command: Some(expansion.command.clone()),
            });
            launches.push(QueuedTerminalLaunch {
                tab_id,
                pane_id,
                profile: profile.clone(),
                working_directory: working_directory.clone(),
                wsl_directory: wsl_directory.clone(),
                wsl_cwd_file,
                terminal_theme: terminal_theme.clone(),
                settings: terminal_settings.clone(),
            });
        }
        tab.activate_pane(active_pane_id);

        if let Some(view) = active_view {
            view.update(cx, |view, cx| {
                view.apply_input(
                    &TerminalInput::Text(format!("{}\r", expansions[0].command)),
                    cx,
                )
            });
        }

        self.enqueue_multi_command_launches(launches, window, cx);

        self.multi_command = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn submit_single_multi_command(
        &mut self,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let active_pane_id = tab.active_pane;
        let active_view = tab.pane(active_pane_id).and_then(|pane| pane.view.clone());
        if active_view.is_none() {
            let Some(pane) = tab.pane_mut(active_pane_id) else {
                return;
            };
            pane.pending_command = Some(command);
        } else if let Some(view) = active_view {
            view.update(cx, |view, cx| {
                view.apply_input(&TerminalInput::Text(format!("{command}\r")), cx)
            });
        }

        self.multi_command = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn enqueue_multi_command_launches(
        &mut self,
        launches: impl IntoIterator<Item = QueuedTerminalLaunch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.multi_command_launches.extend(launches);
        self.start_ready_multi_command_launches(window, cx);
    }

    pub(crate) fn finish_multi_command_launch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.multi_command_launches.complete();
        self.start_ready_multi_command_launches(window, cx);
    }

    fn start_ready_multi_command_launches(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(launch) = self.multi_command_launches.pop_ready() {
            let pane_exists = self
                .tabs
                .iter()
                .find(|tab| tab.id == launch.tab_id)
                .and_then(|tab| tab.pane(launch.pane_id))
                .is_some();
            if !pane_exists {
                self.multi_command_launches.complete();
                continue;
            }
            let path_hyperlink_regexes = launch.settings.path_hyperlink_regexes.clone();
            self.spawn_terminal_with_theme(
                launch.tab_id,
                launch.pane_id,
                launch.profile,
                launch.working_directory,
                launch.wsl_directory,
                launch.wsl_cwd_file,
                launch.terminal_theme,
                &launch.settings,
                path_hyperlink_regexes,
                true,
                window,
                cx,
            );
        }
    }

    fn set_multi_command_error(&mut self, error: String, cx: &mut Context<Self>) {
        if let Some(prompt) = self.multi_command.as_mut() {
            prompt.error = Some(error);
        }
        cx.notify();
    }

    pub(crate) fn select_multi_command_completion(
        &mut self,
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(prompt) = self.multi_command.as_mut() {
            prompt.select_completion(selected);
            self.multi_command_focus.focus(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn multi_command_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "tab" {
            let working_directory = self
                .tabs
                .get(self.active_tab)
                .and_then(Tab::active_pane)
                .and_then(|pane| pane.view.as_ref())
                .and_then(|view| view.read(cx).terminal().read(cx).working_directory())
                .or_else(|| self.working_directory.clone())
                .unwrap_or_else(|| util::paths::home_dir().clone());
            let request = self.multi_command.as_mut().and_then(|prompt| {
                if prompt.completion_loading
                    || prompt.cycle_existing_completion(event.keystroke.modifiers.shift)
                {
                    None
                } else {
                    Some(prompt.begin_completion_request(
                        working_directory,
                        event.keystroke.modifiers.shift,
                    ))
                }
            });
            if let Some(mut request) = request {
                match request.take_source() {
                    CompletionSource::Ready(candidates) => {
                        if let Some(prompt) = self.multi_command.as_mut() {
                            prompt.apply_completion_result(&request, candidates);
                        }
                    }
                    CompletionSource::Filesystem {
                        prefix,
                        home,
                        working_directory,
                    } => {
                        let cancellation = request.cancellation.clone();
                        let scan_cancellation = cancellation.clone();
                        let task = cx.background_spawn(async move {
                            filesystem_candidates_cancellable(
                                &prefix,
                                &home,
                                &working_directory,
                                &scan_cancellation,
                            )
                        });
                        let completion_task = cx.spawn(async move |this, cx| {
                            let candidates = task.await;
                            this.update(cx, |this, cx| {
                                if let Some(prompt) = this.multi_command.as_mut() {
                                    prompt.apply_completion_result(&request, candidates);
                                }
                                cx.notify();
                            })
                            .ok();
                        });
                        if let Some(prompt) = self.multi_command.as_mut() {
                            prompt.set_completion_task(completion_task, cancellation);
                        }
                    }
                }
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let Some(prompt) = self.multi_command.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_multi_command(window, cx),
            "enter" if prompt.accept_completion() => cx.notify(),
            "enter" => self.submit_multi_command(window, cx),
            "up" if !prompt.completion_candidates.is_empty() => {
                prompt.navigate_completion(true);
                cx.notify();
            }
            "down" if !prompt.completion_candidates.is_empty() => {
                prompt.navigate_completion(false);
                cx.notify();
            }
            "backspace" => {
                prompt.clear_completion();
                if prompt.select_all {
                    prompt.query.clear();
                    prompt.cursor = 0;
                } else if prompt.cursor > 0 {
                    let previous = previous_char_boundary(&prompt.query, prompt.cursor);
                    prompt.query.replace_range(previous..prompt.cursor, "");
                    prompt.cursor = previous;
                }
                prompt.select_all = false;
                prompt.error = None;
                prompt.mark_query_changed();
                cx.notify();
            }
            "delete" => {
                prompt.clear_completion();
                if prompt.select_all {
                    prompt.query.clear();
                    prompt.cursor = 0;
                } else if prompt.cursor < prompt.query.len() {
                    let next = next_char_boundary(&prompt.query, prompt.cursor);
                    prompt.query.replace_range(prompt.cursor..next, "");
                }
                prompt.select_all = false;
                prompt.error = None;
                prompt.mark_query_changed();
                cx.notify();
            }
            "left" => {
                prompt.clear_completion();
                prompt.cursor = if prompt.select_all {
                    0
                } else {
                    previous_char_boundary(&prompt.query, prompt.cursor)
                };
                prompt.select_all = false;
                prompt.mark_query_changed();
                cx.notify();
            }
            "right" => {
                prompt.clear_completion();
                prompt.cursor = if prompt.select_all {
                    prompt.query.len()
                } else {
                    next_char_boundary(&prompt.query, prompt.cursor)
                };
                prompt.select_all = false;
                prompt.mark_query_changed();
                cx.notify();
            }
            "home" => {
                prompt.clear_completion();
                prompt.cursor = 0;
                prompt.select_all = false;
                prompt.mark_query_changed();
                cx.notify();
            }
            "end" => {
                prompt.clear_completion();
                prompt.cursor = prompt.query.len();
                prompt.select_all = false;
                prompt.mark_query_changed();
                cx.notify();
            }
            "w" if event.keystroke.modifiers.control => {
                prompt.clear_completion();
                prompt.delete_previous_word();
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                prompt.clear_completion();
                prompt.select_all = !prompt.query.is_empty();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    prompt.clear_completion();
                    if prompt.select_all {
                        prompt.query.clear();
                        prompt.cursor = 0;
                        prompt.select_all = false;
                    }
                    prompt.query.insert_str(prompt.cursor, text);
                    prompt.cursor += text.len();
                    prompt.error = None;
                    prompt.mark_query_changed();
                    cx.notify();
                }
            }
            _ => {}
        }
        cx.stop_propagation();
    }
}
