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
                        .child({
                            let close_button_on_left = window_close_button_on_left(self.button_layout);
                            h_flex()
                                .mb_3()
                                .gap_2()
                                .when(close_button_on_left, |flex| flex.flex_row_reverse())
                                .child(search)
                                .child(
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
                                )
                        })
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

/// Every floating layer rendered above the tab body, in paint order.
///
/// Built in one pass before the window content is composed, because each entry
/// borrows the entity while it reads the state that drives it.
struct ZettaOverlays {
    performance: Option<AnyElement>,
    palette: Option<AnyElement>,
    multi_command: Option<AnyElement>,
    tab_search: Option<AnyElement>,
    settings: Option<AnyElement>,
    tab_icon_picker: Option<AnyElement>,
    theme_picker: Option<AnyElement>,
    overlay_style_picker: Option<AnyElement>,
    serial_console: Option<AnyElement>,
    session_authentication: Option<AnyElement>,
}

impl Zetta {
    fn render_overlays(
        &mut self,
        colors: &ThemeColors,
        error_color: Hsla,
        handle: &WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ZettaOverlays {
        #[cfg(feature = "serial-console")]
        let serial_console = self.render_serial_console_overlay(cx);
        #[cfg(not(feature = "serial-console"))]
        let serial_console: Option<AnyElement> = None;

        ZettaOverlays {
            performance: self.render_performance_overlay(colors, window),
            palette: self.render_command_palette_overlay(colors, handle, cx),
            multi_command: self.render_multi_command_overlay(colors, error_color, handle),
            tab_search: self.render_tab_search_overlay(colors),
            settings: self.render_settings_overlay(window, cx),
            tab_icon_picker: self.render_tab_icon_picker_overlay(window, cx),
            theme_picker: self.render_pane_theme_picker_overlay(colors, handle, cx),
            overlay_style_picker: self.render_overlay_style_picker_overlay(window, cx),
            serial_console,
            session_authentication: self.render_session_authentication_overlay(cx),
        }
    }

    /// Registers every window-level action handler on the root element.
    fn register_actions(content: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        content
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
    }

    /// The error banners shown between the tab bar and the tab body.
    fn render_error_banners(&self, content: gpui::Div) -> gpui::Div {
        let banner = |error: String| {
            Banner::new()
                .severity(Severity::Error)
                .child(Label::new(error).size(LabelSize::Small).line_clamp(3))
        };
        content
            .when_some(self.configuration_error.clone(), |content, error| {
                content.child(
                    div().px_2().py_1().child(
                        banner(error).action_slot(
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
                content.child(div().px_2().py_1().child(banner(error)))
            })
    }

    /// Stacks the chrome, the tab body, and the overlays into the root element.
    fn compose_window_content(
        &self,
        chrome: TitleBarChrome,
        body: AnyElement,
        overlays: ZettaOverlays,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let content = div()
            .key_context("Zetta")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .when(!cfg!(linux_like), |content| {
                content.bg(colors.editor_background)
            });
        let content = Self::register_actions(content, cx)
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
            .child(chrome.title_bar)
            .when_some(chrome.tab_bar, |content, tab_bar| content.child(tab_bar));
        let content = self.render_error_banners(content);

        // Paint order matters: later overlays sit above earlier ones.
        [
            overlays.performance,
            overlays.palette,
            overlays.multi_command,
            overlays.tab_search,
            overlays.settings,
            overlays.tab_icon_picker,
            overlays.theme_picker,
            overlays.overlay_style_picker,
            overlays.serial_console,
            overlays.session_authentication,
        ]
        .into_iter()
        .flatten()
        .fold(
            content.child(div().flex_1().min_h_0().child(body)),
            |content, overlay| content.child(overlay),
        )
    }
}

impl Render for Zetta {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Native mouse resizing is constrained by WindowOptions where the
        // platform supports it. Keep it consistent with resize mode if a
        // compositor reports an undersized bound anyway.
        crate::app::enforce_minimum_window_size(window);
        self.sync_visible_terminals(cx);

        let colors = cx.theme().colors().clone();
        let error_color = cx.theme().status().error;
        let handle = cx.entity().downgrade();
        let frame = WindowFrameGeometry::new(window);

        let chrome = self.render_title_bar_chrome(&frame, &colors, &handle, window, cx);
        let body = self.render_tab_body(
            window,
            frame.rounded_bottom_left,
            frame.rounded_bottom_right,
            frame.bottom_corner_radius,
            cx,
        );
        let overlays = self.render_overlays(&colors, error_color, &handle, window, cx);

        let content = self.compose_window_content(chrome, body, overlays, &colors, cx);
        client_window_frame(content, window, cx)
    }
}
