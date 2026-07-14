#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod zetta_assets;

const ZETTA_APP_ID: &str = "Zetta";
const ZETTA_DEFAULT_THEME: &str = "One Light";

use std::{collections::HashMap, env, fs, path::PathBuf, sync::Arc};

use anyhow::{Context as _, Result};
use config::{Config, ShellProfile};
use gpui::{
    Action, Anchor, App, AppContext as _, Bounds, Context, CursorStyle, Decorations, Entity,
    Focusable, HitboxBehavior, InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent,
    MAX_BUTTONS_PER_SIDE, MouseButton, Pixels, Point, Render, ResizeEdge, Size, Subscription,
    Tiling, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowButton,
    WindowButtonLayout, WindowControlArea, WindowControls, WindowDecorations, WindowOptions,
    actions, canvas, div, point, px, size, transparent_black,
};
use schemars::JsonSchema;
use serde::Deserialize;
use settings::{KeymapFile, KeymapFileLoadResult, Settings as _};
use terminal::{TerminalBuilder, terminal_settings::TerminalSettings};
use terminal_view::{TerminalView, TerminalViewEvent};
use theme::{ActiveTheme, ClientDecorationsExt as _, GlobalTheme, ThemeRegistry};
use ui::{
    ButtonCommon as _, ButtonSize, Clickable as _, Color, IconButton, IconButtonShape, IconName,
    IconSize, Label, LabelSize, PopoverMenu, Tooltip, prelude::*,
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
        IncreaseTerminalFontSize,
        DecreaseTerminalFontSize,
        ResetTerminalFontSize,
        ReloadConfiguration
    ]
);

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta)]
#[serde(deny_unknown_fields)]
struct OpenProfile {
    slot: usize,
}

struct TerminalPane {
    id: u64,
    profile_name: String,
    view: Option<Entity<TerminalView>>,
    error: Option<String>,
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
    custom_title: Option<String>,
    rename_buffer: Option<String>,
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
}

struct Zetta {
    launch_config: Config,
    tabs: Vec<Tab>,
    active_tab: usize,
    selected_profile: usize,
    profiles: Vec<ShellProfile>,
    working_directory: Option<PathBuf>,
    next_tab_id: u64,
    next_pane_id: u64,
    rename_focus: gpui::FocusHandle,
    titlebar_dragging: bool,
    button_layout: WindowButtonLayout,
    _subscriptions: Vec<Subscription>,
}

impl Zetta {
    fn new(config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let button_layout = system_window_button_layout(cx);
        let mut this = Self {
            launch_config: config.clone(),
            tabs: Vec::new(),
            active_tab: 0,
            selected_profile: config.default_profile,
            profiles: config.profiles,
            working_directory: config.working_directory,
            next_tab_id: 1,
            next_pane_id: 1,
            rename_focus: cx.focus_handle(),
            titlebar_dragging: false,
            button_layout,
            _subscriptions: vec![
                cx.observe_button_layout_changed(window, |this, _, cx| {
                    this.button_layout = system_window_button_layout(cx);
                    cx.notify();
                }),
                cx.observe_window_activation(window, |this, window, cx| {
                    if window.is_window_active() && !this.is_renaming_tab() {
                        this.focus_active(window, cx);
                    }
                }),
            ],
        };
        this.open_tab(window, cx);
        this
    }

    fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let working_directory = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
            .and_then(|view| view.read(cx).terminal().read(cx).working_directory())
            .or_else(|| self.working_directory.clone());
        let profile = self.profiles[self.selected_profile].clone();
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        self.tabs.push(Tab {
            id: tab_id,
            panes: vec![TerminalPane {
                id: pane_id,
                profile_name: profile.name.clone(),
                view: None,
                error: None,
            }],
            layout: PaneLayout::Pane(pane_id),
            active_pane: pane_id,
            custom_title: None,
            rename_buffer: None,
            rename_select_all: false,
        });
        self.active_tab = self.tabs.len() - 1;

        self.spawn_terminal(tab_id, pane_id, profile, working_directory, window, cx);
    }

    fn spawn_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: ShellProfile,
        working_directory: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = TerminalSettings::get_global(cx).clone();
        let builder = TerminalBuilder::new(
            working_directory,
            None,
            profile.shell,
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
                        let view = cx.new(|cx| TerminalView::new(terminal, window, cx));
                        cx.subscribe_in(
                            &view,
                            window,
                            move |this, _, event, window, cx| match event {
                                TerminalViewEvent::Close => {
                                    this.close_pane(tab_id, pane_id, window, cx);
                                }
                                TerminalViewEvent::TitleChanged => cx.notify(),
                            },
                        )
                        .detach();
                        let focus_handle = view.focus_handle(cx);
                        cx.on_focus(&focus_handle, window, move |this, _, cx| {
                            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                                tab.active_pane = pane_id;
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
        tab.active_pane = layout.first_pane();
        tab.layout = layout;
        self.active_tab = tab_index;
        self.focus_active(window, cx);
    }

    fn split_active_pane(&mut self, axis: SplitAxis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let tab_id = tab.id;
        let active_pane = tab.active_pane;
        let working_directory = tab
            .active_pane()
            .and_then(|pane| pane.view.as_ref())
            .and_then(|view| view.read(cx).terminal().read(cx).working_directory())
            .or_else(|| self.working_directory.clone());
        let profile = self.profiles[self.selected_profile].clone();
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;

        let tab = &mut self.tabs[self.active_tab];
        if !tab.layout.split(active_pane, axis, pane_id) {
            return;
        }
        tab.panes.push(TerminalPane {
            id: pane_id,
            profile_name: profile.name.clone(),
            view: None,
            error: None,
        });
        tab.active_pane = pane_id;
        self.spawn_terminal(tab_id, pane_id, profile, working_directory, window, cx);
        cx.notify();
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    fn new_window(&mut self, _: &NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        open_zetta_window(self.launch_config.clone(), cx).log_err();
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
        tab.active_pane = pane_id;
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
                eprintln!("Could not reload Zetta configuration: {error:#}");
                return;
            }
        };

        load_user_themes(cx).log_err();
        if let Err(error) = apply_config_settings(&config, cx) {
            eprintln!("Could not reload Zetta configuration: {error:#}");
            return;
        }
        load_keybindings(&config.keymap_path, config.profiles.len(), cx);

        self.selected_profile = config.default_profile;
        self.profiles = config.profiles.clone();
        self.working_directory = config.working_directory.clone();
        self.launch_config = config;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.focus_active(window, cx);
        }
    }

    fn previous_tab(&mut self, _: &PreviousTab, window: &mut Window, cx: &mut Context<Self>) {
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
            tab.rename_buffer = Some(tab.custom_title.clone().unwrap_or(automatic_title));
            tab.rename_select_all = true;
        }
        self.rename_focus.focus(window, cx);
        cx.notify();
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
                    tab.rename_select_all = false;
                } else {
                    buffer.pop();
                }
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                tab.rename_select_all = true;
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if tab.rename_select_all {
                        buffer.clear();
                        tab.rename_select_all = false;
                    }
                    buffer.push_str(text);
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
                        .child(format!("Starting {}...", pane.profile_name))
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

impl Render for Zetta {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let handle = cx.entity().downgrade();
        let supported_controls = window.window_controls();
        let is_maximized = window.is_maximized();
        let left_window_controls = render_window_controls(
            self.button_layout.left,
            supported_controls,
            is_maximized,
            false,
        );
        let right_window_controls = render_window_controls(
            self.button_layout.right,
            supported_controls,
            is_maximized,
            true,
        );
        let title_bar = div()
            .id("zetta-title-bar")
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .h_8()
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(colors.title_bar_background)
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
                let select_handle = handle.clone();
                let close_handle = handle.clone();
                let rename_view = tab.active_pane().and_then(|pane| pane.view.clone());
                let title = if let Some(buffer) = tab.rename_buffer.as_ref() {
                    format!("{buffer}|").into()
                } else if let Some(custom_title) = tab.custom_title.as_ref() {
                    custom_title.clone().into()
                } else if let Some(view) = tab.active_pane().and_then(|pane| pane.view.as_ref()) {
                    view.read(cx).tab_content_text(0, cx)
                } else {
                    tab.active_pane()
                        .map(|pane| pane.profile_name.clone())
                        .unwrap_or_else(|| "Terminal".to_string())
                        .into()
                };
                let content =
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .child(
                            Icon::new(IconName::Terminal)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .child(Label::new(title).truncate().size(LabelSize::Small).color(
                            if selected {
                                Color::Default
                            } else {
                                Color::Muted
                            },
                        ))
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
                    .border_color(colors.border)
                    .when(selected, |this| {
                        this.bg(colors.editor_background)
                            .border_t_2()
                            .border_color(colors.text_accent)
                    })
                    .when(!selected, |this| this.bg(colors.tab_inactive_background))
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
                        IconButton::new(("close-tab", tab.id as usize), IconName::Close)
                            .shape(IconButtonShape::Square)
                            .icon_size(IconSize::XSmall)
                            .aria_label("Close tab")
                            .tooltip(Tooltip::text("Close tab"))
                            .on_click(move |_, window, cx| {
                                close_handle
                                    .update(cx, |this, cx| this.close_tab_at(index, window, cx))
                                    .ok();
                            }),
                    )
            })
            .collect::<Vec<_>>();

        let profile_menu_profiles = self.profiles.clone();
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
                                    .child(Label::new(label_for_row.clone()))
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

        let content = div()
            .key_context("Zetta")
            .size_full()
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
            .on_action(cx.listener(Self::increase_terminal_font_size))
            .on_action(cx.listener(Self::decrease_terminal_font_size))
            .on_action(cx.listener(Self::reset_terminal_font_size))
            .on_action(cx.listener(Self::reload_configuration))
            .when(self.is_renaming_tab(), |content| {
                content.track_focus(&self.rename_focus)
            })
            .on_key_down(cx.listener(Self::rename_key_down))
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
                    .child(div().min_w_0().flex_1()),
            )
            .child(div().flex_1().min_h_0().child(body));

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

fn render_window_controls(
    buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    supported_controls: WindowControls,
    is_maximized: bool,
    right_aligned: bool,
) -> impl IntoElement {
    h_flex()
        .h_full()
        .flex_none()
        .when(right_aligned, |controls| controls.ml_auto())
        .children(buttons.into_iter().flatten().filter_map(|button| {
            match button {
                WindowButton::Minimize if supported_controls.minimize => Some(
                    div()
                        .window_control_area(WindowControlArea::Min)
                        .child(
                            IconButton::new(button.id(), IconName::GenericMinimize)
                                .shape(IconButtonShape::Square)
                                .size(ButtonSize::Large)
                                .icon_size(IconSize::Small)
                                .aria_label("Minimize window")
                                .tooltip(Tooltip::text("Minimize"))
                                .on_click(|_, window, _| window.minimize_window()),
                        )
                        .into_any_element(),
                ),
                WindowButton::Maximize if supported_controls.maximize => Some(
                    div()
                        .window_control_area(WindowControlArea::Max)
                        .child(
                            IconButton::new(
                                button.id(),
                                if is_maximized {
                                    IconName::GenericRestore
                                } else {
                                    IconName::GenericMaximize
                                },
                            )
                            .shape(IconButtonShape::Square)
                            .size(ButtonSize::Large)
                            .icon_size(IconSize::Small)
                            .aria_label(if is_maximized {
                                "Restore window"
                            } else {
                                "Maximize window"
                            })
                            .tooltip(Tooltip::text(if is_maximized {
                                "Restore"
                            } else {
                                "Maximize"
                            }))
                            .on_click(|_, window, _| window.zoom_window()),
                        )
                        .into_any_element(),
                ),
                WindowButton::Close => Some(
                    div()
                        .window_control_area(WindowControlArea::Close)
                        .child(
                            IconButton::new(button.id(), IconName::GenericClose)
                                .shape(IconButtonShape::Square)
                                .size(ButtonSize::Large)
                                .icon_size(IconSize::Small)
                                .aria_label("Close window")
                                .tooltip(Tooltip::text("Close"))
                                .on_click(|_, window, _| window.remove_window()),
                        )
                        .into_any_element(),
                ),
                _ => None,
            }
        }))
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

fn load_user_themes(cx: &mut App) -> Result<()> {
    let themes_dir = config::themes_dir();
    fs::create_dir_all(&themes_dir)
        .with_context(|| format!("creating theme directory {}", themes_dir.display()))?;
    let registry = ThemeRegistry::global(cx);
    for entry in fs::read_dir(&themes_dir)
        .with_context(|| format!("reading theme directory {}", themes_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("reading theme {}", path.display()))?;
        theme_settings::load_user_theme(&registry, &bytes)
            .with_context(|| format!("loading theme {}", path.display()))?;
    }
    Ok(())
}

fn apply_zetta_theme_overrides(cx: &mut App) {
    let mut theme = cx.theme().as_ref().clone();
    let colors = &mut theme.styles.colors;
    colors.scrollbar_thumb_background = colors.text_muted.opacity(0.7);
    colors.scrollbar_thumb_hover_background = colors.text.opacity(0.85);
    colors.scrollbar_thumb_active_background = colors.text_accent.opacity(0.95);
    GlobalTheme::update_theme(cx, Arc::new(theme));
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
        KeyBinding::new("alt-left", FocusPaneLeft, Some("Zetta > Terminal")),
        KeyBinding::new("alt-right", FocusPaneRight, Some("Zetta > Terminal")),
        KeyBinding::new("alt-up", FocusPaneUp, Some("Zetta > Terminal")),
        KeyBinding::new("alt-down", FocusPaneDown, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-tab", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new("f2", RenameTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-=", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-+", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl--", DecreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-0", ResetTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-r",
            ReloadConfiguration,
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

fn open_zetta_window(config: Config, cx: &mut App) -> Result<()> {
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
            cx.new(|cx| Zetta::new(config, window, cx))
        },
    )
    .context("opening Zetta window")?;
    cx.activate(true);
    Ok(())
}

fn run() -> Result<()> {
    let (config_path, keymap_path) = parse_args()?;
    let config = Config::load(config_path.as_deref(), keymap_path)?;
    let keymap_path = config.keymap_path.clone();
    let profile_count = config.profiles.len();
    gpui_platform::application()
        .with_assets(ZettaAssets)
        .run(move |cx: &mut App| {
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

            open_zetta_window(config, cx).expect("failed to open Zetta window");
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
