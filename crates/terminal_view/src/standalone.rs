mod scrollback_temp;
mod terminal_element;
mod terminal_scrollbar;

pub use scrollback_temp::{
    claim_for_editor as claim_scrollback_for_editor, remove_managed as remove_scrollback_file,
    start_cleanup_monitor as start_scrollback_cleanup_monitor,
};

use std::{cmp, ops::Range as StdRange, path::PathBuf, sync::Arc, time::Duration};

use gpui::{
    Action, AnyElement, App, AppContext as _, ClipboardItem, Context, Corners, DismissEvent,
    Entity, EventEmitter, FocusHandle, Focusable, KeyContext, KeyDownEvent, Keystroke, MouseButton,
    MouseDownEvent, Pixels, Point, Render, ScrollWheelEvent, Subscription, Task, Window, actions,
    anchored, deferred, div, px,
};
use schemars::JsonSchema;
use serde::Deserialize;
use settings::{Settings, SettingsStore, TerminalBell, TerminalBlink};
use terminal::{
    Clear, Copy, Event, HoveredWord, MaybeNavigationTarget, Modes, Paste, PasteText, PasteTrimmed,
    ScrollLineDown, ScrollLineUp, ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop,
    Search, ShowCharacterPalette, Terminal, TerminalBounds, ToggleViMode,
    console_palette_for_theme,
    terminal_settings::{CursorShape, TerminalSettings},
};
use terminal_element::TerminalElement;
use terminal_scrollbar::TerminalScrollHandle;
use theme::{ActiveTheme, Theme};
use theme_settings::ThemeSettings;
use ui::{
    ContextMenu, ScrollAxes, ScrollbarStyle, Scrollbars, WithScrollbar,
    prelude::*,
    scrollbars::{self, ScrollbarVisibility},
};
use util::paths::{PathStyle, PathWithPosition, home_dir};

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Action)]
#[action(namespace = terminal)]
pub struct SendText(pub String);

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Action)]
#[action(namespace = terminal)]
pub struct SendKeystroke(pub String);

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Action)]
#[action(namespace = terminal)]
pub struct RenameTerminal;

actions!(
    terminal_view,
    [
        SelectAll,
        ClearClipboard,
        CopyAndClearSelection,
        EditScrollback,
        SearchScrollback,
        SearchNextMatch,
        SearchPreviousMatch,
        DismissSearch,
        SelectAllSearchText,
    ]
);

actions!(zetta, [ChangePaneTheme, SavePaneOutput, SetPaneOverlay]);

#[derive(Clone, Debug)]
pub enum TerminalViewEvent {
    Close,
    TitleChanged,
    Input(TerminalInput),
    OpenEditor(EditorRequest),
}

#[derive(Clone, Debug)]
pub struct EditorRequest {
    pub command: String,
    pub temporary_path: Option<PathBuf>,
}

fn enabled_input_event(
    enabled: bool,
    build: impl FnOnce() -> TerminalViewEvent,
) -> Option<TerminalViewEvent> {
    enabled.then(build)
}

fn search_request_is_current(
    expected_generation: u64,
    expected_query: &str,
    current_generation: u64,
    current_query: Option<&str>,
) -> bool {
    expected_generation == current_generation && current_query == Some(expected_query)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RightClickAction {
    ContextMenu,
    Paste,
    Forward,
}

fn right_click_action(
    terminal_mouse_mode: bool,
    shift: bool,
    clipboard_has_text: bool,
) -> RightClickAction {
    if terminal_mouse_mode && !shift {
        RightClickAction::Forward
    } else if shift || !clipboard_has_text {
        RightClickAction::ContextMenu
    } else {
        RightClickAction::Paste
    }
}

fn pane_customization_context_menu_entries() -> [(&'static str, Box<dyn Action>); 2] {
    [
        ("Change Pane Theme", Box::new(ChangePaneTheme)),
        ("Set Pane Overlay", Box::new(SetPaneOverlay)),
    ]
}

#[derive(Clone, Debug)]
pub enum TerminalInput {
    Keystroke(Keystroke),
    Text(String),
    Paste(String),
}

#[derive(Clone)]
pub enum TerminalMode {
    Standalone,
    Embedded {
        max_lines_when_unfocused: Option<usize>,
    },
}

#[derive(Clone)]
pub enum ContentMode {
    Scrollable,
    Inline {
        displayed_lines: usize,
        total_lines: usize,
    },
}

impl ContentMode {
    pub fn is_scrollable(&self) -> bool {
        matches!(self, Self::Scrollable)
    }
}

pub struct BlockProperties {
    pub height: u8,
    pub render: Box<dyn Send + Fn(&mut BlockContext) -> AnyElement>,
}

pub struct BlockContext<'a, 'b> {
    pub window: &'a mut Window,
    pub context: &'b mut App,
    pub dimensions: TerminalBounds,
}

pub(crate) struct ImeState {
    pub(crate) marked_text: String,
}

#[derive(Debug)]
pub(crate) struct HoverTarget {
    pub(crate) tooltip: String,
    pub(crate) hovered_word: HoveredWord,
}

struct BlinkManager {
    blink_epoch: usize,
    paused: bool,
    visible: bool,
    enabled: bool,
}

impl BlinkManager {
    fn new() -> Self {
        Self {
            blink_epoch: 0,
            paused: false,
            visible: true,
            enabled: false,
        }
    }

    fn next_epoch(&mut self) -> usize {
        self.blink_epoch += 1;
        self.blink_epoch
    }

    fn enable(&mut self, cx: &mut Context<Self>) {
        if self.enabled {
            return;
        }
        self.enabled = true;
        self.visible = false;
        self.blink(self.blink_epoch, cx);
    }

    fn disable(&mut self, cx: &mut Context<Self>) {
        self.enabled = false;
        self.visible = true;
        self.next_epoch();
        cx.notify();
    }

    fn pause(&mut self, cx: &mut Context<Self>) {
        self.visible = true;
        self.paused = true;
        let epoch = self.next_epoch();
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
            this.update(cx, |this, cx| {
                if this.blink_epoch == epoch {
                    this.paused = false;
                    this.blink(epoch, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn blink(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if !self.enabled || self.paused || epoch != self.blink_epoch {
            return;
        }
        self.visible = !self.visible;
        cx.notify();
        let next_epoch = self.next_epoch();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
            this.update(cx, |this, cx| this.blink(next_epoch, cx)).ok();
        })
        .detach();
    }
}

pub struct TerminalView {
    terminal: Entity<Terminal>,
    theme: Option<Arc<Theme>>,
    font_size_override: Option<Pixels>,
    search_focus_handle: FocusHandle,
    search_query: Option<String>,
    search_active_match: Option<usize>,
    search_generation: u64,
    search_matches_limited: bool,
    search_total_matches: usize,
    search_task: Option<Task<()>>,
    search_select_all: bool,
    search_cursor: usize,
    emit_input_events: bool,
    input_enabled: bool,
    pub(crate) focus_handle: FocusHandle,
    cursor_shape: CursorShape,
    blink_manager: Entity<BlinkManager>,
    blinking_terminal_enabled: bool,
    has_bell: bool,
    system_bell_enabled: bool,
    custom_title: Option<String>,
    pub(crate) hover: Option<HoverTarget>,
    pub(crate) mode: TerminalMode,
    pub(crate) scroll_top: Pixels,
    scroll_handle: TerminalScrollHandle,
    pub(crate) ime_state: Option<ImeState>,
    context_menu: Option<(Entity<ContextMenu>, Point<Pixels>, Subscription)>,
    pub(crate) window_corner_radii: Corners<Pixels>,
    pane_resize_mode_active: bool,
    pane_resize_toggle_action: Option<Box<dyn Action>>,
    pane_move_mode_active: bool,
    pane_move_toggle_action: Option<Box<dyn Action>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

fn should_play_system_bell(system_bell_enabled: bool, bell: TerminalBell) -> bool {
    system_bell_enabled && matches!(bell, TerminalBell::System)
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    pub fn has_open_context_menu(&self) -> bool {
        self.context_menu.is_some()
    }

    pub fn has_open_search(&self) -> bool {
        self.search_query.is_some()
    }

    pub fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search_query = None;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_matches_limited = false;
        self.search_total_matches = 0;
        self.search_active_match = None;
        self.search_select_all = false;
        self.search_cursor = 0;
        self.terminal.update(cx, |terminal, _| {
            Arc::make_mut(&mut terminal.matches).clear()
        });
        cx.notify();
    }

    pub fn new(terminal: Entity<Terminal>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_theme(terminal, None, window, cx)
    }

    pub fn new_with_theme(
        terminal: Entity<Terminal>,
        theme: Option<Arc<Theme>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let effective_theme = theme.clone().unwrap_or_else(|| cx.theme().clone());
        let console_palette = console_palette_for_theme(effective_theme.as_ref());
        terminal.update(cx, |terminal, _| {
            terminal.set_reported_theme(theme.clone());
            terminal.set_console_palette(console_palette);
        });
        let focus_handle = cx.focus_handle();
        let search_focus_handle = cx.focus_handle();
        let focus_in = cx.on_focus_in(&focus_handle, window, |view, window, cx| {
            view.focus_in(window, cx)
        });
        let focus_out = cx.on_focus_out(&focus_handle, window, |view, _, window, cx| {
            view.focus_out(window, cx)
        });
        let blink_manager = cx.new(|_| BlinkManager::new());
        let blink_observer = cx.observe(&blink_manager, |_, _, cx| cx.notify());
        let settings_observer = cx.observe_global::<SettingsStore>(Self::settings_changed);
        let terminal_observer = cx.observe(&terminal, |_, _, cx| cx.notify());
        let terminal_events = cx.subscribe_in(
            &terminal,
            window,
            |view, terminal, event, window, cx| match event {
                Event::Wakeup | Event::SelectionsChanged | Event::BreadcrumbsChanged => {
                    window.invalidate_character_coordinates();
                    if matches!(event, Event::Wakeup) {
                        view.refresh_search(cx);
                    }
                    cx.notify();
                }
                Event::TaskFinished { .. } => cx.notify(),
                Event::TerminalExited(_) => cx.notify(),
                Event::Bell => {
                    view.has_bell = true;
                    if should_play_system_bell(
                        view.system_bell_enabled,
                        TerminalSettings::get_global(cx).bell,
                    ) {
                        window.play_system_bell();
                    }
                    cx.notify();
                }
                Event::BlinkChanged(blinking) => {
                    view.blinking_terminal_enabled = *blinking;
                    view.update_blinking(window, cx);
                }
                // The view already repaints through `terminal_observer`; this is
                // for the chrome outside it, which subscribes separately.
                Event::ResizeRequested { .. } | Event::GridSizeChanged => {}
                Event::TitleChanged => cx.emit(TerminalViewEvent::TitleChanged),
                Event::CloseTerminal => cx.emit(TerminalViewEvent::Close),
                Event::NewNavigationTarget(target) => {
                    view.hover = match target
                        .as_ref()
                        .zip(terminal.read(cx).last_content.last_hovered_word.clone())
                    {
                        Some((MaybeNavigationTarget::Url(url), hovered_word)) => {
                            Some(HoverTarget {
                                tooltip: url.clone(),
                                hovered_word,
                            })
                        }
                        Some((MaybeNavigationTarget::PathLike(path), hovered_word)) => {
                            Some(HoverTarget {
                                tooltip: path.maybe_path.clone(),
                                hovered_word,
                            })
                        }
                        None => None,
                    };
                    cx.notify();
                }
                Event::Open(MaybeNavigationTarget::Url(url)) => cx.open_url(url),
                Event::Open(MaybeNavigationTarget::PathLike(target)) => {
                    match local_path_open_action(target) {
                        LocalPathOpenAction::OpenDirectory(path) => cx.open_with_system(&path),
                        LocalPathOpenAction::RevealFile(path) => cx.reveal_path(&path),
                    }
                }
            },
        );

        Self {
            scroll_handle: TerminalScrollHandle::new(terminal.read(cx)),
            terminal,
            theme,
            font_size_override: None,
            search_focus_handle,
            search_query: None,
            search_active_match: None,
            search_generation: 0,
            search_matches_limited: false,
            search_total_matches: 0,
            search_task: None,
            search_select_all: false,
            search_cursor: 0,
            emit_input_events: false,
            input_enabled: true,
            focus_handle,
            cursor_shape: TerminalSettings::get_global(cx).cursor_shape,
            blink_manager,
            blinking_terminal_enabled: false,
            has_bell: false,
            system_bell_enabled: true,
            custom_title: None,
            hover: None,
            mode: TerminalMode::Standalone,
            scroll_top: Pixels::ZERO,
            ime_state: None,
            context_menu: None,
            window_corner_radii: Corners::default(),
            pane_resize_mode_active: false,
            pane_resize_toggle_action: None,
            pane_move_mode_active: false,
            pane_move_toggle_action: None,
            _subscriptions: vec![
                focus_in,
                focus_out,
                blink_observer,
                settings_observer,
                terminal_observer,
                terminal_events,
            ],
        }
    }

    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    pub fn set_system_bell_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.system_bell_enabled != enabled {
            self.system_bell_enabled = enabled;
            cx.notify();
        }
    }

    fn edit_path_like_target(&mut self, target: terminal::PathLikeTarget, cx: &mut Context<Self>) {
        let path = resolve_local_path(&target);
        let request = self.terminal.update(cx, |terminal, _| {
            let command = terminal.editor_command_for_path(&path, target.path_style)?;
            if terminal.editor_should_open_in_new_pane() {
                Some(EditorRequest {
                    command,
                    temporary_path: None,
                })
            } else {
                terminal.submit_editor_command(command);
                None
            }
        });
        if let Some(request) = request {
            cx.emit(TerminalViewEvent::OpenEditor(request));
        }
    }

    fn edit_scrollback(&mut self, _: &EditScrollback, _: &mut Window, cx: &mut Context<Self>) {
        let output = self.terminal.read(cx).get_content_async();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let output = output.await;
            let temporary_file = executor
                .spawn(async move { scrollback_temp::create(&output) })
                .await;

            match temporary_file {
                Ok(path) => {
                    scrollback_temp::start_cleanup_monitor();
                    let update = this.update(cx, |this, cx| {
                        let request = this.terminal.update(cx, |terminal, _| {
                            let command = terminal.editor_command_for_temporary_path(&path)?;
                            if terminal.editor_should_open_in_new_pane() {
                                Some(EditorRequest {
                                    command,
                                    temporary_path: Some(path.clone()),
                                })
                            } else {
                                terminal.submit_editor_command(command);
                                None
                            }
                        });
                        if let Some(request) = request {
                            cx.emit(TerminalViewEvent::OpenEditor(request));
                        }
                    });
                    if update.is_err() {
                        let _ = scrollback_temp::remove_managed(&path);
                    }
                }
                Err(error) => log::error!("failed to create scrollback editor buffer: {error:#}"),
            }
        })
        .detach();
    }

    pub fn set_emit_input_events(&mut self, enabled: bool) {
        self.emit_input_events = enabled;
    }

    pub fn input_enabled(&self) -> bool {
        self.input_enabled
    }

    /// Controls user input without affecting terminal output or programmatic
    /// input used to restore a pane or broadcast an existing command.
    pub fn set_input_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.input_enabled != enabled {
            self.input_enabled = enabled;
            if !enabled {
                self.ime_state = None;
            }
            cx.notify();
        }
    }

    pub fn set_window_corner_radii(
        &mut self,
        corner_radii: Corners<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.window_corner_radii != corner_radii {
            self.window_corner_radii = corner_radii;
            cx.notify();
        }
    }

    /// Surfaces a "Pane Resize Mode" entry in the right-click context menu.
    /// `toggle_action` is only `Some` when the containing tab has 2+ panes,
    /// since resize mode has nothing to do with a single pane.
    ///
    /// Deliberately does not notify: `deploy_context_menu` snapshots this when
    /// the menu opens, so nothing currently on screen depends on it, and the
    /// caller updates it from `Zetta::render`, where an unconditional notify
    /// would schedule a frame on every frame.
    pub fn set_pane_resize_mode_entry(
        &mut self,
        active: bool,
        toggle_action: Option<Box<dyn Action>>,
    ) {
        self.pane_resize_mode_active = active;
        self.pane_resize_toggle_action = toggle_action;
    }

    /// Surfaces a "Pane Move Mode" entry in the right-click context menu.
    /// `toggle_action` is only `Some` when the containing tab has 2+ panes,
    /// since move mode has nothing to do with a single pane.
    ///
    /// Does not notify, for the same reason as
    /// [`Self::set_pane_resize_mode_entry`].
    pub fn set_pane_move_mode_entry(
        &mut self,
        active: bool,
        toggle_action: Option<Box<dyn Action>>,
    ) {
        self.pane_move_mode_active = active;
        self.pane_move_toggle_action = toggle_action;
    }

    pub fn set_theme(&mut self, theme: Option<Arc<Theme>>, cx: &mut Context<Self>) {
        let effective_theme = theme.clone().unwrap_or_else(|| cx.theme().clone());
        let console_palette = console_palette_for_theme(effective_theme.as_ref());
        self.terminal.update(cx, |terminal, _| {
            terminal.set_reported_theme(theme.clone());
            terminal.set_console_palette(console_palette);
        });
        self.theme = theme;
        cx.notify();
    }

    pub fn increase_font_size(&mut self, cx: &mut Context<Self>) {
        let current = self.effective_font_size(cx);
        self.font_size_override = Some(theme_settings::clamp_font_size(current + px(1.0)));
        cx.notify();
    }

    pub fn decrease_font_size(&mut self, cx: &mut Context<Self>) {
        let current = self.effective_font_size(cx);
        self.font_size_override = Some(theme_settings::clamp_font_size(current - px(1.0)));
        cx.notify();
    }

    pub fn reset_font_size(&mut self, cx: &mut Context<Self>) {
        self.font_size_override = None;
        cx.notify();
    }

    fn effective_font_size(&self, cx: &App) -> Pixels {
        self.font_size_override.unwrap_or_else(|| {
            let settings = ThemeSettings::get_global(cx);
            TerminalSettings::get_global(cx).font_size.map_or_else(
                || settings.buffer_font_size(cx),
                |size| theme_settings::adjusted_font_size(size, cx),
            )
        })
    }

    pub fn theme(&self) -> Option<&Arc<Theme>> {
        self.theme.as_ref()
    }

    pub fn tab_content_text(&self, detail: usize, cx: &App) -> SharedString {
        self.custom_title
            .as_ref()
            .filter(|title| !title.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| self.terminal.read(cx).title(detail == 0))
            .into()
    }

    pub fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.custom_title = title.filter(|title| !title.trim().is_empty());
        cx.emit(TerminalViewEvent::TitleChanged);
        cx.notify();
    }

    pub fn custom_title(&self) -> Option<&str> {
        self.custom_title.as_deref()
    }

    pub fn content_mode(&self, _: &Window, _: &App) -> ContentMode {
        ContentMode::Scrollable
    }

    pub(crate) fn terminal_bounds(&self, cx: &App) -> TerminalBounds {
        self.terminal.read(cx).last_content().terminal_bounds
    }

    pub(crate) fn marked_text_range(&self) -> Option<StdRange<usize>> {
        self.ime_state
            .as_ref()
            .map(|state| 0..state.marked_text.encode_utf16().count())
    }

    pub(crate) fn set_marked_text(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.input_enabled {
            return;
        }
        self.ime_state = (!text.is_empty()).then_some(ImeState { marked_text: text });
        cx.notify();
    }

    pub(crate) fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        self.ime_state = None;
        cx.notify();
    }

    pub(crate) fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.input_enabled && !text.is_empty() {
            self.terminal
                .update(cx, |terminal, _| terminal.input(text.as_bytes().to_vec()));
            if let Some(event) = enabled_input_event(self.emit_input_events, || {
                TerminalViewEvent::Input(TerminalInput::Text(text.to_owned()))
            }) {
                cx.emit(event);
            }
        }
    }

    pub fn apply_input(&mut self, input: &TerminalInput, cx: &mut Context<Self>) {
        match input {
            TerminalInput::Keystroke(keystroke) => {
                self.process_keystroke(keystroke, cx);
            }
            TerminalInput::Text(text) => {
                self.terminal
                    .update(cx, |terminal, _| terminal.input(text.as_bytes().to_vec()));
            }
            TerminalInput::Paste(text) => {
                self.terminal.update(cx, |terminal, _| terminal.paste(text));
            }
        }
    }

    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        let cursor_shape = TerminalSettings::get_global(cx).cursor_shape;
        if cursor_shape != self.cursor_shape {
            self.cursor_shape = cursor_shape;
            self.terminal
                .update(cx, |terminal, _| terminal.set_cursor_shape(cursor_shape));
        }
        cx.notify();
    }

    fn update_blinking(&mut self, window: &Window, cx: &mut Context<Self>) {
        let enabled = self.focus_handle.is_focused(window)
            && match TerminalSettings::get_global(cx).blinking {
                TerminalBlink::Off => false,
                TerminalBlink::On => true,
                TerminalBlink::TerminalControlled => self.blinking_terminal_enabled,
            };
        self.blink_manager.update(cx, |manager, cx| {
            if enabled {
                manager.enable(cx)
            } else {
                manager.disable(cx)
            }
        });
    }

    fn focus_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.focus_in());
        self.update_blinking(window, cx);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn focus_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.blink_manager.update(cx, BlinkManager::disable);
        self.terminal.update(cx, |terminal, _| terminal.focus_out());
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn should_show_cursor(&self, focused: bool, cx: &App) -> bool {
        if !focused
            || self
                .terminal
                .read(cx)
                .last_content
                .mode
                .contains(Modes::ALT_SCREEN)
        {
            return true;
        }
        match TerminalSettings::get_global(cx).blinking {
            TerminalBlink::Off => true,
            TerminalBlink::On => self.blink_manager.read(cx).visible,
            TerminalBlink::TerminalControlled => {
                !self.blinking_terminal_enabled || self.blink_manager.read(cx).visible
            }
        }
    }

    fn search_scrollback(
        &mut self,
        _: &SearchScrollback,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_query.is_none() {
            self.search_query = Some(String::new());
            self.search_cursor = 0;
            self.search_select_all = false;
            self.refresh_search(cx);
        }
        self.search_focus_handle.focus(window, cx);
        cx.notify();
    }

    fn dismiss_search(&mut self, _: &DismissSearch, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_query.take().is_none() {
            return;
        }
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_matches_limited = false;
        self.search_total_matches = 0;
        self.search_active_match = None;
        self.search_select_all = false;
        self.search_cursor = 0;
        self.terminal.update(cx, |terminal, _| {
            Arc::make_mut(&mut terminal.matches).clear()
        });
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn select_all_search_text(
        &mut self,
        _: &SelectAllSearchText,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .search_query
            .as_ref()
            .is_some_and(|query| !query.is_empty())
        {
            self.search_select_all = true;
            cx.notify();
        }
    }

    fn insert_search_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(query) = self.search_query.as_mut() {
            if self.search_select_all {
                query.clear();
                self.search_cursor = 0;
            }
            query.insert_str(self.search_cursor, text);
            self.search_cursor += text.len();
            self.search_select_all = false;
            self.refresh_search(cx);
        }
    }

    fn search_next_match(&mut self, _: &SearchNextMatch, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_search(false, cx);
    }

    fn search_previous_match(
        &mut self,
        _: &SearchPreviousMatch,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_search(true, cx);
    }

    fn navigate_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        let match_count = self.terminal.read(cx).matches.len();
        let Some(index) = navigated_match_index(self.search_active_match, match_count, previous)
        else {
            return;
        };
        self.search_active_match = Some(index);
        self.terminal
            .update(cx, |terminal, _| terminal.activate_match(index));
        cx.notify();
    }

    fn refresh_search(&mut self, cx: &mut Context<Self>) {
        self.search_task.take();
        let Some(query) = self.search_query.as_ref().cloned() else {
            return;
        };
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        self.search_matches_limited = false;
        self.search_total_matches = 0;
        if query.is_empty() {
            self.search_active_match = None;
            self.terminal.update(cx, |terminal, _| {
                Arc::make_mut(&mut terminal.matches).clear()
            });
            cx.notify();
            return;
        }
        let Some(search) = Search::new_literal(&query) else {
            return;
        };
        let terminal = self.terminal.clone();
        let executor = cx.background_executor().clone();
        let task = cx.spawn(async move |this, cx| {
            executor.timer(Duration::from_millis(75)).await;
            let Some(search_task) = this
                .update(cx, |this, cx| {
                    search_request_is_current(
                        generation,
                        &query,
                        this.search_generation,
                        this.search_query.as_deref(),
                    )
                    .then(|| terminal.update(cx, |terminal, cx| terminal.find_matches(search, cx)))
                })
                .ok()
                .flatten()
            else {
                return;
            };
            let result = search_task.await;
            this.update(cx, |this, cx| {
                if !search_request_is_current(
                    generation,
                    &query,
                    this.search_generation,
                    this.search_query.as_deref(),
                ) {
                    return;
                }
                let active_match = result.ranges.len().checked_sub(1);
                this.search_active_match = active_match;
                this.search_matches_limited = result.limit_reached;
                this.search_total_matches = result.total_count;
                this.terminal.update(cx, |terminal, _| {
                    terminal.matches = Arc::new(result.ranges);
                    if let Some(index) = active_match {
                        terminal.activate_match(index);
                    }
                });
                cx.notify();
            })
            .ok();
        });
        self.search_task = Some(task);
    }

    fn process_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let (handled, vi_mode_enabled) = self.terminal.update(cx, |terminal, cx| {
            (
                terminal.try_keystroke(keystroke, TerminalSettings::get_global(cx).option_as_meta),
                terminal.vi_mode_enabled(),
            )
        });
        if handled && vi_mode_enabled {
            cx.notify();
        }
        handled
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.input_enabled {
            return;
        }
        if self.search_query.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => self.dismiss_search(&DismissSearch, window, cx),
                "enter" | "f3" if event.keystroke.modifiers.shift => {
                    self.search_previous_match(&SearchPreviousMatch, window, cx)
                }
                "enter" | "f3" => self.search_next_match(&SearchNextMatch, window, cx),
                "backspace" => {
                    if let Some(query) = self.search_query.as_mut() {
                        if self.search_select_all {
                            query.clear();
                            self.search_cursor = 0;
                        } else if self.search_cursor > 0 {
                            let previous = previous_char_boundary(query, self.search_cursor);
                            query.replace_range(previous..self.search_cursor, "");
                            self.search_cursor = previous;
                        }
                    }
                    self.search_select_all = false;
                    self.refresh_search(cx);
                }
                "delete" => {
                    if let Some(query) = self.search_query.as_mut() {
                        if self.search_select_all {
                            query.clear();
                            self.search_cursor = 0;
                        } else if self.search_cursor < query.len() {
                            let next = next_char_boundary(query, self.search_cursor);
                            query.replace_range(self.search_cursor..next, "");
                        }
                    }
                    self.search_select_all = false;
                    self.refresh_search(cx);
                }
                "left" => {
                    if let Some(query) = self.search_query.as_ref() {
                        self.search_cursor = if self.search_select_all {
                            0
                        } else {
                            previous_char_boundary(query, self.search_cursor)
                        };
                    }
                    self.search_select_all = false;
                    cx.notify();
                }
                "right" => {
                    if let Some(query) = self.search_query.as_ref() {
                        self.search_cursor = if self.search_select_all {
                            query.len()
                        } else {
                            next_char_boundary(query, self.search_cursor)
                        };
                    }
                    self.search_select_all = false;
                    cx.notify();
                }
                "home" => {
                    self.search_cursor = 0;
                    self.search_select_all = false;
                    cx.notify();
                }
                "end" => {
                    self.search_cursor = self.search_query.as_ref().map_or(0, String::len);
                    self.search_select_all = false;
                    cx.notify();
                }
                _ if !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.alt =>
                {
                    if let Some(text) = event.keystroke.key_char.as_ref() {
                        self.insert_search_text(text, cx);
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        if self.terminal.read(cx).vi_mode_enabled()
            && event.keystroke.key == "/"
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.alt
        {
            self.search_scrollback(&SearchScrollback, window, cx);
            cx.stop_propagation();
            return;
        }

        self.has_bell = false;
        self.blink_manager.update(cx, BlinkManager::pause);
        if self.process_keystroke(&event.keystroke, cx) {
            if let Some(event) = enabled_input_event(self.emit_input_events, || {
                TerminalViewEvent::Input(TerminalInput::Keystroke(event.keystroke.clone()))
            }) {
                cx.emit(event);
            }
            cx.stop_propagation();
        }
    }

    fn dispatch_context(&self, cx: &App) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add("Terminal");
        if self.search_query.is_some() {
            context.add("scrollback_search");
        }
        if self.terminal.read(cx).vi_mode_enabled() {
            context.add("vi_mode");
        }
        let mode = self.terminal.read(cx).last_content.mode;
        context.set(
            "screen",
            if mode.contains(Modes::ALT_SCREEN) {
                "alt"
            } else {
                "normal"
            },
        );
        if self.terminal.read(cx).last_content.selection.is_some() {
            context.add("selection");
        }
        context
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.copy(None));
    }

    fn copy_and_clear_selection(
        &mut self,
        _: &CopyAndClearSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal
            .update(cx, |terminal, _| terminal.copy(Some(false)));
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if !self.input_enabled {
            return;
        }
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(text) = clipboard.text() {
            if self.search_query.is_some() {
                self.insert_search_text(&text, cx);
            } else {
                self.terminal
                    .update(cx, |terminal, _| terminal.paste(&text));
                if let Some(event) = enabled_input_event(self.emit_input_events, || {
                    TerminalViewEvent::Input(TerminalInput::Paste(text))
                }) {
                    cx.emit(event);
                }
            }
        }
    }

    fn paste_text(&mut self, _: &PasteText, window: &mut Window, cx: &mut Context<Self>) {
        self.paste(&Paste, window, cx);
    }

    fn paste_trimmed(&mut self, _: &PasteTrimmed, _: &mut Window, cx: &mut Context<Self>) {
        if !self.input_enabled {
            return;
        }
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(text) = clipboard.text() {
            let text = trim_paste_text(&text);
            if self.search_query.is_some() {
                self.insert_search_text(text, cx);
            } else {
                self.terminal.update(cx, |terminal, _| terminal.paste(text));
                if let Some(event) = enabled_input_event(self.emit_input_events, || {
                    TerminalViewEvent::Input(TerminalInput::Paste(text.to_owned()))
                }) {
                    cx.emit(event);
                }
            }
        }
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_top = px(0.);
        self.terminal.update(cx, |terminal, _| terminal.clear());
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.select_all());
        cx.notify();
    }

    fn clear_clipboard(&mut self, _: &ClearClipboard, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem {
            entries: Vec::new(),
        });
    }

    fn clipboard_has_content(cx: &App) -> bool {
        cx.read_from_clipboard()
            .is_some_and(|clipboard| clipboard.text().is_some())
    }

    fn deploy_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_resize_mode_active = self.pane_resize_mode_active;
        let pane_resize_toggle_action = self
            .pane_resize_toggle_action
            .as_ref()
            .map(|action| action.boxed_clone());
        let pane_move_mode_active = self.pane_move_mode_active;
        let pane_move_toggle_action = self
            .pane_move_toggle_action
            .as_ref()
            .map(|action| action.boxed_clone());
        let menu = ContextMenu::build(window, cx, |menu, _, _| {
            let menu = menu
                .context(self.focus_handle.clone())
                .action("Copy", Box::new(Copy))
                .action("Paste", Box::new(Paste))
                .action("Paste Trimmed", Box::new(PasteTrimmed))
                .action("Select All", Box::new(SelectAll))
                .action("Edit Scrollback", Box::new(EditScrollback))
                .action("Save Pane Output", Box::new(SavePaneOutput))
                .separator()
                .action("Clear Clipboard", Box::new(ClearClipboard))
                .action("Clear", Box::new(Clear));
            let menu = pane_customization_context_menu_entries()
                .into_iter()
                .fold(menu, |menu, (label, action)| menu.action(label, action));
            menu.when(
                pane_resize_toggle_action.is_some() || pane_move_toggle_action.is_some(),
                |menu| menu.separator(),
            )
            .when_some(pane_resize_toggle_action, |menu, action| {
                menu.action_checked("Pane Resize Mode", action, pane_resize_mode_active)
            })
            .when_some(pane_move_toggle_action, |menu, action| {
                menu.action_checked("Pane Move Mode", action, pane_move_mode_active)
            })
        });
        window.focus(&menu.focus_handle(cx), cx);
        let subscription =
            cx.subscribe_in(&menu, window, |view, _, _: &DismissEvent, window, cx| {
                if view.context_menu.as_ref().is_some_and(|context_menu| {
                    context_menu.0.focus_handle(cx).contains_focused(window, cx)
                }) {
                    view.focus_handle.focus(window, cx);
                }
                view.context_menu.take();
                cx.notify();
            });
        self.context_menu = Some((menu, position, subscription));
        cx.notify();
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.scroll_wheel(
                event,
                TerminalSettings::get_global(cx).scroll_multiplier.max(0.01),
            )
        });
    }

    fn scroll_line_up(&mut self, _: &ScrollLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.scroll_line_up());
        cx.notify();
    }

    fn scroll_line_down(&mut self, _: &ScrollLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.scroll_line_down());
        cx.notify();
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.scroll_page_up());
        cx.notify();
    }

    fn scroll_page_down(&mut self, _: &ScrollPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.scroll_page_down());
        cx.notify();
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.scroll_to_top());
        cx.notify();
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.scroll_to_bottom());
        cx.notify();
    }

    fn toggle_vi_mode(&mut self, _: &ToggleViMode, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.toggle_vi_mode());
        cx.notify();
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn send_text(&mut self, text: &SendText, _: &mut Window, cx: &mut Context<Self>) {
        if !self.input_enabled {
            return;
        }
        self.terminal.update(cx, |terminal, _| {
            terminal.input(text.0.clone().into_bytes())
        });
        if let Some(event) = enabled_input_event(self.emit_input_events, || {
            TerminalViewEvent::Input(TerminalInput::Text(text.0.clone()))
        }) {
            cx.emit(event);
        }
    }

    fn send_keystroke(&mut self, key: &SendKeystroke, _: &mut Window, cx: &mut Context<Self>) {
        if !self.input_enabled {
            return;
        }
        if let Ok(keystroke) = Keystroke::parse(&key.0) {
            if self.process_keystroke(&keystroke, cx) {
                if let Some(event) = enabled_input_event(self.emit_input_events, || {
                    TerminalViewEvent::Input(TerminalInput::Keystroke(keystroke))
                }) {
                    cx.emit(event);
                }
            }
        }
    }
}

#[derive(Default)]
struct TerminalScrollbarSettings;

impl ScrollbarVisibility for TerminalScrollbarSettings {
    fn visibility(&self, cx: &App) -> scrollbars::ShowScrollbar {
        match TerminalSettings::get_global(cx)
            .scrollbar
            .show
            .unwrap_or_default()
        {
            settings::ShowScrollbar::Auto => scrollbars::ShowScrollbar::Auto,
            settings::ShowScrollbar::System => scrollbars::ShowScrollbar::System,
            settings::ShowScrollbar::Always => scrollbars::ShowScrollbar::Always,
            settings::ShowScrollbar::Never => scrollbars::ShowScrollbar::Never,
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scroll_handle.update(self.terminal.read(cx));
        if let Some(offset) = self.scroll_handle.future_display_offset.take() {
            self.terminal.update(cx, |terminal, _| {
                let delta = offset as i32 - terminal.last_content.display_offset as i32;
                match delta.cmp(&0) {
                    cmp::Ordering::Greater => terminal.scroll_up_by(delta as usize),
                    cmp::Ordering::Less => terminal.scroll_down_by(-delta as usize),
                    cmp::Ordering::Equal => {}
                }
            });
        }

        let focused = self.focus_handle.is_focused(window);
        let owns_transient_focus =
            focused || self.has_open_context_menu() || self.search_query.is_some();
        let theme = self.theme.clone().unwrap_or_else(|| cx.theme().clone());
        let search_overlay = self.search_query.as_ref().map(|query| {
            let match_count = self.terminal.read(cx).matches.len();
            let status = if self.search_matches_limited {
                let position = self
                    .search_active_match
                    .map(|index| (index + 1).to_string())
                    .unwrap_or_else(|| "0".to_owned());
                format!(
                    "{position} / {match_count} shown · {} matches",
                    self.search_total_matches
                )
            } else {
                self.search_active_match
                    .map(|index| format!("{} / {}", index + 1, self.search_total_matches))
                    .unwrap_or_else(|| format!("0 / {}", self.search_total_matches))
            };
            let cursor = self.search_cursor.min(query.len());
            let (query_before, query_after) = query.split_at(cursor);
            let query_before = query_before.to_owned();
            let query_after = query_after.to_owned();
            let query_selected = self.search_select_all;
            let search_focus_handle = self.search_focus_handle.clone();

            div()
                .absolute()
                .top_2()
                .left_2()
                .right_2()
                .flex()
                .justify_end()
                .child(
                    div()
                        .id("terminal-search")
                        .track_focus(&self.search_focus_handle)
                        .w_full()
                        .max_w(px(440.0))
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(theme.colors().border)
                        .bg(theme.colors().elevated_surface_background.alpha(1.0))
                        .shadow_sm()
                        .text_sm()
                        .text_color(theme.colors().text)
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            search_focus_handle.focus(window, cx);
                            cx.stop_propagation();
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .when(query_selected, |input| {
                                            input.bg(theme.colors().element_selection_background)
                                        })
                                        .child(div().whitespace_nowrap().child(query_before))
                                        .when(!query_selected, |input| {
                                            input.child(
                                                div()
                                                    .flex_none()
                                                    .w(px(1.0))
                                                    .h(px(16.0))
                                                    .bg(theme.colors().text_accent),
                                            )
                                        })
                                        .child(div().whitespace_nowrap().child(query_after)),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(theme.colors().text_muted)
                                        .child(status),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.colors().text_muted)
                                .child("Enter next  Shift+Enter previous  Esc close"),
                        ),
                )
        });
        div()
            .id("terminal-view")
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .key_context(self.dispatch_context(cx))
            .on_action(cx.listener(Self::send_text))
            .on_action(cx.listener(Self::send_keystroke))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::copy_and_clear_selection))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::paste_text))
            .on_action(cx.listener(Self::paste_trimmed))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear_clipboard))
            .on_action(cx.listener(Self::scroll_line_up))
            .on_action(cx.listener(Self::scroll_line_down))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_action(cx.listener(Self::toggle_vi_mode))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::edit_scrollback))
            .on_action(cx.listener(Self::search_scrollback))
            .on_action(cx.listener(Self::search_next_match))
            .on_action(cx.listener(Self::search_previous_match))
            .on_action(cx.listener(Self::dismiss_search))
            .on_action(cx.listener(Self::select_all_search_text))
            .capture_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    let terminal_mouse_mode = this
                        .terminal
                        .read(cx)
                        .last_content
                        .mode
                        .contains(Modes::MOUSE_MODE);
                    let clipboard_has_text = !terminal_mouse_mode
                        && !event.modifiers.shift
                        && Self::clipboard_has_content(cx);
                    let action = right_click_action(
                        terminal_mouse_mode,
                        event.modifiers.shift,
                        clipboard_has_text,
                    );
                    match action {
                        RightClickAction::ContextMenu => {
                            this.deploy_context_menu(event.position, window, cx);
                        }
                        RightClickAction::Paste => {
                            this.paste(&Paste, window, cx);
                        }
                        RightClickAction::Forward => return,
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("terminal-view-container")
                    .size_full()
                    // The wrapper must remain opaque even when this terminal
                    // owns a client-window corner. Apply the same radii as the
                    // terminal surface instead of removing its background, or
                    // inactive-pane opacity and modal backdrops expose the
                    // transparent window underneath.
                    .when(
                        self.window_corner_radii.bottom_left > Pixels::ZERO,
                        |container| container.rounded_bl(self.window_corner_radii.bottom_left),
                    )
                    .when(
                        self.window_corner_radii.bottom_right > Pixels::ZERO,
                        |container| container.rounded_br(self.window_corner_radii.bottom_right),
                    )
                    .bg(theme.colors().editor_background)
                    .child(
                        TerminalElement::new(
                            self.terminal.clone(),
                            cx.entity(),
                            self.focus_handle.clone(),
                            focused,
                            self.should_show_cursor(focused, cx),
                            None,
                            self.mode.clone(),
                        )
                        .with_theme(self.theme.clone())
                        .with_font_size(self.font_size_override),
                    )
                    .when(owns_transient_focus, |container| {
                        container.custom_scrollbars(
                            Scrollbars::for_settings::<TerminalScrollbarSettings>()
                                .show_along(ScrollAxes::Vertical)
                                .style(ScrollbarStyle::Editor)
                                .tracked_scroll_handle(&self.scroll_handle),
                            window,
                            cx,
                        )
                    }),
            )
            .children(self.context_menu.as_ref().map(|(menu, position, _)| {
                deferred(
                    anchored()
                        .position(*position)
                        .anchor(gpui::Anchor::TopLeft)
                        .child(menu.clone()),
                )
                .with_priority(1)
            }))
            .when_some(search_overlay, |view, search| view.child(search))
    }
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(text.len())
}

fn navigated_match_index(
    active: Option<usize>,
    match_count: usize,
    previous: bool,
) -> Option<usize> {
    if match_count == 0 {
        return None;
    }
    let active = active
        .filter(|index| *index < match_count)
        .unwrap_or(if previous { 0 } else { match_count - 1 });
    Some(if previous {
        active.checked_sub(1).unwrap_or(match_count - 1)
    } else {
        (active + 1) % match_count
    })
}

fn trim_paste_text(text: &str) -> &str {
    text.trim()
}

#[derive(Debug, PartialEq, Eq)]
enum LocalPathOpenAction {
    OpenDirectory(PathBuf),
    RevealFile(PathBuf),
}

fn local_path_open_action(target: &terminal::PathLikeTarget) -> LocalPathOpenAction {
    let path = resolve_local_path(target);
    if path.is_dir() {
        LocalPathOpenAction::OpenDirectory(path)
    } else {
        LocalPathOpenAction::RevealFile(path)
    }
}

fn resolve_local_path(target: &terminal::PathLikeTarget) -> PathBuf {
    let path = PathWithPosition::parse_str(&target.maybe_path).path;
    let path = if target.path_style == PathStyle::local() {
        expand_local_tilde_path(path)
    } else {
        path
    };
    if target.path_style.is_posix() {
        return if is_posix_absolute_path(&path) || path.is_absolute() {
            path
        } else if let Some(terminal_dir) = target
            .terminal_dir
            .as_deref()
            .filter(|directory| is_posix_absolute_path(directory))
        {
            join_posix_path(terminal_dir, &path)
        } else {
            // Keep the relative path for the WSL editor dispatcher. It
            // resolves it against `pwd` before passing the Windows helper a
            // converted absolute path, so it never falls back to C:\WINDOWS.
            path
        };
    }

    let path = normalize_local_path(path);
    if path.is_absolute() {
        path
    } else if let Some(terminal_dir) = &target.terminal_dir {
        terminal_dir.join(path)
    } else {
        path
    }
}

fn expand_local_tilde_path(path: PathBuf) -> PathBuf {
    let Some(path) = path.to_str() else {
        return path;
    };
    let Some(remainder) = path.strip_prefix('~') else {
        return path.into();
    };
    if remainder.is_empty() {
        return home_dir().clone();
    }

    let Some(separator) = remainder.chars().next() else {
        return path.into();
    };
    if !PathStyle::local().separators_ch().contains(&separator) {
        return path.into();
    }

    home_dir().join(&remainder[separator.len_utf8()..])
}

fn is_posix_absolute_path(path: &std::path::Path) -> bool {
    path.to_str().is_some_and(|path| path.starts_with('/'))
}

fn join_posix_path(directory: &std::path::Path, path: &std::path::Path) -> PathBuf {
    let directory = directory.to_string_lossy();
    let path = path.to_string_lossy();
    PathBuf::from(format!(
        "{}/{}",
        directory.trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
}

#[cfg(target_os = "windows")]
fn normalize_local_path(path: PathBuf) -> PathBuf {
    normalize_windows_path(path)
}

#[cfg(not(target_os = "windows"))]
fn normalize_local_path(path: PathBuf) -> PathBuf {
    path
}

#[cfg(any(target_os = "windows", test))]
fn normalize_windows_path(path: PathBuf) -> PathBuf {
    let mut path = path.to_string_lossy().replace('/', "\\");
    if path.starts_with(r"\\.\") {
        return PathBuf::from(path);
    }
    while path.contains("\\.\\") {
        path = path.replace("\\.\\", "\\");
    }
    while let Some(remainder) = path.strip_prefix(".\\") {
        path = remainder.to_owned();
    }
    if path == "." {
        path.clear();
    } else if path.ends_with("\\.") {
        path.pop();
    }
    PathBuf::from(path)
}

#[cfg(test)]
#[path = "tests/standalone.rs"]
mod tests;
