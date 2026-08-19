use super::*;
use crate::rename::resolve_tab_title;

/// The per-frame inputs the tab bar needs, gathered once by `Render for Zetta`
/// so the measured tab row does not read them back out of the entity.
pub(crate) struct TabBarChrome {
    pub(crate) handle: WeakEntity<Zetta>,
    pub(crate) compact_mode: bool,
    pub(crate) title_bar_height: Pixels,
    pub(crate) is_macos_fullscreen: bool,
    pub(crate) rounded_top_right: bool,
    pub(crate) compact_tab_top_left: bool,
    pub(crate) compact_tab_top_right: bool,
    pub(crate) compact_tab_bottom_left: bool,
    pub(crate) compact_tab_bottom_right: bool,
    pub(crate) corner_radius: Pixels,
    pub(crate) tab_bar_background: Hsla,
    /// Background immediately before the compact tab row. The first tab's
    /// rounded left edge reveals this through its transparent corners.
    pub(crate) compact_leading_background: Hsla,
    pub(crate) tab_close_button_on_left: bool,
    pub(crate) is_renaming_tab: bool,
    pub(crate) tab_count: usize,
    pub(crate) pinned_tab_count: usize,
    pub(crate) is_renaming_pinned: bool,
    pub(crate) selected_tab_index: usize,
    pub(crate) tab_move_mode_active: bool,
    /// `Some(true)` when the user is stepping through the right overflow menu,
    /// `Some(false)` for the left one.
    pub(crate) overflow_selection: Option<bool>,
    pub(crate) border_color: Hsla,
    pub(crate) left_menu_handle: PopoverMenuHandle<ui::ContextMenu>,
    pub(crate) right_menu_handle: PopoverMenuHandle<ui::ContextMenu>,
}

fn tab_move_menu_entry_available(tab_count: usize) -> bool {
    tab_count >= 2
}

fn tab_auto_background_enabled(close_policy: &TabClosePolicy) -> bool {
    matches!(close_policy, TabClosePolicy::Background { .. })
}

fn tab_leading_icons(
    background_tab: bool,
    silent_mode: bool,
    custom_icon: Option<IconName>,
    custom_icon_visible: bool,
) -> (Option<IconName>, Option<IconName>, Option<IconName>) {
    (
        background_tab.then_some(IconName::Pin),
        silent_mode.then_some(IconName::BellOff),
        custom_icon.filter(|_| custom_icon_visible),
    )
}

#[derive(Clone, Copy)]
struct TabDrag {
    tab_id: u64,
    pinned: bool,
}

impl Zetta {
    /// The tab bar: a width-measured row of tabs with overflow triggers and the
    /// new-tab button, wrapped in the bar that hosts it. In compact mode the
    /// caller places the result inside the title bar instead of below it.
    pub(crate) fn render_tab_bar(
        &self,
        chrome: TabBarChrome,
        colors: &ThemeColors,
    ) -> gpui::Stateful<gpui::Div> {
        let compact_mode = chrome.compact_mode;
        let title_bar_height = chrome.title_bar_height;
        let tab_move_mode_active = chrome.tab_move_mode_active;
        let rounded_top_right = chrome.rounded_top_right;
        let corner_radius = chrome.corner_radius;
        let tabs_scroll = render_tabs_row(chrome).into_any_element();

        tab_bar_row_height(compact_mode, title_bar_height)
            .id("tab-bar")
            .flex_none()
            .when(compact_mode, |tab_bar| {
                tab_bar
                    .flex_grow_1()
                    .flex_shrink_1()
                    .flex_basis(gpui::relative(0.))
                    .min_w_0()
                    .occlude()
            })
            .flex()
            .items_center()
            .bg(colors.tab_bar_background)
            .when(compact_mode && rounded_top_right, |tab_bar| {
                tab_bar.rounded_tr(corner_radius)
            })
            .when(!compact_mode, |tab_bar| {
                tab_bar
                    .border_t_1()
                    .border_b_1()
                    .border_color(colors.border)
            })
            .on_click(|event, window, cx| {
                cx.stop_propagation();
                if event.click_count() == 2 {
                    window.dispatch_action(Box::new(NewTab), cx)
                }
            })
            .when(tab_move_mode_active, |tab_bar| {
                tab_bar.child(render_tab_move_mode_indicator(
                    compact_mode,
                    title_bar_height,
                    colors,
                ))
            })
            .child(tabs_scroll)
    }
}

fn render_tab_move_mode_indicator(
    compact_mode: bool,
    title_bar_height: Pixels,
    colors: &ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    tab_bar_row_height(compact_mode, title_bar_height)
        .id("tab-move-mode-indicator")
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .border_r_1()
        .border_color(colors.border)
        .bg(colors.status_bar_background)
        .aria_label("Tab move mode")
        .tooltip(Tooltip::text(
            "Tab move mode: Left and Right move the active tab",
        ))
        .child(Label::new("Move tab").size(LabelSize::Small))
        .child(
            Label::new("← →")
                .size(LabelSize::Small)
                .color(Color::Custom(colors.text_muted)),
        )
}

/// The measured row of tabs. The visible range and shrink behaviour depend on
/// the width this row is actually given, so the whole row is built inside a
/// `container_query` rather than during the enclosing render pass.
fn render_tabs_row(chrome: TabBarChrome) -> impl IntoElement {
    let TabBarChrome {
        handle,
        compact_mode,
        title_bar_height,
        is_macos_fullscreen,
        rounded_top_right: _,
        compact_tab_top_left,
        compact_tab_top_right,
        compact_tab_bottom_left,
        compact_tab_bottom_right,
        corner_radius,
        tab_bar_background,
        compact_leading_background,
        tab_close_button_on_left,
        is_renaming_tab,
        tab_count,
        pinned_tab_count,
        is_renaming_pinned,
        selected_tab_index,
        tab_move_mode_active,
        overflow_selection,
        border_color,
        left_menu_handle,
        right_menu_handle,
    } = chrome;

    container_query(move |size, _window, cx| {
        // The new-tab button now renders inside this same measured row (right
        // after the tabs/right overflow trigger) so it stays snug against them
        // instead of sitting at the edge of the bar. Reserve its footprint here,
        // on top of whatever an overflow trigger itself needs, so it can never
        // get pushed out of the measured width and clipped. In compact mode also
        // reserve the drag strip's guaranteed minimum, except in macOS
        // fullscreen where the artificial strip is omitted.
        let show_compact_drag_area = compact_drag_area_visible(compact_mode, is_macos_fullscreen);
        let reserved_chrome_width =
            TAB_OVERFLOW_TRIGGER_WIDTH + compact_drag_area_reserve_width(show_compact_drag_area);
        let pinned_count = pinned_tab_count;
        let unpinned_count = tab_count.saturating_sub(pinned_count);
        let selected_unpinned_index = selected_tab_index
            .saturating_sub(pinned_count)
            .min(unpinned_count.saturating_sub(1));
        let pinned_width = PINNED_TAB_WIDTH * pinned_count
            + if is_renaming_pinned {
                TAB_MAX_WIDTH - PINNED_TAB_WIDTH
            } else {
                px(0.)
            };
        let available_for_tabs = (size.width - reserved_chrome_width - pinned_width).max(px(0.));
        let unpinned_is_renaming = is_renaming_tab && !is_renaming_pinned;
        let is_shrinking =
            tab_bar_tabs_are_shrinking(available_for_tabs, unpinned_is_renaming, unpinned_count);
        let visible_range = tab_bar_visible_tab_range_with_pinned_tabs(
            available_for_tabs,
            unpinned_count,
            selected_unpinned_index,
            unpinned_is_renaming,
            overflow_selection,
            pinned_count > 0,
        );

        let (tabs, left_overflow, right_overflow, first_visible_selected) = handle
            .read_with(cx, |this, cx| {
                let overflow_entries = |range: std::ops::Range<usize>| {
                    range
                        .filter_map(|index| {
                            let absolute_index = pinned_count + index;
                            let tab = this.tabs.get(absolute_index)?;
                            Some((absolute_index, tab_overflow_entry_label(tab, cx)))
                        })
                        .collect::<Vec<_>>()
                };
                let left_overflow = overflow_entries(0..visible_range.start);
                let right_overflow = overflow_entries(visible_range.end..unpinned_count);

                let visible_tabs: Vec<_> = this
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        *index < pinned_count
                            || visible_range.contains(&index.saturating_sub(pinned_count))
                    })
                    .map(|(index, tab)| {
                        let selected = index == this.active_tab;
                        // Resolved once per visible tab and carried through: the
                        // selected tab's neighbour lookups below want the same
                        // themes, and each `theme_for_tab` is a registry lock read
                        // plus an `Arc` clone.
                        let theme = this.theme_for_tab(tab, cx);
                        (index, tab, selected, theme)
                    })
                    .collect();
                let first_visible_selected = visible_tabs
                    .first()
                    .map(|(_, _, sel, _)| *sel)
                    .unwrap_or(false);
                let visible_tabs_for_neighbors = visible_tabs.clone();
                let tabs = visible_tabs
                    .into_iter()
                    .enumerate()
                    .map(|(visible_index, (index, tab, selected, tab_theme))| {
                        let next_selected = visible_tabs_for_neighbors
                            .get(visible_index + 1)
                            .map(|(_, _, next_sel, _)| *next_sel)
                            .unwrap_or(false);
                        let (left_transition_background, right_transition_background) = if selected
                        {
                            let left_background = visible_index
                                .checked_sub(1)
                                .and_then(|index| visible_tabs_for_neighbors.get(index))
                                .map(|(_, _, _, theme)| theme.colors().tab_inactive_background);
                            let right_background = visible_tabs_for_neighbors
                                .get(visible_index + 1)
                                .map(|(_, _, _, theme)| theme.colors().tab_inactive_background);
                            // With no pinned tab or overflow trigger before it,
                            // the first visible tab sits directly beside the
                            // title-bar controls. Its rounded corner must reveal
                            // that background, not the tab bar's, or differing
                            // theme colors leave a square seam at the boundary.
                            let left_edge_background = active_tab_left_edge_background(
                                visible_index,
                                pinned_count,
                                visible_range.start,
                                compact_leading_background,
                            );
                            active_tab_transition_backgrounds(
                                left_background,
                                right_background,
                                left_edge_background,
                                tab_bar_background,
                            )
                        } else {
                            (tab_bar_background, tab_bar_background)
                        };
                        render_tab(
                            TabChrome {
                                index,
                                selected,
                                next_selected,
                                tab_count,
                                pinned: index < pinned_count,
                                tab_move_mode_active,
                                is_shrinking,
                                is_renaming_tab,
                                compact_mode,
                                title_bar_height,
                                tab_close_button_on_left,
                                compact_tab_top_left,
                                compact_tab_top_right,
                                compact_tab_bottom_left,
                                compact_tab_bottom_right,
                                corner_radius,
                                left_transition_background,
                                right_transition_background,
                                handle: &handle,
                            },
                            tab,
                            tab_theme,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();
                (tabs, left_overflow, right_overflow, first_visible_selected)
            })
            .unwrap_or_default();
        let mut tab_iter = tabs.into_iter();
        let pinned_tabs = tab_iter.by_ref().take(pinned_count);

        div()
            .id("tabs-scroll")
            .when(compact_mode, |tabs| tabs.h(title_bar_height))
            .when(!compact_mode, |tabs| tabs.h_full())
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            // The selected compact tab deliberately paints its lower corner
            // transitions into the neighboring controls. The visible-range
            // calculation already constrains the row's layout; clipping is
            // only needed for the standalone tab bar.
            .when(!compact_mode, |tabs| tabs.overflow_hidden())
            // Buttons form one contiguous area with no dividers between them, so
            // this separator (matching the tab bar's own former left border) only
            // belongs here when a tab, not the left overflow trigger, sits first.
            // Also omit when the first visible tab is the active tab in compact mode.
            .when(
                compact_mode && left_overflow.is_empty() && !first_visible_selected,
                |tabs| tabs.border_l_1().border_color(border_color.opacity(0.25)),
            )
            // Keep the pinned prefix ahead of the unpinned overflow control;
            // the overflow menu only represents tabs from the unpinned range.
            .children(pinned_tabs)
            .when(!left_overflow.is_empty(), |bar| {
                let overflow_border = if compact_mode {
                    border_color.opacity(0.5)
                } else {
                    border_color
                };
                bar.child(render_tab_overflow_trigger(
                    false,
                    left_overflow,
                    compact_mode,
                    title_bar_height,
                    overflow_border,
                    left_menu_handle.clone(),
                    handle.clone(),
                ))
            })
            .children(tab_iter)
            .when(!right_overflow.is_empty(), |bar| {
                let overflow_border = if compact_mode {
                    border_color.opacity(0.5)
                } else {
                    border_color
                };
                bar.child(render_tab_overflow_trigger(
                    true,
                    right_overflow,
                    compact_mode,
                    title_bar_height,
                    overflow_border,
                    right_menu_handle.clone(),
                    handle.clone(),
                ))
            })
            .child(render_new_tab_button(compact_mode, title_bar_height))
            .when(show_compact_drag_area, |bar| {
                bar.child(render_compact_drag_area(title_bar_height, handle.clone()))
            })
    })
    .min_w_0()
    .flex_shrink_1()
}

/// Everything a single tab needs that the enclosing measured row already knows.
struct TabChrome<'a> {
    index: usize,
    selected: bool,
    next_selected: bool,
    tab_count: usize,
    pinned: bool,
    tab_move_mode_active: bool,
    is_shrinking: bool,
    is_renaming_tab: bool,
    compact_mode: bool,
    title_bar_height: Pixels,
    tab_close_button_on_left: bool,
    compact_tab_top_left: bool,
    compact_tab_top_right: bool,
    compact_tab_bottom_left: bool,
    compact_tab_bottom_right: bool,
    corner_radius: Pixels,
    left_transition_background: Hsla,
    right_transition_background: Hsla,
    handle: &'a WeakEntity<Zetta>,
}

fn active_tab_shape_visible(compact_mode: bool, selected: bool) -> bool {
    compact_mode && selected
}

fn active_tab_transition_backgrounds(
    left_background: Option<Hsla>,
    right_background: Option<Hsla>,
    left_edge_background: Option<Hsla>,
    tab_bar_background: Hsla,
) -> (Hsla, Hsla) {
    (
        left_background
            .or(left_edge_background)
            .unwrap_or(tab_bar_background),
        right_background.unwrap_or(tab_bar_background),
    )
}

fn active_tab_left_edge_background(
    visible_index: usize,
    pinned_count: usize,
    unpinned_visible_start: usize,
    compact_leading_background: Hsla,
) -> Option<Hsla> {
    (visible_index == 0 && (pinned_count > 0 || unpinned_visible_start == 0))
        .then_some(compact_leading_background)
}

fn render_active_tab_bottom_transition(
    is_left: bool,
    active_background: Hsla,
    tab_bar_background: Hsla,
    corner_radius: Pixels,
) -> gpui::Div {
    div()
        .absolute()
        .bottom_0()
        .when(is_left, |transition| transition.left_0())
        .when(!is_left, |transition| transition.right_0())
        .w(corner_radius)
        .h(corner_radius)
        .bg(active_background)
        .child(
            div()
                .size_full()
                // Leave the lower inner corner transparent so the active fill
                // joins the tab body while the surrounding color follows the
                // concave outer arc.
                .when(is_left, |cutout| cutout.rounded_br(corner_radius))
                .when(!is_left, |cutout| cutout.rounded_bl(corner_radius))
                .bg(tab_bar_background),
        )
}

fn render_active_tab_top_transition_background(
    is_left: bool,
    surrounding_background: Hsla,
    corner_radius: Pixels,
) -> gpui::Div {
    div()
        .absolute()
        .top_0()
        // The active shape's canvas extends one radius beyond the measured
        // tab. Its body starts one radius back in, so place this underlay at
        // that body edge to show the neighboring tab through the rounded top
        // corner instead of the title bar's default background.
        .when(is_left, |background| background.left(corner_radius))
        .when(!is_left, |background| background.right(corner_radius))
        .size(corner_radius)
        .bg(surrounding_background)
}

fn render_active_tab_shape_base_fill(
    top_left: bool,
    top_right: bool,
    bottom_left: bool,
    bottom_right: bool,
    corner_radius: Pixels,
    active_background: Hsla,
) -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .when(bottom_left, |body| body.left(corner_radius))
        .when(!bottom_left, |body| body.left_0())
        .when(bottom_right, |body| body.right(corner_radius))
        .when(!bottom_right, |body| body.right_0())
        .when(top_left, |body| body.rounded_tl(corner_radius))
        .when(top_right, |body| body.rounded_tr(corner_radius))
        .bg(active_background)
}

#[allow(clippy::too_many_arguments)]
fn render_active_tab_shape(
    top_left: bool,
    top_right: bool,
    bottom_left: bool,
    bottom_right: bool,
    active_background: Hsla,
    left_transition_background: Hsla,
    right_transition_background: Hsla,
    corner_radius: Pixels,
) -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        // Preserve the original corner construction, but give it one radius
        // of visual canvas outside each rounded lower edge. The body's equal
        // positive inset then resolves exactly to the measured tab bounds.
        .when(bottom_left, |shape| shape.left(-corner_radius))
        .when(!bottom_left, |shape| shape.left_0())
        .when(bottom_right, |shape| shape.right(-corner_radius))
        .when(!bottom_right, |shape| shape.right_0())
        .when(top_left, |shape| {
            shape.child(render_active_tab_top_transition_background(
                true,
                left_transition_background,
                corner_radius,
            ))
        })
        .when(top_right, |shape| {
            shape.child(render_active_tab_top_transition_background(
                false,
                right_transition_background,
                corner_radius,
            ))
        })
        .child(render_active_tab_shape_base_fill(
            top_left,
            top_right,
            bottom_left,
            bottom_right,
            corner_radius,
            active_background,
        ))
        // If a lower transition is rounded while its upper corner is square,
        // fill the side above the transition so the tab does not acquire a
        // notch at the top.
        .when(bottom_left && !top_left, |shape| {
            shape.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom(corner_radius)
                    .left_0()
                    .w(corner_radius)
                    .bg(active_background),
            )
        })
        .when(bottom_right && !top_right, |shape| {
            shape.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom(corner_radius)
                    .right_0()
                    .w(corner_radius)
                    .bg(active_background),
            )
        })
        .when(bottom_left, |shape| {
            shape.child(render_active_tab_bottom_transition(
                true,
                active_background,
                left_transition_background,
                corner_radius,
            ))
        })
        .when(bottom_right, |shape| {
            shape.child(render_active_tab_bottom_transition(
                false,
                active_background,
                right_transition_background,
                corner_radius,
            ))
        })
}

fn render_tab(chrome: TabChrome<'_>, tab: &Tab, tab_theme: Arc<Theme>, cx: &App) -> AnyElement {
    let TabChrome {
        index,
        selected,
        next_selected,
        tab_count,
        pinned,
        tab_move_mode_active,
        is_shrinking,
        is_renaming_tab,
        compact_mode,
        title_bar_height,
        tab_close_button_on_left,
        compact_tab_top_left,
        compact_tab_top_right,
        compact_tab_bottom_left,
        compact_tab_bottom_right,
        corner_radius,
        left_transition_background,
        right_transition_background,
        handle,
    } = chrome;
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
    let show_active_tab_shape = active_tab_shape_visible(compact_mode, selected);
    let is_renaming_this_tab = pinned && is_renaming_tab && selected && tab.renaming_pane.is_none();
    let select_handle = handle.clone();
    let close_handle = handle.clone();
    let rename_view = tab.active_view();
    let title = if let Some(buffer) = tab
        .rename_buffer
        .as_ref()
        .filter(|_| tab.renaming_pane.is_none())
    {
        if tab.rename_select_all {
            buffer.clone().into()
        } else {
            let cursor = tab.rename_cursor.min(buffer.len());
            let (before, after) = buffer.split_at(cursor);
            format!("{before}|{after}").into()
        }
    } else {
        resolve_tab_title(tab, || {
            if let Some(view) = tab.active_view() {
                view.read(cx).tab_content_text(0, cx)
            } else {
                tab.active_pane()
                    .map(|pane| pane.profile.name.clone())
                    .unwrap_or_else(|| "Terminal".to_string())
                    .into()
            }
        })
    };
    let full_title = if let Some(buffer) = tab
        .rename_buffer
        .as_ref()
        .filter(|_| tab.renaming_pane.is_none())
    {
        buffer.clone().into()
    } else {
        tab_overflow_entry_label(tab, cx)
    };
    let attention_tooltip = tab.attention.as_ref().map(TabAttention::tooltip_text);
    let tab_auto_background = tab_auto_background_enabled(&tab.close_policy);
    let (pin_icon, silent_mode_icon, custom_icon) = tab_leading_icons(
        tab_auto_background,
        tab.silent_mode,
        tab.icon,
        pinned || !is_shrinking || (is_renaming_tab && selected),
    );
    let accessible_title = full_title.clone();
    let content = h_flex()
        .min_w_0()
        .gap_1()
        .when_some(pin_icon, |content, icon| {
            content.child(
                svg()
                    .path(icon.path())
                    .size(px(12.))
                    .flex_none()
                    .text_color(tab_icon),
            )
        })
        .when_some(silent_mode_icon, |content, icon| {
            content.child(
                div()
                    .id(("tab-silent-mode", tab.id as usize))
                    .flex_none()
                    .aria_label("Tab Silent Mode enabled")
                    .tooltip(Tooltip::text(
                        "Tab Silent Mode: terminal bells and notification sounds are muted",
                    ))
                    .child(svg().path(icon.path()).size(px(14.)).text_color(tab_icon)),
            )
        })
        .when_some(attention_tooltip, |content, tooltip| {
            content.child(
                div()
                    .id(("tab-attention", tab.id as usize))
                    .size(px(7.))
                    .flex_none()
                    .rounded_full()
                    .bg(tab_colors.text_accent)
                    .aria_label("Attention required")
                    .tooltip(Tooltip::text(tooltip)),
            )
        })
        // The tab being renamed always keeps its custom icon, even if the
        // rest of the bar is shrinking enough to hide everyone else's.
        .when_some(custom_icon, |content, icon| {
            content.child(
                svg()
                    .path(icon.path())
                    .size(px(14.))
                    .flex_none()
                    .text_color(tab_icon),
            )
        })
        .when(!pinned || is_renaming_this_tab, |content| {
            content.child(
                div()
                    .id(("tab-title", tab.id as usize))
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .when(
                        tab.rename_buffer.is_some()
                            && tab.renaming_pane.is_none()
                            && tab.rename_select_all,
                        |title| title.bg(tab_colors.element_selection_background),
                    )
                    .text_color(tab_text)
                    .child(title),
            )
        })
        .into_any_element();
    let tab_element = tab_bar_row_height(compact_mode, title_bar_height)
        .id(("tab", tab.id as usize))
        .w_full()
        .min_w_0()
        .px_2()
        .flex()
        .when(tab_close_button_on_left, |tab| tab.flex_row_reverse())
        .items_center()
        .gap_1()
        .when(!(compact_mode && (selected || next_selected)), |tab| {
            tab.border_r_1().border_color(if compact_mode {
                tab_colors.border.opacity(0.5)
            } else {
                tab_colors.border
            })
        })
        .when(tab_move_mode_active && selected, |tab| {
            tab.border_b_2().border_color(tab_colors.text_accent)
        })
        .when(!show_active_tab_shape, |tab| tab.bg(tab_background))
        .when(show_active_tab_shape, |tab| {
            tab.relative().child(render_active_tab_shape(
                compact_tab_top_left,
                compact_tab_top_right,
                compact_tab_bottom_left,
                compact_tab_bottom_right,
                tab_background,
                left_transition_background,
                right_transition_background,
                corner_radius,
            ))
        })
        .aria_label(accessible_title)
        .tooltip(Tooltip::text(full_title.clone()))
        .cursor(if tab_move_mode_active {
            CursorStyle::ResizeLeftRight
        } else {
            CursorStyle::OpenHand
        })
        .on_drag(
            TabDrag {
                tab_id: tab.id,
                pinned: tab.pinned,
            },
            |_, _, _, cx| cx.new(|_| gpui::Empty),
        )
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            select_handle
                .update(cx, |this, cx| {
                    this.active_tab = index;
                    this.tab_overflow_selection_side = None;
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
        .when(!pinned, |tab_element| {
            tab_element.child(
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
                    .tooltip(move |_window, cx| Tooltip::for_action("Close tab", &CloseTab, cx))
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
        });
    let menu_handle = handle.clone();
    let tab_silent_mode = tab.silent_mode;
    let tab_shared = tab.shared;
    // The context menu activates this tab before it is rendered. Use
    // the clicked tab's focus so its key context remains valid after
    // that switch, including when the tab was previously inactive.
    let action_context = tab.active_view().map(|view| view.focus_handle(cx));
    let tab_element =
        ui::right_click_menu::<ui::ContextMenu>(("tab-context-menu", tab.id as usize))
            .menu(move |window, cx| {
                menu_handle
                    .update(cx, |this, cx| {
                        this.active_tab = index;
                        this.tab_overflow_selection_side = None;
                        cx.notify();
                    })
                    .ok();
                let action_context = action_context.clone();
                ui::ContextMenu::build(window, cx, move |menu, _, _| {
                    let menu = menu.when_some(action_context, |menu, focus| menu.context(focus));
                    menu.action("Rename Tab", Box::new(RenameTab))
                        .action("Change Tab Icon", Box::new(ChangeTabIcon))
                        .action_checked("Pin Tab", Box::new(ToggleTabPinning), pinned)
                        .action_checked(
                            "Tab Silent Mode",
                            Box::new(ToggleTabSilentMode),
                            tab_silent_mode,
                        )
                        .separator()
                        .action_checked(
                            "Keep running",
                            Box::new(ToggleAutoBackgroundTab),
                            tab_auto_background,
                        )
                        .action_checked("Share Tab", Box::new(ToggleTabSharing), tab_shared)
                        .action("Detach", Box::new(DetachTab))
                        .when(tab_move_menu_entry_available(tab_count), |menu| {
                            menu.separator().action_checked(
                                "Tab Move Mode",
                                Box::new(ToggleTabMoveMode),
                                tab_move_mode_active,
                            )
                        })
                })
            })
            .trigger(move |_, _, _| tab_element)
            .into_any_element();
    let tab_element = if pinned {
        pinned_tab_container(
            tab_element,
            compact_mode,
            title_bar_height,
            is_renaming_this_tab,
        )
    } else {
        responsive_tab_container(
            tab_element,
            compact_mode,
            title_bar_height,
            is_renaming_tab && selected,
        )
    };
    let tab_element = if cx.has_active_drag() {
        tab_element
            .relative()
            .child(render_tab_drop_surface(
                tab.id,
                tab.pinned,
                false,
                tab_colors.drop_target_background,
                tab_colors.drop_target_border,
                handle.clone(),
            ))
            .child(render_tab_drop_surface(
                tab.id,
                tab.pinned,
                true,
                tab_colors.drop_target_background,
                tab_colors.drop_target_border,
                handle.clone(),
            ))
    } else {
        tab_element
    };
    tab_element.into_any_element()
}

fn render_tab_drop_surface(
    target_tab_id: u64,
    target_pinned: bool,
    insert_after: bool,
    drop_target_background: Hsla,
    drop_target_border: Hsla,
    handle: WeakEntity<Zetta>,
) -> gpui::Stateful<gpui::Div> {
    let position = if insert_after {
        TabDropPosition::After(target_tab_id)
    } else {
        TabDropPosition::Before(target_tab_id)
    };
    let drop_handle = handle;
    div()
        .id(format!("tab-drop-{target_tab_id}-{insert_after}"))
        .absolute()
        .top_0()
        .bottom_0()
        .when(insert_after, |surface| surface.right_0())
        .when(!insert_after, |surface| surface.left_0())
        .w(gpui::relative(0.5))
        .can_drop(move |dragged, _, _| {
            dragged
                .downcast_ref::<TabDrag>()
                .is_some_and(|drag| drag.tab_id != target_tab_id && drag.pinned == target_pinned)
        })
        .drag_over::<TabDrag>(move |surface, dragged, _, _| {
            if dragged.tab_id == target_tab_id || dragged.pinned != target_pinned {
                return surface;
            }
            let surface = surface
                .bg(drop_target_background)
                .border_color(drop_target_border);
            if insert_after {
                surface.border_r_2()
            } else {
                surface.border_l_2()
            }
        })
        .on_drop(move |dragged: &TabDrag, _, cx| {
            drop_handle
                .update(cx, |this, cx| {
                    this.reorder_tab(dragged.tab_id, position, cx)
                })
                .ok();
        })
}

#[cfg(test)]
#[path = "tests/tab_bar_render.rs"]
mod tests;
