//! Splitting, closing, and focusing the panes inside a tab, and what a
//! terminal's exit does to the pane that held it.
//!
//! The layout itself lives in `pane.rs`; these are the window-level actions
//! that reshape it, plus the exit handling that decides whether a pane closes
//! or stays open showing why its command failed.

use super::*;

impl Zetta {
    pub(crate) fn close_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_pane_with_policy(tab_id, pane_id, true, window, cx);
    }

    pub(crate) fn terminal_closed(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(identity) = self.run_pane_identity(tab_id, pane_id) {
            crate::run_command::process_run_registry().pane_closed(identity);
        }
        if self.retain_stacked_entries_after_base_exit(tab_id, pane_id, window, cx) {
            return;
        }
        self.close_pane_with_policy(tab_id, pane_id, false, window, cx);
    }

    /// Retains an interactive terminal whose exit cannot be trusted as an
    /// ordinary user close. The terminal entity is kept for reconnect, while
    /// its view is replaced by the sanitized diagnostic pane.
    pub(crate) fn retain_unexpected_terminal_exit(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        exit: &TerminalExited,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(exit_info) = background_pane_exit_from_terminal(exit) else {
            return false;
        };
        let mut profile_name = None;
        let mut terminal = None;
        let mut updated = false;

        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            if let Some(pane) = tab.pane_mut(pane_id)
                && pane.exit.is_none()
            {
                profile_name = Some(pane.profile.name.clone());
                terminal = pane.terminal.clone();
                pane.view = None;
                pane.error = Some(exit_info.reason_text());
                pane.exit = Some(exit_info.clone());
                pane.pending_command = None;
                updated = true;
            }
        } else if let Some(tab) = self
            .background_sessions
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            && let Some(pane) = tab.pane_mut(pane_id)
            && pane.exit.is_none()
        {
            profile_name = Some(pane.profile.name.clone());
            terminal = pane.terminal.clone();
            pane.view = None;
            pane.error = Some(exit_info.reason_text());
            pane.exit = Some(exit_info.clone());
            pane.pending_command = None;
            updated = true;
        }

        if !updated {
            return false;
        }

        if let Some(terminal) = terminal {
            terminal.update(cx, |terminal, cx| terminal.set_ui_visible(false, cx));
        }
        let profile_name = profile_name
            .as_deref()
            .map_or_else(|| "<unknown>".to_owned(), Self::sanitize_exit_context);
        log::warn!(
            "unexpected terminal exit: profile={:?} pane_id={} session_id={} child_pid={:?} source={:?} exit_code={:?} input_sent={} foreground_command={:?}",
            profile_name,
            pane_id,
            tab_id,
            exit.child_pid,
            exit.source,
            exit.exit_code,
            exit.input_sent,
            exit_info.foreground_command,
        );
        cx.notify();
        true
    }

    fn sanitize_exit_context(value: &str) -> String {
        let mut sanitized = value
            .chars()
            .filter(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ' ')
            })
            .take(64)
            .collect::<String>();
        if sanitized.trim().is_empty() {
            sanitized = "<unnamed>".to_owned();
        }
        sanitized
    }

    /// A host shell can exit while command PTYs in its stack are still alive.
    /// Keep the host region and those entries in that case; the base entry is
    /// marked as exited and selection moves to the first stacked entry when
    /// the base terminal was foreground.
    fn retain_stacked_entries_after_base_exit(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        else {
            return false;
        };
        if pane.stack.is_empty() {
            return false;
        }

        pane.terminal = None;
        pane.view = None;
        pane.error = None;
        pane.exit = None;
        pane.base_exited = true;
        pane.pending_command = None;
        pane.stack.select_after_base_exit();
        self.retain_open_visible_terminals();
        self.focus_active(window, cx);
        self.sync_visible_terminals(cx);
        cx.notify();
        true
    }

    fn close_pane_with_policy(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        background_if_last_pane: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        if !self.tabs[tab_index]
            .panes
            .iter()
            .any(|pane| pane.id == pane_id)
        {
            return;
        }
        if self.tabs[tab_index].panes.len() == 1 {
            self.close_tab_at_with_policy(tab_index, background_if_last_pane, window, cx);
            return;
        }

        // Closing a pane changes the dimensions of the survivors. Reflowing millions of retained
        // scrollback rows synchronously during the next paint can freeze the entire application.
        // A layout-driven resize only needs to truncate/grow rows; the shells redraw their live
        // prompts after receiving SIGWINCH.
        let surviving_terminals = self.tabs[tab_index]
            .panes
            .iter()
            .filter(|pane| pane.id != pane_id)
            .flat_map(TerminalPane::all_terminals)
            .cloned()
            .collect::<Vec<_>>();
        for terminal in surviving_terminals {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }

        self.cancel_tab_search_for_tab(tab_id, cx);
        if let Some(identity) = self.run_pane_identity(tab_id, pane_id) {
            crate::run_command::process_run_registry().pane_closed(identity);
        }
        let layout = {
            let tab = &mut self.tabs[tab_index];
            tab.remove_pane(pane_id);
            tab.layout.clone().without(pane_id)
        };
        self.projects.forget_pane(pane_id);
        self.forget_pane_controls([pane_id]);
        self.drop_shared_pane(pane_id);
        self.release_mux_pane(tab_id, pane_id, cx);
        self.retain_open_visible_terminals();
        let Some(layout) = layout else {
            self.close_tab_at_with_policy(tab_index, background_if_last_pane, window, cx);
            return;
        };
        let tab = &mut self.tabs[tab_index];
        tab.layout = layout;
        tab.restore_focus_after_close(pane_id, tab.layout.first_pane());
        self.active_tab = tab_index;
        self.focus_active(window, cx);
    }

    /// Release render-cache references to terminals removed from a tab or pane immediately.
    ///
    /// Rendering normally refreshes this cache on the next frame, but retaining a closed
    /// terminal until then also retains its scrollback and delays its background reclamation.
    pub(crate) fn retain_open_visible_terminals(&mut self) {
        let open_terminals = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .flat_map(|pane| {
                pane.terminal.iter().chain(
                    pane.stack
                        .entries
                        .iter()
                        .filter_map(|entry| entry.terminal.as_ref()),
                )
            })
            .map(Entity::entity_id)
            .collect::<HashSet<_>>();
        self.visible_terminals
            .retain(|terminal| open_terminals.contains(&terminal.entity_id()));
    }

    /// Reconciles the render cache of visible terminals with the active tab's layout.
    ///
    /// Hidden terminals keep parsing PTY output and retaining scrollback, but they must not
    /// continually enqueue work on the foreground executor. A newly visible terminal emits
    /// one consolidated wakeup to render everything produced while it was hidden.
    pub(crate) fn sync_visible_terminals(&mut self, cx: &mut Context<Self>) {
        let visible_terminals = self
            .tabs
            .get(self.active_tab)
            .into_iter()
            .flat_map(|tab| {
                tab.panes.iter().filter_map(|pane| {
                    (tab.pane_is_visible(pane.id)
                        && (!pane.stack.selected_is_base() || pane.exit.is_none()))
                    .then(|| pane.selected_terminal())
                    .flatten()
                })
            })
            .collect::<Vec<_>>();
        for terminal in &self.visible_terminals {
            if !visible_terminals
                .iter()
                .any(|visible| visible.entity_id() == terminal.entity_id())
            {
                terminal.update(cx, |terminal, cx| terminal.set_ui_visible(false, cx));
            }
        }
        for terminal in &visible_terminals {
            if !self
                .visible_terminals
                .iter()
                .any(|visible| visible.entity_id() == terminal.entity_id())
            {
                terminal.update(cx, |terminal, cx| terminal.set_ui_visible(true, cx));
            }
        }
        self.visible_terminals = visible_terminals;
    }

    pub(crate) fn split_active_pane(
        &mut self,
        axis: SplitAxis,
        position: SplitPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        self.split_pane_with_pending_command(
            tab.id,
            tab.active_pane,
            None,
            axis,
            position,
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn split_pane_with_pending_command(
        &mut self,
        tab_id: u64,
        active_pane_id: u64,
        pending_command: Option<String>,
        axis: SplitAxis,
        position: SplitPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        let tab = &self.tabs[tab_index];
        if !can_add_panes(tab.panes.len(), 1) {
            return false;
        }
        let active_pane = tab.pane(active_pane_id);
        let effective_config = self.effective_config();
        let inherit_working_directory = effective_config
            .working_directory_scope
            .inherits_for_new_pane();
        let working_directory_configured = effective_config.working_directory_configured;
        let pane_controls_hidden_by_default = effective_config.pane_controls_hidden_by_default;
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
        let Some(profile) = active_pane.map(|pane| pane.profile.clone()) else {
            return false;
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
        let terminals_resized_by_split = matches!(axis, SplitAxis::Vertical)
            .then(|| {
                tab.panes
                    .iter()
                    .flat_map(TerminalPane::all_terminals)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let wsl_cwd_file = wsl_cwd_tracking_file(&profile, pane_id);
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                pane_controls_hidden_by_default,
                [pane_id],
            ));

        // A vertical split changes terminal widths. Reflowing a large retained buffer during the
        // next paint blocks the UI before the new pane can appear. Preserve logical rows for this
        // layout-driven resize; each shell will redraw its live prompt after SIGWINCH.
        for terminal in terminals_resized_by_split {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }

        self.projects.inherit_pane_root(active_pane_id, pane_id);
        let tab = &mut self.tabs[tab_index];
        tab.maximized_pane = None;
        if !tab.layout.split(active_pane_id, axis, pane_id, position) {
            return false;
        }
        self.active_tab = tab_index;
        tab.push_pane(
            TerminalPane::new(pane_id, profile.clone())
                .with_wsl_cwd_file(wsl_cwd_file.clone())
                .with_pending_command(pending_command),
        );
        tab.activate_pane(pane_id);
        self.spawn_terminal_for_pane(
            TerminalSpawnRequest {
                working_directory,
                wsl_directory,
                wsl_cwd_file,
                ..TerminalSpawnRequest::new(tab_id, pane_id, profile)
            },
            window,
            cx,
        );
        self.focus_active(window, cx);
        cx.notify();
        true
    }

    pub(crate) fn open_editor_in_new_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        request: terminal_view::EditorRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opened = self.split_pane_with_pending_command(
            tab_id,
            pane_id,
            Some(request.command),
            SplitAxis::Vertical,
            SplitPosition::After,
            window,
            cx,
        );
        if !opened && let Some(path) = request.temporary_path {
            terminal_view::remove_scrollback_file(&path);
        }
    }
}

impl Zetta {
    pub(crate) fn close_active_pane(
        &mut self,
        _: &ClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let selection = tab
            .active_pane()
            .map(|pane| pane.focused_stack_selection(window, cx));
        match selection {
            Some(PaneStackSelection::Stacked(entry_id)) => {
                self.close_stacked_pane_by_id(tab.id, tab.active_pane, entry_id, window, cx);
            }
            _ if tab.panes.len() == 1 && tab.pinned => {
                self.prompt_to_confirm_tab_close(tab.id, window, cx);
            }
            _ => self.close_pane(tab.id, tab.active_pane, window, cx),
        }
    }

    pub(crate) fn save_pane_output(
        &mut self,
        _: &SavePaneOutput,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.tabs.get(self.active_tab).and_then(Tab::active_pane) else {
            return;
        };
        let Some(view) = pane.selected_view() else {
            return;
        };
        let is_wsl = is_wsl_shell(&pane.profile.command);
        if !begin_pane_output_save(&mut self.pane_output_save_in_progress) {
            return;
        }

        let terminal = view.read(cx).terminal().clone();
        let output = terminal.read(cx).get_content_async();
        let directory = (!is_wsl)
            .then(|| pane.working_directory(cx))
            .flatten()
            .or_else(|| env::current_dir().ok())
            .unwrap_or_default();

        self.pane_output_error = None;
        let path = cx.prompt_for_new_path(&directory, Some(PANE_OUTPUT_DEFAULT_FILENAME));
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result: Result<()> = async {
                let output = output.await;
                let path = path
                    .await
                    .context("the save dialog closed unexpectedly")?
                    .context("opening the save dialog")?;
                let Some(path) = path else {
                    return Ok(());
                };
                executor
                    .spawn(async move {
                        fs::write(&path, output)
                            .with_context(|| format!("writing pane output to {}", path.display()))
                    })
                    .await
            }
            .await;
            this.update(cx, |this, cx| {
                finish_pane_output_save(&mut this.pane_output_save_in_progress);
                this.pane_output_error = result
                    .err()
                    .map(|error| format!("Could not save pane output: {error:#}"));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn split_horizontal_down(
        &mut self,
        _: &SplitHorizontalDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_active_pane(SplitAxis::Horizontal, SplitPosition::After, window, cx);
    }

    pub(crate) fn split_horizontal_up(
        &mut self,
        _: &SplitHorizontalUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_active_pane(SplitAxis::Horizontal, SplitPosition::Before, window, cx);
    }

    pub(crate) fn split_vertical_right(
        &mut self,
        _: &SplitVerticalRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_active_pane(SplitAxis::Vertical, SplitPosition::After, window, cx);
    }

    pub(crate) fn split_vertical_left(
        &mut self,
        _: &SplitVerticalLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_active_pane(SplitAxis::Vertical, SplitPosition::Before, window, cx);
    }

    pub(crate) fn rotate_pane_layout(
        &mut self,
        _: &RotatePaneLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rotate_pane_layout_in_direction(PaneRotationDirection::Clockwise, window, cx);
    }

    pub(crate) fn rotate_pane_layout_counter_clockwise(
        &mut self,
        _: &RotatePaneLayoutCounterClockwise,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rotate_pane_layout_in_direction(PaneRotationDirection::CounterClockwise, window, cx);
    }

    fn rotate_pane_layout_in_direction(
        &mut self,
        direction: PaneRotationDirection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if !tab.layout.rotate_pane(tab.active_pane, direction) {
            return;
        }
        for terminal in tab.panes.iter().flat_map(TerminalPane::all_terminals) {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }
        cx.notify();
    }
}

impl Zetta {
    pub(crate) fn broadcast_input(
        &mut self,
        tab_id: u64,
        source_pane_id: u64,
        input: &TerminalInput,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };
        if !tab.broadcast_input || tab.active_pane != source_pane_id {
            return;
        }
        let sibling_views = tab
            .panes
            .iter()
            .filter(|pane| pane.id != source_pane_id)
            .filter_map(|pane| pane.view.clone())
            .collect::<Vec<_>>();
        for view in sibling_views {
            view.update(cx, |view, cx| view.apply_input(input, cx));
        }
    }

    pub(crate) fn toggle_broadcast_input(
        &mut self,
        _: &ToggleBroadcastInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.broadcast_input = !tab.broadcast_input;
            let enabled = tab.broadcast_input;
            let views = tab
                .panes
                .iter()
                .filter_map(|pane| pane.view.clone())
                .collect::<Vec<_>>();
            for view in views {
                view.update(cx, |view, _| view.set_emit_input_events(enabled));
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn focus_pane(
        &mut self,
        direction: PaneDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.maximized_pane.is_some() {
            return;
        }
        let Some(pane_id) = tab.visible_layout().and_then(|layout| {
            layout.adjacent_pane(tab.active_pane, direction, &tab.focus_history)
        }) else {
            return;
        };
        tab.activate_pane(pane_id);
        self.focus_active(window, cx);
    }
}
