use super::*;

pub(crate) fn render_font_modal(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    scroll_indicator: &impl Fn(String, &ScrollHandle) -> AnyElement,
    text_input: &impl Fn(String, TextField, SettingsInput) -> AnyElement,
) -> Option<AnyElement> {
    editor.font_query.as_ref().map(|query| {
        let current_font = editor.configuration.terminal_font_family.clone();
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
        let close_handle = handle.clone();
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
                                            editor.clear_dropdown();
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
                    .text_color(colors.text)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .mb_3()
                            .gap_2()
                            .child(div().min_w_0().flex_1().child(text_input(
                                "settings-font-search".to_owned(),
                                query.clone(),
                                SettingsInput::FontSearch,
                            )))
                            .child(
                                div()
                                    .flex_none()
                                    .id("close-font-picker")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(colors.element_selected)
                                    .cursor_pointer()
                                    .bg(colors.element_selected)
                                    .text_color(colors.text)
                                    .hover(|style| style.bg(colors.element_hover))
                                    .tooltip(Tooltip::text("Close font picker (Esc)"))
                                    .on_click(move |_, _, cx| {
                                        close_handle
                                            .update(cx, |this, cx| {
                                                if let Some(editor) = this.settings_editor.as_mut()
                                                {
                                                    editor.clear_dropdown();
                                                    editor.font_query = None;
                                                    editor.focused_input = None;
                                                    editor.focused_control = None;
                                                    editor.message = None;
                                                    cx.notify();
                                                }
                                            })
                                            .ok();
                                    })
                                    .child("Close"),
                            ),
                    )
                    .child(div().relative().min_h_0().flex_1().child(font_rows).child(
                        scroll_indicator("settings-font-scrollbar".to_owned(), &font_scroll),
                    )),
            )
            .into_any_element()
    })
}

/// The Add profile modal, or `None` when no draft is open.
pub(crate) fn render_profile_modal(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    scroll_indicator: &impl Fn(String, &ScrollHandle) -> AnyElement,
    text_input: &impl Fn(String, TextField, SettingsInput) -> AnyElement,
    dropdown: &impl Fn(String, String, SettingsDropdown) -> AnyElement,
) -> Option<AnyElement> {
    editor.profile_draft.as_ref().map(|draft| {
        let [
            name_control,
            program_control,
            arguments_control,
            visibility_control,
            icon_control,
            theme_control,
            dark_theme_control,
            ..,
        ] = profile_draft_controls();
        let draft_scroll = editor.profile_draft_scroll.clone();
        let focus_scroll_request = editor.focus_scroll_request.as_ref();
        let profile_scrollbar =
            scroll_indicator("settings-new-profile-scrollbar".to_owned(), &draft_scroll);
        let draft_field =
            |label: &'static str, control: SettingsControl, content: AnyElement, spaced: bool| {
                let row = div()
                    .when(spaced, |row| row.mt_3())
                    .child(
                        div()
                            .mb_1()
                            .text_xs()
                            .text_color(colors.text_muted)
                            .child(label),
                    )
                    .child(content);
                track_focus_scroll_from(
                    row,
                    focus_scroll_request,
                    &draft_scroll,
                    std::slice::from_ref(&control),
                )
                .into_any_element()
            };
        let profile_theme = dropdown(
            "settings-new-profile-theme".to_owned(),
            draft
                .theme
                .clone()
                .unwrap_or_else(|| "Use application theme".to_owned()),
            SettingsDropdown::ProfileDraftTheme,
        );
        let profile_dark_theme = dropdown(
            "settings-new-profile-dark-theme".to_owned(),
            draft
                .dark_theme
                .clone()
                .unwrap_or_else(|| "Use application theme".to_owned()),
            SettingsDropdown::ProfileDraftDarkTheme,
        );
        let automatic_icon = ProfileIcon::automatic_for_program(&draft.program.text);
        let profile_icon_value = draft.icon.as_ref().unwrap_or(&automatic_icon);
        let profile_icon = h_flex()
            .w_full()
            .gap_2()
            .child(profile_icon_value.render(IconSize::Small))
            .child(div().min_w_0().flex_1().child(dropdown(
                "settings-new-profile-icon".to_owned(),
                ProfileIcon::selector_label(draft.icon.as_ref()).to_owned(),
                SettingsDropdown::ProfileDraftIcon,
            )));
        let visibility_handle = handle.clone();
        let profile_visibility = switch("settings-new-profile-visibility", (!draft.hidden).into())
            .label(if draft.hidden { "Hidden" } else { "Visible" })
            .full_width(true)
            .aria_label("Show profile in Profiles menu")
            .on_click(move |state, window, cx| {
                visibility_handle
                    .update(cx, |this, cx| {
                        this.set_settings_toggle(
                            SettingsToggle::ProfileDraftVisibility,
                            state.selected(),
                            window,
                            cx,
                        );
                    })
                    .ok();
            });
        let draft_fields = [
            draft_field(
                "Profile name",
                name_control,
                text_input(
                    "settings-new-profile-name".to_owned(),
                    draft.name.clone(),
                    SettingsInput::ProfileDraft(ProfileDraftField::Name),
                ),
                false,
            ),
            draft_field(
                "Program",
                program_control,
                text_input(
                    "settings-new-profile-program".to_owned(),
                    draft.program.clone(),
                    SettingsInput::ProfileDraft(ProfileDraftField::Program),
                ),
                true,
            ),
            draft_field(
                "Arguments (comma separated)",
                arguments_control,
                text_input(
                    "settings-new-profile-arguments".to_owned(),
                    draft.arguments.clone(),
                    SettingsInput::ProfileDraft(ProfileDraftField::Arguments),
                ),
                true,
            ),
            draft_field(
                "Shown in Profiles menu",
                visibility_control,
                profile_visibility.into_any_element(),
                true,
            ),
            draft_field("Icon", icon_control, profile_icon.into_any_element(), true),
            draft_field("Light theme", theme_control, profile_theme, true),
            draft_field("Dark theme", dark_theme_control, profile_dark_theme, true),
        ];
        let (close_button, create_button) = profile_modal_buttons(editor, colors, handle);
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
                    .h_full()
                    .max_h(px(520.))
                    .p_6()
                    .flex()
                    .flex_col()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.elevated_surface_background)
                    .text_color(colors.text)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .mb_4()
                            .flex_none()
                            .child(div().min_w_0().flex_1().text_lg().child("Add profile")),
                    )
                    .child(
                        div()
                            .relative()
                            .min_h_0()
                            .flex_1()
                            .child(
                                div()
                                    .id("new-profile-body")
                                    .size_full()
                                    .pr(px(SETTINGS_SCROLLBAR_WIDTH + 2.))
                                    .overflow_y_scroll()
                                    .track_scroll(&draft_scroll)
                                    .children(draft_fields)
                                    .when_some(editor.message.clone(), |body, (_, message)| {
                                        body.child(
                                            div()
                                                .mt_3()
                                                .text_xs()
                                                .text_color(colors.text)
                                                .child(message),
                                        )
                                    }),
                            )
                            .child(profile_scrollbar),
                    )
                    .child(h_flex().mt_5().flex_none().gap_2().justify_end().children([
                        close_button.into_any_element(),
                        create_button.into_any_element(),
                    ])),
            )
            .into_any_element()
    })
}

/// The modal's Close and Create buttons.
///
/// Both are plain divs rather than `Button`s because they carry the form's own
/// focus ring, and Create is disabled until the draft has a program.
fn profile_modal_buttons(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> (AnyElement, AnyElement) {
    let [.., close_control, create_control] = profile_draft_controls();
    let close_handle = handle.clone();
    let create_handle = handle.clone();
    let close_focused = editor.focused_control == Some(close_control.clone());
    let create_focused = editor.focused_control == Some(create_control.clone());
    let close_click_control = close_control.clone();
    let create_click_control = create_control.clone();
    let close_button = div()
        .id("close-settings-profile")
        .flex_none()
        .px_4()
        .py_2()
        .rounded(px(4.))
        .border_1()
        .border_color(if close_focused {
            colors.border_focused
        } else {
            colors.element_selected
        })
        .cursor_pointer()
        .bg(colors.element_selected)
        .text_color(colors.text)
        .hover(|style| style.bg(colors.element_hover))
        .tooltip(Tooltip::text("Close add profile (Esc)"))
        .on_click(move |_, window, cx| {
            close_handle
                .update(cx, |this, cx| {
                    this.focus_settings_control_without_scroll(
                        close_click_control.clone(),
                        window,
                        cx,
                    );
                    this.activate_settings_control(close_click_control.clone(), window, cx);
                })
                .ok();
        })
        .child("Close");
    let create_button = div()
        .id("create-settings-profile")
        .px_4()
        .py_2()
        .rounded(px(4.))
        .border_1()
        .border_color(if create_focused {
            colors.border_focused
        } else {
            colors.element_selected
        })
        .cursor_pointer()
        .bg(colors.element_selected)
        .text_color(colors.text)
        .hover(|style| style.bg(colors.element_hover))
        .tooltip(Tooltip::text("Create profile (Enter)"))
        .child("Create profile")
        .on_click(move |_, window, cx| {
            create_handle
                .update(cx, |this, cx| {
                    this.focus_settings_control_without_scroll(
                        create_click_control.clone(),
                        window,
                        cx,
                    );
                    this.activate_settings_control(create_click_control.clone(), window, cx);
                })
                .ok();
        });
    (
        close_button.into_any_element(),
        create_button.into_any_element(),
    )
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
                .as_ref().map_or_else(|| "Waiting for a key combination…".to_owned(), |keystroke| keymap_keystroke_display(&keystroke.unparse()));
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
                        .text_color(colors.text)
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
                                        .color(Color::Custom(colors.text))
                                        .on_click(move |_, window, cx| {
                                            cancel_handle
                                                .update(cx, |this, cx| {
                                                    this.cancel_keymap_capture(target, window, cx);
                                                })
                                                .ok();
                                        }),
                                )
                                .child(
                                    Button::new("confirm-keymap-capture", "Use shortcut")
                                        .style(ButtonStyle::Filled)
                                        .color(Color::Custom(colors.text))
                                        .disabled(!has_capture)
                                        .on_click(move |_, window, cx| {
                                            confirm_handle
                                                .update(cx, |this, cx| {
                                                    this.commit_keymap_capture(target, window, cx);
                                                })
                                                .ok();
                                        }),
                                ),
                        ),
                )
                .into_any_element()
    })
}

#[cfg(test)]
#[path = "../tests/settings_view/modals.rs"]
mod tests;
