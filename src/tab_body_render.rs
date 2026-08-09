use super::*;

const MINIMIZED_PANE_ENTRY_MIN_WIDTH: Pixels = px(180.);
const MINIMIZED_PANE_ENTRY_GAP: Pixels = px(4.);

fn minimized_pane_capacity(available_width: Pixels, pane_count: usize) -> usize {
    if pane_count == 0 {
        return 0;
    }

    let capacity = ((available_width + MINIMIZED_PANE_ENTRY_GAP)
        / (MINIMIZED_PANE_ENTRY_MIN_WIDTH + MINIMIZED_PANE_ENTRY_GAP))
        .floor() as usize;
    capacity.clamp(1, pane_count)
}

fn visible_minimized_pane_range(
    pane_count: usize,
    selected_index: usize,
    capacity: usize,
) -> std::ops::Range<usize> {
    let capacity = capacity.clamp(1, pane_count);
    let selected_index = selected_index.min(pane_count - 1);
    let page_start = selected_index / capacity * capacity;
    let start = page_start.min(pane_count - capacity);
    start..start + capacity
}

fn resolve_visible_minimized_panes<T>(
    pane_count: usize,
    selected_index: usize,
    capacity: usize,
    mut resolve: impl FnMut(usize) -> Option<T>,
) -> Vec<T> {
    if pane_count == 0 {
        return Vec::new();
    }

    let range = visible_minimized_pane_range(pane_count, selected_index, capacity);
    let mut entries = Vec::with_capacity(range.len());
    entries.extend(range.filter_map(&mut resolve));
    entries
}

impl Zetta {
    pub(crate) fn render_tab_body(
        &self,
        window: &Window,
        rounded_bottom_left: bool,
        rounded_bottom_right: bool,
        bottom_corner_radius: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.tabs.get(self.active_tab) {
            Some(tab) => {
                let tab_theme = tab.theme(cx);
                let tab_colors = tab_theme.colors().clone();
                let tab_error_color = tab_theme.status().error;
                let layout = tab.visible_layout();
                let maximized_pane = tab.maximized_pane.and_then(|pane_id| {
                    tab.pane(pane_id).map(|pane| {
                        (
                            pane_id,
                            tab.displayed_pane_label(pane_id)
                                .unwrap_or_else(|| pane.label()),
                            tab.renaming_pane == Some(pane_id)
                                && tab.rename_select_all
                                && tab.rename_buffer.is_some(),
                        )
                    })
                });
                let minimized_count = tab.minimized_panes.len();
                let minimized_index = tab
                    .selected_minimized_pane
                    .and_then(|selected| {
                        tab.minimized_panes
                            .iter()
                            .position(|pane_id| *pane_id == selected)
                    })
                    .unwrap_or(0);
                let minimized_shelf = tab
                    .minimized_panes
                    .get(minimized_index)
                    .copied()
                    .map(|pane_id| (pane_id, minimized_index, minimized_count));
                let panes_own_window_bottom = maximized_pane.is_none() && minimized_shelf.is_none();
                let maximized_bar_owns_window_bottom =
                    maximized_pane.is_some() && minimized_shelf.is_none();
                let minimized_shelf_owns_window_bottom = minimized_shelf.is_some();
                let content = layout
                    .as_ref()
                    .map(|layout| {
                        self.render_pane_layout(
                            tab,
                            layout,
                            &tab_colors,
                            tab_error_color,
                            window,
                            panes_own_window_bottom,
                            cx,
                        )
                    })
                    .unwrap_or_else(|| div().size_full().into_any_element());
                let handle = cx.entity().downgrade();
                div()
                    .size_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when(self.tab_move_mode, |body| body.key_context("TabMove"))
                    .child(div().min_h_0().flex_1().child(content))
                    .when_some(maximized_pane, |body, (pane_id, pane_label, pane_label_selected)| {
                        let restore_handle = handle.clone();
                        let close_handle = handle.clone();
                        let tab_id = tab.id;
                        body.child(
                            div()
                                .h_7()
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_2()
                                .bg(tab_colors.status_bar_background)
                                .border_t_1()
                                .border_color(tab_colors.border)
                                .when(
                                    maximized_bar_owns_window_bottom && rounded_bottom_left,
                                    |bar| bar.rounded_bl(bottom_corner_radius),
                                )
                                .when(
                                    maximized_bar_owns_window_bottom && rounded_bottom_right,
                                    |bar| bar.rounded_br(bottom_corner_radius),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Icon::new(IconName::Maximize)
                                                .size(IconSize::XSmall)
                                                .color(Color::Custom(tab_colors.text_accent)),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .px_1()
                                                        .rounded_sm()
                                                        .when(pane_label_selected, |label| {
                                                            label.bg(tab_colors.element_selected)
                                                        })
                                                        .child(
                                                            Label::new(pane_label)
                                                                .size(LabelSize::Small)
                                                                .color(Color::Custom(
                                                                    tab_colors.text_muted,
                                                                )),
                                                        ),
                                                )
                                                .child(
                                                    Label::new("maximized")
                                                        .size(LabelSize::Small)
                                                        .color(Color::Custom(tab_colors.text_muted)),
                                                ),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            IconButton::new(
                                                "restore-maximized-pane",
                                                IconName::Minimize,
                                            )
                                            .style(ButtonStyle::Transparent)
                                            .size(ButtonSize::Compact)
                                            .icon_size(IconSize::XSmall)
                                            .icon_color(Color::Custom(tab_colors.icon))
                                            .aria_label("Restore maximized pane")
                                            .tooltip(Tooltip::for_action_title(
                                                "Restore pane to its split position",
                                                &ToggleMaximizePane,
                                            ))
                                            .on_click(move |_, window, cx| {
                                                restore_handle
                                                    .update(cx, |this, cx| {
                                                        this.toggle_maximize_pane_by_id(
                                                            pane_id, window, cx,
                                                        );
                                                    })
                                                    .ok();
                                            }),
                                        )
                                        .child(
                                            IconButton::new(
                                                "close-maximized-pane",
                                                IconName::Close,
                                            )
                                            .style(ButtonStyle::Transparent)
                                            .size(ButtonSize::Compact)
                                            .icon_size(IconSize::XSmall)
                                            .icon_color(Color::Custom(tab_colors.icon))
                                            .aria_label("Close pane")
                                            .tooltip(Tooltip::for_action_title(
                                                "Close pane",
                                                &ClosePane,
                                            ))
                                            .on_click(move |_, window, cx| {
                                                close_handle
                                                    .update(cx, |this, cx| {
                                                        this.close_pane(
                                                            tab_id, pane_id, window, cx,
                                                        );
                                                    })
                                                    .ok();
                                            }),
                                        ),
                                ),
                        )
                    })
                    .when_some(minimized_shelf, |body, (pane_id, index, count)| {
                        let previous_handle = handle.clone();
                        let next_handle = handle.clone();
                        let close_handle = handle.clone();
                        let tab_id = tab.id;
                        let tab_index = self.active_tab;
                        let entries_handle = handle.clone();
                        let entry_colors = tab_colors.clone();
                        body.child(
                            div()
                                .id("minimized-panes-shelf")
                                .key_context("Terminal")
                                .track_focus(&self.minimized_panes_focus)
                                .h_8()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .bg(tab_colors.status_bar_background)
                                .border_t_1()
                                .border_color(tab_colors.border)
                                .when(
                                    minimized_shelf_owns_window_bottom && rounded_bottom_left,
                                    |shelf| shelf.rounded_bl(bottom_corner_radius),
                                )
                                .when(
                                    minimized_shelf_owns_window_bottom && rounded_bottom_right,
                                    |shelf| shelf.rounded_br(bottom_corner_radius),
                                )
                                .child(
                                    div()
                                        .id("minimized-panes-status")
                                        .w(px(108.))
                                        .flex_none()
                                        .overflow_hidden()
                                        .tooltip(Tooltip::text(
                                            "Use the pane controls to select or restore",
                                        ))
                                        .child(
                                            Label::new(format!(
                                                "Minimized {}/{}",
                                                index + 1,
                                                count
                                            ))
                                            .size(LabelSize::Small)
                                            .color(Color::Custom(tab_colors.text_muted))
                                            .line_clamp(1),
                                        ),
                                )
                                .when(count > 1, |shelf| {
                                    shelf.child(
                                        IconButton::new(
                                            "previous-minimized-pane",
                                            IconName::ChevronLeft,
                                        )
                                        .style(ButtonStyle::Transparent)
                                        .size(ButtonSize::Compact)
                                        .icon_size(IconSize::XSmall)
                                        .icon_color(Color::Custom(tab_colors.icon))
                                        .aria_label("Select previous minimized pane")
                                        .tooltip(Tooltip::for_action_title(
                                            "Select previous minimized pane",
                                            &SelectPreviousMinimizedPane,
                                        ))
                                        .on_click(move |_, window, cx| {
                                            previous_handle
                                                .update(cx, |this, cx| {
                                                    this.select_previous_minimized_pane(
                                                        &SelectPreviousMinimizedPane,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }),
                                    )
                                })
                                .child(
                                    container_query(move |size, _, cx| {
                                        let capacity =
                                            minimized_pane_capacity(size.width, count);
                                        let visible_entries = entries_handle
                                            .read_with(cx, |this, _| {
                                                let Some(tab) = this
                                                    .tabs
                                                    .get(tab_index)
                                                    .filter(|candidate| candidate.id == tab_id)
                                                else {
                                                    return Vec::new();
                                                };
                                                resolve_visible_minimized_panes(
                                                    tab.minimized_panes.len(),
                                                    index,
                                                    capacity,
                                                    |entry_index| {
                                                        let entry_pane_id = *tab
                                                            .minimized_panes
                                                            .get(entry_index)?;
                                                        let pane = tab.pane(entry_pane_id)?;
                                                        let pane_label = tab
                                                            .displayed_pane_label(entry_pane_id)
                                                            .unwrap_or_else(|| pane.label());
                                                        Some((
                                                            entry_index,
                                                            entry_pane_id,
                                                            format!(
                                                                "{pane_label} · {}",
                                                                pane.profile.name
                                                            ),
                                                        ))
                                                    },
                                                )
                                            })
                                            .unwrap_or_default();
                                        div()
                                            .size_full()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .overflow_hidden()
                                            .children(visible_entries.into_iter().map(
                                                |(entry_index, entry_pane_id, shelf_label)| {
                                                    let is_selected = entry_index == index;
                                                    let restore_handle = entries_handle.clone();
                                                    div()
                                                    .id((
                                                        "restore-minimized-pane",
                                                        entry_pane_id as usize,
                                                    ))
                                                    .h_6()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .px_2()
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(if is_selected {
                                                        entry_colors.border_focused
                                                    } else {
                                                        entry_colors.border
                                                    })
                                                    .bg(if is_selected {
                                                        entry_colors.element_selected
                                                    } else {
                                                        entry_colors.element_background
                                                    })
                                                    .cursor_pointer()
                                                    .overflow_hidden()
                                                    .tooltip(Tooltip::for_action_title(
                                                        if is_selected {
                                                            format!(
                                                                "{shelf_label}\nSelected minimized pane; restore"
                                                            )
                                                        } else {
                                                            format!("{shelf_label}\nRestore minimized pane")
                                                        },
                                                        &RestoreMinimizedPane,
                                                    ))
                                                    .on_click(move |_, window, cx| {
                                                        restore_handle
                                                            .update(cx, |this, cx| {
                                                                this.restore_minimized_pane_by_id(
                                                                    entry_pane_id,
                                                                    window,
                                                                    cx,
                                                                );
                                                            })
                                                            .ok();
                                                    })
                                                    .child(
                                                        Icon::new(IconName::Dash)
                                                            .size(IconSize::XSmall)
                                                            .color(Color::Custom(
                                                                entry_colors.text_accent,
                                                            )),
                                                    )
                                                    .child(
                                                        Label::new(shelf_label)
                                                            .flex_1()
                                                            .size(LabelSize::Small)
                                                            .color(Color::Custom(
                                                                entry_colors.text,
                                                            ))
                                                            .line_clamp(1),
                                                    )
                                                },
                                            ))
                                    })
                                    .h_6()
                                    .min_w_0()
                                    .flex_1(),
                                )
                                .when(count > 1, |shelf| {
                                    shelf.child(
                                        IconButton::new(
                                            "next-minimized-pane",
                                            IconName::ChevronRight,
                                        )
                                        .style(ButtonStyle::Transparent)
                                        .size(ButtonSize::Compact)
                                        .icon_size(IconSize::XSmall)
                                        .icon_color(Color::Custom(tab_colors.icon))
                                        .aria_label("Select next minimized pane")
                                        .tooltip(Tooltip::for_action_title(
                                            "Select next minimized pane",
                                            &SelectNextMinimizedPane,
                                        ))
                                        .on_click(move |_, window, cx| {
                                            next_handle
                                                .update(cx, |this, cx| {
                                                    this.select_next_minimized_pane(
                                                        &SelectNextMinimizedPane,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }),
                                    )
                                })
                                .child(
                                    IconButton::new("close-minimized-pane", IconName::Close)
                                        .style(ButtonStyle::Transparent)
                                        .size(ButtonSize::Compact)
                                        .icon_size(IconSize::XSmall)
                                        .icon_color(Color::Custom(tab_colors.icon))
                                        .aria_label("Close minimized pane")
                                        .tooltip(Tooltip::for_action_title(
                                            "Close selected minimized pane",
                                            &ClosePane,
                                        ))
                                        .on_click(move |_, window, cx| {
                                            close_handle
                                                .update(cx, |this, cx| {
                                                    this.close_pane(tab_id, pane_id, window, cx);
                                                })
                                                .ok();
                                        }),
                                ),
                        )
                    })
                    .into_any_element()
            }
            None => div().size_full().into_any_element(),
        }
    }
}

#[cfg(test)]
#[path = "tests/tab_body_render.rs"]
mod tests;
