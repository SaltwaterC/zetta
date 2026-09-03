//! What a window watches on a background pane: terminal events, exits, the
//! process refresh, and the catalog it publishes for other processes to list.
//!
//! These run for the life of a pane rather than at a transition, which is why
//! they are kept apart from the detach and reconnect paths that start them.

use super::*;

impl Zetta {
    pub(crate) fn observe_background_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        terminal: Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        if !self.background_observed_panes.insert(pane_id) {
            return;
        }
        let run_identity = self.run_pane_identity(tab_id, pane_id);
        let run_registry = crate::run_command::process_run_registry();
        cx.subscribe(&terminal, move |this, _, event: &TerminalEvent, cx| {
            if let Some(identity) = run_identity {
                match event {
                    TerminalEvent::TrackingReady => run_registry.tracking_ready(identity),
                    TerminalEvent::CommandStarted { command } => {
                        run_registry.command_started(identity, command.clone())
                    }
                    TerminalEvent::CommandFinished { exit_code } => {
                        run_registry.command_finished(identity, *exit_code)
                    }
                    TerminalEvent::TerminalExited(_) => run_registry.terminal_lost(identity),
                    _ => {}
                }
            }
            match event {
                TerminalEvent::CommandStarted { command } => this.update_active_command(
                    tab_id,
                    pane_id,
                    crate::session_state::valid_restore_command(command),
                ),
                TerminalEvent::CommandFinished { .. } | TerminalEvent::TerminalExited(_) => {
                    this.update_active_command(tab_id, pane_id, None)
                }
                _ => {}
            }
            match event {
                TerminalEvent::TerminalExited(exit)
                    if exit.is_unexpected()
                        && this.retain_unexpected_terminal_exit(tab_id, pane_id, exit, cx) =>
                {
                    this.publish_background_session_catalog(cx);
                }
                event if terminal_event_requires_worktree_detection(event) => {
                    this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                    this.publish_background_session_catalog(cx);
                }
                TerminalEvent::CloseTerminal
                    if !this.retain_background_stacked_entries_after_base_exit(pane_id, cx) =>
                {
                    this.reap_background_pane(pane_id, cx);
                }
                _ => {}
            }
        })
        .detach();
    }

    /// Keeps the durable pane model in step with shell lifecycle markers.
    /// Foreground process argv is intentionally not used here: it can describe
    /// an editor, interpreter, or helper rather than text the user entered.
    pub(crate) fn update_active_command(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        command: Option<String>,
    ) {
        if let Some(pane) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.active_command = command;
            return;
        }
        if let Some(pane) = self
            .background_sessions
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.active_command = command;
        }
    }

    fn retain_background_stacked_entries_after_base_exit(
        &mut self,
        pane_id: u64,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self
            .background_sessions
            .iter_mut()
            .find_map(|tab| tab.pane_mut(pane_id))
        else {
            return false;
        };
        if pane.stack.is_empty() {
            return false;
        }

        pane.terminal = None;
        pane.base_exited = true;
        pane.stack.select_after_base_exit();
        self.background_observed_panes.remove(&pane_id);
        self.publish_background_session_catalog(cx);
        true
    }

    pub(crate) fn observe_background_stacked_terminal(
        &mut self,
        pane_id: u64,
        entry_id: u64,
        terminal: Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        if !self.background_observed_panes.insert(entry_id) {
            return;
        }
        let run_identity = self.background_sessions.iter().find_map(|tab| {
            let entry = tab
                .pane(pane_id)?
                .stack
                .entries
                .iter()
                .find(|entry| entry.id == entry_id)?;
            Some(crate::run_command::RunPaneIdentity::new(
                tab.attention_id,
                entry.routing_id,
            ))
        });
        let run_registry = crate::run_command::process_run_registry();
        cx.subscribe(&terminal, move |this, _, event: &TerminalEvent, cx| {
            if let Some(identity) = run_identity {
                match event {
                    TerminalEvent::TrackingReady => run_registry.tracking_ready(identity),
                    TerminalEvent::CommandStarted { command } => {
                        run_registry.command_started(identity, command.clone())
                    }
                    TerminalEvent::CommandFinished { exit_code } => {
                        run_registry.command_finished(identity, *exit_code)
                    }
                    TerminalEvent::TerminalExited(_) => run_registry.terminal_lost(identity),
                    _ => {}
                }
            }
            match event {
                TerminalEvent::TaskFinished { exit_code } => {
                    let Some(tab_id) = this
                        .background_sessions
                        .iter()
                        .find(|tab| {
                            tab.pane(pane_id).is_some_and(|pane| {
                                pane.stack.entries.iter().any(|entry| entry.id == entry_id)
                            })
                        })
                        .map(|tab| tab.id)
                    else {
                        return;
                    };
                    this.stacked_task_finished(tab_id, pane_id, entry_id, *exit_code, cx);
                }
                TerminalEvent::CloseTerminal => {
                    let Some(tab_id) = this
                        .background_sessions
                        .iter()
                        .find(|tab| {
                            tab.pane(pane_id).is_some_and(|pane| {
                                pane.stack.entries.iter().any(|entry| entry.id == entry_id)
                            })
                        })
                        .map(|tab| tab.id)
                    else {
                        return;
                    };
                    let removed = this
                        .background_sessions
                        .iter_mut()
                        .find(|tab| tab.id == tab_id)
                        .and_then(|tab| tab.pane_mut(pane_id))
                        .and_then(|pane| pane.stack.remove(entry_id));
                    if removed.is_some() {
                        if let Some(identity) = run_identity {
                            run_registry.pane_closed(identity);
                        }
                        this.background_observed_panes.remove(&entry_id);
                        this.publish_background_session_catalog(cx);
                    }
                }
                _ => {}
            }
        })
        .detach();
    }

    fn reap_background_pane(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let Some(removed_pane_ids) =
            remove_exited_background_pane(&mut self.background_sessions, pane_id)
        else {
            return;
        };
        for pane_id in removed_pane_ids {
            self.background_observed_panes.remove(&pane_id);
        }
        self.publish_background_session_catalog(cx);
        if self.background_sessions.is_empty() {
            cx.defer(prune_empty_dormant_runners);
        }
    }

    pub(super) fn schedule_background_process_refresh(&mut self, cx: &mut Context<Self>) {
        if self.background_process_refresh_running || self.background_sessions.is_empty() {
            return;
        }
        self.background_process_refresh_running = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(BACKGROUND_PROCESS_REFRESH_INTERVAL).await;
                let keep_refreshing = this
                    .update(cx, |this, cx| {
                        if this.background_sessions.is_empty() {
                            this.background_process_refresh_running = false;
                            return false;
                        }
                        for terminal in this
                            .background_sessions
                            .iter()
                            .flat_map(|tab| tab.panes.iter())
                            .flat_map(|pane| {
                                pane.terminal.iter().cloned().chain(
                                    pane.stack
                                        .entries
                                        .iter()
                                        .filter_map(|entry| entry.terminal.clone()),
                                )
                            })
                        {
                            terminal.update(cx, Terminal::refresh_foreground_process);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_refreshing {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn publish_background_session_catalog(&mut self, cx: &mut Context<Self>) {
        let sessions = self
            .background_sessions
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                self.background_session_summary(
                    tab,
                    self.background_sessions.authentication_at(index).is_some(),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        self.background_session_picker_entries = Self::picker_entries_from_summaries(&sessions);
        if let Err(error) = self.background_sessions.publish(sessions) {
            eprintln!("Could not publish background session catalog: {error:#}");
        }
        cx.defer(refresh_process_background_sessions);
    }

    pub(super) fn background_session_summary(
        &self,
        tab: &Tab,
        authentication_required: bool,
        cx: &App,
    ) -> BackgroundSessionSummary {
        let title = self.background_session_title(tab, cx);
        let panes = tab
            .panes
            .iter()
            .map(|pane| {
                let (terminal_title, foreground_command) = pane
                    .terminal
                    .as_ref()
                    .map(|terminal| {
                        let terminal = terminal.read(cx);
                        (
                            Some(terminal.title(false)),
                            terminal.foreground_process_command_line(),
                        )
                    })
                    .unwrap_or_default();
                let working_directory = pane.working_directory(cx);
                let state = if pane.error.is_some() || pane.exit.is_some() {
                    BackgroundPaneState::Failed
                } else if pane.terminal.is_some() {
                    BackgroundPaneState::Running
                } else {
                    BackgroundPaneState::Starting
                };
                let (program, arguments) = pane.profile.command.program_and_args();
                let configured_command = std::iter::once(program)
                    .chain(arguments.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" ");
                let application = application_from_command_line(foreground_command.as_deref())
                    .unwrap_or_else(|| {
                        pane.generated_label
                            .as_deref()
                            .and_then(|label| {
                                if label.starts_with("HTTP: ") {
                                    Some("Zetta HTTP server")
                                } else if label.starts_with("TFTP: ") {
                                    Some("Zetta TFTP server")
                                } else if label.starts_with("Serial: ") {
                                    Some("Serial console")
                                } else {
                                    None
                                }
                            })
                            .map(str::to_owned)
                            .unwrap_or_else(|| pane.profile.command.program_and_args().0)
                    });
                BackgroundPaneSummary {
                    id: pane.id,
                    label: pane.label(),
                    profile: pane.profile.name.clone(),
                    configured_command,
                    application,
                    foreground_command,
                    terminal_title,
                    working_directory,
                    state,
                    exit: pane.exit.clone(),
                }
            })
            .collect();
        BackgroundSessionSummary {
            id: tab.id,
            title,
            authentication_required,
            active_pane: tab.active_pane,
            layout: background_pane_layout(&tab.layout),
            panes,
            held: false,
            // The multiplexer decides whose a session is; a client describing
            // one only says what it contains.
            scoped_to: None,
            // As with the scope: the multiplexer publishes the sealed key from
            // the protection it was given, so a summary never carries one.
            key_envelope: None,
        }
    }

    fn background_session_title(&self, tab: &Tab, cx: &App) -> String {
        resolve_tab_title(tab, || {
            tab.active_terminal()
                .map(|terminal| terminal.read(cx).title(false).into())
                .unwrap_or_else(|| format!("Tab {}", tab.id).into())
        })
        .to_string()
    }

    pub(super) fn connect_terminal_view(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.configure_terminal_view_silent_mode(tab_id, &view, cx);
        let visible = self.tabs.get(self.active_tab).is_some_and(|tab| {
            tab.id == tab_id
                && tab.pane_is_visible(pane_id)
                && tab
                    .pane(pane_id)
                    .is_some_and(|pane| pane.stack.selected_is_base())
        });
        let terminal = view.read(cx).terminal().clone();
        let display_only = !terminal.read(cx).is_pty();
        terminal.update(cx, |terminal, cx| terminal.set_ui_visible(visible, cx));
        let run_identity = self.run_pane_identity(tab_id, pane_id);
        let run_registry = crate::run_command::process_run_registry();
        if let Some(identity) = run_identity {
            run_registry.pane_reopened(identity);
        }
        if self.shared_panes.contains_key(&pane_id) {
            self.subscribe_shared_pane_size(pane_id, &terminal, window, cx);
        }

        cx.subscribe_in(
            &terminal,
            window,
            move |this, _, event: &TerminalEvent, _, _| {
                if let TerminalEvent::CommandStarted { command } = event {
                    this.update_active_command(
                        tab_id,
                        pane_id,
                        crate::session_state::valid_restore_command(command),
                    );
                } else if matches!(
                    event,
                    TerminalEvent::CommandFinished { .. } | TerminalEvent::TerminalExited(_)
                ) {
                    this.update_active_command(tab_id, pane_id, None);
                }
                let Some(identity) = run_identity else {
                    return;
                };
                match event {
                    TerminalEvent::TrackingReady => run_registry.tracking_ready(identity),
                    TerminalEvent::CommandStarted { command } => {
                        run_registry.command_started(identity, command.clone())
                    }
                    TerminalEvent::CommandFinished { exit_code } => {
                        run_registry.command_finished(identity, *exit_code)
                    }
                    TerminalEvent::TerminalExited(_) => run_registry.terminal_lost(identity),
                    _ => {}
                }
            },
        )
        .detach();

        let pane_label = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.generated_label.as_deref());
        let is_http_server = cfg!(feature = "http-server")
            && pane_label.is_some_and(|label| label.starts_with("HTTP: "));
        let is_tftp_server = cfg!(feature = "tftp-server")
            && pane_label.is_some_and(|label| label.starts_with("TFTP: "));
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => this.terminal_closed(tab_id, pane_id, window, cx),
                TerminalViewEvent::TitleChanged => {
                    this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                    this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                    cx.notify();
                }
                TerminalViewEvent::Input(input)
                    if server_input_stops_server(input, is_http_server, is_tftp_server) =>
                {
                    this.terminal_closed(tab_id, pane_id, window, cx);
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
        let focus_handle = view.focus_handle(cx);
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
        let emit_input_events = is_http_server
            || is_tftp_server
            || self
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_some_and(|tab| tab.broadcast_input);
        view.update(cx, |view, _| view.set_emit_input_events(emit_input_events));
        if let Some(pane) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.view = Some(view);
            pane.error = None;
            pane.exit = None;
            if !display_only {
                pane.base_exited = false;
            }
        }
        self.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
        self.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
    }

    pub(super) fn connect_stacked_terminal_view(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.configure_terminal_view_silent_mode(tab_id, &view, cx);
        let visible = self.tabs.get(self.active_tab).is_some_and(|tab| {
            tab.id == tab_id
                && tab.pane_is_visible(pane_id)
                && tab.pane(pane_id).is_some_and(|pane| {
                    pane.stack.selected == PaneStackSelection::Stacked(entry_id)
                })
        });
        let terminal = view.read(cx).terminal().clone();
        terminal.update(cx, |terminal, cx| terminal.set_ui_visible(visible, cx));
        let run_identity = self.run_stacked_pane_identity(tab_id, pane_id, entry_id);
        let run_registry = crate::run_command::process_run_registry();
        if let Some(identity) = run_identity {
            run_registry.pane_reopened(identity);
        }
        cx.subscribe_in(
            &terminal,
            window,
            move |this, terminal, event: &TerminalEvent, _window, cx| {
                if let Some(identity) = run_identity {
                    match event {
                        TerminalEvent::TrackingReady => run_registry.tracking_ready(identity),
                        TerminalEvent::CommandStarted { command } => {
                            run_registry.command_started(identity, command.clone())
                        }
                        TerminalEvent::CommandFinished { exit_code } => {
                            run_registry.command_finished(identity, *exit_code)
                        }
                        TerminalEvent::TerminalExited(_) => run_registry.terminal_lost(identity),
                        _ => {}
                    }
                }
                match event {
                    TerminalEvent::TaskFinished { exit_code } => {
                        this.stacked_task_finished(tab_id, pane_id, entry_id, *exit_code, cx);
                    }
                    TerminalEvent::ResizeRequested { .. } => {
                        terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
                    }
                    _ => {}
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => {
                    this.stacked_terminal_closed(tab_id, pane_id, entry_id, window, cx);
                }
                TerminalViewEvent::TitleChanged => cx.notify(),
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
            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.activate_stack_entry(pane_id, PaneStackSelection::Stacked(entry_id));
                cx.notify();
            }
            this.activate_current_project(window, cx);
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
        if let Some(entry) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
            .and_then(|pane| {
                pane.stack
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == entry_id)
            })
        {
            entry.view = Some(view);
        }
    }
}

#[inline]
fn server_input_stops_server(
    input: &TerminalInput,
    is_http_server: bool,
    is_tftp_server: bool,
) -> bool {
    (is_http_server || is_tftp_server) && byte_stream_pane::ctrl_c_interrupts_byte_stream(input)
}
