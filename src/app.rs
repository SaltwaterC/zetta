use super::*;
use strum::IntoEnumIterator as _;
use zeroize::Zeroizing;

const PANE_CONTROLS_IDLE_DELAY: Duration = Duration::from_millis(1200);
const BACKGROUND_PROCESS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const PERFORMANCE_PANE_STRESS_COUNT: usize = 4;
const MINIMUM_PANE_COLUMNS: usize = 2;
const MINIMUM_PANE_ROWS: usize = 1;
const PANE_RESIZE_REPEAT_DELAY: Duration = Duration::from_millis(400);
const PANE_RESIZE_REPEAT_INTERVAL: Duration = Duration::from_millis(75);
const PANE_RESIZE_GUTTER_SIZE: Pixels = px(20.);
const PANE_SPLIT_SEPARATOR_SIZE: Pixels = px(1.);

/// Cached font enumeration for settings font picker
pub(crate) struct FontCache {
    pub fonts: Arc<[String]>,
}

/// Cached icon entries (icon + precomputed lowercase label) shared by the
/// tab icon picker, used both for per-tab icons and the config default icon.
pub(crate) struct IconCache {
    pub entries: Arc<[IconEntry]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReconnectRequest {
    None,
    Immediate(usize),
    Choose,
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

fn resize_cell_count(current: usize, delta: isize, minimum: usize) -> usize {
    if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs()).max(minimum)
    } else {
        current.saturating_add(delta as usize).max(minimum)
    }
}

fn pane_input_enabled(modal_pane_mode_active: bool) -> bool {
    !modal_pane_mode_active
}

fn pane_resize_menu_entry_available(pane_count: usize) -> bool {
    pane_count >= 2
}

fn pane_move_menu_entry_available(pane_count: usize) -> bool {
    pane_count >= 2
}

fn pane_resize_cell_delta(
    layout: &PaneLayout,
    pane_id: u64,
    axis: SplitAxis,
    directional_delta: isize,
) -> isize {
    if layout
        .resize_boundary(pane_id, axis)
        .is_some_and(|boundary| !boundary.active_is_first)
    {
        -directional_delta
    } else {
        directional_delta
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum PaneResizeDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PaneResizeDirection {
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }

    fn bit(self) -> u8 {
        match self {
            Self::Left => 1 << 0,
            Self::Right => 1 << 1,
            Self::Up => 1 << 2,
            Self::Down => 1 << 3,
        }
    }
}

#[derive(Default)]
struct PaneResizeKeys {
    pressed: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaneResizeGutter {
    tab_id: u64,
    first_pane: u64,
    second_pane: u64,
    axis: SplitAxis,
}

struct PaneResizeDrag {
    gutter: PaneResizeGutter,
    first_panes: Vec<u64>,
    second_panes: Vec<u64>,
}

/// Identifies a pane being dragged in pane-move mode. The same value serves
/// as both the drag payload (this pane is being dragged) and, when rendered
/// for a different pane, the drop target's identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaneMoveDrag {
    tab_id: u64,
    pane_id: u64,
}

impl PaneResizeKeys {
    /// Returns whether `direction` was newly pressed.
    fn press(&mut self, direction: PaneResizeDirection) -> bool {
        let bit = direction.bit();
        if self.pressed & bit == 0 {
            self.pressed |= bit;
            true
        } else {
            false
        }
    }

    fn release(&mut self, direction: PaneResizeDirection) {
        self.pressed &= !direction.bit();
    }

    fn clear(&mut self) {
        self.pressed = 0;
    }

    fn is_empty(&self) -> bool {
        self.pressed == 0
    }

    fn len(&self) -> u32 {
        self.pressed.count_ones()
    }

    fn delta(&self) -> (isize, isize) {
        let held = |direction: PaneResizeDirection| self.pressed & direction.bit() != 0;
        (
            (held(PaneResizeDirection::Right) as isize)
                - (held(PaneResizeDirection::Left) as isize),
            (held(PaneResizeDirection::Down) as isize) - (held(PaneResizeDirection::Up) as isize),
        )
    }
}

#[derive(Default)]
struct WindowResize {
    width_delta: f32,
    height_delta: f32,
}

impl WindowResize {
    fn add(&mut self, axis: SplitAxis, delta: f32) {
        match axis {
            SplitAxis::Vertical => self.width_delta += delta,
            SplitAxis::Horizontal => self.height_delta += delta,
        }
    }
}

fn minimum_resized_window_extent(current: f32, requested: f32, minimum: Pixels) -> f32 {
    requested.max(current.min(f32::from(minimum)))
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

fn resize_window(window: &mut Window, resize: WindowResize, cx: &App) -> bool {
    if resize.width_delta == 0. && resize.height_delta == 0. {
        return false;
    }
    let bounds = window.bounds();
    let current_width = f32::from(bounds.size.width);
    let current_height = f32::from(bounds.size.height);
    let mut requested_width = current_width + resize.width_delta;
    let mut requested_height = current_height + resize.height_delta;

    // Clamp programmatic resizes before issuing them so resize-mode keypresses
    // never produce an undersized window.
    requested_width = minimum_resized_window_extent(
        current_width,
        requested_width,
        ZETTA_MINIMUM_WINDOW_SIZE.width,
    );
    requested_height = minimum_resized_window_extent(
        current_height,
        requested_height,
        ZETTA_MINIMUM_WINDOW_SIZE.height,
    );

    if window.is_maximized() {
        let wants_growth =
            resize.width_delta.is_sign_positive() || resize.height_delta.is_sign_positive();
        if resize.width_delta.is_sign_positive() {
            requested_width = current_width;
        }
        if resize.height_delta.is_sign_positive() {
            requested_height = current_height;
        }
        if wants_growth {
            // A maximized window is pinned to the screen bounds, so growth has
            // nowhere to go. Un-maximize so the next resize can actually widen
            // or heighten the window, matching floating-window behavior.
            window.zoom_window();
        }
    } else if window.is_fullscreen() {
        if resize.width_delta.is_sign_positive() {
            requested_width = current_width;
        }
        if resize.height_delta.is_sign_positive() {
            requested_height = current_height;
        }
    }
    if (resize.width_delta.is_sign_positive() || resize.height_delta.is_sign_positive())
        && let Some(display) = window.display(cx)
    {
        let visible = display.visible_bounds();
        if resize.width_delta.is_sign_positive() {
            let maximum = f32::from(visible.right() - bounds.origin.x);
            requested_width = requested_width.min(maximum).max(current_width);
        }
        if resize.height_delta.is_sign_positive() {
            let maximum = f32::from(visible.bottom() - bounds.origin.y);
            requested_height = requested_height.min(maximum).max(current_height);
        }
    }
    if requested_width <= 0.
        || requested_height <= 0.
        || ((requested_width - current_width).abs() < f32::EPSILON
            && (requested_height - current_height).abs() < f32::EPSILON)
    {
        return false;
    }

    window.resize(size(px(requested_width), px(requested_height)));
    true
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
    pane_resize_keys: PaneResizeKeys,
    pane_resize_repeat_generation: u64,
    pane_resize_drag: Option<PaneResizeDrag>,
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
            tab.push_pane(TerminalPane {
                id: pane_id,
                label_number: 0,
                generated_label: Some(format!("Stress {:02}", index + 2)),
                custom_label: None,
                overlay_text: None,
                overlay_font_size: None,
                overlay_opacity: None,
                overlay_color: None,
                profile: profile.clone(),
                terminal: None,
                view: None,
                error: None,
                wsl_cwd_file: None,
                pending_command: None,
            });
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
            panes: vec![TerminalPane {
                id: pane_id,
                label_number: 1,
                generated_label: None,
                custom_label: None,
                overlay_text: None,
                overlay_font_size: None,
                overlay_opacity: None,
                overlay_color: None,
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
        tab.push_pane(TerminalPane {
            id: pane_id,
            label_number: 0,
            generated_label: None,
            custom_label: None,
            overlay_text: None,
            overlay_font_size: None,
            overlay_opacity: None,
            overlay_color: None,
            profile: profile.clone(),
            terminal: None,
            view: None,
            error: None,
            wsl_cwd_file: wsl_cwd_file.clone(),
            pending_command,
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
        let visible = self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.id == tab_id && tab.pane_is_visible(pane_id));
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
        cx.on_focus_in(&focus_handle, window, move |this, _, cx| {
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
            tab.push_pane(TerminalPane {
                id: *pane_id,
                label_number: 0,
                generated_label: None,
                custom_label: None,
                overlay_text: None,
                overlay_font_size: None,
                overlay_opacity: None,
                overlay_color: None,
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

    pub(crate) fn toggle_pane_resize_mode(
        &mut self,
        _: &TogglePaneResizeMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .get(self.active_tab)
            .is_none_or(|tab| tab.active_pane().is_none())
        {
            return;
        }
        self.pane_resize_mode = !self.pane_resize_mode;
        self.pane_resize_keys.clear();
        self.cancel_pane_resize_repeat();
        self.pane_resize_drag = None;
        if self.pane_resize_mode {
            self.pane_move_mode = false;
        }
        let input_enabled = pane_input_enabled(self.pane_resize_mode || self.pane_move_mode);
        for view in self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| pane.view.as_ref())
        {
            view.update(cx, |view, cx| view.set_input_enabled(input_enabled, cx));
        }
        cx.notify();
    }

    pub(crate) fn toggle_pane_move_mode(
        &mut self,
        _: &TogglePaneMoveMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .get(self.active_tab)
            .is_none_or(|tab| tab.active_pane().is_none())
        {
            return;
        }
        self.pane_move_mode = !self.pane_move_mode;
        if self.pane_move_mode {
            self.pane_resize_mode = false;
            self.pane_resize_keys.clear();
            self.cancel_pane_resize_repeat();
            self.pane_resize_drag = None;
        }
        let input_enabled = pane_input_enabled(self.pane_resize_mode || self.pane_move_mode);
        for view in self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| pane.view.as_ref())
        {
            view.update(cx, |view, cx| view.set_input_enabled(input_enabled, cx));
        }
        cx.notify();
    }

    pub(crate) fn move_pane_left(
        &mut self,
        _: &MovePaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_pane(PaneDirection::Left, window, cx);
    }

    pub(crate) fn move_pane_right(
        &mut self,
        _: &MovePaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_pane(PaneDirection::Right, window, cx);
    }

    pub(crate) fn move_pane_up(
        &mut self,
        _: &MovePaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_pane(PaneDirection::Up, window, cx);
    }

    pub(crate) fn move_pane_down(
        &mut self,
        _: &MovePaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_pane(PaneDirection::Down, window, cx);
    }

    fn move_active_pane(
        &mut self,
        direction: PaneDirection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_move_mode {
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.maximized_pane.is_some() {
            return;
        }
        if !tab.layout.move_pane(tab.active_pane, direction) {
            return;
        }
        for terminal in tab.panes.iter().filter_map(|pane| pane.terminal.as_ref()) {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }
        cx.notify();
    }

    fn move_pane_via_drag(
        &mut self,
        dragged: PaneMoveDrag,
        target: PaneMoveDrag,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_move_mode
            || dragged.tab_id != target.tab_id
            || dragged.pane_id == target.pane_id
        {
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == target.tab_id) else {
            return;
        };
        if tab.maximized_pane.is_some() {
            return;
        }
        if !tab.layout.swap_panes(dragged.pane_id, target.pane_id) {
            return;
        }
        tab.activate_pane(dragged.pane_id);
        for terminal in tab.panes.iter().filter_map(|pane| pane.terminal.as_ref()) {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }
        cx.notify();
    }

    pub(crate) fn resize_pane_left(
        &mut self,
        _: &ResizePaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_active_pane_in_direction(PaneResizeDirection::Left, window, cx);
    }

    pub(crate) fn resize_pane_right(
        &mut self,
        _: &ResizePaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_active_pane_in_direction(PaneResizeDirection::Right, window, cx);
    }

    pub(crate) fn resize_pane_up(
        &mut self,
        _: &ResizePaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_active_pane_in_direction(PaneResizeDirection::Up, window, cx);
    }

    pub(crate) fn resize_pane_down(
        &mut self,
        _: &ResizePaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_active_pane_in_direction(PaneResizeDirection::Down, window, cx);
    }

    pub(crate) fn pane_resize_key_up(
        &mut self,
        event: &KeyUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.pane_resize_mode
            && let Some(direction) = PaneResizeDirection::from_key(&event.keystroke.key)
        {
            self.pane_resize_keys.release(direction);
            if self.pane_resize_keys.is_empty() {
                self.cancel_pane_resize_repeat();
            }
        }
    }

    fn resize_active_pane_in_direction(
        &mut self,
        direction: PaneResizeDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_resize_mode {
            return;
        }
        let held_key_count = self.pane_resize_keys.len();
        if !self.pane_resize_keys.press(direction) {
            // Preserve the platform's native repeat for an ordinary one-key
            // resize. Synthetic repeat is only needed once multiple held keys
            // must be combined into a single two-axis operation.
            if held_key_count == 1 {
                let (columns_delta, rows_delta) = match direction {
                    PaneResizeDirection::Left => (-1, 0),
                    PaneResizeDirection::Right => (1, 0),
                    PaneResizeDirection::Up => (0, -1),
                    PaneResizeDirection::Down => (0, 1),
                };
                self.resize_active_pane_by_cells(columns_delta, rows_delta, window, cx);
            }
            return;
        }
        self.resize_active_pane_by_held_keys(window, cx);
        if held_key_count == 1 {
            self.start_pane_resize_repeat(window, cx);
        }
    }

    fn resize_active_pane_by_held_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (columns_delta, rows_delta) = self.pane_resize_keys.delta();
        if columns_delta == 0 && rows_delta == 0 {
            return;
        }
        self.resize_active_pane_by_cells(columns_delta, rows_delta, window, cx);
    }

    fn start_pane_resize_repeat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane_resize_repeat_generation = self.pane_resize_repeat_generation.wrapping_add(1);
        let generation = self.pane_resize_repeat_generation;
        let this = cx.entity().downgrade();
        let executor = cx.background_executor().clone();
        window
            .spawn(cx, async move |cx| {
                executor.timer(PANE_RESIZE_REPEAT_DELAY).await;
                loop {
                    let repeating = this
                        .update_in(cx, |this, window, cx| {
                            let repeating = this.pane_resize_mode
                                && this.pane_resize_repeat_generation == generation
                                && !this.pane_resize_keys.is_empty();
                            if repeating {
                                this.resize_active_pane_by_held_keys(window, cx);
                            }
                            repeating
                        })
                        .unwrap_or(false);
                    if !repeating {
                        break;
                    }
                    executor.timer(PANE_RESIZE_REPEAT_INTERVAL).await;
                }
            })
            .detach();
    }

    fn cancel_pane_resize_repeat(&mut self) {
        self.pane_resize_repeat_generation = self.pane_resize_repeat_generation.wrapping_add(1);
    }

    pub(crate) fn resize_pane_gutter_drag(
        &mut self,
        gutter: PaneResizeGutter,
        split_bounds: Bounds<Pixels>,
        pointer_position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_resize_mode {
            return;
        }
        if self
            .pane_resize_drag
            .as_ref()
            .is_none_or(|drag| drag.gutter != gutter)
            && !self.begin_pane_resize_drag(gutter)
        {
            return;
        }

        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == gutter.tab_id) else {
            return;
        };
        let Some(drag) = self.pane_resize_drag.as_ref() else {
            return;
        };
        let (split_start, split_extent, pointer_coordinate) = match gutter.axis {
            SplitAxis::Vertical => (
                split_bounds.left(),
                split_bounds.size.width,
                pointer_position.x,
            ),
            SplitAxis::Horizontal => (
                split_bounds.top(),
                split_bounds.size.height,
                pointer_position.y,
            ),
        };
        let available_extent = f32::from(split_extent) - f32::from(PANE_SPLIT_SEPARATOR_SIZE);
        if available_extent <= 0. {
            return;
        }
        let Some(first_ratio) = self.tabs[tab_index].layout.split_ratio(
            gutter.first_pane,
            gutter.second_pane,
            gutter.axis,
        ) else {
            return;
        };

        let current_first_extent = available_extent * first_ratio;
        let requested_first_extent = (f32::from(pointer_coordinate - split_start)
            - f32::from(PANE_SPLIT_SEPARATOR_SIZE) / 2.)
            .clamp(0., available_extent);
        let first_capacity =
            self.minimum_pane_capacity(tab_index, &drag.first_panes, gutter.axis, cx);
        let second_capacity =
            self.minimum_pane_capacity(tab_index, &drag.second_panes, gutter.axis, cx);
        let layout_delta =
            (requested_first_extent - current_first_extent).clamp(-first_capacity, second_capacity);
        if layout_delta == 0. {
            return;
        }

        if self.tabs[tab_index].layout.adjust_split_ratio(
            gutter.first_pane,
            gutter.second_pane,
            gutter.axis,
            layout_delta / available_extent,
        ) {
            // A gutter drag changes terminal geometry just like keyboard pane
            // resizing, so defer scrollback reflow until that resize arrives.
            for terminal in self.tabs[tab_index]
                .panes
                .iter()
                .filter_map(|pane| pane.terminal.as_ref())
            {
                terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
            }
            cx.notify();
        }
    }

    fn begin_pane_resize_drag(&mut self, gutter: PaneResizeGutter) -> bool {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == gutter.tab_id) else {
            return false;
        };
        if tab.maximized_pane.is_some() || !tab.minimized_panes.is_empty() {
            return false;
        }
        let Some((first_panes, second_panes)) =
            tab.layout
                .split_panes(gutter.first_pane, gutter.second_pane, gutter.axis)
        else {
            return false;
        };
        self.pane_resize_drag = Some(PaneResizeDrag {
            gutter,
            first_panes,
            second_panes,
        });
        true
    }

    fn resize_active_pane_by_cells(
        &mut self,
        columns_delta: isize,
        rows_delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane_resize_mode {
            return;
        }
        let (tab_id, pane_id, bounds, columns_delta, rows_delta) = {
            let Some(tab) = self.tabs.get_mut(self.active_tab) else {
                return;
            };
            // The terminal focus is authoritative while a keybinding is being
            // handled. This also protects against focus notifications that
            // have not yet updated Tab::active_pane.
            let pane_id = tab
                .panes
                .iter()
                .find(|pane| {
                    pane.view
                        .as_ref()
                        .is_some_and(|view| view.focus_handle(cx).contains_focused(window, cx))
                })
                .map(|pane| pane.id)
                .unwrap_or(tab.active_pane);
            tab.activate_pane(pane_id);
            let Some(bounds) = tab
                .pane(pane_id)
                .and_then(|pane| pane.terminal.as_ref())
                .map(|terminal| terminal.read(cx).last_content().terminal_bounds)
            else {
                return;
            };
            let layout = tab.visible_layout();
            let columns_delta = layout.as_ref().map_or(columns_delta, |layout| {
                pane_resize_cell_delta(layout, pane_id, SplitAxis::Vertical, columns_delta)
            });
            let rows_delta = layout.as_ref().map_or(rows_delta, |layout| {
                pane_resize_cell_delta(layout, pane_id, SplitAxis::Horizontal, rows_delta)
            });
            (tab.id, pane_id, bounds, columns_delta, rows_delta)
        };
        let columns = resize_cell_count(bounds.num_columns(), columns_delta, MINIMUM_PANE_COLUMNS);
        let rows = resize_cell_count(bounds.num_lines(), rows_delta, MINIMUM_PANE_ROWS);
        self.resize_pane_to(tab_id, pane_id, Some(columns), Some(rows), window, cx);
    }

    fn resize_pane_to(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        columns: Option<usize>,
        rows: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(bounds) = self.tabs[tab_index]
            .pane(pane_id)
            .and_then(|pane| pane.terminal.as_ref())
            .map(|terminal| terminal.read(cx).last_content().terminal_bounds)
        else {
            return;
        };

        let mut changed = false;
        let mut window_resize = WindowResize::default();
        if let Some(columns) = columns {
            let (layout_changed, window_delta) = self.resize_pane_axis(
                tab_index,
                pane_id,
                SplitAxis::Vertical,
                columns.max(MINIMUM_PANE_COLUMNS),
                bounds.num_columns(),
                bounds.cell_width(),
                cx,
            );
            changed |= layout_changed;
            window_resize.add(SplitAxis::Vertical, window_delta);
        }
        if let Some(rows) = rows {
            let (layout_changed, window_delta) = self.resize_pane_axis(
                tab_index,
                pane_id,
                SplitAxis::Horizontal,
                rows.max(MINIMUM_PANE_ROWS),
                bounds.num_lines(),
                bounds.line_height(),
                cx,
            );
            changed |= layout_changed;
            window_resize.add(SplitAxis::Horizontal, window_delta);
        }
        changed |= resize_window(window, window_resize, cx);
        if changed {
            // The next terminal size change is driven by pane geometry, so do
            // not synchronously reflow retained scrollback for every keypress.
            if let Some(tab) = self.tabs.get(tab_index) {
                for terminal in tab.panes.iter().filter_map(|pane| pane.terminal.as_ref()) {
                    terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
                }
            }
            cx.notify();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resize_pane_axis(
        &mut self,
        tab_index: usize,
        pane_id: u64,
        axis: SplitAxis,
        requested_cells: usize,
        current_cells: usize,
        cell_size: Pixels,
        cx: &mut Context<Self>,
    ) -> (bool, f32) {
        if requested_cells == current_cells {
            return (false, 0.);
        }
        let requested_delta =
            (requested_cells as isize - current_cells as isize) as f32 * f32::from(cell_size);
        let (active_region, boundary, can_adjust_layout) = {
            let tab = &self.tabs[tab_index];
            let Some(layout) = tab.visible_layout() else {
                return (false, 0.);
            };
            let Some(region) = layout
                .regions()
                .into_iter()
                .find(|region| region.id == pane_id)
            else {
                return (false, 0.);
            };
            let region_fraction = match axis {
                SplitAxis::Vertical => region.right - region.left,
                SplitAxis::Horizontal => region.bottom - region.top,
            };
            (
                region_fraction,
                layout.resize_boundary(pane_id, axis),
                tab.maximized_pane.is_none() && tab.minimized_panes.is_empty(),
            )
        };
        if active_region <= f32::EPSILON {
            return (false, 0.);
        }

        let current_pixels = current_cells as f32 * f32::from(cell_size);
        let root_pixels = current_pixels / active_region;
        let mut remaining_delta = requested_delta;
        let mut changed = false;

        if can_adjust_layout && let Some(boundary) = boundary {
            let parent_pixels = root_pixels * boundary.parent_fraction;
            if parent_pixels > 0. {
                let layout_delta = if requested_delta.is_sign_positive() {
                    let available =
                        self.minimum_pane_capacity(tab_index, &boundary.sibling_panes, axis, cx);
                    requested_delta.min(available)
                } else {
                    requested_delta
                };
                if layout_delta != 0.
                    && self.tabs[tab_index].layout.adjust_resize_boundary(
                        pane_id,
                        axis,
                        layout_delta / parent_pixels,
                    )
                {
                    remaining_delta -= layout_delta;
                    changed = true;
                }
            }
        }

        let window_delta = if remaining_delta != 0. {
            let target_fraction = (active_region
                + (requested_delta - remaining_delta) / root_pixels)
                .max(f32::EPSILON);
            remaining_delta / target_fraction
        } else {
            0.
        };
        (changed, window_delta)
    }

    fn minimum_pane_capacity(
        &self,
        tab_index: usize,
        sibling_panes: &[u64],
        axis: SplitAxis,
        cx: &App,
    ) -> f32 {
        sibling_panes
            .iter()
            .filter_map(|pane_id| self.tabs[tab_index].pane(*pane_id))
            .filter_map(|pane| pane.terminal.as_ref())
            .map(|terminal| {
                let bounds = terminal.read(cx).last_content().terminal_bounds;
                let (available, minimum) = match axis {
                    SplitAxis::Vertical => (
                        f32::from(bounds.width()),
                        f32::from(bounds.cell_width()) * MINIMUM_PANE_COLUMNS as f32,
                    ),
                    SplitAxis::Horizontal => (
                        f32::from(bounds.height()),
                        f32::from(bounds.line_height()) * MINIMUM_PANE_ROWS as f32,
                    ),
                };
                (available - minimum).max(0.)
            })
            .reduce(f32::min)
            .unwrap_or(0.)
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

    pub(crate) fn change_tab_icon(
        &mut self,
        _: &ChangeTabIcon,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_tab_icon_picker(self.active_tab, window, cx);
    }

    pub(crate) fn set_active_tab_icon_from_cli(
        &mut self,
        icon: Option<IconName>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return false;
        };
        tab.icon = icon;
        cx.notify();
        true
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

    /// Shared icon entries (icon + precomputed lowercase label) backing the
    /// tab icon picker, whether opened for a specific tab or for the
    /// config default icon. Reads the background-populated cache, falling
    /// back to a lazily-computed set if it isn't ready yet.
    pub(crate) fn tab_icon_entries(&self) -> Arc<[IconEntry]> {
        self.icon_cache
            .get()
            .map(|cache| cache.entries.clone())
            .unwrap_or_else(fallback_icon_entries)
    }

    pub(crate) fn open_tab_icon_picker(
        &mut self,
        tab_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if tab_index >= self.tabs.len() {
            return;
        }
        let current_icon = self.tabs.get(tab_index).and_then(|tab| tab.icon);
        let entries = self.tab_icon_entries();
        self.tab_icon_picker = Some(TabIconPicker::new(
            TabIconPickerTarget::Tab(tab_index),
            current_icon,
            &entries,
        ));
        if let Some(picker) = self.tab_icon_picker.as_ref() {
            picker.scroll.scroll_to_item(picker.selected);
        }
        self.tab_icon_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_default_tab_icon_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_editor.is_none() {
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.focused_control = Some(SettingsControl::DefaultTabIconPicker);
            editor.focused_input = None;
        }
        let current_icon = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.configuration.default_tab_icon);
        let entries = self.tab_icon_entries();
        self.tab_icon_picker = Some(TabIconPicker::new(
            TabIconPickerTarget::Default,
            current_icon,
            &entries,
        ));
        if let Some(picker) = self.tab_icon_picker.as_ref() {
            picker.scroll.scroll_to_item(picker.selected);
        }
        self.tab_icon_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_tab_icon_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.tab_icon_picker.take().map(|picker| picker.target);
        if target == Some(TabIconPickerTarget::Default) {
            self.settings_focus.focus(window, cx);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn set_tab_icon(
        &mut self,
        icon: Option<IconName>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.tab_icon_picker.as_ref() else {
            return;
        };
        match picker.target {
            TabIconPickerTarget::Tab(tab_index) => {
                if let Some(tab) = self.tabs.get_mut(tab_index) {
                    tab.icon = icon;
                }
            }
            TabIconPickerTarget::Default => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.configuration.default_tab_icon = icon;
                    editor.configuration_dirty = true;
                    editor.message = None;
                }
            }
        }
        self.dismiss_tab_icon_picker(window, cx);
    }

    pub(crate) fn tab_icon_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        if self.tab_icon_picker.is_none() {
            return;
        }
        cx.stop_propagation();
        if event.keystroke.key == "escape" {
            self.dismiss_tab_icon_picker(window, cx);
            return;
        }
        let entries = self.tab_icon_entries();
        let activate = {
            let mut activate = None;
            let mut query_changed = false;
            let mut selection_changed = false;
            let Some(picker) = self.tab_icon_picker.as_mut() else {
                return;
            };
            let options = picker.options(&entries).to_vec();
            match event.keystroke.key.as_str() {
                "left" if !command => picker.query.move_left(),
                "right" if !command => picker.query.move_right(),
                "up" if !command => {
                    picker.selected = picker.selected.saturating_sub(7);
                    selection_changed = true;
                }
                "down" if !command => {
                    picker.selected = (picker.selected + 7).min(options.len().saturating_sub(1));
                    selection_changed = true;
                }
                "tab" if !command => {
                    if event.keystroke.modifiers.shift {
                        picker.selected = picker.selected.saturating_sub(1);
                    } else {
                        picker.selected =
                            (picker.selected + 1).min(options.len().saturating_sub(1));
                    }
                    selection_changed = true;
                }
                "enter" if !command => {
                    activate = options.get(picker.selected).copied();
                }
                "backspace" => {
                    picker.query.backspace();
                    query_changed = true;
                }
                "delete" => {
                    picker.query.delete();
                    query_changed = true;
                }
                "home" => {
                    picker.query.cursor = 0;
                    picker.query.select_all = false;
                }
                "end" => {
                    picker.query.cursor = picker.query.text.len();
                    picker.query.select_all = false;
                }
                "a" if command => picker.query.select_all(),
                _ if !command
                    && !event.keystroke.modifiers.alt
                    && event.keystroke.key_char.is_some() =>
                {
                    if let Some(text) = event.keystroke.key_char.as_ref() {
                        picker.query.insert(text);
                        query_changed = true;
                    }
                }
                _ => return,
            }
            if query_changed {
                picker.selected = 0;
                selection_changed = true;
            }
            if selection_changed {
                picker.scroll.scroll_to_item(picker.selected);
            }
            activate
        };
        if let Some(icon) = activate {
            self.set_tab_icon(icon, window, cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn change_pane_theme(
        &mut self,
        _: &ChangePaneTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_pane_theme_picker(window, cx);
    }

    pub(crate) fn open_pane_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Falls back to the global theme when the pane has neither an
        // override nor a profile theme, so the tick mark always lands on
        // whichever theme is actually showing, not just an active override.
        let current_theme_name = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
            .and_then(|view| view.read(cx).theme().cloned())
            .or_else(|| Some(cx.theme().clone()))
            .map(|theme| theme.name.to_string());
        let mut theme_names = ThemeRegistry::global(cx)
            .list()
            .into_iter()
            .map(|theme| theme.name.to_string())
            .collect::<Vec<_>>();
        theme_names.sort();
        theme_names.dedup();
        let reset_command = PaletteCommand {
            name: "Reset to profile default".to_owned(),
            shortcut: None,
            action: Box::new(ResetPaneTheme),
        };
        let theme_commands = theme_names
            .into_iter()
            .map(|name| PaletteCommand {
                name: name.clone(),
                shortcut: None,
                action: Box::new(ApplyPaneTheme { name }),
            })
            .collect();
        let mut picker = CommandPalette::with_pinned_first(reset_command, theme_commands);
        if let Some(index) = current_theme_name.as_deref().and_then(|current| {
            picker
                .commands
                .iter()
                .position(|command| command.name == current)
        }) {
            picker.selected = index;
        }
        picker.scroll_to_selected();
        self.theme_picker_current = current_theme_name;
        self.theme_picker = Some(picker);
        self.theme_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_pane_theme_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.theme_picker = None;
        self.theme_picker_current = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn run_pane_theme_picker_command(
        &mut self,
        command_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = self
            .theme_picker
            .as_ref()
            .and_then(|palette| palette.commands.get(command_index))
            .map(|command| command.action.boxed_clone());
        self.theme_picker = None;
        self.focus_active(window, cx);
        if let Some(action) = action {
            window.dispatch_action(action, cx);
        }
        cx.notify();
    }

    pub(crate) fn theme_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.theme_picker.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_pane_theme_picker(window, cx),
            "up" => {
                picker.selected = picker.selected.saturating_sub(1);
                picker.scroll_to_selected();
                cx.notify();
            }
            "down" => {
                picker.selected =
                    (picker.selected + 1).min(picker.matches().len().saturating_sub(1));
                picker.scroll_to_selected();
                cx.notify();
            }
            "enter" => {
                let command = picker.matches().get(picker.selected).copied();
                if let Some(command) = command {
                    self.run_pane_theme_picker_command(command, window, cx);
                }
            }
            "backspace" => {
                if picker.select_all {
                    picker.query.clear();
                    picker.cursor = 0;
                    picker.refresh_matches();
                } else if picker.cursor > 0 {
                    let previous = previous_char_boundary(&picker.query, picker.cursor);
                    picker.query.replace_range(previous..picker.cursor, "");
                    picker.cursor = previous;
                    picker.refresh_matches();
                }
                picker.select_all = false;
                picker.selected = 0;
                picker.scroll_to_selected();
                cx.notify();
            }
            "delete" => {
                if picker.select_all {
                    picker.query.clear();
                    picker.cursor = 0;
                    picker.refresh_matches();
                } else if picker.cursor < picker.query.len() {
                    let next = next_char_boundary(&picker.query, picker.cursor);
                    picker.query.replace_range(picker.cursor..next, "");
                    picker.refresh_matches();
                }
                picker.select_all = false;
                picker.selected = 0;
                picker.scroll_to_selected();
                cx.notify();
            }
            "left" => {
                picker.cursor = if picker.select_all {
                    0
                } else {
                    previous_char_boundary(&picker.query, picker.cursor)
                };
                picker.select_all = false;
                cx.notify();
            }
            "right" => {
                picker.cursor = if picker.select_all {
                    picker.query.len()
                } else {
                    next_char_boundary(&picker.query, picker.cursor)
                };
                picker.select_all = false;
                cx.notify();
            }
            "home" => {
                picker.cursor = 0;
                picker.select_all = false;
                cx.notify();
            }
            "end" => {
                picker.cursor = picker.query.len();
                picker.select_all = false;
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                picker.select_all = !picker.query.is_empty();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if picker.select_all {
                        picker.query.clear();
                        picker.cursor = 0;
                        picker.select_all = false;
                    }
                    picker.query.insert_str(picker.cursor, text);
                    picker.cursor += text.len();
                    picker.refresh_matches();
                    picker.selected = 0;
                    picker.scroll_to_selected();
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    pub(crate) fn apply_pane_theme(
        &mut self,
        action: &ApplyPaneTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_active_pane_theme(Some(action.name.clone()), cx);
    }

    pub(crate) fn reset_pane_theme(
        &mut self,
        _: &ResetPaneTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_active_pane_theme(None, cx);
    }

    /// Applies `theme_name` to the active pane's terminal view only: it never
    /// touches `pane.profile`, `self.profiles`, or the configuration file, so
    /// the change is lost on tab close or the next configuration reload.
    /// `None` restores whatever theme the pane's profile is configured with.
    /// Shared by the interactive picker and the `panetheme` CLI command.
    pub(crate) fn set_active_pane_theme(
        &mut self,
        theme_name: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((profile, view)) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.clone().map(|view| (pane.profile.clone(), view)))
        else {
            return false;
        };
        let theme = match theme_name {
            Some(name) => match ThemeRegistry::global(cx).get(&name) {
                Ok(theme) => Some(with_zetta_theme_overrides(theme)),
                Err(_) => return false,
            },
            None => resolve_profile_theme(&profile, cx).ok().flatten(),
        };
        view.update(cx, |view, cx| view.set_theme(theme, cx));
        cx.notify();
        true
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

    pub(crate) fn set_pane_overlay(
        &mut self,
        _: &SetPaneOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.tabs.get(self.active_tab).map(|tab| tab.active_pane) else {
            return;
        };
        self.begin_pane_overlay_edit(pane_id, window, cx);
    }

    pub(crate) fn reset_pane_overlay(
        &mut self,
        _: &ResetPaneOverlay,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_active_pane_overlay(None, None, None, None, cx);
    }

    pub(crate) fn begin_pane_overlay_edit(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(pane) = tab.pane(pane_id) else {
            return;
        };
        let text = pane.overlay_text.clone().unwrap_or_default();
        tab.activate_pane(pane_id);
        tab.editing_overlay_pane = Some(pane_id);
        tab.overlay_cursor = text.len();
        tab.overlay_buffer = Some(text);
        tab.overlay_select_all = true;
        self.rename_focus.focus(window, cx);
        cx.notify();
    }

    /// Sets the active pane's overlay text and style directly, bypassing the
    /// inline edit buffer. Shared by the `overlay` CLI command and its
    /// process-control handler; never touches `config.json`, so the overlay
    /// is lost when the pane closes or the configuration reloads. `text:
    /// None` clears the overlay along with its style. Every call fully
    /// replaces the previous text and style rather than merging with it.
    pub(crate) fn set_active_pane_overlay(
        &mut self,
        text: Option<String>,
        font_size: Option<OverlayFontSize>,
        opacity: Option<f32>,
        color: Option<gpui::Hsla>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return false;
        };
        let active_pane = tab.active_pane;
        let Some(pane) = tab.pane_mut(active_pane) else {
            return false;
        };
        pane.overlay_text = text;
        pane.overlay_font_size = font_size;
        pane.overlay_opacity = opacity;
        pane.overlay_color = color;
        cx.notify();
        true
    }

    /// Whether the overlay-style selector is open for the active tab.
    pub(crate) fn is_picking_overlay_style(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.overlay_style_picker.is_some())
    }

    /// Opens the overlay-style selector for `pane_id`, seeded with the pane's
    /// current font size, colour, and opacity (the colour falling back to the
    /// theme's text colour) and focused for keyboard adjustment. Everything
    /// previews changes live on the pane; nothing is committed until
    /// [`Self::apply_overlay_style_picker`] runs.
    pub(crate) fn begin_overlay_style_picker(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let Some(pane) = tab.pane(pane_id) else {
            return;
        };
        let current_color = pane.overlay_color.unwrap_or(cx.theme().colors().text);
        let mut picker = OverlayStylePicker {
            pane_id,
            section: OverlayPickerSection::FontSize,
            font_size: pane.overlay_font_size.unwrap_or(OverlayFontSize::DEFAULT),
            original_font_size: pane.overlay_font_size,
            hue: 0.,
            saturation: 0.,
            value: 1.,
            original_color: pane.overlay_color,
            opacity_percent: OverlayStylePicker::percent_for_opacity(pane.overlay_opacity),
            original_opacity: pane.overlay_opacity,
            hex_buffer: String::new(),
        };
        let (hue, saturation, value) = overlay_picker_hsv_from_hsla(current_color);
        picker.set_color(hue, saturation, value);
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.overlay_style_picker = Some(picker);
        }
        self.overlay_style_focus.focus(window, cx);
        cx.notify();
    }

    /// Copies the picker's current font size, colour, and opacity to its
    /// pane, previewing the selection live without committing the picker.
    fn preview_overlay_style(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(picker) = tab.overlay_style_picker.as_mut() else {
            return;
        };
        let pane_id = picker.pane_id;
        let font_size = picker.font_size;
        let color = picker.color();
        let opacity = picker.opacity_percent as f32 / 100.;
        if let Some(pane) = tab.pane_mut(pane_id) {
            pane.overlay_font_size = Some(font_size);
            pane.overlay_color = Some(color);
            pane.overlay_opacity = Some(opacity);
        }
        cx.notify();
    }

    /// The section of the overlay-style selector the keyboard adjusts.
    pub(crate) fn set_overlay_picker_section(
        &mut self,
        section: OverlayPickerSection,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        if picker.section == section {
            return;
        }
        picker.section = section;
        cx.notify();
    }

    /// Cycles the keyboard-adjusted overlay-style section by `delta` steps.
    pub(crate) fn adjust_overlay_picker_section(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        let next = picker.section.step(delta);
        if picker.section == next {
            return;
        }
        picker.section = next;
        cx.notify();
    }

    /// Selects the exact overlay font size and previews it on the affected
    /// pane; does not commit the picker.
    pub(crate) fn set_overlay_font_size(
        &mut self,
        font_size: OverlayFontSize,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        if picker.font_size == font_size {
            return;
        }
        picker.font_size = font_size;
        self.preview_overlay_style(cx);
    }

    /// Cycles the overlay font size by `delta` sizes.
    pub(crate) fn adjust_overlay_font_size(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(next) = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_ref())
            .map(|picker| picker.font_size.step(delta))
        else {
            return;
        };
        self.set_overlay_font_size(next, cx);
    }

    /// Selects the exact overlay colour in HSV space, normalized like the
    /// picker's hue bar and saturation/brightness square, and previews it on
    /// the affected pane; does not commit the picker.
    pub(crate) fn set_overlay_color_hsv(
        &mut self,
        hue: f32,
        saturation: f32,
        value: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        let hue = hue.rem_euclid(1.);
        let saturation = saturation.clamp(0., 1.);
        let value = value.clamp(0., 1.);
        if (picker.hue - hue).abs() < f32::EPSILON
            && (picker.saturation - saturation).abs() < f32::EPSILON
            && (picker.value - value).abs() < f32::EPSILON
        {
            return;
        }
        picker.set_color(hue, saturation, value);
        self.preview_overlay_style(cx);
    }

    /// Rotates the overlay colour's hue by `delta` turns.
    pub(crate) fn adjust_overlay_hue(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        picker.adjust_hue(delta);
        self.preview_overlay_style(cx);
    }

    /// Moves the overlay colour's saturation by `delta`.
    pub(crate) fn adjust_overlay_saturation(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        picker.adjust_saturation(delta);
        self.preview_overlay_style(cx);
    }

    /// Moves the overlay colour's brightness by `delta`.
    pub(crate) fn adjust_overlay_value(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        picker.adjust_value(delta);
        self.preview_overlay_style(cx);
    }

    /// Feeds one hex digit into the overlay colour's hex field; once a full
    /// `#rrggbb` colour is complete it is previewed on the pane.
    pub(crate) fn overlay_hex_input(&mut self, ch: char, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        if picker.hex_input(ch) {
            self.preview_overlay_style(cx);
        } else {
            cx.notify();
        }
    }

    /// Backspaces the overlay colour's hex field; a now-complete colour is
    /// applied to the pane.
    pub(crate) fn overlay_hex_backspace(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        if picker.hex_backspace() {
            self.preview_overlay_style(cx);
        } else {
            cx.notify();
        }
    }

    /// Highlights the exact `percent` in the overlay-opacity slider and
    /// previews it on the affected pane; does not commit the picker.
    pub(crate) fn set_overlay_opacity_percent(&mut self, percent: usize, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        let percent = percent.clamp(0, 100);
        if picker.opacity_percent == percent {
            return;
        }
        picker.opacity_percent = percent;
        self.preview_overlay_style(cx);
    }

    /// Nudges the highlighted overlay opacity by `delta` percentage points.
    pub(crate) fn adjust_overlay_opacity_percent(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        let next = (picker.opacity_percent as isize + delta).clamp(0, 100) as usize;
        if next == picker.opacity_percent {
            return;
        }
        picker.opacity_percent = next;
        self.preview_overlay_style(cx);
    }

    /// Commits the picker's font size, colour, and opacity to the pane and
    /// closes the selector.
    pub(crate) fn apply_overlay_style_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(picker) = tab.overlay_style_picker.take() else {
            return;
        };
        if let Some(pane) = tab.pane_mut(picker.pane_id) {
            pane.overlay_font_size = Some(picker.font_size);
            pane.overlay_color = Some(picker.color());
            pane.overlay_opacity = Some(picker.opacity_percent as f32 / 100.);
        }
        self.focus_active(window, cx);
    }

    /// Closes the overlay-style selector and restores the pane's font size,
    /// colour, and opacity from before it opened.
    pub(crate) fn cancel_overlay_style_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(picker) = tab.overlay_style_picker.take() else {
            return;
        };
        if let Some(pane) = tab.pane_mut(picker.pane_id) {
            pane.overlay_font_size = picker.original_font_size;
            pane.overlay_color = picker.original_color;
            pane.overlay_opacity = picker.original_opacity;
        }
        self.focus_active(window, cx);
    }

    /// Applies the overlay's text and proceeds straight to the live style
    /// selector in the same palette-driven flow. Skips the picker when the
    /// entered text was empty (the overlay was cleared).
    pub(crate) fn commit_overlay_text_then_pick_style(
        &mut self,
        pane_id: u64,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let has_text = text.is_some();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(pane) = tab.pane_mut(pane_id) {
                pane.overlay_text = text;
            }
            tab.overlay_buffer = None;
            tab.overlay_select_all = false;
        }
        if has_text {
            self.begin_overlay_style_picker(pane_id, window, cx);
        } else {
            self.focus_active(window, cx);
        }
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_pane_layout(
        &self,
        tab: &Tab,
        layout: &PaneLayout,
        colors: &ThemeColors,
        error_color: gpui::Hsla,
        window: &Window,
        owns_window_bottom: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let edges = PaneWindowEdges::all().with_bottom(owns_window_bottom);
        let corner_radii = edges.client_corner_radii(window);
        div()
            .when(self.pane_resize_mode, |layout| {
                layout.key_context("PaneResize")
            })
            .when(self.pane_move_mode, |layout| layout.key_context("PaneMove"))
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex_grow_1()
            .flex_basis(gpui::relative(0.))
            .overflow_hidden()
            // Use one opaque surface behind every pane layout. This fills
            // terminal-grid margins and pane separators consistently while
            // retaining the outer client-window corners.
            .when(corner_radii.bottom_left > Pixels::ZERO, |layout| {
                layout.rounded_bl(corner_radii.bottom_left)
            })
            .when(corner_radii.bottom_right > Pixels::ZERO, |layout| {
                layout.rounded_br(corner_radii.bottom_right)
            })
            .bg(colors.border)
            .child(self.render_pane_layout_with_edges(
                tab,
                layout,
                colors,
                error_color,
                window,
                edges,
                cx,
            ))
            .into_any_element()
    }

    fn render_pane_resize_gutter(
        &self,
        gutter: PaneResizeGutter,
        first_ratio: f32,
        colors: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let cursor = match gutter.axis {
            SplitAxis::Vertical => CursorStyle::ResizeLeftRight,
            SplitAxis::Horizontal => CursorStyle::ResizeUpDown,
        };
        div()
            .id(format!(
                "pane-resize-gutter-{}-{}-{}",
                gutter.tab_id, gutter.first_pane, gutter.second_pane
            ))
            .absolute()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .hover(|gutter| gutter.bg(colors.element_hover))
            .cursor(cursor)
            .when(matches!(gutter.axis, SplitAxis::Vertical), |gutter| {
                gutter
                    .left(gpui::relative(first_ratio))
                    .ml(-PANE_RESIZE_GUTTER_SIZE / 2.)
                    .w(PANE_RESIZE_GUTTER_SIZE)
                    .h_full()
            })
            .when(matches!(gutter.axis, SplitAxis::Horizontal), |gutter| {
                gutter
                    .top(gpui::relative(first_ratio))
                    .mt(-PANE_RESIZE_GUTTER_SIZE / 2.)
                    .h(PANE_RESIZE_GUTTER_SIZE)
                    .w_full()
            })
            .on_drag(gutter, |_, _, _, cx| cx.new(|_| gpui::Empty))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_pane_layout_with_edges(
        &self,
        tab: &Tab,
        layout: &PaneLayout,
        colors: &ThemeColors,
        error_color: gpui::Hsla,
        window: &Window,
        edges: PaneWindowEdges,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match layout {
            PaneLayout::Pane(pane_id) => {
                let Some(pane) = tab.pane(*pane_id) else {
                    return div().size_full().into_any_element();
                };
                let corner_radii = edges.client_corner_radii(window);
                let pane_label = tab
                    .displayed_pane_label(*pane_id)
                    .unwrap_or_else(|| pane.label());
                let pane_overlay = tab.displayed_pane_overlay(*pane_id);
                let pane_terminal = pane.terminal.as_ref();
                let pane_size = pane_terminal.map(|terminal| {
                    let bounds = terminal.read(cx).last_content().terminal_bounds;
                    terminal_size_label(bounds.num_columns(), bounds.num_lines())
                });
                let pane_label_selected = tab.renaming_pane == Some(*pane_id)
                    && tab.rename_select_all
                    && tab.rename_buffer.is_some();
                let pane_overlay_editing = tab.editing_overlay_pane == Some(*pane_id);
                let pane_overlay_font_size =
                    pane.overlay_font_size.unwrap_or(OverlayFontSize::DEFAULT);
                let pane_overlay_base_opacity =
                    pane.overlay_opacity.unwrap_or(DEFAULT_OVERLAY_OPACITY);
                let pane_overlay_color = pane.overlay_color.unwrap_or(colors.text);
                let pane_overlay_top = match pane_overlay_font_size {
                    // The line box sits on the glyph's internal leading
                    // (measured: 6px at `sm`, 14px at `3xl`), so each size
                    // offsets by `overlay_pane_inset() - leading(size)` to keep
                    // the visible gap to the pane edge constant.
                    OverlayFontSize::Small => px(8.),
                    OverlayFontSize::Base => px(7.),
                    OverlayFontSize::Large => px(6.),
                    OverlayFontSize::ExtraLarge => px(5.),
                    OverlayFontSize::ExtraExtraLarge => px(3.),
                    OverlayFontSize::ExtraExtraExtraLarge => px(0.),
                };
                let active = pane.view.as_ref().is_some_and(|view| {
                    view.focus_handle(cx).is_focused(window)
                        || view.read(cx).has_open_context_menu()
                        || view.read(cx).has_open_search()
                        || self.tab_search.as_ref().is_some_and(|search| {
                            search.tab_id == tab.id && tab.active_pane == *pane_id
                        })
                }) || (pane.view.is_none() && tab.active_pane == *pane_id);
                let pane_resize_toggle_action = pane_resize_menu_entry_available(tab.panes.len())
                    .then(|| Box::new(TogglePaneResizeMode) as Box<dyn Action>);
                let pane_move_toggle_action = pane_move_menu_entry_available(tab.panes.len())
                    .then(|| Box::new(TogglePaneMoveMode) as Box<dyn Action>);
                let content = match (&pane.view, &pane.error) {
                    (Some(view), _) => {
                        view.update(cx, |view, cx| {
                            view.set_window_corner_radii(corner_radii, cx);
                            view.set_pane_resize_mode_entry(
                                self.pane_resize_mode,
                                pane_resize_toggle_action,
                            );
                            view.set_pane_move_mode_entry(
                                self.pane_move_mode,
                                pane_move_toggle_action,
                            );
                        });
                        div().size_full().child(view.clone()).into_any_element()
                    }
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
                    .child(
                        div()
                            .size_full()
                            .when(!active, |pane| {
                                pane.opacity(self.launch_config.inactive_pane_opacity)
                            })
                            .child(content),
                    )
                    .when_some(
                        self.pane_resize_mode.then_some(pane_size.clone()).flatten(),
                        |pane, pane_size| {
                            pane.child(
                                div()
                                    .absolute()
                                    .right(px(6.))
                                    .bottom(px(6.))
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.status_bar_background)
                                    .text_sm()
                                    .text_color(colors.text)
                                    .child(format!("{pane_label} {pane_size}")),
                            )
                        },
                    )
                    .when(self.pane_move_mode, |pane| {
                        let overlay_label = if tab.active_pane == *pane_id {
                            format!("{pane_label} Move mode")
                        } else {
                            pane_label.clone()
                        };
                        pane.child(
                            div()
                                .absolute()
                                .right(px(6.))
                                .bottom(px(6.))
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .bg(colors.status_bar_background)
                                .text_sm()
                                .text_color(colors.text)
                                .child(overlay_label),
                        )
                    })
                    .when_some(pane_overlay, |pane, overlay| {
                        pane.child(
                            div()
                                .id(("terminal-pane-overlay", *pane_id as usize))
                                .absolute()
                                .right(px(14.))
                                .top(pane_overlay_top)
                                .max_w(px(320.))
                                .map(|element| match pane_overlay_font_size {
                                    OverlayFontSize::Small => element.text_sm(),
                                    OverlayFontSize::Base => element.text_base(),
                                    OverlayFontSize::Large => element.text_lg(),
                                    OverlayFontSize::ExtraLarge => element.text_xl(),
                                    OverlayFontSize::ExtraExtraLarge => element.text_2xl(),
                                    OverlayFontSize::ExtraExtraExtraLarge => element.text_3xl(),
                                })
                                .text_color(pane_overlay_color)
                                .opacity(if pane_overlay_editing {
                                    1.
                                } else {
                                    pane_overlay_base_opacity
                                })
                                .overflow_hidden()
                                .child(overlay),
                        )
                    })
                    .when(
                        tab.maximized_pane.is_none()
                            && (tab.renaming_pane == Some(*pane_id)
                                || (tab.panes.len() > 1
                                    && self.pane_controls_visible_for == Some(*pane_id))),
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
                                            .tooltip(Tooltip::for_action_title(
                                                pane_label_tooltip,
                                                &RenamePane,
                                            ))
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
                                    .when(tab.panes.len() > 1, |controls| {
                                        controls
                                            .when_some(pane_size.clone(), |controls, pane_size| {
                                                controls.child(
                                                    Label::new(pane_size)
                                                        .size(LabelSize::Small)
                                                        .color(Color::Custom(colors.text_muted)),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(
                                                        IconButton::new(
                                                            (
                                                                "minimize-terminal-pane",
                                                                *pane_id as usize,
                                                            ),
                                                            IconName::Dash,
                                                        )
                                                        .style(ButtonStyle::Transparent)
                                                        .size(ButtonSize::Compact)
                                                        .icon_size(IconSize::XSmall)
                                                        .icon_color(Color::Custom(colors.icon))
                                                        .aria_label("Minimize pane")
                                                        .tooltip(Tooltip::for_action_title(
                                                            "Minimize pane",
                                                            &MinimizePane,
                                                        ))
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
                                                            (
                                                                "maximize-terminal-pane",
                                                                *pane_id as usize,
                                                            ),
                                                            IconName::Maximize,
                                                        )
                                                        .style(ButtonStyle::Transparent)
                                                        .size(ButtonSize::Compact)
                                                        .icon_size(IconSize::XSmall)
                                                        .icon_color(Color::Custom(colors.icon))
                                                        .aria_label("Maximize pane")
                                                        .tooltip(Tooltip::for_action_title(
                                                            "Maximize pane",
                                                            &ToggleMaximizePane,
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
                                                .tooltip(Tooltip::for_action_title(
                                                    "Close pane",
                                                    &ClosePane,
                                                ))
                                                .on_click(move |_, window, cx| {
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
                                                }),
                                            )
                                    }),
                            )
                        },
                    )
                    .when(
                        self.pane_move_mode && tab.panes.len() > 1 && tab.maximized_pane.is_none(),
                        |pane| {
                            let pane_move_drag = PaneMoveDrag {
                                tab_id: tab.id,
                                pane_id: *pane_id,
                            };
                            // A dedicated top-most overlay, rather than handlers on the
                            // pane itself, so `occlude` can block every mouse
                            // interaction with the terminal underneath (selection,
                            // clicks, scroll) while move mode is active: the pane must
                            // act as a plain drag handle, not a terminal.
                            pane.child(
                                div()
                                    .id(("pane-move-drag-surface", *pane_id as usize))
                                    .absolute()
                                    .inset_0()
                                    .cursor(CursorStyle::OpenHand)
                                    .occlude()
                                    .on_drag(pane_move_drag, |_, _, _, cx| cx.new(|_| gpui::Empty))
                                    .on_drop(cx.listener(
                                        move |this, dragged: &PaneMoveDrag, _window, cx| {
                                            this.move_pane_via_drag(*dragged, pane_move_drag, cx);
                                        },
                                    )),
                            )
                        },
                    )
                    .into_any_element()
            }
            PaneLayout::Split {
                axis,
                first_ratio,
                first,
                second,
            } => {
                let first_ratio = PaneLayout::ratio_fraction(*first_ratio);
                let second_ratio = 1. - first_ratio;
                let pane_resize_enabled = self.pane_resize_mode
                    && tab.maximized_pane.is_none()
                    && tab.minimized_panes.is_empty();
                let gutter = PaneResizeGutter {
                    tab_id: tab.id,
                    first_pane: first.first_pane(),
                    second_pane: second.first_pane(),
                    axis: *axis,
                };
                let first_child = div()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow(first_ratio)
                    .flex_basis(gpui::relative(0.))
                    .child(self.render_pane_layout_with_edges(
                        tab,
                        first,
                        colors,
                        error_color,
                        window,
                        edges.first(*axis),
                        cx,
                    ));
                let second_child = div()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow(second_ratio)
                    .flex_basis(gpui::relative(0.))
                    .child(self.render_pane_layout_with_edges(
                        tab,
                        second,
                        colors,
                        error_color,
                        window,
                        edges.second(*axis),
                        cx,
                    ));
                let split = div()
                    .relative()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow_1()
                    .flex_basis(gpui::relative(0.))
                    .flex()
                    .when(matches!(axis, SplitAxis::Horizontal), |split| {
                        split.flex_col()
                    })
                    .gap_px();
                if pane_resize_enabled {
                    split
                        .on_drag_move::<PaneResizeGutter>(cx.listener(
                            move |this, event: &gpui::DragMoveEvent<PaneResizeGutter>, _, cx| {
                                if *event.drag(cx) == gutter {
                                    this.resize_pane_gutter_drag(
                                        gutter,
                                        event.bounds,
                                        event.event.position,
                                        cx,
                                    );
                                }
                            },
                        ))
                        .child(first_child)
                        .child(second_child)
                        .child(self.render_pane_resize_gutter(gutter, first_ratio, colors, cx))
                        .into_any_element()
                } else {
                    split
                        .child(first_child)
                        .child(second_child)
                        .into_any_element()
                }
            }
        }
    }
}

#[cfg(not(feature = "serial-console"))]
impl Zetta {
    pub(crate) fn toggle_serial_console(
        &mut self,
        _: &ToggleSerialConsole,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.configuration_error = Some("Serial console support is disabled in this build".into());
        cx.notify();
    }
}

#[cfg(not(feature = "http-server"))]
impl Zetta {
    pub(crate) fn start_http_server(
        &mut self,
        _: &StartHttpServer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.configuration_error = Some("HTTP server support is disabled in this build".into());
        cx.notify();
    }
}

#[cfg(not(feature = "tftp-server"))]
impl Zetta {
    pub(crate) fn start_tftp_server(
        &mut self,
        _: &StartTftpServer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.configuration_error = Some("TFTP server support is disabled in this build".into());
        cx.notify();
    }
}

#[inline]
fn server_input_stops_server(
    input: &TerminalInput,
    is_http_server: bool,
    is_tftp_server: bool,
) -> bool {
    let _ = (input, is_http_server, is_tftp_server);
    #[cfg(feature = "http-server")]
    if is_http_server && crate::http_server_ui::http_input_stops_server(input) {
        return true;
    }
    #[cfg(feature = "tftp-server")]
    if is_tftp_server && crate::tftp_server_ui::tftp_input_stops_server(input) {
        return true;
    }
    false
}

#[derive(Clone, Copy, Default)]
struct PaneWindowEdges {
    right: bool,
    bottom: bool,
    left: bool,
}

impl PaneWindowEdges {
    const fn all() -> Self {
        Self {
            right: true,
            bottom: true,
            left: true,
        }
    }

    const fn with_bottom(mut self, bottom: bool) -> Self {
        self.bottom = bottom;
        self
    }

    fn first(self, axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::Horizontal => Self {
                bottom: false,
                ..self
            },
            SplitAxis::Vertical => Self {
                right: false,
                ..self
            },
        }
    }

    fn second(self, axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::Horizontal => self,
            SplitAxis::Vertical => Self {
                left: false,
                ..self
            },
        }
    }

    fn client_corner_radii(self, window: &Window) -> gpui::Corners<Pixels> {
        if !cfg!(any(target_os = "linux", target_os = "freebsd")) {
            return gpui::Corners::default();
        }
        let Decorations::Client { tiling } = window.window_decorations() else {
            return gpui::Corners::default();
        };
        let radius = theme::CLIENT_SIDE_DECORATION_ROUNDING - px(1.);

        // The title and tab bars own the top window corners. A terminal pane
        // can only meet the client frame at the bottom, so applying top radii
        // here creates an internal gap above a pane (and in split layouts).
        gpui::Corners {
            top_left: Pixels::ZERO,
            top_right: Pixels::ZERO,
            bottom_right: if self.bottom && self.right && !tiling.bottom && !tiling.right {
                radius
            } else {
                Pixels::ZERO
            },
            bottom_left: if self.bottom && self.left && !tiling.bottom && !tiling.left {
                radius
            } else {
                Pixels::ZERO
            },
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
