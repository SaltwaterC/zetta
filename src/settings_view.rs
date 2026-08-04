use super::*;

use crate::startup::keymap_keystroke_display;

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
                .w(px(10.))
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
            let focused = editor.focused_input == Some(input);
            let centered = match input {
                SettingsInput::Configuration(
                    ConfigTextField::FontSize | ConfigTextField::ScrollHistory,
                ) => true,
                #[cfg(feature = "http-server")]
                SettingsInput::Configuration(ConfigTextField::HttpServerPort) => true,
                #[cfg(feature = "tftp-server")]
                SettingsInput::Configuration(ConfigTextField::TftpServerPort) => true,
                _ => false,
            };
            let cursor = field.cursor.min(field.text.len());
            let (before, after) = field.text.split_at(cursor);
            let input_handle = handle.clone();
            div()
                .id(id)
                .h_9()
                .w_full()
                .min_w(px(180.))
                .px_2()
                .flex()
                .items_center()
                .when(centered, |input| input.justify_center().text_center())
                .overflow_hidden()
                .rounded(px(4.))
                .border_1()
                .border_color(if focused {
                    colors.border_focused
                } else {
                    colors.border
                })
                .bg(colors.editor_background)
                .cursor_text()
                .when(field.select_all && focused, |input| {
                    input.bg(colors.element_selection_background)
                })
                .when(!focused, |input| {
                    input.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(field.text.clone()),
                    )
                })
                .when(focused, |input| {
                    input
                        .child(div().whitespace_nowrap().child(before.to_owned()))
                        .when(!field.select_all, |input| {
                            input.child(
                                div()
                                    .flex_none()
                                    .w(px(1.))
                                    .h(px(16.))
                                    .bg(colors.text_accent),
                            )
                        })
                        .child(div().whitespace_nowrap().child(after.to_owned()))
                })
                .on_click(move |_, window, cx| {
                    input_handle
                        .update(cx, |this, cx| this.focus_settings_input(input, window, cx))
                        .ok();
                })
                .into_any_element()
        };

        let dropdown = |id: String,
                        label: String,
                        options: Arc<[String]>,
                        selection: SettingsDropdown,
                        _window: &mut Window,
                        _cx: &mut Context<Self>|
         -> gpui::AnyElement {
            let menu_handle = handle.clone();
            let focused = editor.focused_control == Some(SettingsControl::Dropdown(selection));
            let open = editor.open_dropdown == Some(selection);
            let active_index = editor.dropdown_index.min(options.len().saturating_sub(1));
            let dropdown_query = editor.dropdown_query.clone();
            let matching_indices = (!dropdown_query.is_empty())
                .then(|| fuzzy_match_indices(&options, &dropdown_query));
            let no_matches = matching_indices
                .as_ref()
                .is_some_and(|indices| indices.is_empty());
            let trigger = ButtonLike::new(id.clone())
                .style(ButtonStyle::Outlined)
                .toggle_state(focused)
                .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                .full_width()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(Label::new(label))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
                )
                .on_click(move |_, window, cx| {
                    menu_handle
                        .update(cx, |this, cx| {
                            this.focus_settings_control_without_scroll(
                                SettingsControl::Dropdown(selection),
                                window,
                                cx,
                            );
                            this.open_settings_dropdown(selection, cx);
                        })
                        .ok();
                });
            let option_handle = handle.clone();
            let option_row = |index: usize, option: &String| {
                let value = option.clone();
                let selected = index == active_index;
                let handle = option_handle.clone();
                div()
                    .id(format!("{id}-option-{index}"))
                    .px_2()
                    .py_1()
                    .rounded(px(3.))
                    .cursor_pointer()
                    .when(selected, |row| row.bg(colors.element_selected))
                    .hover(|style| style.bg(colors.element_hover))
                    .child(value.clone())
                    .on_click(move |_, _, cx| {
                        handle
                            .update(cx, |this, cx| {
                                this.set_settings_dropdown(selection, value.clone(), cx);
                                if let Some(editor) = this.settings_editor.as_mut() {
                                    editor.open_dropdown = None;
                                }
                                cx.notify();
                            })
                            .ok();
                    })
            };
            let option_rows = matching_indices
                .as_ref()
                .map(|indices| {
                    indices
                        .iter()
                        .map(|index| option_row(*index, &options[*index]))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| {
                    options
                        .iter()
                        .enumerate()
                        .map(|(index, option)| option_row(index, option))
                        .collect::<Vec<_>>()
                });
            div()
                .relative()
                .flex()
                .flex_col()
                .child(trigger)
                .when(open, |dropdown| {
                    dropdown.child(
                        deferred(
                            anchored()
                                .position_mode(AnchoredPositionMode::Local)
                                .position(point(px(0.), px(40.)))
                                .snap_to_window_with_margin(px(8.))
                                .child(
                                    div()
                                        .id(format!("{id}-options"))
                                        .min_w(px(180.))
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(colors.border_focused)
                                        .bg(colors.elevated_surface_background)
                                        .overflow_hidden()
                                        .when(!dropdown_query.is_empty(), |menu| {
                                            menu.child(
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child(format!("Search: {dropdown_query}")),
                                            )
                                        })
                                        .child(
                                            div()
                                                .id(format!("{id}-options-list"))
                                                .max_h(px(260.))
                                                .overflow_y_scroll()
                                                .track_scroll(&editor.dropdown_scroll)
                                                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                                                .p_1()
                                                .when(no_matches, |list| {
                                                    list.child(
                                                        div()
                                                            .px_2()
                                                            .py_1()
                                                            .text_color(colors.text_muted)
                                                            .child("No matches"),
                                                    )
                                                })
                                                .children(option_rows),
                                        ),
                                ),
                        )
                        .with_priority(1),
                    )
                })
                .into_any_element()
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

        let content = match editor.page {
            SettingsPage::Configuration => {
                let configuration = &editor.configuration;
                let mut profile_names = editor.profile_names.to_vec();
                profile_names.extend(
                    configuration
                        .profiles
                        .iter()
                        .map(|profile| profile.name.text.clone())
                        .filter(|name| !name.trim().is_empty()),
                );
                profile_names.sort();
                profile_names.dedup();
                let default_profile = dropdown(
                    "settings-default-profile".to_owned(),
                    configuration.default_profile.clone(),
                    profile_names.into(),
                    SettingsDropdown::DefaultProfile,
                    window,
                    cx,
                );
                let new_tab_profile = dropdown(
                    "settings-new-tab-profile".to_owned(),
                    configuration.new_tab_profile.label().to_owned(),
                    vec!["Default".to_owned(), "Inherit".to_owned()].into(),
                    SettingsDropdown::NewTabProfile,
                    window,
                    cx,
                );
                let theme = dropdown(
                    "settings-theme".to_owned(),
                    configuration.theme.clone(),
                    editor.themes.clone(),
                    SettingsDropdown::Theme,
                    window,
                    cx,
                );
                let current_default_tab_icon = configuration.default_tab_icon;
                let default_tab_icon_handle = handle.clone();
                let default_tab_icon = h_flex()
                    .id("default-tab-icon-picker-trigger")
                    .h_9()
                    .w_full()
                    .min_w(px(180.))
                    .px_3()
                    .justify_between()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(
                        if editor.focused_control == Some(SettingsControl::DefaultTabIconPicker) {
                            colors.border_focused
                        } else {
                            colors.border
                        },
                    )
                    .bg(colors.editor_background)
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.element_hover))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(
                                current_default_tab_icon.unwrap_or(IconName::Dash),
                            ))
                            .child(
                                current_default_tab_icon
                                    .map(tab_icon_label)
                                    .unwrap_or_else(|| "None".to_owned()),
                            ),
                    )
                    .child(
                        svg()
                            .path(IconName::ChevronDown.path())
                            .size(px(14.))
                            .text_color(colors.icon_muted),
                    )
                    .on_click(move |_, window, cx| {
                        default_tab_icon_handle
                            .update(cx, |this, cx| {
                                this.open_default_tab_icon_picker(window, cx);
                            })
                            .ok();
                    })
                    .into_any_element();
                let working_directory_scope = dropdown(
                    "settings-working-directory-scope".to_owned(),
                    configuration.working_directory_scope.label().to_owned(),
                    vec!["None".to_owned(), "Pane".to_owned(), "Tab".to_owned()].into(),
                    SettingsDropdown::WorkingDirectoryScope,
                    window,
                    cx,
                );
                let pane_controls_position = dropdown(
                    "settings-pane-controls-position".to_owned(),
                    configuration.pane_controls_position.label().to_owned(),
                    vec!["Right".to_owned(), "Left".to_owned()].into(),
                    SettingsDropdown::PaneControlsPosition,
                    window,
                    cx,
                );
                let pane_controls_default_visibility = dropdown(
                    "settings-pane-controls-default-visibility".to_owned(),
                    if configuration.pane_controls_hidden_by_default {
                        "Hidden".to_owned()
                    } else {
                        "Visible".to_owned()
                    },
                    vec!["Visible".to_owned(), "Hidden".to_owned()].into(),
                    SettingsDropdown::PaneControlsDefaultVisibility,
                    window,
                    cx,
                );
                let current_font = configuration.terminal_font_family.clone();
                let picker_handle = handle.clone();
                let font_family = h_flex()
                    .id("terminal-font-family-picker-trigger")
                    .h_9()
                    .w_full()
                    .min_w(px(180.))
                    .px_3()
                    .justify_between()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(
                        if editor.focused_control == Some(SettingsControl::FontPicker) {
                            colors.border_focused
                        } else {
                            colors.border
                        },
                    )
                    .bg(colors.editor_background)
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.element_hover))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(current_font.clone())
                            .child(current_font),
                    )
                    .child(
                        svg()
                            .path(IconName::ChevronDown.path())
                            .size(px(14.))
                            .text_color(colors.icon_muted),
                    )
                    .on_click(move |_, window, cx| {
                        picker_handle
                            .update(cx, |this, cx| {
                                if let Some(editor) = this.settings_editor.as_mut() {
                                    editor.font_query = Some(TextField::default());
                                    editor.scroll_geometry_initialized = false;
                                }
                                this.focus_settings_input(SettingsInput::FontSearch, window, cx);
                            })
                            .ok();
                    })
                    .into_any_element();
                let mut rows = vec![
                    setting_row(
                        "Default profile",
                        "Profile selected when Zetta starts",
                        editor.focused_control
                            == Some(SettingsControl::Dropdown(SettingsDropdown::DefaultProfile)),
                        default_profile,
                    ),
                    setting_row(
                        "New Tab profile",
                        "Profile selected when opening a new tab",
                        editor.focused_control
                            == Some(SettingsControl::Dropdown(SettingsDropdown::NewTabProfile)),
                        new_tab_profile,
                    ),
                    setting_row(
                        "Theme",
                        "Application color theme",
                        editor.focused_control
                            == Some(SettingsControl::Dropdown(SettingsDropdown::Theme)),
                        theme,
                    ),
                    setting_row(
                        "Default tab icon",
                        "Icon shown on new tabs; choose None to hide it",
                        editor.focused_control == Some(SettingsControl::DefaultTabIconPicker),
                        default_tab_icon,
                    ),
                    setting_row(
                        "Terminal font size",
                        "Point size from 6 through 100",
                        editor.focused_control
                            == Some(SettingsControl::Numeric(NumericSetting::FontSize)),
                        numeric(
                            "settings-font-size",
                            configuration.terminal_font_size.clone(),
                            NumericSetting::FontSize,
                            ConfigTextField::FontSize,
                        ),
                    ),
                    setting_row(
                        "Terminal font family",
                        "Search bundled and system-installed font families",
                        editor.focused_control == Some(SettingsControl::FontPicker),
                        font_family,
                    ),
                    setting_row(
                        "Working directory",
                        "Initial directory; ~ expands to your home directory",
                        editor.focused_control
                            == Some(SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::WorkingDirectory,
                            ))),
                        text_input(
                            "settings-working-directory".to_owned(),
                            configuration.working_directory.clone(),
                            SettingsInput::Configuration(ConfigTextField::WorkingDirectory),
                        ),
                    ),
                    setting_row(
                        "Inherit working directory scope",
                        "Choose which new shells inherit the active pane's current directory",
                        editor.focused_control
                            == Some(SettingsControl::Dropdown(
                                SettingsDropdown::WorkingDirectoryScope,
                            )),
                        working_directory_scope,
                    ),
                    setting_row(
                        "Scrollback history",
                        "Enter 0 through Max; steppers accelerate across the range",
                        editor.focused_control
                            == Some(SettingsControl::Numeric(NumericSetting::ScrollHistory)),
                        numeric(
                            "settings-scroll-history",
                            configuration.max_scroll_history_lines.clone(),
                            NumericSetting::ScrollHistory,
                            ConfigTextField::ScrollHistory,
                        ),
                    ),
                    setting_row(
                        "Inactive pane opacity",
                        "Dimming level as a percentage",
                        editor.focused_control == Some(SettingsControl::Opacity),
                        opacity_slider(configuration.inactive_pane_opacity),
                    ),
                    setting_row(
                        "Hide pane size",
                        "Hide the active pane dimensions from the title bar",
                        editor.focused_control
                            == Some(SettingsControl::Toggle(SettingsToggle::PaneSize)),
                        setting_toggle(
                            "settings-hide-pane-size",
                            configuration.hide_pane_size,
                            SettingsToggle::PaneSize,
                        ),
                    ),
                    setting_row(
                        "Hide title bar labels",
                        "Hide text such as Menu, Profile, and Keep running",
                        editor.focused_control
                            == Some(SettingsControl::Toggle(SettingsToggle::TitleBarLabels)),
                        setting_toggle(
                            "settings-hide-title-bar-labels",
                            configuration.hide_title_bar_labels,
                            SettingsToggle::TitleBarLabels,
                        ),
                    ),
                    setting_row(
                        "Hide title bar buttons",
                        "Hide title bar buttons such as Keep running and Detach",
                        editor.focused_control
                            == Some(SettingsControl::Toggle(SettingsToggle::TitleBarButtons)),
                        setting_toggle(
                            "settings-hide-title-bar-buttons",
                            configuration.hide_title_bar_buttons,
                            SettingsToggle::TitleBarButtons,
                        ),
                    ),
                    #[cfg(target_os = "macos")]
                    setting_row(
                        "Hide title bar menus",
                        "Hide the Menu and Profile menus from the title bar",
                        editor.focused_control
                            == Some(SettingsControl::Toggle(SettingsToggle::TitleBarMenus)),
                        setting_toggle(
                            "settings-hide-title-bar-menus",
                            configuration.hide_title_bar_menus,
                            SettingsToggle::TitleBarMenus,
                        ),
                    ),
                    setting_row(
                        "Pane controls position",
                        "Keep pane overlay controls on the right or move them to the left",
                        editor.focused_control
                            == Some(SettingsControl::Dropdown(
                                SettingsDropdown::PaneControlsPosition,
                            )),
                        pane_controls_position,
                    ),
                    setting_row(
                        "Pane controls by default",
                        "Start new panes with overlay controls visible or hidden",
                        editor.focused_control
                            == Some(SettingsControl::Dropdown(
                                SettingsDropdown::PaneControlsDefaultVisibility,
                            )),
                        pane_controls_default_visibility,
                    ),
                ];
                #[cfg(feature = "http-server")]
                rows.push(setting_row(
                    "HTTP server port",
                    "TCP port used when starting the static HTTP server",
                    editor.focused_control
                        == Some(SettingsControl::Numeric(NumericSetting::HttpServerPort)),
                    numeric(
                        "settings-http-server-port",
                        configuration.http_server_port.clone(),
                        NumericSetting::HttpServerPort,
                        ConfigTextField::HttpServerPort,
                    ),
                ));
                #[cfg(feature = "tftp-server")]
                rows.push(setting_row(
                    "TFTP server port",
                    "UDP port used when starting the TFTP server",
                    editor.focused_control
                        == Some(SettingsControl::Numeric(NumericSetting::TftpServerPort)),
                    numeric(
                        "settings-tftp-server-port",
                        configuration.tftp_server_port.clone(),
                        NumericSetting::TftpServerPort,
                        ConfigTextField::TftpServerPort,
                    ),
                ));
                rows.push(
                    div()
                        .pt_4()
                        .pb_2()
                        .text_sm()
                        .text_color(colors.text_muted)
                        .child("Profiles")
                        .into_any_element(),
                );
                for (index, profile) in configuration.profiles.iter().enumerate() {
                    let profile_focused = editor.focused_control
                        == Some(SettingsControl::Dropdown(SettingsDropdown::ProfileTheme(
                            index,
                        )))
                        || editor.focused_control
                            == Some(SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileName(index),
                            )))
                        || editor.focused_control
                            == Some(SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileProgram(index),
                            )))
                        || editor.focused_control
                            == Some(SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileArguments(index),
                            )))
                        || editor.focused_control
                            == Some(SettingsControl::Toggle(SettingsToggle::ProfileVisibility(
                                index,
                            )))
                        || editor.focused_control == Some(SettingsControl::RemoveProfile(index));
                    let mut theme_options = vec!["Use application theme".to_owned()];
                    theme_options.extend(editor.themes.iter().cloned());
                    let profile_theme = profile
                        .theme
                        .clone()
                        .unwrap_or_else(|| "Use application theme".to_owned());
                    let profile_theme = dropdown(
                        format!("settings-profile-{index}-theme"),
                        profile_theme,
                        theme_options.into(),
                        SettingsDropdown::ProfileTheme(index),
                        window,
                        cx,
                    );
                    let card = if profile.detected {
                        let visibility_handle = handle.clone();
                        let profile_visibility = switch(
                            format!("settings-profile-{index}-visibility"),
                            (!profile.hidden).into(),
                        )
                        .label(if profile.hidden { "Hidden" } else { "Visible" })
                        .full_width(true)
                        .aria_label("Show profile in Profiles menu")
                        .on_click(move |state, window, cx| {
                            visibility_handle
                                .update(cx, |this, cx| {
                                    this.set_settings_toggle(
                                        SettingsToggle::ProfileVisibility(index),
                                        state.selected(),
                                        window,
                                        cx,
                                    );
                                })
                                .ok();
                        });
                        h_flex()
                            .p_3()
                            .mb_2()
                            .gap_4()
                            .justify_between()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(if profile_focused {
                                colors.border_focused
                            } else {
                                colors.border
                            })
                            .bg(if profile_focused {
                                colors.element_selected
                            } else {
                                colors.editor_background
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(colors.text)
                                            .child(profile.name.text.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors.text_muted)
                                            .child("Detected profile"),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w(px(480.))
                                    .flex_none()
                                    .gap_3()
                                    .child(profile_visibility)
                                    .child(div().w(px(330.)).flex_none().child(profile_theme)),
                            )
                            .into_any_element()
                    } else {
                        let remove_handle = handle.clone();
                        div()
                            .p_3()
                            .mb_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(if profile_focused {
                                colors.border_focused
                            } else {
                                colors.border
                            })
                            .bg(if profile_focused {
                                colors.element_selected
                            } else {
                                colors.editor_background
                            })
                            .child(
                                h_flex()
                                    .items_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .mb_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child("Profile name"),
                                            )
                                            .child(text_input(
                                                format!("settings-profile-{index}-name"),
                                                profile.name.clone(),
                                                SettingsInput::Configuration(
                                                    ConfigTextField::ProfileName(index),
                                                ),
                                            )),
                                    )
                                    .child(
                                        IconButton::new(
                                            ("remove-settings-profile", index),
                                            IconName::Trash,
                                        )
                                        .icon_size(IconSize::Small)
                                        .toggle_state(
                                            editor.focused_control
                                                == Some(SettingsControl::RemoveProfile(index)),
                                        )
                                        .selected_style(ButtonStyle::OutlinedCustom(
                                            colors.border_focused,
                                        ))
                                        .tooltip(Tooltip::text("Remove profile"))
                                        .on_click(
                                            move |_, _, cx| {
                                                remove_handle
                                                    .update(cx, |this, cx| {
                                                        if let Some(editor) =
                                                            this.settings_editor.as_mut()
                                                        {
                                                            editor
                                                                .configuration
                                                                .profiles
                                                                .remove(index);
                                                            editor.configuration_dirty = true;
                                                            cx.notify();
                                                        }
                                                    })
                                                    .ok();
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .mt_2()
                                    .items_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .mb_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child("Program"),
                                            )
                                            .child(text_input(
                                                format!("settings-profile-{index}-program"),
                                                profile.program.clone(),
                                                SettingsInput::Configuration(
                                                    ConfigTextField::ProfileProgram(index),
                                                ),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .mb_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child("Arguments (comma separated)"),
                                            )
                                            .child(text_input(
                                                format!("settings-profile-{index}-arguments"),
                                                profile.arguments.clone(),
                                                SettingsInput::Configuration(
                                                    ConfigTextField::ProfileArguments(index),
                                                ),
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .child(
                                        div()
                                            .mb_1()
                                            .text_xs()
                                            .text_color(colors.text_muted)
                                            .child("Theme"),
                                    )
                                    .child(profile_theme),
                            )
                            .into_any_element()
                    };
                    rows.push(card);
                }
                let add_handle = handle.clone();
                rows.push(
                    div()
                        .id("add-settings-profile")
                        .h_9()
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(
                            if editor.focused_control == Some(SettingsControl::AddProfile) {
                                colors.border_focused
                            } else {
                                colors.border
                            },
                        )
                        .when(
                            editor.focused_control == Some(SettingsControl::AddKeymapSection),
                            |button| button.bg(colors.element_selected),
                        )
                        .when(
                            editor.focused_control == Some(SettingsControl::AddProfile),
                            |button| button.bg(colors.element_selected),
                        )
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child("Add profile")
                        .on_click(move |_, window, cx| {
                            add_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut() {
                                        editor.profile_draft = Some(settings_editor::ProfileForm {
                                            name: TextField::default(),
                                            program: TextField::default(),
                                            arguments: TextField::default(),
                                            theme: None,
                                            hidden: false,
                                            detected: false,
                                        });
                                        editor.message = None;
                                    }
                                    this.focus_settings_input(
                                        SettingsInput::ProfileDraft(ProfileDraftField::Name),
                                        window,
                                        cx,
                                    );
                                })
                                .ok();
                        })
                        .into_any_element(),
                );
                div().children(rows).into_any_element()
            }
            SettingsPage::Themes => {
                let search = text_input(
                    "settings-theme-extension-search".to_owned(),
                    editor.theme_extension_query.clone(),
                    SettingsInput::ThemeSearch,
                );
                let search_handle = handle.clone();
                let mut rows = vec![
                    div()
                        .mb_3()
                        .child(
                            div()
                                .mb_1()
                                .text_sm()
                                .child("Download themes from Zed extensions"),
                        )
                        .child(
                            div()
                                .mb_3()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(
                                    "Only declared theme JSON files are installed. Other extension features are ignored.",
                                )
                                .child(
                                    div().mt_1().child(
                                        ButtonLink::new(
                                            "Browse the Zed themes store",
                                            "https://zed.dev/extensions?filter=themes",
                                        )
                                        .label_size(LabelSize::Small),
                                    ),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().child(search))
                                .child(
                                    div()
                                        .id("search-theme-extensions")
                                        .h_9()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(
                                            if editor.focused_control
                                                == Some(SettingsControl::SearchThemes)
                                            {
                                                colors.border_focused
                                            } else {
                                                colors.border
                                            },
                                        )
                                        .when(
                                            editor.focused_control
                                                == Some(SettingsControl::SearchThemes),
                                            |button| button.bg(colors.element_selected),
                                        )
                                        .cursor_pointer()
                                        .hover(|style| style.bg(colors.element_hover))
                                        .on_click(move |_, window, cx| {
                                            search_handle
                                                .update(cx, |this, cx| {
                                                    this.fetch_theme_extensions(window, cx)
                                                })
                                                .ok();
                                        })
                                        .child(if editor.theme_extensions_loading {
                                            "Loading…"
                                        } else {
                                            "Search"
                                        }),
                                ),
                        )
                        .into_any_element(),
                ];
                if !editor.installed_theme_extensions.is_empty() {
                    rows.push(
                        div()
                            .mt_2()
                            .mb_2()
                            .text_sm()
                            .child("Installed from Zed extensions")
                            .into_any_element(),
                    );
                    for installed in &editor.installed_theme_extensions {
                        let id = installed.id.clone();
                        let removing = editor
                            .theme_extension_downloading
                            .as_ref()
                            .is_some_and(|active| active.as_ref() == installed.id);
                        let disabled = editor.theme_extension_downloading.is_some();
                        let remove_handle = handle.clone();
                        let theme_names = installed.theme_names.join(", ");
                        let focused = editor.focused_control
                            == Some(SettingsControl::RemoveTheme(installed.id.clone()));
                        rows.push(
                            div()
                                .mb_2()
                                .p_3()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(if focused {
                                    colors.border_focused
                                } else {
                                    colors.border
                                })
                                .bg(if focused {
                                    colors.element_selected
                                } else {
                                    colors.editor_background
                                })
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .gap_3()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .child(div().text_sm().child(installed.id.clone()))
                                                .child(
                                                    div()
                                                        .mt_1()
                                                        .text_xs()
                                                        .text_color(colors.text_muted)
                                                        .child(format!(
                                                            "{} theme file{}{}",
                                                            installed.file_count,
                                                            if installed.file_count == 1 {
                                                                ""
                                                            } else {
                                                                "s"
                                                            },
                                                            if theme_names.is_empty() {
                                                                String::new()
                                                            } else {
                                                                format!(" · {theme_names}")
                                                            }
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id(format!(
                                                    "remove-theme-extension-{}",
                                                    installed.id
                                                ))
                                                .h_8()
                                                .px_3()
                                                .flex()
                                                .items_center()
                                                .flex_none()
                                                .rounded(px(4.))
                                                .border_1()
                                                .border_color(
                                                    if editor.focused_control
                                                        == Some(SettingsControl::RemoveTheme(
                                                            installed.id.clone(),
                                                        ))
                                                    {
                                                        colors.border_focused
                                                    } else {
                                                        colors.border
                                                    },
                                                )
                                                .when(!disabled, |button| {
                                                    button
                                                        .cursor_pointer()
                                                        .hover(|style| {
                                                            style.bg(colors.element_hover)
                                                        })
                                                        .on_click(move |_, window, cx| {
                                                            remove_handle
                                                                .update(cx, |this, cx| {
                                                                    this.remove_theme_extension(
                                                                        id.clone(),
                                                                        window,
                                                                        cx,
                                                                    )
                                                                })
                                                                .ok();
                                                        })
                                                })
                                                .child(if removing {
                                                    "Removing…"
                                                } else {
                                                    "Remove"
                                                }),
                                        ),
                                )
                                .into_any_element(),
                        );
                    }
                }
                if editor.theme_extensions.is_empty() && !editor.theme_extensions_loading {
                    rows.push(
                        div()
                            .py_6()
                            .text_center()
                            .text_color(colors.text_muted)
                            .child(if editor.theme_extensions_searched {
                                "No matching theme extensions found."
                            } else {
                                "Enter a theme name and select Search."
                            })
                            .into_any_element(),
                    );
                }
                for extension in &editor.theme_extensions {
                    let id = extension.id.clone();
                    let downloading = editor
                        .theme_extension_downloading
                        .as_ref()
                        .is_some_and(|active| active == &id);
                    let already_installed = editor
                        .installed_theme_extensions
                        .iter()
                        .any(|installed| installed.id == extension.id.as_ref());
                    let disabled =
                        editor.theme_extension_downloading.is_some() || already_installed;
                    let install_handle = handle.clone();
                    let focused = editor.focused_control
                        == Some(SettingsControl::InstallTheme(extension.id.clone()));
                    let description = extension
                        .description
                        .clone()
                        .unwrap_or_else(|| "Theme extension for Zed".to_owned());
                    let author = if extension.authors.is_empty() {
                        String::new()
                    } else {
                        format!(" by {}", extension.authors.join(", "))
                    };
                    rows.push(
                        div()
                            .mb_2()
                            .p_3()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if focused {
                                colors.border_focused
                            } else {
                                colors.border
                            })
                            .bg(if focused {
                                colors.element_selected
                            } else {
                                colors.editor_background
                            })
                            .child(
                                h_flex()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .child(format!("{}{}", extension.name, author)),
                                            )
                                            .child(
                                                div()
                                                    .mt_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child(description),
                                            )
                                            .child(
                                                div()
                                                    .mt_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child(format!(
                                                        "{} downloads · version {}",
                                                        extension.download_count, extension.version
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id(format!("install-theme-extension-{}", extension.id))
                                            .h_8()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .flex_none()
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(
                                                if editor.focused_control
                                                    == Some(SettingsControl::InstallTheme(
                                                        extension.id.clone(),
                                                    ))
                                                {
                                                    colors.border_focused
                                                } else {
                                                    colors.border
                                                },
                                            )
                                            .when(!disabled, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(colors.element_hover))
                                                    .on_click(move |_, window, cx| {
                                                        install_handle
                                                            .update(cx, |this, cx| {
                                                                this.download_theme_extension(
                                                                    id.clone(),
                                                                    window,
                                                                    cx,
                                                                )
                                                            })
                                                            .ok();
                                                    })
                                            })
                                            .child(if downloading {
                                                "Installing…"
                                            } else if already_installed {
                                                "Installed"
                                            } else {
                                                "Install"
                                            }),
                                    ),
                            )
                            .into_any_element(),
                    );
                }
                div().children(rows).into_any_element()
            }
            SettingsPage::Keymap => {
                let mut sections = Vec::new();
                sections.push(
                    div()
                        .mb_3()
                        .text_sm()
                        .child("Type an accelerator in the field, or click Record to capture one from your keyboard.")
                        .child(
                            div()
                                .mt_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Recording opens a confirmation dialog: press Return to use the captured shortcut or Esc to cancel."),
                        )
                        .into_any_element(),
                );
                for (section_index, section) in editor.keymap.sections.iter().enumerate() {
                    let section_focused = matches!(
                        editor.focused_control.as_ref(),
                        Some(SettingsControl::Input(SettingsInput::Keymap(
                            KeymapTextField::Context(index)
                        ))) if *index == section_index
                    ) || editor.focused_control
                        == Some(SettingsControl::AddBinding(section_index));
                    let mut bindings = Vec::new();
                    for (binding_index, binding) in section.bindings.iter().enumerate() {
                        let binding_focused = editor.focused_control
                            == Some(SettingsControl::Input(SettingsInput::Keymap(
                                KeymapTextField::Keystroke(section_index, binding_index),
                            )))
                            || editor.focused_control
                                == Some(SettingsControl::RemoveBinding(
                                    section_index,
                                    binding_index,
                                ))
                            || editor.focused_control
                                == Some(SettingsControl::CaptureKeymap(
                                    KeymapTextField::Keystroke(section_index, binding_index),
                                ));
                        let action = dropdown(
                            format!("settings-binding-{section_index}-{binding_index}-action"),
                            binding.action_name(),
                            editor.actions.clone(),
                            SettingsDropdown::BindingAction(section_index, binding_index),
                            window,
                            cx,
                        );
                        let template = binding.action_parameter("name").map(|name| {
                            dropdown(
                                format!(
                                    "settings-binding-{section_index}-{binding_index}-template"
                                ),
                                name,
                                editor.pane_template_names.clone(),
                                SettingsDropdown::BindingTemplate(section_index, binding_index),
                                window,
                                cx,
                            )
                        });
                        let profile = binding.action_usize_parameter("slot").map(|slot| {
                            let name = editor
                                .profile_names
                                .get(slot.saturating_sub(1))
                                .cloned()
                                .unwrap_or_else(|| format!("Profile {slot}"));
                            dropdown(
                                format!("settings-binding-{section_index}-{binding_index}-profile"),
                                name,
                                editor.profile_names.clone(),
                                SettingsDropdown::BindingProfile(section_index, binding_index),
                                window,
                                cx,
                            )
                        });
                        let remove_handle = handle.clone();
                        let capture_handle = handle.clone();
                        bindings.push(
                            h_flex()
                                .mb_2()
                                .p_1()
                                .gap_2()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(if binding_focused {
                                    colors.border_focused
                                } else {
                                    colors.border_variant
                                })
                                .when(binding_focused, |row| {
                                    row.bg(colors.element_selected)
                                })
                                .child(
                                    h_flex()
                                        .w(px(330.))
                                        .gap_1()
                                        .flex_none()
                                        .child(text_input(
                                            format!(
                                                "settings-binding-{section_index}-{binding_index}-key"
                                            ),
                                            binding.keystroke.clone(),
                                            SettingsInput::Keymap(
                                                KeymapTextField::Keystroke(
                                                    section_index,
                                                    binding_index,
                                                ),
                                            ),
                                        ))
                                        .child(
                                            Button::new(
                                                format!(
                                                    "record-settings-binding-{section_index}-{binding_index}"
                                                ),
                                                "Record",
                                            )
                                            .style(ButtonStyle::Outlined)
                                            .size(ButtonSize::Compact)
                                            .on_click(move |_, window, cx| {
                                                capture_handle
                                                    .update(cx, |this, cx| {
                                                        this.start_keymap_capture(
                                                            KeymapTextField::Keystroke(
                                                                section_index,
                                                                binding_index,
                                                            ),
                                                            window,
                                                            cx,
                                                        )
                                                    })
                                                    .ok();
                                            }),
                                        ),
                                )
                                .child(div().min_w_0().flex_1().child(action))
                                .when_some(template, |row, template| {
                                    row.child(div().w(px(180.)).flex_none().child(template))
                                })
                                .when_some(profile, |row, profile| {
                                    row.child(div().w(px(180.)).flex_none().child(profile))
                                })
                                .child(
                                    IconButton::new(
                                        format!("remove-settings-binding-{section_index}-{binding_index}"),
                                        IconName::Trash,
                                    )
                                    .icon_size(IconSize::Small)
                                    .toggle_state(
                                        editor.focused_control
                                            == Some(SettingsControl::RemoveBinding(
                                                section_index,
                                                binding_index,
                                            )),
                                    )
                                    .selected_style(ButtonStyle::OutlinedCustom(
                                        colors.border_focused,
                                    ))
                                    .tooltip(Tooltip::text("Remove binding"))
                                    .on_click(move |_, _, cx| {
                                        remove_handle
                                            .update(cx, |this, cx| {
                                                if let Some(editor) =
                                                    this.settings_editor.as_mut()
                                                {
                                                    editor.keymap.sections[section_index]
                                                        .bindings
                                                        .remove(binding_index);
                                                    editor.keymap_dirty = true;
                                                    cx.notify();
                                                }
                                            })
                                            .ok();
                                    }),
                                )
                                .into_any_element(),
                        );
                    }
                    let add_handle = handle.clone();
                    sections.push(
                        div()
                            .p_3()
                            .mb_3()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(if section_focused {
                                colors.border_focused
                            } else {
                                colors.border
                            })
                            .bg(if section_focused {
                                colors.element_selected
                            } else {
                                colors.editor_background
                            })
                            .child(
                                h_flex()
                                    .mb_3()
                                    .gap_2()
                                    .child(div().flex_none().text_sm().child("Context"))
                                    .child(div().min_w_0().flex_1().child(text_input(
                                        format!("settings-keymap-section-{section_index}-context"),
                                        section.context.clone(),
                                        SettingsInput::Keymap(KeymapTextField::Context(
                                            section_index,
                                        )),
                                    ))),
                            )
                            .children(bindings)
                            .child(
                                div()
                                    .id(("add-settings-binding", section_index))
                                    .h_8()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(
                                        if editor.focused_control
                                            == Some(SettingsControl::AddBinding(section_index))
                                        {
                                            colors.border_focused
                                        } else {
                                            colors.border
                                        },
                                    )
                                    .when(
                                        editor.focused_control
                                            == Some(SettingsControl::AddBinding(section_index)),
                                        |button| button.bg(colors.element_selected),
                                    )
                                    .cursor_pointer()
                                    .hover(|style| style.bg(colors.element_hover))
                                    .child("Add binding")
                                    .on_click(move |_, _, cx| {
                                        add_handle
                                            .update(cx, |this, cx| {
                                                if let Some(editor) = this.settings_editor.as_mut()
                                                {
                                                    editor.keymap.sections[section_index]
                                                        .bindings
                                                        .push(BindingForm {
                                                            keystroke: TextField::new(
                                                                "ctrl-shift-x",
                                                            ),
                                                            action: serde_json::Value::String(
                                                                "zetta::NewTab".to_owned(),
                                                            ),
                                                        });
                                                    editor.keymap_dirty = true;
                                                    cx.notify();
                                                }
                                            })
                                            .ok();
                                    }),
                            )
                            .into_any_element(),
                    );
                }
                let add_handle = handle.clone();
                sections.push(
                    div()
                        .id("add-keymap-section")
                        .h_9()
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(
                            if editor.focused_control == Some(SettingsControl::AddKeymapSection) {
                                colors.border_focused
                            } else {
                                colors.border
                            },
                        )
                        .when(
                            editor.focused_control == Some(SettingsControl::AddKeymapSection),
                            |button| button.bg(colors.element_selected),
                        )
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child("Add keymap context")
                        .on_click(move |_, _, cx| {
                            add_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut() {
                                        editor
                                            .keymap
                                            .sections
                                            .push(KeymapSectionForm::new("Zetta > Terminal"));
                                        editor.keymap_dirty = true;
                                        cx.notify();
                                    }
                                })
                                .ok();
                        })
                        .into_any_element(),
                );
                div().children(sections).into_any_element()
            }
        };

        let font_modal = editor.font_query.as_ref().map(|query| {
            let current_font = editor.configuration.terminal_font_family.clone();
            let close_font_picker_button = || {
                let close_handle = handle.clone();
                IconButton::new("close-font-picker", IconName::Close)
                    .icon_size(IconSize::Small)
                    .toggle_state(editor.focused_control == Some(SettingsControl::CloseFontPicker))
                    .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                    .tooltip(Tooltip::text("Close font picker"))
                    .on_click(move |_, _, cx| {
                        close_handle
                            .update(cx, |this, cx| {
                                if let Some(editor) = this.settings_editor.as_mut() {
                                    editor.font_query = None;
                                    editor.focused_input = None;
                                    cx.notify();
                                }
                            })
                            .ok();
                    })
            };
            let filtered_fonts = matching_font_indices(&editor.normalized_fonts, &query.text);
            let fonts = editor.fonts.clone();
            let font_handle = handle.clone();
            let font_colors = colors.clone();
            let focused_control = editor.focused_control.clone();
            let font_rows = uniform_list(
                "settings-font-list",
                filtered_fonts.len(),
                move |range, _, _| {
                    range
                        .map(|row_index| {
                            let index = filtered_fonts[row_index];
                            let font = &fonts[index];
                            let selected = *font == current_font;
                            let focused = focused_control == Some(SettingsControl::Font(index));
                            let value = font.clone();
                            let row_handle = font_handle.clone();
                            h_flex()
                                .id(("settings-font-option", index))
                                .h_10()
                                .px_3()
                                .justify_between()
                                .cursor_pointer()
                                .rounded(px(4.))
                                .when(selected || focused, |row| {
                                    row.bg(font_colors.element_selected)
                                })
                                .hover(|style| style.bg(font_colors.element_hover))
                                .child(
                                    div()
                                        .font_family(font.clone())
                                        .text_sm()
                                        .child(font.clone()),
                                )
                                .when(selected, |row| {
                                    row.child(
                                        svg()
                                            .path(IconName::Check.path())
                                            .size(px(14.))
                                            .text_color(font_colors.text_accent),
                                    )
                                })
                                .on_click(move |_, _, cx| {
                                    row_handle
                                        .update(cx, |this, cx| {
                                            if let Some(editor) = this.settings_editor.as_mut() {
                                                editor.configuration.terminal_font_family =
                                                    value.clone();
                                                editor.configuration_dirty = true;
                                                editor.font_query = None;
                                                editor.focused_input = None;
                                                editor.message = None;
                                                cx.notify();
                                            }
                                        })
                                        .ok();
                                })
                        })
                        .collect::<Vec<_>>()
                },
            )
            .h_full()
            .track_scroll(&editor.font_scroll);
            let font_scroll = editor.font_scroll.0.borrow().base_handle.clone();
            div()
                .id("font-picker-modal")
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
                        .max_w(px(560.))
                        .h_full()
                        .max_h(px(520.))
                        .p_3()
                        .flex()
                        .flex_col()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .mb_3()
                                .gap_2()
                                .when(close_button_on_left, |header| {
                                    header.child(close_font_picker_button())
                                })
                                .child(div().min_w_0().flex_1().child(text_input(
                                    "settings-font-search".to_owned(),
                                    query.clone(),
                                    SettingsInput::FontSearch,
                                )))
                                .when(!close_button_on_left, |header| {
                                    header.child(close_font_picker_button())
                                }),
                        )
                        .child(div().relative().min_h_0().flex_1().child(font_rows).child(
                            scroll_indicator("settings-font-scrollbar".to_owned(), &font_scroll),
                        )),
                )
                .into_any_element()
        });

        let profile_modal = editor.profile_draft.as_ref().map(|draft| {
            let mut theme_options = vec!["Use application theme".to_owned()];
            theme_options.extend(editor.themes.iter().cloned());
            let profile_theme = dropdown(
                "settings-new-profile-theme".to_owned(),
                draft
                    .theme
                    .clone()
                    .unwrap_or_else(|| "Use application theme".to_owned()),
                theme_options.into(),
                SettingsDropdown::ProfileDraftTheme,
                window,
                cx,
            );
            let close_new_profile_button = || {
                let cancel_handle = handle.clone();
                IconButton::new("close-new-profile", IconName::Close)
                    .icon_size(IconSize::Small)
                    .toggle_state(
                        editor.focused_control == Some(SettingsControl::CloseProfileDialog),
                    )
                    .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                    .on_click(move |_, _, cx| {
                        cancel_handle
                            .update(cx, |this, cx| {
                                if let Some(editor) = this.settings_editor.as_mut() {
                                    editor.profile_draft = None;
                                    editor.focused_input = None;
                                    editor.message = None;
                                    cx.notify();
                                }
                            })
                            .ok();
                    })
            };
            let create_handle = handle.clone();
            div()
                .id("new-profile-modal")
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
                        .id("new-profile-form")
                        .w_full()
                        .max_w(px(640.))
                        .p_6()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .mb_4()
                                .gap_2()
                                .when(close_button_on_left, |header| {
                                    header.child(close_new_profile_button())
                                })
                                .child(div().min_w_0().flex_1().text_lg().child("Add profile"))
                                .when(!close_button_on_left, |header| {
                                    header.child(close_new_profile_button())
                                }),
                        )
                        .child(
                            div()
                                .mb_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Profile name"),
                        )
                        .child(text_input(
                            "settings-new-profile-name".to_owned(),
                            draft.name.clone(),
                            SettingsInput::ProfileDraft(ProfileDraftField::Name),
                        ))
                        .child(
                            div()
                                .mt_3()
                                .mb_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Program"),
                        )
                        .child(text_input(
                            "settings-new-profile-program".to_owned(),
                            draft.program.clone(),
                            SettingsInput::ProfileDraft(ProfileDraftField::Program),
                        ))
                        .child(
                            div()
                                .mt_3()
                                .mb_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Arguments (comma separated)"),
                        )
                        .child(text_input(
                            "settings-new-profile-arguments".to_owned(),
                            draft.arguments.clone(),
                            SettingsInput::ProfileDraft(ProfileDraftField::Arguments),
                        ))
                        .child(
                            div()
                                .mt_3()
                                .mb_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Theme"),
                        )
                        .child(profile_theme)
                        .when_some(editor.message.clone(), |modal, (_, message)| {
                            modal.child(
                                div()
                                    .mt_3()
                                    .text_xs()
                                    .text_color(colors.text)
                                    .child(message),
                            )
                        })
                        .child(
                            h_flex().mt_5().justify_end().child(
                                div()
                                    .id("create-settings-profile")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(
                                        if editor.focused_control
                                            == Some(SettingsControl::CreateProfile)
                                        {
                                            colors.border_focused
                                        } else {
                                            colors.element_selected
                                        },
                                    )
                                    .cursor_pointer()
                                    .bg(colors.element_selected)
                                    .hover(|style| style.bg(colors.element_hover))
                                    .child("Create profile")
                                    .on_click(move |_, _, cx| {
                                        create_handle
                                            .update(cx, |this, cx| {
                                                let Some(editor) = this.settings_editor.as_mut()
                                                else {
                                                    return;
                                                };
                                                let valid = editor
                                                    .profile_draft
                                                    .as_ref()
                                                    .is_some_and(|draft| {
                                                        !draft.name.text.trim().is_empty()
                                                            && !draft.program.text.trim().is_empty()
                                                    });
                                                if !valid {
                                                    editor.message = Some((
                                                        true,
                                                        "Profile name and program are required."
                                                            .to_owned(),
                                                    ));
                                                    cx.notify();
                                                    return;
                                                }
                                                let draft = editor.profile_draft.take().unwrap();
                                                editor.configuration.profiles.push(draft);
                                                editor.configuration_dirty = true;
                                                editor.focused_input = None;
                                                editor.message = None;
                                                cx.notify();
                                            })
                                            .ok();
                                    }),
                            ),
                        ),
                )
                .into_any_element()
        });

        let keymap_capture_modal = editor.keymap_capture.as_ref().map(|capture| {
            let target = capture.target;
            let captured = capture
                .keystroke
                .as_ref()
                .map(|keystroke| keymap_keystroke_display(&keystroke.unparse()))
                .unwrap_or_else(|| "Waiting for a key combination…".to_owned());
            let has_capture = capture.keystroke.is_some();
            let cancel_handle = handle.clone();
            let confirm_handle = handle.clone();
            div()
                .id("keymap-capture-modal")
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
                        .id("keymap-capture-dialog")
                        .w_full()
                        .max_w(px(520.))
                        .p_6()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border_focused)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .child(
                            div()
                                .text_lg()
                                .child("Record keyboard shortcut"),
                        )
                        .child(
                            div()
                                .mt_2()
                                .text_sm()
                                .text_color(colors.text_muted)
                                .child("Press and hold the desired key combination. The shortcut will be shown below before it changes the keymap."),
                        )
                        .child(
                            div()
                                .mt_5()
                                .min_h(px(64.))
                                .px_4()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(colors.border)
                                .bg(colors.editor_background)
                                .text_lg()
                                .child(captured),
                        )
                        .child(
                            div()
                                .mt_3()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child("Return: confirm and put it in the accelerator field · Esc: cancel")
                                .child(
                                    div()
                                        .mt_1()
                                        .child("To bind plain Esc or Return, type escape or enter in the field instead."),
                                ),
                        )
                        .child(
                            h_flex()
                                .mt_5()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("cancel-keymap-capture", "Cancel")
                                        .style(ButtonStyle::Outlined)
                                        .on_click(move |_, window, cx| {
                                            cancel_handle
                                                .update(cx, |this, cx| {
                                                    this.cancel_keymap_capture(target, window, cx)
                                                })
                                                .ok();
                                        }),
                                )
                                .child(
                                    Button::new("confirm-keymap-capture", "Use shortcut")
                                        .style(ButtonStyle::Filled)
                                        .disabled(!has_capture)
                                        .on_click(move |_, window, cx| {
                                            confirm_handle
                                                .update(cx, |this, cx| {
                                                    this.commit_keymap_capture(target, window, cx)
                                                })
                                                .ok();
                                        }),
                                ),
                        ),
                )
                .into_any_element()
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
                        .child(
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
                                )),
                        )
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
                        .when_some(keymap_capture_modal, |dialog, modal| dialog.child(modal)),
                )
                .into_any_element(),
        )
    }
}
