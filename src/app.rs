use super::*;

pub(crate) struct Zetta {
    pub(crate) launch_config: Config,
    pub(crate) configuration_error: Option<String>,
    pub(crate) pane_output_error: Option<String>,
    pub(crate) pane_output_save_in_progress: bool,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active_tab: usize,
    pub(crate) selected_profile: usize,
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
    pub(crate) tab_search_focus: gpui::FocusHandle,
    pub(crate) tab_search: Option<TabSearch>,
    pub(crate) titlebar_dragging: bool,
    pub(crate) button_layout: WindowButtonLayout,
    pub(crate) performance_overlay: Option<PerformanceOverlay>,
    pub(crate) performance_overlay_generation: u64,
    pub(crate) terminal_spawn_notify_pending: bool,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl Zetta {
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
            active_tab: 0,
            selected_profile: config.default_profile,
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
            tab_search_focus: cx.focus_handle(),
            tab_search: None,
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
                        && !this.is_renaming_tab()
                        && this.command_palette.is_none()
                        && this.multi_command.is_none()
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
        let profile = self.profiles[self.selected_profile].clone();
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
                profile: profile.clone(),
                view: None,
                error: None,
                wsl_cwd_file: wsl_cwd_file.clone(),
                pending_command: None,
            }],
            pane_indices: HashMap::from([(pane_id, 0)]),
            layout: PaneLayout::Pane(pane_id),
            active_pane: pane_id,
            focus_history: vec![pane_id],
            broadcast_input: false,
            custom_title: None,
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
        let command = if is_wsl_shell(&profile.command) {
            wsl_shell_with_tracking(
                profile.command,
                wsl_directory.as_deref(),
                wsl_cwd_file.as_deref(),
            )
        } else {
            profile.command
        };
        let builder = TerminalBuilder::new(
            working_directory,
            None,
            command,
            HashMap::default(),
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
                            TerminalView::new_with_theme(terminal, terminal_theme, window, cx)
                        });
                        cx.subscribe_in(
                            &view,
                            window,
                            move |this, _, event, window, cx| match event {
                                TerminalViewEvent::Close => {
                                    this.close_pane(tab_id, pane_id, window, cx);
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
                            pane.view = Some(view.clone());
                            if let Some(command) = pane.pending_command.take() {
                                view.update(cx, |view, cx| {
                                    view.apply_input(
                                        &TerminalInput::Text(format!("{command}\r")),
                                        cx,
                                    )
                                });
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
        if index >= self.tabs.len() {
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
            self.close_tab_at(tab_index, window, cx);
            return;
        }

        let tab = &mut self.tabs[tab_index];
        tab.remove_pane(pane_id);
        let Some(layout) = tab.layout.clone().without(pane_id) else {
            self.close_tab_at(tab_index, window, cx);
            return;
        };
        tab.restore_focus_after_close(pane_id, layout.first_pane());
        tab.layout = layout;
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
        if !tab.layout.split(active_pane_id, axis, pane_id) {
            return;
        }
        tab.push_pane(TerminalPane {
            id: pane_id,
            profile: profile.clone(),
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
        self.selected_profile = index;
        self.open_tab(window, cx);
    }

    pub(crate) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active_tab, window, cx);
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
        if !tab.layout.replace(active_pane_id, replacement) {
            return;
        }
        tab.panes.reserve(new_pane_count);
        for (pane_id, wsl_cwd_file) in &new_panes {
            tab.push_pane(TerminalPane {
                id: *pane_id,
                profile: profile.clone(),
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
        let Some(pane_id) = tab.layout.adjacent_pane(tab.active_pane, direction) else {
            return;
        };
        tab.activate_pane(pane_id);
        self.focus_active(window, cx);
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
        self.performance_overlay = Some(PerformanceOverlay::new(
            window.window_handle().window_id(),
            generation,
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
            cx.quit();
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
            cx.update(|cx| cx.quit());
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

        self.selected_profile = config.default_profile;
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
            tab.rename_cursor = title.len();
            tab.rename_buffer = Some(title);
            tab.rename_select_all = false;
        }
        self.rename_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
        {
            view.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn is_renaming_tab(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.rename_buffer.is_some())
    }

    pub(crate) fn render_pane_layout(
        &self,
        tab: &Tab,
        layout: &PaneLayout,
        colors: &ThemeColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match layout {
            PaneLayout::Pane(pane_id) => {
                let Some(pane) = tab.pane(*pane_id) else {
                    return div().size_full().into_any_element();
                };
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
                        .text_color(cx.theme().status().error)
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
                .child(self.render_pane_layout(tab, first, colors, window, cx))
                .child(self.render_pane_layout(tab, second, colors, window, cx))
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
