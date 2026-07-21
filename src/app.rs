use super::*;

const PANE_CONTROLS_IDLE_DELAY: Duration = Duration::from_millis(1200);
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

fn background_authentication_for_close(
    policy: &TabClosePolicy,
    background_if_pinned: bool,
) -> Option<Option<SessionAuthentication>> {
    if background_if_pinned {
        policy.background_authentication()
    } else {
        None
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

fn pane_controls_hide_delay(last_motion: Instant, now: Instant) -> Option<Duration> {
    let elapsed = now.saturating_duration_since(last_motion);
    let remaining = PANE_CONTROLS_IDLE_DELAY.checked_sub(elapsed)?;
    (!remaining.is_zero()).then_some(remaining)
}

fn new_tab_profile(
    active_profile: Option<&Profile>,
    profiles: &[Profile],
    default_profile: usize,
) -> Option<Profile> {
    active_profile
        .cloned()
        .or_else(|| profiles.get(default_profile).cloned())
}

pub(crate) struct Zetta {
    pub(crate) launch_config: Config,
    pub(crate) configuration_error: Option<String>,
    pub(crate) pane_output_error: Option<String>,
    pub(crate) pane_output_save_in_progress: bool,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) background_sessions: BackgroundSessionRunner<Tab>,
    pub(crate) background_observed_panes: HashSet<u64>,
    pub(crate) background_process_refresh_running: bool,
    pub(crate) background_session_picker_entries: Vec<(u64, String, String)>,
    pub(crate) reconnect_menu_handle: PopoverMenuHandle<ui::ContextMenu>,
    pub(crate) session_authentication_focus: gpui::FocusHandle,
    pub(crate) session_authentication: Option<SessionAuthenticationPrompt>,
    pub(crate) session_authentication_generation: u64,
    pub(crate) active_tab: usize,
    pub(crate) profiles: Vec<Profile>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) next_tab_id: u64,
    pub(crate) next_pane_id: u64,
    pub(crate) rename_focus: gpui::FocusHandle,
    pub(crate) command_palette_focus: gpui::FocusHandle,
    pub(crate) command_palette: Option<CommandPalette>,
    pub(crate) multi_command_focus: gpui::FocusHandle,
    pub(crate) multi_command: Option<MultiCommandPrompt>,
    pub(crate) multi_command_catalog: CompletionCatalog,
    pub(crate) multi_command_launches: BoundedLaunchQueue<QueuedTerminalLaunch>,
    pub(crate) settings_focus: gpui::FocusHandle,
    pub(crate) settings_editor: Option<SettingsEditor>,
    pub(crate) serial_console_focus: gpui::FocusHandle,
    pub(crate) serial_console: Option<SerialConsolePrompt>,
    pub(crate) serial_console_generation: u64,
    pub(crate) tab_search_focus: gpui::FocusHandle,
    pub(crate) tab_search: Option<TabSearch>,
    pub(crate) minimized_panes_focus: gpui::FocusHandle,
    pub(crate) pane_controls_visible_for: Option<u64>,
    pub(crate) pane_controls_last_motion: Instant,
    pub(crate) pane_controls_hide_task: Option<Task<()>>,
    pub(crate) titlebar_dragging: bool,
    pub(crate) button_layout: WindowButtonLayout,
    pub(crate) performance_overlay: Option<PerformanceOverlay>,
    pub(crate) performance_overlay_generation: u64,
    pub(crate) terminal_spawn_notify_pending: bool,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl Zetta {
    pub(crate) fn prepare_for_background_window_close(&mut self, cx: &mut Context<Self>) {
        let tabs = std::mem::take(&mut self.tabs);
        let mut preserved_any = false;
        for tab in tabs {
            if let Some(authentication) = tab.close_policy.background_authentication() {
                self.store_background_tab(tab, authentication, cx);
                preserved_any = true;
            }
        }
        if preserved_any {
            self.finish_background_session_change(cx);
        }
        self.active_tab = 0;
        self.command_palette = None;
        self.multi_command = None;
        self.settings_editor = None;
        self.serial_console = None;
        self.session_authentication = None;
        self.tab_search = None;
        cx.notify();
    }

    pub(crate) fn attach_to_reopened_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.button_layout = system_window_button_layout(cx);
        self._subscriptions
            .push(cx.observe_button_layout_changed(window, |this, _, cx| {
                this.button_layout = system_window_button_layout(cx);
                cx.notify();
            }));
        self._subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active()
                    && !this.is_renaming()
                    && this.command_palette.is_none()
                    && this.multi_command.is_none()
                    && this.serial_console.is_none()
                    && this.session_authentication.is_none()
                    && this.tab_search.is_none()
                {
                    this.focus_active(window, cx);
                }
            }));
        if self.tabs.is_empty() {
            self.open_tab(window, cx);
        }
    }

    pub(crate) fn resume_hidden_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            self.open_tab(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn configure_pane_profile_stress(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let active_pane_id = tab.active_pane;
        let Some(profile) = tab.active_profile().cloned() else {
            return;
        };
        let mut pane_ids = vec![active_pane_id];
        while pane_ids.len() < MAX_PANES_PER_TAB {
            let pane_id = self.next_pane_id;
            self.next_pane_id += 1;
            tab.push_pane(TerminalPane {
                id: pane_id,
                label_number: 0,
                generated_label: Some(format!("Stress {:02}", pane_ids.len() + 1)),
                custom_label: None,
                profile: profile.clone(),
                terminal: None,
                view: None,
                error: None,
                wsl_cwd_file: None,
                pending_command: None,
            });
            pane_ids.push(pane_id);
        }
        tab.layout = PaneLayout::tiled(&pane_ids).expect("a stress profile has panes");
        tab.minimized_panes = pane_ids.into_iter().skip(1).collect();
        tab.selected_minimized_pane = tab.minimized_panes.last().copied();
        tab.maximized_pane = None;
        tab.activate_pane(active_pane_id);
        cx.notify();
    }

    pub(crate) fn new(
        config: Config,
        configuration_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let button_layout = system_window_button_layout(cx);
        let mut this = Self {
            launch_config: config.clone(),
            configuration_error,
            pane_output_error: None,
            pane_output_save_in_progress: false,
            tabs: Vec::new(),
            background_sessions: BackgroundSessionRunner::default(),
            background_observed_panes: HashSet::new(),
            background_process_refresh_running: false,
            background_session_picker_entries: Vec::new(),
            reconnect_menu_handle: PopoverMenuHandle::default(),
            session_authentication_focus: cx.focus_handle(),
            session_authentication: None,
            session_authentication_generation: 0,
            active_tab: 0,
            profiles: config.profiles,
            working_directory: config.working_directory,
            next_tab_id: 1,
            next_pane_id: 1,
            rename_focus: cx.focus_handle(),
            command_palette_focus: cx.focus_handle(),
            command_palette: None,
            multi_command_focus: cx.focus_handle(),
            multi_command: None,
            multi_command_catalog: CompletionCatalog::default(),
            multi_command_launches: BoundedLaunchQueue::new(MAX_CONCURRENT_MULTI_COMMAND_SPAWNS),
            settings_focus: cx.focus_handle(),
            settings_editor: None,
            serial_console_focus: cx.focus_handle(),
            serial_console: None,
            serial_console_generation: 0,
            tab_search_focus: cx.focus_handle(),
            tab_search: None,
            minimized_panes_focus: cx.focus_handle(),
            pane_controls_visible_for: None,
            pane_controls_last_motion: Instant::now(),
            pane_controls_hide_task: None,
            titlebar_dragging: false,
            button_layout,
            performance_overlay: None,
            performance_overlay_generation: 0,
            terminal_spawn_notify_pending: false,
            _subscriptions: vec![
                cx.observe_button_layout_changed(window, |this, _, cx| {
                    this.button_layout = system_window_button_layout(cx);
                    cx.notify();
                }),
                cx.observe_window_activation(window, |this, window, cx| {
                    if window.is_window_active()
                        && !this.is_renaming()
                        && this.command_palette.is_none()
                        && this.multi_command.is_none()
                        && this.serial_console.is_none()
                        && this.session_authentication.is_none()
                        && this.tab_search.is_none()
                    {
                        this.focus_active(window, cx);
                    }
                }),
            ],
        };
        this.load_multi_command_catalog(cx);
        this.open_tab(window, cx);
        this
    }

    pub(crate) fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_profile = self.tabs.get(self.active_tab).and_then(Tab::active_profile);
        let Some(profile) = new_tab_profile(
            active_profile,
            &self.profiles,
            self.launch_config.default_profile,
        ) else {
            return;
        };
        self.open_tab_with_profile(profile, window, cx);
    }

    pub(crate) fn open_tab_with_profile(
        &mut self,
        profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_pane = self.tabs.get(self.active_tab).and_then(Tab::active_pane);
        let inherited_working_directory = active_pane
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.view.as_ref())
            .and_then(|view| view.read(cx).terminal().read(cx).working_directory());
        let inherited_wsl_directory = active_pane
            .filter(|pane| pane.profile.name.eq_ignore_ascii_case(&profile.name))
            .and_then(|pane| pane.wsl_working_directory(cx));
        let (working_directory, wsl_directory) = launch_working_directory(
            &profile,
            inherited_working_directory,
            inherited_wsl_directory,
            self.working_directory.clone(),
            self.launch_config.working_directory_configured,
        );
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let wsl_cwd_file = wsl_cwd_tracking_file(&profile, pane_id);
        self.tabs.push(Tab {
            id: tab_id,
            panes: vec![TerminalPane {
                id: pane_id,
                label_number: 1,
                generated_label: None,
                custom_label: None,
                profile: profile.clone(),
                terminal: None,
                view: None,
                error: None,
                wsl_cwd_file: wsl_cwd_file.clone(),
                pending_command: None,
            }],
            pane_indices: HashMap::from([(pane_id, 0)]),
            next_pane_label: 2,
            layout: PaneLayout::Pane(pane_id),
            active_pane: pane_id,
            focus_history: vec![pane_id],
            maximized_pane: None,
            minimized_panes: Vec::new(),
            selected_minimized_pane: None,
            broadcast_input: false,
            close_policy: TabClosePolicy::Close,
            custom_title: None,
            renaming_pane: None,
            rename_buffer: None,
            rename_cursor: 0,
            rename_select_all: false,
        });
        self.active_tab = self.tabs.len() - 1;

        self.spawn_terminal(
            tab_id,
            pane_id,
            profile,
            working_directory,
            wsl_directory,
            wsl_cwd_file,
            window,
            cx,
        );
    }

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
        let is_wsl = is_wsl_shell(&profile.command);
        let command = if is_wsl {
            wsl_shell_with_tracking(
                profile.command,
                wsl_directory.as_deref(),
                wsl_cwd_file.as_deref(),
            )
        } else {
            profile.command
        };
        let environment = if is_wsl {
            HashMap::default()
        } else {
            native_terminal_environment().into_iter().collect()
        };
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
                        cx.subscribe_in(
                            &view,
                            window,
                            move |this, _, event, window, cx| match event {
                                TerminalViewEvent::Close => {
                                    this.terminal_closed(tab_id, pane_id, window, cx);
                                }
                                TerminalViewEvent::TitleChanged => {
                                    cx.notify();
                                }
                                TerminalViewEvent::Input(input) => {
                                    this.broadcast_input(tab_id, pane_id, input, cx);
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
                        view.update(cx, |view, _| view.set_emit_input_events(emit_input_events));
                        cx.on_focus(&focus_handle, window, move |this, _, cx| {
                            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                                tab.activate_pane(pane_id);
                                cx.notify();
                            }
                        })
                        .detach();
                        let tab_index = this.tabs.iter().position(|tab| tab.id == tab_id);
                        let should_focus = tab_index.is_some_and(|index| {
                            index == this.active_tab && this.tabs[index].active_pane == pane_id
                        });
                        if let Some(pane) = tab_index
                            .and_then(|index| this.tabs.get_mut(index))
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.terminal = Some(terminal.clone());
                            pane.view = Some(view.clone());
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
                                this.observe_background_terminal(pane_id, terminal, cx);
                                this.publish_background_session_catalog(cx);
                            }
                        }
                        if should_focus {
                            view.focus_handle(cx).focus(window, cx);
                        }
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

    pub(crate) fn close_tab_at(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_tab_at_with_policy(index, true, window, cx);
    }

    fn close_tab_at_with_policy(
        &mut self,
        index: usize,
        background_if_pinned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        let background_authentication = background_authentication_for_close(
            &self.tabs[index].close_policy,
            background_if_pinned,
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
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        if index < self.active_tab {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.focus_active(window, cx);
    }

    pub(crate) fn close_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_pane_with_policy(tab_id, pane_id, true, window, cx);
    }

    fn terminal_closed(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_pane_with_policy(tab_id, pane_id, false, window, cx);
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

        let tab = &mut self.tabs[tab_index];
        tab.remove_pane(pane_id);
        let Some(layout) = tab.layout.clone().without(pane_id) else {
            self.close_tab_at_with_policy(tab_index, background_if_last_pane, window, cx);
            return;
        };
        tab.layout = layout;
        tab.restore_focus_after_close(pane_id, tab.layout.first_pane());
        self.active_tab = tab_index;
        self.focus_active(window, cx);
    }

    pub(crate) fn split_active_pane(
        &mut self,
        axis: SplitAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if !can_add_panes(tab.panes.len(), 1) {
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
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let wsl_cwd_file = wsl_cwd_tracking_file(&profile, pane_id);

        let tab = &mut self.tabs[self.active_tab];
        tab.maximized_pane = None;
        if !tab.layout.split(active_pane_id, axis, pane_id) {
            return;
        }
        tab.push_pane(TerminalPane {
            id: pane_id,
            label_number: 0,
            generated_label: None,
            custom_label: None,
            profile: profile.clone(),
            terminal: None,
            view: None,
            error: None,
            wsl_cwd_file: wsl_cwd_file.clone(),
            pending_command: None,
        });
        tab.activate_pane(pane_id);
        self.spawn_terminal(
            tab_id,
            pane_id,
            profile,
            working_directory,
            wsl_directory,
            wsl_cwd_file,
            window,
            cx,
        );
        cx.notify();
    }

    pub(crate) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    pub(crate) fn new_window(&mut self, _: &NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        open_zetta_window(
            self.launch_config.clone(),
            self.configuration_error.clone(),
            false,
            None,
            false,
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
        let Some(index) = action.slot.checked_sub(1) else {
            return;
        };
        if index >= self.profiles.len() {
            return;
        }
        let profile = self.profiles[index].clone();
        self.open_tab_with_profile(profile, window, cx);
    }

    pub(crate) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active_tab, window, cx);
    }

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

    fn move_tab_to_background(
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
        self.store_background_tab(tab, authentication, cx);
        self.finish_background_session_change(cx);
    }

    fn store_background_tab(
        &mut self,
        mut tab: Tab,
        authentication: Option<SessionAuthentication>,
        cx: &mut Context<Self>,
    ) {
        tab.rename_buffer = None;
        tab.renaming_pane = None;
        for pane in &mut tab.panes {
            pane.view = None;
        }
        let terminals = tab
            .panes
            .iter()
            .filter_map(|pane| Some((pane.id, pane.terminal.clone()?)))
            .collect::<Vec<_>>();
        self.background_sessions.detach(tab, authentication);
        for (pane_id, terminal) in terminals {
            self.observe_background_terminal(pane_id, terminal.clone(), cx);
            terminal.update(cx, Terminal::refresh_foreground_process);
        }
    }

    fn finish_background_session_change(&mut self, cx: &mut Context<Self>) {
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
    ) {
        if runner_id != self.background_sessions.runner_id() {
            let Some(source) = zetta_for_runner(runner_id, cx) else {
                return;
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
                return;
            }
            let verifier = source
                .read(cx)
                .background_session_authentication(session_id);
            if verifier.is_some() {
                self.prompt_to_reconnect_session(runner_id, session_id, window, cx);
                return;
            }
            let tab = source.update(cx, |source, cx| {
                source.take_background_session_by_id(session_id, None, cx)
            });
            if let Some(tab) = tab {
                prune_empty_dormant_runners(cx);
                self.attach_reconnected_tab(tab, true, window, cx);
            }
            return;
        }
        let Some(index) = self
            .background_sessions
            .iter()
            .position(|tab| tab.id == session_id)
        else {
            return;
        };
        let Some(tab) = self.background_sessions.iter().nth(index) else {
            return;
        };
        if self.background_sessions.authentication_at(index).is_some() {
            self.prompt_to_reconnect_session(runner_id, tab.id, window, cx);
            return;
        }
        let session_id = tab.id;
        if let Some(tab) = self.take_background_session_by_id(session_id, None, cx) {
            self.attach_reconnected_tab(tab, false, window, cx);
        }
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
                tab.panes
                    .iter()
                    .all(|pane| pane.terminal.is_some() || pane.error.is_some())
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
    ) {
        let tab = if runner_id == self.background_sessions.runner_id() {
            self.take_background_session_by_id(session_id, Some(authorization), cx)
        } else {
            let Some(source) = zetta_for_runner(runner_id, cx) else {
                return;
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
                return;
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
        }
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
        let tab_id = tab.id;
        let panes = tab
            .panes
            .iter()
            .filter_map(|pane| {
                Some((
                    pane.id,
                    pane.terminal.clone()?,
                    resolve_profile_theme(&pane.profile, cx),
                ))
            })
            .collect::<Vec<_>>();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;

        for (pane_id, terminal, terminal_theme) in panes {
            match terminal_theme {
                Ok(theme) => {
                    let view =
                        cx.new(|cx| TerminalView::new_with_theme(terminal, theme, window, cx));
                    self.connect_terminal_view(tab_id, pane_id, view, window, cx);
                }
                Err(error) => {
                    if let Some(pane) = self.tabs[self.active_tab].pane_mut(pane_id) {
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

    fn observe_background_terminal(
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
                TerminalEvent::CloseTerminal => {
                    this.reap_background_pane(pane_id, cx);
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
                            .flat_map(|tab| &tab.panes)
                            .filter_map(|pane| pane.terminal.clone())
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

    fn publish_background_session_catalog(&mut self, cx: &mut Context<Self>) {
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
                let (terminal_title, foreground_command, working_directory) = pane
                    .terminal
                    .as_ref()
                    .map(|terminal| {
                        let terminal = terminal.read(cx);
                        (
                            Some(terminal.title(false)),
                            terminal.foreground_process_command_line(),
                            terminal.working_directory(),
                        )
                    })
                    .unwrap_or_default();
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
        tab.custom_title.clone().unwrap_or_else(|| {
            tab.active_pane()
                .and_then(|pane| pane.terminal.as_ref())
                .map(|terminal| terminal.read(cx).title(false))
                .unwrap_or_else(|| format!("Tab {}", tab.id))
        })
    }

    fn connect_terminal_view(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_label = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.generated_label.as_deref());
        let is_http_server = pane_label.is_some_and(|label| label.starts_with("HTTP: "));
        let is_tftp_server = pane_label.is_some_and(|label| label.starts_with("TFTP: "));
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => this.terminal_closed(tab_id, pane_id, window, cx),
                TerminalViewEvent::TitleChanged => cx.notify(),
                TerminalViewEvent::Input(input)
                    if (is_http_server
                        && crate::http_server_ui::http_input_stops_server(input))
                        || (is_tftp_server
                            && crate::tftp_server_ui::tftp_input_stops_server(input)) =>
                {
                    this.terminal_closed(tab_id, pane_id, window, cx);
                }
                TerminalViewEvent::Input(input) => {
                    this.broadcast_input(tab_id, pane_id, input, cx);
                }
            },
        )
        .detach();
        let focus_handle = view.focus_handle(cx);
        cx.on_focus(&focus_handle, window, move |this, _, cx| {
            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.activate_pane(pane_id);
                cx.notify();
            }
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
        }
    }

    pub(crate) fn close_active_pane(
        &mut self,
        _: &ClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        self.close_pane(tab.id, tab.active_pane, window, cx);
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
        let Some(view) = pane.view.as_ref() else {
            return;
        };
        let view = view.clone();
        let is_wsl = is_wsl_shell(&pane.profile.command);
        if !begin_pane_output_save(&mut self.pane_output_save_in_progress) {
            return;
        }

        let terminal = view.read(cx).terminal().clone();
        let output = terminal.read(cx).get_content_async();
        let directory = (!is_wsl)
            .then(|| terminal.read(cx).working_directory())
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

    pub(crate) fn split_horizontal(
        &mut self,
        _: &SplitHorizontal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_active_pane(SplitAxis::Horizontal, window, cx);
    }

    pub(crate) fn split_vertical(
        &mut self,
        _: &SplitVertical,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_active_pane(SplitAxis::Vertical, window, cx);
    }

    pub(crate) fn rotate_pane_layout(
        &mut self,
        _: &RotatePaneLayout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if !tab.layout.rotate_two_pane_split() {
            return;
        }
        for terminal in tab.panes.iter().filter_map(|pane| pane.terminal.as_ref()) {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }
        cx.notify();
    }

    pub(crate) fn apply_pane_split_template(
        &mut self,
        action: &ApplyPaneSplitTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(new_pane_count) = self
            .launch_config
            .pane_split_templates
            .get(&action.name)
            .map(|template| template.pane_count() - 1)
        else {
            self.configuration_error = Some(format!(
                "Pane split template {:?} is not configured",
                action.name
            ));
            cx.notify();
            return;
        };
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if !can_add_panes(tab.panes.len(), new_pane_count) {
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
        let terminal_theme = match resolve_profile_theme(&profile, cx) {
            Ok(theme) => theme,
            Err(error) => {
                self.configuration_error = Some(format!(
                    "Could not apply profile theme for pane template: {error:#}"
                ));
                cx.notify();
                return;
            }
        };
        let mut terminal_settings = TerminalSpawnSettings::current(cx);
        let inherited_wsl_directory = active_pane.and_then(|pane| pane.wsl_working_directory(cx));
        let (working_directory, wsl_directory) = launch_working_directory(
            &profile,
            inherited_working_directory,
            inherited_wsl_directory,
            self.working_directory.clone(),
            self.launch_config.working_directory_configured,
        );

        let new_pane_ids = (0..new_pane_count).map(|_| {
            let pane_id = self.next_pane_id;
            self.next_pane_id += 1;
            pane_id
        });
        let new_panes = prepare_pane_launches(new_pane_ids, |pane_id| {
            wsl_cwd_tracking_file(&profile, pane_id)
        });
        let mut all_pane_ids =
            std::iter::once(active_pane_id).chain(new_panes.iter().map(|(pane_id, _)| *pane_id));
        let replacement = pane_layout_from_configured_template(
            &self.launch_config.pane_split_templates,
            &action.name,
            &mut all_pane_ids,
        )
        .expect("the configured pane template was resolved before allocating panes");

        let tab = &mut self.tabs[self.active_tab];
        tab.maximized_pane = None;
        if !tab.layout.replace(active_pane_id, replacement) {
            return;
        }
        tab.panes.reserve(new_pane_count);
        for (pane_id, wsl_cwd_file) in &new_panes {
            tab.push_pane(TerminalPane {
                id: *pane_id,
                label_number: 0,
                generated_label: None,
                custom_label: None,
                profile: profile.clone(),
                terminal: None,
                view: None,
                error: None,
                wsl_cwd_file: wsl_cwd_file.clone(),
                pending_command: None,
            });
        }
        tab.activate_pane(active_pane_id);

        let spawn_count = new_panes.len();
        for (index, (pane_id, wsl_cwd_file)) in new_panes.into_iter().enumerate() {
            let path_hyperlink_regexes =
                terminal_settings.path_hyperlink_regexes(index + 1 == spawn_count);
            self.spawn_terminal_with_theme(
                tab_id,
                pane_id,
                profile.clone(),
                working_directory.clone(),
                wsl_directory.clone(),
                wsl_cwd_file,
                terminal_theme.clone(),
                &terminal_settings,
                path_hyperlink_regexes,
                false,
                window,
                cx,
            );
        }
        self.focus_active(window, cx);
        cx.notify();
    }

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
        let Some(pane_id) = tab
            .visible_layout()
            .and_then(|layout| layout.adjacent_pane(tab.active_pane, direction))
        else {
            return;
        };
        tab.activate_pane(pane_id);
        self.focus_active(window, cx);
    }

    pub(crate) fn toggle_maximize_pane_by_id(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.toggle_maximize(pane_id) {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn toggle_maximize_pane(
        &mut self,
        _: &ToggleMaximizePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.tabs.get(self.active_tab).map(|tab| tab.active_pane) else {
            return;
        };
        self.toggle_maximize_pane_by_id(pane_id, window, cx);
    }

    pub(crate) fn minimize_pane_by_id(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.minimize(pane_id) {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn minimize_pane(
        &mut self,
        _: &MinimizePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.tabs.get(self.active_tab).map(|tab| tab.active_pane) else {
            return;
        };
        self.minimize_pane_by_id(pane_id, window, cx);
    }

    pub(crate) fn restore_minimized_pane_by_id(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.restore_minimized(pane_id) {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn restore_minimized_pane(
        &mut self,
        _: &RestoreMinimizedPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.restore_last_minimized() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn select_previous_minimized_pane(
        &mut self,
        _: &SelectPreviousMinimizedPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .tabs
            .get_mut(self.active_tab)
            .is_some_and(Tab::select_previous_minimized);
        if selected {
            cx.notify();
        }
    }

    pub(crate) fn select_next_minimized_pane(
        &mut self,
        _: &SelectNextMinimizedPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .tabs
            .get_mut(self.active_tab)
            .is_some_and(Tab::select_next_minimized);
        if selected {
            cx.notify();
        }
    }

    pub(crate) fn focus_pane_left(
        &mut self,
        _: &FocusPaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Left, window, cx);
    }

    pub(crate) fn focus_pane_right(
        &mut self,
        _: &FocusPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Right, window, cx);
    }

    pub(crate) fn focus_pane_up(
        &mut self,
        _: &FocusPaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Up, window, cx);
    }

    pub(crate) fn focus_pane_down(
        &mut self,
        _: &FocusPaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Down, window, cx);
    }

    pub(crate) fn increase_terminal_font_size(
        &mut self,
        _: &IncreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::increase_buffer_font_size(cx);
    }

    pub(crate) fn decrease_terminal_font_size(
        &mut self,
        _: &DecreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::decrease_buffer_font_size(cx);
    }

    pub(crate) fn reset_terminal_font_size(
        &mut self,
        _: &ResetTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::reset_buffer_font_size(cx);
    }

    pub(crate) fn increase_pane_font_size(
        &mut self,
        _: &IncreasePaneFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.clone())
        {
            view.update(cx, TerminalView::increase_font_size);
        }
    }

    pub(crate) fn decrease_pane_font_size(
        &mut self,
        _: &DecreasePaneFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.clone())
        {
            view.update(cx, TerminalView::decrease_font_size);
        }
    }

    pub(crate) fn reset_pane_font_size(
        &mut self,
        _: &ResetPaneFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.clone())
        {
            view.update(cx, TerminalView::reset_font_size);
        }
    }

    pub(crate) fn toggle_performance_overlay(
        &mut self,
        _: &TogglePerformanceOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.performance_overlay_generation = self.performance_overlay_generation.wrapping_add(1);
        if self.performance_overlay.take().is_some() {
            disable_frame_tracing();
            cx.notify();
            return;
        }

        enable_frame_tracing();
        let generation = self.performance_overlay_generation;
        let (pane_count, minimized_pane_count) = self
            .tabs
            .get(self.active_tab)
            .map(|tab| (tab.panes.len(), tab.minimized_panes.len()))
            .unwrap_or_default();
        self.performance_overlay = Some(PerformanceOverlay::new(
            window.window_handle().window_id(),
            generation,
            pane_count,
            minimized_pane_count,
        ));
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(PERFORMANCE_SAMPLE_INTERVAL).await;
                let keep_sampling = this
                    .update(cx, |this, cx| {
                        let Some(overlay) = this.performance_overlay.as_mut() else {
                            return false;
                        };
                        if overlay.generation != generation {
                            return false;
                        }
                        overlay.sample();
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_sampling {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn start_performance_report(
        &mut self,
        options: PerformanceReportOptions,
        status: PerformanceReportStatus,
        cx: &mut Context<Self>,
    ) {
        let Some(overlay) = self.performance_overlay.as_mut() else {
            *status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Err(
                "performance overlay was not enabled before report capture".to_owned(),
            ));
            quit_zetta_process(cx);
            return;
        };
        overlay.begin_report();

        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(options.duration).await;
            let result = this
                .update(cx, |this, _| {
                    this.performance_overlay
                        .as_mut()
                        .context("performance overlay closed before report completed")?
                        .write_report(&options.path, options.duration)
                })
                .unwrap_or_else(Err)
                .map_err(|error| format!("{error:#}"));
            *status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
            cx.update(quit_zetta_process);
        })
        .detach();
    }

    pub(crate) fn reload_configuration(
        &mut self,
        _: &ReloadConfiguration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let config_path = self.launch_config.config_path.clone();
        let keymap_override = self.launch_config.keymap_override.clone();
        let config = match Config::load(Some(&config_path), keymap_override) {
            Ok(config) => config,
            Err(error) => {
                self.configuration_error = Some(format!(
                    "Could not load {}: {error:#}",
                    config_path.display()
                ));
                cx.notify();
                return;
            }
        };

        load_user_themes(cx).log_err();
        if let Err(error) = apply_config_settings(&config, cx) {
            self.configuration_error = Some(format!(
                "Could not apply {}: {error:#}",
                config_path.display()
            ));
            cx.notify();
            return;
        }
        let profile_themes = match config
            .profiles
            .iter()
            .map(|profile| {
                resolve_profile_theme(profile, cx).map(|theme| (profile.name.to_lowercase(), theme))
            })
            .collect::<Result<HashMap<_, _>>>()
        {
            Ok(themes) => themes,
            Err(error) => {
                self.configuration_error = Some(format!(
                    "Could not apply {}: {error:#}",
                    config_path.display()
                ));
                cx.notify();
                return;
            }
        };
        for pane in self.tabs.iter_mut().flat_map(|tab| &mut tab.panes) {
            if let Some(profile) = config
                .profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(&pane.profile.name))
            {
                pane.profile = profile.clone();
            } else {
                pane.profile.theme = None;
            }
            if let Some(view) = pane.view.as_ref() {
                let theme = profile_themes
                    .get(&pane.profile.name.to_lowercase())
                    .cloned()
                    .flatten();
                view.update(cx, |view, cx| view.set_theme(theme, cx));
            }
        }
        load_keybindings(&config.keymap_path, config.profiles.len(), cx);

        #[cfg(windows)]
        windows_integration::update_profile_jump_list(config.profiles.clone());

        self.profiles = config.profiles.clone();
        self.working_directory = config.working_directory.clone();
        self.launch_config = config;
        self.configuration_error = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
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
            self.focus_active(window, cx);
        }
    }

    pub(crate) fn rename_tab(
        &mut self,
        _: &RenameTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
            .cloned();
        if let Some(view) = view {
            self.begin_rename(view, window, cx);
        }
    }

    pub(crate) fn begin_rename(
        &mut self,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let automatic_title = view.read(cx).tab_content_text(0, cx).to_string();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let title = tab.custom_title.clone().unwrap_or(automatic_title);
            tab.renaming_pane = None;
            tab.rename_cursor = title.len();
            tab.rename_buffer = Some(title);
            tab.rename_select_all = false;
        }
        self.rename_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn rename_pane(
        &mut self,
        _: &RenamePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.tabs.get(self.active_tab).map(|tab| tab.active_pane) else {
            return;
        };
        self.begin_pane_rename(pane_id, window, cx);
    }

    pub(crate) fn begin_pane_rename(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(label) = tab.pane(pane_id).map(TerminalPane::label) else {
            return;
        };
        tab.activate_pane(pane_id);
        tab.renaming_pane = Some(pane_id);
        tab.rename_cursor = label.len();
        tab.rename_buffer = Some(label);
        tab.rename_select_all = true;
        self.rename_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let active_is_visible = tab.pane_is_visible(tab.active_pane);
            if active_is_visible {
                if let Some(view) = tab.active_pane().and_then(|pane| pane.view.as_ref()) {
                    view.focus_handle(cx).focus(window, cx);
                }
            } else if !tab.minimized_panes.is_empty() {
                self.minimized_panes_focus.focus(window, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn show_pane_controls(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visibility_changed = self.pane_controls_visible_for != Some(pane_id);
        self.pane_controls_visible_for = Some(pane_id);
        self.pane_controls_last_motion = Instant::now();

        if self.pane_controls_hide_task.is_none() {
            let executor = cx.background_executor().clone();
            self.pane_controls_hide_task = Some(cx.spawn_in(window, async move |this, cx| {
                let mut remaining = PANE_CONTROLS_IDLE_DELAY;
                loop {
                    executor.timer(remaining).await;
                    let next_delay = this
                        .update(cx, |this, cx| {
                            let next_delay = pane_controls_hide_delay(
                                this.pane_controls_last_motion,
                                Instant::now(),
                            );
                            if next_delay.is_none() {
                                this.pane_controls_visible_for = None;
                                this.pane_controls_hide_task.take();
                                cx.notify();
                            }
                            next_delay
                        })
                        .ok()
                        .flatten();
                    let Some(next_delay) = next_delay else {
                        break;
                    };
                    remaining = next_delay;
                }
            }));
        }

        if visibility_changed {
            cx.notify();
        }
    }

    pub(crate) fn is_renaming(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.rename_buffer.is_some())
    }

    pub(crate) fn render_pane_layout(
        &self,
        tab: &Tab,
        layout: &PaneLayout,
        colors: &ThemeColors,
        error_color: gpui::Hsla,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match layout {
            PaneLayout::Pane(pane_id) => {
                let Some(pane) = tab.pane(*pane_id) else {
                    return div().size_full().into_any_element();
                };
                let pane_label = tab
                    .displayed_pane_label(*pane_id)
                    .unwrap_or_else(|| pane.label());
                let pane_label_selected = tab.renaming_pane == Some(*pane_id)
                    && tab.rename_select_all
                    && tab.rename_buffer.is_some();
                let active = pane.view.as_ref().is_some_and(|view| {
                    view.focus_handle(cx).is_focused(window)
                        || view.read(cx).has_open_context_menu()
                        || view.read(cx).has_open_search()
                        || self.tab_search.as_ref().is_some_and(|search| {
                            search.tab_id == tab.id && tab.active_pane == *pane_id
                        })
                }) || (pane.view.is_none() && tab.active_pane == *pane_id);
                let content = match (&pane.view, &pane.error) {
                    (Some(view), _) => div().size_full().child(view.clone()).into_any_element(),
                    (_, Some(error)) => div()
                        .size_full()
                        .p_4()
                        .bg(colors.editor_background)
                        .text_color(error_color)
                        .child("Unable to start shell")
                        .child(div().mt_2().text_sm().child(error.clone()))
                        .into_any_element(),
                    _ => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(colors.editor_background)
                        .text_color(colors.text_muted)
                        .child(format!("Starting {}...", pane.profile.name))
                        .into_any_element(),
                };
                div()
                    .id(("terminal-pane", *pane_id as usize))
                    .relative()
                    .when(
                        tab.panes.len() > 1 && tab.maximized_pane.is_none(),
                        |pane| {
                            let pane_id = *pane_id;
                            pane.on_mouse_move(cx.listener(move |this, _, window, cx| {
                                this.show_pane_controls(pane_id, window, cx);
                            }))
                        },
                    )
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow_1()
                    .flex_basis(gpui::relative(0.))
                    .overflow_hidden()
                    .bg(gpui::black())
                    .child(
                        div()
                            .size_full()
                            .when(!active, |pane| {
                                pane.opacity(self.launch_config.inactive_pane_opacity)
                            })
                            .child(content),
                    )
                    .when(
                        tab.panes.len() > 1
                            && tab.maximized_pane.is_none()
                            && (self.pane_controls_visible_for == Some(*pane_id)
                                || tab.renaming_pane == Some(*pane_id)),
                        |pane| {
                            let maximize_handle = cx.entity().downgrade();
                            let minimize_handle = cx.entity().downgrade();
                            let close_handle = cx.entity().downgrade();
                            let rename_handle = cx.entity().downgrade();
                            let tab_id = tab.id;
                            let maximize_pane_id = *pane_id;
                            let minimize_pane_id = *pane_id;
                            let close_pane_id = *pane_id;
                            let rename_pane_id = *pane_id;
                            let pane_label_tooltip =
                                format!("{pane_label}\nDouble-click to label this pane");
                            pane.child(
                                div()
                                    .absolute()
                                    .top(px(4.))
                                    .when(
                                        self.launch_config.pane_controls_position
                                            == PaneControlsPosition::Left,
                                        |controls| controls.left(px(4.)),
                                    )
                                    .when(
                                        self.launch_config.pane_controls_position
                                            == PaneControlsPosition::Right,
                                        |controls| controls.right(px(4.)),
                                    )
                                    .flex()
                                    .when(
                                        self.launch_config.pane_controls_position
                                            == PaneControlsPosition::Left,
                                        |controls| controls.flex_row_reverse(),
                                    )
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .id(("terminal-pane-label", *pane_id as usize))
                                            .h_6()
                                            .max_w(px(240.))
                                            .flex()
                                            .items_center()
                                            .px_2()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(colors.border)
                                            .bg(colors.status_bar_background)
                                            .when(pane_label_selected, |label| {
                                                label.bg(colors.element_selected)
                                            })
                                            .cursor_text()
                                            .overflow_hidden()
                                            .tooltip(Tooltip::text(pane_label_tooltip))
                                            .on_click(move |event, window, cx| {
                                                if event.click_count() == 2 {
                                                    cx.stop_propagation();
                                                    rename_handle
                                                        .update(cx, |this, cx| {
                                                            this.begin_pane_rename(
                                                                rename_pane_id,
                                                                window,
                                                                cx,
                                                            );
                                                        })
                                                        .ok();
                                                }
                                            })
                                            .child(
                                                Label::new(pane_label)
                                                    .size(LabelSize::Small)
                                                    .color(Color::Custom(colors.text_muted)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                IconButton::new(
                                                    ("minimize-terminal-pane", *pane_id as usize),
                                                    IconName::Dash,
                                                )
                                                .style(ButtonStyle::Transparent)
                                                .size(ButtonSize::Compact)
                                                .icon_size(IconSize::XSmall)
                                                .icon_color(Color::Custom(colors.icon))
                                                .aria_label("Minimize pane")
                                                .tooltip(Tooltip::text("Minimize pane"))
                                                .on_click(move |_, window, cx| {
                                                    minimize_handle
                                                        .update(cx, |this, cx| {
                                                            this.minimize_pane_by_id(
                                                                minimize_pane_id,
                                                                window,
                                                                cx,
                                                            );
                                                        })
                                                        .ok();
                                                }),
                                            )
                                            .child(
                                                IconButton::new(
                                                    ("maximize-terminal-pane", *pane_id as usize),
                                                    IconName::Maximize,
                                                )
                                                .style(ButtonStyle::Transparent)
                                                .size(ButtonSize::Compact)
                                                .icon_size(IconSize::XSmall)
                                                .icon_color(Color::Custom(colors.icon))
                                                .aria_label("Maximize pane")
                                                .tooltip(Tooltip::text(
                                                    "Maximize pane (Shift-Escape)",
                                                ))
                                                .on_click(move |_, window, cx| {
                                                    maximize_handle
                                                        .update(cx, |this, cx| {
                                                            this.toggle_maximize_pane_by_id(
                                                                maximize_pane_id,
                                                                window,
                                                                cx,
                                                            );
                                                        })
                                                        .ok();
                                                }),
                                            ),
                                    )
                                    .child(
                                        IconButton::new(
                                            ("close-terminal-pane", *pane_id as usize),
                                            IconName::Close,
                                        )
                                        .style(ButtonStyle::Transparent)
                                        .size(ButtonSize::Compact)
                                        .icon_size(IconSize::XSmall)
                                        .icon_color(Color::Custom(colors.icon))
                                        .aria_label("Close pane")
                                        .tooltip(Tooltip::text("Close pane (Ctrl-Shift-X)"))
                                        .on_click(
                                            move |_, window, cx| {
                                                close_handle
                                                    .update(cx, |this, cx| {
                                                        this.close_pane(
                                                            tab_id,
                                                            close_pane_id,
                                                            window,
                                                            cx,
                                                        );
                                                    })
                                                    .ok();
                                            },
                                        ),
                                    ),
                            )
                        },
                    )
                    .into_any_element()
            }
            PaneLayout::Split {
                axis,
                first,
                second,
            } => div()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex_grow_1()
                .flex_basis(gpui::relative(0.))
                .flex()
                .when(matches!(axis, SplitAxis::Horizontal), |split| {
                    split.flex_col()
                })
                .gap_px()
                .bg(colors.border)
                .child(self.render_pane_layout(tab, first, colors, error_color, window, cx))
                .child(self.render_pane_layout(tab, second, colors, error_color, window, cx))
                .into_any_element(),
        }
    }
}

impl Drop for Zetta {
    fn drop(&mut self) {
        if self.performance_overlay.is_some() {
            disable_frame_tracing();
        }
    }
}

fn enable_frame_tracing() {
    if PERFORMANCE_OVERLAY_COUNT.fetch_add(1, Ordering::AcqRel) == 0 {
        PERFORMANCE_OWNS_FRAME_TRACING
            .store(profiler::set_frame_trace_enabled(true), Ordering::Release);
    }
}

fn disable_frame_tracing() {
    let previous = PERFORMANCE_OVERLAY_COUNT.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0);
    if previous == 1 && PERFORMANCE_OWNS_FRAME_TRACING.swap(false, Ordering::AcqRel) {
        profiler::set_frame_trace_enabled(false);
    }
}

#[cfg(test)]
#[path = "tests/app.rs"]
mod tests;
