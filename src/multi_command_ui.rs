use super::*;

/// Whether a tab that already has `existing` panes can hold the `expansions`
/// panes a tiled multi-command would open, or the message to show instead.
///
/// The command replaces the active pane and tiles the rest beside it, so it
/// needs `expansions - 1` more panes rather than `expansions`.
fn multi_command_pane_budget(existing: usize, expansions: usize) -> Result<(), String> {
    let additional = expansions.saturating_sub(1);
    if can_add_panes(existing, additional) {
        return Ok(());
    }
    Err(format!(
        "This tab has {existing} panes; the command would exceed the {MAX_PANES_PER_TAB}-pane limit"
    ))
}

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
        self.multi_command_mode = CommandPromptMode::Multi;
        self.multi_command = Some(MultiCommandPrompt::new(self.multi_command_catalog.clone()));
        self.multi_command_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn toggle_stacked_command(
        &mut self,
        _: &ToggleStackedCommand,
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
        self.multi_command_mode = CommandPromptMode::Stacked;
        self.multi_command = Some(MultiCommandPrompt::new(self.multi_command_catalog.clone()));
        self.multi_command_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_multi_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.multi_command = None;
        self.multi_command_mode = CommandPromptMode::Multi;
        self.focus_active(window, cx);
    }

    pub(crate) fn submit_multi_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.multi_command_mode == CommandPromptMode::Stacked {
            self.submit_stacked_command(window, cx);
            return;
        }
        let Some(template) = self
            .multi_command
            .as_ref()
            .map(|prompt| prompt.query.text.clone())
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

        let inherit_working_directory = self
            .effective_config()
            .working_directory_scope
            .inherits_for_new_pane();
        let working_directory_configured = self.effective_config().working_directory_configured;
        let project = self.active_project_config().cloned();
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        // The command replaces the active pane and tiles the rest beside it.
        let additional = expansions.len() - 1;
        if let Err(message) = multi_command_pane_budget(tab.panes.len(), expansions.len()) {
            self.set_multi_command_error(message, cx);
            return;
        }

        let tab_id = tab.id;
        let tab_theme_override = tab.theme_override.clone();
        let active_pane_id = tab.active_pane;
        let active_pane = tab.active_pane();
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
        let Some(profile) = tab.active_profile().cloned() else {
            return;
        };
        let inherited_wsl_directory = active_pane
            .filter(|_| inherit_working_directory)
            .and_then(|pane| pane.wsl_working_directory(cx));
        let (working_directory, wsl_directory) = launch_working_directory(
            &profile,
            inherited_working_directory,
            inherited_wsl_directory,
            self.working_directory.clone(),
            working_directory_configured,
        );
        let terminal_theme = match resolve_terminal_theme(
            None,
            tab_theme_override.as_deref(),
            &profile,
            project.as_deref(),
            cx,
        ) {
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
        for pane_id in &new_pane_ids {
            self.projects.inherit_pane_root(active_pane_id, *pane_id);
        }
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
            tab.push_pane(
                TerminalPane::new(pane_id, profile.clone())
                    .with_generated_label(expansion.label.clone())
                    .with_wsl_cwd_file(wsl_cwd_file.clone())
                    .with_pending_command(Some(expansion.command.clone())),
            );
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
                );
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
                view.apply_input(&TerminalInput::Text(format!("{command}\r")), cx);
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
            self.spawn_terminal(
                TerminalSpawnRequest {
                    working_directory: launch.working_directory,
                    wsl_directory: launch.wsl_directory,
                    wsl_cwd_file: launch.wsl_cwd_file,
                    terminal_theme: launch.terminal_theme,
                    path_hyperlink_regexes,
                    tracked_multi_command_launch: true,
                    ..TerminalSpawnRequest::new(launch.tab_id, launch.pane_id, launch.profile)
                },
                &launch.settings,
                window,
                cx,
            );
        }
    }

    pub(crate) fn set_multi_command_error(&mut self, error: String, cx: &mut Context<Self>) {
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
                .and_then(|pane| pane.working_directory(cx))
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
        // Settled before this prompt's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        match apply_clipboard_shortcut(&mut prompt.query, &event.keystroke, cx) {
            ClipboardOutcome::Ignored => {}
            ClipboardOutcome::Unchanged => {
                cx.notify();
                return;
            }
            ClipboardOutcome::Edited => {
                // The same bookkeeping typing does: a stale completion no longer
                // describes the query, and a previous error no longer describes
                // what is in the field.
                prompt.clear_completion();
                prompt.error = None;
                prompt.mark_query_changed();
                cx.notify();
                return;
            }
        }
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
            "w" if event.keystroke.modifiers.control => {
                prompt.clear_completion();
                prompt.delete_previous_word();
                cx.notify();
            }
            // Every key the field answers to invalidates the completion, whose
            // candidates describe the query they were requested for and the
            // position they were requested at. An edit also clears the error,
            // which no longer describes what is in the field.
            _ => match apply_text_field_key(&mut prompt.query, &event.keystroke) {
                TextFieldEdit::Ignored => {}
                TextFieldEdit::CursorMoved => {
                    prompt.clear_completion();
                    prompt.mark_query_changed();
                    cx.notify();
                }
                TextFieldEdit::Edited => {
                    prompt.clear_completion();
                    prompt.error = None;
                    prompt.mark_query_changed();
                    cx.notify();
                }
            },
        }
        cx.stop_propagation();
    }

    /// The centred multi-command prompt, backdrop included.
    pub(crate) fn render_multi_command_overlay(
        &mut self,
        colors: &ThemeColors,
        error_color: Hsla,
        handle: &WeakEntity<Self>,
    ) -> Option<AnyElement> {
        let multi_command_focus = self.multi_command_focus.clone();
        let stacked_prompt = self.multi_command_mode == CommandPromptMode::Stacked;
        let prompt = self.multi_command.as_mut()?;
        let (query_before, query_after) = prompt.rendered_query_parts();
        let query_empty = prompt.query.text.is_empty();
        let query_selected = prompt.query.select_all;
        let placeholder = query_empty.then(|| {
            SharedString::from(if stacked_prompt {
                "git status"
            } else {
                "ssh {{a,b,c,d}}.example.com"
            })
        });
        let query = field_text_run(
            query_before,
            query_after,
            query_selected,
            placeholder,
            colors,
        );
        let error = prompt.error.clone();
        let completion_selected = prompt.completion_selected;
        let completion_count = prompt.completion_candidates.len();
        let completion_loading = prompt.completion_loading;
        let completion_visible_start = completion_selected.unwrap_or(0).saturating_sub(7);
        let completion_rows = prompt
            .completion_candidates
            .iter()
            .skip(completion_visible_start)
            .take(8)
            .enumerate()
            .map(|(index, candidate)| {
                let completion_index = completion_visible_start + index;
                let completion_handle = handle.clone();
                div()
                    .id(("multi-command-completion", completion_index))
                    .h_7()
                    .px_3()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(colors.text)
                    .when(completion_selected == Some(completion_index), |row| {
                        row.bg(colors.element_selected)
                    })
                    .hover(|style| style.bg(colors.element_hover))
                    .on_click(move |_, window, cx| {
                        completion_handle
                            .update(cx, |this, cx| {
                                this.select_multi_command_completion(completion_index, window, cx);
                            })
                            .ok();
                    })
                    .child(candidate.clone())
            })
            .collect::<Vec<_>>();
        let dismiss_handle = handle.clone();

        Some(
            div()
                .id(if stacked_prompt {
                    "stacked-command-backdrop"
                } else {
                    "multi-command-backdrop"
                })
                .absolute()
                .inset_0()
                .pt(px(72.))
                .px_4()
                .flex()
                .items_start()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    dismiss_handle
                        .update(cx, |this, cx| this.dismiss_multi_command(window, cx))
                        .ok();
                })
                .child(
                    div()
                        .id(if stacked_prompt {
                            "stacked-command-prompt"
                        } else {
                            "multi-command-prompt"
                        })
                        .track_focus(&multi_command_focus)
                        .w_full()
                        .max_w(px(680.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .text_color(colors.text)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h_12()
                                .px_3()
                                .flex()
                                .items_center()
                                .text_color(colors.text)
                                .child(div().text_color(colors.text_accent).mr_2().child("$"))
                                .child(query),
                        )
                        .when(completion_count > 0, |prompt| {
                            prompt.child(
                                div()
                                    .py_1()
                                    .border_t_1()
                                    .border_color(colors.border)
                                    .children(completion_rows),
                            )
                        })
                        .child(
                            div()
                                .min_h_9()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(colors.border)
                                .text_xs()
                                .text_color(if error.is_some() {
                                    error_color
                                } else {
                                    colors.text_muted
                                })
                                .child(error.unwrap_or_else(|| {
                                    if completion_loading {
                                        "Loading completions…".to_owned()
                                    } else if completion_count > 0 {
                                        format!(
                                            "{completion_count} completion{} · Tab next · Shift+Tab previous",
                                            if completion_count == 1 { "" } else { "s" }
                                        )
                                    } else {
                                        if stacked_prompt {
                                            "Run one command in a stacked pane · Tab complete · Enter run · Esc cancel"
                                                .to_owned()
                                        } else {
                                            "Double-brace values become tiled panes · Tab complete · Enter run · Esc cancel"
                                                .to_owned()
                                        }
                                    }
                                })),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "tests/multi_command_ui.rs"]
mod tests;
