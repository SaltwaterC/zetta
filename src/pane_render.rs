use super::*;

const PANE_RESIZE_GUTTER_SIZE: Pixels = px(20.);

fn pane_resize_menu_entry_available(pane_count: usize) -> bool {
    pane_count >= 2
}

fn pane_move_menu_entry_available(pane_count: usize) -> bool {
    pane_count >= 2
}

fn terminal_focus_placeholder(focus: &gpui::FocusHandle, content: impl IntoElement) -> gpui::Div {
    div()
        .size_full()
        .track_focus(focus)
        .key_context("Terminal")
        .child(content)
}

fn with_inactive_pane_opacity(pane: gpui::Div, active: bool, inactive_opacity: f32) -> gpui::Div {
    pane.when(!active, |pane| pane.opacity(inactive_opacity))
}

/// The badge a pane shows in its bottom-right corner while resize or move mode
/// is on. Both modes draw the same chip and differ only in what they write in
/// it, so the two callers pass a finished label.
fn pane_status_badge(label: String, colors: &ThemeColors) -> gpui::Div {
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
        .child(label)
}

/// A pane's overlay text, positioned against the pane's top-right corner.
///
/// The vertical offset is per font size on purpose: the line box sits on the
/// glyph's internal leading (measured: 6px at `sm`, 14px at `3xl`), so each size
/// offsets by `overlay_pane_inset() - leading(size)` to keep the visible gap to
/// the pane edge constant.
fn pane_overlay_element(
    pane_id: u64,
    overlay: String,
    pane: &TerminalPane,
    editing: bool,
    colors: &ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let font_size = pane.overlay_font_size.unwrap_or(OverlayFontSize::DEFAULT);
    let base_opacity = pane.overlay_opacity.unwrap_or(DEFAULT_OVERLAY_OPACITY);
    let color = pane.overlay_color.unwrap_or(colors.text);
    let top = match font_size {
        OverlayFontSize::Small => px(8.),
        OverlayFontSize::Base => px(7.),
        OverlayFontSize::Large => px(6.),
        OverlayFontSize::ExtraLarge => px(5.),
        OverlayFontSize::ExtraExtraLarge => px(3.),
        OverlayFontSize::ExtraExtraExtraLarge => px(0.),
    };
    div()
        .id(("terminal-pane-overlay", pane_id as usize))
        .absolute()
        .right(px(14.))
        .top(top)
        .max_w(px(320.))
        .map(|element| match font_size {
            OverlayFontSize::Small => element.text_sm(),
            OverlayFontSize::Base => element.text_base(),
            OverlayFontSize::Large => element.text_lg(),
            OverlayFontSize::ExtraLarge => element.text_xl(),
            OverlayFontSize::ExtraExtraLarge => element.text_2xl(),
            OverlayFontSize::ExtraExtraExtraLarge => element.text_3xl(),
        })
        .text_color(color)
        .opacity(if editing { 1. } else { base_opacity })
        .overflow_hidden()
        .child(overlay)
}

fn stacked_entry_status(entry: &StackedPane) -> String {
    match entry.state {
        StackedPaneState::Starting => "starting".to_owned(),
        StackedPaneState::Running => "running".to_owned(),
        StackedPaneState::Completed => entry
            .exit_code
            .map_or_else(|| "completed".to_owned(), |code| format!("exit {code}")),
        StackedPaneState::Failed => "failed".to_owned(),
    }
}

fn stacked_rows_container(background: gpui::Hsla) -> gpui::Div {
    div().flex_none().w_full().flex().flex_col().bg(background)
}

fn stacked_rows_backdrop(background: gpui::Hsla) -> gpui::Div {
    div().flex_none().w_full().bg(background)
}

impl Zetta {
    fn render_stacked_rows(
        &self,
        tab: &Tab,
        pane: &TerminalPane,
        colors: &ThemeColors,
        editor_background: gpui::Hsla,
        terminal_background: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let selected = pane.stack.selected;
        let mut rows = stacked_rows_container(terminal_background);

        if (!pane.base_exited || pane.exit.is_some())
            && !matches!(selected, PaneStackSelection::Base)
        {
            rows = rows.child(self.render_stacked_row(
                tab,
                pane,
                PaneStackSelection::Base,
                "Interactive shell".to_owned(),
                if pane.exit.is_some() {
                    "failed".to_owned()
                } else if pane.base_exited {
                    "exited".to_owned()
                } else {
                    "running".to_owned()
                },
                colors,
                cx,
            ));
        }

        for entry in &pane.stack.entries {
            if selected == PaneStackSelection::Stacked(entry.id) {
                continue;
            }
            rows = rows.child(self.render_stacked_row(
                tab,
                pane,
                PaneStackSelection::Stacked(entry.id),
                entry.command.clone(),
                stacked_entry_status(entry),
                colors,
                cx,
            ));
        }
        stacked_rows_backdrop(editor_background)
            .child(rows)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_stacked_row(
        &self,
        tab: &Tab,
        pane: &TerminalPane,
        selection: PaneStackSelection,
        command: String,
        status: String,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tab_id = tab.id;
        let pane_id = pane.id;
        let select_handle = cx.entity().downgrade();
        let close_handle = cx.entity().downgrade();
        let row_id = match selection {
            PaneStackSelection::Base => 0,
            PaneStackSelection::Stacked(id) => id as usize,
        };
        let close_label = match selection {
            PaneStackSelection::Base => "Close pane",
            PaneStackSelection::Stacked(_) => "Close stacked command",
        };
        div()
            .id(format!("stacked-pane-row-{pane_id}-{row_id}"))
            .h_6()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .border_b_1()
            .border_color(colors.border)
            .hover(|row| row.bg(colors.element_hover))
            .on_click(move |_, window, cx| {
                select_handle
                    .update(cx, |this, cx| {
                        this.select_stacked_pane_by_id(tab_id, pane_id, selection, window, cx);
                    })
                    .ok();
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .text_color(colors.text)
                    .child(command),
            )
            .child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child(status),
            )
            .child(
                IconButton::new(
                    format!("close-stacked-pane-{pane_id}-{row_id}"),
                    IconName::Close,
                )
                .style(ButtonStyle::Transparent)
                .size(ButtonSize::Compact)
                .icon_size(IconSize::XSmall)
                .icon_color(Color::Custom(colors.icon))
                .aria_label(close_label)
                .tooltip(Tooltip::text(close_label))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    close_handle
                        .update(cx, |this, cx| match selection {
                            PaneStackSelection::Base => {
                                this.close_pane(tab_id, pane_id, window, cx);
                            }
                            PaneStackSelection::Stacked(entry_id) => {
                                this.close_stacked_pane_by_id(
                                    tab_id, pane_id, entry_id, window, cx,
                                );
                            }
                        })
                        .ok();
                }),
            )
            .into_any_element()
    }

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
                PaneLayoutContext {
                    tab,
                    colors,
                    error_color,
                    corner_radius,
                },
                layout,
                edges,
                window,
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

    /// Renders a tab's pane tree.
    ///
    /// `edges` is which of the window's own edges this subtree touches, so a
    /// pane at a corner can round the client corner it owns; it is narrowed as
    /// the tree is walked rather than recomputed from the layout.
    fn render_pane_layout_with_edges(
        &self,
        context: PaneLayoutContext<'_>,
        layout: &PaneLayout,
        edges: PaneWindowEdges,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match layout {
            PaneLayout::Pane(pane_id) => {
                self.render_pane_leaf(context, *pane_id, edges, window, cx)
            }
            PaneLayout::Split {
                axis,
                first_ratio,
                first,
                second,
            } => self.render_pane_split(
                context,
                *axis,
                *first_ratio,
                first,
                second,
                edges,
                window,
                cx,
            ),
        }
    }

    /// One pane: its terminal (or the stack entry it is showing), its label,
    /// its overlay, and the window corners it owns.
    fn render_pane_leaf(
        &self,
        context: PaneLayoutContext<'_>,
        pane_id: u64,
        edges: PaneWindowEdges,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let PaneLayoutContext { tab, colors, .. } = context;
        let Some(pane) = tab.pane(pane_id) else {
            return div().size_full().into_any_element();
        };
        let pane_label = tab
            .displayed_pane_label(pane_id)
            .unwrap_or_else(|| pane.label());
        let pane_size = pane.selected_terminal().map(|terminal| {
            let bounds = terminal.read(cx).last_content().terminal_bounds;
            terminal_size_label(bounds.num_columns(), bounds.num_lines())
        });
        let content = self.render_pane_body(context, pane, edges, window, cx);
        div()
            .id(("terminal-pane", pane_id as usize))
            .relative()
            .when(
                tab.panes.len() > 1 && tab.maximized_pane.is_none(),
                |pane| {
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
            .child(content)
            .when_some(
                self.pane_resize_mode.then_some(pane_size.clone()).flatten(),
                |element, pane_size| {
                    element.child(pane_status_badge(
                        format!("{pane_label} {pane_size}"),
                        colors,
                    ))
                },
            )
            .when(self.pane_move_mode, |element| {
                let overlay_label = if tab.active_pane == pane_id {
                    format!("{pane_label} Move mode")
                } else {
                    pane_label.clone()
                };
                element.child(pane_status_badge(overlay_label, colors))
            })
            .when_some(tab.displayed_pane_overlay(pane_id), |element, overlay| {
                element.child(pane_overlay_element(
                    pane_id,
                    overlay,
                    pane,
                    tab.editing_overlay_pane == Some(pane_id),
                    colors,
                ))
            })
            .when(
                tab.maximized_pane.is_none()
                    && pane.stack.is_empty()
                    && (tab.renaming_pane == Some(pane_id)
                        || (tab.panes.len() > 1
                            && self.pane_controls_visible_for == Some(pane_id))),
                |element| {
                    element.child(
                        self.render_pane_controls(context, pane_id, pane_label, pane_size, cx),
                    )
                },
            )
            .when(
                self.pane_move_mode && tab.panes.len() > 1 && tab.maximized_pane.is_none(),
                |pane| {
                    let pane_move_drag = PaneMoveDrag {
                        tab_id: tab.id,
                        pane_id,
                    };
                    // A dedicated top-most overlay, rather than handlers on the
                    // pane itself, so `occlude` can block every mouse
                    // interaction with the terminal underneath (selection,
                    // clicks, scroll) while move mode is active: the pane must
                    // act as a plain drag handle, not a terminal.
                    pane.child(
                        div()
                            .id(("pane-move-drag-surface", pane_id as usize))
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

    /// What fills a pane: the terminal view it is showing, or the message that
    /// stands in for one, under any stacked-command rows and the inactive-pane
    /// dimming.
    fn render_pane_body(
        &self,
        context: PaneLayoutContext<'_>,
        pane: &TerminalPane,
        edges: PaneWindowEdges,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let PaneLayoutContext {
            tab,
            colors,
            error_color,
            corner_radius,
        } = context;
        let pane_id = pane.id;
        let corner_radii = edges.client_corner_radii(window, corner_radius);
        let selected_view = pane.selected_view();
        let (editor_background, terminal_background) = selected_view
            .as_ref()
            .and_then(|view| {
                view.read(cx).theme().map(|theme| {
                    let colors = theme.colors();
                    (colors.editor_background, colors.terminal_background)
                })
            })
            .unwrap_or((colors.editor_background, colors.terminal_background));
        let active = selected_view.as_ref().is_some_and(|view| {
            view.focus_handle(cx).is_focused(window)
                || view.read(cx).has_open_context_menu()
                || view.read(cx).has_open_search()
                || self
                    .tab_search
                    .as_ref()
                    .is_some_and(|search| search.tab_id == tab.id && tab.active_pane == pane_id)
        }) || (selected_view.is_none() && tab.active_pane == pane_id);
        let pane_resize_toggle_action = pane_resize_menu_entry_available(tab.panes.len())
            .then(|| Box::new(TogglePaneResizeMode) as Box<dyn Action>);
        let pane_move_toggle_action = pane_move_menu_entry_available(tab.panes.len())
            .then(|| Box::new(TogglePaneMoveMode) as Box<dyn Action>);
        let selected_error = match pane.stack.selected {
            PaneStackSelection::Base => pane.error.as_ref(),
            PaneStackSelection::Stacked(_) => pane
                .stack
                .selected_entry()
                .and_then(|entry| entry.error.as_ref()),
        };
        let selected_profile_name = match pane.stack.selected {
            PaneStackSelection::Base => pane.profile.name.clone(),
            PaneStackSelection::Stacked(_) => pane.stack.selected_entry().map_or_else(
                || pane.profile.name.clone(),
                |entry| entry.profile.name.clone(),
            ),
        };
        let content = match (&selected_view, selected_error) {
            (Some(view), _) => {
                view.update(cx, |view, cx| {
                    view.set_window_corner_radii(corner_radii, cx);
                    view.set_pane_resize_mode_entry(
                        self.pane_resize_mode,
                        pane_resize_toggle_action,
                    );
                    view.set_pane_move_mode_entry(self.pane_move_mode, pane_move_toggle_action);
                });
                // Cached so a frame the terminal did not cause — an overlay
                // scrolling, a title-bar hover, a tab rename — does not re-lay
                // out every visible cell of every pane. GPUI busts the cache on
                // `cx.notify()` from the view (output, blink, focus, theme),
                // on a bounds/text-style change, and on `cx.refresh()`.
                div()
                    .size_full()
                    .child(
                        view.clone()
                            .cached(gpui::StyleRefinement::default().size_full()),
                    )
                    .into_any_element()
            }
            (_, Some(error)) => {
                let heading = if error.starts_with("Run:") {
                    "Run"
                } else {
                    pane.exit
                        .as_ref()
                        .map_or("Unable to start command", |exit| exit.heading())
                };
                div()
                    .size_full()
                    .p_4()
                    .bg(colors.editor_background)
                    .text_color(error_color)
                    .child(heading)
                    .child(div().mt_2().text_sm().child(error.clone()))
                    .into_any_element()
            }
            (None, _) if pane.base_exited && pane.stack.selected_is_base() => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.editor_background)
                .text_color(colors.text_muted)
                .child("Interactive shell exited")
                .into_any_element(),
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.editor_background)
                .text_color(colors.text_muted)
                .child(format!("Starting {selected_profile_name}..."))
                .into_any_element(),
        };
        let content = if selected_view.is_none() && tab.active_pane == pane_id {
            terminal_focus_placeholder(&self.terminal_placeholder_focus, content).into_any_element()
        } else {
            content
        };
        let content = if pane.stack.is_empty() {
            content
        } else {
            div()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.render_stacked_rows(
                    tab,
                    pane,
                    colors,
                    editor_background,
                    terminal_background,
                    cx,
                ))
                .child(
                    div()
                        .min_w_0()
                        .min_h_0()
                        .flex_grow_1()
                        .flex_basis(gpui::relative(0.))
                        .child(content),
                )
                .into_any_element()
        };
        with_inactive_pane_opacity(
            div().size_full().child(content),
            active,
            self.projects
                .config_for_pane(tab.active_pane)
                .map_or(self.launch_config.inactive_pane_opacity, |project| {
                    project.effective.inactive_pane_opacity
                }),
        )
        .into_any_element()
    }

    /// The controls a pane shows while it is hovered or being renamed: its
    /// label, and — when the tab has more than one pane — its grid size and the
    /// minimize/maximize/close buttons.
    ///
    /// A single pane being renamed reaches this too, which is why the buttons
    /// keep their own `panes.len() > 1` test rather than inheriting the
    /// caller's.
    fn render_pane_controls(
        &self,
        context: PaneLayoutContext<'_>,
        pane_id: u64,
        pane_label: String,
        pane_size: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let PaneLayoutContext { tab, colors, .. } = context;
        let pane_label_selected = tab.pane_rename_selected(pane_id);
        let maximize_handle = cx.entity().downgrade();
        let minimize_handle = cx.entity().downgrade();
        let close_handle = cx.entity().downgrade();
        let rename_handle = cx.entity().downgrade();
        let tab_id = tab.id;
        let maximize_pane_id = pane_id;
        let minimize_pane_id = pane_id;
        let close_pane_id = pane_id;
        let rename_pane_id = pane_id;
        let pane_label_tooltip = format!("{pane_label}\nDouble-click to label this pane");
        div()
            .absolute()
            .top(px(4.))
            .when(
                self.launch_config.pane_controls_position == PaneControlsPosition::Left,
                |controls| controls.left(px(4.)),
            )
            .when(
                self.launch_config.pane_controls_position == PaneControlsPosition::Right,
                |controls| controls.right(px(4.)),
            )
            .flex()
            .when(
                self.launch_config.pane_controls_position == PaneControlsPosition::Left,
                |controls| controls.flex_row_reverse(),
            )
            .items_center()
            .gap_1()
            .child(
                div()
                    .id(("terminal-pane-label", pane_id as usize))
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
                    .tooltip(Tooltip::for_action_title(pane_label_tooltip, &RenamePane))
                    .on_click(move |event, window, cx| {
                        if event.click_count() == 2 {
                            cx.stop_propagation();
                            rename_handle
                                .update(cx, |this, cx| {
                                    this.begin_pane_rename(rename_pane_id, window, cx);
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
                    .when_some(pane_size, |controls, pane_size| {
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
                                    ("minimize-terminal-pane", pane_id as usize),
                                    IconName::Dash,
                                )
                                .style(ButtonStyle::Transparent)
                                .size(ButtonSize::Compact)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Custom(colors.icon))
                                .aria_label("Minimize pane")
                                .tooltip(Tooltip::for_action_title("Minimize pane", &MinimizePane))
                                .on_click(move |_, window, cx| {
                                    minimize_handle
                                        .update(cx, |this, cx| {
                                            this.minimize_pane_by_id(minimize_pane_id, window, cx);
                                        })
                                        .ok();
                                }),
                            )
                            .child(
                                IconButton::new(
                                    ("maximize-terminal-pane", pane_id as usize),
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
                        IconButton::new(("close-terminal-pane", pane_id as usize), IconName::Close)
                            .style(ButtonStyle::Transparent)
                            .size(ButtonSize::Compact)
                            .icon_size(IconSize::XSmall)
                            .icon_color(Color::Custom(colors.icon))
                            .aria_label("Close pane")
                            .tooltip(Tooltip::for_action_title("Close pane", &ClosePane))
                            .on_click(move |_, window, cx| {
                                close_handle
                                    .update(cx, |this, cx| {
                                        this.close_pane(tab_id, close_pane_id, window, cx);
                                    })
                                    .ok();
                            }),
                    )
            })
            .into_any_element()
    }

    /// One split: the two subtrees at their ratio, and the gutter that resizes
    /// them while pane-resize mode is on.
    #[allow(clippy::too_many_arguments)]
    fn render_pane_split(
        &self,
        context: PaneLayoutContext<'_>,
        axis: SplitAxis,
        first_ratio: u16,
        first: &PaneLayout,
        second: &PaneLayout,
        edges: PaneWindowEdges,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let PaneLayoutContext { tab, colors, .. } = context;
        let first_ratio = PaneLayout::ratio_fraction(first_ratio);
        let second_ratio = 1. - first_ratio;
        let pane_resize_enabled =
            self.pane_resize_mode && tab.maximized_pane.is_none() && tab.minimized_panes.is_empty();
        let gutter = PaneResizeGutter {
            tab_id: tab.id,
            first_pane: first.first_pane(),
            second_pane: second.first_pane(),
            axis,
        };
        let first_child = div()
            .min_w_0()
            .min_h_0()
            .flex_grow(first_ratio)
            .flex_basis(gpui::relative(0.))
            .child(self.render_pane_layout_with_edges(
                context,
                first,
                edges.first(axis),
                window,
                cx,
            ));
        let second_child = div()
            .min_w_0()
            .min_h_0()
            .flex_grow(second_ratio)
            .flex_basis(gpui::relative(0.))
            .child(self.render_pane_layout_with_edges(
                context,
                second,
                edges.second(axis),
                window,
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

/// What every level of a pane layout renders against.
///
/// Only the layout node and its window edges change as the tree is walked, so
/// the rest travels as one `Copy` bundle rather than as five parameters
/// threaded through each recursion.
#[derive(Clone, Copy)]
struct PaneLayoutContext<'a> {
    tab: &'a Tab,
    colors: &'a ThemeColors,
    error_color: gpui::Hsla,
    corner_radius: Pixels,
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
