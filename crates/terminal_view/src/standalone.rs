mod terminal_element;
mod terminal_scrollbar;

use std::{cmp, ops::Range as StdRange, time::Duration};

use gpui::{
    Action, AnyElement, App, AppContext as _, ClipboardEntry, Context, Entity, EventEmitter,
    DismissEvent, FocusHandle, Focusable, KeyContext, KeyDownEvent, Keystroke, MouseButton,
    MouseDownEvent, Pixels, Point, Render, ScrollWheelEvent, Subscription, Window, actions,
    anchored, deferred, div, px,
};
use schemars::JsonSchema;
use serde::Deserialize;
use settings::{Settings, SettingsStore, TerminalBell, TerminalBlink};
use terminal::{
    Clear, Copy, Event, HoveredWord, MaybeNavigationTarget, Modes, Paste, PasteText,
    ScrollLineDown, ScrollLineUp, ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop,
    ShowCharacterPalette, Terminal, TerminalBounds, ToggleViMode,
    terminal_settings::{CursorShape, TerminalSettings},
};
use terminal_element::TerminalElement;
use terminal_scrollbar::TerminalScrollHandle;
use theme::ActiveTheme;
use ui::{
    ContextMenu, ScrollAxes, ScrollbarStyle, Scrollbars, WithScrollbar,
    prelude::*,
    scrollbars::{self, ScrollbarVisibility},
};

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

actions!(terminal_view, [SelectAll]);

#[derive(Clone, Copy, Debug)]
pub enum TerminalViewEvent {
    Close,
    TitleChanged,
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
            cx.background_executor()
                .timer(CURSOR_BLINK_INTERVAL)
                .await;
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
            cx.background_executor()
                .timer(CURSOR_BLINK_INTERVAL)
                .await;
            this.update(cx, |this, cx| this.blink(next_epoch, cx)).ok();
        })
        .detach();
    }
}

pub struct TerminalView {
    terminal: Entity<Terminal>,
    pub(crate) focus_handle: FocusHandle,
    cursor_shape: CursorShape,
    blink_manager: Entity<BlinkManager>,
    blinking_terminal_enabled: bool,
    has_bell: bool,
    custom_title: Option<String>,
    pub(crate) hover: Option<HoverTarget>,
    pub(crate) mode: TerminalMode,
    pub(crate) scroll_top: Pixels,
    scroll_handle: TerminalScrollHandle,
    pub(crate) ime_state: Option<ImeState>,
    context_menu: Option<(Entity<ContextMenu>, Point<Pixels>, Subscription)>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    pub fn has_open_context_menu(&self) -> bool {
        self.context_menu.is_some()
    }

    pub fn new(
        terminal: Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
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
                        cx.emit(TerminalViewEvent::TitleChanged);
                    }
                    cx.notify();
                }
                Event::Bell => {
                    view.has_bell = true;
                    if matches!(TerminalSettings::get_global(cx).bell, TerminalBell::System) {
                        window.play_system_bell();
                    }
                    cx.notify();
                }
                Event::BlinkChanged(blinking) => {
                    view.blinking_terminal_enabled = *blinking;
                    view.update_blinking(window, cx);
                }
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
                Event::Open(MaybeNavigationTarget::PathLike(_)) => {}
            },
        );

        Self {
            scroll_handle: TerminalScrollHandle::new(terminal.read(cx)),
            terminal,
            focus_handle,
            cursor_shape: TerminalSettings::get_global(cx).cursor_shape,
            blink_manager,
            blinking_terminal_enabled: false,
            has_bell: false,
            custom_title: None,
            hover: None,
            mode: TerminalMode::Standalone,
            scroll_top: Pixels::ZERO,
            ime_state: None,
            context_menu: None,
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
        self.ime_state = (!text.is_empty()).then_some(ImeState { marked_text: text });
        cx.notify();
    }

    pub(crate) fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        self.ime_state = None;
        cx.notify();
    }

    pub(crate) fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.terminal
                .update(cx, |terminal, _| terminal.input(text.as_bytes().to_vec()));
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
        if !focused || self.terminal.read(cx).last_content.mode.contains(Modes::ALT_SCREEN) {
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

    fn process_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        self.terminal.update(cx, |terminal, cx| {
            terminal.try_keystroke(keystroke, TerminalSettings::get_global(cx).option_as_meta)
        })
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.has_bell = false;
        self.blink_manager.update(cx, BlinkManager::pause);
        if self.process_keystroke(&event.keystroke, cx) {
            cx.stop_propagation();
        }
    }

    fn dispatch_context(&self, cx: &App) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add("Terminal");
        let mode = self.terminal.read(cx).last_content.mode;
        context.set("screen", if mode.contains(Modes::ALT_SCREEN) { "alt" } else { "normal" });
        if self.terminal.read(cx).last_content.selection.is_some() {
            context.add("selection");
        }
        context
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.copy(None));
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else { return };
        match clipboard.entries().first() {
            Some(ClipboardEntry::Image(image)) if !image.bytes.is_empty() => {
                self.terminal.update(cx, |terminal, _| terminal.input(vec![0x16]));
            }
            _ => {
                if let Some(text) = clipboard.text() {
                    self.terminal.update(cx, |terminal, _| terminal.paste(&text));
                }
            }
        }
    }

    fn paste_text(&mut self, _: &PasteText, window: &mut Window, cx: &mut Context<Self>) {
        self.paste(&Paste, window, cx);
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_top = px(0.);
        self.terminal.update(cx, |terminal, _| terminal.clear());
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.select_all());
        cx.notify();
    }

    fn deploy_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = ContextMenu::build(window, cx, |menu, _, _| {
            menu.context(self.focus_handle.clone())
                .action("Copy", Box::new(Copy))
                .action("Paste", Box::new(Paste))
                .action("Paste Text", Box::new(PasteText))
                .action("Select All", Box::new(SelectAll))
                .separator()
                .action("Clear", Box::new(Clear))
        });
        window.focus(&menu.focus_handle(cx), cx);
        let subscription = cx.subscribe_in(
            &menu,
            window,
            |view, _, _: &DismissEvent, window, cx| {
                view.context_menu.take();
                view.focus_handle.focus(window, cx);
                cx.notify();
            },
        );
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
        self.terminal.update(cx, |terminal, _| terminal.scroll_line_up());
        cx.notify();
    }

    fn scroll_line_down(&mut self, _: &ScrollLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.scroll_line_down());
        cx.notify();
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.scroll_page_up());
        cx.notify();
    }

    fn scroll_page_down(&mut self, _: &ScrollPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.scroll_page_down());
        cx.notify();
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.scroll_to_top());
        cx.notify();
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.scroll_to_bottom());
        cx.notify();
    }

    fn toggle_vi_mode(&mut self, _: &ToggleViMode, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| terminal.toggle_vi_mode());
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
        self.terminal
            .update(cx, |terminal, _| terminal.input(text.0.clone().into_bytes()));
    }

    fn send_keystroke(&mut self, key: &SendKeystroke, _: &mut Window, cx: &mut Context<Self>) {
        if let Ok(keystroke) = Keystroke::parse(&key.0) {
            self.process_keystroke(&keystroke, cx);
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
        let owns_transient_focus = focused || self.has_open_context_menu();
        div()
            .id("terminal-view")
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .key_context(self.dispatch_context(cx))
            .on_action(cx.listener(Self::send_text))
            .on_action(cx.listener(Self::send_keystroke))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::paste_text))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::scroll_line_up))
            .on_action(cx.listener(Self::scroll_line_down))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_action(cx.listener(Self::toggle_vi_mode))
            .on_action(cx.listener(Self::show_character_palette))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if !this.terminal.read(cx).mouse_mode(event.modifiers.shift) {
                        this.terminal.update(cx, |terminal, _| {
                            terminal.select_word_at_event_position(event)
                        });
                        this.deploy_context_menu(event.position, window, cx);
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .id("terminal-view-container")
                    .size_full()
                    .bg(cx.theme().colors().editor_background)
                    .child(TerminalElement::new(
                        self.terminal.clone(),
                        cx.entity(),
                        self.focus_handle.clone(),
                        focused,
                        self.should_show_cursor(focused, cx),
                        None,
                        self.mode.clone(),
                    ))
                    .when(owns_transient_focus, |container| {
                        container.custom_scrollbars(
                            Scrollbars::for_settings::<TerminalScrollbarSettings>()
                                .show_along(ScrollAxes::Vertical)
                                .style(ScrollbarStyle::Editor)
                                .with_stable_track_along(
                                    ScrollAxes::Vertical,
                                    cx.theme().colors().editor_background,
                                )
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
    }
}
