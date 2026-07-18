use super::*;

impl Render for Zetta {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let error_color = cx.theme().status().error;
        let handle = cx.entity().downgrade();
        let broadcast_input = self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.broadcast_input);
        let supported_controls = window.window_controls();
        let is_maximized = window.is_maximized();
        let client_decorations = matches!(window.window_decorations(), Decorations::Client { .. });
        let left_window_controls = render_window_controls(
            self.button_layout.left,
            supported_controls,
            is_maximized,
            false,
            client_decorations,
            cx,
        );
        let right_window_controls = render_window_controls(
            self.button_layout.right,
            supported_controls,
            is_maximized,
            true,
            client_decorations,
            cx,
        );
        let title_bar_background = if cfg!(any(target_os = "linux", target_os = "freebsd"))
            && !window.is_window_active()
        {
            colors.title_bar_inactive_background
        } else {
            colors.title_bar_background
        };
        let title_bar = div()
            .id("zetta-title-bar")
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .h(platform_title_bar_height(window))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(title_bar_background)
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
                let title = if let Some(buffer) = tab.rename_buffer.as_ref() {
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
                let full_title = if let Some(buffer) = tab.rename_buffer.as_ref() {
                    buffer.clone().into()
                } else if let Some(custom_title) = tab.custom_title.as_ref() {
                    custom_title.clone().into()
                } else if let Some(view) = tab.active_pane().and_then(|pane| pane.view.as_ref()) {
                    view.read(cx).tab_content_text(1, cx)
                } else {
                    tab.active_pane()
                        .map(|pane| pane.profile.name.clone())
                        .unwrap_or_else(|| "Terminal".to_string())
                        .into()
                };
                let content = h_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        svg()
                            .path(IconName::Terminal.path())
                            .size(px(14.))
                            .flex_none()
                            .text_color(tab_icon),
                    )
                    .child(
                        div()
                            .id(("tab-title", tab.id as usize))
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .when(
                                tab.rename_buffer.is_some() && tab.rename_select_all,
                                |title| title.bg(tab_colors.element_selection_background),
                            )
                            .tooltip(Tooltip::text(full_title))
                            .text_color(tab_text)
                            .child(title),
                    )
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
                    .border_color(tab_colors.border)
                    .bg(tab_background)
                    .when(selected, |this| {
                        this.border_t_2().border_color(tab_colors.text_accent)
                    })
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
                            .tooltip(Tooltip::text("Close tab"))
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
            })
            .collect::<Vec<_>>();

        let profile_menu_profiles = self.profiles.clone();
        let default_profile = self.launch_config.default_profile;
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
                        let is_default = index == default_profile;
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
                                            .child(Label::new(label_for_row.clone()).color(
                                                if is_default {
                                                    Color::Accent
                                                } else {
                                                    Color::Default
                                                },
                                            )),
                                    )
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
            Some(tab) => self.render_pane_layout(tab, &tab.layout, &colors, window, cx),
            None => div().size_full().into_any_element(),
        };
        let performance_overlay = self.performance_overlay.as_ref().map(|overlay| {
            let metrics = overlay.metrics;
            let rows = [
                ("Draw FPS", format!("{:.1}", metrics.draw_fps)),
                (
                    "Frame avg / p95",
                    format!(
                        "{:.2} / {:.2} ms",
                        metrics.average_draw_ms, metrics.p95_draw_ms
                    ),
                ),
                (
                    "Invalidation avg",
                    format!("{:.2} ms", metrics.average_latency_ms),
                ),
                ("Frames > 8.3 ms", metrics.slow_120_hz.to_string()),
                ("Frames > 16.7 ms", metrics.slow_60_hz.to_string()),
                (
                    "Window",
                    if window.is_window_active() {
                        "Active".to_owned()
                    } else {
                        "Inactive".to_owned()
                    },
                ),
            ];
            div()
                .id("performance-overlay")
                .absolute()
                .top(px(74.))
                .right(px(10.))
                .w(px(232.))
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .rounded(px(4.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.elevated_surface_background.opacity(0.96))
                .shadow_sm()
                .text_sm()
                .text_color(colors.text)
                .child(
                    div()
                        .pb_1()
                        .border_b_1()
                        .border_color(colors.border)
                        .child("Performance"),
                )
                .children(rows.into_iter().map(|(label, value)| {
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_3()
                        .child(div().text_color(colors.text_muted).child(label))
                        .child(div().child(value))
                }))
                .into_any_element()
        });

        let tab_search_overlay = self.tab_search.as_ref().map(|search| {
            let cursor = search.cursor.min(search.query.len());
            let (before, after) = search.query.split_at(cursor);
            let before = before.to_owned();
            let after = after.to_owned();
            let selected = search.select_all;
            let status = search
                .active_match
                .map(|index| format!("{} / {}", index + 1, search.matches.len()))
                .unwrap_or_else(|| format!("0 / {}", search.matches.len()));

            div()
                .absolute()
                .top(px(74.0))
                .left_2()
                .right_2()
                .flex()
                .justify_end()
                .child(
                    div()
                        .id("tab-scrollback-search")
                        .track_focus(&self.tab_search_focus)
                        .w_full()
                        .max_w(px(460.0))
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background.alpha(1.0))
                        .shadow_sm()
                        .text_sm()
                        .text_color(colors.text)
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .when(selected, |input| {
                                            input.bg(colors.element_selection_background)
                                        })
                                        .child(div().whitespace_nowrap().child(before))
                                        .when(!selected, |input| {
                                            input.child(
                                                div()
                                                    .flex_none()
                                                    .w(px(1.0))
                                                    .h(px(16.0))
                                                    .bg(colors.text_accent),
                                            )
                                        })
                                        .child(div().whitespace_nowrap().child(after)),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(colors.text_muted)
                                        .child(status),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("All panes  Enter next  Shift+Enter previous  Esc close"),
                        ),
                )
                .into_any_element()
        });

        let palette_overlay = self.command_palette.as_ref().map(|palette| {
            let cursor = palette.cursor.min(palette.query.len());
            let (query_before, query_after) = palette.query.split_at(cursor);
            let query_before = query_before.to_owned();
            let query_after = query_after.to_owned();
            let query_empty = palette.query.is_empty();
            let query_selected = palette.select_all;
            let matches = palette.matches();
            let selected = palette.selected;
            let result_count = matches.len();
            let visible_start = selected.saturating_sub(9);
            let rows = matches
                .iter()
                .copied()
                .skip(visible_start)
                .take(10)
                .enumerate()
                .map(|(position, command_index)| {
                    let command = &palette.commands[command_index];
                    let row_handle = handle.clone();
                    div()
                        .id(("command-palette-row", command_index))
                        .h_9()
                        .w_full()
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(colors.text)
                        .when(visible_start + position == selected, |row| {
                            row.bg(colors.element_selected)
                        })
                        .hover(|style| style.bg(colors.element_hover))
                        .on_click(move |_, window, cx| {
                            row_handle
                                .update(cx, |this, cx| {
                                    this.run_palette_command(command_index, window, cx)
                                })
                                .ok();
                        })
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(command.name.clone()),
                        )
                        .when_some(command.shortcut.clone(), |row, shortcut| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(colors.text_muted)
                                    .child(shortcut),
                            )
                        })
                })
                .collect::<Vec<_>>();
            let dismiss_handle = handle.clone();

            div()
                .id("command-palette-backdrop")
                .absolute()
                .inset_0()
                .pt(px(72.))
                .px_4()
                .flex()
                .items_start()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    dismiss_handle
                        .update(cx, |this, cx| this.dismiss_command_palette(window, cx))
                        .ok();
                })
                .child(
                    div()
                        .id("command-palette")
                        .track_focus(&self.command_palette_focus)
                        .w_full()
                        .max_w(px(680.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h_12()
                                .px_3()
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(colors.border)
                                .text_color(colors.text)
                                .child(div().text_color(colors.text_accent).mr_2().child(">"))
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(query_selected, |input| {
                                            input.bg(colors.element_selection_background)
                                        })
                                        .child(div().whitespace_nowrap().child(query_before))
                                        .when(!query_selected, |input| {
                                            input.child(
                                                div()
                                                    .flex_none()
                                                    .w(px(1.0))
                                                    .h(px(16.0))
                                                    .bg(colors.text_accent),
                                            )
                                        })
                                        .child(div().whitespace_nowrap().child(query_after))
                                        .when(query_empty, |input| {
                                            input.child(
                                                div()
                                                    .text_color(colors.text_placeholder)
                                                    .child("Type a command"),
                                            )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .py_1()
                                .when(result_count == 0, |list| {
                                    list.child(
                                        div()
                                            .h_12()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .text_sm()
                                            .text_color(colors.text_muted)
                                            .child("No matching commands"),
                                    )
                                })
                                .children(rows),
                        )
                        .child(
                            div()
                                .h_7()
                                .px_3()
                                .flex()
                                .items_center()
                                .border_t_1()
                                .border_color(colors.border)
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(format!(
                                    "{result_count} command{}",
                                    if result_count == 1 { "" } else { "s" }
                                )),
                        ),
                )
                .into_any_element()
        });

        let multi_command_overlay = self.multi_command.as_mut().map(|prompt| {
            let (query_before, query_after) = prompt.rendered_query_parts();
            let query_empty = prompt.query.is_empty();
            let query_selected = prompt.select_all;
            let error = prompt.error.clone();
            let completion_selected = prompt.completion_selected;
            let completion_count = prompt.completion_candidates.len();
            let completion_loading = prompt.completion_loading;
            let completion_visible_start = completion_selected.unwrap_or(0).saturating_sub(7);
            let completion_rows = prompt
                .completion_candidates
                .iter()
                .skip(completion_visible_start)
                .take(8)
                .enumerate()
                .map(|(index, candidate)| {
                    let completion_index = completion_visible_start + index;
                    let completion_handle = handle.clone();
                    div()
                        .id(("multi-command-completion", completion_index))
                        .h_7()
                        .px_3()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(colors.text)
                        .when(
                            completion_selected == Some(completion_index),
                            |row| row.bg(colors.element_selected),
                        )
                        .hover(|style| style.bg(colors.element_hover))
                        .on_click(move |_, window, cx| {
                            completion_handle
                                .update(cx, |this, cx| {
                                    this.select_multi_command_completion(
                                        completion_index,
                                        window,
                                        cx,
                                    )
                                })
                                .ok();
                        })
                        .child(candidate.clone())
                })
                .collect::<Vec<_>>();
            let dismiss_handle = handle.clone();

            div()
                .id("multi-command-backdrop")
                .absolute()
                .inset_0()
                .pt(px(72.))
                .px_4()
                .flex()
                .items_start()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    dismiss_handle
                        .update(cx, |this, cx| this.dismiss_multi_command(window, cx))
                        .ok();
                })
                .child(
                    div()
                        .id("multi-command-prompt")
                        .track_focus(&self.multi_command_focus)
                        .w_full()
                        .max_w(px(680.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h_12()
                                .px_3()
                                .flex()
                                .items_center()
                                .text_color(colors.text)
                                .child(div().text_color(colors.text_accent).mr_2().child("$"))
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(query_selected, |input| {
                                            input.bg(colors.element_selection_background)
                                        })
                                        .child(
                                            div()
                                                .whitespace_nowrap()
                                                .child(query_before),
                                        )
                                        .when(!query_selected, |input| {
                                            input.child(
                                                div()
                                                    .flex_none()
                                                    .w(px(1.0))
                                                    .h(px(16.0))
                                                    .bg(colors.text_accent),
                                            )
                                        })
                                        .child(
                                            div().whitespace_nowrap().child(query_after),
                                        )
                                        .when(query_empty, |input| {
                                            input.child(
                                                div()
                                                    .text_color(colors.text_placeholder)
                                                    .child("ssh {{a,b,c,d}}.example.com"),
                                            )
                                        }),
                                ),
                        )
                        .when(completion_count > 0, |prompt| {
                            prompt.child(
                                div()
                                    .py_1()
                                    .border_t_1()
                                    .border_color(colors.border)
                                    .children(completion_rows),
                            )
                        })
                        .child(
                            div()
                                .min_h_9()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(colors.border)
                                .text_xs()
                                .text_color(if error.is_some() {
                                    error_color
                                } else {
                                    colors.text_muted
                                })
                                .child(error.unwrap_or_else(|| {
                                    if completion_loading {
                                        "Loading completions…".to_owned()
                                    } else if completion_count > 0 {
                                        format!(
                                            "{completion_count} completion{} · Tab next · Shift+Tab previous",
                                            if completion_count == 1 { "" } else { "s" }
                                        )
                                    } else {
                                        "Double-brace values become tiled panes · Tab complete · Enter run · Esc cancel"
                                            .to_owned()
                                    }
                                })),
                        ),
                )
                .into_any_element()
        });

        let settings_overlay = self.render_settings_overlay(window, cx);

        let content = div()
            .key_context("Zetta")
            .size_full()
            .relative()
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
            .on_action(cx.listener(Self::apply_pane_split_template))
            .on_action(cx.listener(Self::focus_pane_left))
            .on_action(cx.listener(Self::focus_pane_right))
            .on_action(cx.listener(Self::focus_pane_up))
            .on_action(cx.listener(Self::focus_pane_down))
            .on_action(cx.listener(Self::toggle_broadcast_input))
            .on_action(cx.listener(Self::toggle_multi_command))
            .on_action(cx.listener(Self::increase_terminal_font_size))
            .on_action(cx.listener(Self::decrease_terminal_font_size))
            .on_action(cx.listener(Self::reset_terminal_font_size))
            .on_action(cx.listener(Self::increase_pane_font_size))
            .on_action(cx.listener(Self::decrease_pane_font_size))
            .on_action(cx.listener(Self::reset_pane_font_size))
            .on_action(cx.listener(Self::save_pane_output))
            .on_action(cx.listener(Self::search_tab_scrollback))
            .on_action(cx.listener(Self::reload_configuration))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::toggle_performance_overlay))
            .when(self.is_renaming_tab(), |content| {
                content.track_focus(&self.rename_focus)
            })
            .on_key_down(cx.listener(Self::command_palette_key_down))
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
                    .child(
                        IconButton::new("toggle-broadcast-input", IconName::Keyboard)
                            .shape(IconButtonShape::Wide)
                            .size(ButtonSize::Large)
                            .width(px(32.))
                            .icon_size(IconSize::Small)
                            .toggle_state(broadcast_input)
                            .aria_label("Broadcast input to all panes")
                            .tooltip(Tooltip::text(if broadcast_input {
                                "Broadcast input is on (Ctrl-Shift-I)"
                            } else {
                                "Broadcast input to all panes (Ctrl-Shift-I)"
                            }))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleBroadcastInput), cx)
                            }),
                    )
                    .child(
                        IconButton::new("open-settings", IconName::Settings)
                            .shape(IconButtonShape::Wide)
                            .size(ButtonSize::Large)
                            .width(px(32.))
                            .icon_size(IconSize::Small)
                            .aria_label("Open settings")
                            .tooltip(Tooltip::text("Open settings (Ctrl+,)"))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleSettings), cx)
                            }),
                    )
                    .child(div().min_w_0().flex_1()),
            )
            .when_some(self.configuration_error.clone(), |content, error| {
                content.child(
                    div().px_2().py_1().child(
                        Banner::new()
                            .severity(Severity::Error)
                            .child(Label::new(error).size(LabelSize::Small).line_clamp(3))
                            .action_slot(
                                IconButton::new("reload-invalid-configuration", IconName::RotateCw)
                                    .shape(IconButtonShape::Square)
                                    .icon_size(IconSize::Small)
                                    .aria_label("Reload configuration")
                                    .tooltip(Tooltip::text("Reload configuration"))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(ReloadConfiguration), cx)
                                    }),
                            ),
                    ),
                )
            })
            .when_some(self.pane_output_error.clone(), |content, error| {
                content.child(
                    div().px_2().py_1().child(
                        Banner::new()
                            .severity(Severity::Error)
                            .child(Label::new(error).size(LabelSize::Small).line_clamp(3)),
                    ),
                )
            })
            .child(div().flex_1().min_h_0().child(body))
            .when_some(performance_overlay, |content, overlay| {
                content.child(overlay)
            })
            .when_some(palette_overlay, |content, overlay| content.child(overlay))
            .when_some(multi_command_overlay, |content, overlay| {
                content.child(overlay)
            })
            .when_some(tab_search_overlay, |content, overlay| {
                content.child(overlay)
            });
        let content =
            content.when_some(settings_overlay, |content, overlay| content.child(overlay));

        client_window_frame(content, window, cx)
    }
}
