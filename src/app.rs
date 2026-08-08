use super::*;
use strum::IntoEnumIterator as _;

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
                    tab.pane_is_visible(pane.id)
                        .then(|| pane.terminal.clone())
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

    pub(crate) fn hide_window(
        &mut self,
        _: &HideWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cfg!(target_os = "macos") {
            cx.hide();
        } else if window.is_minimizable() {
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
}

impl Drop for Zetta {
    fn drop(&mut self) {
        if self.performance_overlay.is_some() {
            disable_frame_tracing();
        }
    }
}

#[cfg(test)]
#[path = "tests/app.rs"]
mod tests;
