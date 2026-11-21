//! Stacked panes, several commands sharing one pane's place: moving between
//! entries, closing them, and following each command until it finishes.

use super::*;

/// Where a stacked entry lives: the tab, the pane whose stack holds it, and its
/// own id.
#[derive(Clone, Copy)]
pub(crate) struct StackEntryAddress {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
    pub(crate) entry_id: u64,
}

impl Zetta {
    /// The subscriptions a visible stacked entry's terminal and view need,
    /// whether it was just spawned or came back from a background session:
    /// its exit, its close, its view's events, input gating, and focus
    /// selecting it in the stack.
    pub(crate) fn subscribe_stacked_entry(
        &mut self,
        entry: StackEntryAddress,
        terminal: &Entity<Terminal>,
        view: &Entity<TerminalView>,
        run_identity: Option<crate::run_command::RunPaneIdentity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let StackEntryAddress {
            tab_id,
            pane_id,
            entry_id,
        } = entry;
        let run_registry = crate::run_command::process_run_registry();
        cx.subscribe_in(
            terminal,
            window,
            move |this, _terminal, event: &TerminalEvent, _window, cx| {
                if let Some(identity) = run_identity {
                    run_registry.observe_terminal_event(identity, event);
                }
                if let TerminalEvent::TaskFinished { exit_code } = event {
                    this.stacked_task_finished(tab_id, pane_id, entry_id, *exit_code, cx);
                }
            },
        )
        .detach();
        cx.subscribe_in(
            view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => {
                    this.stacked_terminal_closed(tab_id, pane_id, entry_id, window, cx);
                }
                TerminalViewEvent::TitleChanged => cx.notify(),
                TerminalViewEvent::PasteError(error) => this.show_error_notice(error.clone(), cx),
                TerminalViewEvent::Input(_) => {}
                TerminalViewEvent::OpenEditor(request) => {
                    this.open_editor_in_new_pane(tab_id, pane_id, request.clone(), window, cx);
                }
            },
        )
        .detach();
        let input_enabled = self.terminal_input_enabled();
        view.update(cx, |view, cx| {
            view.set_emit_input_events(false);
            view.set_input_enabled(input_enabled, cx);
        });
        let focus_handle = view.focus_handle(cx);
        cx.on_focus_in(&focus_handle, window, move |this, window, cx| {
            if let Some(tab) = this.tabs.by_id_mut(tab_id) {
                tab.activate_stack_entry(pane_id, PaneStackSelection::Stacked(entry_id));
                cx.notify();
            }
            this.activate_current_project(window, cx);
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
    }

    pub(crate) fn select_previous_stacked_pane(
        &mut self,
        _: &SelectPreviousStackedPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_stacked_pane(false, window, cx);
    }

    pub(crate) fn select_next_stacked_pane(
        &mut self,
        _: &SelectNextStackedPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_stacked_pane(true, window, cx);
    }

    fn cycle_stacked_pane(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let view = {
            let Some(tab) = self.current_tab_mut() else {
                return;
            };
            let pane_id = tab.active_pane;
            let Some(pane) = tab.pane_mut(pane_id) else {
                return;
            };
            let selection = if pane.base_exited {
                pane.stack.cycle_without_base(forward)
            } else {
                pane.stack.cycle(forward)
            };
            if selection.is_none() {
                return;
            }
            tab.active_view()
        };
        if let Some(view) = view {
            let focus_handle = view.focus_handle(cx);
            self.focus_terminal_if_allowed(&focus_handle, window, cx);
        } else {
            self.focus_active(window, cx);
        }
        self.sync_visible_terminals(cx);
        cx.notify();
    }

    pub(crate) fn select_stacked_pane_by_id(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        selection: PaneStackSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = {
            let Some(tab) = self.tabs.by_id_mut(tab_id) else {
                return;
            };
            tab.activate_stack_entry(pane_id, selection);
            tab.active_view()
        };
        if let Some(view) = view {
            let focus_handle = view.focus_handle(cx);
            self.focus_terminal_if_allowed(&focus_handle, window, cx);
        } else {
            self.focus_active(window, cx);
        }
        self.sync_visible_terminals(cx);
        cx.notify();
    }

    pub(crate) fn close_stacked_pane(
        &mut self,
        _: &CloseStackedPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((tab_id, pane_id, entry_id)) = self.current_tab().and_then(|tab| {
            let Some(PaneStackSelection::Stacked(entry_id)) = tab
                .active_pane()
                .map(|pane| pane.focused_stack_selection(window, cx))
            else {
                return None;
            };
            Some((tab.id, tab.active_pane, entry_id))
        }) else {
            return;
        };
        self.close_stacked_pane_by_id(tab_id, pane_id, entry_id, window, cx);
    }

    pub(crate) fn close_stacked_pane_by_id(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let run_identity = self.run_stacked_pane_identity(tab_id, pane_id, entry_id);
        if let Some(tab) = self.tabs.by_id_mut(tab_id) {
            let (removed, close_host) = {
                let Some(pane) = tab.pane_mut(pane_id) else {
                    return;
                };
                let removed = pane.stack.remove(entry_id);
                let close_host = removed.is_some() && pane.base_exited && pane.stack.is_empty();
                (removed, close_host)
            };
            if removed.is_none() {
                return;
            }
            if let Some(identity) = run_identity {
                crate::run_command::process_run_registry().pane_closed(identity);
            }
            self.background_observed_panes.remove(&entry_id);
            if close_host {
                self.close_pane(tab_id, pane_id, window, cx);
                return;
            }
            self.retain_open_visible_terminals();
            self.focus_active(window, cx);
            self.sync_visible_terminals(cx);
            cx.notify();
            return;
        }

        let removed = self
            .background_sessions
            .pane_in_mut(tab_id, pane_id)
            .and_then(|pane| pane.stack.remove(entry_id));
        if removed.is_some() {
            if let Some(identity) = run_identity {
                crate::run_command::process_run_registry().pane_closed(identity);
            }
            self.background_observed_panes.remove(&entry_id);
            self.publish_background_session_catalog(cx);
        }
    }

    pub(crate) fn submit_stacked_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self
            .multi_command
            .as_ref()
            .map(|prompt| prompt.query.text.clone())
        else {
            return;
        };
        if command.trim().is_empty() {
            self.set_multi_command_error("Enter a command to run".to_owned(), cx);
            return;
        }

        let project = self.active_project_config().cloned();
        let working_directory_configured = self.effective_config().working_directory_configured;
        let Some(tab) = self.current_tab() else {
            return;
        };
        let tab_id = tab.id;
        let tab_theme_override = tab.theme_override.clone();
        let pane_id = tab.active_pane;
        let Some(host) = tab.pane(pane_id) else {
            return;
        };
        if host.stack.entries.len() >= MAX_PANES_PER_TAB.saturating_sub(1) {
            self.set_multi_command_error(
                format!(
                    "This pane has reached the {MAX_PANES_PER_TAB}-entry stacked-command limit"
                ),
                cx,
            );
            return;
        }
        let profile = host.profile.clone();
        let inherited_working_directory = (!is_wsl_shell(&profile.command))
            .then(|| host.working_directory(cx))
            .flatten();
        let inherited_wsl_directory = is_wsl_shell(&profile.command)
            .then(|| host.wsl_working_directory(cx))
            .flatten();
        let (working_directory, wsl_directory) = launch_working_directory(
            &profile,
            inherited_working_directory,
            inherited_wsl_directory,
            self.working_directory.clone(),
            working_directory_configured,
        );
        let terminal_theme = match self.pane_theme(
            tab_id,
            &profile,
            project.as_deref(),
            ThemeOverrides {
                pane: None,
                tab: tab_theme_override.as_deref(),
            },
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
        let entry_id = take_next_id(&mut self.next_pane_id);
        let mut settings = TerminalSpawnSettings::current(cx);
        let entry = StackedPane::new(
            entry_id,
            command,
            profile.clone(),
            working_directory.clone(),
            wsl_directory.clone(),
        );
        let inserted = self
            .current_tab_mut()
            .and_then(|tab| tab.pane_mut(pane_id))
            .is_some_and(|pane| pane.stack.push(entry));
        if !inserted {
            return;
        }

        self.multi_command = None;
        self.multi_command_mode = CommandPromptMode::Multi;
        let Some((command, profile, working_directory, wsl_directory)) = self
            .current_tab()
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| {
                pane.stack.entry(entry_id).map(|entry| {
                    (
                        entry.command.clone(),
                        entry.profile.clone(),
                        entry.working_directory.clone(),
                        entry.wsl_directory.clone(),
                    )
                })
            })
        else {
            return;
        };
        self.spawn_stacked_terminal(
            StackedTerminalSpawnRequest {
                tab_id,
                pane_id,
                entry_id,
                command,
                profile,
                working_directory,
                wsl_directory,
                terminal_theme,
            },
            &mut settings,
            true,
            window,
            cx,
        );
        self.focus_active(window, cx);
        self.sync_visible_terminals(cx);
        cx.notify();
    }

    pub(crate) fn stacked_task_finished(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
        exit_code: Option<i32>,
        cx: &mut Context<Self>,
    ) {
        let run_identity = self.run_stacked_pane_identity(tab_id, pane_id, entry_id);
        let mut found = false;
        if let Some(tab) = self.tabs.by_id_mut(tab_id) {
            if let Some(entry) = tab
                .pane_mut(pane_id)
                .and_then(|pane| pane.stack.entry_mut(entry_id))
            {
                entry.state = StackedPaneState::Completed;
                entry.exit_code = exit_code;
                found = true;
            }
        } else if let Some(entry) = self
            .background_sessions
            .pane_in_mut(tab_id, pane_id)
            .and_then(|pane| pane.stack.entry_mut(entry_id))
        {
            entry.state = StackedPaneState::Completed;
            entry.exit_code = exit_code;
            found = true;
        }
        if found {
            if let Some(identity) = run_identity {
                crate::run_command::process_run_registry().command_finished(identity, exit_code);
            }
            if !self.tabs.iter().any(|tab| tab.id == tab_id) {
                self.publish_background_session_catalog(cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn stacked_terminal_closed(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_stacked_pane_by_id(tab_id, pane_id, entry_id, window, cx);
    }
}

#[cfg(test)]
#[path = "tests/stacked_panes.rs"]
mod tests;
