use super::*;
use strum::IntoEnumIterator as _;

const PANE_CONTROLS_IDLE_DELAY: Duration = Duration::from_millis(1200);
const PERFORMANCE_PANE_STRESS_COUNT: usize = 4;

/// Cached font enumeration for settings font picker
pub(crate) struct FontCache {
    pub fonts: Arc<[String]>,
}

/// Cached icon entries (icon + precomputed lowercase label) shared by the
/// tab icon picker, used both for per-tab icons and the config default icon.
pub(crate) struct IconCache {
    pub entries: Arc<[IconEntry]>,
}

#[derive(Clone, Copy)]
enum ApplicationMenuDirection {
    Left,
    Right,
}

fn adjacent_application_menu_index(
    menu_count: usize,
    current_index: usize,
    direction: ApplicationMenuDirection,
) -> usize {
    match direction {
        ApplicationMenuDirection::Left => current_index.checked_sub(1).unwrap_or(menu_count - 1),
        ApplicationMenuDirection::Right => (current_index + 1) % menu_count,
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

fn pane_controls_hide_delay(last_motion: Instant, now: Instant) -> Option<Duration> {
    let elapsed = now.saturating_duration_since(last_motion);
    let remaining = PANE_CONTROLS_IDLE_DELAY.checked_sub(elapsed)?;
    (!remaining.is_zero()).then_some(remaining)
}

fn toggle_hidden_pane_controls(hidden_panes: &mut HashSet<u64>, pane_ids: &[u64]) -> bool {
    let hide = pane_ids
        .iter()
        .any(|pane_id| !hidden_panes.contains(pane_id));
    if hide {
        hidden_panes.extend(pane_ids.iter().copied());
    } else {
        for pane_id in pane_ids {
            hidden_panes.remove(pane_id);
        }
    }
    hide
}

fn default_hidden_pane_controls(
    pane_controls_hidden_by_default: bool,
    pane_ids: impl IntoIterator<Item = u64>,
) -> HashSet<u64> {
    if pane_controls_hidden_by_default {
        pane_ids.into_iter().collect()
    } else {
        HashSet::default()
    }
}

fn reset_pane_controls_visibility(
    hidden_panes: &mut HashSet<u64>,
    pane_controls_hidden_by_default: bool,
    pane_ids: impl IntoIterator<Item = u64>,
) {
    for pane_id in pane_ids {
        if pane_controls_hidden_by_default {
            hidden_panes.insert(pane_id);
        } else {
            hidden_panes.remove(&pane_id);
        }
    }
}

fn new_tab_profile(
    active_profile: Option<&Profile>,
    profiles: &[Profile],
    default_profile: usize,
    new_tab_profile: NewTabProfile,
) -> Option<Profile> {
    match new_tab_profile {
        NewTabProfile::Default => profiles.get(default_profile).cloned(),
        NewTabProfile::Inherit => active_profile
            .cloned()
            .or_else(|| profiles.get(default_profile).cloned()),
    }
}

/// Applies a `--profile`/`--theme` launch override (profile name lowercased,
/// theme name) to `profile` if its name matches, case-insensitively. Mutates
/// only this in-memory clone, so it never touches `Zetta::profiles` or the
/// settings UI, and is naturally lost once the process exits.
fn apply_launch_theme_override(
    profile: &mut Profile,
    launch_theme_override: Option<&(String, String)>,
) {
    if let Some((override_name, override_theme)) = launch_theme_override
        && profile.name.to_lowercase() == *override_name
    {
        profile.theme = Some(override_theme.clone());
    }
}

pub(crate) fn pane_input_enabled(modal_pane_mode_active: bool) -> bool {
    !modal_pane_mode_active
}

fn clamp_window_size_to_minimum(window_size: Size<Pixels>) -> Size<Pixels> {
    size(
        window_size.width.max(ZETTA_MINIMUM_WINDOW_SIZE.width),
        window_size.height.max(ZETTA_MINIMUM_WINDOW_SIZE.height),
    )
}

pub(crate) fn enforce_minimum_window_size(window: &mut Window) {
    let current_size = window.bounds().size;
    let clamped_size = clamp_window_size_to_minimum(current_size);
    if clamped_size != current_size {
        window.resize(clamped_size);
    }
}

pub(crate) struct Zetta {
    pub(crate) launch_config: Config,
    /// A `--profile`/`--theme` launch override: (profile name lowercased,
    /// theme name). Applied in `open_tab_with_profile` to every tab opened
    /// with that profile for the rest of this process, never written back to
    /// `launch_config`/`profiles` or the settings UI.
    pub(crate) launch_theme_override: Option<(String, String)>,
    pub(crate) configuration_error: Option<String>,
    pub(crate) pane_output_error: Option<String>,
    pub(crate) pane_output_save_in_progress: bool,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) background_sessions: BackgroundSessionRunner<Tab>,
    pub(crate) background_observed_panes: HashSet<u64>,
    pub(crate) background_process_refresh_running: bool,
    pub(crate) background_session_picker_entries: Vec<(u64, String, String)>,
    pub(crate) application_menu_handle: PopoverMenuHandle<ui::ContextMenu>,
    pub(crate) profile_menu_handle: PopoverMenuHandle<ui::ContextMenu>,
    pub(crate) reconnect_menu_handle: PopoverMenuHandle<ui::ContextMenu>,
    pub(crate) tab_overflow_left_menu_handle: PopoverMenuHandle<ui::ContextMenu>,
    pub(crate) tab_overflow_right_menu_handle: PopoverMenuHandle<ui::ContextMenu>,
    /// Which edge's overflow menu is currently open, so plain Tab/PageUp/PageDown
    /// (otherwise claimed by the menu's own list navigation) can keep cycling the
    /// active tab instead of just moving the popover's highlighted row.
    pub(crate) tab_overflow_keyboard_menu_edge: Option<bool>,
    /// Which side's overflow menu the current `active_tab` was picked from, so the
    /// visible tab range keeps it anchored at the edge it slid in from instead of
    /// jumping to whichever edge the default (unhinted) placement would choose.
    pub(crate) tab_overflow_selection_side: Option<bool>,
    pub(crate) application_menu_switch_pending: bool,
    pub(crate) session_authentication_focus: gpui::FocusHandle,
    pub(crate) session_authentication: Option<SessionAuthenticationPrompt>,
    pub(crate) session_authentication_generation: u64,
    pub(crate) active_tab: usize,
    pub(crate) visible_terminals: Vec<Entity<Terminal>>,
    pub(crate) profiles: Vec<Profile>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) next_tab_id: u64,
    pub(crate) next_pane_id: u64,
    pub(crate) rename_focus: gpui::FocusHandle,
    /// Focused while the overlay-style selector is open, so the section
    /// keys, arrow keys, Enter, and Escape operate it instead of reaching
    /// the terminal.
    pub(crate) overlay_style_focus: gpui::FocusHandle,
    pub(crate) command_palette_focus: gpui::FocusHandle,
    pub(crate) command_palette: Option<CommandPalette>,
    pub(crate) multi_command_focus: gpui::FocusHandle,
    pub(crate) multi_command: Option<MultiCommandPrompt>,
    pub(crate) multi_command_catalog: CompletionCatalog,
    pub(crate) multi_command_launches: BoundedLaunchQueue<QueuedTerminalLaunch>,
    pub(crate) settings_focus: gpui::FocusHandle,
    pub(crate) settings_editor: Option<SettingsEditor>,
    pub(crate) font_cache: Arc<OnceLock<FontCache>>,
    pub(crate) icon_cache: Arc<OnceLock<IconCache>>,
    pub(crate) tab_icon_picker_focus: gpui::FocusHandle,
    pub(crate) tab_icon_picker: Option<TabIconPicker>,
    pub(crate) theme_picker_focus: gpui::FocusHandle,
    pub(crate) theme_picker: Option<CommandPalette>,
    /// Name of the row representing the pane's currently effective theme,
    /// ticked in the picker regardless of keyboard-selection position.
    pub(crate) theme_picker_current: Option<String>,
    #[cfg(feature = "serial-console")]
    pub(crate) serial_console_focus: gpui::FocusHandle,
    #[cfg(feature = "serial-console")]
    pub(crate) serial_console: Option<SerialConsolePrompt>,
    #[cfg(feature = "serial-console")]
    pub(crate) serial_console_generation: u64,
    pub(crate) tab_search_focus: gpui::FocusHandle,
    pub(crate) tab_search: Option<TabSearch>,
    pub(crate) minimized_panes_focus: gpui::FocusHandle,
    pub(crate) pane_controls_visible_for: Option<u64>,
    pub(crate) pane_controls_hidden_for: HashSet<u64>,
    pub(crate) pane_controls_last_motion: Instant,
    pub(crate) pane_controls_hide_task: Option<Task<()>>,
    pub(crate) pane_resize_mode: bool,
    pub(crate) pane_resize_keys: PaneResizeKeys,
    pub(crate) pane_resize_repeat_generation: u64,
    pub(crate) pane_resize_drag: Option<PaneResizeDrag>,
    pub(crate) pane_move_mode: bool,
    pub(crate) titlebar_dragging: bool,
    pub(crate) button_layout: WindowButtonLayout,
    pub(crate) performance_overlay: Option<PerformanceOverlay>,
    pub(crate) performance_overlay_generation: u64,
    pub(crate) terminal_spawn_notify_pending: bool,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl Zetta {
    fn serial_console_is_open(&self) -> bool {
        #[cfg(feature = "serial-console")]
        {
            self.serial_console.is_some()
        }
        #[cfg(not(feature = "serial-console"))]
        {
            false
        }
    }

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
        #[cfg(feature = "serial-console")]
        {
            self.serial_console = None;
        }
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
                    && !this.serial_console_is_open()
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

    pub(crate) fn configure_pane_profile_stress(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let active_pane_id = tab.active_pane;
        let tab_id = tab.id;
        let Some(profile) = tab.active_profile().cloned() else {
            return;
        };
        let mut pane_ids = vec![active_pane_id];
        let mut added_pane_ids = Vec::with_capacity(PERFORMANCE_PANE_STRESS_COUNT - 1);
        while pane_ids.len() < PERFORMANCE_PANE_STRESS_COUNT {
            let pane_id = self.next_pane_id;
            self.next_pane_id += 1;
            pane_ids.push(pane_id);
            added_pane_ids.push(pane_id);
        }
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                added_pane_ids.iter().copied(),
            ));

        let tab = &mut self.tabs[self.active_tab];
        for (index, pane_id) in added_pane_ids.iter().copied().enumerate() {
            tab.push_pane(
                TerminalPane::new(pane_id, profile.clone())
                    .with_generated_label(format!("Stress {:02}", index + 2)),
            );
        }
        tab.layout = PaneLayout::tiled(&pane_ids).expect("a stress profile has panes");
        tab.minimized_panes.clear();
        tab.selected_minimized_pane = None;
        tab.maximized_pane = None;
        tab.activate_pane(active_pane_id);

        let working_directory = self.working_directory.clone();
        for pane_id in added_pane_ids {
            self.spawn_terminal(
                tab_id,
                pane_id,
                profile.clone(),
                working_directory.clone(),
                None,
                None,
                window,
                cx,
            );
        }
        cx.notify();
    }

    pub(crate) fn new(
        config: Config,
        configuration_error: Option<String>,
        initial_profile: Option<Profile>,
        launch_theme_override: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let button_layout = system_window_button_layout(cx);
        let mut this = Self {
            launch_config: config.clone(),
            launch_theme_override,
            configuration_error,
            pane_output_error: None,
            pane_output_save_in_progress: false,
            tabs: Vec::new(),
            background_sessions: BackgroundSessionRunner::default(),
            background_observed_panes: HashSet::new(),
            background_process_refresh_running: false,
            background_session_picker_entries: Vec::new(),
            application_menu_handle: PopoverMenuHandle::default(),
            profile_menu_handle: PopoverMenuHandle::default(),
            reconnect_menu_handle: PopoverMenuHandle::default(),
            tab_overflow_left_menu_handle: PopoverMenuHandle::default(),
            tab_overflow_right_menu_handle: PopoverMenuHandle::default(),
            tab_overflow_keyboard_menu_edge: None,
            tab_overflow_selection_side: None,
            application_menu_switch_pending: false,
            session_authentication_focus: cx.focus_handle(),
            session_authentication: None,
            session_authentication_generation: 0,
            active_tab: 0,
            visible_terminals: Vec::new(),
            profiles: config.profiles,
            working_directory: config.working_directory,
            next_tab_id: 1,
            next_pane_id: 1,
            rename_focus: cx.focus_handle(),
            overlay_style_focus: cx.focus_handle(),
            command_palette_focus: cx.focus_handle(),
            command_palette: None,
            multi_command_focus: cx.focus_handle(),
            multi_command: None,
            multi_command_catalog: CompletionCatalog::default(),
            multi_command_launches: BoundedLaunchQueue::new(MAX_CONCURRENT_MULTI_COMMAND_SPAWNS),
            settings_focus: cx.focus_handle(),
            settings_editor: None,
            font_cache: Arc::new(OnceLock::new()),
            icon_cache: Arc::new(OnceLock::new()),
            tab_icon_picker_focus: cx.focus_handle(),
            tab_icon_picker: None,
            theme_picker_focus: cx.focus_handle(),
            theme_picker: None,
            theme_picker_current: None,
            #[cfg(feature = "serial-console")]
            serial_console_focus: cx.focus_handle(),
            #[cfg(feature = "serial-console")]
            serial_console: None,
            #[cfg(feature = "serial-console")]
            serial_console_generation: 0,
            tab_search_focus: cx.focus_handle(),
            tab_search: None,
            minimized_panes_focus: cx.focus_handle(),
            pane_controls_visible_for: None,
            pane_controls_hidden_for: HashSet::new(),
            pane_controls_last_motion: Instant::now(),
            pane_controls_hide_task: None,
            pane_resize_mode: false,
            pane_resize_keys: PaneResizeKeys::default(),
            pane_resize_repeat_generation: 0,
            pane_resize_drag: None,
            pane_move_mode: false,
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
                        && !this.serial_console_is_open()
                        && this.session_authentication.is_none()
                        && this.tab_search.is_none()
                    {
                        this.focus_active(window, cx);
                    }
                }),
            ],
        };
        // Initialize font and icon caches in background
        let text_system = cx.text_system().clone();
        let font_cache = this.font_cache.clone();
        let icon_cache = this.icon_cache.clone();
        cx.background_executor()
            .spawn(async move {
                // Font cache
                let mut fonts = text_system.all_font_names();
                fonts.sort_by_key(|f| f.to_lowercase());
                fonts.dedup();
                font_cache
                    .set(FontCache {
                        fonts: fonts.into(),
                    })
                    .ok();

                // Icon cache
                let all_icons: Vec<ui::IconName> = ui::IconName::iter().collect();
                let entries: Arc<[IconEntry]> = build_icon_entries(&all_icons).into();
                icon_cache.set(IconCache { entries }).ok();
            })
            .detach();

        this.load_multi_command_catalog(cx);
        if let Some(profile) = initial_profile {
            this.open_tab_with_profile(profile, window, cx);
        } else {
            this.open_tab(window, cx);
        }
        this
    }

    pub(crate) fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_profile = self.tabs.get(self.active_tab).and_then(Tab::active_profile);
        let Some(profile) = new_tab_profile(
            active_profile,
            &self.profiles,
            self.launch_config.default_profile,
            self.launch_config.new_tab_profile,
        ) else {
            return;
        };
        self.open_tab_with_profile(profile, window, cx);
    }

    pub(crate) fn open_tab_with_profile(
        &mut self,
        mut profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        apply_launch_theme_override(&mut profile, self.launch_theme_override.as_ref());
        let active_pane = self.tabs.get(self.active_tab).and_then(Tab::active_pane);
        let inherit_working_directory = self
            .launch_config
            .working_directory_scope
            .inherits_for_new_tab();
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
        let inherited_wsl_directory = active_pane
            .filter(|_| inherit_working_directory)
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
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                [pane_id],
            ));
        self.tabs.push(Tab {
            id: tab_id,
            panes: vec![
                TerminalPane::new(pane_id, profile.clone())
                    .with_label_number(1)
                    .with_wsl_cwd_file(wsl_cwd_file.clone()),
            ],
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
            icon: self.launch_config.default_tab_icon,
            renaming_pane: None,
            rename_buffer: None,
            rename_cursor: 0,
            rename_select_all: false,
            editing_overlay_pane: None,
            overlay_buffer: None,
            overlay_cursor: 0,
            overlay_select_all: false,
            overlay_style_picker: None,
        });
        self.active_tab = self.tabs.len() - 1;

        // Stop the previously active terminal from driving the foreground executor before
        // starting the asynchronous PTY setup. Waiting for that setup to finish before the next
        // render leaves high-volume output fully active during the entire tab-spawn operation.
        for terminal in std::mem::take(&mut self.visible_terminals) {
            terminal.update(cx, |terminal, cx| terminal.set_ui_visible(false, cx));
        }
        cx.notify();

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
                        let input_enabled =
                            pane_input_enabled(this.pane_resize_mode || this.pane_move_mode);
                        view.update(cx, |view, cx| {
                            view.set_emit_input_events(emit_input_events);
                            view.set_input_enabled(input_enabled, cx);
                        });
                        cx.on_focus_in(&focus_handle, window, move |this, _, cx| {
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
        let tab_id = self.tabs[index].id;
        self.cancel_tab_search_for_tab(tab_id, cx);
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
        let closed_pane_ids = self.tabs[index]
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        self.forget_pane_controls(closed_pane_ids);
        self.tabs.remove(index);
        self.retain_open_visible_terminals();
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
            .filter_map(|pane| pane.terminal.clone())
        {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
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

    pub(crate) fn terminal_closed(
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

        // Closing a pane changes the dimensions of the survivors. Reflowing millions of retained
        // scrollback rows synchronously during the next paint can freeze the entire application.
        // A layout-driven resize only needs to truncate/grow rows; the shells redraw their live
        // prompts after receiving SIGWINCH.
        let surviving_terminals = self.tabs[tab_index]
            .panes
            .iter()
            .filter(|pane| pane.id != pane_id)
            .filter_map(|pane| pane.terminal.clone())
            .collect::<Vec<_>>();
        for terminal in surviving_terminals {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }

        self.cancel_tab_search_for_tab(tab_id, cx);
        let layout = {
            let tab = &mut self.tabs[tab_index];
            tab.remove_pane(pane_id);
            tab.layout.clone().without(pane_id)
        };
        self.forget_pane_controls([pane_id]);
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
    fn retain_open_visible_terminals(&mut self) {
        let open_terminals = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| pane.terminal.as_ref())
            .map(Entity::entity_id)
            .collect::<HashSet<_>>();
        self.visible_terminals
            .retain(|terminal| open_terminals.contains(&terminal.entity_id()));
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
        let inherit_working_directory = self
            .launch_config
            .working_directory_scope
            .inherits_for_new_pane();
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
            self.launch_config.working_directory_configured,
        );
        let terminals_resized_by_split = matches!(axis, SplitAxis::Vertical)
            .then(|| {
                tab.panes
                    .iter()
                    .filter_map(|pane| pane.terminal.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let wsl_cwd_file = wsl_cwd_tracking_file(&profile, pane_id);
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                [pane_id],
            ));

        // A vertical split changes terminal widths. Reflowing a large retained buffer during the
        // next paint blocks the UI before the new pane can appear. Preserve logical rows for this
        // layout-driven resize; each shell will redraw its live prompt after SIGWINCH.
        for terminal in terminals_resized_by_split {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }

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

    pub(crate) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    pub(crate) fn new_window(&mut self, _: &NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        open_zetta_window(
            self.launch_config.clone(),
            self.configuration_error.clone(),
            None,
            None,
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
        let Some(index) = visible_profile_index(
            &self.profiles,
            &self.launch_config.hidden_profiles,
            action.slot,
        ) else {
            return;
        };
        let profile = self.profiles[index].clone();
        self.open_tab_with_profile(profile, window, cx);
    }

    pub(crate) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active_tab, window, cx);
    }

    pub(crate) fn close_window(
        &mut self,
        _: &CloseWindow,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.remove_window();
    }

    pub(crate) fn minimize_window(
        &mut self,
        _: &MinimizeWindow,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if window.is_minimizable() {
            window.minimize_window();
        }
    }

    pub(crate) fn zoom_window(
        &mut self,
        _: &ZoomWindow,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if window.is_resizable() {
            window.zoom_window();
        }
    }

    pub(crate) fn close_all_windows(
        &mut self,
        _: &CloseAllWindows,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_window_id = window.window_handle().window_id();
        for window_handle in cx.windows() {
            if window_handle.window_id() == current_window_id {
                window.remove_window();
            } else {
                window_handle
                    .update(cx, |_, window, _| window.remove_window())
                    .log_err();
            }
        }
    }

    pub(crate) fn open_application_menu(
        &mut self,
        _: &OpenApplicationMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.application_menu_handle.show(window, cx);
    }

    fn title_bar_menu_handles(&self) -> [PopoverMenuHandle<ui::ContextMenu>; 2] {
        [
            self.application_menu_handle.clone(),
            self.profile_menu_handle.clone(),
        ]
    }

    fn navigate_application_menus(
        &mut self,
        direction: ApplicationMenuDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Keep auto-repeat from starting another handoff before the new menu
        // receives its deferred focus update.
        if self.application_menu_switch_pending {
            return;
        }

        // Keep the navigable menus in title-bar order. Adding a new top-level
        // menu only requires adding its handle here.
        let handles = self.title_bar_menu_handles();
        let Some(current_index) = handles
            .iter()
            .position(|handle| handle.is_focused(window, cx))
        else {
            cx.propagate();
            return;
        };
        let next_index = adjacent_application_menu_index(handles.len(), current_index, direction);

        // A popover restores its previous focus when dismissed. Hiding the
        // current menu before the next one has focus briefly returns focus to
        // the terminal, causing a visible pane redraw and allowing repeated
        // arrow keys to reach it. Open the replacement first, then dismiss
        // the current menu after the replacement's deferred focus update.
        self.application_menu_switch_pending = true;
        let current_handle = handles[current_index].clone();
        let next_handle = handles[next_index].clone();
        let zetta = cx.entity().downgrade();
        next_handle.show(window, cx);
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, cx| {
                current_handle.hide(cx);
                zetta
                    .update(cx, |this, _| this.application_menu_switch_pending = false)
                    .ok();
            });
        });
    }

    pub(crate) fn activate_application_menu_left(
        &mut self,
        _: &ActivateApplicationMenuLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_application_menus(ApplicationMenuDirection::Left, window, cx);
    }

    pub(crate) fn activate_application_menu_right(
        &mut self,
        _: &ActivateApplicationMenuRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_application_menus(ApplicationMenuDirection::Right, window, cx);
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
        let inherit_working_directory = self
            .launch_config
            .working_directory_scope
            .inherits_for_new_pane();
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
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
        let inherited_wsl_directory = active_pane
            .filter(|_| inherit_working_directory)
            .and_then(|pane| pane.wsl_working_directory(cx));
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
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                new_panes.iter().map(|(pane_id, _)| *pane_id),
            ));
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
            tab.push_pane(
                TerminalPane::new(*pane_id, profile.clone())
                    .with_wsl_cwd_file(wsl_cwd_file.clone()),
            );
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
        overlay.workload = options.workload;
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

    pub(crate) fn edit_config_file(
        &mut self,
        _: &EditConfigFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.launch_config.config_path.clone();
        self.edit_settings_file_in_active_pane(path, window, cx);
    }

    pub(crate) fn edit_keymap_file(
        &mut self,
        _: &EditKeymapFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.launch_config.keymap_path.clone();
        self.edit_settings_file_in_active_pane(path, window, cx);
    }

    /// Runs Zetta's editor dispatcher against the active pane's shell, mirroring how a
    /// clicked path or `EditScrollback` opens an editor: reused in place when the pane's
    /// foreground process is the shell, otherwise split into a fresh pane.
    fn edit_settings_file_in_active_pane(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let tab_id = tab.id;
        let Some(pane) = tab.active_pane() else {
            return;
        };
        let pane_id = pane.id;
        let Some(terminal) = pane.terminal.clone() else {
            return;
        };
        let (command, open_in_new_pane) = terminal.update(cx, |terminal, _| {
            (
                terminal.editor_command_for_path(&path, terminal.native_path_style()),
                terminal.editor_should_open_in_new_pane(),
            )
        });
        let Some(command) = command else {
            return;
        };
        if open_in_new_pane {
            self.open_editor_in_new_pane(
                tab_id,
                pane_id,
                terminal_view::EditorRequest {
                    command,
                    temporary_path: None,
                },
                window,
                cx,
            );
        } else {
            terminal.update(cx, |terminal, _| terminal.submit_editor_command(command));
        }
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
        let profile_count = visible_profile_count(&config.profiles, &config.hidden_profiles);
        load_keybindings(&config.keymap_path, profile_count, cx);

        #[cfg(windows)]
        windows_integration::update_profile_jump_list(config.profiles.clone());

        if config.pane_controls_hidden_by_default
            != self.launch_config.pane_controls_hidden_by_default
        {
            reset_pane_controls_visibility(
                &mut self.pane_controls_hidden_for,
                config.pane_controls_hidden_by_default,
                self.tabs
                    .iter()
                    .flat_map(|tab| tab.panes.iter().map(|pane| pane.id)),
            );
            self.pane_controls_visible_for = None;
        }
        self.profiles = config.profiles.clone();
        self.working_directory = config.working_directory.clone();
        self.launch_config = config;
        #[cfg(target_os = "macos")]
        update_native_macos_menus(
            cx,
            &self.profiles,
            &self.launch_config.hidden_profiles,
            self.launch_config.default_profile,
        );
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
    fn dismiss_tab_overflow_menus(&mut self, cx: &mut App) {
        if self.tab_overflow_keyboard_menu_edge.take().is_some() {
            self.tab_overflow_left_menu_handle.hide(cx);
            self.tab_overflow_right_menu_handle.hide(cx);
        }
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
        let side_is_right = index > self.active_tab;
        self.active_tab = index;
        self.tab_overflow_selection_side = Some(side_is_right);
        self.dismiss_tab_overflow_menus(cx);
        self.focus_active(window, cx);
    }

    pub(crate) fn rename_tab(
        &mut self,
        _: &RenameTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_tab_rename(self.active_tab, window, cx);
    }

    pub(crate) fn begin_tab_rename(
        &mut self,
        tab_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let automatic_title = self
            .tabs
            .get(tab_index)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
            .map(|view| view.read(cx).tab_content_text(0, cx).to_string())
            .or_else(|| {
                self.tabs
                    .get(tab_index)
                    .and_then(Tab::active_pane)
                    .map(|pane| pane.profile.name.clone())
            })
            .unwrap_or_else(|| "Terminal".to_owned());
        self.active_tab = tab_index;
        self.begin_rename_with_title(tab_index, automatic_title, window, cx);
    }

    pub(crate) fn begin_rename(
        &mut self,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let automatic_title = view.read(cx).tab_content_text(0, cx).to_string();
        self.begin_rename_with_title(self.active_tab, automatic_title, window, cx);
    }

    fn begin_rename_with_title(
        &mut self,
        tab_index: usize,
        automatic_title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tabs.get_mut(tab_index) {
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
        if self.pane_controls_hidden_for.contains(&pane_id) {
            return;
        }
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

    fn forget_pane_controls(&mut self, pane_ids: impl IntoIterator<Item = u64>) {
        for pane_id in pane_ids {
            self.pane_controls_hidden_for.remove(&pane_id);
            if self.pane_controls_visible_for == Some(pane_id) {
                self.pane_controls_visible_for = None;
            }
        }
    }

    fn toggle_pane_controls_for(&mut self, pane_ids: &[u64], cx: &mut Context<Self>) {
        if pane_ids.is_empty() {
            return;
        }
        if toggle_hidden_pane_controls(&mut self.pane_controls_hidden_for, pane_ids)
            && self
                .pane_controls_visible_for
                .is_some_and(|pane_id| pane_ids.contains(&pane_id))
        {
            self.pane_controls_visible_for = None;
        }
        cx.notify();
    }

    pub(crate) fn toggle_pane_controls(
        &mut self,
        _: &TogglePaneControls,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_id = self.tabs.get(self.active_tab).map(|tab| tab.active_pane);
        if let Some(pane_id) = pane_id {
            self.toggle_pane_controls_for(&[pane_id], cx);
        }
    }

    pub(crate) fn toggle_tab_pane_controls(
        &mut self,
        _: &ToggleTabPaneControls,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_ids = self
            .tabs
            .get(self.active_tab)
            .map(|tab| tab.panes.iter().map(|pane| pane.id).collect::<Vec<_>>())
            .unwrap_or_default();
        self.toggle_pane_controls_for(&pane_ids, cx);
    }

    pub(crate) fn is_renaming(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.rename_buffer.is_some())
    }

    pub(crate) fn is_editing_pane_overlay(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.overlay_buffer.is_some())
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
