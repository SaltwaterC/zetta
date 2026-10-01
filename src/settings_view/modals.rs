use super::*;
use crate::settings_view::widgets::activate_on_click;
use crate::ui_tokens::RADIUS_CONTROL;

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
                        h_flex()
                            .id(("settings-font-option", index))
                            .h_10()
                            .px_3()
                            .justify_between()
                            .cursor_pointer()
                            .rounded(RADIUS_CONTROL)
                            .border_1()
                            // Focus is the ring, selection the fill, so the
                            // keyboard can be followed across the current font.
                            .border_color(if focused {
                                font_colors.border_focused
                            } else {
                                gpui::transparent_black()
                            })
                            .when(selected, |row| row.bg(font_colors.element_selected))
                            .when(!selected, |row| {
                                row.hover(|style| style.bg(font_colors.element_hover))
                            })
                            .child(
                                div()
                                    .font_family(font.clone())
                                    .text_sm()
                                    .child(font.clone()),
                            )
                            .when(selected, |row| {
                                row.child(
                                    Icon::new(IconName::Check)
                                        .size(IconSize::Small)
                                        .color(Color::Custom(font_colors.text_accent)),
                                )
                            })
                            .on_click(activate_on_click(
                                &font_handle,
                                SettingsControl::Font(index),
                            ))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .h_full()
        .track_scroll(&editor.font_scroll);
        let font_scroll = editor.font_scroll.0.borrow().base_handle.clone();
        let panel =
            modal_panel("font-picker", colors)
                .max_w(DIALOG_WIDTH_MEDIUM)
                .h_full()
                .max_h(px(520.))
                .p_3()
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
                            DialogButton::new("close-font-picker", "Close", ButtonRole::Secondary)
                                .key_tooltip("Close font picker", SurfaceKey::Escape)
                                .focused(
                                    editor.focused_control == Some(SettingsControl::CloseModal),
                                )
                                .render(
                                    colors,
                                    activate_on_click(handle, SettingsControl::CloseModal),
                                ),
                        ),
                )
                .child(div().relative().min_h_0().flex_1().child(font_rows).child(
                    scroll_indicator("settings-font-scrollbar".to_owned(), &font_scroll),
                ));
        modal(
            modal_backdrop(
                "font-picker-modal",
                Placement::Centered,
                BackdropClick::Swallow,
            ),
            panel,
        )
    })
}

/// The Add profile modal, or `None` when no draft is open.
pub(crate) fn render_profile_modal(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    status: &theme::StatusColors,
    handle: &WeakEntity<Zetta>,
    scroll_indicator: &impl Fn(String, &ScrollHandle) -> AnyElement,
    text_input: &impl Fn(String, TextField, SettingsInput) -> AnyElement,
    dropdown: &impl Fn(String, String, SettingsDropdown) -> AnyElement,
) -> Option<AnyElement> {
    editor.profile_draft.as_ref().map(|draft| {
        let draft_scroll = editor.profile_draft_scroll.clone();
        let focus_scroll_request = editor.focus_scroll_request.as_ref();
        let profile_scrollbar =
            scroll_indicator("settings-new-profile-scrollbar".to_owned(), &draft_scroll);
        let tracked = |content: AnyElement, controls: &[SettingsControl]| {
            track_focus_scroll_from(div().mt_3(), focus_scroll_request, &draft_scroll, controls)
                .child(content)
                .into_any_element()
        };
        let name_input = SettingsInput::ProfileDraft(ProfileDraftField::Name);
        let program_input = SettingsInput::ProfileDraft(ProfileDraftField::Program);
        let automatic_icon = ProfileIcon::automatic_for_program(&draft.program.text);
        let overrides = super::pages::profile_override_fields(
            super::pages::ProfileOverrides {
                id_prefix: "settings-new-profile".to_owned(),
                visibility: SettingsToggle::ProfileDraftVisibility,
                icon: SettingsDropdown::ProfileDraftIcon,
                theme: SettingsDropdown::ProfileDraftTheme,
                dark_theme: SettingsDropdown::ProfileDraftDarkTheme,
                profile: draft,
                automatic_icon: &automatic_icon,
            },
            colors,
            handle,
            dropdown,
        );
        let draft_fields = [
            tracked(
                super::pages::profile_field(
                    "Profile name",
                    text_input(
                        "settings-new-profile-name".to_owned(),
                        draft.name.clone(),
                        name_input,
                    ),
                    colors,
                ),
                &[SettingsControl::Input(name_input)],
            ),
            tracked(
                super::pages::profile_field(
                    "Program",
                    text_input(
                        "settings-new-profile-program".to_owned(),
                        draft.program.clone(),
                        program_input,
                    ),
                    colors,
                ),
                &[SettingsControl::Input(program_input)],
            ),
            div()
                .mt_3()
                .child(super::pages::profile_field(
                    "Arguments",
                    argument_list(
                        editor,
                        ProfileTarget::Draft,
                        &draft.arguments,
                        "settings-new-profile",
                        &draft_scroll,
                        colors,
                        handle,
                    ),
                    colors,
                ))
                .into_any_element(),
            tracked(
                super::pages::profile_fields_grid(overrides).into_any_element(),
                &[
                    SettingsControl::Toggle(SettingsToggle::ProfileDraftVisibility),
                    SettingsControl::Dropdown(SettingsDropdown::ProfileDraftIcon),
                    SettingsControl::Dropdown(SettingsDropdown::ProfileDraftTheme),
                    SettingsControl::Dropdown(SettingsDropdown::ProfileDraftDarkTheme),
                ],
            ),
        ];
        let can_create =
            Zetta::profile_draft_has_required_fields(&draft.name.text, &draft.program.text);
        // As tall as the form, up to a limit it scrolls within, rather than a
        // fixed height that left a band of nothing above the buttons.
        let panel = dialog_panel("new-profile-form", DIALOG_WIDTH_LARGE, colors)
            .max_h(px(520.))
            .child(dialog_title("Add profile", colors))
            .child(
                // Sized by its content and shrunk only once the panel reaches
                // its limit, so the form scrolls only when it has to.
                div()
                    .relative()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("new-profile-body")
                            .min_h_0()
                            .pr(px(SETTINGS_SCROLLBAR_WIDTH + 2.))
                            .overflow_y_scroll()
                            .track_scroll(&draft_scroll)
                            .children(draft_fields)
                            .when_some(editor.message.clone(), |body, (tone, message)| {
                                body.child(div().mt_3().child(crate::ui_messages::status_message(
                                    tone, message, colors, status,
                                )))
                            }),
                    )
                    .child(profile_scrollbar),
            )
            .child(
                dialog_buttons()
                    .flex_none()
                    .child(
                        DialogButton::new(
                            "close-settings-profile",
                            "Cancel",
                            ButtonRole::Secondary,
                        )
                        .key_tooltip("Close add profile", SurfaceKey::Escape)
                        .focused(editor.focused_control == Some(SettingsControl::CloseModal))
                        .render(
                            colors,
                            activate_on_click(handle, SettingsControl::CloseModal),
                        ),
                    )
                    .child(
                        DialogButton::new(
                            "create-settings-profile",
                            "Create profile",
                            ButtonRole::Primary,
                        )
                        // Disabled until the draft has what a profile needs, as
                        // this used to claim it was without being so.
                        .enabled(can_create)
                        .key_tooltip("Create profile", SurfaceKey::Enter)
                        .focused(editor.focused_control == Some(SettingsControl::CreateProfile))
                        .render(
                            colors,
                            activate_on_click(handle, SettingsControl::CreateProfile),
                        ),
                    ),
            );
        modal(
            modal_backdrop(
                "new-profile-modal",
                Placement::Centered,
                BackdropClick::Swallow,
            ),
            panel,
        )
    })
}

pub(crate) fn render_keymap_capture_modal(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> Option<AnyElement> {
    editor.keymap_capture.as_ref().map(|capture| {
        let target = capture.target;
        let captured = capture.keystroke.as_ref().map_or_else(
            || "Waiting for a key combination…".to_owned(),
            |keystroke| keymap_keystroke_display(&keystroke.unparse()),
        );
        let has_capture = capture.keystroke.is_some();
        let cancel_handle = handle.clone();
        let confirm_handle = handle.clone();
        // Neither button is in the tab order: while recording, Tab is a key to
        // record, not a way to move. Return and Esc are the keyboard's way out,
        // and the hint says so.
        let panel = dialog_panel("keymap-capture-dialog", DIALOG_WIDTH_MEDIUM, colors)
            .child(dialog_title("Record keyboard shortcut", colors))
            .child(div().text_sm().text_color(colors.text_muted).child(
                "Press and hold the key combination. It is shown below before it changes \
                     the keymap.",
            ))
            .child(
                div()
                    .min_h(px(64.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(RADIUS_CONTROL)
                    .border_1()
                    .border_color(colors.border_focused)
                    .bg(colors.editor_background)
                    .text_lg()
                    .child(captured),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(hint_line(
                        key_hints(&[("Return", "use it"), ("Esc", "cancel")]),
                        colors,
                    ))
                    .child(hint_line(
                        "To bind plain Esc or Return, type escape or enter in the field instead.",
                        colors,
                    )),
            )
            .child(
                dialog_buttons()
                    .child(
                        DialogButton::new("cancel-keymap-capture", "Cancel", ButtonRole::Secondary)
                            .key_tooltip("Cancel recording", SurfaceKey::Escape)
                            .render(colors, move |_, window, cx| {
                                cancel_handle
                                    .update(cx, |this, cx| {
                                        this.cancel_keymap_capture(target, window, cx);
                                    })
                                    .ok();
                            }),
                    )
                    .child(
                        DialogButton::new(
                            "confirm-keymap-capture",
                            "Use shortcut",
                            ButtonRole::Primary,
                        )
                        .enabled(has_capture)
                        .key_tooltip("Use the recorded shortcut", SurfaceKey::Enter)
                        .render(colors, move |_, window, cx| {
                            confirm_handle
                                .update(cx, |this, cx| {
                                    this.commit_keymap_capture(target, window, cx);
                                })
                                .ok();
                        }),
                    ),
            );
        modal(
            modal_backdrop(
                "keymap-capture-modal",
                Placement::Centered,
                BackdropClick::Swallow,
            ),
            panel,
        )
    })
}

/// The unsaved-changes confirmation, or `None` when no way out is waiting on
/// it. See `settings_ui::close_guard`.
pub(crate) fn render_close_request_modal(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> Option<AnyElement> {
    let request = editor.close_request?;
    let leaving_builder = request == crate::settings_ui::CloseRequest::ProjectBuilder;
    let focused = |control: SettingsControl| editor.focused_control.as_ref() == Some(&control);
    let can_save = request.can_save_first(editor);
    let panel = dialog_panel("settings-close-request", DIALOG_WIDTH_SMALL, colors)
        .child(dialog_title("Discard unsaved changes?", colors))
        .child(div().text_sm().text_color(colors.text_muted).child(if leaving_builder {
            "This project's configuration has changes that have not been saved."
        } else if can_save {
            "Your settings have changes that have not been saved."
        } else {
            "The open project's configuration has changes that have not been saved; save them \
             with Save before closing, or discard them."
        }))
        .child(
            dialog_buttons()
                .child(
                    DialogButton::new("settings-keep-editing", "Keep editing", ButtonRole::Secondary)
                        .key_tooltip("Keep editing", SurfaceKey::Escape)
                        .focused(focused(SettingsControl::KeepEditing))
                        .render(colors, activate_on_click(handle, SettingsControl::KeepEditing)),
                )
                .child(
                    DialogButton::new(
                        "settings-discard-changes",
                        "Discard changes",
                        ButtonRole::Destructive,
                    )
                    .focused(focused(SettingsControl::DiscardChanges))
                    .render(colors, activate_on_click(handle, SettingsControl::DiscardChanges)),
                )
                .when(can_save, |buttons| {
                    buttons.child(
                        DialogButton::new(
                            "settings-save-before-closing",
                            "Save",
                            ButtonRole::Primary,
                        )
                        .focused(focused(SettingsControl::SaveBeforeClosing))
                        .render(
                            colors,
                            activate_on_click(handle, SettingsControl::SaveBeforeClosing),
                        ),
                    )
                }),
        );
    Some(modal(
        modal_backdrop(
            "settings-close-request-modal",
            Placement::Centered,
            BackdropClick::Swallow,
        ),
        panel,
    ))
}

#[cfg(test)]
#[path = "../tests/settings_view/modals.rs"]
mod tests;
