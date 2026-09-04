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
    dropdown_field, text_field, track_focus_scroll, track_focus_scroll_from,
};

#[cfg(test)]
#[path = "tests/settings_view.rs"]
mod tests;

/// The dialog's page tabs, in the order they are shown.
///
/// `pub(crate)` so `settings_ui::controls`' sidecar can check it against the
/// keyboard tab order, which is built from a separate list.
pub(crate) const SETTINGS_PAGE_TABS: [(SettingsPage, &str, &str); 5] = [
    (
        SettingsPage::Configuration,
        "settings-configuration-tab",
        "Configuration",
    ),
    (SettingsPage::Themes, "settings-themes-tab", "Themes"),
    (SettingsPage::Keymap, "settings-keymap-tab", "Keymap"),
    (
        SettingsPage::PaneTemplates,
        "settings-pane-templates-tab",
        "Templates",
    ),
    (SettingsPage::Projects, "settings-projects-tab", "Projects"),
];

/// One page tab. The five differ only in the page they select, their element id
/// and their label, so they are built from [`SETTINGS_PAGE_TABS`] rather than
/// spelled out five times.
///
/// A tab highlights both while its page is open and while it holds keyboard
/// focus, so the tab row shows focus the way the form rows do.
fn settings_page_tab(
    page: SettingsPage,
    id: &'static str,
    label: &'static str,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> gpui::Stateful<gpui::Div> {
    let handle = handle.clone();
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded(px(4.))
        .cursor_pointer()
        .when(
            editor.page == page || editor.focused_control == Some(SettingsControl::Tab(page)),
            |tab| tab.bg(colors.element_selected),
        )
        .on_click(move |_, window, cx| {
            handle
                .update(cx, |this, cx| this.select_settings_page(page, window, cx))
                .ok();
        })
        .child(label)
}

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
                           control_id: SettingsControl,
                           control: gpui::AnyElement| {
            widgets.setting_row(label, description, control_id, control)
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

    /// The dialog header's Close and Save buttons.
    ///
    /// `save_in_progress` disables Save rather than hiding it, so the header
    /// keeps its width while a write is in flight; `unsaved_changes` is what
    /// puts the asterisk on it.
    fn render_settings_header_actions(
        &self,
        editor: &SettingsEditor,
        colors: &ThemeColors,
        handle: &WeakEntity<Self>,
        save_in_progress: bool,
        unsaved_changes: bool,
    ) -> gpui::Div {
        let close_handle = handle.clone();
        let save_handle = handle.clone();
        h_flex()
            .gap_2()
            .child(
                div()
                    .id("close-settings")
                    .px_3()
                    .py_1()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if editor.focused_control == Some(SettingsControl::Close) {
                        colors.border_focused
                    } else {
                        colors.element_selected
                    })
                    .cursor_pointer()
                    .bg(colors.element_selected)
                    .text_color(colors.text)
                    .hover(|style| style.bg(colors.element_hover))
                    .tooltip(Tooltip::text("Close settings (Esc)"))
                    .on_click(move |_, window, cx| {
                        close_handle
                            .update(cx, |this, cx| this.dismiss_settings(window, cx))
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
                    .border_color(if editor.focused_control == Some(SettingsControl::Save) {
                        colors.border_focused
                    } else {
                        colors.element_selected
                    })
                    .bg(colors.element_selected)
                    .text_color(colors.text)
                    .when(!save_in_progress, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(colors.element_hover))
                            .tooltip(Tooltip::for_action_title_in(
                                "Save settings",
                                &SaveSettings,
                                &self.settings_focus,
                            ))
                            .on_click(move |_, window, cx| {
                                save_handle
                                    .update(cx, |this, cx| this.save_settings(window, cx))
                                    .ok();
                            })
                    })
                    .when(save_in_progress, |button| button.opacity(0.65))
                    .child(if save_in_progress {
                        "Saving…"
                    } else if unsaved_changes {
                        "Save *"
                    } else {
                        "Save"
                    }),
            )
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

        let profile_modal = modals::render_profile_modal(
            editor,
            &colors,
            &handle,
            &scroll_indicator,
            &text_input,
            &dropdown,
        );

        let keymap_capture_modal = modals::render_keymap_capture_modal(editor, &colors, &handle);

        // Rendered once, as a sibling of the dialog content, regardless of which page or
        // row opened it (see `DropdownRenderState` for why it can't render inline).
        let dropdown_popup = editor.open_dropdown.map(|selection| {
            Self::dropdown_popup_widget(selection, colors.clone(), handle.clone(), dropdown_state)
        });

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
                Some(project) => crate::project::ProjectConfig::path_for(&project.config_root)
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
                        .text_color(colors.text)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .h_12()
                                .px_3()
                                .flex_none()
                                .justify_between()
                                .border_b_1()
                                .border_color(colors.border)
                                .child(SETTINGS_PAGE_TABS.iter().fold(
                                    h_flex().gap_1(),
                                    |row, (page, id, label)| {
                                        row.child(settings_page_tab(
                                            *page, id, label, editor, &colors, &handle,
                                        ))
                                    },
                                ))
                                .child(self.render_settings_header_actions(
                                    editor,
                                    &colors,
                                    &handle,
                                    settings_save_in_progress,
                                    unsaved_changes,
                                )),
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
