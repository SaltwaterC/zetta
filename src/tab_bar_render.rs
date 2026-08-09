use super::*;

/// The per-frame inputs the tab bar needs, gathered once by `Render for Zetta`
/// so the measured tab row does not read them back out of the entity.
pub(crate) struct TabBarChrome {
    pub(crate) handle: WeakEntity<Zetta>,
    pub(crate) compact_mode: bool,
    pub(crate) title_bar_height: Pixels,
    pub(crate) tab_close_button_on_left: bool,
    pub(crate) is_renaming_tab: bool,
    pub(crate) tab_count: usize,
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

#[derive(Clone, Copy)]
struct TabDrag {
    tab_id: u64,
}

impl Zetta {
    /// The tab bar: a width-measured row of tabs with overflow triggers and the
    /// new-tab button, wrapped in the bar that hosts it. In compact mode the
    /// caller places the result inside the title bar instead of below it.
    pub(crate) fn render_tab_bar(
        &self,
        chrome: TabBarChrome,
        colors: &ThemeColors,
        rounded_top_right: bool,
        bottom_corner_radius: Pixels,
    ) -> gpui::Stateful<gpui::Div> {
        let compact_mode = chrome.compact_mode;
        let title_bar_height = chrome.title_bar_height;
        let tab_move_mode_active = chrome.tab_move_mode_active;
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
                tab_bar.rounded_tr(bottom_corner_radius)
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
        tab_close_button_on_left,
        is_renaming_tab,
        tab_count,
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
        // reserve the drag strip's guaranteed minimum, so tabs growing to fill
        // the bar can't force it to eat into their own width instead.
        let reserved_chrome_width = TAB_OVERFLOW_TRIGGER_WIDTH
            + if compact_mode {
                COMPACT_DRAG_AREA_MIN_WIDTH
            } else {
                px(0.)
            };
        let available_for_tabs = (size.width - reserved_chrome_width).max(px(0.));
        let is_shrinking =
            tab_bar_tabs_are_shrinking(available_for_tabs, is_renaming_tab, tab_count);
        let visible_range = tab_bar_visible_tab_range(
            available_for_tabs,
            tab_count,
            selected_tab_index,
            is_renaming_tab,
            overflow_selection,
        );

        let (tabs, left_overflow, right_overflow, first_visible_selected) = handle
            .read_with(cx, |this, cx| {
                let overflow_entries = |range: std::ops::Range<usize>| {
                    range
                        .filter_map(|index| {
                            let tab = this.tabs.get(index)?;
                            Some((index, tab_overflow_entry_label(tab, cx)))
                        })
                        .collect::<Vec<_>>()
                };
                let left_overflow = overflow_entries(0..visible_range.start);
                let right_overflow = overflow_entries(visible_range.end..tab_count);

                let visible_tabs: Vec<_> = this
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| visible_range.contains(index))
                    .map(|(index, tab)| {
                        let selected = index == this.active_tab;
                        (index, tab, selected)
                    })
                    .collect();
                let first_visible_selected = visible_tabs
                    .first()
                    .map(|(_, _, sel)| *sel)
                    .unwrap_or(false);
                let visible_tabs_for_next = visible_tabs.clone();
                let tabs = visible_tabs
                    .into_iter()
                    .enumerate()
                    .map(|(visible_index, (index, tab, selected))| {
                        let next_selected = visible_tabs_for_next
                            .get(visible_index + 1)
                            .map(|(_, _, next_sel)| *next_sel)
                            .unwrap_or(false);
                        render_tab(
                            TabChrome {
                                index,
                                selected,
                                next_selected,
                                tab_count,
                                tab_move_mode_active,
                                is_shrinking,
                                is_renaming_tab,
                                compact_mode,
                                title_bar_height,
                                tab_close_button_on_left,
                                handle: &handle,
                            },
                            tab,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();

                (tabs, left_overflow, right_overflow, first_visible_selected)
            })
            .unwrap_or_default();

        div()
            .id("tabs-scroll")
            .when(compact_mode, |tabs| tabs.h(title_bar_height))
            .when(!compact_mode, |tabs| tabs.h_full())
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            // Buttons form one contiguous area with no dividers between them, so
            // this separator (matching the tab bar's own former left border) only
            // belongs here when a tab, not the left overflow trigger, sits first.
            // Also omit when the first visible tab is the active tab in compact mode.
            .when(
                compact_mode && left_overflow.is_empty() && !first_visible_selected,
                |tabs| tabs.border_l_1().border_color(border_color.opacity(0.25)),
            )
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
            .children(tabs)
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
            .when(compact_mode, |bar| {
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
    tab_move_mode_active: bool,
    is_shrinking: bool,
    is_renaming_tab: bool,
    compact_mode: bool,
    title_bar_height: Pixels,
    tab_close_button_on_left: bool,
    handle: &'a WeakEntity<Zetta>,
}

fn render_tab(chrome: TabChrome<'_>, tab: &Tab, cx: &App) -> AnyElement {
    let TabChrome {
        index,
        selected,
        next_selected,
        tab_count,
        tab_move_mode_active,
        is_shrinking,
        is_renaming_tab,
        compact_mode,
        title_bar_height,
        tab_close_button_on_left,
        handle,
    } = chrome;
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
    let full_title = if let Some(buffer) = tab
        .rename_buffer
        .as_ref()
        .filter(|_| tab.renaming_pane.is_none())
    {
        buffer.clone().into()
    } else {
        tab_overflow_entry_label(tab, cx)
    };
    let content = h_flex()
        .min_w_0()
        .gap_1()
        .when(
            matches!(tab.close_policy, TabClosePolicy::Background { .. }),
            |content| {
                content.child(
                    svg()
                        .path(IconName::Pin.path())
                        .size(px(12.))
                        .flex_none()
                        .text_color(tab_icon),
                )
            },
        )
        // The tab being renamed always keeps its icon, even if the
        // rest of the bar is shrinking enough to hide everyone else's.
        .when(!is_shrinking || (is_renaming_tab && selected), |content| {
            content.when_some(tab.icon, |content, icon| {
                content.child(
                    svg()
                        .path(icon.path())
                        .size(px(14.))
                        .flex_none()
                        .text_color(tab_icon),
                )
            })
        })
        .child(
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
                .tooltip(Tooltip::text(full_title))
                .text_color(tab_text)
                .child(title),
        )
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
        .bg(tab_background)
        .cursor(if tab_move_mode_active {
            CursorStyle::ResizeLeftRight
        } else {
            CursorStyle::OpenHand
        })
        .on_drag(TabDrag { tab_id: tab.id }, |_, _, _, cx| {
            cx.new(|_| gpui::Empty)
        })
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
        );
    let menu_handle = handle.clone();
    // The context menu activates this tab before it is rendered. Use
    // the clicked tab's focus so its key context remains valid after
    // that switch, including when the tab was previously inactive.
    let action_context = tab
        .active_pane()
        .and_then(|pane| pane.view.as_ref())
        .map(|view| view.focus_handle(cx));
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
    let tab_element = responsive_tab_container(
        tab_element,
        compact_mode,
        title_bar_height,
        is_renaming_tab && selected,
    );
    let tab_element = if cx.has_active_drag() {
        tab_element
            .relative()
            .child(render_tab_drop_surface(
                tab.id,
                false,
                tab_colors.drop_target_background,
                tab_colors.drop_target_border,
                handle.clone(),
            ))
            .child(render_tab_drop_surface(
                tab.id,
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
                .is_some_and(|drag| drag.tab_id != target_tab_id)
        })
        .drag_over::<TabDrag>(move |surface, dragged, _, _| {
            if dragged.tab_id == target_tab_id {
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
