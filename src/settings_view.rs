use super::*;

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
            let centered = matches!(
                input,
                SettingsInput::Configuration(
                    ConfigTextField::FontSize
                        | ConfigTextField::ScrollHistory
                        | ConfigTextField::HttpServerPort
                )
            );
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
            let selected = label.clone();
            let is_binding_action = matches!(selection, SettingsDropdown::BindingAction(_, _));
            let trigger = ButtonLike::new(id.clone())
                .style(ButtonStyle::Outlined)
                .full_width()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(Label::new(label))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
                );
            PopoverMenu::new(format!("{id}-popover"))
                .full_width(true)
                .trigger(trigger)
                .anchor(Anchor::TopLeft)
                .menu(move |window, cx| {
                    let options = options.clone();
                    let selected = selected.clone();
                    let menu_handle = menu_handle.clone();
                    Some(ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                        for option in options.iter() {
                            let value = option.clone();
                            let option_label = option.clone();
                            let toggled = option_label == selected;
                            let handle = menu_handle.clone();
                            if is_binding_action {
                                let rendered_label = option_label.clone();
                                menu = menu.custom_entry(
                                    move |_, _| {
                                        h_flex()
                                            .gap_2()
                                            .whitespace_nowrap()
                                            .child(
                                                div()
                                                    .w(px(16.))
                                                    .flex_none()
                                                    .text_center()
                                                    .child(if toggled { "✓" } else { "" }),
                                            )
                                            .child(Label::new(rendered_label.clone()))
                                            .into_any_element()
                                    },
                                    move |_, cx| {
                                        handle
                                            .update(cx, |this, cx| {
                                                this.set_settings_dropdown(
                                                    selection,
                                                    value.clone(),
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    },
                                );
                            } else {
                                menu = menu.toggleable_entry(
                                    option_label,
                                    toggled,
                                    IconPosition::Start,
                                    None,
                                    move |_, cx| {
                                        handle
                                            .update(cx, |this, cx| {
                                                this.set_settings_dropdown(
                                                    selection,
                                                    value.clone(),
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    },
                                );
                            }
                        }
                        menu
                    }))
                })
                .into_any_element()
        };

        let setting_row =
            |label: &'static str, description: &'static str, control: gpui::AnyElement| {
                h_flex()
                    .w_full()
                    .min_h(px(54.))
                    .py_2()
                    .gap_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border_variant)
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

        let numeric = |id: &'static str,
                       field: TextField,
                       setting: NumericSetting,
                       input: ConfigTextField|
         -> gpui::AnyElement {
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
                .border_color(colors.border)
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
                let mut profile_names = editor.profile_names.clone();
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
                let theme = dropdown(
                    "settings-theme".to_owned(),
                    configuration.theme.clone(),
                    editor.themes.clone(),
                    SettingsDropdown::Theme,
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
                    .border_color(colors.border)
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
                        default_profile,
                    ),
                    setting_row("Theme", "Application color theme", theme),
                    setting_row(
                        "Terminal font size",
                        "Point size from 6 through 100",
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
                        font_family,
                    ),
                    setting_row(
                        "Working directory",
                        "Initial directory; ~ expands to your home directory",
                        text_input(
                            "settings-working-directory".to_owned(),
                            configuration.working_directory.clone(),
                            SettingsInput::Configuration(ConfigTextField::WorkingDirectory),
                        ),
                    ),
                    setting_row(
                        "Scrollback history",
                        "Enter 0 through Max; steppers accelerate across the range",
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
                        opacity_slider(configuration.inactive_pane_opacity),
                    ),
                    setting_row(
                        "Pane controls position",
                        "Keep pane overlay controls on the right or move them to the left",
                        pane_controls_position,
                    ),
                    setting_row(
                        "HTTP server port",
                        "TCP port used when starting the static HTTP server",
                        numeric(
                            "settings-http-server-port",
                            configuration.http_server_port.clone(),
                            NumericSetting::HttpServerPort,
                            ConfigTextField::HttpServerPort,
                        ),
                    ),
                ];
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
                        h_flex()
                            .p_3()
                            .mb_2()
                            .gap_4()
                            .justify_between()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.editor_background)
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
                            .child(div().w(px(330.)).flex_none().child(profile_theme))
                            .into_any_element()
                    } else {
                        let remove_handle = handle.clone();
                        div()
                            .p_3()
                            .mb_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.editor_background)
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
                        .border_color(colors.border)
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
                                        .border_color(colors.border)
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
                        rows.push(
                            div()
                                .mb_2()
                                .p_3()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(colors.border)
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
                                                .border_color(colors.border)
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
                            .border_color(colors.border)
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
                                            .border_color(colors.border)
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
                for (section_index, section) in editor.keymap.sections.iter().enumerate() {
                    let mut bindings = Vec::new();
                    for (binding_index, binding) in section.bindings.iter().enumerate() {
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
                        let remove_handle = handle.clone();
                        bindings.push(
                            h_flex()
                                .mb_2()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(220.))
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
                                        )),
                                )
                                .child(div().min_w_0().flex_1().child(action))
                                .when_some(template, |row, template| {
                                    row.child(div().w(px(180.)).flex_none().child(template))
                                })
                                .child(
                                    IconButton::new(
                                        format!("remove-settings-binding-{section_index}-{binding_index}"),
                                        IconName::Trash,
                                    )
                                    .icon_size(IconSize::Small)
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
                            .border_color(colors.border)
                            .bg(colors.editor_background)
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
                                    .border_color(colors.border)
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
                        .border_color(colors.border)
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
            let font_rows = uniform_list(
                "settings-font-list",
                filtered_fonts.len(),
                move |range, _, _| {
                    range
                        .map(|row_index| {
                            let index = filtered_fonts[row_index];
                            let font = &fonts[index];
                            let selected = *font == current_font;
                            let value = font.clone();
                            let row_handle = font_handle.clone();
                            h_flex()
                                .id(("settings-font-option", index))
                                .h_10()
                                .px_3()
                                .justify_between()
                                .cursor_pointer()
                                .rounded(px(4.))
                                .when(selected, |row| row.bg(font_colors.element_selected))
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

        let config_handle = handle.clone();
        let themes_handle = handle.clone();
        let keymap_handle = handle.clone();
        let save_handle = handle.clone();
        let close_settings_button = || {
            let close_handle = handle.clone();
            IconButton::new("close-settings", IconName::Close)
                .icon_size(IconSize::Small)
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
                                                    editor.page == SettingsPage::Configuration,
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
                                                .when(editor.page == SettingsPage::Themes, |tab| {
                                                    tab.bg(colors.element_selected)
                                                })
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
                                                .when(editor.page == SettingsPage::Keymap, |tab| {
                                                    tab.bg(colors.element_selected)
                                                })
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
                        .when_some(profile_modal, |dialog, modal| dialog.child(modal)),
                )
                .into_any_element(),
        )
    }
}
