//! Opening, closing, pinning, and ordering tabs.
//!
//! A tab's *content* is `pane.rs`'s; what is here is the tab's place in the
//! window — which one is active, what order they sit in, and what a close has
//! to settle first (a pinned tab's confirmation, a background session's
//! protection) before the tab can go.

use super::*;

impl Zetta {
    pub(crate) fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_profile = self.tabs.get(self.active_tab).and_then(Tab::active_profile);
        let project = self.active_project_config().cloned();
        let effective = project
            .as_ref()
            .map(|project| &project.effective)
            .unwrap_or(&self.launch_config);
        let Some(profile) = new_tab_profile(
            active_profile,
            &self.profiles,
            effective.default_profile,
            effective.new_tab_profile,
        ) else {
            return;
        };
        self.open_tab_with_profile_context(
            profile,
            project,
            NewTabOrigin::CurrentSession,
            None,
            None,
            TerminalLaunch::Spawn,
            window,
            cx,
        );
    }

    pub(crate) fn open_tab_with_profile(
        &mut self,
        profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.active_project_config().cloned();
        self.open_tab_with_profile_context(
            profile,
            project,
            NewTabOrigin::CurrentSession,
            None,
            None,
            TerminalLaunch::Spawn,
            window,
            cx,
        );
    }

    pub(crate) fn open_tab_with_profile_in_project(
        &mut self,
        profile: Profile,
        project: Arc<ProjectConfig>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_tab_with_profile_context(
            profile,
            Some(project),
            NewTabOrigin::ProjectEntry,
            None,
            None,
            TerminalLaunch::Spawn,
            window,
            cx,
        );
    }

    pub(crate) fn open_command_in_new_tab(
        &mut self,
        request: PaneCommand,
        working_directory: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        anyhow::ensure!(
            request.direction.is_none()
                && request.label.is_none()
                && request.pane.is_none()
                && request.overlay.is_none()
                && !request.stack
                && !request.list
                && !request.command.is_empty(),
            "a default-terminal command must contain only a command and its arguments"
        );
        let project = self.active_project_config().cloned();
        let effective = project
            .as_ref()
            .map(|project| &project.effective)
            .unwrap_or(&self.launch_config);
        let active_profile = self.tabs.get(self.active_tab).and_then(Tab::active_profile);
        let profile = new_tab_profile(
            active_profile,
            &self.profiles,
            effective.default_profile,
            effective.new_tab_profile,
        )
        .context("no terminal profile is configured")?;
        self.open_tab_with_profile_context(
            profile,
            project,
            NewTabOrigin::CurrentSession,
            Some(request.command),
            working_directory,
            TerminalLaunch::Spawn,
            window,
            cx,
        );
        Ok(())
    }

    #[cfg(windows)]
    pub(crate) fn open_windows_handoff(
        &mut self,
        request: crate::windows_integration::WindowsHandoffRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let project = self.active_project_config().cloned();
        let effective = project
            .as_ref()
            .map(|project| &project.effective)
            .unwrap_or(&self.launch_config);
        let active_profile = self.tabs.get(self.active_tab).and_then(Tab::active_profile);
        let Some(profile) = new_tab_profile(
            active_profile,
            &self.profiles,
            effective.default_profile,
            effective.new_tab_profile,
        ) else {
            return false;
        };
        let title = request
            .startup
            .as_ref()
            .and_then(|startup| startup.title.clone());
        self.open_tab_with_profile_context(
            profile,
            project,
            NewTabOrigin::CurrentSession,
            None,
            None,
            TerminalLaunch::Handoff(request),
            window,
            cx,
        );
        if let Some(title) = title.filter(|title| !title.is_empty())
            && let Some(tab) = self.tabs.get_mut(self.active_tab)
        {
            tab.custom_title = Some(title);
            cx.notify();
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_tab_with_profile_context(
        &mut self,
        mut profile: Profile,
        project: Option<Arc<ProjectConfig>>,
        origin: NewTabOrigin,
        pending_command: Option<Vec<String>>,
        working_directory_override: Option<PathBuf>,
        launch: TerminalLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        apply_launch_theme_override(&mut profile, self.launch_theme_override.as_ref());
        let mut pending_command_error = None;
        let pending_command =
            pending_command.and_then(|command| {
                match quote_pane_command_for_shell(&profile.command, &command) {
                    Ok(command) => Some(command),
                    Err(error) => {
                        pending_command_error =
                            Some(format!("Could not prepare command: {error:#}"));
                        None
                    }
                }
            });
        let active_pane = self.tabs.get(self.active_tab).and_then(Tab::active_pane);
        let effective = project
            .as_ref()
            .map(|project| &project.effective)
            .unwrap_or(&self.launch_config);
        let inherit_working_directory =
            origin.inherits_working_directory(effective.working_directory_scope);
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
        let inherited_wsl_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| pane.profile.name.eq_ignore_ascii_case(&profile.name))
            .and_then(|pane| pane.wsl_working_directory(cx));
        let (working_directory, wsl_directory) = if working_directory_override
            .as_ref()
            .is_some_and(|_| !is_wsl_shell(&profile.command))
        {
            (working_directory_override, None)
        } else {
            launch_working_directory(
                &profile,
                inherited_working_directory,
                inherited_wsl_directory,
                effective.working_directory.clone(),
                effective.working_directory_configured,
            )
        };
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let attention_id = if cx.has_global::<ZettaProcessState>() {
            let process = cx.global_mut::<ZettaProcessState>();
            let attention_id = process.next_attention_id;
            process.next_attention_id += 1;
            attention_id
        } else {
            let attention_id = self.next_attention_id;
            self.next_attention_id += 1;
            attention_id
        };
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let wsl_cwd_file = wsl_cwd_tracking_file(&profile, pane_id);
        if let Some(project) = &project {
            self.projects
                .pane_roots
                .insert(pane_id, project.root.clone());
        }
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                [pane_id],
            ));
        self.tabs.push(Tab {
            id: tab_id,
            attention_id,
            attention: None,
            panes: vec![
                TerminalPane::new(pane_id, profile.clone())
                    .with_label_number(1)
                    .with_wsl_cwd_file(wsl_cwd_file.clone())
                    .with_pending_command(pending_command),
            ],
            pane_indices: HashMap::from([(pane_id, 0)]),
            next_pane_label: 2,
            theme_override: None,
            layout: PaneLayout::Pane(pane_id),
            active_pane: pane_id,
            focus_history: vec![pane_id],
            maximized_pane: None,
            minimized_panes: Vec::new(),
            selected_minimized_pane: None,
            broadcast_input: false,
            silent_mode: false,
            close_policy: TabClosePolicy::Close,
            shared: false,
            custom_title: None,
            worktree_seed_title: None,
            process_title: None,
            // Never seed from `effective`: see `apply_project_tab_icon`'s doc comment
            // for why a new tab must start from the non-project default even when
            // opening directly into a project.
            icon: self.launch_config.default_tab_icon,
            icon_override: TabIconOverride::None,
            pinned: false,
            renaming_pane: None,
            rename_buffer: None,
            editing_overlay_pane: None,
            overlay_buffer: None,
            overlay_style_picker: None,
        });
        self.active_tab = self.tabs.len() - 1;
        if let Some(error) = pending_command_error
            && let Some(pane) = self.tabs.last_mut().and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.error = Some(error);
        }

        // Stop the previously active terminal from driving the foreground executor before
        // starting the asynchronous PTY setup. Waiting for that setup to finish before the next
        // render leaves high-volume output fully active during the entire tab-spawn operation.
        for terminal in std::mem::take(&mut self.visible_terminals) {
            terminal.update(cx, |terminal, cx| terminal.set_ui_visible(false, cx));
        }
        cx.notify();

        match launch {
            TerminalLaunch::Spawn => self.spawn_terminal_for_pane(
                TerminalSpawnRequest {
                    working_directory,
                    wsl_directory,
                    wsl_cwd_file,
                    ..TerminalSpawnRequest::new(tab_id, pane_id, profile)
                },
                window,
                cx,
            ),
            #[cfg(windows)]
            TerminalLaunch::Handoff(request) => {
                self.spawn_windows_handoff_terminal(tab_id, pane_id, profile, request, window, cx)
            }
        }
        if project.is_some() {
            self.activate_current_project(window, cx);
        }
        self.focus_active(window, cx);
    }

    pub(crate) fn close_tab_at(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_tab_at_with_policy(index, true, window, cx);
    }

    pub(super) fn close_tab_at_with_policy(
        &mut self,
        index: usize,
        background_if_pinned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        let tab_id = self.tabs[index].id;
        self.cancel_tab_search_for_tab(tab_id, cx);
        let has_failed_pane = self.tabs[index]
            .panes
            .iter()
            .any(|pane| pane.exit.is_some());
        let background_authentication = background_authentication_for_close(
            &self.tabs[index].close_policy,
            self.tabs[index].shared,
            background_if_pinned,
            has_failed_pane,
        );
        if let Some(authentication) = background_authentication {
            self.move_tab_to_background(index, authentication, cx);
            if self.tabs.is_empty() {
                window.remove_window();
            } else {
                self.focus_active(window, cx);
            }
            return;
        }
        let closed_pane_ids = self.tabs[index]
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        self.projects
            .forget_tab(tab_id, closed_pane_ids.iter().copied());
        let run_registry = crate::run_command::process_run_registry();
        for pane in &self.tabs[index].panes {
            run_registry.pane_closed(crate::run_command::RunPaneIdentity::new(
                self.tabs[index].attention_id,
                pane.routing_id,
            ));
            for entry in &pane.stack.entries {
                run_registry.pane_closed(crate::run_command::RunPaneIdentity::new(
                    self.tabs[index].attention_id,
                    entry.routing_id,
                ));
            }
        }
        for pane_id in &closed_pane_ids {
            self.drop_shared_pane(*pane_id);
            self.release_mux_pane(tab_id, *pane_id, cx);
        }
        self.mux_panes.forget_tab(tab_id);
        self.forget_pane_controls(closed_pane_ids);
        self.tabs.remove(index);
        self.retain_open_visible_terminals();
        self.disable_tab_move_mode_if_unavailable(cx);
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        if index < self.active_tab {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        // Returning to a tab can change its pane bounds during the first paint. Keep that
        // visibility transition from synchronously reflowing its complete retained history.
        for terminal in self.tabs[self.active_tab]
            .panes
            .iter()
            .flat_map(TerminalPane::all_terminals)
        {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }
        self.focus_active(window, cx);
    }
}

impl Zetta {
    pub(crate) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    pub(crate) fn new_window(&mut self, _: &NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .active_project_config()
            .map(|project| project.as_ref().clone());
        open_zetta_window(
            self.launch_config.clone(),
            self.configuration_error.clone(),
            None,
            project,
            None,
            None,
            false,
            None,
            false,
            self.no_mux,
            None,
            None,
            None,
            None,
            cx,
        )
        .log_err();
    }

    pub(crate) fn open_profile(
        &mut self,
        action: &OpenProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hidden_profiles = self.effective_config().hidden_profiles.clone();
        let Some(index) = visible_profile_index(&self.profiles, &hidden_profiles, action.slot)
        else {
            return;
        };
        let profile = self.profiles[index].clone();
        self.open_tab_with_profile(profile, window, cx);
    }

    pub(crate) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_id) = self.tabs.get(self.active_tab).map(|tab| tab.id) else {
            return;
        };
        if self.tabs[self.active_tab].pinned {
            self.prompt_to_confirm_tab_close(tab_id, window, cx);
        } else {
            self.close_tab_at(self.active_tab, window, cx);
        }
    }

    pub(crate) fn toggle_tab_pinning(
        &mut self,
        _: &ToggleTabPinning,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(insertion_index) = toggle_tab_pinning_in_order(&mut self.tabs, self.active_tab)
        else {
            return;
        };
        self.active_tab = insertion_index;
        self.tab_overflow_selection_side = None;
        cx.notify();
    }
}

impl Zetta {
    pub(crate) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.tab_overflow_selection_side = None;
            self.dismiss_tab_overflow_menus(cx);
            self.focus_active(window, cx);
        }
    }

    pub(crate) fn previous_tab(
        &mut self,
        _: &PreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
            self.tab_overflow_selection_side = None;
            self.dismiss_tab_overflow_menus(cx);
            self.focus_active(window, cx);
        }
    }

    /// Closes any open tab-overflow popover before the active tab changes underneath
    /// it. Without this, wrapping past the edge of the tab bar while a keyboard-opened
    /// overflow menu is still showing leaves that (now stale) popover holding focus,
    /// so the terminal never gets it back.
    pub(super) fn dismiss_tab_overflow_menus(&mut self, cx: &mut App) {
        if self.tab_overflow_keyboard_menu_edge.take().is_some() {
            self.tab_overflow_left_menu_handle.hide(cx);
            self.tab_overflow_right_menu_handle.hide(cx);
        }
    }

    pub(crate) fn terminal_input_enabled(&self) -> bool {
        pane_input_enabled(self.pane_resize_mode || self.pane_move_mode || self.tab_move_mode)
    }

    pub(crate) fn update_terminal_input_enabled(&self, cx: &mut App) {
        let enabled = self.terminal_input_enabled();
        for view in self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .flat_map(TerminalPane::all_views)
        {
            view.update(cx, |view, cx| view.set_input_enabled(enabled, cx));
        }
    }

    pub(crate) fn disable_tab_move_mode_if_unavailable(&mut self, cx: &mut App) {
        if self.tabs.len() < 2 && self.tab_move_mode {
            self.tab_move_mode = false;
            self.update_terminal_input_enabled(cx);
        }
    }

    pub(crate) fn toggle_tab_move_mode(
        &mut self,
        _: &ToggleTabMoveMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() < 2 && !self.tab_move_mode {
            return;
        }

        self.tab_move_mode = !self.tab_move_mode;
        self.tab_overflow_selection_side = None;
        self.dismiss_tab_overflow_menus(cx);
        if self.tab_move_mode {
            self.pane_resize_mode = false;
            self.pane_move_mode = false;
            self.pane_resize_keys.clear();
            self.pane_resize_repeat_generation = self.pane_resize_repeat_generation.wrapping_add(1);
            self.pane_resize_drag = None;
        }
        self.update_terminal_input_enabled(cx);
        self.focus_active(window, cx);
    }

    pub(crate) fn move_tab_left(
        &mut self,
        _: &MoveTabLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_tab(TabMoveDirection::Left, window, cx);
    }

    pub(crate) fn move_tab_right(
        &mut self,
        _: &MoveTabRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_tab(TabMoveDirection::Right, window, cx);
    }

    fn move_active_tab(
        &mut self,
        direction: TabMoveDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tab_move_mode {
            return;
        }
        let Some(source_id) = self.tabs.get(self.active_tab).map(|tab| tab.id) else {
            return;
        };
        let enabled = tab_move_preserves_pinning(&self.tabs, self.active_tab, direction);
        let Some(active_tab_index) = move_item_by_id(
            &mut self.tabs,
            source_id,
            direction,
            source_id,
            enabled,
            |tab| tab.id,
        ) else {
            return;
        };

        self.active_tab = active_tab_index;
        self.tab_overflow_selection_side = None;
        self.dismiss_tab_overflow_menus(cx);
        self.focus_active(window, cx);
    }

    pub(crate) fn reorder_tab(
        &mut self,
        tab_id: u64,
        position: TabDropPosition,
        cx: &mut Context<Self>,
    ) {
        let Some(active_tab_id) = self.tabs.get(self.active_tab).map(|tab| tab.id) else {
            return;
        };
        if !tab_drop_preserves_pinning(&self.tabs, tab_id, position) {
            return;
        }
        let Some(active_tab_index) =
            reorder_items_by_id(&mut self.tabs, tab_id, position, active_tab_id, |tab| {
                tab.id
            })
        else {
            return;
        };

        self.active_tab = active_tab_index;
        self.tab_overflow_selection_side = None;
        cx.notify();
    }

    pub(crate) fn select_overflow_tab(
        &mut self,
        action: &SelectOverflowTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = action.index;
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        // Any overflowed tab is either entirely left of the visible range (index <
        // active_tab) or entirely right of it (index > active_tab); keep the tab
        // bar anchored on the side the user picked it from.
        let Some(side_is_right) = tab_overflow_selection_side(index, self.active_tab) else {
            return;
        };
        self.active_tab = index;
        self.tab_overflow_selection_side = Some(side_is_right);
        self.dismiss_tab_overflow_menus(cx);
        self.focus_active(window, cx);
    }
}

#[cfg(test)]
#[path = "../tests/app/tabs.rs"]
mod tests;
