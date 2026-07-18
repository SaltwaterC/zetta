#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod command_palette;
mod config;
mod settings_editor;
mod theme_extensions;
mod zetta_assets;

const ZETTA_APP_ID: &str = "Zetta";
const ZETTA_DEFAULT_THEME: &str = "One Light";

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context as _, Result};
use command_palette::{CommandPalette, PaletteCommand, humanize_action_name};
use config::{Config, Profile};
use gpui::{
    Action, Anchor, AnyElement, App, AppContext as _, Bounds, Context, CursorStyle, Decorations,
    Entity, Focusable, FrameTiming, FrameTimingCollector, HitboxBehavior, InteractiveElement as _,
    IntoElement, KeyBinding, KeyDownEvent, MAX_BUTTONS_PER_SIDE, MouseButton, Pixels, Point,
    Render, ResizeEdge, ScrollHandle, SharedString, Size, Subscription, Task, Tiling,
    TitlebarOptions, UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds,
    WindowButton, WindowButtonLayout, WindowControlArea, WindowControls, WindowDecorations,
    WindowId, WindowOptions, actions, canvas, div, point, profiler, px, size, svg,
    transparent_black, uniform_list,
};
use schemars::JsonSchema;
use serde::Deserialize;
use settings::{KeymapFile, KeymapFileLoadResult, Settings as _};
use settings_editor::{
    BindingForm, ConfigTextField, ConfigurationForm, KeymapForm, KeymapSectionForm,
    KeymapTextField, SettingsPage, TextField, save as save_settings_file,
};
use task::Shell;
use terminal::{
    Paste, PasteTrimmed, Range, Search, TerminalBuilder, terminal_settings::TerminalSettings,
};
use terminal_view::{
    ClearClipboard, CopyAndClearSelection, DismissSearch, SearchNextMatch, SearchPreviousMatch,
    SearchScrollback, SelectAll, SelectAllSearchText, TerminalInput, TerminalView,
    TerminalViewEvent,
};
use theme::{ActiveTheme, ClientDecorationsExt as _, GlobalTheme, Theme, ThemeRegistry};
use theme_extensions::{InstalledThemeExtension, ThemeExtension};
use ui::{
    Banner, ButtonCommon as _, ButtonLike, ButtonSize, ButtonStyle, Clickable as _, Color, Icon,
    IconButton, IconButtonShape, IconName, IconPosition, IconSize, Label, LabelSize, PopoverMenu,
    Severity, Tooltip, prelude::*,
};
use util::{ResultExt as _, paths::PathStyle};
use zetta_assets::ZettaAssets;

actions!(
    zetta,
    [
        NewTab,
        NewWindow,
        CloseTab,
        NextTab,
        PreviousTab,
        RenameTab,
        SplitHorizontal,
        SplitVertical,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        ToggleBroadcastInput,
        IncreaseTerminalFontSize,
        DecreaseTerminalFontSize,
        ResetTerminalFontSize,
        IncreasePaneFontSize,
        DecreasePaneFontSize,
        ResetPaneFontSize,
        SearchTabScrollback,
        ReloadConfiguration,
        ToggleCommandPalette,
        ToggleSettings,
        TogglePerformanceOverlay
    ]
);

static PERFORMANCE_OVERLAY_COUNT: AtomicUsize = AtomicUsize::new(0);
static PERFORMANCE_OWNS_FRAME_TRACING: AtomicBool = AtomicBool::new(false);
const PERFORMANCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const FRAME_BUDGET_120_HZ: Duration = Duration::from_nanos(8_333_333);
const FRAME_BUDGET_60_HZ: Duration = Duration::from_nanos(16_666_667);

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta)]
#[serde(deny_unknown_fields)]
struct OpenProfile {
    slot: usize,
}

struct TerminalPane {
    id: u64,
    profile: Profile,
    view: Option<Entity<TerminalView>>,
    error: Option<String>,
    wsl_cwd_file: Option<PathBuf>,
}

impl TerminalPane {
    fn wsl_working_directory(&self, cx: &App) -> Option<String> {
        if let Some(directory) = self.view.as_ref().and_then(|view| {
            view.read(cx)
                .terminal()
                .read(cx)
                .reported_working_directory()
                .map(str::to_owned)
        }) {
            return Some(directory);
        }

        let path = self.wsl_cwd_file.as_ref()?;
        let directory = fs::read_to_string(path).ok()?;
        let directory = directory.trim_end_matches(['\r', '\n']);
        (directory.starts_with('/') && !directory.contains(['\r', '\n', '\0']))
            .then(|| directory.to_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy)]
enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PaneRegion {
    id: u64,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PaneLayout {
    Pane(u64),
    Split {
        axis: SplitAxis,
        first: Box<PaneLayout>,
        second: Box<PaneLayout>,
    },
}

impl PaneLayout {
    fn split(&mut self, target: u64, axis: SplitAxis, new_pane: u64) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    first: Box::new(Self::Pane(target)),
                    second: Box::new(Self::Pane(new_pane)),
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split { first, second, .. } => {
                first.split(target, axis, new_pane) || second.split(target, axis, new_pane)
            }
        }
    }

    fn without(self, target: u64) -> Option<Self> {
        match self {
            Self::Pane(id) => (id != target).then_some(Self::Pane(id)),
            Self::Split {
                axis,
                first,
                second,
            } => match (first.without(target), second.without(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(layout), None) | (None, Some(layout)) => Some(layout),
                (None, None) => None,
            },
        }
    }

    fn first_pane(&self) -> u64 {
        match self {
            Self::Pane(id) => *id,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    fn regions(&self) -> Vec<PaneRegion> {
        let mut regions = Vec::new();
        self.collect_regions(0., 0., 1., 1., &mut regions);
        regions
    }

    fn collect_regions(
        &self,
        left: f32,
        top: f32,
        width: f32,
        height: f32,
        regions: &mut Vec<PaneRegion>,
    ) {
        match self {
            Self::Pane(id) => regions.push(PaneRegion {
                id: *id,
                left,
                right: left + width,
                top,
                bottom: top + height,
            }),
            Self::Split {
                axis: SplitAxis::Horizontal,
                first,
                second,
            } => {
                first.collect_regions(left, top, width, height / 2., regions);
                second.collect_regions(left, top + height / 2., width, height / 2., regions);
            }
            Self::Split {
                axis: SplitAxis::Vertical,
                first,
                second,
            } => {
                first.collect_regions(left, top, width / 2., height, regions);
                second.collect_regions(left + width / 2., top, width / 2., height, regions);
            }
        }
    }

    fn adjacent_pane(&self, active: u64, direction: PaneDirection) -> Option<u64> {
        let regions = self.regions();
        let source = regions.iter().find(|region| region.id == active)?;
        let source_x = (source.left + source.right) / 2.;
        let source_y = (source.top + source.bottom) / 2.;

        regions
            .iter()
            .filter(|candidate| candidate.id != active)
            .filter_map(|candidate| {
                let candidate_x = (candidate.left + candidate.right) / 2.;
                let candidate_y = (candidate.top + candidate.bottom) / 2.;
                let (primary, perpendicular) = match direction {
                    PaneDirection::Left if candidate_x < source_x => {
                        (source_x - candidate_x, (source_y - candidate_y).abs())
                    }
                    PaneDirection::Right if candidate_x > source_x => {
                        (candidate_x - source_x, (source_y - candidate_y).abs())
                    }
                    PaneDirection::Up if candidate_y < source_y => {
                        (source_y - candidate_y, (source_x - candidate_x).abs())
                    }
                    PaneDirection::Down if candidate_y > source_y => {
                        (candidate_y - source_y, (source_x - candidate_x).abs())
                    }
                    _ => return None,
                };
                Some((primary + perpendicular * 2., candidate.id))
            })
            .min_by(|(left_score, _), (right_score, _)| left_score.total_cmp(right_score))
            .map(|(_, id)| id)
    }
}

struct Tab {
    id: u64,
    panes: Vec<TerminalPane>,
    layout: PaneLayout,
    active_pane: u64,
    focus_history: Vec<u64>,
    broadcast_input: bool,
    custom_title: Option<String>,
    rename_buffer: Option<String>,
    rename_cursor: usize,
    rename_select_all: bool,
}

impl Tab {
    fn pane(&self, id: u64) -> Option<&TerminalPane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    fn pane_mut(&mut self, id: u64) -> Option<&mut TerminalPane> {
        self.panes.iter_mut().find(|pane| pane.id == id)
    }

    fn active_pane(&self) -> Option<&TerminalPane> {
        self.pane(self.active_pane)
    }

    fn active_profile(&self) -> Option<&Profile> {
        self.active_pane().map(|pane| &pane.profile)
    }

    fn activate_pane(&mut self, id: u64) {
        if self.pane(id).is_none() {
            return;
        }
        self.focus_history.retain(|pane_id| *pane_id != id);
        self.focus_history.push(id);
        self.active_pane = id;
    }

    fn restore_focus_after_close(&mut self, closed: u64, fallback: u64) {
        let surviving = self.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
        self.focus_history
            .retain(|pane_id| *pane_id != closed && surviving.contains(pane_id));

        if self.active_pane != closed && surviving.contains(&self.active_pane) {
            return;
        }
        let next = self.focus_history.last().copied().unwrap_or(fallback);
        self.activate_pane(next);
    }

    fn theme(&self, cx: &App) -> Arc<Theme> {
        self.active_pane()
            .and_then(|pane| pane.view.as_ref())
            .and_then(|view| view.read(cx).theme().cloned())
            .or_else(|| {
                self.active_profile()
                    .and_then(|profile| resolve_profile_theme(profile, cx).ok().flatten())
            })
            .unwrap_or_else(|| cx.theme().clone())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PerformanceMetrics {
    draw_fps: f64,
    average_draw_ms: f64,
    p95_draw_ms: f64,
    average_latency_ms: f64,
    slow_120_hz: usize,
    slow_60_hz: usize,
}

impl PerformanceMetrics {
    fn from_timings(timings: &[FrameTiming], elapsed: Duration) -> Self {
        if timings.is_empty() || elapsed.is_zero() {
            return Self::default();
        }

        let mut draw_durations = timings
            .iter()
            .map(FrameTiming::draw_duration)
            .collect::<Vec<_>>();
        draw_durations.sort_unstable();
        let total_draw = draw_durations.iter().sum::<Duration>();
        let p95_index = ((draw_durations.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(draw_durations.len() - 1);
        let latencies = timings
            .iter()
            .filter_map(FrameTiming::dirty_to_draw_duration)
            .collect::<Vec<_>>();
        let average_latency_ms = if latencies.is_empty() {
            0.0
        } else {
            latencies.iter().sum::<Duration>().as_secs_f64() * 1_000.0 / latencies.len() as f64
        };

        Self {
            draw_fps: timings.len() as f64 / elapsed.as_secs_f64(),
            average_draw_ms: total_draw.as_secs_f64() * 1_000.0 / timings.len() as f64,
            p95_draw_ms: draw_durations[p95_index].as_secs_f64() * 1_000.0,
            average_latency_ms,
            slow_120_hz: draw_durations
                .iter()
                .filter(|duration| **duration > FRAME_BUDGET_120_HZ)
                .count(),
            slow_60_hz: draw_durations
                .iter()
                .filter(|duration| **duration > FRAME_BUDGET_60_HZ)
                .count(),
        }
    }
}

struct PerformanceOverlay {
    collector: FrameTimingCollector,
    window_id: WindowId,
    sampled_at: Instant,
    metrics: PerformanceMetrics,
    generation: u64,
}

impl PerformanceOverlay {
    fn new(window_id: WindowId, generation: u64) -> Self {
        Self {
            collector: FrameTimingCollector::new(),
            window_id,
            sampled_at: Instant::now(),
            metrics: PerformanceMetrics::default(),
            generation,
        }
    }

    fn sample(&mut self) {
        let now = Instant::now();
        let timings = self
            .collector
            .collect_unseen()
            .into_iter()
            .filter(|timing| timing.window_id == self.window_id)
            .collect::<Vec<_>>();
        self.metrics = PerformanceMetrics::from_timings(&timings, now - self.sampled_at);
        self.sampled_at = now;
    }
}

#[derive(Clone, Copy)]
struct TabSearchMatch {
    pane_id: u64,
    match_index: usize,
}

struct TabSearch {
    tab_id: u64,
    query: String,
    cursor: usize,
    select_all: bool,
    generation: u64,
    matches: Vec<TabSearchMatch>,
    active_match: Option<usize>,
    task: Option<Task<()>>,
}

fn tab_search_request_is_current(
    search: Option<&TabSearch>,
    tab_id: u64,
    generation: u64,
    query: &str,
) -> bool {
    search.is_some_and(|search| {
        search.tab_id == tab_id && search.generation == generation && search.query == query
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsInput {
    Configuration(ConfigTextField),
    Keymap(KeymapTextField),
    ThemeSearch,
    FontSearch,
    ProfileDraft(ProfileDraftField),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileDraftField {
    Name,
    Program,
    Arguments,
}

fn adjacent_profile_draft_field(
    current: Option<ProfileDraftField>,
    reverse: bool,
) -> ProfileDraftField {
    match (current, reverse) {
        (Some(ProfileDraftField::Name), false) => ProfileDraftField::Program,
        (Some(ProfileDraftField::Program), false) => ProfileDraftField::Arguments,
        (Some(ProfileDraftField::Arguments), false) | (None, false) => ProfileDraftField::Name,
        (Some(ProfileDraftField::Arguments), true) => ProfileDraftField::Program,
        (Some(ProfileDraftField::Program), true) => ProfileDraftField::Name,
        (Some(ProfileDraftField::Name), true) | (None, true) => ProfileDraftField::Arguments,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsDropdown {
    DefaultProfile,
    Theme,
    ProfileTheme(usize),
    ProfileDraftTheme,
    BindingAction(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumericSetting {
    FontSize,
    ScrollHistory,
}

#[derive(Clone)]
struct SettingsEditor {
    page: SettingsPage,
    configuration: ConfigurationForm,
    keymap: KeymapForm,
    profile_names: Vec<String>,
    themes: Arc<[String]>,
    theme_extension_query: TextField,
    theme_extensions: Vec<ThemeExtension>,
    installed_theme_extensions: Vec<InstalledThemeExtension>,
    theme_extensions_loading: bool,
    theme_extensions_searched: bool,
    theme_extension_downloading: Option<Arc<str>>,
    actions: Arc<[String]>,
    fonts: Arc<[String]>,
    normalized_fonts: Arc<[String]>,
    font_query: Option<TextField>,
    profile_draft: Option<settings_editor::ProfileForm>,
    settings_scroll: ScrollHandle,
    font_scroll: UniformListScrollHandle,
    numeric_repeat_generation: u64,
    scroll_geometry_initialized: bool,
    focused_input: Option<SettingsInput>,
    configuration_dirty: bool,
    keymap_dirty: bool,
    message: Option<(bool, String)>,
}

struct Zetta {
    launch_config: Config,
    configuration_error: Option<String>,
    tabs: Vec<Tab>,
    active_tab: usize,
    selected_profile: usize,
    profiles: Vec<Profile>,
    working_directory: Option<PathBuf>,
    next_tab_id: u64,
    next_pane_id: u64,
    rename_focus: gpui::FocusHandle,
    command_palette_focus: gpui::FocusHandle,
    command_palette: Option<CommandPalette>,
    settings_focus: gpui::FocusHandle,
    settings_editor: Option<SettingsEditor>,
    tab_search_focus: gpui::FocusHandle,
    tab_search: Option<TabSearch>,
    titlebar_dragging: bool,
    button_layout: WindowButtonLayout,
    performance_overlay: Option<PerformanceOverlay>,
    performance_overlay_generation: u64,
    _subscriptions: Vec<Subscription>,
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn matching_font_indices(normalized_fonts: &[String], query: &str) -> Arc<[usize]> {
    let search = query.to_lowercase();
    normalized_fonts
        .iter()
        .enumerate()
        .filter_map(|(index, font)| (search.is_empty() || font.contains(&search)).then_some(index))
        .collect::<Vec<_>>()
        .into()
}

fn adjusted_scroll_history(current: u64, direction: i32, maximum: u64) -> u64 {
    let step_basis = if direction < 0 {
        current.saturating_sub(1)
    } else {
        current
    };
    let step = match step_basis {
        0..100_000 => 1_000,
        100_000..1_000_000 => 100_000,
        1_000_000..10_000_000 => 1_000_000,
        10_000_000..100_000_000 => 10_000_000,
        _ => 100_000_000,
    };
    if direction < 0 {
        current.saturating_sub(step)
    } else {
        current.saturating_add(step).min(maximum)
    }
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(text.len())
}

impl Zetta {
    fn new(
        config: Config,
        configuration_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let button_layout = system_window_button_layout(cx);
        let mut this = Self {
            launch_config: config.clone(),
            configuration_error,
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
            settings_focus: cx.focus_handle(),
            settings_editor: None,
            tab_search_focus: cx.focus_handle(),
            tab_search: None,
            titlebar_dragging: false,
            button_layout,
            performance_overlay: None,
            performance_overlay_generation: 0,
            _subscriptions: vec![
                cx.observe_button_layout_changed(window, |this, _, cx| {
                    this.button_layout = system_window_button_layout(cx);
                    cx.notify();
                }),
                cx.observe_window_activation(window, |this, window, cx| {
                    if window.is_window_active()
                        && !this.is_renaming_tab()
                        && this.command_palette.is_none()
                        && this.tab_search.is_none()
                    {
                        this.focus_active(window, cx);
                    }
                }),
            ],
        };
        this.open_tab(window, cx);
        this
    }

    fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            }],
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

    fn spawn_terminal(
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
        let settings = TerminalSettings::get_global(cx).clone();
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
            settings.path_hyperlink_regexes,
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
                        }
                        if should_focus {
                            view.focus_handle(cx).focus(window, cx);
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(error) => {
                    this.update(cx, |this, cx| {
                        if let Some(pane) = this
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.error = Some(format!("{error:#}"));
                        }
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
    }

    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
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

    fn close_pane(
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
        tab.panes.retain(|pane| pane.id != pane_id);
        let Some(layout) = tab.layout.clone().without(pane_id) else {
            self.close_tab_at(tab_index, window, cx);
            return;
        };
        tab.restore_focus_after_close(pane_id, layout.first_pane());
        tab.layout = layout;
        self.active_tab = tab_index;
        self.focus_active(window, cx);
    }

    fn split_active_pane(&mut self, axis: SplitAxis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
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
        tab.panes.push(TerminalPane {
            id: pane_id,
            profile: profile.clone(),
            view: None,
            error: None,
            wsl_cwd_file: wsl_cwd_file.clone(),
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

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    fn new_window(&mut self, _: &NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        open_zetta_window(
            self.launch_config.clone(),
            self.configuration_error.clone(),
            cx,
        )
        .log_err();
    }

    fn open_profile(&mut self, action: &OpenProfile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = action.slot.checked_sub(1) else {
            return;
        };
        if index >= self.profiles.len() {
            return;
        }
        self.selected_profile = index;
        self.open_tab(window, cx);
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active_tab, window, cx);
    }

    fn split_horizontal(
        &mut self,
        _: &SplitHorizontal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_active_pane(SplitAxis::Horizontal, window, cx);
    }

    fn split_vertical(&mut self, _: &SplitVertical, window: &mut Window, cx: &mut Context<Self>) {
        self.split_active_pane(SplitAxis::Vertical, window, cx);
    }

    fn broadcast_input(
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

    fn toggle_broadcast_input(
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

    fn focus_pane(
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

    fn focus_pane_left(&mut self, _: &FocusPaneLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(PaneDirection::Left, window, cx);
    }

    fn focus_pane_right(
        &mut self,
        _: &FocusPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Right, window, cx);
    }

    fn focus_pane_up(&mut self, _: &FocusPaneUp, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(PaneDirection::Up, window, cx);
    }

    fn focus_pane_down(&mut self, _: &FocusPaneDown, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(PaneDirection::Down, window, cx);
    }

    fn increase_terminal_font_size(
        &mut self,
        _: &IncreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::increase_buffer_font_size(cx);
    }

    fn decrease_terminal_font_size(
        &mut self,
        _: &DecreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::decrease_buffer_font_size(cx);
    }

    fn reset_terminal_font_size(
        &mut self,
        _: &ResetTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::reset_buffer_font_size(cx);
    }

    fn increase_pane_font_size(
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

    fn decrease_pane_font_size(
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

    fn reset_pane_font_size(
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

    fn toggle_performance_overlay(
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

    fn reload_configuration(
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

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.focus_active(window, cx);
        }
    }

    fn previous_tab(&mut self, _: &PreviousTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
            self.focus_active(window, cx);
        }
    }

    fn rename_tab(&mut self, _: &RenameTab, window: &mut Window, cx: &mut Context<Self>) {
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

    fn begin_rename(
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

    fn search_tab_scrollback(
        &mut self,
        _: &SearchTabScrollback,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab_search.is_none() {
            let Some(tab) = self.tabs.get(self.active_tab) else {
                return;
            };
            let tab_id = tab.id;
            let views = tab
                .panes
                .iter()
                .filter_map(|pane| pane.view.clone())
                .collect::<Vec<_>>();
            for view in views {
                view.update(cx, TerminalView::clear_search);
            }
            self.command_palette = None;
            self.tab_search = Some(TabSearch {
                tab_id,
                query: String::new(),
                cursor: 0,
                select_all: false,
                generation: 0,
                matches: Vec::new(),
                active_match: None,
                task: None,
            });
            self.refresh_tab_search(cx);
        }
        self.tab_search_focus.focus(window, cx);
        cx.notify();
    }

    fn clear_tab_search_matches(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let terminals = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .into_iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| pane.view.as_ref())
            .map(|view| view.read(cx).terminal().clone())
            .collect::<Vec<_>>();
        for terminal in terminals {
            terminal.update(cx, |terminal, _| {
                Arc::make_mut(&mut terminal.matches).clear()
            });
        }
    }

    fn dismiss_tab_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = self.tab_search.take() else {
            return;
        };
        self.clear_tab_search_matches(search.tab_id, cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn refresh_tab_search(&mut self, cx: &mut Context<Self>) {
        let Some(search_state) = self.tab_search.as_mut() else {
            return;
        };
        search_state.task.take();
        search_state.generation = search_state.generation.wrapping_add(1);
        search_state.matches.clear();
        search_state.active_match = None;
        let tab_id = search_state.tab_id;
        let query = search_state.query.clone();
        let generation = search_state.generation;

        let terminals = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .into_iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| {
                pane.view
                    .as_ref()
                    .map(|view| (pane.id, view.read(cx).terminal().clone()))
            })
            .collect::<Vec<_>>();
        for (_, terminal) in &terminals {
            terminal.update(cx, |terminal, _| {
                Arc::make_mut(&mut terminal.matches).clear()
            });
        }
        if query.is_empty() {
            cx.notify();
            return;
        }
        let Some(pattern) = Search::new(&regex::escape(&query)) else {
            return;
        };
        let executor = cx.background_executor().clone();
        let task = cx.spawn(async move |this, cx| {
            executor.timer(Duration::from_millis(75)).await;
            let Some(tasks) = this
                .update(cx, |this, cx| {
                    let valid = tab_search_request_is_current(
                        this.tab_search.as_ref(),
                        tab_id,
                        generation,
                        &query,
                    );
                    valid.then(|| {
                        terminals
                            .into_iter()
                            .map(|(pane_id, terminal)| {
                                let task = terminal.update(cx, |terminal, cx| {
                                    terminal.find_matches(pattern.clone(), cx)
                                });
                                (pane_id, terminal, task)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .ok()
                .flatten()
            else {
                return;
            };
            let mut results = Vec::with_capacity(tasks.len());
            for (pane_id, terminal, task) in tasks {
                let matches: Vec<Range> = task.await;
                results.push((pane_id, terminal, matches));
            }
            this.update(cx, |this, cx| {
                let valid = tab_search_request_is_current(
                    this.tab_search.as_ref(),
                    tab_id,
                    generation,
                    &query,
                );
                if !valid {
                    return;
                }

                let mut aggregated = Vec::new();
                for (pane_id, terminal, matches) in results {
                    let match_count = matches.len();
                    terminal.update(cx, |terminal, _| terminal.matches = Arc::new(matches));
                    aggregated.extend((0..match_count).map(|match_index| TabSearchMatch {
                        pane_id,
                        match_index,
                    }));
                }
                let active_match = aggregated.len().checked_sub(1);
                if let Some(search) = this.tab_search.as_mut() {
                    search.matches = aggregated;
                    search.active_match = active_match;
                }
                if let Some(index) = active_match {
                    this.activate_tab_search_match(index, cx);
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(search) = self.tab_search.as_mut() {
            search.task = Some(task);
        }
    }

    fn activate_tab_search_match(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((tab_id, search_match)) = self.tab_search.as_ref().and_then(|search| {
            search
                .matches
                .get(index)
                .copied()
                .map(|search_match| (search.tab_id, search_match))
        }) else {
            return;
        };
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        tab.activate_pane(search_match.pane_id);
        let terminal = tab
            .pane(search_match.pane_id)
            .and_then(|pane| pane.view.as_ref())
            .map(|view| view.read(cx).terminal().clone());
        if let Some(terminal) = terminal {
            terminal.update(cx, |terminal, _| {
                terminal.activate_match(search_match.match_index)
            });
        }
        cx.notify();
    }

    fn navigate_tab_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        let Some(search) = self.tab_search.as_mut() else {
            return;
        };
        let match_count = search.matches.len();
        if match_count == 0 {
            return;
        }
        let current = search
            .active_match
            .unwrap_or(if previous { 0 } else { match_count - 1 });
        let index = if previous {
            current.checked_sub(1).unwrap_or(match_count - 1)
        } else {
            (current + 1) % match_count
        };
        search.active_match = Some(index);
        self.activate_tab_search_match(index, cx);
    }

    fn insert_tab_search_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(search) = self.tab_search.as_mut() else {
            return;
        };
        if search.select_all {
            search.query.clear();
            search.cursor = 0;
        }
        search.query.insert_str(search.cursor, text);
        search.cursor += text.len();
        search.select_all = false;
        self.refresh_tab_search(cx);
    }

    fn tab_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_tab_search(window, cx),
            "enter" | "f3" if event.keystroke.modifiers.shift => self.navigate_tab_search(true, cx),
            "enter" | "f3" => self.navigate_tab_search(false, cx),
            "backspace" => {
                if let Some(search) = self.tab_search.as_mut() {
                    if search.select_all {
                        search.query.clear();
                        search.cursor = 0;
                    } else if search.cursor > 0 {
                        let previous = previous_char_boundary(&search.query, search.cursor);
                        search.query.replace_range(previous..search.cursor, "");
                        search.cursor = previous;
                    }
                    search.select_all = false;
                }
                self.refresh_tab_search(cx);
            }
            "delete" => {
                if let Some(search) = self.tab_search.as_mut() {
                    if search.select_all {
                        search.query.clear();
                        search.cursor = 0;
                    } else if search.cursor < search.query.len() {
                        let next = next_char_boundary(&search.query, search.cursor);
                        search.query.replace_range(search.cursor..next, "");
                    }
                    search.select_all = false;
                }
                self.refresh_tab_search(cx);
            }
            "left" => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.cursor = if search.select_all {
                        0
                    } else {
                        previous_char_boundary(&search.query, search.cursor)
                    };
                    search.select_all = false;
                }
                cx.notify();
            }
            "right" => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.cursor = if search.select_all {
                        search.query.len()
                    } else {
                        next_char_boundary(&search.query, search.cursor)
                    };
                    search.select_all = false;
                }
                cx.notify();
            }
            "home" => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.cursor = 0;
                    search.select_all = false;
                }
                cx.notify();
            }
            "end" => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.cursor = search.query.len();
                    search.select_all = false;
                }
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.select_all = !search.query.is_empty();
                }
                cx.notify();
            }
            "v" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.insert_tab_search_text(&text, cx);
                }
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    self.insert_tab_search_text(text, cx);
                }
            }
            _ => {}
        }
        cx.stop_propagation();
    }

    fn toggle_settings(&mut self, _: &ToggleSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_editor.is_some() {
            self.dismiss_settings(window, cx);
            return;
        }

        self.command_palette = None;
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }

        let configuration =
            match ConfigurationForm::load(&self.launch_config.config_path, &self.launch_config) {
                Ok(configuration) => configuration,
                Err(error) => {
                    self.configuration_error = Some(format!("Could not open settings: {error:#}"));
                    cx.notify();
                    return;
                }
            };
        let keymap = match KeymapForm::load(&self.launch_config.keymap_path) {
            Ok(keymap) => keymap,
            Err(error) => {
                self.configuration_error =
                    Some(format!("Could not open keymap settings: {error:#}"));
                cx.notify();
                return;
            }
        };
        let mut actions = window
            .available_actions(cx)
            .into_iter()
            .map(|action| action.name().to_owned())
            .collect::<Vec<_>>();
        actions.sort();
        actions.dedup();
        let mut themes = ThemeRegistry::global(cx)
            .list()
            .into_iter()
            .map(|theme| theme.name.to_string())
            .collect::<Vec<_>>();
        themes.sort();
        themes.dedup();
        let installed_theme_extensions = Vec::new();
        let mut fonts = cx.text_system().all_font_names();
        if !fonts.contains(&configuration.terminal_font_family) {
            fonts.push(configuration.terminal_font_family.clone());
        }
        fonts.sort_by_key(|font| font.to_lowercase());
        fonts.dedup();
        self.settings_editor = Some(SettingsEditor {
            page: SettingsPage::Configuration,
            configuration,
            keymap,
            profile_names: self
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
            themes: themes.into(),
            theme_extension_query: TextField::default(),
            theme_extensions: Vec::new(),
            installed_theme_extensions,
            theme_extensions_loading: false,
            theme_extensions_searched: false,
            theme_extension_downloading: None,
            actions: actions.into(),
            normalized_fonts: fonts
                .iter()
                .map(|font| font.to_lowercase())
                .collect::<Vec<_>>()
                .into(),
            fonts: fonts.into(),
            font_query: None,
            profile_draft: None,
            settings_scroll: ScrollHandle::new(),
            font_scroll: UniformListScrollHandle::new(),
            numeric_repeat_generation: 0,
            scroll_geometry_initialized: false,
            focused_input: None,
            configuration_dirty: false,
            keymap_dirty: false,
            message: None,
        });
        let themes_dir = config::themes_dir();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let installed = executor
                    .spawn(async move { theme_extensions::installed(&themes_dir) })
                    .await;
                this.update_in(cx, |this, _, cx| {
                    if let (Some(editor), Ok(installed)) =
                        (this.settings_editor.as_mut(), installed)
                    {
                        editor.installed_theme_extensions = installed;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn dismiss_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_editor = None;
        self.focus_active(window, cx);
    }

    fn fetch_theme_extensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor.theme_extensions_loading {
            return;
        }
        let query = editor.theme_extension_query.text.trim().to_owned();
        if query.is_empty() {
            editor.theme_extensions.clear();
            editor.theme_extensions_searched = false;
            editor.message = Some((false, "Enter a theme name to search.".to_owned()));
            cx.notify();
            return;
        }
        editor.theme_extensions_loading = true;
        editor.theme_extensions_searched = true;
        editor.message = None;
        let http = cx.http_client();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = theme_extensions::fetch(http, &query).await;
                this.update_in(cx, |this, _, cx| {
                    let Some(editor) = this.settings_editor.as_mut() else {
                        return;
                    };
                    editor.theme_extensions_loading = false;
                    match result {
                        Ok(extensions) => editor.theme_extensions = extensions,
                        Err(error) => {
                            editor.message =
                                Some((true, format!("Could not load themes: {error:#}")));
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    fn download_theme_extension(
        &mut self,
        extension_id: Arc<str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor.theme_extension_downloading.is_some() {
            return;
        }
        let Some(extension) = editor
            .theme_extensions
            .iter()
            .find(|extension| extension.id == extension_id)
            .cloned()
        else {
            return;
        };
        editor.theme_extension_downloading = Some(extension_id);
        editor.message = Some((false, format!("Downloading {}…", extension.name)));
        let name = extension.name.clone();
        let http = cx.http_client();
        let themes_dir = config::themes_dir();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = theme_extensions::download(
                    http,
                    &extension,
                    &themes_dir,
                    executor.clone(),
                )
                .await;
                let installed_theme_extensions = if result.is_ok() {
                    executor
                        .spawn(async move { theme_extensions::installed(&themes_dir) })
                        .await
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                this.update_in(cx, |this, window, cx| {
                    if let Some(editor) = this.settings_editor.as_mut() {
                        editor.theme_extension_downloading = None;
                    }
                    match result {
                        Ok(count) => {
                            this.reload_configuration(&ReloadConfiguration, window, cx);
                            let mut themes = ThemeRegistry::global(cx)
                                .list()
                                .into_iter()
                                .map(|theme| theme.name.to_string())
                                .collect::<Vec<_>>();
                            themes.sort();
                            themes.dedup();
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.installed_theme_extensions = installed_theme_extensions;
                                editor.themes = themes.into();
                                editor.message = Some((
                                    false,
                                    format!(
                                        "Installed {name} ({count} theme file{}). Theme selectors have been reloaded.",
                                        if count == 1 { "" } else { "s" }
                                    ),
                                ));
                            }
                            this.settings_focus.focus(window, cx);
                        }
                        Err(error) => {
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.message =
                                    Some((true, format!("Could not install {name}: {error:#}")));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    fn remove_theme_extension(
        &mut self,
        extension_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(installed) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| (editor.theme_extension_downloading.is_none()).then_some(editor))
            .and_then(|editor| {
                editor
                    .installed_theme_extensions
                    .iter()
                    .find(|extension| extension.id == extension_id)
            })
            .cloned()
        else {
            return;
        };
        let in_use = self.settings_editor.as_ref().is_some_and(|editor| {
            installed.theme_names.iter().any(|theme| {
                editor.configuration.theme == *theme
                    || editor.configuration.profiles.iter().any(|profile| {
                        profile
                            .theme
                            .as_ref()
                            .is_some_and(|selected| selected == theme)
                    })
            })
        });
        if in_use {
            if let Some(editor) = self.settings_editor.as_mut() {
                editor.message = Some((
                    true,
                    "Choose and save replacement application/profile themes before removing this extension."
                        .to_owned(),
                ));
            }
            cx.notify();
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.theme_extension_downloading = Some(Arc::from(extension_id.clone()));
            editor.message = Some((false, format!("Removing {extension_id}…")));
        }

        let themes_dir = config::themes_dir();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let id_for_work = extension_id.clone();
                let result = executor
                    .spawn(async move {
                        let count = theme_extensions::remove(&id_for_work, &themes_dir)?;
                        let installed = theme_extensions::installed(&themes_dir)?;
                        anyhow::Ok((count, installed))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    if let Some(editor) = this.settings_editor.as_mut() {
                        editor.theme_extension_downloading = None;
                    }
                    match result {
                        Ok((count, installed_theme_extensions)) => {
                            let theme_names = installed
                                .theme_names
                                .iter()
                                .cloned()
                                .map(SharedString::from)
                                .collect::<Vec<_>>();
                            let registry = ThemeRegistry::global(cx);
                            registry.remove_user_themes(&theme_names);
                            theme_settings::load_bundled_themes(&registry);
                            this.reload_configuration(&ReloadConfiguration, window, cx);

                            let mut themes = ThemeRegistry::global(cx)
                                .list()
                                .into_iter()
                                .map(|theme| theme.name.to_string())
                                .collect::<Vec<_>>();
                            themes.sort();
                            themes.dedup();
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.themes = themes.into();
                                editor.installed_theme_extensions = installed_theme_extensions;
                                editor.message = Some((
                                    false,
                                    format!(
                                        "Removed {extension_id} ({count} theme file{}). Theme selectors have been reloaded.",
                                        if count == 1 { "" } else { "s" }
                                    ),
                                ));
                            }
                            this.settings_focus.focus(window, cx);
                        }
                        Err(error) => {
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.message = Some((
                                    true,
                                    format!("Could not remove {extension_id}: {error:#}"),
                                ));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    fn select_settings_page(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.page = page;
            editor.message = None;
            editor.focused_input = None;
            editor.font_query = None;
            editor.profile_draft = None;
            editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn focus_settings_input(
        &mut self,
        input: SettingsInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        editor.focused_input = Some(input);
        let field = match input {
            SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
            SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
            SettingsInput::ThemeSearch => Some(&mut editor.theme_extension_query),
            SettingsInput::FontSearch => editor.font_query.as_mut(),
            SettingsInput::ProfileDraft(field) => {
                editor.profile_draft.as_mut().map(|draft| match field {
                    ProfileDraftField::Name => &mut draft.name,
                    ProfileDraftField::Program => &mut draft.program,
                    ProfileDraftField::Arguments => &mut draft.arguments,
                })
            }
        };
        if let Some(field) = field {
            field.cursor = field.text.len();
            field.select_all =
                !matches!(input, SettingsInput::ProfileDraft(_)) && !field.text.is_empty();
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn focus_adjacent_profile_draft(
        &mut self,
        reverse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.settings_editor.as_ref().and_then(|editor| {
            if let Some(SettingsInput::ProfileDraft(field)) = editor.focused_input {
                Some(field)
            } else {
                None
            }
        });
        let field = adjacent_profile_draft_field(current, reverse);
        self.focus_settings_input(SettingsInput::ProfileDraft(field), window, cx);
    }

    fn set_settings_dropdown(
        &mut self,
        dropdown: SettingsDropdown,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        match dropdown {
            SettingsDropdown::DefaultProfile => {
                editor.configuration.default_profile = value;
            }
            SettingsDropdown::Theme => editor.configuration.theme = value,
            SettingsDropdown::ProfileTheme(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::ProfileDraftTheme => {
                if let Some(profile) = editor.profile_draft.as_mut() {
                    profile.theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::BindingAction(section, binding) => {
                if let Some(binding) = editor
                    .keymap
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.bindings.get_mut(binding))
                {
                    binding.action = serde_json::Value::String(value);
                }
            }
        }
        match dropdown {
            SettingsDropdown::BindingAction(_, _) => editor.keymap_dirty = true,
            SettingsDropdown::ProfileDraftTheme => {}
            _ => editor.configuration_dirty = true,
        }
        editor.message = None;
        cx.notify();
    }

    fn adjust_numeric_setting(
        &mut self,
        setting: NumericSetting,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let configuration = &mut editor.configuration;
        match setting {
            NumericSetting::FontSize => {
                let current = configuration
                    .terminal_font_size
                    .text
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(14.);
                let value = (current + direction as f32).clamp(6., 100.);
                configuration.terminal_font_size = TextField::new(format!("{value}"));
            }
            NumericSetting::ScrollHistory => {
                let maximum = terminal::MAX_SCROLL_HISTORY_LINES as u64;
                let current = if configuration
                    .max_scroll_history_lines
                    .text
                    .trim()
                    .eq_ignore_ascii_case("max")
                {
                    maximum
                } else {
                    configuration
                        .max_scroll_history_lines
                        .text
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0)
                        .min(maximum)
                };
                let value = adjusted_scroll_history(current, direction, maximum);
                configuration.max_scroll_history_lines = TextField::new(if value == maximum {
                    "Max".to_owned()
                } else {
                    value.to_string()
                });
            }
        }
        editor.configuration_dirty = true;
        editor.message = None;
        cx.notify();
    }

    fn begin_numeric_repeat(
        &mut self,
        setting: NumericSetting,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        let generation = editor.numeric_repeat_generation;
        self.adjust_numeric_setting(setting, direction, cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            loop {
                let repeating = this
                    .update(cx, |this, cx| {
                        let repeating = this
                            .settings_editor
                            .as_ref()
                            .is_some_and(|editor| editor.numeric_repeat_generation == generation);
                        if repeating {
                            this.adjust_numeric_setting(setting, direction, cx);
                        }
                        repeating
                    })
                    .unwrap_or(false);
                if !repeating {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(75))
                    .await;
            }
        })
        .detach();
    }

    fn end_numeric_repeat(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        }
        cx.notify();
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.settings_editor.as_ref() else {
            return;
        };
        let config_path = self.launch_config.config_path.clone();
        let result = (|| -> Result<()> {
            let keymap = if editor.keymap_dirty {
                let keymap = editor.keymap.to_json()?;
                validate_keymap_contents(&keymap, cx)?;
                Some(keymap)
            } else {
                None
            };
            let configuration = if editor.configuration_dirty {
                let configuration = editor.configuration.to_json()?;
                Config::parse(
                    &configuration,
                    Some(&config_path),
                    self.launch_config.keymap_override.clone(),
                )?;
                Some(configuration)
            } else {
                None
            };

            if let Some(keymap) = keymap {
                save_settings_file(&self.launch_config.keymap_path, &keymap)?;
            }
            if let Some(configuration) = configuration {
                save_settings_file(&config_path, &configuration)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.settings_editor = None;
                self.reload_configuration(&ReloadConfiguration, window, cx);
            }
            Err(error) => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.message = Some((true, format!("Not saved: {error:#}")));
                }
                cx.notify();
            }
        }
    }

    fn settings_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.settings_editor.as_ref().is_some_and(|editor| {
                    editor.font_query.is_some() || editor.profile_draft.is_some()
                }) {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.font_query = None;
                        editor.profile_draft = None;
                        editor.focused_input = None;
                        editor.message = None;
                    }
                    cx.notify();
                } else {
                    self.dismiss_settings(window, cx);
                }
            }
            "s" if command => self.save_settings(window, cx),
            "1" if command => self.select_settings_page(SettingsPage::Configuration, window, cx),
            "2" if command => self.select_settings_page(SettingsPage::Themes, window, cx),
            "3" if command => self.select_settings_page(SettingsPage::Keymap, window, cx),
            "tab"
                if self
                    .settings_editor
                    .as_ref()
                    .is_some_and(|editor| editor.profile_draft.is_some()) =>
            {
                self.focus_adjacent_profile_draft(event.keystroke.modifiers.shift, window, cx);
            }
            "enter" => {
                let search = self
                    .settings_editor
                    .as_ref()
                    .is_some_and(|editor| editor.focused_input == Some(SettingsInput::ThemeSearch));
                if search {
                    self.fetch_theme_extensions(window, cx);
                } else if let Some(editor) = self.settings_editor.as_mut() {
                    editor.focused_input = None;
                    cx.notify();
                }
            }
            key => {
                let Some(editor) = self.settings_editor.as_mut() else {
                    return;
                };
                let Some(input) = editor.focused_input else {
                    cx.stop_propagation();
                    return;
                };
                let field = match input {
                    SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
                    SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
                    SettingsInput::ThemeSearch => Some(&mut editor.theme_extension_query),
                    SettingsInput::FontSearch => editor.font_query.as_mut(),
                    SettingsInput::ProfileDraft(field) => {
                        editor.profile_draft.as_mut().map(|draft| match field {
                            ProfileDraftField::Name => &mut draft.name,
                            ProfileDraftField::Program => &mut draft.program,
                            ProfileDraftField::Arguments => &mut draft.arguments,
                        })
                    }
                };
                let Some(field) = field else {
                    return;
                };
                match key {
                    "backspace" => field.backspace(),
                    "delete" => field.delete(),
                    "left" => field.move_left(),
                    "right" => field.move_right(),
                    "home" => {
                        field.cursor = 0;
                        field.select_all = false;
                    }
                    "end" => {
                        field.cursor = field.text.len();
                        field.select_all = false;
                    }
                    "a" if command => field.select_all(),
                    "v" if command => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            field.insert(&text);
                        }
                    }
                    _ if !command && !event.keystroke.modifiers.alt => {
                        if let Some(text) = event.keystroke.key_char.as_ref() {
                            field.insert(text);
                        }
                    }
                    _ => {}
                }
                match input {
                    SettingsInput::Configuration(_) => editor.configuration_dirty = true,
                    SettingsInput::Keymap(_) => editor.keymap_dirty = true,
                    SettingsInput::ThemeSearch
                    | SettingsInput::FontSearch
                    | SettingsInput::ProfileDraft(_) => {}
                }
                editor.message = None;
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    fn render_settings_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let editor = self.settings_editor.as_ref()?;
        let colors = cx.theme().colors().clone();
        let handle = cx.entity().downgrade();
        if !editor.scroll_geometry_initialized {
            let geometry_handle = handle.clone();
            window.on_next_frame(move |_, cx| {
                geometry_handle
                    .update(cx, |this, cx| {
                        if let Some(editor) = this.settings_editor.as_mut() {
                            editor.scroll_geometry_initialized = true;
                            cx.notify();
                        }
                    })
                    .ok();
            });
        }

        let scroll_indicator = |id: String, scroll: &ScrollHandle| -> gpui::AnyElement {
            let viewport = scroll.bounds().size.height;
            let maximum = scroll.max_offset().y;
            let content_height = viewport + maximum;
            let thumb_fraction = if content_height > px(0.) {
                (viewport / content_height).clamp(0.08, 1.)
            } else {
                1.
            };
            let progress = if maximum > px(0.) {
                (-scroll.offset().y / maximum).clamp(0., 1.)
            } else {
                0.
            };
            let top_fraction = progress * (1. - thumb_fraction);
            let click_scroll = scroll.clone();
            let click_handle = handle.clone();
            let wheel_scroll = scroll.clone();
            let wheel_handle = handle.clone();
            div()
                .id(id)
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(px(10.))
                .bg(colors.scrollbar_track_background)
                .cursor_pointer()
                .child(
                    div()
                        .absolute()
                        .right(px(2.))
                        .top(gpui::relative(top_fraction))
                        .h(gpui::relative(thumb_fraction))
                        .w(px(6.))
                        .rounded_full()
                        .bg(colors.scrollbar_thumb_background),
                )
                .on_scroll_wheel(move |event, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height());
                    let offset = wheel_scroll.offset();
                    let minimum = -wheel_scroll.max_offset().y;
                    wheel_scroll
                        .set_offset(point(offset.x, (offset.y + delta.y).clamp(minimum, px(0.))));
                    wheel_handle.update(cx, |_, cx| cx.notify()).ok();
                    cx.stop_propagation();
                })
                .on_click(move |event, _, cx| {
                    let bounds = click_scroll.bounds();
                    let maximum = click_scroll.max_offset().y;
                    if bounds.size.height > px(0.) && maximum > px(0.) {
                        let progress = ((event.position().y - bounds.top()) / bounds.size.height)
                            .clamp(0., 1.);
                        let offset = click_scroll.offset();
                        click_scroll.set_offset(point(offset.x, -(maximum * progress)));
                        click_handle.update(cx, |_, cx| cx.notify()).ok();
                    }
                    cx.stop_propagation();
                })
                .into_any_element()
        };

        let text_input = |id: String, field: TextField, input: SettingsInput| -> gpui::AnyElement {
            let focused = editor.focused_input == Some(input);
            let centered = matches!(
                input,
                SettingsInput::Configuration(
                    ConfigTextField::FontSize | ConfigTextField::ScrollHistory
                )
            );
            let cursor = field.cursor.min(field.text.len());
            let (before, after) = field.text.split_at(cursor);
            let input_handle = handle.clone();
            div()
                .id(id)
                .h_9()
                .w_full()
                .min_w(px(180.))
                .px_2()
                .flex()
                .items_center()
                .when(centered, |input| input.justify_center().text_center())
                .overflow_hidden()
                .rounded(px(4.))
                .border_1()
                .border_color(if focused {
                    colors.border_focused
                } else {
                    colors.border
                })
                .bg(colors.editor_background)
                .cursor_text()
                .when(field.select_all && focused, |input| {
                    input.bg(colors.element_selection_background)
                })
                .when(!focused, |input| {
                    input.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(field.text.clone()),
                    )
                })
                .when(focused, |input| {
                    input
                        .child(div().whitespace_nowrap().child(before.to_owned()))
                        .when(!field.select_all, |input| {
                            input.child(
                                div()
                                    .flex_none()
                                    .w(px(1.))
                                    .h(px(16.))
                                    .bg(colors.text_accent),
                            )
                        })
                        .child(div().whitespace_nowrap().child(after.to_owned()))
                })
                .on_click(move |_, window, cx| {
                    input_handle
                        .update(cx, |this, cx| this.focus_settings_input(input, window, cx))
                        .ok();
                })
                .into_any_element()
        };

        let dropdown = |id: String,
                        label: String,
                        options: Arc<[String]>,
                        selection: SettingsDropdown,
                        _window: &mut Window,
                        _cx: &mut Context<Self>|
         -> gpui::AnyElement {
            let menu_handle = handle.clone();
            let selected = label.clone();
            let is_binding_action = matches!(selection, SettingsDropdown::BindingAction(_, _));
            let trigger = ButtonLike::new(id.clone())
                .style(ButtonStyle::Outlined)
                .full_width()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(Label::new(label))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
                );
            PopoverMenu::new(format!("{id}-popover"))
                .full_width(true)
                .trigger(trigger)
                .anchor(Anchor::TopLeft)
                .menu(move |window, cx| {
                    let options = options.clone();
                    let selected = selected.clone();
                    let menu_handle = menu_handle.clone();
                    Some(ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                        for option in options.iter() {
                            let value = option.clone();
                            let option_label = option.clone();
                            let toggled = option_label == selected;
                            let handle = menu_handle.clone();
                            if is_binding_action {
                                let rendered_label = option_label.clone();
                                menu = menu.custom_entry(
                                    move |_, _| {
                                        h_flex()
                                            .gap_2()
                                            .whitespace_nowrap()
                                            .child(
                                                div()
                                                    .w(px(16.))
                                                    .flex_none()
                                                    .text_center()
                                                    .child(if toggled { "✓" } else { "" }),
                                            )
                                            .child(Label::new(rendered_label.clone()))
                                            .into_any_element()
                                    },
                                    move |_, cx| {
                                        handle
                                            .update(cx, |this, cx| {
                                                this.set_settings_dropdown(
                                                    selection,
                                                    value.clone(),
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    },
                                );
                            } else {
                                menu = menu.toggleable_entry(
                                    option_label,
                                    toggled,
                                    IconPosition::Start,
                                    None,
                                    move |_, cx| {
                                        handle
                                            .update(cx, |this, cx| {
                                                this.set_settings_dropdown(
                                                    selection,
                                                    value.clone(),
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    },
                                );
                            }
                        }
                        menu
                    }))
                })
                .into_any_element()
        };

        let setting_row =
            |label: &'static str, description: &'static str, control: gpui::AnyElement| {
                h_flex()
                    .w_full()
                    .min_h(px(54.))
                    .py_2()
                    .gap_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(div().text_sm().text_color(colors.text).child(label))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.text_muted)
                                    .child(description),
                            ),
                    )
                    .child(div().w(px(330.)).flex_none().child(control))
                    .into_any_element()
            };

        let numeric = |id: &'static str,
                       field: TextField,
                       setting: NumericSetting,
                       input: ConfigTextField|
         -> gpui::AnyElement {
            let decrease_down = handle.clone();
            let decrease_up = handle.clone();
            let decrease_out = handle.clone();
            let increase_down = handle.clone();
            let increase_up = handle.clone();
            let increase_out = handle.clone();
            h_flex()
                .id(id)
                .h_9()
                .w_full()
                .rounded(px(4.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.editor_background)
                .child(
                    div()
                        .id(format!("{id}-decrease"))
                        .h_full()
                        .w_9()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child("−")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            decrease_down
                                .update(cx, |this, cx| this.begin_numeric_repeat(setting, -1, cx))
                                .ok();
                        })
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            decrease_up
                                .update(cx, |this, cx| this.end_numeric_repeat(cx))
                                .ok();
                        })
                        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                            decrease_out
                                .update(cx, |this, cx| this.end_numeric_repeat(cx))
                                .ok();
                        }),
                )
                .child(div().min_w_0().flex_1().child(text_input(
                    format!("{id}-value"),
                    field,
                    SettingsInput::Configuration(input),
                )))
                .child(
                    div()
                        .id(format!("{id}-increase"))
                        .h_full()
                        .w_9()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child("+")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            increase_down
                                .update(cx, |this, cx| this.begin_numeric_repeat(setting, 1, cx))
                                .ok();
                        })
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            increase_up
                                .update(cx, |this, cx| this.end_numeric_repeat(cx))
                                .ok();
                        })
                        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                            increase_out
                                .update(cx, |this, cx| this.end_numeric_repeat(cx))
                                .ok();
                        }),
                )
                .into_any_element()
        };
        let opacity_slider = |opacity: f32| -> gpui::AnyElement {
            let selected = (opacity.clamp(0., 1.) * 20.).round() as usize;
            let stops = (0usize..=20)
                .map(|step| {
                    let slider_handle = handle.clone();
                    div()
                        .id(("inactive-opacity-stop", step))
                        .h_full()
                        .flex_1()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            slider_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut() {
                                        editor.configuration.inactive_pane_opacity =
                                            step as f32 / 20.;
                                        editor.configuration_dirty = true;
                                        editor.message = None;
                                        cx.notify();
                                    }
                                })
                                .ok();
                        })
                })
                .collect::<Vec<_>>();
            let fraction = selected as f32 / 20.;
            h_flex()
                .w_full()
                .gap_3()
                .child(
                    div()
                        .relative()
                        .h_5()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .h_1()
                                .rounded_full()
                                .bg(colors.element_background),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .w(gpui::relative(fraction))
                                .h_1()
                                .rounded_full()
                                .bg(colors.text_accent),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(gpui::relative(fraction))
                                .ml(px(-5.))
                                .size(px(10.))
                                .rounded_full()
                                .border_1()
                                .border_color(colors.border_focused)
                                .bg(colors.text_accent),
                        )
                        .child(h_flex().absolute().inset_0().children(stops)),
                )
                .child(
                    div()
                        .w(px(44.))
                        .text_right()
                        .text_sm()
                        .child(format!("{}%", selected * 5)),
                )
                .into_any_element()
        };

        let content = match editor.page {
            SettingsPage::Configuration => {
                let configuration = &editor.configuration;
                let mut profile_names = editor.profile_names.clone();
                profile_names.extend(
                    configuration
                        .profiles
                        .iter()
                        .map(|profile| profile.name.text.clone())
                        .filter(|name| !name.trim().is_empty()),
                );
                profile_names.sort();
                profile_names.dedup();
                let default_profile = dropdown(
                    "settings-default-profile".to_owned(),
                    configuration.default_profile.clone(),
                    profile_names.into(),
                    SettingsDropdown::DefaultProfile,
                    window,
                    cx,
                );
                let theme = dropdown(
                    "settings-theme".to_owned(),
                    configuration.theme.clone(),
                    editor.themes.clone(),
                    SettingsDropdown::Theme,
                    window,
                    cx,
                );
                let current_font = configuration.terminal_font_family.clone();
                let picker_handle = handle.clone();
                let font_family = h_flex()
                    .id("terminal-font-family-picker-trigger")
                    .h_9()
                    .w_full()
                    .min_w(px(180.))
                    .px_3()
                    .justify_between()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.editor_background)
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.element_hover))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(current_font.clone())
                            .child(current_font),
                    )
                    .child(
                        svg()
                            .path(IconName::ChevronDown.path())
                            .size(px(14.))
                            .text_color(colors.icon_muted),
                    )
                    .on_click(move |_, window, cx| {
                        picker_handle
                            .update(cx, |this, cx| {
                                if let Some(editor) = this.settings_editor.as_mut() {
                                    editor.font_query = Some(TextField::default());
                                    editor.scroll_geometry_initialized = false;
                                }
                                this.focus_settings_input(SettingsInput::FontSearch, window, cx);
                            })
                            .ok();
                    })
                    .into_any_element();
                let mut rows = vec![
                    setting_row(
                        "Default profile",
                        "Profile selected when Zetta starts",
                        default_profile,
                    ),
                    setting_row("Theme", "Application color theme", theme),
                    setting_row(
                        "Terminal font size",
                        "Point size from 6 through 100",
                        numeric(
                            "settings-font-size",
                            configuration.terminal_font_size.clone(),
                            NumericSetting::FontSize,
                            ConfigTextField::FontSize,
                        ),
                    ),
                    setting_row(
                        "Terminal font family",
                        "Search bundled and system-installed font families",
                        font_family,
                    ),
                    setting_row(
                        "Working directory",
                        "Initial directory; ~ expands to your home directory",
                        text_input(
                            "settings-working-directory".to_owned(),
                            configuration.working_directory.clone(),
                            SettingsInput::Configuration(ConfigTextField::WorkingDirectory),
                        ),
                    ),
                    setting_row(
                        "Scrollback history",
                        "Enter 0 through Max; steppers accelerate across the range",
                        numeric(
                            "settings-scroll-history",
                            configuration.max_scroll_history_lines.clone(),
                            NumericSetting::ScrollHistory,
                            ConfigTextField::ScrollHistory,
                        ),
                    ),
                    setting_row(
                        "Inactive pane opacity",
                        "Dimming level as a percentage",
                        opacity_slider(configuration.inactive_pane_opacity),
                    ),
                ];
                rows.push(
                    div()
                        .pt_4()
                        .pb_2()
                        .text_sm()
                        .text_color(colors.text_muted)
                        .child("Profiles")
                        .into_any_element(),
                );
                for (index, profile) in configuration.profiles.iter().enumerate() {
                    let mut theme_options = vec!["Use application theme".to_owned()];
                    theme_options.extend(editor.themes.iter().cloned());
                    let profile_theme = profile
                        .theme
                        .clone()
                        .unwrap_or_else(|| "Use application theme".to_owned());
                    let profile_theme = dropdown(
                        format!("settings-profile-{index}-theme"),
                        profile_theme,
                        theme_options.into(),
                        SettingsDropdown::ProfileTheme(index),
                        window,
                        cx,
                    );
                    let card = if profile.detected {
                        h_flex()
                            .p_3()
                            .mb_2()
                            .gap_4()
                            .justify_between()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.editor_background)
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(colors.text)
                                            .child(profile.name.text.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors.text_muted)
                                            .child("Detected profile"),
                                    ),
                            )
                            .child(div().w(px(330.)).flex_none().child(profile_theme))
                            .into_any_element()
                    } else {
                        let remove_handle = handle.clone();
                        div()
                            .p_3()
                            .mb_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.editor_background)
                            .child(
                                h_flex()
                                    .items_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .mb_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child("Profile name"),
                                            )
                                            .child(text_input(
                                                format!("settings-profile-{index}-name"),
                                                profile.name.clone(),
                                                SettingsInput::Configuration(
                                                    ConfigTextField::ProfileName(index),
                                                ),
                                            )),
                                    )
                                    .child(
                                        IconButton::new(
                                            ("remove-settings-profile", index),
                                            IconName::Trash,
                                        )
                                        .icon_size(IconSize::Small)
                                        .tooltip(Tooltip::text("Remove profile"))
                                        .on_click(
                                            move |_, _, cx| {
                                                remove_handle
                                                    .update(cx, |this, cx| {
                                                        if let Some(editor) =
                                                            this.settings_editor.as_mut()
                                                        {
                                                            editor
                                                                .configuration
                                                                .profiles
                                                                .remove(index);
                                                            editor.configuration_dirty = true;
                                                            cx.notify();
                                                        }
                                                    })
                                                    .ok();
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .mt_2()
                                    .items_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .mb_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child("Program"),
                                            )
                                            .child(text_input(
                                                format!("settings-profile-{index}-program"),
                                                profile.program.clone(),
                                                SettingsInput::Configuration(
                                                    ConfigTextField::ProfileProgram(index),
                                                ),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .mb_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child("Arguments (comma separated)"),
                                            )
                                            .child(text_input(
                                                format!("settings-profile-{index}-arguments"),
                                                profile.arguments.clone(),
                                                SettingsInput::Configuration(
                                                    ConfigTextField::ProfileArguments(index),
                                                ),
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .child(
                                        div()
                                            .mb_1()
                                            .text_xs()
                                            .text_color(colors.text_muted)
                                            .child("Theme"),
                                    )
                                    .child(profile_theme),
                            )
                            .into_any_element()
                    };
                    rows.push(card);
                }
                let add_handle = handle.clone();
                rows.push(
                    div()
                        .id("add-settings-profile")
                        .h_9()
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(colors.border)
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child("Add profile")
                        .on_click(move |_, window, cx| {
                            add_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut() {
                                        editor.profile_draft = Some(settings_editor::ProfileForm {
                                            name: TextField::default(),
                                            program: TextField::default(),
                                            arguments: TextField::default(),
                                            theme: None,
                                            detected: false,
                                        });
                                        editor.message = None;
                                    }
                                    this.focus_settings_input(
                                        SettingsInput::ProfileDraft(ProfileDraftField::Name),
                                        window,
                                        cx,
                                    );
                                })
                                .ok();
                        })
                        .into_any_element(),
                );
                div().children(rows).into_any_element()
            }
            SettingsPage::Themes => {
                let search = text_input(
                    "settings-theme-extension-search".to_owned(),
                    editor.theme_extension_query.clone(),
                    SettingsInput::ThemeSearch,
                );
                let search_handle = handle.clone();
                let mut rows = vec![
                    div()
                        .mb_3()
                        .child(
                            div()
                                .mb_1()
                                .text_sm()
                                .child("Download themes from Zed extensions"),
                        )
                        .child(
                            div()
                                .mb_3()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(
                                    "Only declared theme JSON files are installed. Other extension features are ignored.",
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().child(search))
                                .child(
                                    div()
                                        .id("search-theme-extensions")
                                        .h_9()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(colors.border)
                                        .cursor_pointer()
                                        .hover(|style| style.bg(colors.element_hover))
                                        .on_click(move |_, window, cx| {
                                            search_handle
                                                .update(cx, |this, cx| {
                                                    this.fetch_theme_extensions(window, cx)
                                                })
                                                .ok();
                                        })
                                        .child(if editor.theme_extensions_loading {
                                            "Loading…"
                                        } else {
                                            "Search"
                                        }),
                                ),
                        )
                        .into_any_element(),
                ];
                if !editor.installed_theme_extensions.is_empty() {
                    rows.push(
                        div()
                            .mt_2()
                            .mb_2()
                            .text_sm()
                            .child("Installed from Zed extensions")
                            .into_any_element(),
                    );
                    for installed in &editor.installed_theme_extensions {
                        let id = installed.id.clone();
                        let removing = editor
                            .theme_extension_downloading
                            .as_ref()
                            .is_some_and(|active| active.as_ref() == installed.id);
                        let disabled = editor.theme_extension_downloading.is_some();
                        let remove_handle = handle.clone();
                        let theme_names = installed.theme_names.join(", ");
                        rows.push(
                            div()
                                .mb_2()
                                .p_3()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(colors.border)
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .gap_3()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .child(div().text_sm().child(installed.id.clone()))
                                                .child(
                                                    div()
                                                        .mt_1()
                                                        .text_xs()
                                                        .text_color(colors.text_muted)
                                                        .child(format!(
                                                            "{} theme file{}{}",
                                                            installed.file_count,
                                                            if installed.file_count == 1 {
                                                                ""
                                                            } else {
                                                                "s"
                                                            },
                                                            if theme_names.is_empty() {
                                                                String::new()
                                                            } else {
                                                                format!(" · {theme_names}")
                                                            }
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id(format!(
                                                    "remove-theme-extension-{}",
                                                    installed.id
                                                ))
                                                .h_8()
                                                .px_3()
                                                .flex()
                                                .items_center()
                                                .flex_none()
                                                .rounded(px(4.))
                                                .border_1()
                                                .border_color(colors.border)
                                                .when(!disabled, |button| {
                                                    button
                                                        .cursor_pointer()
                                                        .hover(|style| {
                                                            style.bg(colors.element_hover)
                                                        })
                                                        .on_click(move |_, window, cx| {
                                                            remove_handle
                                                                .update(cx, |this, cx| {
                                                                    this.remove_theme_extension(
                                                                        id.clone(),
                                                                        window,
                                                                        cx,
                                                                    )
                                                                })
                                                                .ok();
                                                        })
                                                })
                                                .child(if removing {
                                                    "Removing…"
                                                } else {
                                                    "Remove"
                                                }),
                                        ),
                                )
                                .into_any_element(),
                        );
                    }
                }
                if editor.theme_extensions.is_empty() && !editor.theme_extensions_loading {
                    rows.push(
                        div()
                            .py_6()
                            .text_center()
                            .text_color(colors.text_muted)
                            .child(if editor.theme_extensions_searched {
                                "No matching theme extensions found."
                            } else {
                                "Enter a theme name and select Search."
                            })
                            .into_any_element(),
                    );
                }
                for extension in &editor.theme_extensions {
                    let id = extension.id.clone();
                    let downloading = editor
                        .theme_extension_downloading
                        .as_ref()
                        .is_some_and(|active| active == &id);
                    let already_installed = editor
                        .installed_theme_extensions
                        .iter()
                        .any(|installed| installed.id == extension.id.as_ref());
                    let disabled =
                        editor.theme_extension_downloading.is_some() || already_installed;
                    let install_handle = handle.clone();
                    let description = extension
                        .description
                        .clone()
                        .unwrap_or_else(|| "Theme extension for Zed".to_owned());
                    let author = if extension.authors.is_empty() {
                        String::new()
                    } else {
                        format!(" by {}", extension.authors.join(", "))
                    };
                    rows.push(
                        div()
                            .mb_2()
                            .p_3()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(colors.border)
                            .child(
                                h_flex()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .child(format!("{}{}", extension.name, author)),
                                            )
                                            .child(
                                                div()
                                                    .mt_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child(description),
                                            )
                                            .child(
                                                div()
                                                    .mt_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child(format!(
                                                        "{} downloads · version {}",
                                                        extension.download_count, extension.version
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id(format!("install-theme-extension-{}", extension.id))
                                            .h_8()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .flex_none()
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(colors.border)
                                            .when(!disabled, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(colors.element_hover))
                                                    .on_click(move |_, window, cx| {
                                                        install_handle
                                                            .update(cx, |this, cx| {
                                                                this.download_theme_extension(
                                                                    id.clone(),
                                                                    window,
                                                                    cx,
                                                                )
                                                            })
                                                            .ok();
                                                    })
                                            })
                                            .child(if downloading {
                                                "Installing…"
                                            } else if already_installed {
                                                "Installed"
                                            } else {
                                                "Install"
                                            }),
                                    ),
                            )
                            .into_any_element(),
                    );
                }
                div().children(rows).into_any_element()
            }
            SettingsPage::Keymap => {
                let mut sections = Vec::new();
                for (section_index, section) in editor.keymap.sections.iter().enumerate() {
                    let mut bindings = Vec::new();
                    for (binding_index, binding) in section.bindings.iter().enumerate() {
                        let action = dropdown(
                            format!("settings-binding-{section_index}-{binding_index}-action"),
                            binding.action_name(),
                            editor.actions.clone(),
                            SettingsDropdown::BindingAction(section_index, binding_index),
                            window,
                            cx,
                        );
                        let remove_handle = handle.clone();
                        bindings.push(
                            h_flex()
                                .mb_2()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(220.))
                                        .flex_none()
                                        .child(text_input(
                                            format!(
                                                "settings-binding-{section_index}-{binding_index}-key"
                                            ),
                                            binding.keystroke.clone(),
                                            SettingsInput::Keymap(
                                                KeymapTextField::Keystroke(
                                                    section_index,
                                                    binding_index,
                                                ),
                                            ),
                                        )),
                                )
                                .child(div().min_w_0().flex_1().child(action))
                                .child(
                                    IconButton::new(
                                        format!("remove-settings-binding-{section_index}-{binding_index}"),
                                        IconName::Trash,
                                    )
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text("Remove binding"))
                                    .on_click(move |_, _, cx| {
                                        remove_handle
                                            .update(cx, |this, cx| {
                                                if let Some(editor) =
                                                    this.settings_editor.as_mut()
                                                {
                                                    editor.keymap.sections[section_index]
                                                        .bindings
                                                        .remove(binding_index);
                                                    editor.keymap_dirty = true;
                                                    cx.notify();
                                                }
                                            })
                                            .ok();
                                    }),
                                )
                                .into_any_element(),
                        );
                    }
                    let add_handle = handle.clone();
                    sections.push(
                        div()
                            .p_3()
                            .mb_3()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.editor_background)
                            .child(
                                h_flex()
                                    .mb_3()
                                    .gap_2()
                                    .child(div().flex_none().text_sm().child("Context"))
                                    .child(div().min_w_0().flex_1().child(text_input(
                                        format!("settings-keymap-section-{section_index}-context"),
                                        section.context.clone(),
                                        SettingsInput::Keymap(KeymapTextField::Context(
                                            section_index,
                                        )),
                                    ))),
                            )
                            .children(bindings)
                            .child(
                                div()
                                    .id(("add-settings-binding", section_index))
                                    .h_8()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(colors.border)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(colors.element_hover))
                                    .child("Add binding")
                                    .on_click(move |_, _, cx| {
                                        add_handle
                                            .update(cx, |this, cx| {
                                                if let Some(editor) = this.settings_editor.as_mut()
                                                {
                                                    editor.keymap.sections[section_index]
                                                        .bindings
                                                        .push(BindingForm {
                                                            keystroke: TextField::new(
                                                                "ctrl-shift-x",
                                                            ),
                                                            action: serde_json::Value::String(
                                                                "zetta::NewTab".to_owned(),
                                                            ),
                                                        });
                                                    editor.keymap_dirty = true;
                                                    cx.notify();
                                                }
                                            })
                                            .ok();
                                    }),
                            )
                            .into_any_element(),
                    );
                }
                let add_handle = handle.clone();
                sections.push(
                    div()
                        .id("add-keymap-section")
                        .h_9()
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(colors.border)
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child("Add keymap context")
                        .on_click(move |_, _, cx| {
                            add_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut() {
                                        editor
                                            .keymap
                                            .sections
                                            .push(KeymapSectionForm::new("Zetta > Terminal"));
                                        editor.keymap_dirty = true;
                                        cx.notify();
                                    }
                                })
                                .ok();
                        })
                        .into_any_element(),
                );
                div().children(sections).into_any_element()
            }
        };

        let font_modal = editor.font_query.as_ref().map(|query| {
            let current_font = editor.configuration.terminal_font_family.clone();
            let close_handle = handle.clone();
            let filtered_fonts = matching_font_indices(&editor.normalized_fonts, &query.text);
            let fonts = editor.fonts.clone();
            let font_handle = handle.clone();
            let font_colors = colors.clone();
            let font_rows = uniform_list(
                "settings-font-list",
                filtered_fonts.len(),
                move |range, _, _| {
                    range
                        .map(|row_index| {
                            let index = filtered_fonts[row_index];
                            let font = &fonts[index];
                            let selected = *font == current_font;
                            let value = font.clone();
                            let row_handle = font_handle.clone();
                            h_flex()
                                .id(("settings-font-option", index))
                                .h_10()
                                .px_3()
                                .justify_between()
                                .cursor_pointer()
                                .rounded(px(4.))
                                .when(selected, |row| row.bg(font_colors.element_selected))
                                .hover(|style| style.bg(font_colors.element_hover))
                                .child(
                                    div()
                                        .font_family(font.clone())
                                        .text_sm()
                                        .child(font.clone()),
                                )
                                .when(selected, |row| {
                                    row.child(
                                        svg()
                                            .path(IconName::Check.path())
                                            .size(px(14.))
                                            .text_color(font_colors.text_accent),
                                    )
                                })
                                .on_click(move |_, _, cx| {
                                    row_handle
                                        .update(cx, |this, cx| {
                                            if let Some(editor) = this.settings_editor.as_mut() {
                                                editor.configuration.terminal_font_family =
                                                    value.clone();
                                                editor.configuration_dirty = true;
                                                editor.font_query = None;
                                                editor.focused_input = None;
                                                editor.message = None;
                                                cx.notify();
                                            }
                                        })
                                        .ok();
                                })
                        })
                        .collect::<Vec<_>>()
                },
            )
            .h_full()
            .track_scroll(&editor.font_scroll);
            let font_scroll = editor.font_scroll.0.borrow().base_handle.clone();
            div()
                .id("font-picker-modal")
                .absolute()
                .inset_0()
                .p_8()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.55))
                .occlude()
                .child(
                    div()
                        .w_full()
                        .max_w(px(560.))
                        .h_full()
                        .max_h(px(520.))
                        .p_3()
                        .flex()
                        .flex_col()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .mb_3()
                                .gap_2()
                                .child(div().min_w_0().flex_1().child(text_input(
                                    "settings-font-search".to_owned(),
                                    query.clone(),
                                    SettingsInput::FontSearch,
                                )))
                                .child(
                                    IconButton::new("close-font-picker", IconName::Close)
                                        .icon_size(IconSize::Small)
                                        .tooltip(Tooltip::text("Close font picker"))
                                        .on_click(move |_, _, cx| {
                                            close_handle
                                                .update(cx, |this, cx| {
                                                    if let Some(editor) =
                                                        this.settings_editor.as_mut()
                                                    {
                                                        editor.font_query = None;
                                                        editor.focused_input = None;
                                                        cx.notify();
                                                    }
                                                })
                                                .ok();
                                        }),
                                ),
                        )
                        .child(div().relative().min_h_0().flex_1().child(font_rows).child(
                            scroll_indicator("settings-font-scrollbar".to_owned(), &font_scroll),
                        )),
                )
                .into_any_element()
        });

        let profile_modal = editor.profile_draft.as_ref().map(|draft| {
            let mut theme_options = vec!["Use application theme".to_owned()];
            theme_options.extend(editor.themes.iter().cloned());
            let profile_theme = dropdown(
                "settings-new-profile-theme".to_owned(),
                draft
                    .theme
                    .clone()
                    .unwrap_or_else(|| "Use application theme".to_owned()),
                theme_options.into(),
                SettingsDropdown::ProfileDraftTheme,
                window,
                cx,
            );
            let cancel_handle = handle.clone();
            let create_handle = handle.clone();
            div()
                .id("new-profile-modal")
                .absolute()
                .inset_0()
                .p_8()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.55))
                .occlude()
                .child(
                    div()
                        .id("new-profile-form")
                        .w_full()
                        .max_w(px(640.))
                        .p_6()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .mb_4()
                                .justify_between()
                                .child(div().text_lg().child("Add profile"))
                                .child(
                                    IconButton::new("close-new-profile", IconName::Close)
                                        .icon_size(IconSize::Small)
                                        .on_click(move |_, _, cx| {
                                            cancel_handle
                                                .update(cx, |this, cx| {
                                                    if let Some(editor) =
                                                        this.settings_editor.as_mut()
                                                    {
                                                        editor.profile_draft = None;
                                                        editor.focused_input = None;
                                                        editor.message = None;
                                                        cx.notify();
                                                    }
                                                })
                                                .ok();
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .mb_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Profile name"),
                        )
                        .child(text_input(
                            "settings-new-profile-name".to_owned(),
                            draft.name.clone(),
                            SettingsInput::ProfileDraft(ProfileDraftField::Name),
                        ))
                        .child(
                            div()
                                .mt_3()
                                .mb_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Program"),
                        )
                        .child(text_input(
                            "settings-new-profile-program".to_owned(),
                            draft.program.clone(),
                            SettingsInput::ProfileDraft(ProfileDraftField::Program),
                        ))
                        .child(
                            div()
                                .mt_3()
                                .mb_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Arguments (comma separated)"),
                        )
                        .child(text_input(
                            "settings-new-profile-arguments".to_owned(),
                            draft.arguments.clone(),
                            SettingsInput::ProfileDraft(ProfileDraftField::Arguments),
                        ))
                        .child(
                            div()
                                .mt_3()
                                .mb_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Theme"),
                        )
                        .child(profile_theme)
                        .when_some(editor.message.clone(), |modal, (_, message)| {
                            modal.child(
                                div()
                                    .mt_3()
                                    .text_xs()
                                    .text_color(colors.text)
                                    .child(message),
                            )
                        })
                        .child(
                            h_flex().mt_5().justify_end().child(
                                div()
                                    .id("create-settings-profile")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .bg(colors.element_selected)
                                    .hover(|style| style.bg(colors.element_hover))
                                    .child("Create profile")
                                    .on_click(move |_, _, cx| {
                                        create_handle
                                            .update(cx, |this, cx| {
                                                let Some(editor) = this.settings_editor.as_mut()
                                                else {
                                                    return;
                                                };
                                                let valid = editor
                                                    .profile_draft
                                                    .as_ref()
                                                    .is_some_and(|draft| {
                                                        !draft.name.text.trim().is_empty()
                                                            && !draft.program.text.trim().is_empty()
                                                    });
                                                if !valid {
                                                    editor.message = Some((
                                                        true,
                                                        "Profile name and program are required."
                                                            .to_owned(),
                                                    ));
                                                    cx.notify();
                                                    return;
                                                }
                                                let draft = editor.profile_draft.take().unwrap();
                                                editor.configuration.profiles.push(draft);
                                                editor.configuration_dirty = true;
                                                editor.focused_input = None;
                                                editor.message = None;
                                                cx.notify();
                                            })
                                            .ok();
                                    }),
                            ),
                        ),
                )
                .into_any_element()
        });

        let config_handle = handle.clone();
        let themes_handle = handle.clone();
        let keymap_handle = handle.clone();
        let save_handle = handle.clone();
        let close_handle = handle.clone();
        let path = match editor.page {
            SettingsPage::Configuration => self.launch_config.config_path.display().to_string(),
            SettingsPage::Themes => format!(
                "Zed theme extensions · installed in {}",
                config::themes_dir().display()
            ),
            SettingsPage::Keymap => self.launch_config.keymap_path.display().to_string(),
        };
        Some(
            div()
                .id("settings-backdrop")
                .absolute()
                .inset_0()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.3))
                .occlude()
                .child(
                    div()
                        .id("settings-editor")
                        .track_focus(&self.settings_focus)
                        .relative()
                        .size_full()
                        .max_w(px(980.))
                        .max_h(px(680.))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .h_12()
                                .px_3()
                                .flex_none()
                                .justify_between()
                                .border_b_1()
                                .border_color(colors.border)
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .id("settings-configuration-tab")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .when(
                                                    editor.page == SettingsPage::Configuration,
                                                    |tab| tab.bg(colors.element_selected),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    config_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_settings_page(
                                                                SettingsPage::Configuration,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                                .child("Configuration"),
                                        )
                                        .child(
                                            div()
                                                .id("settings-themes-tab")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .when(editor.page == SettingsPage::Themes, |tab| {
                                                    tab.bg(colors.element_selected)
                                                })
                                                .on_click(move |_, window, cx| {
                                                    themes_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_settings_page(
                                                                SettingsPage::Themes,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                                .child("Themes"),
                                        )
                                        .child(
                                            div()
                                                .id("settings-keymap-tab")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .when(editor.page == SettingsPage::Keymap, |tab| {
                                                    tab.bg(colors.element_selected)
                                                })
                                                .on_click(move |_, window, cx| {
                                                    keymap_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_settings_page(
                                                                SettingsPage::Keymap,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                                .child("Keymap"),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("save-settings")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .bg(colors.element_selected)
                                                .hover(|style| style.bg(colors.element_hover))
                                                .on_click(move |_, window, cx| {
                                                    save_handle
                                                        .update(cx, |this, cx| {
                                                            this.save_settings(window, cx)
                                                        })
                                                        .ok();
                                                })
                                                .child(
                                                    if editor.configuration_dirty
                                                        || editor.keymap_dirty
                                                    {
                                                        "Save *"
                                                    } else {
                                                        "Save"
                                                    },
                                                ),
                                        )
                                        .child(
                                            IconButton::new("close-settings", IconName::Close)
                                                .icon_size(IconSize::Small)
                                                .tooltip(Tooltip::text("Close settings"))
                                                .on_click(move |_, window, cx| {
                                                    close_handle
                                                        .update(cx, |this, cx| {
                                                            this.dismiss_settings(window, cx)
                                                        })
                                                        .ok();
                                                }),
                                        ),
                                ),
                        )
                        .child(
                            h_flex()
                                .h_9()
                                .px_3()
                                .flex_none()
                                .border_b_1()
                                .border_color(colors.border)
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(path),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .id("settings-form-scroll")
                                        .size_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&editor.settings_scroll)
                                        .px_5()
                                        .py_3()
                                        .text_color(colors.text)
                                        .child(content),
                                )
                                .child(scroll_indicator(
                                    "settings-form-scrollbar".to_owned(),
                                    &editor.settings_scroll,
                                )),
                        )
                        .when_some(editor.message.clone(), |dialog, (error, message)| {
                            dialog.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .border_t_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .text_color(if error {
                                        colors.text
                                    } else {
                                        colors.text_muted
                                    })
                                    .child(message),
                            )
                        })
                        .when_some(font_modal, |dialog, modal| dialog.child(modal))
                        .when_some(profile_modal, |dialog, modal| dialog.child(modal)),
                )
                .into_any_element(),
        )
    }

    fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if self.command_palette.is_some() {
            self.dismiss_command_palette(window, cx);
            return;
        }

        let terminal_focus = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
            .map(|view| view.focus_handle(cx));
        let commands = window
            .available_actions(cx)
            .into_iter()
            .filter(|action| action.name() != ToggleCommandPalette.name())
            .map(|action| {
                let shortcut = terminal_focus
                    .as_ref()
                    .and_then(|focus| {
                        window.highest_precedence_binding_for_action_in(action.as_ref(), focus)
                    })
                    .map(|binding| {
                        binding
                            .keystrokes()
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" ")
                    });
                PaletteCommand {
                    name: humanize_action_name(action.name()),
                    shortcut,
                    action,
                }
            })
            .collect();
        self.command_palette = Some(CommandPalette::new(commands));
        self.command_palette_focus.focus(window, cx);
        cx.notify();
    }

    fn dismiss_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_palette = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn run_palette_command(
        &mut self,
        command_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = self
            .command_palette
            .as_ref()
            .and_then(|palette| palette.commands.get(command_index))
            .map(|command| command.action.boxed_clone());
        self.command_palette = None;
        self.focus_active(window, cx);
        if let Some(action) = action {
            window.dispatch_action(action, cx);
        }
        cx.notify();
    }

    fn command_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_editor.is_some() {
            self.settings_key_down(event, window, cx);
            return;
        }
        if self.tab_search.is_some() {
            self.tab_search_key_down(event, window, cx);
            return;
        }
        let Some(palette) = self.command_palette.as_mut() else {
            self.rename_key_down(event, window, cx);
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_command_palette(window, cx),
            "up" => {
                palette.selected = palette.selected.saturating_sub(1);
                cx.notify();
            }
            "down" => {
                palette.selected =
                    (palette.selected + 1).min(palette.matches().len().saturating_sub(1));
                cx.notify();
            }
            "enter" => {
                let command = palette.matches().get(palette.selected).copied();
                if let Some(command) = command {
                    self.run_palette_command(command, window, cx);
                }
            }
            "backspace" => {
                if palette.select_all {
                    palette.query.clear();
                    palette.cursor = 0;
                    palette.refresh_matches();
                } else if palette.cursor > 0 {
                    let previous = previous_char_boundary(&palette.query, palette.cursor);
                    palette.query.replace_range(previous..palette.cursor, "");
                    palette.cursor = previous;
                    palette.refresh_matches();
                }
                palette.select_all = false;
                palette.selected = 0;
                cx.notify();
            }
            "delete" => {
                if palette.select_all {
                    palette.query.clear();
                    palette.cursor = 0;
                    palette.refresh_matches();
                } else if palette.cursor < palette.query.len() {
                    let next = next_char_boundary(&palette.query, palette.cursor);
                    palette.query.replace_range(palette.cursor..next, "");
                    palette.refresh_matches();
                }
                palette.select_all = false;
                palette.selected = 0;
                cx.notify();
            }
            "left" => {
                palette.cursor = if palette.select_all {
                    0
                } else {
                    previous_char_boundary(&palette.query, palette.cursor)
                };
                palette.select_all = false;
                cx.notify();
            }
            "right" => {
                palette.cursor = if palette.select_all {
                    palette.query.len()
                } else {
                    next_char_boundary(&palette.query, palette.cursor)
                };
                palette.select_all = false;
                cx.notify();
            }
            "home" => {
                palette.cursor = 0;
                palette.select_all = false;
                cx.notify();
            }
            "end" => {
                palette.cursor = palette.query.len();
                palette.select_all = false;
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                palette.select_all = !palette.query.is_empty();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if palette.select_all {
                        palette.query.clear();
                        palette.cursor = 0;
                        palette.select_all = false;
                    }
                    palette.query.insert_str(palette.cursor, text);
                    palette.cursor += text.len();
                    palette.refresh_matches();
                    palette.selected = 0;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn rename_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(buffer) = tab.rename_buffer.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "enter" => {
                let title = buffer.trim().to_string();
                tab.custom_title = (!title.is_empty()).then_some(title);
                tab.rename_buffer = None;
                tab.rename_select_all = false;
                self.focus_active(window, cx);
            }
            "escape" => {
                tab.rename_buffer = None;
                tab.rename_select_all = false;
                self.focus_active(window, cx);
            }
            "backspace" => {
                if tab.rename_select_all {
                    buffer.clear();
                    tab.rename_cursor = 0;
                    tab.rename_select_all = false;
                } else if tab.rename_cursor > 0 {
                    let previous = previous_char_boundary(buffer, tab.rename_cursor);
                    buffer.replace_range(previous..tab.rename_cursor, "");
                    tab.rename_cursor = previous;
                }
                cx.notify();
            }
            "delete" => {
                if tab.rename_select_all {
                    buffer.clear();
                    tab.rename_cursor = 0;
                    tab.rename_select_all = false;
                } else if tab.rename_cursor < buffer.len() {
                    let next = next_char_boundary(buffer, tab.rename_cursor);
                    buffer.replace_range(tab.rename_cursor..next, "");
                }
                cx.notify();
            }
            "left" => {
                tab.rename_cursor = if tab.rename_select_all {
                    0
                } else {
                    previous_char_boundary(buffer, tab.rename_cursor)
                };
                tab.rename_select_all = false;
                cx.notify();
            }
            "right" => {
                tab.rename_cursor = if tab.rename_select_all {
                    buffer.len()
                } else {
                    next_char_boundary(buffer, tab.rename_cursor)
                };
                tab.rename_select_all = false;
                cx.notify();
            }
            "home" => {
                tab.rename_cursor = 0;
                tab.rename_select_all = false;
                cx.notify();
            }
            "end" => {
                tab.rename_cursor = buffer.len();
                tab.rename_select_all = false;
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                tab.rename_select_all = !buffer.is_empty();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if tab.rename_select_all {
                        buffer.clear();
                        tab.rename_cursor = 0;
                        tab.rename_select_all = false;
                    }
                    buffer.insert_str(tab.rename_cursor, text);
                    tab.rename_cursor += text.len();
                    cx.notify();
                }
            }
            _ => {}
        }
        cx.stop_propagation();
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn is_renaming_tab(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.rename_buffer.is_some())
    }

    fn render_pane_layout(
        &self,
        tab: &Tab,
        layout: &PaneLayout,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
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
                .child(self.render_pane_layout(tab, first, window, cx))
                .child(self.render_pane_layout(tab, second, window, cx))
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

impl Render for Zetta {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let handle = cx.entity().downgrade();
        let broadcast_input = self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.broadcast_input);
        let supported_controls = window.window_controls();
        let is_maximized = window.is_maximized();
        let client_decorations = matches!(window.window_decorations(), Decorations::Client { .. });
        let left_window_controls = render_window_controls(
            self.button_layout.left,
            supported_controls,
            is_maximized,
            false,
            client_decorations,
            cx,
        );
        let right_window_controls = render_window_controls(
            self.button_layout.right,
            supported_controls,
            is_maximized,
            true,
            client_decorations,
            cx,
        );
        let title_bar_background = if cfg!(any(target_os = "linux", target_os = "freebsd"))
            && !window.is_window_active()
        {
            colors.title_bar_inactive_background
        } else {
            colors.title_bar_background
        };
        let title_bar = div()
            .id("zetta-title-bar")
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .h(platform_title_bar_height(window))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(title_bar_background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.titlebar_dragging = true;
                    this.focus_active(window, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.titlebar_dragging = false;
                    this.focus_active(window, cx);
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.titlebar_dragging = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.titlebar_dragging {
                    this.titlebar_dragging = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    if cfg!(target_os = "macos") {
                        window.titlebar_double_click();
                    } else {
                        window.zoom_window();
                    }
                }
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Label::new("Zetta").size(LabelSize::Small)),
            )
            .child(left_window_controls)
            .child(right_window_controls);
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let selected = index == self.active_tab;
                let tab_theme = tab.theme(cx);
                let tab_colors = tab_theme.colors();
                let tab_background = if selected {
                    tab_colors.tab_active_background
                } else {
                    tab_colors.tab_inactive_background
                };
                let tab_text = if selected {
                    tab_colors.text
                } else {
                    tab_colors.text_muted
                };
                let tab_icon = if selected {
                    tab_colors.icon
                } else {
                    tab_colors.icon_muted
                };
                let select_handle = handle.clone();
                let close_handle = handle.clone();
                let rename_view = tab.active_pane().and_then(|pane| pane.view.clone());
                let title = if let Some(buffer) = tab.rename_buffer.as_ref() {
                    if tab.rename_select_all {
                        buffer.clone().into()
                    } else {
                        let cursor = tab.rename_cursor.min(buffer.len());
                        let (before, after) = buffer.split_at(cursor);
                        format!("{before}|{after}").into()
                    }
                } else if let Some(custom_title) = tab.custom_title.as_ref() {
                    custom_title.clone().into()
                } else if let Some(view) = tab.active_pane().and_then(|pane| pane.view.as_ref()) {
                    view.read(cx).tab_content_text(0, cx)
                } else {
                    tab.active_pane()
                        .map(|pane| pane.profile.name.clone())
                        .unwrap_or_else(|| "Terminal".to_string())
                        .into()
                };
                let full_title = if let Some(buffer) = tab.rename_buffer.as_ref() {
                    buffer.clone().into()
                } else if let Some(custom_title) = tab.custom_title.as_ref() {
                    custom_title.clone().into()
                } else if let Some(view) = tab.active_pane().and_then(|pane| pane.view.as_ref()) {
                    view.read(cx).tab_content_text(1, cx)
                } else {
                    tab.active_pane()
                        .map(|pane| pane.profile.name.clone())
                        .unwrap_or_else(|| "Terminal".to_string())
                        .into()
                };
                let content = h_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        svg()
                            .path(IconName::Terminal.path())
                            .size(px(14.))
                            .flex_none()
                            .text_color(tab_icon),
                    )
                    .child(
                        div()
                            .id(("tab-title", tab.id as usize))
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .when(
                                tab.rename_buffer.is_some() && tab.rename_select_all,
                                |title| title.bg(tab_colors.element_selection_background),
                            )
                            .tooltip(Tooltip::text(full_title))
                            .text_color(tab_text)
                            .child(title),
                    )
                    .into_any_element();
                div()
                    .id(("tab", tab.id as usize))
                    .h_8()
                    .w(px(180.))
                    .min_w(px(80.))
                    .max_w(px(180.))
                    .flex_shrink_1()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_r_1()
                    .border_color(tab_colors.border)
                    .bg(tab_background)
                    .when(selected, |this| {
                        this.border_t_2().border_color(tab_colors.text_accent)
                    })
                    .on_click(move |event, window, cx| {
                        select_handle
                            .update(cx, |this, cx| {
                                this.active_tab = index;
                                if event.click_count() == 2
                                    && let Some(view) = rename_view.as_ref()
                                {
                                    this.begin_rename(view.clone(), window, cx);
                                } else {
                                    this.focus_active(window, cx);
                                }
                            })
                            .ok();
                    })
                    .child(div().min_w_0().flex_1().overflow_hidden().child(content))
                    .child(
                        div()
                            .id(("close-tab", tab.id as usize))
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(tab_colors.element_hover))
                            .aria_label("Close tab")
                            .tooltip(Tooltip::text("Close tab"))
                            .child(
                                svg()
                                    .path(IconName::Close.path())
                                    .size(px(12.))
                                    .text_color(tab_icon),
                            )
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                close_handle
                                    .update(cx, |this, cx| this.close_tab_at(index, window, cx))
                                    .ok();
                            }),
                    )
            })
            .collect::<Vec<_>>();

        let profile_menu_profiles = self.profiles.clone();
        let default_profile = self.launch_config.default_profile;
        let profile_menu_handle = handle.clone();
        let profile_menu = PopoverMenu::new("new-tab-profile-menu")
            .trigger_with_tooltip(
                IconButton::new("new-tab-profile-menu-trigger", IconName::ChevronDown)
                    .shape(IconButtonShape::Wide)
                    .size(ButtonSize::Large)
                    .width(px(32.))
                    .icon_size(IconSize::Small)
                    .aria_label("New tab profile"),
                Tooltip::text("New tab profile"),
            )
            .anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                let profiles = profile_menu_profiles.clone();
                let handle = profile_menu_handle.clone();
                Some(ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                    for (index, profile) in profiles.iter().enumerate() {
                        let is_default = index == default_profile;
                        let label = profile.name.clone();
                        let label_for_row = label.clone();
                        let shortcut = profile_shortcut_label(index + 1);
                        let handle = handle.clone();
                        menu = menu.custom_entry(
                            move |_, _| {
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .when(is_default, |row| {
                                                row.child(
                                                    Icon::new(IconName::Check)
                                                        .size(IconSize::Small)
                                                        .color(Color::Accent),
                                                )
                                            })
                                            .when(!is_default, |row| row.child(div().w_4()))
                                            .child(Label::new(label_for_row.clone()).color(
                                                if is_default {
                                                    Color::Accent
                                                } else {
                                                    Color::Default
                                                },
                                            )),
                                    )
                                    .when_some(shortcut.clone(), |row, shortcut| {
                                        row.child(
                                            Label::new(shortcut)
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                        )
                                    })
                                    .into_any_element()
                            },
                            move |window, cx| {
                                handle
                                    .update(cx, |this, cx| {
                                        this.selected_profile = index;
                                        this.open_tab(window, cx);
                                    })
                                    .ok();
                            },
                        );
                    }
                    menu
                }))
            });

        let body = match self.tabs.get(self.active_tab) {
            Some(tab) => self.render_pane_layout(tab, &tab.layout, window, cx),
            None => div().size_full().into_any_element(),
        };
        let performance_overlay = self.performance_overlay.as_ref().map(|overlay| {
            let metrics = overlay.metrics;
            let rows = [
                ("Draw FPS", format!("{:.1}", metrics.draw_fps)),
                (
                    "Frame avg / p95",
                    format!(
                        "{:.2} / {:.2} ms",
                        metrics.average_draw_ms, metrics.p95_draw_ms
                    ),
                ),
                (
                    "Invalidation avg",
                    format!("{:.2} ms", metrics.average_latency_ms),
                ),
                ("Frames > 8.3 ms", metrics.slow_120_hz.to_string()),
                ("Frames > 16.7 ms", metrics.slow_60_hz.to_string()),
                (
                    "Window",
                    if window.is_window_active() {
                        "Active".to_owned()
                    } else {
                        "Inactive".to_owned()
                    },
                ),
            ];
            div()
                .id("performance-overlay")
                .absolute()
                .top(px(74.))
                .right(px(10.))
                .w(px(232.))
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .rounded(px(4.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.elevated_surface_background.opacity(0.96))
                .shadow_sm()
                .text_sm()
                .text_color(colors.text)
                .child(
                    div()
                        .pb_1()
                        .border_b_1()
                        .border_color(colors.border)
                        .child("Performance"),
                )
                .children(rows.into_iter().map(|(label, value)| {
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_3()
                        .child(div().text_color(colors.text_muted).child(label))
                        .child(div().child(value))
                }))
                .into_any_element()
        });

        let tab_search_overlay = self.tab_search.as_ref().map(|search| {
            let cursor = search.cursor.min(search.query.len());
            let (before, after) = search.query.split_at(cursor);
            let before = before.to_owned();
            let after = after.to_owned();
            let selected = search.select_all;
            let status = search
                .active_match
                .map(|index| format!("{} / {}", index + 1, search.matches.len()))
                .unwrap_or_else(|| format!("0 / {}", search.matches.len()));

            div()
                .absolute()
                .top(px(74.0))
                .left_2()
                .right_2()
                .flex()
                .justify_end()
                .child(
                    div()
                        .id("tab-scrollback-search")
                        .track_focus(&self.tab_search_focus)
                        .w_full()
                        .max_w(px(460.0))
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background.alpha(1.0))
                        .shadow_sm()
                        .text_sm()
                        .text_color(colors.text)
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .when(selected, |input| {
                                            input.bg(colors.element_selection_background)
                                        })
                                        .child(div().whitespace_nowrap().child(before))
                                        .when(!selected, |input| {
                                            input.child(
                                                div()
                                                    .flex_none()
                                                    .w(px(1.0))
                                                    .h(px(16.0))
                                                    .bg(colors.text_accent),
                                            )
                                        })
                                        .child(div().whitespace_nowrap().child(after)),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(colors.text_muted)
                                        .child(status),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("All panes  Enter next  Shift+Enter previous  Esc close"),
                        ),
                )
                .into_any_element()
        });

        let palette_overlay = self.command_palette.as_ref().map(|palette| {
            let cursor = palette.cursor.min(palette.query.len());
            let (query_before, query_after) = palette.query.split_at(cursor);
            let query_before = query_before.to_owned();
            let query_after = query_after.to_owned();
            let query_empty = palette.query.is_empty();
            let query_selected = palette.select_all;
            let matches = palette.matches();
            let selected = palette.selected;
            let result_count = matches.len();
            let visible_start = selected.saturating_sub(9);
            let rows = matches
                .iter()
                .copied()
                .skip(visible_start)
                .take(10)
                .enumerate()
                .map(|(position, command_index)| {
                    let command = &palette.commands[command_index];
                    let row_handle = handle.clone();
                    div()
                        .id(("command-palette-row", command_index))
                        .h_9()
                        .w_full()
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(colors.text)
                        .when(visible_start + position == selected, |row| {
                            row.bg(colors.element_selected)
                        })
                        .hover(|style| style.bg(colors.element_hover))
                        .on_click(move |_, window, cx| {
                            row_handle
                                .update(cx, |this, cx| {
                                    this.run_palette_command(command_index, window, cx)
                                })
                                .ok();
                        })
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(command.name.clone()),
                        )
                        .when_some(command.shortcut.clone(), |row, shortcut| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(colors.text_muted)
                                    .child(shortcut),
                            )
                        })
                })
                .collect::<Vec<_>>();
            let dismiss_handle = handle.clone();

            div()
                .id("command-palette-backdrop")
                .absolute()
                .inset_0()
                .pt(px(72.))
                .px_4()
                .flex()
                .items_start()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    dismiss_handle
                        .update(cx, |this, cx| this.dismiss_command_palette(window, cx))
                        .ok();
                })
                .child(
                    div()
                        .id("command-palette")
                        .track_focus(&self.command_palette_focus)
                        .w_full()
                        .max_w(px(680.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h_12()
                                .px_3()
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(colors.border)
                                .text_color(colors.text)
                                .child(div().text_color(colors.text_accent).mr_2().child(">"))
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(query_selected, |input| {
                                            input.bg(colors.element_selection_background)
                                        })
                                        .child(div().whitespace_nowrap().child(query_before))
                                        .when(!query_selected, |input| {
                                            input.child(
                                                div()
                                                    .flex_none()
                                                    .w(px(1.0))
                                                    .h(px(16.0))
                                                    .bg(colors.text_accent),
                                            )
                                        })
                                        .child(div().whitespace_nowrap().child(query_after))
                                        .when(query_empty, |input| {
                                            input.child(
                                                div()
                                                    .text_color(colors.text_placeholder)
                                                    .child("Type a command"),
                                            )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .py_1()
                                .when(result_count == 0, |list| {
                                    list.child(
                                        div()
                                            .h_12()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .text_sm()
                                            .text_color(colors.text_muted)
                                            .child("No matching commands"),
                                    )
                                })
                                .children(rows),
                        )
                        .child(
                            div()
                                .h_7()
                                .px_3()
                                .flex()
                                .items_center()
                                .border_t_1()
                                .border_color(colors.border)
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(format!(
                                    "{result_count} command{}",
                                    if result_count == 1 { "" } else { "s" }
                                )),
                        ),
                )
                .into_any_element()
        });

        let settings_overlay = self.render_settings_overlay(window, cx);

        let content = div()
            .key_context("Zetta")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(colors.editor_background)
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::new_window))
            .on_action(cx.listener(Self::open_profile))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::split_horizontal))
            .on_action(cx.listener(Self::split_vertical))
            .on_action(cx.listener(Self::focus_pane_left))
            .on_action(cx.listener(Self::focus_pane_right))
            .on_action(cx.listener(Self::focus_pane_up))
            .on_action(cx.listener(Self::focus_pane_down))
            .on_action(cx.listener(Self::toggle_broadcast_input))
            .on_action(cx.listener(Self::increase_terminal_font_size))
            .on_action(cx.listener(Self::decrease_terminal_font_size))
            .on_action(cx.listener(Self::reset_terminal_font_size))
            .on_action(cx.listener(Self::increase_pane_font_size))
            .on_action(cx.listener(Self::decrease_pane_font_size))
            .on_action(cx.listener(Self::reset_pane_font_size))
            .on_action(cx.listener(Self::search_tab_scrollback))
            .on_action(cx.listener(Self::reload_configuration))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::toggle_performance_overlay))
            .when(self.is_renaming_tab(), |content| {
                content.track_focus(&self.rename_focus)
            })
            .on_key_down(cx.listener(Self::command_palette_key_down))
            .child(title_bar)
            .child(
                div()
                    .h_8()
                    .flex_none()
                    .flex()
                    .items_center()
                    .bg(colors.tab_bar_background)
                    .border_t_1()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .id("tabs-scroll")
                            .h_full()
                            .min_w_0()
                            .flex_shrink_1()
                            .flex()
                            .items_center()
                            .overflow_x_scroll()
                            .children(tabs),
                    )
                    .child(
                        div()
                            .ml_1()
                            .mr_2()
                            .h_8()
                            .flex_none()
                            .flex()
                            .items_center()
                            .child(
                                IconButton::new("new-tab", IconName::Plus)
                                    .shape(IconButtonShape::Wide)
                                    .size(ButtonSize::Large)
                                    .width(px(32.))
                                    .icon_size(IconSize::Small)
                                    .aria_label("New tab")
                                    .tooltip(Tooltip::text("New tab"))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(NewTab), cx)
                                    }),
                            )
                            .child(profile_menu),
                    )
                    .child(
                        IconButton::new("toggle-broadcast-input", IconName::Keyboard)
                            .shape(IconButtonShape::Wide)
                            .size(ButtonSize::Large)
                            .width(px(32.))
                            .icon_size(IconSize::Small)
                            .toggle_state(broadcast_input)
                            .aria_label("Broadcast input to all panes")
                            .tooltip(Tooltip::text(if broadcast_input {
                                "Broadcast input is on (Ctrl-Shift-I)"
                            } else {
                                "Broadcast input to all panes (Ctrl-Shift-I)"
                            }))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleBroadcastInput), cx)
                            }),
                    )
                    .child(
                        IconButton::new("open-settings", IconName::Settings)
                            .shape(IconButtonShape::Wide)
                            .size(ButtonSize::Large)
                            .width(px(32.))
                            .icon_size(IconSize::Small)
                            .aria_label("Open settings")
                            .tooltip(Tooltip::text("Open settings (Ctrl+,)"))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleSettings), cx)
                            }),
                    )
                    .child(div().min_w_0().flex_1()),
            )
            .when_some(self.configuration_error.clone(), |content, error| {
                content.child(
                    div().px_2().py_1().child(
                        Banner::new()
                            .severity(Severity::Error)
                            .child(Label::new(error).size(LabelSize::Small).line_clamp(3))
                            .action_slot(
                                IconButton::new("reload-invalid-configuration", IconName::RotateCw)
                                    .shape(IconButtonShape::Square)
                                    .icon_size(IconSize::Small)
                                    .aria_label("Reload configuration")
                                    .tooltip(Tooltip::text("Reload configuration"))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(ReloadConfiguration), cx)
                                    }),
                            ),
                    ),
                )
            })
            .child(div().flex_1().min_h_0().child(body))
            .when_some(performance_overlay, |content, overlay| {
                content.child(overlay)
            })
            .when_some(palette_overlay, |content, overlay| content.child(overlay))
            .when_some(tab_search_overlay, |content, overlay| {
                content.child(overlay)
            });
        let content =
            content.when_some(settings_overlay, |content, overlay| content.child(overlay));

        client_window_frame(content, window, cx)
    }
}

const RESIZE_HANDLE: Pixels = px(10.);

fn system_window_button_layout(cx: &App) -> WindowButtonLayout {
    #[cfg(target_os = "linux")]
    if let Some(layout) = read_gnome_button_layout() {
        return layout;
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        cx.button_layout()
            .unwrap_or_else(WindowButtonLayout::linux_default)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = cx;
        WindowButtonLayout {
            left: [None; MAX_BUTTONS_PER_SIDE],
            right: [None; MAX_BUTTONS_PER_SIDE],
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "macos")))]
    {
        let _ = cx;
        WindowButtonLayout {
            left: [None; MAX_BUTTONS_PER_SIDE],
            right: [
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
                Some(WindowButton::Close),
            ],
        }
    }
}

#[cfg(target_os = "linux")]
fn read_gnome_button_layout() -> Option<WindowButtonLayout> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.wm.preferences", "button-layout"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    parse_gsettings_button_layout(std::str::from_utf8(&output.stdout).ok()?)
}

#[cfg(target_os = "linux")]
fn parse_gsettings_button_layout(output: &str) -> Option<WindowButtonLayout> {
    let output = output.trim();
    let layout = output
        .strip_prefix('\'')
        .and_then(|output| output.strip_suffix('\''))
        .unwrap_or(output);
    WindowButtonLayout::parse(layout).ok()
}

fn platform_title_bar_height(window: &Window) -> Pixels {
    if cfg!(target_os = "windows") {
        px(32.)
    } else {
        (1.75 * window.rem_size()).max(px(34.))
    }
}

#[cfg(target_os = "windows")]
fn render_window_controls(
    buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    _supported_controls: WindowControls,
    is_maximized: bool,
    right_aligned: bool,
    _client_decorations: bool,
    cx: &App,
) -> AnyElement {
    if !right_aligned || buttons.iter().all(Option::is_none) {
        return div().into_any_element();
    }

    let colors = cx.theme().colors();
    let caption_button = |id, glyph, area, close: bool| {
        let hover_background = if close {
            gpui::rgba(0xe81120ff).into()
        } else {
            colors.ghost_element_hover
        };
        let active_background = if close {
            gpui::rgba(0xe81120cc).into()
        } else {
            colors.ghost_element_active
        };

        h_flex()
            .id(id)
            .h_full()
            .w(px(36.))
            .flex_none()
            .justify_center()
            .content_center()
            .occlude()
            .text_size(px(10.))
            .text_color(colors.text)
            .hover(move |style| {
                if close {
                    style.bg(hover_background).text_color(gpui::white())
                } else {
                    style.bg(hover_background)
                }
            })
            .active(move |style| {
                if close {
                    style
                        .bg(active_background)
                        .text_color(gpui::white().opacity(0.8))
                } else {
                    style.bg(active_background)
                }
            })
            .window_control_area(area)
            .child(glyph)
            .into_any_element()
    };

    h_flex()
        .id("windows-window-controls")
        .h_full()
        .ml_auto()
        .flex_none()
        .font_family("Segoe Fluent Icons")
        .child(caption_button(
            "minimize",
            "\u{e921}",
            WindowControlArea::Min,
            false,
        ))
        .child(caption_button(
            if is_maximized { "restore" } else { "maximize" },
            if is_maximized { "\u{e923}" } else { "\u{e922}" },
            WindowControlArea::Max,
            false,
        ))
        .child(caption_button(
            "close",
            "\u{e8bb}",
            WindowControlArea::Close,
            true,
        ))
        .into_any_element()
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn render_window_controls(
    buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    supported_controls: WindowControls,
    is_maximized: bool,
    right_aligned: bool,
    client_decorations: bool,
    cx: &App,
) -> AnyElement {
    if !client_decorations {
        return div().into_any_element();
    }

    let colors = cx.theme().colors();
    let controls = buttons.into_iter().flatten().filter_map(|button| {
        let (icon, area, enabled) = match button {
            WindowButton::Minimize => (
                IconName::GenericMinimize,
                WindowControlArea::Min,
                supported_controls.minimize,
            ),
            WindowButton::Maximize => (
                if is_maximized {
                    IconName::GenericRestore
                } else {
                    IconName::GenericMaximize
                },
                WindowControlArea::Max,
                supported_controls.maximize,
            ),
            WindowButton::Close => (IconName::GenericClose, WindowControlArea::Close, true),
        };
        enabled.then(|| {
            let action_button = button;
            h_flex()
                .id(button.id())
                .group("")
                .h_5()
                .w_5()
                .flex_none()
                .cursor_pointer()
                .justify_center()
                .content_center()
                .rounded_2xl()
                .hover(move |style| style.bg(colors.ghost_element_hover))
                .active(move |style| style.bg(colors.ghost_element_hover))
                .window_control_area(area)
                .child(
                    svg()
                        .size_4()
                        .flex_none()
                        .path(icon.path())
                        .text_color(colors.icon)
                        .group_hover("", move |style| style.text_color(colors.icon_muted)),
                )
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    match action_button {
                        WindowButton::Minimize => window.minimize_window(),
                        WindowButton::Maximize => window.zoom_window(),
                        WindowButton::Close => window.remove_window(),
                    }
                })
                .into_any_element()
        })
    });

    h_flex()
        .id(if right_aligned {
            "right-window-controls"
        } else {
            "left-window-controls"
        })
        .h_full()
        .flex_none()
        .gap_3()
        .px_3()
        .when(right_aligned, |controls| controls.ml_auto())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(controls)
        .into_any_element()
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
fn render_window_controls(
    _buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    _supported_controls: WindowControls,
    _is_maximized: bool,
    _right_aligned: bool,
    _client_decorations: bool,
    _cx: &App,
) -> AnyElement {
    div().into_any_element()
}

fn client_window_frame(
    content: impl IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let decorations = window.window_decorations();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    if matches!(decorations, Decorations::Client { .. }) {
        window.set_client_inset(RESIZE_HANDLE);
    }

    div()
        .id("window-frame")
        .size_full()
        .bg(transparent_black())
        .map(|frame| match decorations {
            Decorations::Server => frame,
            Decorations::Client { .. } => frame
                .rounded_client_corners(tiling)
                .when(!tiling.top, |frame| frame.pt(RESIZE_HANDLE))
                .when(!tiling.bottom, |frame| frame.pb(RESIZE_HANDLE))
                .when(!tiling.left, |frame| frame.pl(RESIZE_HANDLE))
                .when(!tiling.right, |frame| frame.pr(RESIZE_HANDLE))
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    let size = window.window_bounds().get_bounds().size;
                    if let Some(edge) = resize_edge(event.position, size, tiling) {
                        window.start_window_resize(edge);
                        cx.stop_propagation();
                    }
                }),
        })
        .child(
            div()
                .size_full()
                .overflow_hidden()
                .border_1()
                .border_color(cx.theme().colors().border)
                .rounded_client_corners(tiling)
                .child(content),
        )
        .when(matches!(decorations, Decorations::Client { .. }), |frame| {
            frame.child(
                canvas(
                    |_bounds, window, _| {
                        window.insert_hitbox(
                            Bounds::new(
                                point(px(0.), px(0.)),
                                window.window_bounds().get_bounds().size,
                            ),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_bounds, hitbox, window, _| {
                        let Some(edge) = resize_edge(
                            window.mouse_position(),
                            window.window_bounds().get_bounds().size,
                            tiling,
                        ) else {
                            return;
                        };
                        let cursor = match edge {
                            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                            ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                CursorStyle::ResizeUpLeftDownRight
                            }
                            ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                CursorStyle::ResizeUpRightDownLeft
                            }
                        };
                        window.set_cursor_style(cursor, &hitbox);
                    },
                )
                .absolute()
                .size_full(),
            )
        })
}

fn resize_edge(
    position: Point<Pixels>,
    window_size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let corner = RESIZE_HANDLE * 2.;
    let left = position.x < corner;
    let right = position.x > window_size.width - corner;
    let top = position.y < corner;
    let bottom = position.y > window_size.height - corner;

    if top && left && !tiling.top && !tiling.left {
        Some(ResizeEdge::TopLeft)
    } else if top && right && !tiling.top && !tiling.right {
        Some(ResizeEdge::TopRight)
    } else if bottom && left && !tiling.bottom && !tiling.left {
        Some(ResizeEdge::BottomLeft)
    } else if bottom && right && !tiling.bottom && !tiling.right {
        Some(ResizeEdge::BottomRight)
    } else if position.y < RESIZE_HANDLE && !tiling.top {
        Some(ResizeEdge::Top)
    } else if position.y > window_size.height - RESIZE_HANDLE && !tiling.bottom {
        Some(ResizeEdge::Bottom)
    } else if position.x < RESIZE_HANDLE && !tiling.left {
        Some(ResizeEdge::Left)
    } else if position.x > window_size.width - RESIZE_HANDLE && !tiling.right {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

fn parse_args() -> Result<(Option<PathBuf>, Option<PathBuf>)> {
    let mut config = None;
    let mut keymap = None;
    let mut args = env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--config" => config = Some(args.next().context("--config requires a path")?.into()),
            "--keymap" => keymap = Some(args.next().context("--keymap requires a path")?.into()),
            "--help" | "-h" => {
                println!("Zetta terminal\n\nUsage: zetta [--config PATH] [--keymap PATH]");
                std::process::exit(0);
            }
            unknown => anyhow::bail!("unknown argument {unknown:?}"),
        }
    }
    Ok((config, keymap))
}

fn load_startup_config(
    config_path: Option<&Path>,
    keymap_path: Option<PathBuf>,
) -> (Config, Option<String>) {
    match Config::load(config_path, keymap_path.clone()) {
        Ok(config) => (config, None),
        Err(error) => (
            Config::defaults(config_path, keymap_path),
            Some(format!("Could not load configuration: {error:#}")),
        ),
    }
}

fn profile_keybindings(slot: usize) -> Vec<KeyBinding> {
    const SHIFTED_DIGITS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];
    let action = OpenProfile { slot };
    vec![
        KeyBinding::new(
            &format!("ctrl-{}", SHIFTED_DIGITS[slot - 1]),
            action.clone(),
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            &format!("ctrl-alt-{slot}"),
            action,
            Some("Zetta > Terminal"),
        ),
    ]
}

fn profile_shortcut_label(slot: usize) -> Option<String> {
    (1..=9)
        .contains(&slot)
        .then(|| format!("Ctrl+Shift+{slot}"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ThemeFileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn changed_theme_files(
    themes_dir: &Path,
    cache: &mut HashMap<PathBuf, ThemeFileStamp>,
) -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    let mut present = std::collections::HashSet::new();
    for entry in fs::read_dir(themes_dir)
        .with_context(|| format!("reading theme directory {}", themes_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry.metadata()?;
        let stamp = ThemeFileStamp {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        };
        present.insert(path.clone());
        if cache.get(&path) != Some(&stamp) {
            cache.insert(path.clone(), stamp);
            changed.push(path);
        }
    }
    cache.retain(|path, _| present.contains(path));
    Ok(changed)
}

fn load_user_themes(cx: &mut App) -> Result<()> {
    static THEME_FILE_CACHE: OnceLock<Mutex<HashMap<PathBuf, ThemeFileStamp>>> = OnceLock::new();
    let themes_dir = config::themes_dir();
    fs::create_dir_all(&themes_dir)
        .with_context(|| format!("creating theme directory {}", themes_dir.display()))?;
    let registry = ThemeRegistry::global(cx);
    let paths = changed_theme_files(
        &themes_dir,
        &mut THEME_FILE_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )?;
    for path in paths {
        let bytes = fs::read(&path).with_context(|| format!("reading theme {}", path.display()))?;
        theme_settings::load_user_theme(&registry, &bytes)
            .with_context(|| format!("loading theme {}", path.display()))?;
    }
    Ok(())
}

fn with_zetta_theme_overrides(theme: Arc<Theme>) -> Arc<Theme> {
    let mut theme = theme.as_ref().clone();
    let colors = &mut theme.styles.colors;
    colors.scrollbar_thumb_background = colors.text_muted.opacity(0.7);
    colors.scrollbar_thumb_hover_background = colors.text.opacity(0.85);
    colors.scrollbar_thumb_active_background = colors.text_accent.opacity(0.95);
    Arc::new(theme)
}

fn apply_zetta_theme_overrides(cx: &mut App) {
    GlobalTheme::update_theme(cx, with_zetta_theme_overrides(cx.theme().clone()));
}

fn resolve_profile_theme(profile: &Profile, cx: &App) -> Result<Option<Arc<Theme>>> {
    profile
        .theme
        .as_deref()
        .map(|name| {
            ThemeRegistry::global(cx)
                .get(name)
                .map(with_zetta_theme_overrides)
                .with_context(|| format!("using theme {name:?} for profile {:?}", profile.name))
        })
        .transpose()
}

fn is_wsl_shell(shell: &Shell) -> bool {
    let program = match shell {
        Shell::System => return false,
        Shell::Program(program) | Shell::WithArguments { program, .. } => program,
    };
    program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("wsl.exe"))
}

fn launch_working_directory(
    profile: &Profile,
    inherited: Option<PathBuf>,
    inherited_wsl: Option<String>,
    fallback: Option<PathBuf>,
    fallback_is_configured: bool,
) -> (Option<PathBuf>, Option<String>) {
    // Windows process inspection sees the cwd of wsl.exe, not of its Linux shell.
    // Passing that value to a new WSL session leaks Zetta's own launch directory.
    let is_wsl = is_wsl_shell(&profile.command);
    let has_inherited_wsl = inherited_wsl.is_some();
    let working_directory = if is_wsl && has_inherited_wsl {
        None
    } else if is_wsl {
        fallback_is_configured.then_some(fallback).flatten()
    } else {
        inherited.or(fallback)
    };
    let wsl_directory = if is_wsl && has_inherited_wsl {
        inherited_wsl
    } else {
        (is_wsl && !fallback_is_configured).then(|| "~".to_owned())
    };
    (working_directory, wsl_directory)
}

fn wsl_cwd_tracking_file(profile: &Profile, pane_id: u64) -> Option<PathBuf> {
    (cfg!(windows) && is_wsl_shell(&profile.command)).then(|| {
        let path = env::temp_dir().join(format!("zetta-wsl-cwd-{}-{pane_id}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    })
}

const WSL_CWD_TRACKER: &str = r#"marker="$(wslpath -u "$1" 2>/dev/null || true)"
shell="${SHELL:-}"
if [ ! -x "$shell" ]; then
    shell="$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f7)"
fi
[ -x "$shell" ] || shell=/bin/sh

cwd_command='case "$PWD" in /*) printf "\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\" "$PWD" "$PWD";; esac'
case "${shell##*/}" in
    bash)
        PROMPT_COMMAND="${cwd_command}${PROMPT_COMMAND:+;${PROMPT_COMMAND}}"
        export PROMPT_COMMAND
        exec "$shell" -l
        ;;
    fish)
        exec "$shell" -l -C 'function __zetta_report_cwd --on-event fish_prompt; if string match -qr "^/" -- "$PWD"; printf "\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\" "$PWD" "$PWD"; end; end'
        ;;
    zsh)
        integration_zdotdir="$(mktemp -d "${TMPDIR:-/tmp}/zetta-zsh-XXXXXX" 2>/dev/null || true)"
        if [ -n "$integration_zdotdir" ]; then
            export ZETTA_ORIGINAL_ZDOTDIR="${ZDOTDIR:-$HOME}"
            export ZETTA_INTEGRATION_ZDOTDIR="$integration_zdotdir"
            cat > "$integration_zdotdir/.zshenv" <<'ZETTA_ZSHENV'
ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"

function __zetta_report_cwd() {
    [[ "$PWD" == /* ]] && printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD"
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_report_cwd
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR
ZETTA_ZSHENV
            ZDOTDIR="$integration_zdotdir"
            export ZDOTDIR
            exec "$shell" -l
        fi
        ;;
esac

# Shells without an injection mechanism retain the legacy tracker.
parent=$$
if [ -n "$marker" ]; then
    (
        previous=
        while kill -0 "$parent" 2>/dev/null; do
            cwd="$(readlink "/proc/$parent/cwd" 2>/dev/null)" || break
            if [ "$cwd" != "$previous" ]; then
                printf '%s\n' "$cwd" > "${marker}.tmp" && mv -f "${marker}.tmp" "$marker"
                previous="$cwd"
            fi
            sleep 0.1
        done
        rm -f "$marker" "${marker}.tmp"
    ) </dev/null >/dev/null 2>&1 &
fi
exec "$shell" -l"#;

fn wsl_shell_with_tracking(
    shell: Shell,
    directory: Option<&str>,
    cwd_file: Option<&Path>,
) -> Shell {
    match shell {
        Shell::Program(program) => {
            wsl_command_with_tracking(program, Vec::new(), None, directory, cwd_file)
        }
        Shell::WithArguments {
            program,
            args,
            title_override,
        } => wsl_command_with_tracking(program, args, title_override, directory, cwd_file),
        Shell::System => Shell::System,
    }
}

fn wsl_command_with_tracking(
    program: String,
    mut args: Vec<String>,
    title_override: Option<String>,
    directory: Option<&str>,
    cwd_file: Option<&Path>,
) -> Shell {
    let exec_index = args.iter().position(|arg| arg == "--exec" || arg == "-e");
    if let Some(directory) = directory
        && !args
            .iter()
            .take(exec_index.unwrap_or(args.len()))
            .any(|arg| arg == "--cd" || arg.starts_with("--cd="))
    {
        args.splice(
            exec_index.unwrap_or(args.len())..exec_index.unwrap_or(args.len()),
            ["--cd".to_owned(), directory.to_owned()],
        );
    }
    if exec_index.is_none()
        && let Some(cwd_file) = cwd_file
    {
        args.extend([
            "--exec".to_owned(),
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            WSL_CWD_TRACKER.to_owned(),
            "zetta-wsl-cwd".to_owned(),
            cwd_file.to_string_lossy().into_owned(),
        ]);
    }
    Shell::WithArguments {
        program,
        args,
        title_override,
    }
}

fn apply_config_settings(config: &Config, cx: &mut App) -> Result<()> {
    let theme_name = selected_theme_name(config.theme.as_deref());
    let theme = ThemeRegistry::global(cx)
        .get(theme_name)
        .with_context(|| format!("using Zed theme {theme_name:?}"))?;
    GlobalTheme::update_theme(cx, theme);
    apply_zetta_theme_overrides(cx);

    let mut terminal_settings = TerminalSettings::get_global(cx).clone();
    terminal_settings.font_family = Some(theme_settings::FontFamilyName(
        config.terminal_font_family.clone().into(),
    ));
    terminal_settings.font_size = config.terminal_font_size.map(px);
    terminal_settings.copy_on_select = true;
    terminal_settings.max_scroll_history_lines = Some(config.max_scroll_history_lines);
    TerminalSettings::override_global(terminal_settings, cx);
    Ok(())
}

fn selected_theme_name(configured_theme: Option<&str>) -> &str {
    configured_theme.unwrap_or(ZETTA_DEFAULT_THEME)
}

fn normalize_keymap_key_names(content: &str) -> String {
    content
        .replace("page-up", "pageup")
        .replace("page-down", "pagedown")
}

fn validate_keymap_contents(content: &str, cx: &mut App) -> Result<()> {
    let content = normalize_keymap_key_names(content);
    match KeymapFile::load(&content, cx) {
        KeymapFileLoadResult::Success { .. } => Ok(()),
        KeymapFileLoadResult::SomeFailedToLoad { error_message, .. } => {
            anyhow::bail!("some key bindings are invalid: {error_message}")
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            Err(error).context("parsing keymap JSON")
        }
    }
}

fn load_keybindings(path: &PathBuf, profile_count: usize, cx: &mut App) {
    cx.clear_key_bindings();
    match KeymapFile::load_asset_allow_partial_failure(settings::DEFAULT_KEYMAP_PATH, cx) {
        Ok(bindings) => cx.bind_keys(bindings),
        Err(error) => eprintln!("Could not load the default terminal keymap: {error:#}"),
    }
    let mut bindings = vec![
        KeyBinding::new("ctrl-shift-t", NewTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-n", NewWindow, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-o", SplitHorizontal, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-e", SplitVertical, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-a", SelectAll, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-backspace",
            ClearClipboard,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("alt-left", FocusPaneLeft, Some("Zetta > Terminal")),
        KeyBinding::new("alt-right", FocusPaneRight, Some("Zetta > Terminal")),
        KeyBinding::new("alt-up", FocusPaneUp, Some("Zetta > Terminal")),
        KeyBinding::new("alt-down", FocusPaneDown, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-i",
            ToggleBroadcastInput,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-tab", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-pageup", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-pagedown", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-c",
            CopyAndClearSelection,
            Some("Zetta > Terminal && selection"),
        ),
        KeyBinding::new("ctrl-v", Paste, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-f", SearchScrollback, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt-f", SearchTabScrollback, Some("Zetta > Terminal")),
        KeyBinding::new(
            "enter",
            SearchNextMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "shift-enter",
            SearchPreviousMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "f3",
            SearchNextMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "shift-f3",
            SearchPreviousMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "escape",
            DismissSearch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "ctrl-a",
            SelectAllSearchText,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new("ctrl-alt-v", PasteTrimmed, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-p",
            ToggleCommandPalette,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-,", ToggleSettings, Some("Zetta > Terminal")),
        KeyBinding::new("f2", RenameTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-=", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-+", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl--", DecreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-0", ResetTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt-=", IncreasePaneFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt-+", IncreasePaneFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt--", DecreasePaneFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt-0", ResetPaneFontSize, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-r",
            ReloadConfiguration,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "ctrl-shift-f12",
            TogglePerformanceOverlay,
            Some("Zetta > Terminal"),
        ),
        // Override Zed's inherited `pane::CloseActiveItem` binding in terminal focus.
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Terminal")),
    ];
    bindings.extend((1..=profile_count.min(9)).flat_map(profile_keybindings));
    cx.bind_keys(bindings);
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let content = normalize_keymap_key_names(&content);
    match KeymapFile::load(&content, cx) {
        KeymapFileLoadResult::Success { key_bindings } => cx.bind_keys(key_bindings),
        KeymapFileLoadResult::SomeFailedToLoad {
            key_bindings,
            error_message,
        } => {
            eprintln!(
                "Some key bindings in {} were ignored: {error_message}",
                path.display()
            );
            cx.bind_keys(key_bindings);
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            eprintln!("Could not load {}: {error:#}", path.display());
        }
    }
}

fn open_zetta_window(
    config: Config,
    configuration_error: Option<String>,
    cx: &mut App,
) -> Result<()> {
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(520.), px(320.))),
            app_id: Some(ZETTA_APP_ID.to_owned()),
            titlebar: Some(TitlebarOptions {
                title: Some("Zetta".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(9.), px(9.))),
            }),
            app_owns_titlebar_drag: true,
            window_background: WindowBackgroundAppearance::Transparent,
            window_decorations: Some(WindowDecorations::Client),
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title("Zetta");
            cx.new(|cx| Zetta::new(config, configuration_error, window, cx))
        },
    )
    .context("opening Zetta window")?;
    cx.activate(true);
    Ok(())
}

fn run() -> Result<()> {
    let (config_path, keymap_path) = parse_args()?;
    let (config, configuration_error) = load_startup_config(config_path.as_deref(), keymap_path);
    let keymap_path = config.keymap_path.clone();
    let profile_count = config.profiles.len();
    let http_client = Arc::new(
        reqwest_client::ReqwestClient::user_agent(concat!("Zetta/", env!("CARGO_PKG_VERSION")))
            .context("initializing HTTP client")?,
    );
    gpui_platform::application()
        .with_assets(ZettaAssets)
        .run(move |cx: &mut App| {
            cx.set_http_client(http_client);
            menu::init();
            zed_actions::init();
            release_channel::init(semver::Version::new(0, 1, 0), cx);
            settings::init(cx);
            theme_settings::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
            load_user_themes(cx).log_err();
            ZettaAssets.load_fonts(cx).log_err();
            apply_config_settings(&config, cx).expect("failed to apply Zetta configuration");
            load_keybindings(&keymap_path, profile_count, cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            open_zetta_window(config, configuration_error, cx)
                .expect("failed to open Zetta window");
        });
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Zetta failed to start: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_user_themes_are_not_reloaded() {
        let themes_dir = env::temp_dir().join(format!(
            "zetta-theme-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&themes_dir).unwrap();
        let theme_path = themes_dir.join("test.json");
        fs::write(&theme_path, "one").unwrap();
        let mut cache = HashMap::new();

        assert_eq!(
            changed_theme_files(&themes_dir, &mut cache).unwrap(),
            [theme_path.clone()]
        );
        assert!(
            changed_theme_files(&themes_dir, &mut cache)
                .unwrap()
                .is_empty()
        );

        fs::write(&theme_path, "a longer theme").unwrap();
        assert_eq!(
            changed_theme_files(&themes_dir, &mut cache).unwrap(),
            [theme_path]
        );
        fs::remove_dir_all(themes_dir).unwrap();
    }

    #[test]
    fn font_filter_uses_pre_normalized_names_and_preserves_indices() {
        let fonts = vec![
            "jetbrains mono".to_owned(),
            "fira code".to_owned(),
            "fira mono".to_owned(),
        ];
        assert_eq!(&*matching_font_indices(&fonts, "FIRA"), &[1, 2]);
        assert_eq!(&*matching_font_indices(&fonts, "code"), &[1]);
    }

    #[test]
    fn stale_tab_search_work_is_rejected() {
        let search = TabSearch {
            tab_id: 7,
            query: "cargo".into(),
            cursor: 0,
            select_all: false,
            generation: 4,
            matches: Vec::new(),
            active_match: None,
            task: None,
        };
        assert!(tab_search_request_is_current(Some(&search), 7, 4, "cargo"));
        assert!(!tab_search_request_is_current(Some(&search), 7, 3, "cargo"));
        assert!(!tab_search_request_is_current(Some(&search), 7, 4, "rust"));
    }

    #[test]
    fn settings_options_are_shared_without_copying_the_collection() {
        let options: Arc<[String]> = vec!["One".into(), "Two".into()].into();
        let menu_options = options.clone();
        assert!(Arc::ptr_eq(&options, &menu_options));
        assert_eq!(&*menu_options, &["One", "Two"]);
    }

    #[test]
    fn performance_metrics_report_fps_percentiles_and_slow_frames() {
        let draw_start = Instant::now();
        let timing = |milliseconds| FrameTiming {
            window_id: WindowId::from(1),
            dirty_at: Some(draw_start),
            invalidations: 1,
            draw_start,
            draw_end: draw_start + Duration::from_millis(milliseconds),
        };
        let metrics = PerformanceMetrics::from_timings(
            &[timing(5), timing(10), timing(20)],
            Duration::from_secs(1),
        );

        assert!((metrics.draw_fps - 3.0).abs() < f64::EPSILON);
        assert!((metrics.average_draw_ms - 11.666_666).abs() < 0.001);
        assert!((metrics.p95_draw_ms - 20.0).abs() < f64::EPSILON);
        assert!((metrics.average_latency_ms - 11.666_666).abs() < 0.001);
        assert_eq!(metrics.slow_120_hz, 2);
        assert_eq!(metrics.slow_60_hz, 1);
    }

    #[test]
    fn profile_draft_tab_navigation_moves_forward_and_backward() {
        assert_eq!(
            adjacent_profile_draft_field(Some(ProfileDraftField::Name), false),
            ProfileDraftField::Program
        );
        assert_eq!(
            adjacent_profile_draft_field(Some(ProfileDraftField::Program), false),
            ProfileDraftField::Arguments
        );
        assert_eq!(
            adjacent_profile_draft_field(Some(ProfileDraftField::Arguments), true),
            ProfileDraftField::Program
        );
        assert_eq!(
            adjacent_profile_draft_field(Some(ProfileDraftField::Name), true),
            ProfileDraftField::Arguments
        );
    }

    #[test]
    fn scroll_history_steps_cover_the_full_range_without_jumping_to_max() {
        let maximum = i32::MAX as u64;
        assert_eq!(adjusted_scroll_history(100_000, 1, maximum), 200_000);
        assert_eq!(adjusted_scroll_history(100_000, -1, maximum), 99_000);
        assert_eq!(
            adjusted_scroll_history(maximum, -1, maximum),
            maximum - 100_000_000
        );
        assert_eq!(adjusted_scroll_history(maximum - 1, 1, maximum), maximum);
    }

    #[test]
    fn performance_metrics_handle_an_idle_sample() {
        assert_eq!(
            PerformanceMetrics::from_timings(&[], Duration::from_secs(1)),
            PerformanceMetrics::default()
        );
    }

    #[test]
    fn invalid_startup_config_falls_back_and_reports_the_error() {
        let config_path = env::temp_dir().join(format!(
            "zetta-invalid-config-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&config_path, r#"{"theme": "One Light",}"#).unwrap();

        let (config, error) = load_startup_config(Some(&config_path), None);

        fs::remove_file(&config_path).unwrap();
        assert_eq!(config.config_path, config_path);
        assert_eq!(config.default_profile, 0);
        let error = error.expect("invalid JSON should be reported");
        assert!(error.contains("Could not load configuration"));
        assert!(error.contains("parsing"));
        assert!(error.contains("line 1 column"));
    }

    #[test]
    fn terminal_environment_identifies_zetta() {
        let mut env = HashMap::from([("ZED_TERM".to_string(), "true".to_string())]);

        terminal::insert_zetta_terminal_env(&mut env, &"0.1.0");

        assert_eq!(env.get("ZETTA_TERM").map(String::as_str), Some("true"));
        assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("zetta"));
        assert_eq!(
            env.get("TERM_PROGRAM_VERSION").map(String::as_str),
            Some("0.1.0")
        );
        assert!(!env.contains_key("ZED_TERM"));
    }

    #[test]
    fn defaults_to_light_theme_without_overriding_configuration() {
        assert_eq!(selected_theme_name(None), "One Light");
        assert_eq!(selected_theme_name(Some("One Dark")), "One Dark");
    }

    #[test]
    fn linux_desktop_entry_matches_app_id() {
        let desktop_entry = include_str!("../resources/linux/Zetta.desktop");
        assert!(desktop_entry.contains(&format!("\nIcon={ZETTA_APP_ID}\n")));
        assert!(desktop_entry.contains(&format!("\nStartupWMClass={ZETTA_APP_ID}\n")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_quoted_gsettings_button_layout() {
        let layout = parse_gsettings_button_layout("'close,minimize,maximize:'\n").unwrap();
        assert_eq!(
            layout.left,
            [
                Some(WindowButton::Close),
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
            ]
        );
        assert_eq!(layout.right, [None; MAX_BUTTONS_PER_SIDE]);
    }

    #[test]
    fn profile_shortcuts_match_shifted_and_fallback_chords() {
        const SHIFTED_DIGITS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];
        for (index, symbol) in SHIFTED_DIGITS.into_iter().enumerate() {
            let slot = index + 1;
            let bindings = profile_keybindings(slot);
            let shifted = gpui::Keystroke::parse(&format!("ctrl-{symbol}")).unwrap();
            let fallback = gpui::Keystroke::parse(&format!("ctrl-alt-{slot}")).unwrap();
            assert_eq!(bindings[0].match_keystrokes(&[shifted]), Some(false));
            assert_eq!(bindings[1].match_keystrokes(&[fallback]), Some(false));
        }
    }

    #[test]
    fn profile_shortcut_labels_cover_the_number_row() {
        assert_eq!(profile_shortcut_label(1).as_deref(), Some("Ctrl+Shift+1"));
        assert_eq!(profile_shortcut_label(9).as_deref(), Some("Ctrl+Shift+9"));
        assert_eq!(profile_shortcut_label(10), None);
    }

    #[test]
    fn wsl_home_is_applied_to_detected_wsl_commands() {
        let shell = Shell::WithArguments {
            program: "C:\\Windows\\System32\\wsl.exe".to_owned(),
            args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
            title_override: Some("WSL: Ubuntu".to_owned()),
        };

        assert!(is_wsl_shell(&shell));
        assert!(matches!(
            wsl_shell_with_tracking(shell, Some("~"), None),
            Shell::WithArguments { args, title_override, .. }
                if args == ["--distribution", "Ubuntu", "--cd", "~"]
                    && title_override.as_deref() == Some("WSL: Ubuntu")
        ));
    }

    #[test]
    fn native_shells_are_not_treated_as_wsl() {
        assert!(!is_wsl_shell(&Shell::Program("pwsh.exe".to_owned())));
    }

    #[test]
    fn explicit_wsl_directory_is_not_overridden() {
        let shell = Shell::WithArguments {
            program: "wsl.exe".to_owned(),
            args: vec!["--cd".to_owned(), "/work".to_owned()],
            title_override: None,
        };

        assert!(matches!(
            wsl_shell_with_tracking(shell, Some("~"), None),
            Shell::WithArguments { args, .. } if args == ["--cd", "/work"]
        ));
    }

    #[test]
    fn wsl_ignores_the_windows_side_inherited_directory() {
        let profile = Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::WithArguments {
                program: "wsl.exe".to_owned(),
                args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
                title_override: None,
            },
            theme: None,
        };

        let (directory, wsl_directory) = launch_working_directory(
            &profile,
            Some(PathBuf::from(r"C:\source\zetta")),
            None,
            Some(PathBuf::from(r"C:\Users\stefan")),
            false,
        );

        assert_eq!(directory, None);
        assert_eq!(wsl_directory.as_deref(), Some("~"));
    }

    #[test]
    fn native_profiles_still_inherit_the_active_directory() {
        let profile = Profile {
            name: "PowerShell".to_owned(),
            command: Shell::Program("pwsh.exe".to_owned()),
            theme: None,
        };
        let inherited = PathBuf::from(r"C:\source\zetta");

        let (directory, wsl_directory) = launch_working_directory(
            &profile,
            Some(inherited.clone()),
            None,
            Some(PathBuf::from(r"C:\Users\stefan")),
            false,
        );

        assert_eq!(directory, Some(inherited));
        assert_eq!(wsl_directory, None);
    }

    #[test]
    fn configured_directory_overrides_the_windows_side_wsl_directory() {
        let profile = Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::Program("wsl.exe".to_owned()),
            theme: None,
        };
        let configured = PathBuf::from(r"C:\Users\stefan");

        let (directory, wsl_directory) = launch_working_directory(
            &profile,
            Some(PathBuf::from(r"C:\source\zetta")),
            None,
            Some(configured.clone()),
            true,
        );

        assert_eq!(directory, Some(configured));
        assert_eq!(wsl_directory, None);
    }

    #[test]
    fn tracked_wsl_directory_takes_precedence_over_the_initial_configuration() {
        let profile = Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::Program("wsl.exe".to_owned()),
            theme: None,
        };

        let (directory, wsl_directory) = launch_working_directory(
            &profile,
            None,
            Some("/work".to_owned()),
            Some(PathBuf::from(r"C:\Users\stefan")),
            true,
        );

        assert_eq!(directory, None);
        assert_eq!(wsl_directory.as_deref(), Some("/work"));
    }

    #[test]
    fn wsl_inherits_the_tracked_linux_directory() {
        let profile = Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::Program("wsl.exe".to_owned()),
            theme: None,
        };

        let (directory, wsl_directory) = launch_working_directory(
            &profile,
            Some(PathBuf::from(r"C:\source\zetta")),
            Some("/home/stefan/source/zetta".to_owned()),
            Some(PathBuf::from(r"C:\Users\stefan")),
            false,
        );

        assert_eq!(directory, None);
        assert_eq!(wsl_directory.as_deref(), Some("/home/stefan/source/zetta"));
    }

    #[test]
    fn wsl_tracker_wraps_the_default_login_shell() {
        let marker = Path::new(r"C:\Users\stefan\AppData\Local\Temp\zetta-cwd");
        let shell = wsl_shell_with_tracking(
            Shell::WithArguments {
                program: "wsl.exe".to_owned(),
                args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
                title_override: None,
            },
            Some("/work"),
            Some(marker),
        );

        assert!(matches!(
            shell,
            Shell::WithArguments { args, .. }
                if args[..4] == ["--distribution", "Ubuntu", "--cd", "/work"]
                    && args[4..8] == ["--exec", "/bin/sh", "-c", WSL_CWD_TRACKER]
                    && args.last().map(String::as_str) == marker.to_str()
        ));
    }

    #[test]
    fn wsl_wrapper_prefers_prompt_cwd_reports_and_keeps_a_shell_fallback() {
        assert!(WSL_CWD_TRACKER.contains("PROMPT_COMMAND="));
        assert!(WSL_CWD_TRACKER.contains("--on-event fish_prompt"));
        assert!(WSL_CWD_TRACKER.contains("add-zsh-hook precmd __zetta_report_cwd"));
        assert!(WSL_CWD_TRACKER.contains("source \"$ZDOTDIR/.zshenv\""));
        assert!(WSL_CWD_TRACKER.contains("rm -rf -- \"$ZETTA_INTEGRATION_ZDOTDIR\""));
        assert!(!WSL_CWD_TRACKER.contains("source \"$ZDOTDIR/.zshrc\""));
        assert!(WSL_CWD_TRACKER.contains("]7;file://localhost"));
        assert!(WSL_CWD_TRACKER.contains("]2;zetta-cwd:"));
        assert!(WSL_CWD_TRACKER.contains("readlink \"/proc/$parent/cwd\""));
    }

    #[test]
    fn normalizes_hyphenated_page_key_names() {
        let keymap = r#"{"ctrl-page-up":"zetta::NextTab","ctrl-page-down":"zetta::PreviousTab"}"#;
        assert_eq!(
            normalize_keymap_key_names(keymap),
            r#"{"ctrl-pageup":"zetta::NextTab","ctrl-pagedown":"zetta::PreviousTab"}"#
        );
    }

    #[test]
    fn nested_pane_layouts_split_and_collapse() {
        let mut layout = PaneLayout::Pane(1);
        assert!(layout.split(1, SplitAxis::Horizontal, 2));
        assert!(layout.split(2, SplitAxis::Vertical, 3));
        assert!(!layout.split(99, SplitAxis::Vertical, 4));

        let layout = layout.without(2).unwrap();
        assert_eq!(
            layout,
            PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(PaneLayout::Pane(1)),
                second: Box::new(PaneLayout::Pane(3)),
            }
        );
    }

    #[test]
    fn split_profile_comes_from_the_active_pane() {
        let system = Profile {
            name: "System".to_owned(),
            command: task::Shell::System,
            theme: None,
        };
        let zsh = Profile {
            name: "Zsh".to_owned(),
            command: task::Shell::Program("zsh".to_owned()),
            theme: Some("One Light".to_owned()),
        };
        let tab = Tab {
            id: 1,
            panes: vec![
                TerminalPane {
                    id: 1,
                    profile: system,
                    view: None,
                    error: None,
                    wsl_cwd_file: None,
                },
                TerminalPane {
                    id: 2,
                    profile: zsh,
                    view: None,
                    error: None,
                    wsl_cwd_file: None,
                },
            ],
            layout: PaneLayout::Split {
                axis: SplitAxis::Vertical,
                first: Box::new(PaneLayout::Pane(1)),
                second: Box::new(PaneLayout::Pane(2)),
            },
            active_pane: 2,
            focus_history: vec![1, 2],
            broadcast_input: false,
            custom_title: None,
            rename_buffer: None,
            rename_cursor: 0,
            rename_select_all: false,
        };

        let profile = tab.active_profile().unwrap();
        assert_eq!(profile.name, "Zsh");
        assert_eq!(profile.theme.as_deref(), Some("One Light"));
    }

    #[test]
    fn closing_active_pane_restores_previous_focus() {
        let profile = Profile {
            name: "System".to_owned(),
            command: task::Shell::System,
            theme: None,
        };
        let pane = |id| TerminalPane {
            id,
            profile: profile.clone(),
            view: None,
            error: None,
            wsl_cwd_file: None,
        };
        let mut tab = Tab {
            id: 1,
            panes: vec![pane(1), pane(2), pane(3)],
            layout: PaneLayout::Pane(1),
            active_pane: 3,
            focus_history: vec![1, 2, 3],
            broadcast_input: false,
            custom_title: None,
            rename_buffer: None,
            rename_cursor: 0,
            rename_select_all: false,
        };

        tab.panes.retain(|pane| pane.id != 3);
        tab.restore_focus_after_close(3, 1);

        assert_eq!(tab.active_pane, 2);
        assert_eq!(tab.focus_history, vec![1, 2]);
    }

    #[test]
    fn closing_inactive_pane_preserves_focus() {
        let profile = Profile {
            name: "System".to_owned(),
            command: task::Shell::System,
            theme: None,
        };
        let pane = |id| TerminalPane {
            id,
            profile: profile.clone(),
            view: None,
            error: None,
            wsl_cwd_file: None,
        };
        let mut tab = Tab {
            id: 1,
            panes: vec![pane(1), pane(2), pane(3)],
            layout: PaneLayout::Pane(1),
            active_pane: 3,
            focus_history: vec![1, 2, 3],
            broadcast_input: false,
            custom_title: None,
            rename_buffer: None,
            rename_cursor: 0,
            rename_select_all: false,
        };

        tab.panes.retain(|pane| pane.id != 1);
        tab.restore_focus_after_close(1, 2);

        assert_eq!(tab.active_pane, 3);
        assert_eq!(tab.focus_history, vec![2, 3]);
    }

    #[test]
    fn directional_focus_moves_between_quarter_panes() {
        let mut layout = PaneLayout::Pane(1);
        assert!(layout.split(1, SplitAxis::Horizontal, 2));
        assert!(layout.split(1, SplitAxis::Vertical, 3));
        assert!(layout.split(2, SplitAxis::Vertical, 4));

        assert_eq!(layout.adjacent_pane(1, PaneDirection::Right), Some(3));
        assert_eq!(layout.adjacent_pane(1, PaneDirection::Down), Some(2));
        assert_eq!(layout.adjacent_pane(3, PaneDirection::Down), Some(4));
        assert_eq!(layout.adjacent_pane(4, PaneDirection::Left), Some(2));
        assert_eq!(layout.adjacent_pane(4, PaneDirection::Up), Some(3));
        assert_eq!(layout.regions().len(), 4);
    }

    #[test]
    fn resize_handles_cover_edges_and_respect_tiling() {
        let window = size(px(800.), px(600.));
        let untiled = Tiling::default();
        assert_eq!(
            resize_edge(point(px(1.), px(1.)), window, untiled),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge(point(px(799.), px(300.)), window, untiled),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            resize_edge(point(px(9.), px(300.)), window, untiled),
            Some(ResizeEdge::Left)
        );
        assert_eq!(resize_edge(point(px(11.), px(300.)), window, untiled), None);
        assert_eq!(
            resize_edge(point(px(400.), px(300.)), window, untiled),
            None
        );

        let tiled_left = Tiling {
            left: true,
            ..Tiling::default()
        };
        assert_eq!(
            resize_edge(point(px(1.), px(300.)), window, tiled_left),
            None
        );
    }
}
