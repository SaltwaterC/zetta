use super::*;
use crate::settings_ui::{invalidate_controls_cache, refresh_keymap_cache};

use crate::startup::keymap_keystroke_display;

mod form_widgets;
mod modals;
mod pages;
mod pane_templates;
mod projects;
mod widgets;

pub(crate) use form_widgets::SettingsFormWidgets;
pub(crate) use widgets::{
    DropdownRenderState, KEYMAP_ROW_HEIGHT, SETTINGS_SCROLLBAR_WIDTH, action_button, control_row,
    dropdown_field, text_field, track_focus_scroll,
};

impl Zetta {
    /// The settings page and the scroll region it lives in.
    ///
    /// Rendered inside its own cached view (see `view_boundary`) so scrolling a
    /// modal or a dropdown popup layered over the dialog reuses the page
    /// instead of rebuilding every row of it.
    pub(crate) fn render_settings_page_region(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let editor = self.settings_editor.as_ref()?;
        let colors = self.window_theme(cx).colors().clone();
        let handle = cx.entity().downgrade();
        let widgets = SettingsFormWidgets::new(editor, colors.clone(), handle.clone());
        let scroll_indicator =
            |id: String, scroll: &ScrollHandle| widgets.scroll_indicator(id, scroll);
        let text_input = |id: String, field: TextField, input: SettingsInput| {
            widgets.text_input(id, field, input)
        };
        let dropdown = |id: String, label: String, selection: SettingsDropdown| {
            widgets.dropdown(id, label, selection)
        };
        let setting_row = |label: &'static str,
                           description: &'static str,
                           focused: bool,
                           control: gpui::AnyElement| {
            widgets.setting_row(label, description, focused, control)
        };
        let setting_toggle = |id: &'static str, value: bool, toggle: SettingsToggle| {
            widgets.setting_toggle(id, value, toggle)
        };
        let numeric =
            |id: &'static str,
             field: TextField,
             setting: NumericSetting,
             input: ConfigTextField| widgets.numeric(id, field, setting, input);
        let opacity_slider =
            |opacity: f32, target: OpacityTarget| widgets.opacity_slider(opacity, target);
        let focus_status_access = if cx.has_global::<ZettaProcessState>() {
            cx.global::<ZettaProcessState>()
                .silent_mode
                .focus_status_access()
        } else {
            FocusStatusAccess::Unknown
        };
        let content = pages::render_settings_pages(
            editor,
            &colors,
            &handle,
            &cx.entity(),
            focus_status_access,
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
        let region = if editor.page == SettingsPage::Keymap {
            div()
                .size_full()
                .relative()
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
                .size_full()
                .relative()
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

        Some(region)
    }

    pub(crate) fn render_settings_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let editor = self.settings_editor.as_ref()?;
        let colors = self.window_theme(cx).colors().clone();
        let handle = cx.entity().downgrade();
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

        let widgets = SettingsFormWidgets::new(editor, colors.clone(), handle.clone());
        let scroll_indicator =
            |id: String, scroll: &ScrollHandle| widgets.scroll_indicator(id, scroll);
        let text_input = |id: String, field: TextField, input: SettingsInput| {
            widgets.text_input(id, field, input)
        };
        let dropdown = |id: String, label: String, selection: SettingsDropdown| {
            widgets.dropdown(id, label, selection)
        };

        let profile_icon_automatic = match editor.open_dropdown {
            Some(SettingsDropdown::ProfileIcon(index)) => editor
                .configuration
                .profiles
                .get(index)
                .map(|profile| profile.automatic_icon.clone()),
            Some(SettingsDropdown::ProfileDraftIcon) => editor
                .profile_draft
                .as_ref()
                .map(|profile| ProfileIcon::automatic_for_program(&profile.program.text)),
            _ => None,
        };
        let dropdown_state = DropdownRenderState {
            dropdown_index: editor.dropdown_index,
            dropdown_query: editor.dropdown_query.clone(),
            options: editor.open_dropdown_options.clone(),
            rows: editor.open_dropdown_rows.clone(),
            widest_row: editor.open_dropdown_widest_row,
            dropdown_scroll: editor.dropdown_scroll.clone(),
            dropdown_anchor: editor.dropdown_anchor,
            profile_icon_automatic,
        };

        let page_region = self.settings_page_region_element(cx);

        let editor = self.settings_editor.as_ref()?;
        let font_modal =
            modals::render_font_modal(editor, &colors, &handle, &scroll_indicator, &text_input);

        let profile_modal =
            modals::render_profile_modal(editor, &colors, &handle, &text_input, &dropdown);

        let keymap_capture_modal = modals::render_keymap_capture_modal(editor, &colors, &handle);

        // Rendered once, as a sibling of the dialog content, regardless of which page or
        // row opened it (see `DropdownRenderState` for why it can't render inline).
        let dropdown_popup = editor.open_dropdown.map(|selection| {
            Self::dropdown_popup_widget(selection, colors.clone(), handle.clone(), dropdown_state)
        });

        let config_handle = handle.clone();
        let themes_handle = handle.clone();
        let keymap_handle = handle.clone();
        let templates_handle = handle.clone();
        let projects_handle = handle.clone();
        let close_handle = handle.clone();
        let save_handle = handle.clone();
        // The header Save button is scoped to whatever the visible page edits,
        // which is the open project's file rather than the user configuration
        // while the projects builder is up.
        let project = crate::settings_ui::project_editor(editor);
        let settings_save_in_progress = editor.settings_save_in_progress
            || project.is_some_and(|project| project.save_in_progress);
        let unsaved_changes = match project {
            Some(project) => project.dirty,
            None => editor.configuration_dirty || editor.keymap_dirty,
        };
        let path = match editor.page {
            SettingsPage::Configuration => self.launch_config.config_path.display().to_string(),
            SettingsPage::Themes => format!(
                "Zed theme extensions · installed in {}",
                config::themes_dir().display()
            ),
            SettingsPage::Keymap => self.launch_config.keymap_path.display().to_string(),
            SettingsPage::PaneTemplates => {
                "pane_split_templates · built-ins are read-only presets".to_owned()
            }
            SettingsPage::Projects => match project {
                Some(project) => crate::project::ProjectConfig::path_for(&project.root)
                    .display()
                    .to_string(),
                None => self.projects.registry.path().display().to_string(),
            },
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
                        .key_context("Settings")
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
                                        )
                                        .child(
                                            div()
                                                .id("settings-pane-templates-tab")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .when(
                                                    editor.page == SettingsPage::PaneTemplates
                                                        || editor.focused_control
                                                            == Some(SettingsControl::Tab(
                                                                SettingsPage::PaneTemplates,
                                                            )),
                                                    |tab| tab.bg(colors.element_selected),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    templates_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_settings_page(
                                                                SettingsPage::PaneTemplates,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                                .child("Templates"),
                                        )
                                        .child(
                                            div()
                                                .id("settings-projects-tab")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .cursor_pointer()
                                                .when(
                                                    editor.page == SettingsPage::Projects
                                                        || editor.focused_control
                                                            == Some(SettingsControl::Tab(
                                                                SettingsPage::Projects,
                                                            )),
                                                    |tab| tab.bg(colors.element_selected),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    projects_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_settings_page(
                                                                SettingsPage::Projects,
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                                .child("Projects"),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("close-settings")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .border_1()
                                                .border_color(
                                                    if editor.focused_control
                                                        == Some(SettingsControl::Close)
                                                    {
                                                        colors.border_focused
                                                    } else {
                                                        colors.element_selected
                                                    },
                                                )
                                                .cursor_pointer()
                                                .bg(colors.element_selected)
                                                .hover(|style| style.bg(colors.element_hover))
                                                .tooltip(Tooltip::text("Close settings (Esc)"))
                                                .on_click(move |_, window, cx| {
                                                    close_handle
                                                        .update(cx, |this, cx| {
                                                            this.dismiss_settings(window, cx)
                                                        })
                                                        .ok();
                                                })
                                                .child("Close"),
                                        )
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
                                                .bg(colors.element_selected)
                                                .when(!settings_save_in_progress, |button| {
                                                    button
                                                        .cursor_pointer()
                                                        .hover(|style| {
                                                            style.bg(colors.element_hover)
                                                        })
                                                        .tooltip(Tooltip::for_action_title_in(
                                                            "Save settings",
                                                            &SaveSettings,
                                                            &self.settings_focus,
                                                        ))
                                                        .on_click(move |_, window, cx| {
                                                            save_handle
                                                                .update(cx, |this, cx| {
                                                                    this.save_settings(window, cx)
                                                                })
                                                                .ok();
                                                        })
                                                })
                                                .when(settings_save_in_progress, |button| {
                                                    button.opacity(0.65)
                                                })
                                                .child(if settings_save_in_progress {
                                                    "Saving…"
                                                } else if unsaved_changes {
                                                    "Save *"
                                                } else {
                                                    "Save"
                                                }),
                                        ),
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
                        .child(page_region)
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
