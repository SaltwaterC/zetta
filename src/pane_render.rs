use super::*;

const PANE_RESIZE_GUTTER_SIZE: Pixels = px(20.);

fn pane_resize_menu_entry_available(pane_count: usize) -> bool {
    pane_count >= 2
}

fn pane_move_menu_entry_available(pane_count: usize) -> bool {
    pane_count >= 2
}

impl Zetta {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_pane_layout(
        &self,
        tab: &Tab,
        layout: &PaneLayout,
        colors: &ThemeColors,
        error_color: gpui::Hsla,
        window: &Window,
        owns_window_bottom: bool,
        corner_radius: Pixels,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let edges = PaneWindowEdges::all().with_bottom(owns_window_bottom);
        let corner_radii = edges.client_corner_radii(window, corner_radius);
        div()
            .when(self.pane_resize_mode, |layout| {
                layout.key_context("PaneResize")
            })
            .when(self.pane_move_mode, |layout| layout.key_context("PaneMove"))
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex_grow_1()
            .flex_basis(gpui::relative(0.))
            .overflow_hidden()
            // Use one opaque surface behind every pane layout. This fills
            // terminal-grid margins and pane separators consistently while
            // retaining the outer client-window corners.
            .when(corner_radii.bottom_left > Pixels::ZERO, |layout| {
                layout.rounded_bl(corner_radii.bottom_left)
            })
            .when(corner_radii.bottom_right > Pixels::ZERO, |layout| {
                layout.rounded_br(corner_radii.bottom_right)
            })
            .bg(colors.border)
            .child(self.render_pane_layout_with_edges(
                tab,
                layout,
                colors,
                error_color,
                window,
                edges,
                corner_radius,
                cx,
            ))
            .into_any_element()
    }

    fn render_pane_resize_gutter(
        &self,
        gutter: PaneResizeGutter,
        first_ratio: f32,
        colors: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let cursor = match gutter.axis {
            SplitAxis::Vertical => CursorStyle::ResizeLeftRight,
            SplitAxis::Horizontal => CursorStyle::ResizeUpDown,
        };
        div()
            .id(format!(
                "pane-resize-gutter-{}-{}-{}",
                gutter.tab_id, gutter.first_pane, gutter.second_pane
            ))
            .absolute()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .hover(|gutter| gutter.bg(colors.element_hover))
            .cursor(cursor)
            .when(matches!(gutter.axis, SplitAxis::Vertical), |gutter| {
                gutter
                    .left(gpui::relative(first_ratio))
                    .ml(-PANE_RESIZE_GUTTER_SIZE / 2.)
                    .w(PANE_RESIZE_GUTTER_SIZE)
                    .h_full()
            })
            .when(matches!(gutter.axis, SplitAxis::Horizontal), |gutter| {
                gutter
                    .top(gpui::relative(first_ratio))
                    .mt(-PANE_RESIZE_GUTTER_SIZE / 2.)
                    .h(PANE_RESIZE_GUTTER_SIZE)
                    .w_full()
            })
            .on_drag(gutter, |_, _, _, cx| cx.new(|_| gpui::Empty))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_pane_layout_with_edges(
        &self,
        tab: &Tab,
        layout: &PaneLayout,
        colors: &ThemeColors,
        error_color: gpui::Hsla,
        window: &Window,
        edges: PaneWindowEdges,
        corner_radius: Pixels,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match layout {
            PaneLayout::Pane(pane_id) => {
                let Some(pane) = tab.pane(*pane_id) else {
                    return div().size_full().into_any_element();
                };
                let corner_radii = edges.client_corner_radii(window, corner_radius);
                let pane_label = tab
                    .displayed_pane_label(*pane_id)
                    .unwrap_or_else(|| pane.label());
                let pane_overlay = tab.displayed_pane_overlay(*pane_id);
                let pane_terminal = pane.terminal.as_ref();
                let pane_size = pane_terminal.map(|terminal| {
                    let bounds = terminal.read(cx).last_content().terminal_bounds;
                    terminal_size_label(bounds.num_columns(), bounds.num_lines())
                });
                let pane_label_selected = tab.renaming_pane == Some(*pane_id)
                    && tab.rename_select_all
                    && tab.rename_buffer.is_some();
                let pane_overlay_editing = tab.editing_overlay_pane == Some(*pane_id);
                let pane_overlay_font_size =
                    pane.overlay_font_size.unwrap_or(OverlayFontSize::DEFAULT);
                let pane_overlay_base_opacity =
                    pane.overlay_opacity.unwrap_or(DEFAULT_OVERLAY_OPACITY);
                let pane_overlay_color = pane.overlay_color.unwrap_or(colors.text);
                let pane_overlay_top = match pane_overlay_font_size {
                    // The line box sits on the glyph's internal leading
                    // (measured: 6px at `sm`, 14px at `3xl`), so each size
                    // offsets by `overlay_pane_inset() - leading(size)` to keep
                    // the visible gap to the pane edge constant.
                    OverlayFontSize::Small => px(8.),
                    OverlayFontSize::Base => px(7.),
                    OverlayFontSize::Large => px(6.),
                    OverlayFontSize::ExtraLarge => px(5.),
                    OverlayFontSize::ExtraExtraLarge => px(3.),
                    OverlayFontSize::ExtraExtraExtraLarge => px(0.),
                };
                let active = pane.view.as_ref().is_some_and(|view| {
                    view.focus_handle(cx).is_focused(window)
                        || view.read(cx).has_open_context_menu()
                        || view.read(cx).has_open_search()
                        || self.tab_search.as_ref().is_some_and(|search| {
                            search.tab_id == tab.id && tab.active_pane == *pane_id
                        })
                }) || (pane.view.is_none() && tab.active_pane == *pane_id);
                let pane_resize_toggle_action = pane_resize_menu_entry_available(tab.panes.len())
                    .then(|| Box::new(TogglePaneResizeMode) as Box<dyn Action>);
                let pane_move_toggle_action = pane_move_menu_entry_available(tab.panes.len())
                    .then(|| Box::new(TogglePaneMoveMode) as Box<dyn Action>);
                let content = match (&pane.view, &pane.error) {
                    (Some(view), _) => {
                        view.update(cx, |view, cx| {
                            view.set_window_corner_radii(corner_radii, cx);
                            view.set_pane_resize_mode_entry(
                                self.pane_resize_mode,
                                pane_resize_toggle_action,
                            );
                            view.set_pane_move_mode_entry(
                                self.pane_move_mode,
                                pane_move_toggle_action,
                            );
                        });
                        div().size_full().child(view.clone()).into_any_element()
                    }
                    (_, Some(error)) => div()
                        .size_full()
                        .p_4()
                        .bg(colors.editor_background)
                        .text_color(error_color)
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
                    .relative()
                    .when(
                        tab.panes.len() > 1 && tab.maximized_pane.is_none(),
                        |pane| {
                            let pane_id = *pane_id;
                            pane.on_mouse_move(cx.listener(move |this, _, window, cx| {
                                this.show_pane_controls(pane_id, window, cx);
                            }))
                        },
                    )
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow_1()
                    .flex_basis(gpui::relative(0.))
                    .overflow_hidden()
                    .child(
                        div()
                            .size_full()
                            .when(!active, |pane| {
                                pane.opacity(self.launch_config.inactive_pane_opacity)
                            })
                            .child(content),
                    )
                    .when_some(
                        self.pane_resize_mode.then_some(pane_size.clone()).flatten(),
                        |pane, pane_size| {
                            pane.child(
                                div()
                                    .absolute()
                                    .right(px(6.))
                                    .bottom(px(6.))
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.status_bar_background)
                                    .text_sm()
                                    .text_color(colors.text)
                                    .child(format!("{pane_label} {pane_size}")),
                            )
                        },
                    )
                    .when(self.pane_move_mode, |pane| {
                        let overlay_label = if tab.active_pane == *pane_id {
                            format!("{pane_label} Move mode")
                        } else {
                            pane_label.clone()
                        };
                        pane.child(
                            div()
                                .absolute()
                                .right(px(6.))
                                .bottom(px(6.))
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .bg(colors.status_bar_background)
                                .text_sm()
                                .text_color(colors.text)
                                .child(overlay_label),
                        )
                    })
                    .when_some(pane_overlay, |pane, overlay| {
                        pane.child(
                            div()
                                .id(("terminal-pane-overlay", *pane_id as usize))
                                .absolute()
                                .right(px(14.))
                                .top(pane_overlay_top)
                                .max_w(px(320.))
                                .map(|element| match pane_overlay_font_size {
                                    OverlayFontSize::Small => element.text_sm(),
                                    OverlayFontSize::Base => element.text_base(),
                                    OverlayFontSize::Large => element.text_lg(),
                                    OverlayFontSize::ExtraLarge => element.text_xl(),
                                    OverlayFontSize::ExtraExtraLarge => element.text_2xl(),
                                    OverlayFontSize::ExtraExtraExtraLarge => element.text_3xl(),
                                })
                                .text_color(pane_overlay_color)
                                .opacity(if pane_overlay_editing {
                                    1.
                                } else {
                                    pane_overlay_base_opacity
                                })
                                .overflow_hidden()
                                .child(overlay),
                        )
                    })
                    .when(
                        tab.maximized_pane.is_none()
                            && (tab.renaming_pane == Some(*pane_id)
                                || (tab.panes.len() > 1
                                    && self.pane_controls_visible_for == Some(*pane_id))),
                        |pane| {
                            let maximize_handle = cx.entity().downgrade();
                            let minimize_handle = cx.entity().downgrade();
                            let close_handle = cx.entity().downgrade();
                            let rename_handle = cx.entity().downgrade();
                            let tab_id = tab.id;
                            let maximize_pane_id = *pane_id;
                            let minimize_pane_id = *pane_id;
                            let close_pane_id = *pane_id;
                            let rename_pane_id = *pane_id;
                            let pane_label_tooltip =
                                format!("{pane_label}\nDouble-click to label this pane");
                            pane.child(
                                div()
                                    .absolute()
                                    .top(px(4.))
                                    .when(
                                        self.launch_config.pane_controls_position
                                            == PaneControlsPosition::Left,
                                        |controls| controls.left(px(4.)),
                                    )
                                    .when(
                                        self.launch_config.pane_controls_position
                                            == PaneControlsPosition::Right,
                                        |controls| controls.right(px(4.)),
                                    )
                                    .flex()
                                    .when(
                                        self.launch_config.pane_controls_position
                                            == PaneControlsPosition::Left,
                                        |controls| controls.flex_row_reverse(),
                                    )
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .id(("terminal-pane-label", *pane_id as usize))
                                            .h_6()
                                            .max_w(px(240.))
                                            .flex()
                                            .items_center()
                                            .px_2()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(colors.border)
                                            .bg(colors.status_bar_background)
                                            .when(pane_label_selected, |label| {
                                                label.bg(colors.element_selected)
                                            })
                                            .cursor_text()
                                            .overflow_hidden()
                                            .tooltip(Tooltip::for_action_title(
                                                pane_label_tooltip,
                                                &RenamePane,
                                            ))
                                            .on_click(move |event, window, cx| {
                                                if event.click_count() == 2 {
                                                    cx.stop_propagation();
                                                    rename_handle
                                                        .update(cx, |this, cx| {
                                                            this.begin_pane_rename(
                                                                rename_pane_id,
                                                                window,
                                                                cx,
                                                            );
                                                        })
                                                        .ok();
                                                }
                                            })
                                            .child(
                                                Label::new(pane_label)
                                                    .size(LabelSize::Small)
                                                    .color(Color::Custom(colors.text_muted)),
                                            ),
                                    )
                                    .when(tab.panes.len() > 1, |controls| {
                                        controls
                                            .when_some(pane_size.clone(), |controls, pane_size| {
                                                controls.child(
                                                    Label::new(pane_size)
                                                        .size(LabelSize::Small)
                                                        .color(Color::Custom(colors.text_muted)),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(
                                                        IconButton::new(
                                                            (
                                                                "minimize-terminal-pane",
                                                                *pane_id as usize,
                                                            ),
                                                            IconName::Dash,
                                                        )
                                                        .style(ButtonStyle::Transparent)
                                                        .size(ButtonSize::Compact)
                                                        .icon_size(IconSize::XSmall)
                                                        .icon_color(Color::Custom(colors.icon))
                                                        .aria_label("Minimize pane")
                                                        .tooltip(Tooltip::for_action_title(
                                                            "Minimize pane",
                                                            &MinimizePane,
                                                        ))
                                                        .on_click(move |_, window, cx| {
                                                            minimize_handle
                                                                .update(cx, |this, cx| {
                                                                    this.minimize_pane_by_id(
                                                                        minimize_pane_id,
                                                                        window,
                                                                        cx,
                                                                    );
                                                                })
                                                                .ok();
                                                        }),
                                                    )
                                                    .child(
                                                        IconButton::new(
                                                            (
                                                                "maximize-terminal-pane",
                                                                *pane_id as usize,
                                                            ),
                                                            IconName::Maximize,
                                                        )
                                                        .style(ButtonStyle::Transparent)
                                                        .size(ButtonSize::Compact)
                                                        .icon_size(IconSize::XSmall)
                                                        .icon_color(Color::Custom(colors.icon))
                                                        .aria_label("Maximize pane")
                                                        .tooltip(Tooltip::for_action_title(
                                                            "Maximize pane",
                                                            &ToggleMaximizePane,
                                                        ))
                                                        .on_click(move |_, window, cx| {
                                                            maximize_handle
                                                                .update(cx, |this, cx| {
                                                                    this.toggle_maximize_pane_by_id(
                                                                        maximize_pane_id,
                                                                        window,
                                                                        cx,
                                                                    );
                                                                })
                                                                .ok();
                                                        }),
                                                    ),
                                            )
                                            .child(
                                                IconButton::new(
                                                    ("close-terminal-pane", *pane_id as usize),
                                                    IconName::Close,
                                                )
                                                .style(ButtonStyle::Transparent)
                                                .size(ButtonSize::Compact)
                                                .icon_size(IconSize::XSmall)
                                                .icon_color(Color::Custom(colors.icon))
                                                .aria_label("Close pane")
                                                .tooltip(Tooltip::for_action_title(
                                                    "Close pane",
                                                    &ClosePane,
                                                ))
                                                .on_click(move |_, window, cx| {
                                                    close_handle
                                                        .update(cx, |this, cx| {
                                                            this.close_pane(
                                                                tab_id,
                                                                close_pane_id,
                                                                window,
                                                                cx,
                                                            );
                                                        })
                                                        .ok();
                                                }),
                                            )
                                    }),
                            )
                        },
                    )
                    .when(
                        self.pane_move_mode && tab.panes.len() > 1 && tab.maximized_pane.is_none(),
                        |pane| {
                            let pane_move_drag = PaneMoveDrag {
                                tab_id: tab.id,
                                pane_id: *pane_id,
                            };
                            // A dedicated top-most overlay, rather than handlers on the
                            // pane itself, so `occlude` can block every mouse
                            // interaction with the terminal underneath (selection,
                            // clicks, scroll) while move mode is active: the pane must
                            // act as a plain drag handle, not a terminal.
                            pane.child(
                                div()
                                    .id(("pane-move-drag-surface", *pane_id as usize))
                                    .absolute()
                                    .inset_0()
                                    .cursor(CursorStyle::OpenHand)
                                    .occlude()
                                    .on_drag(pane_move_drag, |_, _, _, cx| cx.new(|_| gpui::Empty))
                                    .on_drop(cx.listener(
                                        move |this, dragged: &PaneMoveDrag, _window, cx| {
                                            this.move_pane_via_drag(*dragged, pane_move_drag, cx);
                                        },
                                    )),
                            )
                        },
                    )
                    .into_any_element()
            }
            PaneLayout::Split {
                axis,
                first_ratio,
                first,
                second,
            } => {
                let first_ratio = PaneLayout::ratio_fraction(*first_ratio);
                let second_ratio = 1. - first_ratio;
                let pane_resize_enabled = self.pane_resize_mode
                    && tab.maximized_pane.is_none()
                    && tab.minimized_panes.is_empty();
                let gutter = PaneResizeGutter {
                    tab_id: tab.id,
                    first_pane: first.first_pane(),
                    second_pane: second.first_pane(),
                    axis: *axis,
                };
                let first_child = div()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow(first_ratio)
                    .flex_basis(gpui::relative(0.))
                    .child(self.render_pane_layout_with_edges(
                        tab,
                        first,
                        colors,
                        error_color,
                        window,
                        edges.first(*axis),
                        corner_radius,
                        cx,
                    ));
                let second_child = div()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow(second_ratio)
                    .flex_basis(gpui::relative(0.))
                    .child(self.render_pane_layout_with_edges(
                        tab,
                        second,
                        colors,
                        error_color,
                        window,
                        edges.second(*axis),
                        corner_radius,
                        cx,
                    ));
                let split = div()
                    .relative()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow_1()
                    .flex_basis(gpui::relative(0.))
                    .flex()
                    .when(matches!(axis, SplitAxis::Horizontal), |split| {
                        split.flex_col()
                    })
                    .gap_px();
                if pane_resize_enabled {
                    split
                        .on_drag_move::<PaneResizeGutter>(cx.listener(
                            move |this, event: &gpui::DragMoveEvent<PaneResizeGutter>, _, cx| {
                                if *event.drag(cx) == gutter {
                                    this.resize_pane_gutter_drag(
                                        gutter,
                                        event.bounds,
                                        event.event.position,
                                        cx,
                                    );
                                }
                            },
                        ))
                        .child(first_child)
                        .child(second_child)
                        .child(self.render_pane_resize_gutter(gutter, first_ratio, colors, cx))
                        .into_any_element()
                } else {
                    split
                        .child(first_child)
                        .child(second_child)
                        .into_any_element()
                }
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct PaneWindowEdges {
    right: bool,
    bottom: bool,
    left: bool,
}

impl PaneWindowEdges {
    const fn all() -> Self {
        Self {
            right: true,
            bottom: true,
            left: true,
        }
    }

    const fn with_bottom(mut self, bottom: bool) -> Self {
        self.bottom = bottom;
        self
    }

    fn first(self, axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::Horizontal => Self {
                bottom: false,
                ..self
            },
            SplitAxis::Vertical => Self {
                right: false,
                ..self
            },
        }
    }

    fn second(self, axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::Horizontal => self,
            SplitAxis::Vertical => Self {
                left: false,
                ..self
            },
        }
    }

    fn client_corner_radii(self, window: &Window, corner_radius: Pixels) -> gpui::Corners<Pixels> {
        if !cfg!(linux_like) {
            return gpui::Corners::default();
        }
        let Decorations::Client { tiling } = window.window_decorations() else {
            return gpui::Corners::default();
        };
        // The title and tab bars own the top window corners. A terminal pane
        // can only meet the client frame at the bottom, so applying top radii
        // here creates an internal gap above a pane (and in split layouts).
        gpui::Corners {
            top_left: Pixels::ZERO,
            top_right: Pixels::ZERO,
            bottom_right: if self.bottom && self.right && !tiling.bottom && !tiling.right {
                corner_radius
            } else {
                Pixels::ZERO
            },
            bottom_left: if self.bottom && self.left && !tiling.bottom && !tiling.left {
                corner_radius
            } else {
                Pixels::ZERO
            },
        }
    }
}

#[cfg(test)]
#[path = "tests/pane_render.rs"]
mod tests;
