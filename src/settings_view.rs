use super::*;
use crate::settings_ui::{KeymapRow, invalidate_controls_cache, invalidate_keymap_cache};

use crate::startup::keymap_keystroke_display;

mod modals;
mod pages;
mod widgets;

pub(crate) use widgets::{DropdownRenderState, KEYMAP_ROW_HEIGHT, SETTINGS_SCROLLBAR_WIDTH};

impl Zetta {
    pub(crate) fn render_settings_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let editor = self.settings_editor.as_ref()?;
        let colors = cx.theme().colors().clone();
        let handle = cx.entity().downgrade();
        let close_button_on_left = window_close_button_on_left(self.button_layout);
        if !editor.scroll_geometry_initialized {
            let geometry_handle = handle.clone();
            window.on_next_frame(move |_, cx| {
                geometry_handle
                    .update(cx, |this, cx| {
                        if let Some(editor) = this.settings_editor.as_mut() {
                            editor.scroll_geometry_initialized = true;
                            cx.notify();
                        }
                    })
                    .ok();
            });
        }

        let scroll_indicator = |id: String, scroll: &ScrollHandle| -> gpui::AnyElement {
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
            let wheel_scroll = scroll.clone();
            let wheel_handle = handle.clone();
            div()
                .id(id)
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(px(SETTINGS_SCROLLBAR_WIDTH))
                .bg(colors.scrollbar_track_background)
                .cursor_pointer()
                .child(
                    div()
                        .absolute()
                        .right(px(2.))
                        .top(gpui::relative(top_fraction))
                        .h(gpui::relative(thumb_fraction))
                        .w(px(6.))
                        .rounded_full()
                        .bg(colors.scrollbar_thumb_background),
                )
                .on_scroll_wheel(move |event, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height());
                    let offset = wheel_scroll.offset();
                    let minimum = -wheel_scroll.max_offset().y;
                    wheel_scroll
                        .set_offset(point(offset.x, (offset.y + delta.y).clamp(minimum, px(0.))));
                    wheel_handle.update(cx, |_, cx| cx.notify()).ok();
                    cx.stop_propagation();
                })
                .on_click(move |event, _, cx| {
                    let bounds = click_scroll.bounds();
                    let maximum = click_scroll.max_offset().y;
                    if bounds.size.height > px(0.) && maximum > px(0.) {
                        let progress = ((event.position().y - bounds.top()) / bounds.size.height)
                            .clamp(0., 1.);
                        let offset = click_scroll.offset();
                        click_scroll.set_offset(point(offset.x, -(maximum * progress)));
                        click_handle.update(cx, |_, cx| cx.notify()).ok();
                    }
                    cx.stop_propagation();
                })
                .into_any_element()
        };

        let text_input = |id: String, field: TextField, input: SettingsInput| -> gpui::AnyElement {
            Self::text_input_widget(
                id,
                field,
                input,
                editor.focused_input,
                colors.clone(),
                handle.clone(),
            )
        };

        let dropdown_state = DropdownRenderState {
            dropdown_index: editor.dropdown_index,
            dropdown_query: editor.dropdown_query.clone(),
            dropdown_filtered_options: editor.dropdown_filtered_options.clone(),
            dropdown_scroll: editor.dropdown_scroll.clone(),
            dropdown_anchor: editor.dropdown_anchor,
        };
        let dropdown =
            |id: String, label: String, selection: SettingsDropdown| -> gpui::AnyElement {
                let focused = editor.focused_control == Some(SettingsControl::Dropdown(selection));
                Self::dropdown_trigger_widget(
                    id,
                    label,
                    selection,
                    focused,
                    colors.clone(),
                    handle.clone(),
                )
            };

        let setting_row = |label: &'static str,
                           description: &'static str,
                           focused: bool,
                           control: gpui::AnyElement| {
            h_flex()
                .w_full()
                .min_h(px(54.))
                .px_2()
                .py_2()
                .gap_4()
                .justify_between()
                .border_b_1()
                .border_color(if focused {
                    colors.border_focused
                } else {
                    colors.border_variant
                })
                .when(focused, |row| row.bg(colors.element_selected))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(div().text_sm().text_color(colors.text).child(label))
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(description),
                        ),
                )
                .child(div().w(px(330.)).flex_none().child(control))
                .into_any_element()
        };

        let setting_toggle =
            |id: &'static str, value: bool, toggle: SettingsToggle| -> gpui::AnyElement {
                let toggle_handle = handle.clone();
                switch(id, value.into())
                    .label(if value { "On" } else { "Off" })
                    .full_width(true)
                    .aria_label(id)
                    .on_click(move |state, window, cx| {
                        toggle_handle
                            .update(cx, |this, cx| {
                                this.set_settings_toggle(toggle, state.selected(), window, cx);
                            })
                            .ok();
                    })
                    .into_any_element()
            };

        let numeric = |id: &'static str,
                       field: TextField,
                       setting: NumericSetting,
                       input: ConfigTextField|
         -> gpui::AnyElement {
            let focused = editor.focused_control == Some(SettingsControl::Numeric(setting));
            let decrease_down = handle.clone();
            let decrease_up = handle.clone();
            let decrease_out = handle.clone();
            let increase_down = handle.clone();
            let increase_up = handle.clone();
            let increase_out = handle.clone();
            h_flex()
                .id(id)
                .h_9()
                .w_full()
                .rounded(px(4.))
                .border_1()
                .border_color(if focused {
                    colors.border_focused
                } else {
                    colors.border
                })
                .bg(colors.editor_background)
                .child(
                    div()
                        .id(format!("{id}-decrease"))
                        .h_full()
                        .w_9()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child("−")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            decrease_down
                                .update(cx, |this, cx| this.begin_numeric_repeat(setting, -1, cx))
                                .ok();
                        })
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            decrease_up
                                .update(cx, |this, cx| this.end_numeric_repeat(cx))
                                .ok();
                        })
                        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                            decrease_out
                                .update(cx, |this, cx| this.end_numeric_repeat(cx))
                                .ok();
                        }),
                )
                .child(div().min_w_0().flex_1().child(text_input(
                    format!("{id}-value"),
                    field,
                    SettingsInput::Configuration(input),
                )))
                .child(
                    div()
                        .id(format!("{id}-increase"))
                        .h_full()
                        .w_9()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child("+")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            increase_down
                                .update(cx, |this, cx| this.begin_numeric_repeat(setting, 1, cx))
                                .ok();
                        })
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            increase_up
                                .update(cx, |this, cx| this.end_numeric_repeat(cx))
                                .ok();
                        })
                        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                            increase_out
                                .update(cx, |this, cx| this.end_numeric_repeat(cx))
                                .ok();
                        }),
                )
                .into_any_element()
        };
        let opacity_slider = |opacity: f32| -> gpui::AnyElement {
            let selected = (opacity.clamp(0., 1.) * 20.).round() as usize;
            let focused = editor.focused_control == Some(SettingsControl::Opacity);
            let stops = (0usize..=20)
                .map(|step| {
                    let slider_handle = handle.clone();
                    div()
                        .id(("inactive-opacity-stop", step))
                        .h_full()
                        .flex_1()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            slider_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut() {
                                        editor.configuration.inactive_pane_opacity =
                                            step as f32 / 20.;
                                        editor.configuration_dirty = true;
                                        editor.message = None;
                                        cx.notify();
                                    }
                                })
                                .ok();
                        })
                })
                .collect::<Vec<_>>();
            let fraction = selected as f32 / 20.;
            h_flex()
                .w_full()
                .gap_3()
                .rounded(px(4.))
                .border_1()
                .border_color(if focused {
                    colors.border_focused
                } else {
                    colors.border
                })
                .child(
                    div()
                        .relative()
                        .h_5()
                        .min_w_0()
                        .flex_1()
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
                                .w(gpui::relative(fraction))
                                .h_1()
                                .rounded_full()
                                .bg(colors.text_accent),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(gpui::relative(fraction))
                                .ml(px(-5.))
                                .size(px(10.))
                                .rounded_full()
                                .border_1()
                                .border_color(colors.border_focused)
                                .bg(colors.text_accent),
                        )
                        .child(h_flex().absolute().inset_0().children(stops)),
                )
                .child(
                    div()
                        .w(px(44.))
                        .text_right()
                        .text_sm()
                        .child(format!("{}%", selected * 5)),
                )
                .into_any_element()
        };

        let content = pages::render_settings_pages(
            editor,
            &colors,
            &handle,
            &cx.entity(),
            &scroll_indicator,
            &text_input,
            &dropdown,
            &setting_row,
            &setting_toggle,
            &numeric,
            &opacity_slider,
        );

        // The keymap list virtualizes its rows with `uniform_list`, which only clips to a
        // bounded viewport when its parent isn't itself `overflow: scroll` (an overflow-scroll
        // parent gives its child unconstrained height so it can be scrolled over, which would
        // make the list size itself to fit every row instead of virtualizing). So the keymap
        // page owns its own scroll region instead of sharing the generic one below.
        let scroll_region = if editor.page == SettingsPage::Keymap {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("settings-keymap-form")
                        .size_full()
                        .px_5()
                        .py_3()
                        .text_color(colors.text)
                        .child(content),
                )
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("settings-form-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&editor.settings_scroll)
                        .px_5()
                        .py_3()
                        .text_color(colors.text)
                        .child(content),
                )
                .child(scroll_indicator(
                    "settings-form-scrollbar".to_owned(),
                    &editor.settings_scroll,
                ))
                .into_any_element()
        };

        let font_modal = modals::render_font_modal(
            editor,
            &colors,
            &handle,
            close_button_on_left,
            &scroll_indicator,
            &text_input,
        );

        let profile_modal = modals::render_profile_modal(
            editor,
            &colors,
            &handle,
            close_button_on_left,
            &text_input,
            &dropdown,
        );

        let keymap_capture_modal = modals::render_keymap_capture_modal(editor, &colors, &handle);

        // Rendered once, as a sibling of the dialog content, regardless of which page or
        // row opened it (see `DropdownRenderState` for why it can't render inline).
        let dropdown_popup = editor.open_dropdown.map(|selection| {
            let (_, options) = Self::settings_dropdown_options(editor, selection);
            Self::dropdown_popup_widget(
                options,
                selection,
                colors.clone(),
                handle.clone(),
                dropdown_state.clone(),
            )
        });

        let config_handle = handle.clone();
        let themes_handle = handle.clone();
        let keymap_handle = handle.clone();
        let save_handle = handle.clone();
        let close_settings_button = || {
            let close_handle = handle.clone();
            IconButton::new("close-settings", IconName::Close)
                .icon_size(IconSize::Small)
                .toggle_state(editor.focused_control == Some(SettingsControl::Close))
                .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                .tooltip(Tooltip::text("Close settings"))
                .on_click(move |_, window, cx| {
                    close_handle
                        .update(cx, |this, cx| this.dismiss_settings(window, cx))
                        .ok();
                })
        };
        let path = match editor.page {
            SettingsPage::Configuration => self.launch_config.config_path.display().to_string(),
            SettingsPage::Themes => format!(
                "Zed theme extensions · installed in {}",
                config::themes_dir().display()
            ),
            SettingsPage::Keymap => self.launch_config.keymap_path.display().to_string(),
        };
        Some(
            div()
                .id("settings-backdrop")
                .absolute()
                .inset_0()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.3))
                .occlude()
                .child(
                    div()
                        .id("settings-editor")
                        .track_focus(&self.settings_focus)
                        .relative()
                        .size_full()
                        .max_w(px(980.))
                        .max_h(px(680.))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .h_12()
                                .px_3()
                                .flex_none()
                                .justify_between()
                                .border_b_1()
                                .border_color(colors.border)
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .when(close_button_on_left, |controls| {
                                            controls
                                                .child(div().mr_1().child(close_settings_button()))
                                        })
                                        .child(
                                            div()
                                                .id("settings-configuration-tab")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .when(
                                                    editor.page == SettingsPage::Configuration
                                                        || editor.focused_control
                                                            == Some(SettingsControl::Tab(
                                                                SettingsPage::Configuration,
                                                            )),
                                                    |tab| tab.bg(colors.element_selected),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    config_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_settings_page(
                                                                SettingsPage::Configuration,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                                .child("Configuration"),
                                        )
                                        .child(
                                            div()
                                                .id("settings-themes-tab")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .when(
                                                    editor.page == SettingsPage::Themes
                                                        || editor.focused_control
                                                            == Some(SettingsControl::Tab(
                                                                SettingsPage::Themes,
                                                            )),
                                                    |tab| tab.bg(colors.element_selected),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    themes_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_settings_page(
                                                                SettingsPage::Themes,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                                .child("Themes"),
                                        )
                                        .child(
                                            div()
                                                .id("settings-keymap-tab")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .when(
                                                    editor.page == SettingsPage::Keymap
                                                        || editor.focused_control
                                                            == Some(SettingsControl::Tab(
                                                                SettingsPage::Keymap,
                                                            )),
                                                    |tab| tab.bg(colors.element_selected),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    keymap_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_settings_page(
                                                                SettingsPage::Keymap,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                                .child("Keymap"),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("save-settings")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .border_1()
                                                .border_color(
                                                    if editor.focused_control
                                                        == Some(SettingsControl::Save)
                                                    {
                                                        colors.border_focused
                                                    } else {
                                                        colors.element_selected
                                                    },
                                                )
                                                .cursor_pointer()
                                                .bg(colors.element_selected)
                                                .hover(|style| style.bg(colors.element_hover))
                                                .on_click(move |_, window, cx| {
                                                    save_handle
                                                        .update(cx, |this, cx| {
                                                            this.save_settings(window, cx)
                                                        })
                                                        .ok();
                                                })
                                                .child(
                                                    if editor.configuration_dirty
                                                        || editor.keymap_dirty
                                                    {
                                                        "Save *"
                                                    } else {
                                                        "Save"
                                                    },
                                                ),
                                        )
                                        .when(!close_button_on_left, |controls| {
                                            controls.child(close_settings_button())
                                        }),
                                ),
                        )
                        .child(
                            h_flex()
                                .h_9()
                                .px_3()
                                .flex_none()
                                .border_b_1()
                                .border_color(colors.border)
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(path),
                        )
                        .child(scroll_region)
                        .when_some(editor.message.clone(), |dialog, (error, message)| {
                            dialog.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .border_t_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .text_color(if error {
                                        colors.text
                                    } else {
                                        colors.text_muted
                                    })
                                    .child(message),
                            )
                        })
                        .when_some(font_modal, |dialog, modal| dialog.child(modal))
                        .when_some(profile_modal, |dialog, modal| dialog.child(modal))
                        .when_some(keymap_capture_modal, |dialog, modal| dialog.child(modal))
                        .when_some(dropdown_popup, |dialog, popup| dialog.child(popup)),
                )
                .into_any_element(),
        )
    }
}
