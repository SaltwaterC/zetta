//! The tab bar's geometry and its fixed controls: how wide a tab is, which
//! tabs fit, the overflow menu that lists the rest, and the new-tab button.

use super::*;

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

pub(super) fn tab_bar_visible_tab_range_with_empty_capacity(
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
                .map_or_else(|| "Terminal".to_string(), |pane| pane.profile.name.clone())
                .into()
        }
    })
}

/// A tab that did not fit in the bar, as its overflow menu offers it.
///
/// The menu is the only place such a tab is visible, so it carries the same
/// remote/local distinction the rendered tab does — there its stripe would have
/// nothing to attach to, so the entry takes an icon instead.
#[derive(Clone)]
pub(crate) struct TabOverflowEntry {
    pub(crate) index: usize,
    pub(crate) label: SharedString,
    pub(crate) remote: bool,
}

impl TabOverflowEntry {
    pub(crate) fn icon(&self) -> Option<IconName> {
        self.remote.then_some(IconName::Server)
    }
}

pub(crate) fn render_tab_overflow_trigger(
    is_right: bool,
    entries: Vec<TabOverflowEntry>,
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
                    Tooltip::text(tooltip_label),
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
                        for entry in entries.iter().cloned() {
                            let index = entry.index;
                            menu = match entry.icon() {
                                // `ContextMenuEntry::action` only records the
                                // action for the keybinding hint; the handler
                                // is what dispatches it, which is what
                                // `ContextMenu::action` installs for the plain
                                // entries below.
                                Some(icon) => menu.item(
                                    ui::ContextMenuEntry::new(entry.label)
                                        .icon(icon)
                                        // Trailing, so one marked entry does
                                        // not indent its label past every
                                        // unmarked one's.
                                        .icon_position(ui::IconPosition::End)
                                        .action(Box::new(SelectOverflowTab { index }))
                                        .handler(move |window, cx| {
                                            window.dispatch_action(
                                                Box::new(SelectOverflowTab { index }),
                                                cx,
                                            );
                                        }),
                                ),
                                None => {
                                    menu.action(entry.label, Box::new(SelectOverflowTab { index }))
                                }
                            };
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

pub(super) fn compact_tab_neighbor_control_mask(background: Hsla) -> gpui::Div {
    div().absolute().inset_0().rounded_sm().bg(background)
}

pub(super) fn render_compact_tab_neighbor_control(
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
                    window.dispatch_action(Box::new(NewTab), cx);
                }),
        )
        .into_any_element()
}

pub(super) fn new_tab_button_container(compact_mode: bool, compact_height: Pixels) -> gpui::Div {
    tab_bar_row_height(compact_mode, compact_height)
        .when(compact_mode, |button| button.ml(px(0.)).mr(px(12.)))
        .when(!compact_mode, |button| button.ml_1().mr_2())
        .flex_none()
        .flex()
        .items_center()
}
