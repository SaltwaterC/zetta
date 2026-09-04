//! The `Zetta` view: the window's state, and the lifecycle of what it holds.
//!
//! The root owns the struct itself, how a window is built, resumed and closed,
//! and the free predicates its actions decide from. The actions are grouped by
//! what they act on:
//!
//! - `tabs.rs` — opening, closing, pinning and ordering tabs.
//! - `panes.rs` — splitting, closing and focusing panes, and terminal exits.
//! - `pane_templates.rs` — applying a template, and `--replace-pane`.
//! - `window_actions.rs` — window actions and application-menu navigation.
//! - `attention.rs` — routing an attention ID to the tab that owns it.

use super::*;
use crate::command_panes::{PaneCommand, quote_pane_command_for_shell};
use crate::configuration_reload::ConfigurationReloadFeedback;
use crate::process_control::ReplacePaneRequest;
use strum::IntoEnumIterator as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabDropPosition {
    Before(u64),
    After(u64),
    /// No tab surface was hit. The tab bar does not construct this during a
    /// normal drag, but it keeps the outside-drop no-op explicit — which is
    /// why only the sidecar builds one, to pin that the drop is a no-op.
    #[cfg_attr(not(test), allow(dead_code))]
    Outside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabMoveDirection {
    Left,
    Right,
}

/// Whether a new tab continues the current session or enters a project.
///
/// A tab opened inside the session inherits the active pane's directory when
/// `working_directory_scope` asks for it. Entering a project does not: the point
/// of opening a project is to start in the project's own working directory, so
/// inheriting would carry whatever directory the interactive session happened to
/// be sitting in into the new project tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewTabOrigin {
    CurrentSession,
    ProjectEntry,
}

pub(crate) enum TerminalLaunch {
    Spawn,
    #[cfg(windows)]
    Handoff(crate::windows_integration::WindowsHandoffRequest),
}

impl NewTabOrigin {
    fn inherits_working_directory(self, scope: WorkingDirectoryScope) -> bool {
        match self {
            Self::CurrentSession => scope.inherits_for_new_tab(),
            Self::ProjectEntry => false,
        }
    }
}

fn reorder_items_by_id<T>(
    items: &mut Vec<T>,
    source_id: u64,
    position: TabDropPosition,
    active_id: u64,
    item_id: impl Fn(&T) -> u64,
) -> Option<usize> {
    let (target_id, insert_after) = match position {
        TabDropPosition::Before(target_id) => (target_id, false),
        TabDropPosition::After(target_id) => (target_id, true),
        TabDropPosition::Outside => return None,
    };
    let source_index = items.iter().position(|item| item_id(item) == source_id)?;
    let target_index = items.iter().position(|item| item_id(item) == target_id)?;
    if !items.iter().any(|item| item_id(item) == active_id) {
        return None;
    }

    if source_index == target_index {
        return None;
    }

    // The target's index is measured before removing the source. Adjust it when
    // the source was before the target so the insertion remains relative to the
    // same stable target item.
    let target_index_after_removal = target_index - (source_index < target_index) as usize;
    let insertion_index = target_index_after_removal + insert_after as usize;
    if insertion_index == source_index {
        return None;
    }

    let item = items.remove(source_index);
    items.insert(insertion_index, item);

    // The active item is identified before the move and found again afterward,
    // so moving either the active or an inactive tab preserves logical focus.
    items.iter().position(|item| item_id(item) == active_id)
}

fn move_item_by_id<T>(
    items: &mut Vec<T>,
    source_id: u64,
    direction: TabMoveDirection,
    active_id: u64,
    enabled: bool,
    item_id: impl Fn(&T) -> u64,
) -> Option<usize> {
    if !enabled {
        return None;
    }

    let source_index = items.iter().position(|item| item_id(item) == source_id)?;
    let target_id = match direction {
        TabMoveDirection::Left => source_index
            .checked_sub(1)
            .and_then(|target_index| items.get(target_index))
            .map(&item_id),
        TabMoveDirection::Right => source_index
            .checked_add(1)
            .and_then(|target_index| items.get(target_index))
            .map(&item_id),
    }?;
    let position = match direction {
        TabMoveDirection::Left => TabDropPosition::Before(target_id),
        TabMoveDirection::Right => TabDropPosition::After(target_id),
    };
    reorder_items_by_id(items, source_id, position, active_id, item_id)
}

fn tab_overflow_selection_side(selected_index: usize, active_index: usize) -> Option<bool> {
    (selected_index != active_index).then_some(selected_index > active_index)
}

pub(crate) fn pinned_tab_count(tabs: &[Tab]) -> usize {
    tabs.iter().take_while(|tab| tab.pinned).count()
}

pub(crate) fn tab_drop_preserves_pinning(
    tabs: &[Tab],
    source_id: u64,
    position: TabDropPosition,
) -> bool {
    let Some(source_pinned) = tabs
        .iter()
        .find(|tab| tab.id == source_id)
        .map(|tab| tab.pinned)
    else {
        return false;
    };
    let target_id = match position {
        TabDropPosition::Before(target_id) | TabDropPosition::After(target_id) => target_id,
        TabDropPosition::Outside => return false,
    };
    tabs.iter()
        .find(|tab| tab.id == target_id)
        .is_some_and(|tab| tab.pinned == source_pinned)
}

fn tab_move_preserves_pinning(tabs: &[Tab], index: usize, direction: TabMoveDirection) -> bool {
    let Some(tab) = tabs.get(index) else {
        return false;
    };
    let target_index = match direction {
        TabMoveDirection::Left => index.checked_sub(1),
        TabMoveDirection::Right => index.checked_add(1),
    };
    target_index
        .and_then(|target_index| tabs.get(target_index))
        .is_some_and(|target| target.pinned == tab.pinned)
}

pub(crate) fn insert_tab_in_pin_order(tabs: &mut Vec<Tab>, tab: Tab) -> usize {
    let insertion_index = if tab.pinned {
        pinned_tab_count(tabs)
    } else {
        tabs.len()
    };
    tabs.insert(insertion_index, tab);
    insertion_index
}

pub(crate) fn toggle_tab_pinning_in_order(tabs: &mut Vec<Tab>, index: usize) -> Option<usize> {
    if index >= tabs.len() {
        return None;
    }
    let mut tab = tabs.remove(index);
    tab.pinned = !tab.pinned;
    let insertion_index = pinned_tab_count(tabs);
    tabs.insert(insertion_index, tab);
    Some(insertion_index)
}

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

/// How long a transient notice stays on screen. Longer than the configuration
/// reload's confirmation, because a notice is usually a sentence of guidance
/// rather than a word of acknowledgement, and has to be readable in one pass.
pub(crate) const TRANSIENT_NOTICE_DURATION: Duration = Duration::from_secs(8);

/// A short-lived informational banner, shown and then taken away again.
///
/// Separate from `configuration_error` and `pane_output_error`, which stay until
/// something replaces them: those report a state the user has to act on, whereas
/// this reports something that has just happened, or advice about what to do
/// instead. Leaving that kind of message on screen makes it read as an unresolved
/// error and gives the user no way to clear it.
///
/// The generation is what stops an earlier notice's timer from taking a later
/// notice away with it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TransientNotice {
    message: Option<String>,
    generation: u64,
}

impl TransientNotice {
    fn show(&mut self, message: String) -> u64 {
        self.message = Some(message);
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn dismiss_if_current(&mut self, generation: u64) -> bool {
        if self.generation != generation || self.message.is_none() {
            return false;
        }
        self.message = None;
        true
    }

    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl Zetta {
    /// Says something once, and takes it back down again.
    ///
    /// For anything the user does not have to act on: a confirmation that a
    /// toggle took effect, or a note that the thing they asked for has to be
    /// done somewhere else.
    /// Resolves automatic session protection again after configuration changed,
    /// or for the first time when doing it inline would have meant waiting on
    /// the network.
    ///
    /// The window is left without automatic protection while a fetch is in
    /// flight, which means a tab detached in that window asks for a secret
    /// instead. That is the safe direction to fail: the session is still
    /// protected, just by something the user typed.
    #[cfg(feature = "session-persistence")]
    pub(crate) fn refresh_auto_protect(&mut self, cx: &mut Context<Self>) {
        self.auto_protect_generation = self.auto_protect_generation.wrapping_add(1);
        let generation = self.auto_protect_generation;
        let persistence = self.launch_config.sessions.persistence.clone();
        if !crate::session_auto_protect::SessionAutoProtect::resolution_is_blocking(&persistence) {
            let mut error = None;
            self.auto_protect = resolve_auto_protect(&self.launch_config, &mut error);
            if let Some(error) = error {
                self.configuration_error = Some(error);
            }
            return;
        }
        self.auto_protect = None;
        cx.spawn(async move |this, cx| {
            let resolved = cx
                .background_spawn(async move {
                    crate::session_auto_protect::SessionAutoProtect::resolve(&persistence)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.auto_protect_generation != generation {
                    return;
                }
                match resolved {
                    Ok(auto_protect) => this.auto_protect = auto_protect.map(std::sync::Arc::new),
                    Err(error) => this.show_notice(
                        format!("Could not set up automatic session protection: {error:#}"),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn show_notice(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        let generation = self.transient_notice.show(message.into());
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(TRANSIENT_NOTICE_DURATION).await;
            this.update(cx, |this, cx| {
                if this.transient_notice.dismiss_if_current(generation) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn background_authentication_for_close(
    policy: &TabClosePolicy,
    shared: bool,
    allow_background: bool,
    failed_pane: bool,
) -> Option<Option<SessionAuthentication>> {
    if allow_background && !failed_pane {
        policy
            .background_authentication()
            .or_else(|| shared.then_some(None))
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
pub(crate) fn apply_launch_theme_override(
    profile: &mut Profile,
    launch_theme_override: Option<&(String, String)>,
) {
    if let Some((override_name, override_theme)) = launch_theme_override
        && profile.name.to_lowercase() == *override_name
    {
        profile.theme = Some(override_theme.clone());
        profile.dark_theme = Some(override_theme.clone());
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

/// Resolves automatic session protection, folding a failure into the
/// configuration error the window already shows.
///
/// A failure is surfaced rather than ignored because it changes what happens the
/// next time a tab is detached: without it, the secret dialog appears for a user
/// who asked never to see it again, and the reason would otherwise be invisible.
#[cfg(feature = "session-persistence")]
pub(crate) fn resolve_auto_protect(
    config: &Config,
    configuration_error: &mut Option<String>,
) -> Option<std::sync::Arc<crate::session_auto_protect::SessionAutoProtect>> {
    match crate::session_auto_protect::SessionAutoProtect::resolve(&config.sessions.persistence) {
        Ok(auto_protect) => auto_protect.map(std::sync::Arc::new),
        Err(error) => {
            let message = format!("Could not set up automatic session protection: {error:#}");
            *configuration_error = Some(match configuration_error.take() {
                Some(existing) => format!("{existing}\n{message}"),
                None => message,
            });
            None
        }
    }
}

pub(crate) struct Zetta {
    pub(crate) launch_config: Config,
    /// The recipients and identity that protect a background session with the
    /// user's age key rather than a dialog. `None` when automatic protection is
    /// off, or configured without something it needs.
    ///
    /// Resolved when configuration is loaded and kept, because a `github:`
    /// recipient is a network fetch and detaching a tab is not the moment to
    /// make one. Behind an `Arc` so the background thread that runs Argon2id can
    /// hold it without borrowing the window.
    #[cfg(feature = "session-persistence")]
    pub(crate) auto_protect:
        Option<std::sync::Arc<crate::session_auto_protect::SessionAutoProtect>>,
    /// Bumped by every refresh, so a resolution that went to the network cannot
    /// land after a later configuration reload and reinstate what it replaced.
    #[cfg(feature = "session-persistence")]
    pub(crate) auto_protect_generation: u64,
    pub(crate) project_detection_base: Arc<Config>,
    pub(crate) projects: ProjectState,
    /// A `--profile`/`--theme` launch override: (profile name lowercased,
    /// theme name). Applied in `open_tab_with_profile` to every tab opened
    /// with that profile for the rest of this process, never written back to
    /// `launch_config`/`profiles` or the settings UI.
    pub(crate) launch_theme_override: Option<(String, String)>,
    /// Incremented by successful configuration reloads. Serialized live
    /// sessions from an older generation in this process must not restore
    /// pane-theme overrides that the reload cleared.
    pub(crate) configuration_generation: u64,
    pub(crate) configuration_error: Option<String>,
    pub(crate) configuration_reload_feedback: ConfigurationReloadFeedback,
    pub(crate) pane_output_error: Option<String>,
    pub(crate) pane_output_save_in_progress: bool,
    pub(crate) transient_notice: TransientNotice,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) background_sessions: BackgroundSessionRunner<Tab>,
    /// The multiplexer that owns every pane's process, connected on first use.
    /// `None` until then. Normal launches require the daemon; `--no-mux` is an
    /// explicit compatibility escape hatch for the legacy in-process owner.
    pub(crate) mux: Option<MuxRuntime>,
    #[cfg(feature = "session-persistence")]
    /// Invalidates a disk-recovery task whenever the effective configuration or
    /// the runtime it belongs to changes.
    pub(crate) mux_recovery_generation: u64,
    #[cfg(feature = "session-persistence")]
    pub(crate) mux_recovery_task: Option<Task<()>>,
    pub(crate) no_mux: bool,
    pub(crate) mux_panes: MuxPanes,
    /// The panes this window shows in shared mode, keyed by pane id. A shared
    /// pane's terminal reads a relayed byte stream rather than the pty, so the
    /// shared connection and the sizes that arrive on it live here.
    pub(crate) shared_panes: HashMap<u64, crate::mux::SharedPaneEntry>,
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
    pub(crate) remote_session_focus: gpui::FocusHandle,
    pub(crate) remote_session_picker: Option<crate::remote_session_ui::RemoteSessionPicker>,
    pub(crate) remote_session_target: Option<zmux::remote::RemoteTarget>,
    #[cfg(feature = "session-persistence")]
    /// Public age ciphertext kept while a remote automatically protected
    /// session is being unlocked. The recovered session secret itself never
    /// lives in app state; it stays in `SessionSecret`/`Zeroizing` values.
    pub(crate) remote_session_key_envelope: Option<String>,
    pub(crate) close_confirmation_focus: gpui::FocusHandle,
    pub(crate) close_tab_confirmation: Option<CloseTabConfirmation>,
    pub(crate) active_tab: usize,
    /// Focuses the active pane while its selected terminal view is being
    /// replaced or is still starting. The corresponding render node carries
    /// the `Terminal` key context so terminal-scoped actions remain routed to
    /// Zetta during that transition.
    pub(crate) terminal_placeholder_focus: gpui::FocusHandle,
    pub(crate) visible_terminals: Vec<Entity<Terminal>>,
    pub(crate) profiles: Vec<Profile>,
    /// How many `ctrl-shift-{number}` profile shortcuts are currently bound.
    /// A project can add, hide, or unhide profiles, so the effective slot count
    /// changes while the window runs; see `refresh_profile_shortcuts`.
    pub(crate) profile_shortcut_slots: usize,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) next_tab_id: u64,
    pub(crate) next_attention_id: u64,
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
    pub(crate) multi_command_mode: CommandPromptMode,
    pub(crate) multi_command_catalog: CompletionCatalog,
    pub(crate) multi_command_launches: BoundedLaunchQueue<QueuedTerminalLaunch>,
    /// Render boundaries created on first use; see `view_boundary`.
    pub(crate) title_bar_chrome_view: Option<Entity<ZettaSubview>>,
    pub(crate) settings_surface_view: Option<Entity<ZettaSubview>>,
    pub(crate) tab_icon_picker_view: Option<Entity<ZettaSubview>>,
    pub(crate) settings_page_view: Option<Entity<ZettaSubview>>,
    pub(crate) settings_focus: gpui::FocusHandle,
    pub(crate) settings_editor: Option<SettingsEditor>,
    pub(crate) settings_loading: bool,
    pub(crate) settings_pending_page: Option<SettingsPage>,
    pub(crate) font_cache: Arc<OnceLock<FontCache>>,
    pub(crate) icon_cache: Arc<OnceLock<IconCache>>,
    pub(crate) tab_icon_picker_focus: gpui::FocusHandle,
    pub(crate) tab_icon_picker: Option<TabIconPicker>,
    pub(crate) theme_picker_focus: gpui::FocusHandle,
    pub(crate) theme_picker: Option<CommandPalette>,
    /// Scope whose selector is currently open.
    pub(crate) theme_picker_scope: ThemeScope,
    /// Name of the row representing the currently effective theme, ticked in
    /// the picker regardless of keyboard-selection position.
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
    pub(crate) tab_move_mode: bool,
    pub(crate) titlebar_dragging: bool,
    pub(crate) button_layout: WindowButtonLayout,
    pub(crate) performance_overlay: Option<PerformanceOverlay>,
    pub(crate) performance_overlay_generation: u64,
    pub(crate) terminal_spawn_notify_pending: bool,
    pub(crate) _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
pub(crate) struct ZettaLaunchOptions {
    pub(crate) initial_profile: Option<Profile>,
    pub(crate) initial_project: Option<ProjectConfig>,
    pub(crate) launch_theme_override: Option<(String, String)>,
    pub(crate) no_mux: bool,
    pub(crate) initial_command: Option<Vec<String>>,
    pub(crate) initial_working_directory: Option<PathBuf>,
    pub(crate) initial_launch: Option<TerminalLaunch>,
}

/// The window's starting state, as [`Zetta::new`] has resolved it: the
/// configuration it launched with, whatever went wrong reading it, the project
/// registry, and what the platform says about window buttons.
///
/// Distinct from [`ZettaLaunchOptions`], which is what the launch *asked* for;
/// this is what was made of it. Passed to [`Zetta::with_launch_state`] as one
/// value so the resolution above it and the 60-field literal below it stay
/// separable.
struct LaunchState {
    config: Config,
    configuration_error: Option<String>,
    projects: ProjectState,
    button_layout: WindowButtonLayout,
    no_mux: bool,
    launch_theme_override: Option<(String, String)>,
    #[cfg(feature = "session-persistence")]
    auto_protect: Option<Arc<crate::session_auto_protect::SessionAutoProtect>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CloseTabConfirmation {
    pub(crate) tab_id: u64,
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
            if let Some(authentication) =
                background_authentication_for_close(&tab.close_policy, tab.shared, true, false)
            {
                self.store_background_tab(tab, authentication, cx);
                preserved_any = true;
            }
        }
        if preserved_any {
            self.finish_background_session_change(cx);
        }
        self.active_tab = 0;
        self.tab_move_mode = false;
        self.command_palette = None;
        self.multi_command = None;
        self.multi_command_mode = CommandPromptMode::Multi;
        self.settings_editor = None;
        self.settings_loading = false;
        self.settings_pending_page = None;
        #[cfg(feature = "serial-console")]
        {
            self.serial_console = None;
        }
        self.session_authentication = None;
        self.remote_session_target = None;
        #[cfg(feature = "session-persistence")]
        {
            self.remote_session_key_envelope = None;
        }
        self.close_tab_confirmation = None;
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
                    this.focus_after_window_activation(window, cx);
                }
            }));
        self._subscriptions
            .push(cx.observe_window_appearance(window, |this, window, cx| {
                this.handle_window_appearance_change(window, cx);
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
        mut configuration_error: Option<String>,
        launch_options: ZettaLaunchOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ZettaLaunchOptions {
            initial_profile,
            initial_project,
            launch_theme_override,
            no_mux,
            initial_command,
            initial_working_directory,
            initial_launch,
        } = launch_options;
        let button_layout = system_window_button_layout(cx);
        let projects = match ProjectState::load() {
            Ok(projects) => projects,
            Err(error) => {
                let message = format!("Could not load the project registry: {error:#}");
                configuration_error = Some(match configuration_error {
                    Some(existing) => format!("{existing}\n{message}"),
                    None => message,
                });
                ProjectState::new(ProjectRegistry::empty())
            }
        };
        // Resolved inline only when that costs nothing. A `github:` recipient is
        // an HTTP fetch, and a window must not wait on one to open, so that case
        // starts without automatic protection and fills it in below.
        #[cfg(feature = "session-persistence")]
        let auto_protect =
            (!crate::session_auto_protect::SessionAutoProtect::resolution_is_blocking(
                &config.sessions.persistence,
            ))
            .then(|| resolve_auto_protect(&config, &mut configuration_error))
            .flatten();
        let mut this = Self::with_launch_state(
            LaunchState {
                config,
                configuration_error,
                projects,
                button_layout,
                no_mux,
                launch_theme_override,
                #[cfg(feature = "session-persistence")]
                auto_protect,
            },
            window,
            cx,
        );
        this.warm_font_and_icon_caches(cx);
        // Only does anything when resolving needed the network, which is why it
        // was skipped above; the cheap case is already resolved.
        #[cfg(feature = "session-persistence")]
        if this.auto_protect.is_none() {
            this.refresh_auto_protect(cx);
        }
        let mut initial_launch = Some(initial_launch.unwrap_or(TerminalLaunch::Spawn));
        if let Some(project) = initial_project {
            let project = this.projects.insert_config(project);
            let profile = initial_profile.or_else(|| {
                project
                    .effective
                    .profiles
                    .get(project.effective.default_profile)
                    .cloned()
            });
            if let Some(profile) = profile {
                this.open_tab_with_profile_context(
                    profile,
                    Some(project),
                    NewTabOrigin::ProjectEntry,
                    initial_command.clone(),
                    initial_working_directory.clone(),
                    initial_launch.take().unwrap_or(TerminalLaunch::Spawn),
                    window,
                    cx,
                );
            } else {
                this.open_tab(window, cx);
            }
        } else if let Some(profile) = initial_profile {
            this.open_tab_with_profile_context(
                profile,
                None,
                NewTabOrigin::CurrentSession,
                initial_command.clone(),
                initial_working_directory.clone(),
                initial_launch.take().unwrap_or(TerminalLaunch::Spawn),
                window,
                cx,
            );
        } else {
            let profile = new_tab_profile(
                None,
                &this.profiles,
                this.launch_config.default_profile,
                this.launch_config.new_tab_profile,
            );
            if let Some(profile) = profile {
                this.open_tab_with_profile_context(
                    profile,
                    None,
                    NewTabOrigin::CurrentSession,
                    initial_command,
                    initial_working_directory,
                    initial_launch.take().unwrap_or(TerminalLaunch::Spawn),
                    window,
                    cx,
                );
            }
        }
        this
    }

    /// The window's state before anything has been opened in it.
    ///
    /// Separate from [`Self::new`] so the 60-field literal is not interleaved
    /// with the work that follows it — the cache warm-up, and opening whatever
    /// the launch asked for.
    fn with_launch_state(state: LaunchState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let LaunchState {
            config,
            configuration_error,
            projects,
            button_layout,
            no_mux,
            launch_theme_override,
            #[cfg(feature = "session-persistence")]
            auto_protect,
        } = state;
        Self {
            launch_config: config.clone(),
            #[cfg(feature = "session-persistence")]
            auto_protect,
            #[cfg(feature = "session-persistence")]
            auto_protect_generation: 0,
            project_detection_base: Arc::new(config.clone()),
            projects,
            launch_theme_override,
            configuration_generation: 0,
            configuration_error,
            configuration_reload_feedback: ConfigurationReloadFeedback::default(),
            pane_output_error: None,
            pane_output_save_in_progress: false,
            transient_notice: TransientNotice::default(),
            tabs: Vec::new(),
            background_sessions: BackgroundSessionRunner::default(),
            mux: None,
            #[cfg(feature = "session-persistence")]
            mux_recovery_generation: 0,
            #[cfg(feature = "session-persistence")]
            mux_recovery_task: None,
            no_mux,
            mux_panes: MuxPanes::default(),
            shared_panes: HashMap::new(),
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
            remote_session_focus: cx.focus_handle(),
            remote_session_picker: None,
            remote_session_target: None,
            #[cfg(feature = "session-persistence")]
            remote_session_key_envelope: None,
            close_confirmation_focus: cx.focus_handle(),
            close_tab_confirmation: None,
            active_tab: 0,
            terminal_placeholder_focus: cx.focus_handle(),
            visible_terminals: Vec::new(),
            profile_shortcut_slots: visible_profile_count(
                &config.profiles,
                &config.hidden_profiles,
            ),
            profiles: config.profiles,
            working_directory: config.working_directory,
            next_tab_id: 1,
            next_attention_id: 1,
            next_pane_id: 1,
            rename_focus: cx.focus_handle(),
            overlay_style_focus: cx.focus_handle(),
            command_palette_focus: cx.focus_handle(),
            command_palette: None,
            multi_command_focus: cx.focus_handle(),
            multi_command: None,
            multi_command_mode: CommandPromptMode::Multi,
            multi_command_catalog: CompletionCatalog::default(),
            multi_command_launches: BoundedLaunchQueue::new(MAX_CONCURRENT_MULTI_COMMAND_SPAWNS),
            settings_focus: cx.focus_handle(),
            title_bar_chrome_view: None,
            settings_surface_view: None,
            tab_icon_picker_view: None,
            settings_page_view: None,
            settings_editor: None,
            settings_loading: false,
            settings_pending_page: None,
            font_cache: Arc::new(OnceLock::new()),
            icon_cache: Arc::new(OnceLock::new()),
            tab_icon_picker_focus: cx.focus_handle(),
            tab_icon_picker: None,
            theme_picker_focus: cx.focus_handle(),
            theme_picker: None,
            theme_picker_scope: ThemeScope::Pane,
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
            tab_move_mode: false,
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
                        this.focus_after_window_activation(window, cx);
                    }
                }),
                cx.observe_window_appearance(window, |this, window, cx| {
                    this.handle_window_appearance_change(window, cx);
                }),
            ],
        }
    }

    /// Builds the font and icon caches off the main thread, so the first frame
    /// does not wait on the system font enumeration.
    fn warm_font_and_icon_caches(&mut self, cx: &mut Context<Self>) {
        let this = self;
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
    }
}

impl Drop for Zetta {
    fn drop(&mut self) {
        if self.performance_overlay.is_some() {
            disable_frame_tracing();
        }
    }
}

mod attention;
mod pane_templates;
mod panes;
mod tabs;
mod window_actions;

#[cfg(test)]
#[path = "tests/app.rs"]
mod tests;
