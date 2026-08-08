use super::*;

pub(crate) fn render_font_modal(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    close_button_on_left: bool,
    scroll_indicator: &impl Fn(String, &ScrollHandle) -> AnyElement,
    text_input: &impl Fn(String, TextField, SettingsInput) -> AnyElement,
) -> Option<AnyElement> {
    editor.font_query.as_ref().map(|query| {
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
        // Use cached filtered font indices or compute inline if cache is missing
        let filtered_fonts = if editor.font_search_query_cache == query.text {
            editor
                .font_filtered_indices
                .clone()
                .unwrap_or_else(|| matching_font_indices(&editor.normalized_fonts, &query.text))
        } else {
            matching_font_indices(&editor.normalized_fonts, &query.text)
        };
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
    })
}

pub(crate) fn render_profile_modal(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    close_button_on_left: bool,
    text_input: &impl Fn(String, TextField, SettingsInput) -> AnyElement,
    dropdown: &impl Fn(String, String, SettingsDropdown) -> AnyElement,
) -> Option<AnyElement> {
    editor.profile_draft.as_ref().map(|draft| {
        let profile_theme = dropdown(
            "settings-new-profile-theme".to_owned(),
            draft
                .theme
                .clone()
                .unwrap_or_else(|| "Use application theme".to_owned()),
            SettingsDropdown::ProfileDraftTheme,
        );
        let close_new_profile_button = || {
            let cancel_handle = handle.clone();
            IconButton::new("close-new-profile", IconName::Close)
                .icon_size(IconSize::Small)
                .toggle_state(editor.focused_control == Some(SettingsControl::CloseProfileDialog))
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
                                            let Some(editor) = this.settings_editor.as_mut() else {
                                                return;
                                            };
                                            let valid = editor.profile_draft.as_ref().is_some_and(
                                                |draft| {
                                                    !draft.name.text.trim().is_empty()
                                                        && !draft.program.text.trim().is_empty()
                                                },
                                            );
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
    })
}

pub(crate) fn render_keymap_capture_modal(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> Option<AnyElement> {
    editor.keymap_capture.as_ref().map(|capture| {
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
    })
}
