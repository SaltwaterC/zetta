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
    /// normal drag, but it keeps the outside-drop no-op explicit.
    #[allow(dead_code)]
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
    background_if_pinned: bool,
    failed_pane: bool,
) -> Option<Option<SessionAuthentication>> {
    if background_if_pinned && !failed_pane {
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

fn resolve_cli_replacement_profile(
    profiles: &[Profile],
    requested_name: Option<&str>,
    requested_theme: Option<&str>,
    launch_theme_override: Option<&(String, String)>,
) -> Option<Option<Profile>> {
    match requested_name {
        Some(requested_name) if !requested_name.is_empty() => {
            let mut profile = profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(requested_name))
                .cloned()?;
            apply_launch_theme_override(&mut profile, launch_theme_override);
            if let Some(theme) = requested_theme {
                if theme.is_empty() {
                    return None;
                }
                profile.theme = Some(theme.to_owned());
                profile.dark_theme = Some(theme.to_owned());
            }
            Some(Some(profile))
        }
        Some(_) => None,
        None if requested_theme.is_some() => None,
        None => Some(None),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ResolvedPaneSplitLeaf {
    label: Option<String>,
    profile: Profile,
    environment: HashMap<String, String>,
    overlay_text: Option<String>,
    overlay_font_size: Option<OverlayFontSize>,
    overlay_opacity: Option<f32>,
    overlay_color: Option<Hsla>,
    /// Stacked commands to seed in this pane, already quoted for the resolved
    /// profile's shell the way `zetta pane --stack` quotes them.
    stack: Vec<String>,
}

fn resolve_pane_split_leaves(
    template: &PaneSplitTemplateConfig,
    inherited_profile: &Profile,
    profile_override: Option<&Profile>,
) -> Result<Vec<ResolvedPaneSplitLeaf>> {
    let fallback_profile = profile_override.unwrap_or(inherited_profile);
    template
        .pane_specifications()
        .into_iter()
        .map(|pane: PaneSplitPane| {
            let mut profile = pane.profile.unwrap_or_else(|| fallback_profile.clone());
            if let Some(command) = pane.command {
                profile.command = command.shell();
            }
            if let Some(theme) = pane.theme {
                profile.theme = Some(theme);
            }
            if let Some(dark_theme) = pane.dark_theme {
                profile.dark_theme = Some(dark_theme);
            }
            let (overlay_text, overlay_font_size, overlay_opacity, overlay_color) = match pane
                .overlay
            {
                Some(overlay) => (
                    overlay.text,
                    overlay.size.map(|size| match size {
                        PaneSplitOverlaySize::Small => OverlayFontSize::Small,
                        PaneSplitOverlaySize::Base => OverlayFontSize::Base,
                        PaneSplitOverlaySize::Large => OverlayFontSize::Large,
                        PaneSplitOverlaySize::ExtraLarge => OverlayFontSize::ExtraLarge,
                        PaneSplitOverlaySize::ExtraExtraLarge => OverlayFontSize::ExtraExtraLarge,
                        PaneSplitOverlaySize::ExtraExtraExtraLarge => {
                            OverlayFontSize::ExtraExtraExtraLarge
                        }
                    }),
                    overlay.opacity.map(|opacity| f32::from(opacity) / 100.),
                    overlay
                        .color
                        .map(|color| {
                            overlay_color_from_value(&color).with_context(|| {
                                format!("using pane template overlay color {color:?}")
                            })
                        })
                        .transpose()?,
                ),
                None => (None, None, None, None),
            };
            // Quoting uses the leaf's resolved shell, which is also the shell
            // `stacked_task_shell` runs the entry through.
            let stack = pane
                .stack
                .iter()
                .map(|command| {
                    let argv = std::iter::once(command.program.clone())
                        .chain(command.args.iter().cloned())
                        .collect::<Vec<_>>();
                    quote_pane_command_for_shell(&profile.command, &argv)
                        .with_context(|| format!("using stacked command {:?}", command.program))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(ResolvedPaneSplitLeaf {
                label: pane.label,
                profile,
                environment: pane.env,
                overlay_text,
                overlay_font_size,
                overlay_opacity,
                overlay_color,
                stack,
            })
        })
        .collect()
}

fn apply_pane_split_overlay(pane: &mut TerminalPane, leaf: &ResolvedPaneSplitLeaf) {
    pane.overlay_text = leaf.overlay_text.clone();
    pane.overlay_font_size = leaf.overlay_font_size;
    pane.overlay_opacity = leaf.overlay_opacity;
    pane.overlay_color = leaf.overlay_color;
}

fn pane_split_leaf_requires_restart(pane: &TerminalPane, leaf: &ResolvedPaneSplitLeaf) -> bool {
    pane.profile != leaf.profile
        || pane.environment_overrides != leaf.environment
        || pane.base_exited
        || pane.error.is_some()
        // A retained pane keeps the stack it already has, so seeding a declared
        // stack on top of it would append duplicates every time the template is
        // applied. Rebuilding the pane makes its stack exactly what the template
        // describes.
        || !leaf.stack.is_empty()
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

pub(crate) struct ZettaLaunchOptions {
    pub(crate) initial_profile: Option<Profile>,
    pub(crate) initial_project: Option<ProjectConfig>,
    pub(crate) launch_theme_override: Option<(String, String)>,
    pub(crate) no_mux: bool,
    pub(crate) initial_command: Option<Vec<String>>,
    pub(crate) initial_working_directory: Option<PathBuf>,
    pub(crate) initial_launch: Option<TerminalLaunch>,
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
            if let Some(authentication) = tab.close_policy.background_authentication() {
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
        let mut this = Self {
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
    fn open_tab_with_profile_context(
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
            rename_cursor: 0,
            rename_select_all: false,
            editing_overlay_pane: None,
            overlay_buffer: None,
            overlay_cursor: 0,
            overlay_select_all: false,
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
            TerminalLaunch::Spawn => self.spawn_terminal(
                tab_id,
                pane_id,
                profile,
                working_directory,
                wsl_directory,
                wsl_cwd_file,
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
        let has_failed_pane = self.tabs[index]
            .panes
            .iter()
            .any(|pane| pane.exit.is_some());
        let background_authentication = background_authentication_for_close(
            &self.tabs[index].close_policy,
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
            .map(Self::sanitize_exit_context)
            .unwrap_or_else(|| "<unknown>".to_owned());
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

    pub(crate) fn toggle_fullscreen(
        &mut self,
        _: &ToggleFullscreen,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if window.window_controls().fullscreen {
            window.toggle_fullscreen();
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

    pub(crate) fn apply_pane_split_template(
        &mut self,
        action: &ApplyPaneSplitTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_pane_split_template_with_profile(&action.name, None, window, cx);
    }

    pub(crate) fn replace_active_pane_from_cli(
        &mut self,
        request: ReplacePaneRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if request.split.is_none() && request.profile.is_none() {
            return false;
        }
        let Some(profile_override) = resolve_cli_replacement_profile(
            &self.profiles,
            request.profile.as_deref(),
            request.theme.as_deref(),
            self.launch_theme_override.as_ref(),
        ) else {
            return false;
        };

        if let Some(name) = request.split {
            self.apply_pane_split_template_with_profile(&name, profile_override, window, cx)
        } else {
            self.replace_active_pane_profile(
                profile_override.expect("a profile is required without a split template"),
                window,
                cx,
            )
        }
    }

    pub(crate) fn apply_pane_split_template_with_profile(
        &mut self,
        name: &str,
        profile_override: Option<Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let project = self.active_project_config().cloned();
        let effective = project
            .as_ref()
            .map(|project| &project.effective)
            .unwrap_or(&self.launch_config);
        let templates = effective.pane_split_templates.clone();
        let Some(template) = templates.get(name) else {
            self.configuration_error =
                Some(format!("Pane split template {:?} is not configured", name));
            cx.notify();
            return false;
        };
        let Some(new_pane_count) = template.pane_count().checked_sub(1) else {
            return false;
        };
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return false;
        };
        if !can_add_panes(tab.panes.len(), new_pane_count) {
            return false;
        }
        let tab_id = tab.id;
        let tab_theme_override = tab.theme_override.clone();
        let active_pane_theme_override = tab
            .active_pane()
            .and_then(|pane| pane.theme_override.clone());
        let active_pane_id = tab.active_pane;
        let active_pane = tab.active_pane();
        let Some(active_profile) = tab.active_profile().cloned() else {
            return false;
        };
        let mut leaves =
            match resolve_pane_split_leaves(template, &active_profile, profile_override.as_ref()) {
                Ok(leaves) => leaves,
                Err(error) => {
                    self.configuration_error = Some(format!(
                        "Could not resolve pane split template {:?}: {error:#}",
                        name
                    ));
                    cx.notify();
                    return false;
                }
            };
        if let Some(project) = &project {
            for leaf in &mut leaves {
                let template_environment = std::mem::take(&mut leaf.environment);
                leaf.environment = project.environment.clone();
                leaf.environment.extend(template_environment);
            }
        }
        let terminal_themes = match leaves
            .iter()
            .enumerate()
            .map(|(index, leaf)| {
                resolve_terminal_theme(
                    (index == 0)
                        .then_some(active_pane_theme_override.as_deref())
                        .flatten(),
                    tab_theme_override.as_deref(),
                    &leaf.profile,
                    project.as_deref(),
                    cx,
                )
            })
            .collect::<Result<Vec<_>>>()
        {
            Ok(themes) => themes,
            Err(error) => {
                self.configuration_error = Some(format!(
                    "Could not apply profile theme for pane template: {error:#}"
                ));
                cx.notify();
                return false;
            }
        };
        let active_leaf = &leaves[0];
        let replacing_active =
            active_pane.is_none_or(|pane| pane_split_leaf_requires_restart(pane, active_leaf));
        let inherit_working_directory = effective.working_directory_scope.inherits_for_new_pane();
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
        let inherited_wsl_directory = active_pane
            .filter(|_| inherit_working_directory)
            .and_then(|pane| pane.wsl_working_directory(cx));
        let working_directories = leaves
            .iter()
            .map(|leaf| {
                launch_working_directory(
                    &leaf.profile,
                    inherited_working_directory.clone(),
                    inherited_wsl_directory.clone(),
                    effective.working_directory.clone(),
                    effective.working_directory_configured,
                )
            })
            .collect::<Vec<_>>();
        let mut terminal_settings = TerminalSpawnSettings::current(cx);

        if !replacing_active
            && let Some(terminal) = active_pane.and_then(|pane| pane.terminal.clone())
        {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }

        let new_pane_ids = (0..new_pane_count).map(|_| {
            let pane_id = self.next_pane_id;
            self.next_pane_id += 1;
            pane_id
        });
        let existing_active_wsl_cwd_file = active_pane.and_then(|pane| pane.wsl_cwd_file.clone());
        let new_panes = new_pane_ids
            .enumerate()
            .map(|(index, pane_id)| {
                (
                    pane_id,
                    wsl_cwd_tracking_file(&leaves[index + 1].profile, pane_id),
                )
            })
            .collect::<Vec<_>>();
        for (pane_id, _) in &new_panes {
            self.projects.inherit_pane_root(active_pane_id, *pane_id);
        }
        let active_wsl_cwd_file = if replacing_active {
            wsl_cwd_tracking_file(&active_leaf.profile, active_pane_id)
        } else {
            existing_active_wsl_cwd_file
        };
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                new_panes.iter().map(|(pane_id, _)| *pane_id),
            ));
        let mut all_pane_ids =
            std::iter::once(active_pane_id).chain(new_panes.iter().map(|(pane_id, _)| *pane_id));
        let replacement = pane_layout_from_configured_template(&templates, name, &mut all_pane_ids)
            .expect("the configured pane template was resolved before allocating panes");
        let generated_labels = std::iter::once(active_pane_id)
            .chain(new_panes.iter().map(|(pane_id, _)| *pane_id))
            .zip(leaves.iter().map(|leaf| leaf.label.clone()))
            .collect::<Vec<_>>();
        debug_assert_eq!(generated_labels.len(), new_pane_count + 1);

        let replaced_stack_ids = replacing_active
            .then(|| {
                self.tabs[self.active_tab].pane(active_pane_id).map(|pane| {
                    pane.stack
                        .entries
                        .iter()
                        .map(|entry| entry.id)
                        .collect::<Vec<_>>()
                })
            })
            .flatten()
            .unwrap_or_default();
        for stack_id in replaced_stack_ids {
            self.background_observed_panes.remove(&stack_id);
        }

        let tab = &mut self.tabs[self.active_tab];
        tab.maximized_pane = None;
        if !tab.layout.replace(active_pane_id, replacement) {
            return false;
        }
        if replacing_active {
            let pane = tab
                .pane_mut(active_pane_id)
                .expect("the active pane must remain in a template replacement");
            let _old_terminal = pane.terminal.take();
            pane.view = None;
            pane.error = None;
            pane.exit = None;
            pane.base_exited = false;
            pane.pending_command = None;
            pane.stack = PaneStack::default();
            pane.profile = active_leaf.profile.clone();
            pane.environment_overrides = active_leaf.environment.clone();
            pane.wsl_cwd_file = active_wsl_cwd_file.clone();
            apply_pane_split_overlay(pane, active_leaf);
        } else if let Some(pane) = tab.pane_mut(active_pane_id) {
            pane.profile = active_leaf.profile.clone();
            pane.environment_overrides = active_leaf.environment.clone();
            apply_pane_split_overlay(pane, active_leaf);
        }
        tab.panes.reserve(new_pane_count);
        for (index, (pane_id, wsl_cwd_file)) in new_panes.iter().enumerate() {
            let leaf = &leaves[index + 1];
            let mut pane = TerminalPane::new(*pane_id, leaf.profile.clone())
                .with_wsl_cwd_file(wsl_cwd_file.clone())
                .with_environment_overrides(leaf.environment.clone());
            apply_pane_split_overlay(&mut pane, leaf);
            tab.push_pane(pane);
        }
        tab.apply_generated_labels(generated_labels);
        tab.activate_pane(active_pane_id);
        self.retain_open_visible_terminals();

        // Every declared stacked entry is pushed before any spawn callback can
        // run, so the base terminal's spawn sees a non-base selection and leaves
        // focus to the stacked entry the pane ends up selecting.
        let stacked_leaves = std::iter::once(active_pane_id)
            .chain(new_panes.iter().map(|(pane_id, _)| *pane_id))
            .enumerate()
            .filter(|(leaf_index, _)| !leaves[*leaf_index].stack.is_empty());
        let mut stacked_launches = Vec::new();
        for (leaf_index, pane_id) in stacked_leaves {
            let leaf = &leaves[leaf_index];
            for command in &leaf.stack {
                let entry_id = self.next_pane_id;
                self.next_pane_id += 1;
                let entry = StackedPane::new(
                    entry_id,
                    command.clone(),
                    leaf.profile.clone(),
                    working_directories[leaf_index].0.clone(),
                    working_directories[leaf_index].1.clone(),
                );
                let pushed = self.tabs[self.active_tab]
                    .pane_mut(pane_id)
                    .is_some_and(|pane| pane.stack.push(entry));
                if !pushed {
                    break;
                }
                stacked_launches.push((leaf_index, pane_id, entry_id, command.clone()));
            }
        }

        let spawn_count = new_panes.len() + usize::from(replacing_active) + stacked_launches.len();
        if replacing_active {
            let path_hyperlink_regexes = terminal_settings.path_hyperlink_regexes(spawn_count == 1);
            self.spawn_terminal_with_theme_and_environment(
                tab_id,
                active_pane_id,
                active_leaf.profile.clone(),
                working_directories[0].0.clone(),
                working_directories[0].1.clone(),
                active_wsl_cwd_file,
                terminal_themes[0].clone(),
                &terminal_settings,
                path_hyperlink_regexes,
                active_leaf.environment.clone(),
                false,
                window,
                cx,
            );
        }
        for (index, (pane_id, wsl_cwd_file)) in new_panes.into_iter().enumerate() {
            let leaf_index = index + 1;
            let path_hyperlink_regexes = terminal_settings
                .path_hyperlink_regexes(index + 1 + usize::from(replacing_active) == spawn_count);
            self.spawn_terminal_with_theme_and_environment(
                tab_id,
                pane_id,
                leaves[leaf_index].profile.clone(),
                working_directories[leaf_index].0.clone(),
                working_directories[leaf_index].1.clone(),
                wsl_cwd_file,
                terminal_themes[leaf_index].clone(),
                &terminal_settings,
                path_hyperlink_regexes,
                leaves[leaf_index].environment.clone(),
                false,
                window,
                cx,
            );
        }
        let stacked_count = stacked_launches.len();
        for (index, (leaf_index, pane_id, entry_id, command)) in
            stacked_launches.into_iter().enumerate()
        {
            self.spawn_stacked_terminal(
                tab_id,
                pane_id,
                entry_id,
                command,
                leaves[leaf_index].profile.clone(),
                working_directories[leaf_index].0.clone(),
                working_directories[leaf_index].1.clone(),
                terminal_themes[leaf_index].clone(),
                &mut terminal_settings,
                index + 1 == stacked_count,
                window,
                cx,
            );
        }
        self.focus_active(window, cx);
        cx.notify();
        true
    }

    fn replace_active_pane_profile(
        &mut self,
        profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return false;
        };
        let tab_id = tab.id;
        let tab_theme_override = tab.theme_override.clone();
        let pane_theme_override = tab
            .pane(tab.active_pane)
            .and_then(|pane| pane.theme_override.clone());
        let active_pane_id = tab.active_pane;
        let active_pane = tab.active_pane();
        let effective_config = self.effective_config();
        let inherit_working_directory = effective_config
            .working_directory_scope
            .inherits_for_new_pane();
        let working_directory_configured = effective_config.working_directory_configured;
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
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
        let terminal_theme = match resolve_terminal_theme(
            pane_theme_override.as_deref(),
            tab_theme_override.as_deref(),
            &profile,
            self.active_project_config().map(AsRef::as_ref),
            cx,
        ) {
            Ok(theme) => theme,
            Err(error) => {
                self.configuration_error = Some(format!(
                    "Could not apply profile theme for pane replacement: {error:#}"
                ));
                cx.notify();
                return false;
            }
        };
        let mut terminal_settings = TerminalSpawnSettings::current(cx);
        let path_hyperlink_regexes = terminal_settings.path_hyperlink_regexes(true);
        let wsl_cwd_file = wsl_cwd_tracking_file(&profile, active_pane_id);

        let replaced_stack_ids = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.pane(active_pane_id))
            .map(|pane| {
                pane.stack
                    .entries
                    .iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for stack_id in replaced_stack_ids {
            self.background_observed_panes.remove(&stack_id);
        }

        let Some(pane) = self.tabs[self.active_tab].pane_mut(active_pane_id) else {
            return false;
        };
        let _old_terminal = pane.terminal.take();
        pane.view = None;
        pane.error = None;
        pane.exit = None;
        pane.base_exited = false;
        pane.pending_command = None;
        pane.stack = PaneStack::default();
        pane.profile = profile.clone();
        pane.environment_overrides.clear();
        pane.wsl_cwd_file = wsl_cwd_file.clone();
        self.retain_open_visible_terminals();
        self.spawn_terminal_with_theme(
            tab_id,
            active_pane_id,
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
        self.focus_active(window, cx);
        cx.notify();
        true
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
        let Some(pane_id) = tab.visible_layout().and_then(|layout| {
            layout.adjacent_pane(tab.active_pane, direction, &tab.focus_history)
        }) else {
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

    fn focus_after_window_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_picking_overlay_style() {
            self.overlay_style_focus.focus(window, cx);
        } else {
            self.focus_active(window, cx);
        }
    }

    pub(crate) fn has_visible_tab_by_attention_id(&self, attention_id: u64) -> bool {
        self.tabs.iter().any(|tab| tab.attention_id == attention_id)
    }

    pub(crate) fn pane_theme_by_attention_id(
        &self,
        attention_id: u64,
        pane_id: Option<u64>,
        cx: &App,
    ) -> Option<String> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.attention_id == attention_id)?;
        let (pane, selection, profile, project_pane_id, view) = match pane_id {
            Some(routing_id) => tab.panes.iter().find_map(|pane| {
                if pane.routing_id == routing_id {
                    return Some((
                        pane,
                        PaneStackSelection::Base,
                        &pane.profile,
                        pane.id,
                        pane.view.clone(),
                    ));
                }
                pane.stack.entries.iter().find_map(|entry| {
                    (entry.routing_id == routing_id).then(|| {
                        (
                            pane,
                            PaneStackSelection::Stacked(entry.id),
                            &entry.profile,
                            entry.id,
                            entry.view.clone(),
                        )
                    })
                })
            })?,
            None => {
                let pane = tab.active_pane()?;
                match pane.stack.selected {
                    PaneStackSelection::Base => (
                        pane,
                        PaneStackSelection::Base,
                        &pane.profile,
                        pane.id,
                        pane.view.clone(),
                    ),
                    PaneStackSelection::Stacked(entry_id) => {
                        let entry = pane
                            .stack
                            .entries
                            .iter()
                            .find(|entry| entry.id == entry_id)?;
                        (
                            pane,
                            PaneStackSelection::Stacked(entry.id),
                            &entry.profile,
                            entry.id,
                            entry.view.clone(),
                        )
                    }
                }
            }
        };

        let theme = view
            .and_then(|view| view.read(cx).theme().cloned())
            .or_else(|| {
                resolve_terminal_theme(
                    pane.theme_override(selection),
                    tab.theme_override.as_deref(),
                    profile,
                    self.projects
                        .config_for_pane(project_pane_id)
                        .map(Arc::as_ref),
                    cx,
                )
                .ok()
                .flatten()
            })
            .unwrap_or_else(|| self.application_theme(cx));
        Some(theme.name.to_string())
    }

    pub(crate) fn focus_tab_by_attention_id(
        &mut self,
        attention_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab_index) = self
            .tabs
            .iter()
            .position(|tab| tab.attention_id == attention_id)
        else {
            return false;
        };
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        self.active_tab = tab_index;
        self.tab_overflow_selection_side = None;
        self.dismiss_tab_overflow_menus(cx);
        self.focus_active(window, cx);
        true
    }

    pub(crate) fn attention_id_for_tab(&self, tab_id: u64) -> Option<u64> {
        self.tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.attention_id)
            .or_else(|| {
                self.background_sessions
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .map(|tab| tab.attention_id)
            })
    }

    fn tab_content_is_focused(
        &self,
        tab_index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get(tab_index) else {
            return false;
        };
        if tab.pane_is_visible(tab.active_pane) {
            tab.active_view().map_or_else(
                || self.terminal_placeholder_focus.is_focused(window),
                |view| view.focus_handle(cx).is_focused(window),
            )
        } else {
            !tab.minimized_panes.is_empty() && self.minimized_panes_focus.is_focused(window)
        }
    }

    pub(crate) fn clear_active_tab_attention_if_focused(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !window.is_window_active() {
            return false;
        }
        let active_tab = self.active_tab;
        if !self.tab_content_is_focused(active_tab, window, cx) {
            return false;
        }
        let cleared = self
            .tabs
            .get_mut(active_tab)
            .and_then(|tab| tab.attention.take())
            .is_some();
        if cleared {
            cx.notify();
        }
        cleared
    }

    pub(crate) fn set_tab_attention(
        &mut self,
        request: TabAttentionRequest,
        window: Option<&Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        let attention = TabAttention {
            summary: request.summary,
            body: request.body,
        };
        if let Some(tab_index) = self
            .tabs
            .iter()
            .position(|tab| tab.attention_id == request.attention_id)
        {
            let should_clear = window.is_some_and(|window| {
                self.active_tab == tab_index && self.tab_content_is_focused(tab_index, window, cx)
            });
            self.tabs[tab_index].attention = (!should_clear).then_some(attention);
            cx.notify();
            return true;
        }
        if let Some(tab) = self
            .background_sessions
            .iter_unprotected_mut()
            .find(|tab| tab.attention_id == request.attention_id)
        {
            tab.attention = Some(attention);
            cx.notify();
            return true;
        }
        false
    }

    pub(crate) fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_current_project(window, cx);
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let active_is_visible = tab.pane_is_visible(tab.active_pane);
            if active_is_visible {
                if let Some(view) = tab.active_view() {
                    view.focus_handle(cx).focus(window, cx);
                } else {
                    self.terminal_placeholder_focus.focus(window, cx);
                }
            } else if !tab.minimized_panes.is_empty() {
                self.minimized_panes_focus.focus(window, cx);
            }
        }
        self.clear_active_tab_attention_if_focused(window, cx);
        cx.notify();
    }

    pub(crate) fn active_terminal_focus(&self, cx: &App) -> Option<gpui::FocusHandle> {
        let tab = self.tabs.get(self.active_tab)?;
        if !tab.pane_is_visible(tab.active_pane) {
            return None;
        }
        Some(
            tab.active_view()
                .map(|view| view.focus_handle(cx))
                .unwrap_or_else(|| self.terminal_placeholder_focus.clone()),
        )
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
