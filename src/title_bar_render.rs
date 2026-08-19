use super::*;
use crate::rename::resolve_tab_title;

pub(crate) const TAB_MIN_WIDTH: Pixels = px(80.);
pub(crate) const TAB_MAX_WIDTH: Pixels = px(180.);
// Four pinned-tab indicators (keep-running pin, silent mode, attention, and
// the tab icon), their gaps, and the tab's horizontal padding fit in this
// fixed slot. A pinned tab never participates in unpinned overflow.
pub(crate) const PINNED_TAB_WIDTH: Pixels = px(76.);
// Matches the new-tab button's own footprint (ml_1 + a 32px button + mr_2), so a
// trigger reserved at the edge of the tab bar takes up the same visual space.
pub(crate) const TAB_OVERFLOW_TRIGGER_WIDTH: Pixels = px(44.);
pub(crate) const TITLE_BAR_CONTROL_LABEL_MIN_WIDTH: Pixels = px(720.);
pub(crate) const TITLE_BAR_RECONNECT_LABEL_MIN_WIDTH: Pixels = px(800.);
// Compact mode always keeps at least this much of the tab bar draggable, even
// when tabs grow to fill the rest of it, so the window stays movable from there.
pub(crate) const COMPACT_DRAG_AREA_MIN_WIDTH: Pixels = px(60.);
// One Large button (32px) plus the gap_1 (4px) — see the
// `title-bar-controls` reserve this backs on macOS.
pub(crate) const COMPACT_LEADING_CONTROLS_RESERVE: Pixels = px(36.);

/// Whether macOS-specific title-bar space should still be reserved. The
/// fullscreen flag passed here is already qualified to macOS by the caller,
/// so non-macOS platforms keep their existing layout.
pub(crate) fn macos_title_bar_reservations_enabled(is_macos_fullscreen: bool) -> bool {
    cfg!(target_os = "macos") && !is_macos_fullscreen
}

pub(crate) fn compact_leading_controls_reservation_enabled(
    compact_mode: bool,
    is_macos_fullscreen: bool,
) -> bool {
    compact_mode && macos_title_bar_reservations_enabled(is_macos_fullscreen)
}

pub(crate) fn compact_drag_area_visible(compact_mode: bool, is_macos_fullscreen: bool) -> bool {
    compact_mode && !is_macos_fullscreen
}

pub(crate) fn compact_drag_area_reserve_width(show_compact_drag_area: bool) -> Pixels {
    if show_compact_drag_area {
        COMPACT_DRAG_AREA_MIN_WIDTH
    } else {
        px(0.)
    }
}

/// The tab-bar row's height outside compact mode, where the bar is its own row
/// beneath the title bar.
const TAB_BAR_ROW_HEIGHT: gpui::Rems = gpui::Rems(2.);

/// The tab bar's row height: `compact_height` (the platform title bar's own
/// height) in compact mode, since the tab bar shares that row there, or the
/// standard [`TAB_BAR_ROW_HEIGHT`] otherwise. Shared by every element that
/// lines up with the row — the tabs themselves, the overflow triggers, the
/// new-tab button, and the bar's own container — so the compact/regular swap is
/// only written once.
pub(crate) fn tab_bar_row_height(compact_mode: bool, compact_height: Pixels) -> gpui::Div {
    div()
        .h(TAB_BAR_ROW_HEIGHT)
        .when(compact_mode, |el| el.h(compact_height))
}

/// The height of the whole top-of-window chrome: the title bar, plus the
/// tab-bar row beneath it outside compact mode (in compact mode the tab bar
/// shares the title bar's row, so the title bar's height is the whole of it).
///
/// `Zetta::render` needs this as a number rather than letting layout work it
/// out, because the chrome renders inside a cached view and a cached view is
/// laid out from the style the composer supplies rather than measured from its
/// contents. The chrome's own root is `h_full`, so it always fills whatever
/// this returns; getting it wrong moves the panes rather than silently
/// corrupting the cache, which is what
/// `title_bar_chrome_height_matches_the_rows_it_covers` checks.
///
/// The tab-bar row's borders are deliberately *not* added: layout is border-box,
/// so `border_t_1` and `border_b_1` sit inside [`TAB_BAR_ROW_HEIGHT`]. Adding
/// them pushed every pane down by two pixels.
pub(crate) fn title_bar_chrome_height(
    compact_mode: bool,
    title_bar_height: Pixels,
    rem_size: Pixels,
) -> Pixels {
    if compact_mode {
        title_bar_height
    } else {
        title_bar_height + TAB_BAR_ROW_HEIGHT.to_pixels(rem_size)
    }
}

// Keep the responsive sizing on a native flex item. Elements such as
// `right_click_menu` are custom layout elements and cannot receive the flex
// constraints that control how tabs shrink inside the scroll container.
pub(crate) fn responsive_tab_container(
    child: impl IntoElement + 'static,
    compact_mode: bool,
    compact_height: Pixels,
    is_renaming: bool,
) -> gpui::Div {
    // In compact mode, tabs grow to fill any width left over after the visible
    // tabs, the new-tab button, and the drag strip — otherwise that space would
    // just sit empty. They still cap out at the same TAB_MAX_WIDTH as normal
    // mode, so leftover space beyond that goes to the drag strip instead of
    // making tabs balloon wider than usual. A renamed tab keeps its fixed,
    // non-growing width so the editable title doesn't reflow mid-edit.
    let grows_to_fill = compact_mode && !is_renaming;
    tab_bar_row_height(compact_mode, compact_height)
        .w(TAB_MAX_WIDTH)
        .min_w(if is_renaming {
            TAB_MAX_WIDTH
        } else {
            TAB_MIN_WIDTH
        })
        .max_w(TAB_MAX_WIDTH)
        .when(grows_to_fill, |container| container.flex_grow(1.))
        .flex_shrink(if is_renaming { 0. } else { 1. })
        .child(child)
}

pub(crate) fn pinned_tab_container(
    child: impl IntoElement + 'static,
    compact_mode: bool,
    compact_height: Pixels,
    is_renaming: bool,
) -> gpui::Div {
    let width = if is_renaming {
        TAB_MAX_WIDTH
    } else {
        PINNED_TAB_WIDTH
    };
    tab_bar_row_height(compact_mode, compact_height)
        .w(width)
        .min_w(width)
        .max_w(width)
        .flex_none()
        .child(child)
}

/// Whether the tab bar no longer has room to give every tab (plus, while a
/// rename is in progress, the extra full-width room that tab needs) its max
/// width — used to hide tab icons before labels start getting clipped.
pub(crate) fn tab_bar_tabs_are_shrinking(
    available_width: Pixels,
    is_renaming: bool,
    tab_count: usize,
) -> bool {
    let needed = TAB_MAX_WIDTH * (tab_count + is_renaming as usize) + TAB_OVERFLOW_TRIGGER_WIDTH;
    available_width < needed
}

/// Which tabs fit in the tab bar without needing to overflow into a left/right
/// dropdown menu. The selected tab is always included in the returned range:
/// - `overflow_selection` is `Some(false)`/`Some(true)` right after the user picks
///   a tab from the left/right overflow menu, anchoring it at the edge it slid in
///   from instead of jumping to wherever the default placement would put it.
/// - Otherwise (plain clicks, keyboard cycling) the range keeps the selected tab
///   visible with the least possible movement.
///
/// Capacity depends only on `is_renaming`, not on compact mode: per
/// `responsive_tab_container`, a renaming tab reserves the same full
/// `TAB_MAX_WIDTH` and every other tab can shrink to the same `TAB_MIN_WIDTH`
/// in both the compact and regular tab bars.
pub(crate) fn tab_bar_visible_tab_range(
    available_width: Pixels,
    tab_count: usize,
    selected_index: usize,
    is_renaming: bool,
    overflow_selection: Option<bool>,
) -> std::ops::Range<usize> {
    tab_bar_visible_tab_range_with_empty_capacity(
        available_width,
        tab_count,
        selected_index,
        is_renaming,
        overflow_selection,
        false,
    )
}

pub(crate) fn tab_bar_visible_tab_range_with_pinned_tabs(
    available_width: Pixels,
    tab_count: usize,
    selected_index: usize,
    is_renaming: bool,
    overflow_selection: Option<bool>,
    pinned_tabs_present: bool,
) -> std::ops::Range<usize> {
    if !pinned_tabs_present {
        return tab_bar_visible_tab_range(
            available_width,
            tab_count,
            selected_index,
            is_renaming,
            overflow_selection,
        );
    }
    tab_bar_visible_tab_range_with_empty_capacity(
        available_width,
        tab_count,
        selected_index,
        is_renaming,
        overflow_selection,
        pinned_tabs_present,
    )
}

fn tab_bar_visible_tab_range_with_empty_capacity(
    available_width: Pixels,
    tab_count: usize,
    selected_index: usize,
    is_renaming: bool,
    overflow_selection: Option<bool>,
    allow_empty_capacity: bool,
) -> std::ops::Range<usize> {
    if tab_count == 0 {
        return 0..0;
    }

    let effective_width = (available_width - TAB_OVERFLOW_TRIGGER_WIDTH).max(px(0.));
    let capacity = if is_renaming {
        let remaining = (effective_width - (TAB_MAX_WIDTH - TAB_MIN_WIDTH)).max(px(0.));
        (remaining / TAB_MIN_WIDTH).floor() as usize
    } else {
        (effective_width / TAB_MIN_WIDTH).floor() as usize
    };
    let capacity = if allow_empty_capacity {
        capacity.min(tab_count)
    } else {
        capacity.clamp(1, tab_count)
    };
    if capacity == 0 {
        return 0..0;
    }

    let selected_index = selected_index.min(tab_count - 1);
    let max_start = tab_count - capacity;
    let start = match overflow_selection {
        Some(false) => selected_index.min(max_start),
        _ => selected_index.saturating_sub(capacity - 1).min(max_start),
    };
    start..start + capacity
}

pub(crate) fn title_bar_shows_control_labels(
    viewport_width: Pixels,
    has_reconnect_control: bool,
    hide_labels: bool,
    compact_mode: bool,
) -> bool {
    !compact_mode
        && !hide_labels
        && viewport_width
            >= if has_reconnect_control {
                TITLE_BAR_RECONNECT_LABEL_MIN_WIDTH
            } else {
                TITLE_BAR_CONTROL_LABEL_MIN_WIDTH
            }
}

pub(crate) fn title_bar_buttons_visible(compact_mode: bool, hide_buttons: bool) -> bool {
    !compact_mode && !hide_buttons
}

pub(crate) fn title_bar_broadcast_visible(hide_buttons: bool) -> bool {
    !hide_buttons
}

pub(crate) fn title_bar_silent_visible(hide_buttons: bool) -> bool {
    !hide_buttons
}

pub(crate) fn title_bar_silent_icon(silent_mode: bool) -> IconName {
    if silent_mode {
        IconName::BellOff
    } else {
        IconName::Bell
    }
}

pub(crate) fn title_bar_background_indicator_on_right(
    compact_mode: bool,
    hide_buttons: bool,
    background_session_count: usize,
) -> bool {
    (compact_mode || hide_buttons) && background_session_count > 0
}

pub(crate) fn title_bar_pane_size_visible(compact_mode: bool, hide_pane_size: bool) -> bool {
    !compact_mode && !hide_pane_size
}

pub(crate) fn title_bar_app_label_visible(compact_mode: bool) -> bool {
    !compact_mode
}

pub(crate) fn title_bar_menus_visible(hide_menus: bool) -> bool {
    cfg!(not(target_os = "macos")) || !hide_menus
}

pub(crate) fn reconnect_control_label(show_label: bool) -> &'static str {
    if show_label { "Reconnect" } else { "" }
}

/// The label shown for a tab that has overflowed into a left/right dropdown menu.
/// Unlike the in-bar tab title, this never needs the rename-in-progress branch:
/// `tab_bar_visible_tab_range` always keeps the tab being renamed in the visible
/// range, so a hidden tab can never be the one currently being renamed.
pub(crate) fn tab_overflow_entry_label(tab: &Tab, cx: &App) -> SharedString {
    resolve_tab_title(tab, || {
        if let Some(view) = tab.active_view() {
            view.read(cx).tab_content_text(1, cx)
        } else {
            tab.active_pane()
                .map(|pane| pane.profile.name.clone())
                .unwrap_or_else(|| "Terminal".to_string())
                .into()
        }
    })
}

pub(crate) fn render_tab_overflow_trigger(
    is_right: bool,
    entries: Vec<(usize, SharedString)>,
    compact_mode: bool,
    compact_height: Pixels,
    border_color: Hsla,
    menu_handle: PopoverMenuHandle<ui::ContextMenu>,
    zetta_handle: WeakEntity<Zetta>,
) -> AnyElement {
    let icon = if is_right {
        IconName::ChevronRight
    } else {
        IconName::ChevronLeft
    };
    let count = entries.len();
    let tooltip_label: SharedString = if is_right {
        format!("{count} more tabs to the right")
    } else {
        format!("{count} more tabs to the left")
    }
    .into();

    tab_bar_row_height(compact_mode, compact_height)
        .flex_none()
        .flex()
        .items_center()
        // The tabs each draw their own divider on their right edge, so the left
        // overflow trigger needs its own to keep it from looking fused to the
        // first visible tab; the right trigger already inherits one from the
        // last visible tab's right-edge divider.
        .when(!is_right, |el| el.border_r_1().border_color(border_color))
        .child(
            PopoverMenu::new(("tab-overflow-menu", is_right as usize))
                .with_handle(menu_handle)
                .trigger_with_tooltip(
                    IconButton::new(("tab-overflow-trigger", is_right as usize), icon)
                        .size(ButtonSize::Large)
                        .icon_size(IconSize::Small)
                        .aria_label(tooltip_label.clone()),
                    Tooltip::text(tooltip_label.clone()),
                )
                .anchor(if is_right {
                    Anchor::TopRight
                } else {
                    Anchor::TopLeft
                })
                .menu(move |window, cx| {
                    let entries = entries.clone();
                    let dismiss_handle = zetta_handle.clone();
                    zetta_handle
                        .update(cx, |this, cx| {
                            this.tab_overflow_keyboard_menu_edge = Some(is_right);
                            cx.notify();
                        })
                        .ok();
                    let menu = ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                        for (index, label) in entries.iter().cloned() {
                            menu = menu.action(label, Box::new(SelectOverflowTab { index }));
                        }
                        menu
                    });
                    // Register before PopoverMenu's dismissal listener, matching the
                    // other tab-bar menus, so a menu reached through keyboard
                    // navigation cannot restore focus to the menu it replaced.
                    window
                        .subscribe(&menu, cx, move |menu, _: &DismissEvent, window, cx| {
                            if menu.focus_handle(cx).is_focused(window) {
                                dismiss_handle
                                    .update(cx, |this, cx| {
                                        this.tab_overflow_keyboard_menu_edge = None;
                                        this.focus_active(window, cx);
                                    })
                                    .ok();
                            }
                        })
                        .detach();
                    Some(menu)
                }),
        )
        .into_any_element()
}

fn compact_tab_neighbor_control_mask(background: Hsla) -> gpui::Div {
    div().absolute().inset_0().rounded_sm().bg(background)
}

fn render_compact_tab_neighbor_control(
    child: impl IntoElement + 'static,
    background: Hsla,
) -> AnyElement {
    // The compact tab row paints after the leading controls and its selected
    // tab deliberately extends into its neighbors. Defer the adjacent control
    // so its background and contents are restored over that tab wing.
    deferred(
        div()
            .relative()
            .size(px(32.))
            .child(compact_tab_neighbor_control_mask(background))
            .child(child),
    )
    .with_priority(crate::app_render::TITLE_BAR_CONTROL_PAINT_PRIORITY)
    .into_any_element()
}

pub(crate) fn render_new_tab_button(compact_mode: bool, compact_height: Pixels) -> AnyElement {
    new_tab_button_container(compact_mode, compact_height)
        .child(
            IconButton::new("new-tab", IconName::Plus)
                .shape(IconButtonShape::Wide)
                .size(ButtonSize::Large)
                .width(px(32.))
                .icon_size(IconSize::Small)
                .aria_label("New tab")
                .tooltip(move |_window, cx| Tooltip::for_action("New tab", &NewTab, cx))
                .on_click(|_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(NewTab), cx)
                }),
        )
        .into_any_element()
}

fn new_tab_button_container(compact_mode: bool, compact_height: Pixels) -> gpui::Div {
    tab_bar_row_height(compact_mode, compact_height)
        .when(compact_mode, |button| button.ml(px(0.)).mr(px(12.)))
        .when(!compact_mode, |button| button.ml_1().mr_2())
        .flex_none()
        .flex()
        .items_center()
}

/// Compact mode places the tab bar inside the title bar. Keep a portion of it
/// available for moving the window without making tab hitboxes draggable too.
/// This has to follow the tabs and new-tab button (inside the same measured
/// row) rather than sit outside it, so it only claims the width genuinely left
/// over after them instead of pushing them apart from the rest of the bar.
pub(crate) fn render_compact_drag_area(
    compact_height: Pixels,
    zetta_handle: WeakEntity<Zetta>,
) -> AnyElement {
    let down_handle = zetta_handle.clone();
    let up_handle = zetta_handle.clone();
    let out_handle = zetta_handle.clone();
    div()
        .id("compact-title-bar-drag-area")
        .h(compact_height)
        .flex_1()
        .min_w(COMPACT_DRAG_AREA_MIN_WIDTH)
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            down_handle
                .update(cx, |this, cx| {
                    this.titlebar_dragging = true;
                    this.focus_active(window, cx);
                })
                .ok();
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            up_handle
                .update(cx, |this, cx| {
                    this.titlebar_dragging = false;
                    this.focus_active(window, cx);
                })
                .ok();
        })
        .on_mouse_down_out(move |_, _, cx| {
            out_handle
                .update(cx, |this, _| this.titlebar_dragging = false)
                .ok();
        })
        .on_mouse_move(move |_, window, cx| {
            zetta_handle
                .update(cx, |this, _cx| {
                    if this.titlebar_dragging {
                        this.titlebar_dragging = false;
                        window.start_window_move();
                    }
                })
                .ok();
        })
        .into_any_element()
}

#[derive(Clone)]
pub(crate) enum ProfileMenuShortcut {
    #[cfg(not(target_os = "macos"))]
    Alias(String),
    Binding(ui::KeyBinding),
}

impl ProfileMenuShortcut {
    pub(crate) fn render(&self) -> AnyElement {
        match self {
            #[cfg(not(target_os = "macos"))]
            Self::Alias(label) => Label::new(label.clone())
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element(),
            Self::Binding(binding) => binding.clone().into_any_element(),
        }
    }
}

pub(crate) fn profile_menu_shortcut(
    slot: usize,
    terminal_focus: Option<&gpui::FocusHandle>,
    window: &Window,
    keyboard_mapper: &dyn PlatformKeyboardMapper,
) -> Option<ProfileMenuShortcut> {
    let action = OpenProfile { slot };
    let binding = terminal_focus
        .and_then(|focus| window.highest_precedence_binding_for_action_in(&action, focus))
        .or_else(|| window.highest_precedence_binding_for_action(&action));
    let binding = binding?;

    if slot <= PROFILE_SHORTCUT_KEYS.len() {
        let expected_binding = profile_keybindings(slot, keyboard_mapper)[0].clone();
        if let Some(label) = profile_shortcut_label(slot, &binding, &expected_binding) {
            return Some(profile_shortcut_alias(slot, label));
        }
    }

    Some(ProfileMenuShortcut::Binding(
        ui::KeyBinding::from_keystrokes(binding.keystrokes().to_vec().into(), false),
    ))
}

#[cfg(target_os = "macos")]
pub(crate) fn profile_shortcut_alias_keystroke(slot: usize) -> gpui::KeybindingKeystroke {
    let key = PROFILE_SHORTCUT_KEYS[slot - 1];
    let keystroke = gpui::Keystroke::parse(&format!("ctrl-shift-{key}"))
        .expect("profile shortcut alias must be a valid keystroke");
    gpui::KeybindingKeystroke::from_keystroke(keystroke)
}

#[cfg(target_os = "macos")]
pub(crate) fn profile_shortcut_alias(slot: usize, _label: String) -> ProfileMenuShortcut {
    ProfileMenuShortcut::Binding(ui::KeyBinding::from_keystrokes(
        vec![profile_shortcut_alias_keystroke(slot)].into(),
        false,
    ))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn profile_shortcut_alias(_slot: usize, label: String) -> ProfileMenuShortcut {
    ProfileMenuShortcut::Alias(label)
}

/// Returns focus to the active pane when `menu` is dismissed while focused.
///
/// Registered before `PopoverMenu`'s own dismissal listener so a menu reached
/// through left/right navigation cannot restore focus to the menu it replaced.
fn restore_focus_on_dismiss(
    menu: &Entity<ui::ContextMenu>,
    handle: WeakEntity<Zetta>,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .subscribe(menu, cx, move |menu, _: &DismissEvent, window, cx| {
            if menu.focus_handle(cx).is_focused(window) {
                handle
                    .update(cx, |this, cx| this.focus_active(window, cx))
                    .ok();
            }
        })
        .detach();
}

/// The title bar plus, outside compact mode, the tab bar that renders as its
/// own row underneath. In compact mode the tab bar is folded into the title bar
/// and `tab_bar` is `None`.
pub(crate) struct TitleBarChrome {
    pub(crate) title_bar: AnyElement,
    pub(crate) tab_bar: Option<AnyElement>,
}

impl Zetta {
    /// Assembles the whole top-of-window chrome: window controls, tab bar,
    /// title-bar menus, and the reconnect/broadcast controls.
    pub(crate) fn render_title_bar_chrome(
        &self,
        frame: &WindowFrameGeometry,
        colors: &ThemeColors,
        handle: &WeakEntity<Self>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> TitleBarChrome {
        let compact_mode = self.launch_config.compact_mode;
        let is_macos_fullscreen = cfg!(target_os = "macos") && window.is_fullscreen();
        let title_bar_height = frame.title_bar_height;
        let active_tab = self.tabs.get(self.active_tab);
        let broadcast_input = active_tab.is_some_and(|tab| tab.broadcast_input);
        let silent_mode_state = if cx.has_global::<ZettaProcessState>() {
            cx.global::<ZettaProcessState>().silent_mode
        } else {
            SilentModeState::default()
        };
        let focus_status_access = silent_mode_state.focus_status_access();
        let (auto_background_tab, auto_background_protected) = active_tab
            .map(|tab| match &tab.close_policy {
                TabClosePolicy::Background { authentication } => (true, authentication.is_some()),
                TabClosePolicy::Close => (false, false),
            })
            .unwrap_or_default();
        let background_sessions = self.process_background_session_picker_entries(cx);
        let background_session_count = background_sessions.len();
        let active_pane_size =
            title_bar_pane_size_visible(compact_mode, self.launch_config.hide_pane_size)
                .then(|| {
                    active_tab.and_then(Tab::active_terminal).map(|terminal| {
                        let bounds = terminal.read(cx).last_content().terminal_bounds;
                        terminal_size_label(bounds.num_columns(), bounds.num_lines())
                    })
                })
                .flatten();
        let active_terminal_focus = self.active_terminal_focus(cx);

        let left_window_controls = render_window_controls(
            self.button_layout.left,
            frame.window_control_state,
            false,
            colors,
        );
        let right_window_controls = render_window_controls(
            self.button_layout.right,
            frame.window_control_state,
            true,
            colors,
        );
        let title_bar_background = if cfg!(linux_like) && !window.is_window_active() {
            colors.title_bar_inactive_background
        } else {
            colors.title_bar_background
        };

        let tab_bar = self
            .render_tab_bar(
                TabBarChrome {
                    handle: handle.clone(),
                    compact_mode,
                    title_bar_height,
                    is_macos_fullscreen,
                    rounded_top_right: frame.rounded_top_right,
                    compact_tab_top_left: frame.compact_tab_top_left,
                    compact_tab_top_right: frame.compact_tab_top_right,
                    compact_tab_bottom_left: frame.compact_tab_bottom_left,
                    compact_tab_bottom_right: frame.compact_tab_bottom_right,
                    corner_radius: frame.corner_radius,
                    tab_bar_background: colors.tab_bar_background,
                    tab_close_button_on_left: window_close_button_on_left(self.button_layout),
                    is_renaming_tab: self.is_renaming(),
                    tab_count: self.tabs.len(),
                    pinned_tab_count: pinned_tab_count(&self.tabs),
                    is_renaming_pinned: self.is_renaming()
                        && active_tab.is_some_and(|tab| tab.pinned && tab.renaming_pane.is_none()),
                    selected_tab_index: self.active_tab,
                    tab_move_mode_active: self.tab_move_mode,
                    overflow_selection: self.tab_overflow_selection_side,
                    border_color: colors.border,
                    left_menu_handle: self.tab_overflow_left_menu_handle.clone(),
                    right_menu_handle: self.tab_overflow_right_menu_handle.clone(),
                },
                colors,
            )
            .into_any_element();
        let (compact_tab_bar, regular_tab_bar) = if compact_mode {
            (Some(tab_bar), None)
        } else {
            (None, Some(tab_bar))
        };

        let show_title_bar_control_labels = title_bar_shows_control_labels(
            window.viewport_size().width,
            background_session_count > 0,
            self.launch_config.hide_title_bar_labels,
            compact_mode,
        );
        let show_title_bar_buttons =
            title_bar_buttons_visible(compact_mode, self.launch_config.hide_title_bar_buttons);
        let reconnect_control =
            (show_title_bar_buttons && background_session_count > 0).then(|| {
                self.render_reconnect_control(
                    show_title_bar_control_labels,
                    &background_sessions,
                    handle,
                )
            });
        let right_reconnect_control = title_bar_background_indicator_on_right(
            compact_mode,
            self.launch_config.hide_title_bar_buttons,
            background_session_count,
        )
        .then(|| {
            // This control is outside the regular controls row, so it must
            // block the draggable title-bar hitbox on platforms that use
            // native hit testing for client-side decorations.
            div()
                .occlude()
                .child(self.render_reconnect_control(false, &background_sessions, handle))
                .into_any_element()
        });

        // Keep a reconnect control immediately next to the native/client window controls. The
        // group owns the trailing auto-margin so the two controls cannot be separated by free
        // space when the title-bar controls are hidden or compact mode is enabled.
        let right_title_bar_controls = h_flex()
            .id("title-bar-right-controls")
            .h_full()
            .flex_none()
            .ml_auto()
            .when_some(right_reconnect_control, |controls, reconnect_control| {
                controls.child(reconnect_control)
            })
            .when(title_bar_app_label_visible(compact_mode), |controls| {
                controls.child(
                    div().flex_none().px_2().child(
                        Label::new("Zetta")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                )
            })
            .child(right_window_controls)
            .into_any_element();

        let title_bar = self.render_title_bar(
            title_bar_height,
            title_bar_background,
            frame.rounded_top_left,
            frame.rounded_top_right,
            frame.corner_radius,
            left_window_controls,
            compact_mode,
            is_macos_fullscreen,
            title_bar_menus_visible(self.launch_config.hide_title_bar_menus),
            self.render_application_menu(
                show_title_bar_control_labels,
                handle,
                active_terminal_focus.clone(),
            ),
            self.render_profile_menu(
                show_title_bar_control_labels,
                handle,
                active_terminal_focus,
                cx,
            ),
            show_title_bar_buttons,
            show_title_bar_control_labels,
            auto_background_tab,
            auto_background_protected,
            reconnect_control,
            title_bar_broadcast_visible(self.launch_config.hide_title_bar_buttons),
            broadcast_input,
            title_bar_silent_visible(self.launch_config.hide_title_bar_buttons),
            silent_mode_state.effective(),
            silent_mode_state.system_active(),
            focus_status_access,
            compact_tab_bar,
            active_pane_size,
            right_title_bar_controls,
            cx,
        );

        TitleBarChrome {
            title_bar,
            tab_bar: regular_tab_bar,
        }
    }

    /// The "Profile" menu: one entry per visible profile, each opening a new tab.
    fn render_profile_menu(
        &self,
        show_label: bool,
        handle: &WeakEntity<Self>,
        active_terminal_focus: Option<gpui::FocusHandle>,
        cx: &App,
    ) -> AnyElement {
        let profiles = self.profiles.clone();
        let hidden_profiles = self.effective_config().hidden_profiles.clone();
        let default_profile = self.effective_config().default_profile;
        let menu_handle = handle.clone();
        let dismiss_handle = handle.clone();
        let keyboard_mapper = cx.keyboard_mapper().clone();
        PopoverMenu::new("new-tab-profile-menu")
            .with_handle(self.profile_menu_handle.clone())
            .trigger_with_tooltip(
                Button::new(
                    "new-tab-profile-menu-trigger",
                    if show_label { "Profile" } else { "" },
                )
                .start_icon(Icon::new(IconName::ChevronDown).size(IconSize::Small))
                .style(ButtonStyle::Subtle)
                .size(ButtonSize::Large)
                .aria_label("New tab profile"),
                Tooltip::text("New tab profile"),
            )
            .anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                let profiles = profiles.clone();
                let hidden_profiles = hidden_profiles.clone();
                let handle = menu_handle.clone();
                let dismiss_handle = dismiss_handle.clone();
                let terminal_focus = active_terminal_focus.clone();
                let keyboard_mapper = keyboard_mapper.clone();
                let menu = ui::ContextMenu::build(window, cx, move |mut menu, window, _| {
                    for (visible_index, (index, profile)) in profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, profile)| !profile_is_hidden(profile, &hidden_profiles))
                        .enumerate()
                    {
                        let is_default = index == default_profile;
                        let label_for_row = profile.name.clone();
                        let profile_icon = profile.icon.clone();
                        let shortcut = profile_menu_shortcut(
                            visible_index + 1,
                            terminal_focus.as_ref(),
                            window,
                            keyboard_mapper.as_ref(),
                        );
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
                                            .child(profile_icon.render(IconSize::Small))
                                            .child(Label::new(label_for_row.clone()).color(
                                                if is_default {
                                                    Color::Accent
                                                } else {
                                                    Color::Default
                                                },
                                            )),
                                    )
                                    .when_some(shortcut.clone(), |row, shortcut| {
                                        row.child(shortcut.render())
                                    })
                                    .into_any_element()
                            },
                            move |window, cx| {
                                handle
                                    .update(cx, |this, cx| {
                                        let profile = this.profiles[index].clone();
                                        this.open_tab_with_profile(profile, window, cx);
                                    })
                                    .ok();
                            },
                        );
                    }
                    menu
                });
                restore_focus_on_dismiss(&menu, dismiss_handle, window, cx);
                Some(menu)
            })
            .into_any_element()
    }

    /// The "Menu" button's application menu.
    fn render_application_menu(
        &self,
        show_label: bool,
        handle: &WeakEntity<Self>,
        // The popover receives focus while it is open. Retain the active
        // terminal's context so actions continue to resolve their shortcuts
        // when the user cycles here from the Profile menu.
        action_context: Option<gpui::FocusHandle>,
    ) -> AnyElement {
        let dismiss_handle = handle.clone();
        PopoverMenu::new("application-menu")
            .with_handle(self.application_menu_handle.clone())
            .trigger_with_tooltip(
                Button::new(
                    "application-menu-trigger",
                    if show_label { "Menu" } else { "" },
                )
                .start_icon(Icon::new(IconName::Menu).size(IconSize::Small))
                .style(ButtonStyle::Subtle)
                .size(ButtonSize::Large)
                .aria_label("Application menu"),
                Tooltip::for_action_title("Open application menu", &OpenApplicationMenu),
            )
            .anchor(Anchor::TopLeft)
            .menu(move |window, cx| {
                let dismiss_handle = dismiss_handle.clone();
                let action_context = action_context.clone();
                let menu = ui::ContextMenu::build(window, cx, move |menu, _, _| {
                    let menu =
                        menu.when_some(action_context.clone(), |menu, focus| menu.context(focus));
                    menu.action("New Tab", Box::new(NewTab))
                        .action("New Window", Box::new(NewWindow))
                        .separator()
                        .action("Command Palette", Box::new(ToggleCommandPalette))
                        .action("Open Settings", Box::new(ToggleSettings))
                        .action("Open Themes", Box::new(OpenThemes))
                        .action("Open Keymap", Box::new(OpenKeymap))
                        .action("Open Templates", Box::new(OpenTemplates))
                        .action("Open Projects", Box::new(OpenProjects))
                        .separator()
                        .action("Close Tab", Box::new(CloseTab))
                        .action("Close Window", Box::new(CloseWindow))
                        .action("Close All Windows", Box::new(CloseAllWindows))
                });
                restore_focus_on_dismiss(&menu, dismiss_handle, window, cx);
                Some(menu)
            })
            .into_any_element()
    }

    /// The background-session reconnect control: a plain button for a single
    /// session, a menu of sessions when more than one can be reconnected.
    fn render_reconnect_control(
        &self,
        show_label: bool,
        sessions: &Arc<[ProcessBackgroundSessionEntry]>,
        handle: &WeakEntity<Self>,
    ) -> AnyElement {
        let session_count = sessions.len();
        if session_count == 1 {
            return Button::new("reconnect-session", reconnect_control_label(show_label))
                .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                .style(ButtonStyle::Subtle)
                .size(ButtonSize::Large)
                .aria_label("Reconnect background session")
                .tooltip(Tooltip::for_action_title(
                    "Reconnect background session",
                    &ReconnectSession,
                ))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ReconnectSession), cx))
                .into_any_element();
        }

        let entries = sessions.to_vec();
        let menu_handle = handle.clone();
        PopoverMenu::new("reconnect-session-menu")
            .with_handle(self.reconnect_menu_handle.clone())
            .trigger_with_tooltip(
                Button::new("reconnect-session", reconnect_control_label(show_label))
                    .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Large)
                    .aria_label("Choose background session to reconnect"),
                Tooltip::for_action_title(
                    format!("Choose background session to reconnect ({session_count})"),
                    &ReconnectSession,
                ),
            )
            .anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                let entries = entries.clone();
                let menu_handle = menu_handle.clone();
                Some(ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                    for (runner_id, session_id, title, details) in &entries {
                        let runner_id = *runner_id;
                        let session_id = *session_id;
                        let title = title.clone();
                        let details = details.clone();
                        let handle = menu_handle.clone();
                        menu = menu.custom_entry(
                            move |_, _| {
                                v_flex()
                                    .gap_0p5()
                                    .child(Label::new(title.clone()))
                                    .child(
                                        Label::new(details.clone())
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .into_any_element()
                            },
                            move |window, cx| {
                                handle
                                    .update(cx, |this, cx| {
                                        this.reconnect_background_session(
                                            runner_id, session_id, window, cx,
                                        )
                                    })
                                    .ok();
                            },
                        );
                    }
                    menu
                }))
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_title_bar(
        &self,
        title_bar_height: Pixels,
        title_bar_background: Hsla,
        rounded_top_left: bool,
        rounded_top_right: bool,
        corner_radius: Pixels,
        left_window_controls: AnyElement,
        compact_mode: bool,
        is_macos_fullscreen: bool,
        show_title_bar_menus: bool,
        application_menu: AnyElement,
        profile_menu: AnyElement,
        show_title_bar_buttons: bool,
        show_title_bar_control_labels: bool,
        auto_background_tab: bool,
        auto_background_protected: bool,
        reconnect_control: Option<AnyElement>,
        show_broadcast_control: bool,
        broadcast_input: bool,
        show_silent_control: bool,
        silent_mode: bool,
        system_silent: bool,
        focus_status_access: FocusStatusAccess,
        compact_tab_bar: Option<AnyElement>,
        active_pane_size: Option<String>,
        right_title_bar_controls: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let reserve_macos_title_bar_space =
            macos_title_bar_reservations_enabled(is_macos_fullscreen);
        let reserve_compact_leading_controls =
            compact_leading_controls_reservation_enabled(compact_mode, is_macos_fullscreen);
        let profile_menu = if compact_mode {
            render_compact_tab_neighbor_control(profile_menu, title_bar_background)
        } else {
            profile_menu
        };
        let broadcast_control = show_broadcast_control.then(|| {
            let button = Button::new(
                "toggle-broadcast-input",
                if show_title_bar_control_labels {
                    "Broadcast"
                } else {
                    ""
                },
            )
            .start_icon(Icon::new(IconName::Keyboard).size(IconSize::Small).color(
                if broadcast_input {
                    Color::Selected
                } else {
                    Color::Default
                },
            ))
            .style(ButtonStyle::Subtle)
            .size(ButtonSize::Large)
            .toggle_state(broadcast_input)
            .aria_label("Broadcast input to all panes")
            .tooltip(Tooltip::for_action_title(
                if broadcast_input {
                    "Broadcast input is on"
                } else {
                    "Broadcast input to all panes"
                },
                &ToggleBroadcastInput,
            ))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleBroadcastInput), cx));

            if compact_mode {
                render_compact_tab_neighbor_control(button, title_bar_background)
            } else {
                button.into_any_element()
            }
        });
        let silent_control = show_silent_control.then(|| {
            let silent_tooltip_title = if system_silent {
                "Silent mode is controlled by Do Not Disturb"
            } else if cfg!(target_os = "macos") {
                focus_status_access.tooltip()
            } else {
                "Silence terminal bells and notification sounds"
            };
            let silent_tooltip = Tooltip::for_action_title(silent_tooltip_title, &ToggleSilentMode);
            let button = Button::new(
                "toggle-silent-mode",
                if show_title_bar_control_labels {
                    "Silent"
                } else {
                    ""
                },
            )
            .start_icon(
                Icon::new(title_bar_silent_icon(silent_mode))
                    .size(IconSize::Small)
                    .color(if silent_mode {
                        Color::Selected
                    } else {
                        Color::Default
                    }),
            )
            .style(ButtonStyle::Subtle)
            .size(ButtonSize::Large)
            .toggle_state(silent_mode)
            .disabled(system_silent)
            .aria_label(if silent_mode {
                "Silent mode is on"
            } else {
                "Silent mode is off"
            })
            .tooltip(silent_tooltip)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSilentMode), cx));

            if compact_mode {
                render_compact_tab_neighbor_control(button, title_bar_background)
            } else {
                button.into_any_element()
            }
        });

        div()
            .id("zetta-title-bar")
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .h(title_bar_height)
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(title_bar_background)
            .when(rounded_top_left, |title_bar| {
                title_bar.rounded_tl(corner_radius)
            })
            .when(rounded_top_right, |title_bar| {
                title_bar.rounded_tr(corner_radius)
            })
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
                    } else if window.is_resizable() {
                        window.zoom_window();
                    }
                }
            })
            .child(left_window_controls)
            .child(
                h_flex()
                    .id("title-bar-controls")
                    // Keep application controls out of the draggable native title-bar region.
                    .occlude()
                    .min_w_0()
                    // Drop trailing controls before they can overlap the client corner.
                    .flex_shrink_1()
                    .overflow_hidden()
                    .gap_1()
                    // When the reserve below makes this wider than its content, fill it from
                    // the tab-bar side inward, so any blank space lands next to the traffic
                    // lights instead of between the controls and the tabs.
                    .justify_end()
                    // The traffic lights are native controls even with a client title bar.
                    .when(reserve_macos_title_bar_space, |controls| {
                        controls.ml(px(72.))
                    })
                    // Reserve a minimal, constant gap next to the traffic lights sized for
                    // two Large buttons (e.g. the application and profile menu triggers) —
                    // enough that tabs don't crowd the traffic lights when nothing else is
                    // there, but a `min_w` rather than extra margin so that any menus or the
                    // broadcast button which do render here expand into that reserve instead
                    // of pushing further out.
                    .when(reserve_compact_leading_controls, |controls| {
                        controls.min_w(COMPACT_LEADING_CONTROLS_RESERVE)
                    })
                    .when(show_title_bar_menus, |controls| {
                        controls.child(application_menu).child(profile_menu)
                    })
                    .when(show_title_bar_buttons, |controls| {
                        controls.child(
                            Button::new(
                                "auto-background-tab",
                                if show_title_bar_control_labels {
                                    "Keep running"
                                } else {
                                    ""
                                },
                            )
                            .start_icon(Icon::new(IconName::Pin).size(IconSize::Small))
                            .style(ButtonStyle::Subtle)
                            .size(ButtonSize::Large)
                            .toggle_state(auto_background_tab)
                            .aria_label("Keep this tab running after close")
                            .tooltip(Tooltip::for_action_title(
                                if auto_background_tab {
                                    if auto_background_protected {
                                        "Keep running after close is on · authentication required"
                                    } else {
                                        "Keep running after close is on · no authentication"
                                    }
                                } else {
                                    "Keep this tab running after the tab or window is closed"
                                },
                                &ToggleAutoBackgroundTab,
                            ))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleAutoBackgroundTab), cx)
                            }),
                        )
                    })
                    .when(show_title_bar_buttons, |controls| {
                        controls.child(
                            Button::new(
                                "detach-tab",
                                if show_title_bar_control_labels {
                                    "Detach"
                                } else {
                                    ""
                                },
                            )
                            .start_icon(Icon::new(IconName::Archive).size(IconSize::Small))
                            .style(ButtonStyle::Subtle)
                            .size(ButtonSize::Large)
                            .aria_label("Detach tab")
                            .tooltip(Tooltip::for_action_title(
                                "Detach tab to background",
                                &DetachTab,
                            ))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(DetachTab), cx)
                            }),
                        )
                    })
                    .when_some(reconnect_control, |controls, reconnect_control| {
                        controls.child(reconnect_control)
                    })
                    .when_some(broadcast_control, |controls, broadcast_control| {
                        controls.child(broadcast_control)
                    })
                    .when_some(silent_control, |controls, silent_control| {
                        controls.child(silent_control)
                    }),
            )
            .when_some(compact_tab_bar, |title_bar, tab_bar| {
                title_bar.child(tab_bar)
            })
            .when(!compact_mode, |title_bar| {
                title_bar.child(div().min_w_0().flex_1())
            })
            .when_some(active_pane_size, |title_bar, active_pane_size| {
                title_bar.child(
                    div().flex_none().px_2().child(
                        Label::new(active_pane_size)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                )
            })
            .child(right_title_bar_controls)
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "tests/title_bar_render.rs"]
mod tests;
