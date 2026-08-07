use super::*;

impl Zetta {
    pub(crate) fn render_tab_icon_picker_overlay(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.tab_icon_picker.as_ref()?;
        let entries = self.tab_icon_entries();
        let options = self
            .tab_icon_picker
            .as_mut()
            .expect("checked above")
            .options(&entries)
            .to_vec();
        let picker = self.tab_icon_picker.as_ref()?;
        let colors = cx.theme().colors().clone();
        let handle = cx.entity().downgrade();
        let query = picker.query.clone();
        let query_empty = query.text.is_empty();
        let (query_before, query_after) = if query.select_all {
            (query.text.clone(), String::new())
        } else {
            let cursor = query.cursor.min(query.text.len());
            let (before, after) = query.text.split_at(cursor);
            (before.to_owned(), after.to_owned())
        };
        let has_options = !options.is_empty();
        let selected_icon = match picker.target {
            TabIconPickerTarget::Tab(tab_index) => {
                self.tabs.get(tab_index).and_then(|tab| tab.icon)
            }
            TabIconPickerTarget::Default => self
                .settings_editor
                .as_ref()
                .and_then(|editor| editor.configuration.default_tab_icon),
        };
        let search_handle = handle.clone();
        let close_handle = handle.clone();
        let search = div()
            .id("tab-icon-search")
            .h_9()
            .min_w_0()
            .flex_1()
            .px_2()
            .flex()
            .items_center()
            .overflow_hidden()
            .rounded(px(4.))
            .border_1()
            .border_color(colors.border_focused)
            .bg(colors.editor_background)
            .when(query.select_all, |input| {
                input.bg(colors.element_selection_background)
            })
            .text_color(colors.text)
            .child(
                h_flex()
                    .min_w_0()
                    .when(!query.select_all, |input| {
                        input.child(div().whitespace_nowrap().child(query_before.clone()))
                    })
                    .when(!query.select_all, |input| {
                        input.child(
                            div()
                                .flex_none()
                                .w(px(1.))
                                .h(px(16.))
                                .bg(colors.text_accent),
                        )
                    })
                    .when(query.select_all, |input| {
                        input
                            .text_color(colors.text)
                            .child(div().whitespace_nowrap().child(query_before.clone()))
                    })
                    .child(div().whitespace_nowrap().child(query_after))
                    .when(query_empty, |input| {
                        input
                            .text_color(colors.text_placeholder)
                            .child("Search icons…")
                    }),
            )
            .on_click(move |_, window, cx| {
                search_handle
                    .update(cx, |this, cx| {
                        this.tab_icon_picker_focus.focus(window, cx);
                    })
                    .ok();
            });
        let icon_cells = options.into_iter().enumerate().map(|(index, option)| {
            let label = option
                .map(tab_icon_label)
                .unwrap_or_else(|| "None".to_owned());
            let selected = option == selected_icon;
            let keyboard_selected = index == picker.selected;
            let icon_handle = handle.clone();
            div()
                .id(("tab-icon-option", index))
                .w(px(84.))
                .h(px(68.))
                .p_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .rounded(px(4.))
                .cursor_pointer()
                .when(selected, |cell| cell.bg(colors.element_selected))
                .when(keyboard_selected, |cell| {
                    cell.border_1().border_color(colors.border_focused)
                })
                .hover(|cell| cell.bg(colors.element_hover))
                .when_some(option, |cell, icon| {
                    cell.child(Icon::new(icon).size(IconSize::Medium))
                })
                .when(option.is_none(), |cell| {
                    cell.child(Icon::new(IconName::Dash).size(IconSize::Medium))
                })
                .child(Label::new(label.clone()).size(LabelSize::XSmall).truncate())
                .tooltip(Tooltip::text(label))
                .on_click(move |_, window, cx| {
                    icon_handle
                        .update(cx, |this, cx| {
                            this.set_tab_icon(option, window, cx);
                        })
                        .ok();
                })
        });
        let scroll = picker.scroll.clone();
        let viewport = scroll.bounds().size.height;
        let maximum = scroll.max_offset().y;
        let content_height = viewport + maximum;
        let thumb_fraction = if content_height > px(0.) {
            (viewport / content_height).clamp(0.08, 1.)
        } else {
            1.
        };
        let progress = if maximum > px(0.) {
            (-scroll.offset().y / maximum).clamp(0., 1.)
        } else {
            0.
        };
        let top_fraction = progress * (1. - thumb_fraction);
        let click_scroll = scroll.clone();
        let click_handle = handle.clone();
        let scrollbar = div()
            .id("tab-icon-scrollbar")
            .absolute()
            .top_1()
            .right_0()
            .bottom_1()
            .w(px(8.))
            .bg(colors.scrollbar_track_background)
            .when(maximum <= px(0.), |bar| bar.invisible())
            .child(
                div()
                    .absolute()
                    .right(px(2.))
                    .top(gpui::relative(top_fraction))
                    .h(gpui::relative(thumb_fraction))
                    .w(px(4.))
                    .rounded_full()
                    .bg(colors.scrollbar_thumb_background),
            )
            .on_click(move |event, _, cx| {
                if maximum > px(0.) {
                    let bounds = click_scroll.bounds();
                    let progress =
                        ((event.position().y - bounds.top()) / bounds.size.height).clamp(0., 1.);
                    let offset = click_scroll.offset();
                    click_scroll.set_offset(point(offset.x, -(maximum * progress)));
                    click_handle.update(cx, |_, cx| cx.notify()).ok();
                }
                cx.stop_propagation();
            });
        let icon_grid = div()
            .id("tab-icon-grid")
            .size_full()
            .track_scroll(&picker.scroll)
            .overflow_y_scroll()
            .p_1()
            .pr_3()
            .flex()
            .flex_wrap()
            .content_start()
            .gap_1()
            .children(icon_cells);
        let icon_grid = div()
            .relative()
            .min_h_0()
            .flex_1()
            .child(icon_grid.when(!has_options, |grid| {
                grid.child(
                    div()
                        .w_full()
                        .py_6()
                        .flex()
                        .justify_center()
                        .text_color(colors.text_muted)
                        .child("No icons match your search"),
                )
            }))
            .child(scrollbar);
        Some(
            div()
                .id("tab-icon-picker-modal")
                .absolute()
                .inset_0()
                .p_8()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.55))
                .occlude()
                .child(
                    div()
                        .w_full()
                        .max_w(px(720.))
                        .h_full()
                        .max_h(px(600.))
                        .p_3()
                        .flex()
                        .flex_col()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .child(
                            h_flex().mb_3().gap_2().child(search).child(
                                IconButton::new("close-tab-icon-picker", IconName::Close)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text("Close icon picker"))
                                    .on_click(move |_, window, cx| {
                                        close_handle
                                            .update(cx, |this, cx| {
                                                this.dismiss_tab_icon_picker(window, cx);
                                            })
                                            .ok();
                                    }),
                            ),
                        )
                        .child(icon_grid)
                        .child(
                            h_flex()
                                .mt_2()
                                .w_full()
                                .justify_center()
                                .text_color(colors.text_muted)
                                .text_xs()
                                .child("Tab / Shift-Tab: navigate icons  •  ↑/↓: navigate rows  •  ←/→: move cursor in search  •  Enter: select  •  Esc: close"),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Floating panel that chooses a pane overlay's font size, colour, and
    /// opacity right after its text is entered from the command palette. The
    /// pane under the panel previews each highlighted value; Enter commits
    /// them all and Escape restores the pane's previous values.
    pub(crate) fn render_overlay_style_picker_overlay(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let picker = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_ref())?;
        let colors = cx.theme().colors().clone();
        let handle = cx.entity().downgrade();
        let section = picker.section;
        let opacity_percent = picker.opacity_percent;
        let opacity_fraction = opacity_percent as f32 / 100.;
        let font_size = picker.font_size;
        let hex = picker.hex_buffer.clone();
        let hue = picker.hue;
        let saturation = picker.saturation;
        let value = picker.value;
        let section_boxed = |element: gpui::Div, active: bool, section: OverlayPickerSection| {
            let section_handle = handle.clone();
            element
                .px_3()
                .py_3()
                .rounded(px(6.))
                .border_1()
                .cursor_pointer()
                .border_color(if active {
                    colors.border_focused
                } else {
                    colors.border
                })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    section_handle
                        .update(cx, |this, cx| {
                            this.set_overlay_picker_section(section, cx);
                        })
                        .ok();
                })
        };

        let size_options = OverlayFontSize::ALL
            .iter()
            .enumerate()
            .map(|(index, size)| {
                let size = *size;
                let selected = picker.font_size == size;
                let option_handle = handle.clone();
                div()
                    .id(("overlay-size-option", index))
                    .flex_1()
                    .py_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(selected, |option| option.bg(colors.element_selected))
                    .hover(|option| option.bg(colors.element_hover))
                    .text_center()
                    .text_color(if selected {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .text_sm()
                    .on_click(move |_, _, cx| {
                        option_handle
                            .update(cx, |this, cx| {
                                this.set_overlay_font_size(size, cx);
                            })
                            .ok();
                    })
                    .child(size.cli_name())
            })
            .collect::<Vec<_>>();

        let sv_rows = (0usize..10)
            .map(|row| {
                let row_value = 1. - row as f32 / 9.;
                let row_handle = handle.clone();
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .children((0..12).map(move |column| {
                        let column_saturation = column as f32 / 11.;
                        let cell_color = hsv_to_hsla(hue, column_saturation, row_value);
                        let cell_handle = row_handle.clone();
                        div()
                            .id(("overlay-color-cell", row * 12 + column))
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .h_full()
                            .cursor_pointer()
                            .bg(cell_color)
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cell_handle
                                    .update(cx, |this, cx| {
                                        this.set_overlay_color_hsv(
                                            hue,
                                            column_saturation,
                                            row_value,
                                            cx,
                                        );
                                    })
                                    .ok();
                            })
                    }))
            })
            .collect::<Vec<_>>();

        let hue_segments = (0usize..12)
            .map(|column| {
                let column_hue = column as f32 / 11.;
                let segment_color = hsv_to_hsla(column_hue, 1., 1.);
                let segment_handle = handle.clone();
                div()
                    .id(("overlay-hue-segment", column))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .cursor_pointer()
                    .bg(segment_color)
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        segment_handle
                            .update(cx, |this, cx| {
                                this.set_overlay_color_hsv(column_hue, saturation, value, cx);
                            })
                            .ok();
                    })
            })
            .collect::<Vec<_>>();

        let opacity_stops = (0usize..=20)
            .map(|step| {
                let step_value = step * 5;
                let step_handle = handle.clone();
                div()
                    .id(("overlay-opacity-stop", step))
                    .h_full()
                    .flex_1()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        step_handle
                            .update(cx, |this, cx| {
                                this.set_overlay_opacity_percent(step_value, cx);
                            })
                            .ok();
                    })
            })
            .collect::<Vec<_>>();
        let cancel_handle = handle.clone();
        let cancel_button_handle = handle.clone();
        let apply_handle = handle.clone();
        let hint = match section {
            OverlayPickerSection::FontSize => {
                "← → size · Home/End ends · Tab switch · Enter apply · Esc cancel"
            }
            OverlayPickerSection::Color => "← → saturation · ↑↓ brightness · ⇧←→ hue · Tab switch",
            OverlayPickerSection::Opacity => {
                "← → opacity · Home/End ends · Tab switch · Enter apply · Esc cancel"
            }
        };

        Some(
            div()
                .id("overlay-style-backdrop")
                .absolute()
                .inset_0()
                .pt(px(72.))
                .px_4()
                .flex()
                .items_start()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cancel_handle
                        .update(cx, |this, cx| {
                            this.cancel_overlay_style_picker(window, cx);
                        })
                        .ok();
                })
                .child(
                    div()
                        .id("overlay-style-picker")
                        .track_focus(&self.overlay_style_focus)
                        .w_full()
                        .max_w(px(440.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            h_flex()
                                .h_11()
                                .px_3()
                                .items_center()
                                .justify_between()
                                .border_b_1()
                                .border_color(colors.border)
                                .child(
                                    div()
                                        .text_color(colors.text)
                                        .text_sm()
                                        .child("Overlay style"),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            div()
                                                .w_3()
                                                .h_3()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(colors.border)
                                                .bg(picker.color()),
                                        )
                                        .child(
                                            div().text_color(colors.text_accent).text_sm().child(
                                                format!(
                                                    "{} · {} · {}%",
                                                    font_size.cli_name(),
                                                    hex,
                                                    opacity_percent
                                                ),
                                            ),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .px_4()
                                .py_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    section_boxed(
                                        div(),
                                        section == OverlayPickerSection::FontSize,
                                        OverlayPickerSection::FontSize,
                                    )
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div().text_color(colors.text).text_sm().child("Font size"),
                                    )
                                    .child(h_flex().gap_1().children(size_options)),
                                )
                                .child(
                                    section_boxed(
                                        div(),
                                        section == OverlayPickerSection::Color,
                                        OverlayPickerSection::Color,
                                    )
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .w(px(30.))
                                                    .h(px(18.))
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(colors.border)
                                                    .bg(picker.color()),
                                            )
                                            .child(
                                                div()
                                                    .text_color(colors.text)
                                                    .text_sm()
                                                    .child("Colour"),
                                            )
                                            .child(
                                                div()
                                                    .id("overlay-hex-field")
                                                    .flex_1()
                                                    .min_w_0()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(if section
                                                        == OverlayPickerSection::Color
                                                    {
                                                        colors.border_focused
                                                    } else {
                                                        colors.border
                                                    })
                                                    .bg(colors.element_background)
                                                    .cursor_text()
                                                    .on_click({
                                                        let hex_field_handle = handle.clone();
                                                        move |_, _, cx| {
                                                            hex_field_handle
                                                                .update(cx, |this, cx| {
                                                                    this.set_overlay_picker_section(
                                                                        OverlayPickerSection::Color,
                                                                        cx,
                                                                    );
                                                                })
                                                                .ok();
                                                        }
                                                    })
                                                    .child(
                                                        h_flex()
                                                            .gap_0p5()
                                                            .child(
                                                                div()
                                                                    .text_color(colors.text)
                                                                    .text_sm()
                                                                    .child(hex),
                                                            )
                                                            .when(
                                                                section
                                                                    == OverlayPickerSection::Color,
                                                                |field| {
                                                                    field.child(
                                                                        div()
                                                                            .w(px(1.5))
                                                                            .h(px(13.))
                                                                            .bg(colors.text)
                                                                            .with_animation(
                                                                                "overlay-hex-caret",
                                                                                Animation::new(
                                                                                    Duration::from_millis(
                                                                                        500,
                                                                                    ),
                                                                                )
                                                                                .repeat(),
                                                                                |caret, progress| {
                                                                                    let visible =
                                                                                        (progress * 2.)
                                                                                            .fract()
                                                                                            < 0.5;
                                                                                    caret.opacity(
                                                                                        if visible
                                                                                        {
                                                                                            1.
                                                                                        } else {
                                                                                            0.
                                                                                        },
                                                                                    )
                                                                                },
                                                                            )
                                                                    )
                                                                },
                                                            ),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .relative()
                                            .h(px(152.))
                                            .w_full()
                                            .flex()
                                            .flex_col()
                                            .min_h_0()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(colors.border)
                                            .overflow_hidden()
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_h_0()
                                                    .children(sv_rows),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left(gpui::relative(saturation))
                                                    .top(gpui::relative(1. - value))
                                                    .ml(px(-6.))
                                                    .mt(px(-6.))
                                                    .size(px(12.))
                                                    .rounded_full()
                                                    .border_1()
                                                    .border_color(
                                                        colors.element_selection_background,
                                                    )
                                                    .bg(colors.text_accent),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .relative()
                                            .h(px(18.))
                                            .w_full()
                                            .flex()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(colors.border)
                                            .overflow_hidden()
                                            .child(h_flex().flex_1().children(hue_segments))
                                            .child(
                                                div()
                                                    .absolute()
                                                    .top(px(2.))
                                                    .left(gpui::relative(hue))
                                                    .ml(px(-6.))
                                                    .size(px(12.))
                                                    .rounded_full()
                                                    .border_1()
                                                    .border_color(
                                                        colors.element_selection_background,
                                                    )
                                                    .bg(colors.text_accent),
                                            ),
                                    ),
                                )
                                .child(
                                    section_boxed(
                                        div(),
                                        section == OverlayPickerSection::Opacity,
                                        OverlayPickerSection::Opacity,
                                    )
                                    .flex_col()
                                    .gap_2()
                                    .child(div().text_color(colors.text).text_sm().child("Opacity"))
                                    .child(
                                        div()
                                            .relative()
                                            .h_5()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left_0()
                                                    .right_0()
                                                    .h_1()
                                                    .rounded_full()
                                                    .bg(colors.element_background),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left_0()
                                                    .w(gpui::relative(opacity_fraction))
                                                    .h_1()
                                                    .rounded_full()
                                                    .bg(colors.text_accent),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left(gpui::relative(opacity_fraction))
                                                    .ml(px(-5.))
                                                    .size(px(10.))
                                                    .rounded_full()
                                                    .border_1()
                                                    .border_color(colors.border_focused)
                                                    .bg(colors.text_accent),
                                            )
                                            .child(
                                                h_flex()
                                                    .absolute()
                                                    .inset_0()
                                                    .children(opacity_stops),
                                            ),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(colors.border)
                                .child(div().text_color(colors.text_muted).text_xs().child(hint)),
                        )
                        .child(
                            h_flex()
                                .px_3()
                                .py_3()
                                .gap_2()
                                .justify_end()
                                .border_t_1()
                                .border_color(colors.border)
                                .child(
                                    Button::new("cancel-overlay-style", "Cancel")
                                        .style(ButtonStyle::Outlined)
                                        .on_click(move |_, window, cx| {
                                            cancel_button_handle
                                                .update(cx, |this, cx| {
                                                    this.cancel_overlay_style_picker(window, cx);
                                                })
                                                .ok();
                                        }),
                                )
                                .child(
                                    Button::new("apply-overlay-style", "Apply")
                                        .style(ButtonStyle::Filled)
                                        .on_click(move |_, window, cx| {
                                            apply_handle
                                                .update(cx, |this, cx| {
                                                    this.apply_overlay_style_picker(window, cx);
                                                })
                                                .ok();
                                        }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for Zetta {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Native mouse resizing is constrained by WindowOptions where the
        // platform supports it. Keep it consistent with resize mode if a
        // compositor reports an undersized bound anyway.
        crate::app::enforce_minimum_window_size(window);

        // Hidden terminals keep parsing PTY output and retaining scrollback, but they must not
        // continually enqueue work on the foreground executor. A newly visible terminal emits
        // one consolidated wakeup to render everything produced while it was hidden.
        let visible_terminals = self
            .tabs
            .get(self.active_tab)
            .into_iter()
            .flat_map(|tab| {
                tab.panes.iter().filter_map(|pane| {
                    tab.pane_is_visible(pane.id)
                        .then(|| pane.terminal.clone())
                        .flatten()
                })
            })
            .collect::<Vec<_>>();
        for terminal in &self.visible_terminals {
            if !visible_terminals
                .iter()
                .any(|visible| visible.entity_id() == terminal.entity_id())
            {
                terminal.update(cx, |terminal, cx| terminal.set_ui_visible(false, cx));
            }
        }
        for terminal in &visible_terminals {
            if !self
                .visible_terminals
                .iter()
                .any(|visible| visible.entity_id() == terminal.entity_id())
            {
                terminal.update(cx, |terminal, cx| terminal.set_ui_visible(true, cx));
            }
        }
        self.visible_terminals = visible_terminals;

        let colors = cx.theme().colors().clone();
        let error_color = cx.theme().status().error;
        let handle = cx.entity().downgrade();
        let broadcast_input = self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.broadcast_input);
        let (auto_background_tab, auto_background_protected) = self
            .tabs
            .get(self.active_tab)
            .map(|tab| match &tab.close_policy {
                TabClosePolicy::Background { authentication } => (true, authentication.is_some()),
                TabClosePolicy::Close => (false, false),
            })
            .unwrap_or_default();
        let process_background_sessions = self.process_background_session_picker_entries(cx);
        let background_session_count = process_background_sessions.len();
        #[cfg(any(target_os = "windows", linux_like))]
        let supported_controls = window.window_controls();
        #[cfg(any(target_os = "windows", linux_like))]
        let is_maximized = window.is_maximized();
        #[cfg(any(target_os = "windows", linux_like))]
        let is_resizable = window.is_resizable();
        #[cfg(any(target_os = "windows", linux_like))]
        let is_minimizable = window.is_minimizable();
        let (client_decorations, tiling) = match window.window_decorations() {
            Decorations::Client { tiling } => (true, tiling),
            Decorations::Server => (false, Tiling::default()),
        };
        let window_control_state = WindowControlState {
            #[cfg(any(target_os = "windows", linux_like))]
            supported_controls,
            #[cfg(any(target_os = "windows", linux_like))]
            is_maximized,
            #[cfg(any(target_os = "windows", linux_like))]
            is_resizable,
            #[cfg(any(target_os = "windows", linux_like))]
            is_minimizable,
            #[cfg(linux_like)]
            client_decorations,
        };
        let rounded_top_left =
            cfg!(linux_like) && client_decorations && !tiling.top && !tiling.left;
        let rounded_top_right =
            cfg!(linux_like) && client_decorations && !tiling.top && !tiling.right;
        let rounded_bottom_left =
            cfg!(linux_like) && client_decorations && !tiling.bottom && !tiling.left;
        let rounded_bottom_right =
            cfg!(linux_like) && client_decorations && !tiling.bottom && !tiling.right;
        let bottom_corner_radius = theme::CLIENT_SIDE_DECORATION_ROUNDING - px(1.);
        let tab_close_button_on_left = window_close_button_on_left(self.button_layout);
        let left_window_controls =
            render_window_controls(self.button_layout.left, window_control_state, false, cx);
        let right_window_controls =
            render_window_controls(self.button_layout.right, window_control_state, true, cx);
        let title_bar_background = if cfg!(linux_like) && !window.is_window_active() {
            colors.title_bar_inactive_background
        } else {
            colors.title_bar_background
        };
        let compact_mode = self.launch_config.compact_mode;
        let title_bar_height = platform_title_bar_height(window);
        let active_pane_size =
            title_bar_pane_size_visible(compact_mode, self.launch_config.hide_pane_size)
                .then(|| {
                    self.tabs
                        .get(self.active_tab)
                        .and_then(|tab| tab.active_pane())
                        .and_then(|pane| pane.terminal.as_ref())
                        .map(|terminal| {
                            let bounds = terminal.read(cx).last_content().terminal_bounds;
                            terminal_size_label(bounds.num_columns(), bounds.num_lines())
                        })
                })
                .flatten();
        let active_terminal_focus = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
            .map(|view| view.focus_handle(cx));
        let tab_count = self.tabs.len();
        let selected_tab_index = self.active_tab;
        let is_renaming_tab = self.is_renaming();
        let overflow_selection = self.tab_overflow_selection_side;
        let tab_bar_handle = handle.clone();
        let tab_overflow_border_color = colors.border;
        let tab_overflow_left_menu_handle = self.tab_overflow_left_menu_handle.clone();
        let tab_overflow_right_menu_handle = self.tab_overflow_right_menu_handle.clone();

        let tabs_row = container_query(move |size, _window, cx| {
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

            let (tabs, left_overflow, right_overflow, first_visible_selected) = tab_bar_handle
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
                            let select_handle = tab_bar_handle.clone();
                            let close_handle = tab_bar_handle.clone();
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
                            } else if let Some(view) =
                                tab.active_pane().and_then(|pane| pane.view.as_ref())
                            {
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
                                            |title| {
                                                title.bg(tab_colors.element_selection_background)
                                            },
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
                                .bg(tab_background)
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
                                        .tooltip(move |_window, cx| {
                                            Tooltip::for_action("Close tab", &CloseTab, cx)
                                        })
                                        .child(
                                            svg()
                                                .path(IconName::Close.path())
                                                .size(px(12.))
                                                .text_color(tab_icon),
                                        )
                                        .on_click(move |_, window, cx| {
                                            cx.stop_propagation();
                                            close_handle
                                                .update(cx, |this, cx| {
                                                    this.close_tab_at(index, window, cx)
                                                })
                                                .ok();
                                        }),
                                );
                            let menu_handle = tab_bar_handle.clone();
                            // The context menu activates this tab before it is rendered. Use
                            // the clicked tab's focus so its key context remains valid after
                            // that switch, including when the tab was previously inactive.
                            let action_context = tab
                                .active_pane()
                                .and_then(|pane| pane.view.as_ref())
                                .map(|view| view.focus_handle(cx));
                            let tab_element = ui::right_click_menu::<ui::ContextMenu>((
                                "tab-context-menu",
                                tab.id as usize,
                            ))
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
                                    let menu = menu.when_some(action_context, |menu, focus| {
                                        menu.context(focus)
                                    });
                                    menu.action("Rename Tab", Box::new(RenameTab))
                                        .action("Change Tab Icon", Box::new(ChangeTabIcon))
                                })
                            })
                            .trigger(move |_, _, _| tab_element)
                            .into_any_element();
                            responsive_tab_container(
                                tab_element,
                                compact_mode,
                                title_bar_height,
                                is_renaming_tab && selected,
                            )
                            .into_any_element()
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
                    |tabs| {
                        tabs.border_l_1()
                            .border_color(tab_overflow_border_color.opacity(0.25))
                    },
                )
                .when(!left_overflow.is_empty(), |bar| {
                    let overflow_border = if compact_mode {
                        tab_overflow_border_color.opacity(0.5)
                    } else {
                        tab_overflow_border_color
                    };
                    bar.child(render_tab_overflow_trigger(
                        false,
                        left_overflow,
                        compact_mode,
                        title_bar_height,
                        overflow_border,
                        tab_overflow_left_menu_handle.clone(),
                        tab_bar_handle.clone(),
                    ))
                })
                .children(tabs)
                .when(!right_overflow.is_empty(), |bar| {
                    let overflow_border = if compact_mode {
                        tab_overflow_border_color.opacity(0.5)
                    } else {
                        tab_overflow_border_color
                    };
                    bar.child(render_tab_overflow_trigger(
                        true,
                        right_overflow,
                        compact_mode,
                        title_bar_height,
                        overflow_border,
                        tab_overflow_right_menu_handle.clone(),
                        tab_bar_handle.clone(),
                    ))
                })
                .child(render_new_tab_button(compact_mode, title_bar_height))
                .when(compact_mode, |bar| {
                    bar.child(render_compact_drag_area(
                        title_bar_height,
                        tab_bar_handle.clone(),
                    ))
                })
        })
        .min_w_0()
        .flex_shrink_1();

        let tabs_scroll = tabs_row.into_any_element();

        let tab_bar = tab_bar_row_height(compact_mode, title_bar_height)
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
            .child(tabs_scroll);
        let show_title_bar_control_labels = title_bar_shows_control_labels(
            window.viewport_size().width,
            background_session_count > 0,
            self.launch_config.hide_title_bar_labels,
            compact_mode,
        );
        let show_title_bar_menus = title_bar_menus_visible(self.launch_config.hide_title_bar_menus);
        let show_title_bar_buttons =
            title_bar_buttons_visible(compact_mode, self.launch_config.hide_title_bar_buttons);
        let show_broadcast_control =
            title_bar_broadcast_visible(self.launch_config.hide_title_bar_buttons);
        let (compact_tab_bar, regular_tab_bar) = if compact_mode {
            (Some(tab_bar.into_any_element()), None)
        } else {
            (None, Some(tab_bar.into_any_element()))
        };
        let profile_menu_profiles = self.profiles.clone();
        let hidden_profiles = self.launch_config.hidden_profiles.clone();
        let default_profile = self.launch_config.default_profile;
        let profile_menu_handle = handle.clone();
        let profile_menu_dismiss_handle = handle.clone();
        let profile_menu_terminal_focus = active_terminal_focus.clone();
        let profile_menu_keyboard_mapper = cx.keyboard_mapper().clone();
        let profile_menu = PopoverMenu::new("new-tab-profile-menu")
            .with_handle(self.profile_menu_handle.clone())
            .trigger_with_tooltip(
                Button::new(
                    "new-tab-profile-menu-trigger",
                    if show_title_bar_control_labels {
                        "Profile"
                    } else {
                        ""
                    },
                )
                .start_icon(Icon::new(IconName::ChevronDown).size(IconSize::Small))
                .style(ButtonStyle::Subtle)
                .size(ButtonSize::Large)
                .aria_label("New tab profile"),
                Tooltip::text("New tab profile"),
            )
            .anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                let profiles = profile_menu_profiles.clone();
                let hidden_profiles = hidden_profiles.clone();
                let handle = profile_menu_handle.clone();
                let dismiss_handle = profile_menu_dismiss_handle.clone();
                let terminal_focus = profile_menu_terminal_focus.clone();
                let keyboard_mapper = profile_menu_keyboard_mapper.clone();
                let menu = ui::ContextMenu::build(window, cx, move |mut menu, window, _| {
                    for (visible_index, (index, profile)) in profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, profile)| !profile_is_hidden(profile, &hidden_profiles))
                        .enumerate()
                    {
                        let is_default = index == default_profile;
                        let label = profile.name.clone();
                        let label_for_row = label.clone();
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
                // Register before PopoverMenu's dismissal listener so a menu
                // reached through left/right navigation cannot restore focus to
                // the menu it replaced.
                window
                    .subscribe(&menu, cx, move |menu, _: &DismissEvent, window, cx| {
                        if menu.focus_handle(cx).is_focused(window) {
                            dismiss_handle
                                .update(cx, |this, cx| this.focus_active(window, cx))
                                .ok();
                        }
                    })
                    .detach();
                Some(menu)
            })
            .into_any_element();

        let reconnect_menu_entries = if background_session_count > 1 {
            process_background_sessions.to_vec()
        } else {
            Vec::new()
        };
        let make_reconnect_control = |show_label| {
            let reconnect_menu_entries = reconnect_menu_entries.clone();
            let reconnect_menu_handle = handle.clone();
            let reconnect_menu = PopoverMenu::new("reconnect-session-menu")
                .with_handle(self.reconnect_menu_handle.clone())
                .trigger_with_tooltip(
                    Button::new("reconnect-session", reconnect_control_label(show_label))
                        .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                        .style(ButtonStyle::Subtle)
                        .size(ButtonSize::Large)
                        .aria_label("Choose background session to reconnect"),
                    Tooltip::for_action_title(
                        format!(
                            "Choose background session to reconnect ({background_session_count})"
                        ),
                        &ReconnectSession,
                    ),
                )
                .anchor(Anchor::TopRight)
                .menu(move |window, cx| {
                    let entries = reconnect_menu_entries.clone();
                    let menu_handle = reconnect_menu_handle.clone();
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
                });
            if background_session_count == 1 {
                Button::new("reconnect-session", reconnect_control_label(show_label))
                    .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Large)
                    .aria_label("Reconnect background session")
                    .tooltip(Tooltip::for_action_title(
                        "Reconnect background session",
                        &ReconnectSession,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ReconnectSession), cx)
                    })
                    .into_any_element()
            } else {
                reconnect_menu.into_any_element()
            }
        };
        let reconnect_control = if show_title_bar_buttons && background_session_count > 0 {
            Some(make_reconnect_control(show_title_bar_control_labels))
        } else {
            None
        };
        let right_reconnect_control = if title_bar_background_indicator_on_right(
            compact_mode,
            self.launch_config.hide_title_bar_buttons,
            background_session_count,
        ) {
            // This control is outside the regular controls row, so it must
            // block the draggable title-bar hitbox on platforms that use
            // native hit testing for client-side decorations.
            Some(
                div()
                    .occlude()
                    .child(make_reconnect_control(false))
                    .into_any_element(),
            )
        } else {
            None
        };

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
            .child(right_window_controls)
            .into_any_element();

        let application_menu_dismiss_handle = handle.clone();
        // The popover receives focus while it is open. Retain the active
        // terminal's context so actions continue to resolve their shortcuts
        // when the user cycles here from the Profile menu.
        let application_menu_action_context = active_terminal_focus;
        let application_menu = PopoverMenu::new("application-menu")
            .with_handle(self.application_menu_handle.clone())
            .trigger_with_tooltip(
                Button::new(
                    "application-menu-trigger",
                    if show_title_bar_control_labels {
                        "Menu"
                    } else {
                        ""
                    },
                )
                .start_icon(Icon::new(IconName::Menu).size(IconSize::Small))
                .style(ButtonStyle::Subtle)
                .size(ButtonSize::Large)
                .aria_label("Application menu"),
                Tooltip::for_action_title("Open application menu", &OpenApplicationMenu),
            )
            .anchor(Anchor::TopLeft)
            .menu(move |window, cx| {
                let dismiss_handle = application_menu_dismiss_handle.clone();
                let action_context = application_menu_action_context.clone();
                let menu = ui::ContextMenu::build(window, cx, move |menu, _, _| {
                    let menu =
                        menu.when_some(action_context.clone(), |menu, focus| menu.context(focus));
                    menu.action("New Tab", Box::new(NewTab))
                        .action("New Window", Box::new(NewWindow))
                        .separator()
                        .action("Open Settings", Box::new(ToggleSettings))
                        .action("Open Themes", Box::new(OpenThemes))
                        .action("Open Keymap", Box::new(OpenKeymap))
                        .separator()
                        .action("Close Tab", Box::new(CloseTab))
                        .action("Close Window", Box::new(CloseWindow))
                        .action("Close All Windows", Box::new(CloseAllWindows))
                });
                // Register before PopoverMenu's dismissal listener so a menu
                // reached through left/right navigation cannot restore focus to
                // the menu it replaced.
                window
                    .subscribe(&menu, cx, move |menu, _: &DismissEvent, window, cx| {
                        if menu.focus_handle(cx).is_focused(window) {
                            dismiss_handle
                                .update(cx, |this, cx| this.focus_active(window, cx))
                                .ok();
                        }
                    })
                    .detach();
                Some(menu)
            })
            .into_any_element();

        let title_bar = self.render_title_bar(
            title_bar_height,
            title_bar_background,
            rounded_top_left,
            rounded_top_right,
            left_window_controls,
            compact_mode,
            show_title_bar_menus,
            application_menu,
            profile_menu,
            show_title_bar_buttons,
            show_title_bar_control_labels,
            auto_background_tab,
            auto_background_protected,
            reconnect_control,
            show_broadcast_control,
            broadcast_input,
            compact_tab_bar,
            active_pane_size,
            right_title_bar_controls,
            cx,
        );

        let body = self.render_tab_body(
            window,
            rounded_bottom_left,
            rounded_bottom_right,
            bottom_corner_radius,
            cx,
        );
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
            let retained_match_count = search.matches.len();
            let status = if search.limit_reached {
                let position = search
                    .active_match
                    .map(|index| (index + 1).to_string())
                    .unwrap_or_else(|| "0".to_owned());
                format!(
                    "{position} / {retained_match_count} shown · {} matches",
                    search.total_count
                )
            } else {
                search
                    .active_match
                    .map(|index| format!("{} / {}", index + 1, search.total_count))
                    .unwrap_or_else(|| format!("0 / {}", search.total_count))
            };

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
            let result_count = matches.len();
            let row_handle = handle.clone();
            let row_colors = colors.clone();
            let rows = uniform_list(
                "command-palette-list",
                result_count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, _| {
                    let Some(palette) = this.command_palette.as_ref() else {
                        return Vec::new();
                    };
                    range
                        .map(|position| {
                            let command_index = palette.matches()[position];
                            let command = &palette.commands[command_index];
                            let command_name = command.name.clone();
                            let shortcut = command.shortcut.clone();
                            let row_handle = row_handle.clone();
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
                                .text_color(row_colors.text)
                                .when(position == palette.selected, |row| {
                                    row.bg(row_colors.element_selected)
                                })
                                .hover(|style| style.bg(row_colors.element_hover))
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
                                        .child(command_name),
                                )
                                .when_some(shortcut, |row, shortcut| {
                                    row.child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(row_colors.text_muted)
                                            .child(shortcut),
                                    )
                                })
                        })
                        .collect()
                }),
            )
            .with_sizing_behavior(ListSizingBehavior::Infer)
            .max_h(px(360.))
            .track_scroll(&palette.scroll)
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
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
                                .when(result_count > 0, |list| list.child(rows)),
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

        let theme_picker_overlay = self.theme_picker.as_ref().map(|picker| {
            let cursor = picker.cursor.min(picker.query.len());
            let (query_before, query_after) = picker.query.split_at(cursor);
            let query_before = query_before.to_owned();
            let query_after = query_after.to_owned();
            let query_empty = picker.query.is_empty();
            let query_selected = picker.select_all;
            let matches = picker.matches();
            let result_count = matches.len();
            let row_handle = handle.clone();
            let row_colors = colors.clone();
            let rows = uniform_list(
                "theme-picker-list",
                result_count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, _| {
                    let Some(picker) = this.theme_picker.as_ref() else {
                        return Vec::new();
                    };
                    let current_name = this.theme_picker_current.clone();
                    range
                        .map(|position| {
                            let command_index = picker.matches()[position];
                            let command = &picker.commands[command_index];
                            let command_name = command.name.clone();
                            let is_current = current_name.as_deref() == Some(command.name.as_str());
                            let row_handle = row_handle.clone();
                            div()
                                .id(("theme-picker-row", command_index))
                                .h_9()
                                .w_full()
                                .px_3()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .cursor_pointer()
                                .text_sm()
                                .text_color(row_colors.text)
                                .when(position == picker.selected, |row| {
                                    row.bg(row_colors.element_selected)
                                })
                                .hover(|style| style.bg(row_colors.element_hover))
                                .on_click(move |_, window, cx| {
                                    row_handle
                                        .update(cx, |this, cx| {
                                            this.run_pane_theme_picker_command(
                                                command_index,
                                                window,
                                                cx,
                                            )
                                        })
                                        .ok();
                                })
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .gap_2()
                                        .when(is_current, |row| {
                                            row.child(
                                                Icon::new(IconName::Check)
                                                    .size(IconSize::Small)
                                                    .color(Color::Accent),
                                            )
                                        })
                                        .when(!is_current, |row| row.child(div().w_4()))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(command_name),
                                        ),
                                )
                        })
                        .collect()
                }),
            )
            .with_sizing_behavior(ListSizingBehavior::Infer)
            .max_h(px(360.))
            .track_scroll(&picker.scroll)
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
            let dismiss_handle = handle.clone();

            div()
                .id("theme-picker-backdrop")
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
                        .update(cx, |this, cx| this.dismiss_pane_theme_picker(window, cx))
                        .ok();
                })
                .child(
                    div()
                        .id("theme-picker")
                        .track_focus(&self.theme_picker_focus)
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
                                .child(div().text_color(colors.text_accent).mr_2().child("◑"))
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
                                                    .child("Search pane themes"),
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
                                            .child("No matching themes"),
                                    )
                                })
                                .when(result_count > 0, |list| list.child(rows)),
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
                                .child("Change is not saved to the profile or configuration"),
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
        let tab_icon_picker_overlay = self.render_tab_icon_picker_overlay(window, cx);
        let overlay_style_picker_overlay = self.render_overlay_style_picker_overlay(window, cx);
        #[cfg(feature = "serial-console")]
        let serial_console_overlay = self.render_serial_console_overlay(cx);
        #[cfg(not(feature = "serial-console"))]
        let serial_console_overlay: Option<gpui::AnyElement> = None;
        let session_authentication_overlay = self.render_session_authentication_overlay(cx);

        let content = div()
            .key_context("Zetta")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .when(!cfg!(linux_like), |content| {
                content.bg(colors.editor_background)
            })
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::new_window))
            .on_action(cx.listener(Self::open_application_menu))
            .on_action(cx.listener(Self::activate_application_menu_left))
            .on_action(cx.listener(Self::activate_application_menu_right))
            .on_action(cx.listener(Self::open_profile))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::close_all_windows))
            .on_action(cx.listener(Self::minimize_window))
            .on_action(cx.listener(Self::zoom_window))
            .on_action(cx.listener(Self::open_themes))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::edit_config_file))
            .on_action(cx.listener(Self::edit_keymap_file))
            .on_action(cx.listener(Self::detach_tab))
            .on_action(cx.listener(Self::toggle_auto_background_tab))
            .on_action(cx.listener(Self::reconnect_session))
            .on_action(cx.listener(Self::close_active_pane))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::select_overflow_tab))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::change_tab_icon))
            .on_action(cx.listener(Self::change_pane_theme))
            .on_action(cx.listener(Self::apply_pane_theme))
            .on_action(cx.listener(Self::reset_pane_theme))
            .on_action(cx.listener(Self::rename_pane))
            .on_action(cx.listener(Self::set_pane_overlay))
            .on_action(cx.listener(Self::reset_pane_overlay))
            .on_action(cx.listener(Self::toggle_pane_controls))
            .on_action(cx.listener(Self::toggle_tab_pane_controls))
            .on_action(cx.listener(Self::split_horizontal_down))
            .on_action(cx.listener(Self::split_horizontal_up))
            .on_action(cx.listener(Self::split_vertical_right))
            .on_action(cx.listener(Self::split_vertical_left))
            .on_action(cx.listener(Self::rotate_pane_layout))
            .on_action(cx.listener(Self::rotate_pane_layout_counter_clockwise))
            .on_action(cx.listener(Self::toggle_pane_resize_mode))
            .on_action(cx.listener(Self::resize_pane_left))
            .on_action(cx.listener(Self::resize_pane_right))
            .on_action(cx.listener(Self::resize_pane_up))
            .on_action(cx.listener(Self::resize_pane_down))
            .on_action(cx.listener(Self::toggle_pane_move_mode))
            .on_action(cx.listener(Self::move_pane_left))
            .on_action(cx.listener(Self::move_pane_right))
            .on_action(cx.listener(Self::move_pane_up))
            .on_action(cx.listener(Self::move_pane_down))
            .on_action(cx.listener(Self::apply_pane_split_template))
            .on_action(cx.listener(Self::focus_pane_left))
            .on_action(cx.listener(Self::focus_pane_right))
            .on_action(cx.listener(Self::focus_pane_up))
            .on_action(cx.listener(Self::focus_pane_down))
            .on_action(cx.listener(Self::toggle_maximize_pane))
            .on_action(cx.listener(Self::minimize_pane))
            .on_action(cx.listener(Self::restore_minimized_pane))
            .on_action(cx.listener(Self::select_previous_minimized_pane))
            .on_action(cx.listener(Self::select_next_minimized_pane))
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
            .on_action(cx.listener(Self::toggle_serial_console))
            .on_action(cx.listener(Self::start_http_server))
            .on_action(cx.listener(Self::start_tftp_server))
            .on_action(cx.listener(Self::toggle_performance_overlay))
            .when(
                self.is_renaming() || self.is_editing_pane_overlay(),
                |content| content.track_focus(&self.rename_focus),
            )
            .when(self.is_picking_overlay_style(), |content| {
                content.track_focus(&self.overlay_style_focus)
            })
            .when(self.tab_icon_picker.is_some(), |content| {
                content.track_focus(&self.tab_icon_picker_focus)
            })
            .when(self.theme_picker.is_some(), |content| {
                content.track_focus(&self.theme_picker_focus)
            })
            .capture_key_up(cx.listener(Self::pane_resize_key_up))
            .on_key_down(cx.listener(Self::command_palette_key_down))
            .child(title_bar)
            .when_some(regular_tab_bar, |content, tab_bar| content.child(tab_bar))
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
        let content = content.when_some(tab_icon_picker_overlay, |content, overlay| {
            content.child(overlay)
        });
        let content = content.when_some(theme_picker_overlay, |content, overlay| {
            content.child(overlay)
        });
        let content = content.when_some(overlay_style_picker_overlay, |content, overlay| {
            content.child(overlay)
        });
        let content = content.when_some(serial_console_overlay, |content, overlay| {
            content.child(overlay)
        });
        let content = content.when_some(session_authentication_overlay, |content, overlay| {
            content.child(overlay)
        });

        client_window_frame(content, window, cx)
    }
}
