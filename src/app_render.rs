use super::*;
use crate::configuration_reload::CONFIGURATION_RELOAD_SUCCESS_MESSAGE;
use gpui::{ListSizingBehavior, uniform_list};

// GPUI's standard popover menus use priority 1. Keep every local deferred draw on
// an explicit rung around that fixed priority so title-bar repairs stay below
// menus, blocking overlays stay above them, and popups hosted by a modal stay on
// top of their deferred parent.
pub(crate) const TITLE_BAR_CONTROL_PAINT_PRIORITY: usize = 0;
pub(crate) const TITLE_BAR_POPOVER_PAINT_PRIORITY: usize = 1;
pub(crate) const MODAL_OVERLAY_PAINT_PRIORITY: usize = 2;
pub(crate) const MODAL_POPUP_PAINT_PRIORITY: usize = 3;

const _: () = assert!(TITLE_BAR_CONTROL_PAINT_PRIORITY < TITLE_BAR_POPOVER_PAINT_PRIORITY);
const _: () = assert!(TITLE_BAR_POPOVER_PAINT_PRIORITY < MODAL_OVERLAY_PAINT_PRIORITY);
const _: () = assert!(MODAL_OVERLAY_PAINT_PRIORITY < MODAL_POPUP_PAINT_PRIORITY);

fn modal_overlay(overlay: Option<AnyElement>) -> Option<AnyElement> {
    overlay.map(|overlay| {
        deferred(overlay)
            .with_priority(MODAL_OVERLAY_PAINT_PRIORITY)
            .into_any_element()
    })
}

impl Zetta {
    pub(crate) fn render_tab_icon_picker_overlay(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.tab_icon_picker.as_ref()?;

        // Get options and picker state, then release the borrow
        let (options, picker_state) = {
            let picker = self.tab_icon_picker.as_mut().expect("checked above");
            let opts = picker.options();
            let state = (
                picker.target,
                picker.selected,
                picker.scroll.clone(),
                picker.entries(),
            );
            (opts, state)
        };

        let (target, selected_index, scroll_handle, entries) = picker_state;
        let colors = self.window_theme(cx).colors().clone();
        let handle = cx.entity().downgrade();

        // Get selected icon for highlighting
        let selected_icon = match target {
            TabIconPickerTarget::Tab(tab_index) => {
                self.tabs.get(tab_index).and_then(|tab| tab.icon)
            }
            TabIconPickerTarget::Default => self
                .settings_editor
                .as_ref()
                .and_then(|editor| editor.configuration.default_tab_icon),
            TabIconPickerTarget::ProjectDefault => self
                .settings_editor
                .as_ref()
                .and_then(crate::settings_ui::project_editor)
                .and_then(|project| project.form.default_tab_icon.icon()),
        };

        let picker = self.tab_icon_picker.as_ref()?;
        let query = picker.query.clone();
        let query_empty = query.text.is_empty();
        let (query_before, query_after) = if query.select_all {
            (query.text.clone(), String::new())
        } else {
            let cursor = query.cursor.min(query.text.len());
            let (before, after) = query.text.split_at(cursor);
            (before.to_owned(), after.to_owned())
        };

        // Grid constants
        const ICON_CELL_WIDTH: Pixels = px(84.);
        const ICON_CELL_HEIGHT: Pixels = px(68.);
        const ICON_CELL_PADDING: Pixels = px(1.);
        const ICON_GAP: Pixels = px(1.);
        let row_height = ICON_CELL_HEIGHT + ICON_CELL_PADDING * 2. + ICON_GAP;

        let row_count = options.len().div_ceil(TAB_ICON_COLUMNS);

        // Build search bar
        let search_handle = handle.clone();
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

        let close_handle = handle.clone();
        let close = div()
            .id("close-tab-icon-picker")
            .flex_none()
            .px_3()
            .py_1()
            .rounded(px(4.))
            .border_1()
            .border_color(colors.element_selected)
            .cursor_pointer()
            .bg(colors.element_selected)
            .hover(|style| style.bg(colors.element_hover))
            .tooltip(Tooltip::text("Close tab icon picker (Esc)"))
            .on_click(move |_, window, cx| {
                close_handle
                    .update(cx, |this, cx| this.dismiss_tab_icon_picker(window, cx))
                    .ok();
            })
            .child("Close");

        // Virtualized icon grid using uniform_list
        let row_colors = colors.clone();
        let row_entries = entries.clone();
        let row_handle = handle.clone();
        let row_selected_icon = selected_icon;
        let row_selected_index = selected_index;
        let options_for_rows = options.clone();

        let icon_rows = uniform_list(
            "tab-icon-grid",
            row_count,
            move |range: std::ops::Range<usize>, _, _| {
                let entries = row_entries.clone();
                let options = &options_for_rows;
                let row_handle = row_handle.clone();
                let row_colors = row_colors.clone();
                let row_selected_icon = row_selected_icon;
                let row_selected_index = row_selected_index;

                range
                    .map(|row_index| {
                        let row_start = row_index * TAB_ICON_COLUMNS;
                        let row_end = (row_start + TAB_ICON_COLUMNS).min(options.len());
                        let row_options = &options[row_start..row_end];

                        let cells: Vec<AnyElement> = row_options
                            .iter()
                            .enumerate()
                            .map(|(col_index, option)| {
                                // Copy the option value since IconName is Copy
                                let option = *option;
                                let index = row_start + col_index;
                                let icon = option
                                    .and_then(|index| entries.get(index).map(|entry| entry.icon));
                                let label = option
                                    .and_then(|index| {
                                        entries.get(index).map(|entry| entry.label.clone())
                                    })
                                    .unwrap_or_else(|| SharedString::new_static("None"));
                                let keyboard_selected = index == row_selected_index;
                                let icon_for_click = icon;
                                let icon_handle = row_handle.clone();

                                div()
                                    .id(("tab-icon-option", index))
                                    .w(ICON_CELL_WIDTH)
                                    .h(ICON_CELL_HEIGHT)
                                    .p_1()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap_1()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .when(icon == row_selected_icon, |cell| {
                                        cell.bg(row_colors.element_selected)
                                    })
                                    .when(keyboard_selected, |cell| {
                                        cell.border_1().border_color(row_colors.border_focused)
                                    })
                                    .hover(|cell| cell.bg(row_colors.element_hover))
                                    .when_some(icon, |cell, icon| {
                                        cell.child(Icon::new(icon).size(IconSize::Medium))
                                    })
                                    .when(icon.is_none(), |cell| {
                                        cell.child(Icon::new(IconName::Dash).size(IconSize::Medium))
                                    })
                                    .child(
                                        Label::new(label.clone())
                                            .size(LabelSize::XSmall)
                                            .truncate(),
                                    )
                                    .tooltip(Tooltip::text(label))
                                    .on_click(move |_, window, cx| {
                                        icon_handle
                                            .update(cx, |this, cx| {
                                                this.set_tab_icon(icon_for_click, window, cx);
                                            })
                                            .ok();
                                    })
                                    .into_any_element()
                            })
                            .collect();

                        div()
                            .h(row_height)
                            .flex()
                            .gap_1()
                            .children(cells)
                            .into_any_element()
                    })
                    .collect()
            },
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .w_full()
        .h_full()
        .track_scroll(&scroll_handle)
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

        let has_options = !options.is_empty();

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
                        .child(h_flex().mb_3().gap_2().child(search).child(close))
                        .child(
                            div()
                                .relative()
                                .min_h_0()
                                .flex_1()
                                .overflow_hidden()
                                .when(has_options, |container| container.child(icon_rows))
                                .when(!has_options, |container| {
                                    container.child(
                                        div()
                                            .w_full()
                                            .py_6()
                                            .flex()
                                            .justify_center()
                                            .text_color(colors.text_muted)
                                            .child("No icons match your search"),
                                    )
                                })
                        )
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
        let colors = self.window_theme(cx).colors().clone();
        let handle = cx.entity().downgrade();
        let section = picker.section;
        let opacity_percent = picker.opacity_percent;
        let opacity_fraction = opacity_percent as f32 / 100.;
        let font_size = picker.font_size;
        let hex = picker.hex_buffer.clone();
        let hue = picker.hue;
        let saturation = picker.saturation;
        let value = picker.value;
        let selected_preset_index = OVERLAY_COLOR_PRESETS
            .iter()
            .position(|preset| preset.hex.eq_ignore_ascii_case(&hex));
        let focused_preset_index = picker
            .preset_index
            .min(OVERLAY_COLOR_PRESETS.len().saturating_sub(1));
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

        let preset_rows = OVERLAY_COLOR_PRESETS
            .chunks(OVERLAY_COLOR_PRESET_COLUMNS)
            .enumerate()
            .map(|(row_index, presets)| {
                let row_handle = handle.clone();
                h_flex()
                    .w_full()
                    .gap_1()
                    .children(
                        presets
                            .iter()
                            .enumerate()
                            .map(move |(column_index, preset)| {
                                let preset = *preset;
                                let preset_index =
                                    row_index * OVERLAY_COLOR_PRESET_COLUMNS + column_index;
                                let selected = selected_preset_index == Some(preset_index);
                                let keyboard_focused = section
                                    == OverlayPickerSection::ColorPresets
                                    && focused_preset_index == preset_index;
                                let preset_handle = row_handle.clone();
                                div()
                                    .id(("overlay-color-preset", preset_index))
                                    .h(px(26.))
                                    .flex_1()
                                    .min_w_0()
                                    .px_1()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(if keyboard_focused {
                                        colors.border_focused
                                    } else {
                                        colors.border.opacity(0.)
                                    })
                                    .cursor_pointer()
                                    .when(selected, |swatch| swatch.bg(colors.element_selected))
                                    .hover(|swatch| swatch.bg(colors.element_hover))
                                    .child(
                                        div()
                                            .flex_none()
                                            .size(px(12.))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(colors.border)
                                            .bg(preset.color()),
                                    )
                                    .child(
                                        Label::new(preset.name).size(LabelSize::XSmall).truncate(),
                                    )
                                    .tooltip(Tooltip::text(preset.name))
                                    .on_click(move |_, _, cx| {
                                        preset_handle
                                            .update(cx, |this, cx| {
                                                this.set_overlay_color_preset(preset, cx);
                                            })
                                            .ok();
                                    })
                            }),
                    )
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
            OverlayPickerSection::Color => {
                "← → saturation · ↑↓ brightness · ⇧←→ hue · type hex · Tab switch · Enter apply · Esc cancel"
            }
            OverlayPickerSection::ColorPresets => {
                "← →/↑↓ move · Home/End ends · Tab switch · Enter apply · Esc cancel"
            }
            OverlayPickerSection::Opacity => {
                "← → opacity · Home/End ends · Tab switch · Enter apply · Esc cancel"
            }
        };
        let opacity_section = section_boxed(
            div(),
            section == OverlayPickerSection::Opacity,
            OverlayPickerSection::Opacity,
        )
        .flex_1()
        .min_w_0()
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
                .child(h_flex().absolute().inset_0().children(opacity_stops)),
        );

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
                                    h_flex()
                                        .w_full()
                                        .gap_3()
                                        .child(
                                            section_boxed(
                                                div(),
                                                section == OverlayPickerSection::FontSize,
                                                OverlayPickerSection::FontSize,
                                            )
                                            .flex_1()
                                            .min_w_0()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_color(colors.text)
                                                    .text_sm()
                                                    .child("Font size"),
                                            )
                                            .child(h_flex().gap_1().children(size_options)),
                                        )
                                        .child(opacity_section),
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_3()
                                        .child(
                                            section_boxed(
                                                div(),
                                                section == OverlayPickerSection::Color,
                                                OverlayPickerSection::Color,
                                            )
                                            .flex_1()
                                            .min_w_0()
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
                                                                                                if visible {
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
                                                section == OverlayPickerSection::ColorPresets,
                                                OverlayPickerSection::ColorPresets,
                                            )
                                            .flex_1()
                                            .min_w_0()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_color(colors.text)
                                                    .text_sm()
                                                    .child("Colour presets"),
                                            )
                                            .child(v_flex().gap_1().children(preset_rows)),
                                        ),
                                )
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
    close_confirmation: Option<AnyElement>,
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
        let entity = cx.entity();
        #[cfg(feature = "serial-console")]
        let serial_console = self.render_serial_console_overlay(cx);
        #[cfg(not(feature = "serial-console"))]
        let serial_console: Option<AnyElement> = None;

        ZettaOverlays {
            performance: self.render_performance_overlay(colors, window),
            palette: modal_overlay(self.render_command_palette_overlay(colors, handle, cx)),
            multi_command: modal_overlay(self.render_multi_command_overlay(
                colors,
                error_color,
                handle,
            )),
            tab_search: self.render_tab_search_overlay(colors),
            // Both are rendered inside their own cached view: scrolling or
            // hovering one notifies that view instead of `Zetta`, so the other
            // one — and the window column — is reused for the frame. Each fills
            // the window, matching the `absolute inset_0` backdrop it renders.
            settings: modal_overlay(self.settings_editor.is_some().then(|| {
                overlay_boundary(ZettaSubview::get_or_insert(
                    &mut self.settings_surface_view,
                    render_settings_boundary,
                    &entity,
                    cx,
                ))
                .into_any_element()
            })),
            tab_icon_picker: modal_overlay(self.tab_icon_picker.is_some().then(|| {
                overlay_boundary(ZettaSubview::get_or_insert(
                    &mut self.tab_icon_picker_view,
                    render_tab_icon_picker_boundary,
                    &entity,
                    cx,
                ))
                .into_any_element()
            })),
            theme_picker: modal_overlay(self.render_pane_theme_picker_overlay(colors, handle, cx)),
            overlay_style_picker: modal_overlay(
                self.render_overlay_style_picker_overlay(window, cx),
            ),
            serial_console: modal_overlay(serial_console),
            session_authentication: modal_overlay(self.render_session_authentication_overlay(cx)),
            close_confirmation: modal_overlay(self.render_tab_close_confirmation_overlay(cx)),
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
            .on_action(cx.listener(Self::toggle_tab_pinning))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::close_all_windows))
            .on_action(cx.listener(Self::minimize_window))
            .on_action(cx.listener(Self::hide_window))
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
            .on_action(cx.listener(Self::toggle_tab_move_mode))
            .on_action(cx.listener(Self::move_tab_left))
            .on_action(cx.listener(Self::move_tab_right))
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
            .on_action(cx.listener(Self::toggle_silent_mode))
            .on_action(cx.listener(Self::request_focus_status_access))
            .on_action(cx.listener(Self::toggle_tab_silent_mode))
            .on_action(cx.listener(Self::toggle_multi_command))
            .on_action(cx.listener(Self::toggle_stacked_command))
            .on_action(cx.listener(Self::select_previous_stacked_pane))
            .on_action(cx.listener(Self::select_next_stacked_pane))
            .on_action(cx.listener(Self::close_stacked_pane))
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
            .on_action(cx.listener(Self::save_settings_action))
            .on_action(cx.listener(Self::toggle_serial_console))
            .on_action(cx.listener(Self::start_http_server))
            .on_action(cx.listener(Self::start_tftp_server))
            .on_action(cx.listener(Self::toggle_performance_overlay))
    }

    /// The feedback banners shown between the tab bar and the tab body.
    fn render_feedback_banners(
        &self,
        content: gpui::Div,
        colors: &ThemeColors,
        handle: &WeakEntity<Zetta>,
    ) -> gpui::Div {
        let banner = |error: String| {
            Banner::new()
                .severity(Severity::Error)
                .child(Label::new(error).size(LabelSize::Small).line_clamp(3))
        };
        let feedback_row = |banner: Banner| {
            div()
                .px_2()
                .py_1()
                .when(cfg!(linux_like), |row| row.bg(colors.editor_background))
                .child(banner)
        };
        content
            .when_some(self.projects.offer.clone(), |content, offer| {
                let add_handle = handle.clone();
                let dismiss_handle = handle.clone();
                content.child(feedback_row(
                    Banner::new()
                        .severity(Severity::Warning)
                        .child(
                            Label::new(format!(
                                "Zetta project configuration found in {}. Add this project? Its pane layouts may run commands.",
                                offer.root.display()
                            ))
                            .size(LabelSize::Small)
                            .line_clamp(3),
                        )
                        .action_slot(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("dismiss-project-offer", "Dismiss")
                                        .style(ButtonStyle::Outlined)
                                        .on_click(move |_, _, cx| {
                                            dismiss_handle
                                                .update(cx, |this, cx| {
                                                    this.dismiss_project_offer(cx)
                                                })
                                                .ok();
                                        }),
                                )
                                .child(
                                    Button::new("accept-project-offer", "Add project")
                                        .style(ButtonStyle::Filled)
                                        .on_click(move |_, window, cx| {
                                            add_handle
                                                .update(cx, |this, cx| {
                                                    this.accept_project_offer(window, cx)
                                                })
                                                .ok();
                                        }),
                                ),
                        ),
                ))
            })
            .when(self.configuration_reload_feedback.is_visible(), |content| {
                content.child(feedback_row(
                    Banner::new().severity(Severity::Success).child(
                        Label::new(CONFIGURATION_RELOAD_SUCCESS_MESSAGE).size(LabelSize::Small),
                    ),
                ))
            })
            .when_some(self.configuration_error.clone(), |content, error| {
                content.child(feedback_row(
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
                ))
            })
            .when_some(self.pane_output_error.clone(), |content, error| {
                content.child(feedback_row(banner(error)))
            })
    }

    /// The chrome and the tab body, in the column they share.
    ///
    /// The title bar and tab bar, the banners, and the tab body, stacked.
    ///
    /// The chrome is cached in its own view while the body is not, which is the
    /// whole point of the arrangement: a frame the terminal caused marks the
    /// body's ancestors dirty but leaves the chrome — a sibling — untouched, so
    /// output no longer rebuilds the title bar and tab bar sixty times a second.
    /// Caching the column as a whole cannot do this, and is in fact worse than
    /// not caching at all: a cached view that misses re-renders its subtree with
    /// `Window::refreshing` set, which suppresses every cache nested under it.
    fn render_window_column(
        &mut self,
        colors: &ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let frame = WindowFrameGeometry::new(window, cx);
        let entity = cx.entity();
        let handle = entity.downgrade();
        let chrome_height = title_bar_chrome_height(
            self.launch_config.compact_mode,
            frame.title_bar_height,
            window.rem_size(),
        );
        let chrome = ZettaSubview::get_or_insert(
            &mut self.title_bar_chrome_view,
            render_title_bar_chrome_boundary,
            &entity,
            cx,
        )
        .cached(
            gpui::StyleRefinement::default()
                .w_full()
                .flex_none()
                .h(chrome_height),
        );
        let body = self.render_tab_body(
            window,
            frame.rounded_bottom_left,
            frame.rounded_bottom_right,
            frame.corner_radius,
            cx,
        );
        let column = div().size_full().flex().flex_col().child(chrome);
        self.render_feedback_banners(column, colors, &handle)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// Stacks the window column and the overlays into the root element.
    fn compose_window_content(
        &self,
        column: AnyElement,
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
            .when(self.close_tab_confirmation.is_some(), |content| {
                content.track_focus(&self.close_confirmation_focus)
            })
            .capture_key_up(cx.listener(Self::pane_resize_key_up))
            .on_key_down(cx.listener(Self::command_palette_key_down))
            .child(column);

        // Child order preserves the existing relative order within each paint
        // priority; later overlays on the same rung sit above earlier ones.
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
            overlays.close_confirmation,
        ]
        .into_iter()
        .flatten()
        .fold(content, |content, overlay| content.child(overlay))
    }
}

fn render_settings_page_boundary(
    zetta: &mut Zetta,
    _window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    div()
        .size_full()
        .relative()
        .children(zetta.render_settings_page_region(cx))
        .into_any_element()
}

/// Adapters that give `ZettaSubview` a plain function pointer per boundary.
///
/// The root is `size_full` so the chrome fills exactly the box
/// `title_bar_chrome_height` reserved for it in `render_window_column`.
fn render_title_bar_chrome_boundary(
    zetta: &mut Zetta,
    window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    let colors = zetta.window_theme(cx).colors().clone();
    let handle = cx.entity().downgrade();
    let frame = WindowFrameGeometry::new(window, cx);
    let chrome = zetta.render_title_bar_chrome(&frame, &colors, &handle, window, cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(chrome.title_bar)
        .when_some(chrome.tab_bar, |column, tab_bar| column.child(tab_bar))
        .into_any_element()
}

fn render_settings_boundary(
    zetta: &mut Zetta,
    window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    overlay_boundary_root(zetta.render_settings_overlay(window, cx))
}

fn render_tab_icon_picker_boundary(
    zetta: &mut Zetta,
    window: &mut Window,
    cx: &mut Context<Zetta>,
) -> AnyElement {
    overlay_boundary_root(zetta.render_tab_icon_picker_overlay(window, cx))
}

impl Zetta {
    /// The settings page, wrapped in the cached boundary that keeps it out of
    /// frames driven by a modal or a dropdown popup above it.
    pub(crate) fn settings_page_region_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity();
        ZettaSubview::get_or_insert(
            &mut self.settings_page_view,
            render_settings_page_boundary,
            &entity,
            cx,
        )
        .cached(gpui::StyleRefinement::default().flex_1().min_h_0().w_full())
        .into_any_element()
    }
}

impl Render for Zetta {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Native mouse resizing is constrained by WindowOptions where the
        // platform supports it. Keep it consistent with resize mode if a
        // compositor reports an undersized bound anyway.
        crate::app::enforce_minimum_window_size(window);
        self.sync_visible_terminals(cx);

        let theme = self.window_theme(cx);
        // Zetta's own elements take `colors` explicitly, but Zed's `ui` components
        // — every `Button`/`IconButton`, `Label`, `switch`, `Banner`, and, with no
        // override API at all, every `Tooltip` and `ContextMenu` — resolve their
        // colors from `cx.theme()`. Without this they keep rendering the launch
        // configuration's theme inside a project that selects a different one.
        //
        // Doing it per frame rather than when the project changes is what keeps
        // this correct with several windows open: each window installs its own
        // theme immediately before building the elements that read it, and
        // tooltips and popovers build during that same frame. `update_theme` is a
        // plain global mutation, so it cannot re-enter this render.
        if !Arc::ptr_eq(GlobalTheme::theme(cx), &theme) {
            GlobalTheme::update_theme(cx, theme.clone());
        }
        let colors = theme.colors().clone();
        let error_color = theme.status().error;
        let handle = cx.entity().downgrade();

        // The column itself is composed here rather than behind a boundary of
        // its own: every frame reaches it anyway, and wrapping it in a cache
        // that always misses would suppress the caches inside it.
        let column = self.render_window_column(&colors, window, cx);
        let overlays = self.render_overlays(&colors, error_color, &handle, window, cx);

        let content = self.compose_window_content(column, overlays, &colors, cx);
        client_window_frame(content, window, colors.border)
    }
}

#[cfg(test)]
#[path = "tests/app_render.rs"]
mod tests;
