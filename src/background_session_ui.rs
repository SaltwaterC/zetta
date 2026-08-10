use super::*;
use crate::rename::resolve_tab_title;
use zeroize::Zeroizing;

const BACKGROUND_PROCESS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReconnectRequest {
    None,
    Immediate(usize),
    Choose,
}

fn reconnect_request(session_count: usize) -> ReconnectRequest {
    match session_count {
        0 => ReconnectRequest::None,
        1 => ReconnectRequest::Immediate(0),
        _ => ReconnectRequest::Choose,
    }
}

fn remove_exited_background_pane(
    sessions: &mut BackgroundSessionRunner<Tab>,
    pane_id: u64,
) -> Option<Vec<u64>> {
    let session_index = sessions
        .iter()
        .position(|tab| tab.pane(pane_id).is_some())?;
    let pane_count = sessions.iter().nth(session_index)?.panes.len();
    if pane_count == 1 {
        let tab = sessions.reconnect_at(session_index)?;
        return Some(tab.panes.into_iter().map(|pane| pane.id).collect());
    }

    let tab = sessions.iter_mut().nth(session_index)?;
    let layout = tab.layout.clone().without(pane_id)?;
    tab.remove_pane(pane_id);
    tab.layout = layout;
    tab.restore_focus_after_close(pane_id, tab.layout.first_pane());
    Some(vec![pane_id])
}

impl Zetta {
    pub(crate) fn detach_tab(
        &mut self,
        _: &DetachTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_tab >= self.tabs.len() {
            return;
        }
        let tab = &self.tabs[self.active_tab];
        if let Some(authentication) = tab.close_policy.background_authentication() {
            let tab_id = tab.id;
            self.detach_tab_by_id(tab_id, authentication, window, cx);
        } else {
            self.prompt_to_detach_session(tab.id, window, cx);
        }
    }

    pub(crate) fn toggle_auto_background_tab(
        &mut self,
        _: &ToggleAutoBackgroundTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let tab_id = tab.id;
        if matches!(tab.close_policy, TabClosePolicy::Background { .. }) {
            self.tabs[self.active_tab].close_policy = TabClosePolicy::Close;
            cx.notify();
        } else {
            self.prompt_to_configure_auto_background(tab_id, window, cx);
        }
    }

    pub(crate) fn detach_tab_by_id(
        &mut self,
        tab_id: u64,
        authentication: Option<SessionAuthentication>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        self.active_tab = index;
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        self.move_tab_to_background(self.active_tab, authentication, cx);

        if self.tabs.is_empty() {
            self.active_tab = 0;
            self.open_tab(window, cx);
        } else {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn move_tab_to_background(
        &mut self,
        index: usize,
        authentication: Option<SessionAuthentication>,
        cx: &mut Context<Self>,
    ) {
        let tab = self.tabs.remove(index);
        if index < self.active_tab {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() && !self.tabs.is_empty() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.disable_tab_move_mode_if_unavailable(cx);
        self.store_background_tab(tab, authentication, cx);
        self.finish_background_session_change(cx);
    }

    pub(crate) fn store_background_tab(
        &mut self,
        mut tab: Tab,
        authentication: Option<SessionAuthentication>,
        cx: &mut Context<Self>,
    ) {
        tab.rename_buffer = None;
        tab.renaming_pane = None;
        for pane in &mut tab.panes {
            pane.view = None;
            for entry in &mut pane.stack.entries {
                entry.view = None;
            }
        }
        let terminals = tab
            .panes
            .iter()
            .flat_map(|pane| {
                pane.terminal
                    .iter()
                    .map(move |terminal| (pane.id, None, terminal.clone()))
                    .chain(pane.stack.entries.iter().filter_map(move |entry| {
                        Some((pane.id, Some(entry.id), entry.terminal.clone()?))
                    }))
            })
            .collect::<Vec<_>>();
        self.background_sessions.detach(tab, authentication);
        for (pane_id, stack_id, terminal) in terminals {
            if let Some(stack_id) = stack_id {
                self.observe_background_stacked_terminal(pane_id, stack_id, terminal.clone(), cx);
            } else {
                self.observe_background_terminal(pane_id, terminal.clone(), cx);
            }
            terminal.update(cx, |terminal, cx| {
                terminal.set_ui_visible(false, cx);
                terminal.refresh_foreground_process(cx);
            });
        }
    }

    pub(crate) fn finish_background_session_change(&mut self, cx: &mut Context<Self>) {
        self.schedule_background_process_refresh(cx);
        self.publish_background_session_catalog(cx);
    }

    pub(crate) fn reconnect_session(
        &mut self,
        _: &ReconnectSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self.process_background_session_picker_entries(cx);
        match reconnect_request(entries.len()) {
            ReconnectRequest::None => {}
            ReconnectRequest::Immediate(index) => {
                let (runner_id, session_id, _, _) = &entries[index];
                self.reconnect_process_background_session(*runner_id, *session_id, window, cx);
            }
            ReconnectRequest::Choose => self.reconnect_menu_handle.show(window, cx),
        }
    }
}

impl Zetta {
    pub(crate) fn reconnect_background_session(
        &mut self,
        runner_id: u64,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reconnect_process_background_session(runner_id, session_id, window, cx);
    }

    fn reconnect_process_background_session(
        &mut self,
        runner_id: u64,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ReconnectSessionResult {
        if runner_id != self.background_sessions.runner_id() {
            let Some(source) = zetta_for_runner(runner_id, cx) else {
                return ReconnectSessionResult::SessionNotFound;
            };
            if !source
                .read(cx)
                .background_session_is_transferable(session_id)
            {
                self.pane_output_error = Some(
                    "That background session is still starting. Try attaching it again shortly."
                        .to_owned(),
                );
                cx.notify();
                return ReconnectSessionResult::StillStarting;
            }
            let verifier = source
                .read(cx)
                .background_session_authentication(session_id);
            if verifier.is_some() {
                self.prompt_to_reconnect_session(runner_id, session_id, window, cx);
                return ReconnectSessionResult::AuthenticationFailed;
            }
            let tab = source.update(cx, |source, cx| {
                source.take_background_session_by_id(session_id, None, cx)
            });
            if let Some(tab) = tab {
                prune_empty_dormant_runners(cx);
                self.attach_reconnected_tab(tab, true, window, cx);
                return ReconnectSessionResult::Reconnected;
            }
            return ReconnectSessionResult::SessionNotFound;
        }
        let Some(index) = self
            .background_sessions
            .iter()
            .position(|tab| tab.id == session_id)
        else {
            return ReconnectSessionResult::SessionNotFound;
        };
        let Some(tab) = self.background_sessions.iter().nth(index) else {
            return ReconnectSessionResult::SessionNotFound;
        };
        if self.background_sessions.authentication_at(index).is_some() {
            self.prompt_to_reconnect_session(runner_id, tab.id, window, cx);
            return ReconnectSessionResult::AuthenticationFailed;
        }
        let session_id = tab.id;
        if let Some(tab) = self.take_background_session_by_id(session_id, None, cx) {
            self.attach_reconnected_tab(tab, false, window, cx);
            return ReconnectSessionResult::Reconnected;
        }
        ReconnectSessionResult::SessionNotFound
    }

    pub(crate) fn reconnect_session_from_cli(
        &mut self,
        runner_id: u64,
        session_id: u64,
        secret: Option<String>,
        completion: std::sync::mpsc::Sender<ReconnectSessionResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let verifier = self.process_background_session_authentication(runner_id, session_id, cx);
        if verifier.is_none() {
            let result = if secret.is_none() {
                self.reconnect_process_background_session(runner_id, session_id, window, cx)
            } else {
                ReconnectSessionResult::AuthenticationFailed
            };
            let _ = completion.send(result);
            return;
        }
        let Some(secret) = secret.map(Zeroizing::new) else {
            let _ = completion.send(ReconnectSessionResult::AuthenticationFailed);
            return;
        };
        let generation = self.session_authentication_generation;
        cx.spawn_in(window, async move |this, cx| {
            let authenticated = cx
                .background_spawn(async move {
                    let verifier =
                        verifier.context("the protected session is no longer available")?;
                    Ok::<_, anyhow::Error>(verifier.verify(&secret).then_some(verifier))
                })
                .await
                .ok()
                .flatten();
            let result = this
                .update_in(cx, |this, window, cx| {
                    if this.session_authentication_generation != generation {
                        return ReconnectSessionResult::Rejected;
                    }
                    authenticated.map_or(
                        ReconnectSessionResult::AuthenticationFailed,
                        |authentication| {
                            this.complete_authenticated_reconnect(
                                runner_id,
                                session_id,
                                &authentication,
                                window,
                                cx,
                            )
                        },
                    )
                })
                .unwrap_or(ReconnectSessionResult::Rejected);
            let _ = completion.send(result);
        })
        .detach();
    }

    pub(crate) fn background_session_authentication(
        &self,
        session_id: u64,
    ) -> Option<SessionAuthentication> {
        let index = self
            .background_sessions
            .iter()
            .position(|tab| tab.id == session_id)?;
        self.background_sessions.authentication_at(index).cloned()
    }

    fn background_session_is_transferable(&self, session_id: u64) -> bool {
        self.background_sessions
            .iter()
            .find(|tab| tab.id == session_id)
            .is_some_and(|tab| {
                tab.panes.iter().all(|pane| {
                    (pane.terminal.is_some() || pane.error.is_some())
                        && pane
                            .stack
                            .entries
                            .iter()
                            .all(|entry| entry.terminal.is_some() || entry.error.is_some())
                })
            })
    }

    pub(crate) fn process_background_session_authentication(
        &self,
        runner_id: u64,
        session_id: u64,
        cx: &App,
    ) -> Option<SessionAuthentication> {
        if runner_id == self.background_sessions.runner_id() {
            return self.background_session_authentication(session_id);
        }
        zetta_for_runner(runner_id, cx)?
            .read(cx)
            .background_session_authentication(session_id)
    }

    pub(crate) fn complete_authenticated_reconnect(
        &mut self,
        runner_id: u64,
        session_id: u64,
        authorization: &SessionAuthentication,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ReconnectSessionResult {
        let tab = if runner_id == self.background_sessions.runner_id() {
            self.take_background_session_by_id(session_id, Some(authorization), cx)
        } else {
            let Some(source) = zetta_for_runner(runner_id, cx) else {
                return ReconnectSessionResult::SessionNotFound;
            };
            if !source
                .read(cx)
                .background_session_is_transferable(session_id)
            {
                self.pane_output_error = Some(
                    "That background session is still starting. Try attaching it again shortly."
                        .to_owned(),
                );
                cx.notify();
                return ReconnectSessionResult::StillStarting;
            }
            let tab = source.update(cx, |source, cx| {
                source.take_background_session_by_id(session_id, Some(authorization), cx)
            });
            prune_empty_dormant_runners(cx);
            tab
        };
        if let Some(tab) = tab {
            let transferred = runner_id != self.background_sessions.runner_id();
            self.attach_reconnected_tab(tab, transferred, window, cx);
            return ReconnectSessionResult::Reconnected;
        }
        ReconnectSessionResult::SessionNotFound
    }

    pub(crate) fn take_background_session_by_id(
        &mut self,
        session_id: u64,
        authorization: Option<&SessionAuthentication>,
        cx: &mut Context<Self>,
    ) -> Option<Tab> {
        let index = self
            .background_sessions
            .iter()
            .position(|tab| tab.id == session_id)?;
        match (
            self.background_sessions.authentication_at(index),
            authorization,
        ) {
            (None, None) => {}
            (Some(expected), Some(supplied)) if expected.is_same_verifier(supplied) => {}
            _ => return None,
        }
        let tab = self.background_sessions.reconnect_at(index)?;
        self.publish_background_session_catalog(cx);
        Some(tab)
    }

    pub(crate) fn attach_reconnected_tab(
        &mut self,
        mut tab: Tab,
        transferred: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if transferred {
            let tab_id = self.next_tab_id;
            self.next_tab_id += 1;
            tab.reassign_ids(tab_id, &mut self.next_pane_id);
        }
        self.next_attention_id = self
            .next_attention_id
            .max(tab.attention_id.saturating_add(1));
        if cx.has_global::<ZettaProcessState>() {
            let process = cx.global_mut::<ZettaProcessState>();
            process.next_attention_id = process
                .next_attention_id
                .max(tab.attention_id.saturating_add(1));
        }
        let tab_id = tab.id;
        let panes = tab
            .panes
            .iter()
            .flat_map(|pane| {
                let base = pane.terminal.clone().map(|terminal| {
                    (
                        pane.id,
                        None,
                        terminal,
                        resolve_profile_theme(&pane.profile, cx),
                    )
                });
                let stacked = pane.stack.entries.iter().filter_map(|entry| {
                    Some((
                        pane.id,
                        Some(entry.id),
                        entry.terminal.clone()?,
                        resolve_profile_theme(&entry.profile, cx),
                    ))
                });
                base.into_iter().chain(stacked)
            })
            .collect::<Vec<_>>();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;

        for (pane_id, stack_id, terminal, terminal_theme) in panes {
            match terminal_theme {
                Ok(theme) => {
                    let view = cx.new(|cx| {
                        TerminalView::new_with_theme(terminal.clone(), theme, window, cx)
                    });
                    if let Some(entry_id) = stack_id {
                        self.connect_stacked_terminal_view(
                            tab_id, pane_id, entry_id, view, window, cx,
                        );
                    } else {
                        self.connect_terminal_view(tab_id, pane_id, view, window, cx);
                    }
                }
                Err(error) => {
                    if let Some(entry_id) = stack_id {
                        if let Some(entry) =
                            self.tabs[self.active_tab]
                                .pane_mut(pane_id)
                                .and_then(|pane| {
                                    pane.stack
                                        .entries
                                        .iter_mut()
                                        .find(|entry| entry.id == entry_id)
                                })
                        {
                            entry.error =
                                Some(format!("Could not reattach terminal view: {error:#}"));
                            entry.state = StackedPaneState::Failed;
                        }
                    } else if let Some(pane) = self.tabs[self.active_tab].pane_mut(pane_id) {
                        pane.error = Some(format!("Could not reattach terminal view: {error:#}"));
                    }
                }
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn process_background_session_picker_entries(
        &self,
        cx: &App,
    ) -> Arc<[ProcessBackgroundSessionEntry]> {
        if cx.has_global::<ZettaProcessState>() {
            return cx
                .global::<ZettaProcessState>()
                .background_session_entries
                .clone();
        }
        let runner_id = self.background_sessions.runner_id();
        self.background_session_picker_entries
            .iter()
            .map(|(session_id, title, details)| {
                (runner_id, *session_id, title.clone(), details.clone())
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn picker_entries_from_summaries(
        sessions: &[BackgroundSessionSummary],
    ) -> Vec<(u64, String, String)> {
        sessions
            .iter()
            .rev()
            .map(|session| {
                if session.authentication_required {
                    return (
                        session.id,
                        "Protected session".to_owned(),
                        format!("Session {} · protected", session.id),
                    );
                }
                let mut applications = Vec::new();
                for pane in &session.panes {
                    if !applications.contains(&pane.application) {
                        applications.push(pane.application.clone());
                    }
                }
                let pane_count = session.panes.len();
                let mut details = format!(
                    "Session {} · {pane_count} pane{}",
                    session.id,
                    if pane_count == 1 { "" } else { "s" }
                );
                if !applications.is_empty() {
                    details.push_str(" · ");
                    details.push_str(&applications.join(", "));
                }
                (session.id, session.title.clone(), details)
            })
            .collect()
    }

    pub(crate) fn observe_background_terminal(
        &mut self,
        pane_id: u64,
        terminal: Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        if !self.background_observed_panes.insert(pane_id) {
            return;
        }
        cx.subscribe(
            &terminal,
            move |this, _, event: &TerminalEvent, cx| match event {
                TerminalEvent::TitleChanged => this.publish_background_session_catalog(cx),
                TerminalEvent::CloseTerminal
                    if !this.retain_background_stacked_entries_after_base_exit(pane_id, cx) =>
                {
                    this.reap_background_pane(pane_id, cx);
                }
                _ => {}
            },
        )
        .detach();
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
        cx.subscribe(
            &terminal,
            move |this, _, event: &TerminalEvent, cx| match event {
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
                        this.background_observed_panes.remove(&entry_id);
                        this.publish_background_session_catalog(cx);
                    }
                }
                _ => {}
            },
        )
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

    fn schedule_background_process_refresh(&mut self, cx: &mut Context<Self>) {
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

    fn background_session_summary(
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
                let state = if pane.error.is_some() {
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

    fn connect_terminal_view(
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
        terminal.update(cx, |terminal, cx| terminal.set_ui_visible(visible, cx));

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
                TerminalViewEvent::TitleChanged => cx.notify(),
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
            pane.base_exited = false;
        }
    }

    fn connect_stacked_terminal_view(
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
        cx.subscribe_in(
            &terminal,
            window,
            move |this, terminal, event: &TerminalEvent, _window, cx| match event {
                TerminalEvent::TaskFinished { exit_code } => {
                    this.stacked_task_finished(tab_id, pane_id, entry_id, *exit_code, cx);
                }
                TerminalEvent::ResizeRequested { .. } => {
                    terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
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

#[cfg(test)]
#[path = "tests/background_session_ui.rs"]
mod tests;
