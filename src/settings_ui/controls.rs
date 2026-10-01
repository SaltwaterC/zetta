use super::keymap::{KeymapRow, keymap_filtered_indices, keymap_rows};
use super::pane_templates;
use super::projects;
use super::*;

/// Disarms a destructive control that was pressed once, unless focus is staying
/// on it: the confirming press is a second press of the same button.
pub(crate) fn disarm_unless(editor: &mut SettingsEditor, control: &SettingsControl) {
    if editor.armed_control.as_ref() != Some(control) {
        editor.armed_control = None;
    }
}

/// Whether a destructive control should act now. The first press arms it and
/// says what the second one will do; the second press, with nothing else
/// focused in between, confirms it.
pub(crate) fn confirm_destructive(editor: &mut SettingsEditor, control: &SettingsControl) -> bool {
    if editor.armed_control.as_ref() == Some(control) {
        editor.armed_control = None;
        return true;
    }
    editor.armed_control = Some(control.clone());
    editor.message = Some((
        Tone::Warning,
        "Press again to confirm; this cannot be undone.".to_owned(),
    ));
    false
}

pub(crate) fn adjacent_settings_control_index(
    len: usize,
    current: Option<usize>,
    reverse: bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let current = current.unwrap_or_else(|| if reverse { 0 } else { len - 1 });
    Some(if reverse {
        current.checked_sub(1).unwrap_or(len - 1)
    } else {
        (current + 1) % len
    })
}

pub(super) fn scroll_open_dropdown_to_selection(editor: &mut SettingsEditor) {
    editor.dropdown.scroll_to_selection();
}

/// How many controls at the front of the tab order live in the dialog's fixed
/// header — the page tabs, Close, and Save.
///
/// Scrolling maps a control's position within the *form* to a scroll offset, so
/// counting the header controls as form controls skews every page's mapping.
/// Deriving the count keeps that mapping right when a page tab is added.
pub(crate) fn leading_header_controls(controls: &[SettingsControl]) -> usize {
    controls
        .iter()
        .position(|control| {
            !matches!(
                control,
                SettingsControl::Tab(_) | SettingsControl::Close | SettingsControl::Save
            )
        })
        .unwrap_or(controls.len())
}

pub(crate) fn invalidate_controls_cache(editor: &mut SettingsEditor) {
    editor.controls_cache = None;
    editor.controls_generation = editor.controls_generation.wrapping_add(1);
}

impl Zetta {
    pub(crate) fn settings_controls(editor: &mut SettingsEditor) -> Vec<SettingsControl> {
        // Check cache first
        if let Some(ref cache) = editor.controls_cache {
            return cache.clone();
        }

        let controls = Self::build_settings_controls(editor);
        editor.controls_cache = Some(controls.clone());
        controls
    }

    pub(crate) fn build_settings_controls(editor: &SettingsEditor) -> Vec<SettingsControl> {
        if let Some(query) = editor.font_query.as_ref() {
            let mut controls = vec![
                SettingsControl::Input(SettingsInput::FontSearch),
                SettingsControl::CloseModal,
            ];
            controls.extend(
                matching_font_indices(&editor.normalized_fonts, &query.text)
                    .iter()
                    .copied()
                    .map(SettingsControl::Font),
            );
            return controls;
        }
        if editor.profile_draft.is_some() {
            return profile_draft_controls(
                editor
                    .profile_draft
                    .as_ref()
                    .map_or(0, |draft| draft.arguments.len()),
            );
        }

        if editor.keymap_capture.is_some() {
            return Vec::new();
        }
        if editor.close_request.is_some() {
            return super::close_guard::close_request_controls(editor);
        }

        let mut controls = vec![
            SettingsControl::Tab(SettingsPage::Configuration),
            SettingsControl::Tab(SettingsPage::Themes),
            SettingsControl::Tab(SettingsPage::Keymap),
            SettingsControl::Tab(SettingsPage::PaneTemplates),
            SettingsControl::Tab(SettingsPage::Projects),
            SettingsControl::Close,
            SettingsControl::Save,
        ];
        match editor.page {
            SettingsPage::Configuration => {
                controls.extend(super::configuration_page::configuration_controls(editor));
            }
            SettingsPage::Themes => {
                controls.extend([
                    SettingsControl::OpenThemeStore,
                    SettingsControl::Input(SettingsInput::ThemeSearch),
                    SettingsControl::SearchThemes,
                ]);
                if editor.theme_extension_downloading.is_none() {
                    controls.extend(
                        editor
                            .installed_theme_extensions
                            .iter()
                            .map(|extension| SettingsControl::RemoveTheme(extension.id.clone())),
                    );
                    controls.extend(
                        editor
                            .theme_extensions
                            .iter()
                            .filter(|extension| {
                                !editor
                                    .installed_theme_extensions
                                    .iter()
                                    .any(|installed| installed.id == extension.id.as_ref())
                            })
                            .map(|extension| SettingsControl::InstallTheme(extension.id.clone())),
                    );
                }
            }
            SettingsPage::Keymap => {
                controls.push(SettingsControl::Input(SettingsInput::KeymapSearch));
                let (filtered_sections, filtered_bindings) = keymap_filtered_indices(editor);
                for section_index in filtered_sections {
                    let Some(section) = editor.keymap.sections.get(section_index) else {
                        continue;
                    };
                    controls.push(SettingsControl::Input(SettingsInput::Keymap(
                        KeymapTextField::Context(section_index),
                    )));
                    if let Some(binding_indices) = filtered_bindings.get(&section_index) {
                        for &binding_index in binding_indices {
                            let Some(binding) = section.bindings.get(binding_index) else {
                                continue;
                            };
                            controls.extend([
                                SettingsControl::Input(SettingsInput::Keymap(
                                    KeymapTextField::Keystroke(section_index, binding_index),
                                )),
                                SettingsControl::CaptureKeymap(KeymapTextField::Keystroke(
                                    section_index,
                                    binding_index,
                                )),
                                SettingsControl::Dropdown(SettingsDropdown::BindingAction(
                                    section_index,
                                    binding_index,
                                )),
                            ]);
                            if binding.action_parameter("name").is_some() {
                                controls.push(SettingsControl::Dropdown(
                                    SettingsDropdown::BindingTemplate(section_index, binding_index),
                                ));
                            }
                            if binding.action_usize_parameter("slot").is_some() {
                                controls.push(SettingsControl::Dropdown(
                                    SettingsDropdown::BindingProfile(section_index, binding_index),
                                ));
                            }
                            controls.push(keymap::binding_removal_control(
                                editor,
                                section_index,
                                binding_index,
                            ));
                        }
                    }
                    // Drawn after the bindings and before the Add button, so
                    // tabbed there too (see `keymap_rows_from_matches`).
                    controls
                        .extend((0..section.unbound_defaults.len()).map(|unbound| {
                            SettingsControl::RestoreBinding(section_index, unbound)
                        }));
                    controls.push(SettingsControl::AddBinding(section_index));
                }
                controls.push(SettingsControl::AddKeymapSection);
            }
            SettingsPage::PaneTemplates => {
                controls.extend(pane_templates::pane_template_controls(editor));
            }
            SettingsPage::Projects => controls.extend(projects::project_controls(editor)),
        }
        controls
    }

    pub(crate) fn scroll_settings_control_into_view(&mut self, control: &SettingsControl) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if let Some(query) = editor.font_query.as_ref() {
            if let SettingsControl::Font(index) = control
                && let Some(row_index) =
                    matching_font_position(&editor.normalized_fonts, &query.text, *index)
            {
                editor
                    .font_scroll
                    .scroll_to_item(row_index, ScrollStrategy::Nearest);
            }
            return;
        }
        if editor.page == SettingsPage::Keymap {
            let row = match control {
                SettingsControl::Input(SettingsInput::Keymap(KeymapTextField::Context(
                    section,
                ))) => Some(KeymapRow::SectionHeader(*section)),
                SettingsControl::Input(SettingsInput::Keymap(KeymapTextField::Keystroke(
                    section,
                    binding,
                )))
                | SettingsControl::CaptureKeymap(KeymapTextField::Keystroke(section, binding))
                | SettingsControl::Dropdown(SettingsDropdown::BindingAction(section, binding))
                | SettingsControl::Dropdown(SettingsDropdown::BindingTemplate(section, binding))
                | SettingsControl::Dropdown(SettingsDropdown::BindingProfile(section, binding))
                | SettingsControl::RemoveBinding(section, binding)
                | SettingsControl::UnbindBinding(section, binding) => {
                    Some(KeymapRow::Binding(*section, *binding))
                }
                SettingsControl::RestoreBinding(section, unbound) => {
                    Some(KeymapRow::UnboundDefault(*section, *unbound))
                }
                SettingsControl::AddBinding(section) => Some(KeymapRow::AddBinding(*section)),
                SettingsControl::AddKeymapSection => Some(KeymapRow::AddSection),
                _ => None,
            };
            if let Some(row) = row {
                let rows = keymap_rows(editor);
                if let Some(row_index) = rows.iter().position(|candidate| *candidate == row) {
                    editor
                        .keymap_scroll
                        .scroll_to_item(row_index, ScrollStrategy::Nearest);
                }
            }
            return;
        }
        if editor.profile_draft.is_some()
            && matches!(
                control,
                SettingsControl::Close | SettingsControl::CreateProfile
            )
        {
            editor.focus_scroll_request = None;
            return;
        }
        let controls = Self::settings_controls(editor);
        let Some(index) = controls.iter().position(|candidate| candidate == control) else {
            return;
        };
        let form_start = leading_header_controls(&controls);
        if index < form_start {
            return;
        }
        let form_index = index - form_start;
        let form_count = controls.len().saturating_sub(form_start);
        // A control's position in the tab order only approximates where its row
        // ends up, so this gets close and the row itself corrects the rest once
        // it has been laid out (`widgets::track_focus_scroll`).
        let progress = form_index as f32 / form_count.saturating_sub(1).max(1) as f32;
        let scroll = if editor.profile_draft.is_some() {
            &editor.profile_draft_scroll
        } else {
            &editor.settings_scroll
        };
        let maximum = scroll.max_offset().y;
        let offset = scroll.offset();
        scroll.set_offset(point(offset.x, -(maximum * progress)));
        editor.focus_scroll_request = Some((control.clone(), scroll.offset().y));
    }

    pub(crate) fn focus_settings_control(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_settings_control_with_scroll(control, window, cx, true);
    }

    pub(crate) fn focus_settings_control_without_scroll(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_settings_control_with_scroll(control, window, cx, false);
    }

    fn focus_settings_control_with_scroll(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
        scroll: bool,
    ) {
        if let SettingsControl::Input(input) = control {
            self.focus_settings_input(input, window, cx);
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            disarm_unless(editor, &control);
            check_setting_being_left(editor, &control);
            editor.focused_input = None;
            editor.focused_control = Some(control.clone());
            if !scroll {
                // A click already put the control under the pointer; scrolling
                // to it now would move it out from under the click.
                editor.focus_scroll_request = None;
            }
        }
        if scroll {
            self.scroll_settings_control_into_view(&control);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn focus_adjacent_settings_control(
        &mut self,
        reverse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let controls = Self::settings_controls(editor);
        let current = editor.focused_control.as_ref();
        let current =
            current.and_then(|current| controls.iter().position(|control| control == current));
        if let Some(control) = adjacent_settings_control_index(controls.len(), current, reverse)
            .and_then(|index| controls.get(index))
            .cloned()
        {
            self.focus_settings_control(control, window, cx);
        }
    }
}

/// The controls of a configured profile's card, in the order it draws them.
/// A detected profile's program and arguments come from what is installed, so
/// only its four overrides are controls.
pub(crate) fn profile_controls(
    index: usize,
    detected: bool,
    arguments: usize,
) -> Vec<SettingsControl> {
    let target = ProfileTarget::Configuration(index);
    let mut controls = Vec::new();
    if !detected {
        controls.extend([
            SettingsControl::Input(SettingsInput::Configuration(ConfigTextField::ProfileName(
                index,
            ))),
            SettingsControl::RemoveProfile(index),
            SettingsControl::Input(SettingsInput::Configuration(
                ConfigTextField::ProfileProgram(index),
            )),
        ]);
        controls.extend(argument_controls(target, arguments));
    }
    controls.extend([
        SettingsControl::Toggle(SettingsToggle::ProfileVisibility(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProfileIcon(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProfileTheme(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProfileDarkTheme(index)),
    ]);
    controls
}

/// A profile's argument list: each field and the button that removes it, then
/// the button that adds one.
pub(crate) fn argument_controls(target: ProfileTarget, arguments: usize) -> Vec<SettingsControl> {
    let mut controls = Vec::with_capacity(arguments * 2 + 1);
    for argument in 0..arguments {
        controls.push(SettingsControl::Input(target.argument_input(argument)));
        controls.push(SettingsControl::RemoveProfileArgument(target, argument));
    }
    controls.push(SettingsControl::AddProfileArgument(target));
    controls
}

pub(crate) fn project_profile_controls(index: usize, arguments: usize) -> Vec<SettingsControl> {
    let mut controls = vec![
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileName(index))),
        SettingsControl::RemoveProjectProfile(index),
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileProgram(
            index,
        ))),
    ];
    controls.extend(argument_controls(ProfileTarget::Project(index), arguments));
    controls.extend([
        SettingsControl::Toggle(SettingsToggle::ProjectProfileVisibility(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProjectProfileIcon(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProjectProfileTheme(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProjectProfileDarkTheme(index)),
    ]);
    controls
}

pub(crate) fn profile_draft_controls(arguments: usize) -> Vec<SettingsControl> {
    let mut controls = vec![
        SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Name)),
        SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Program)),
    ];
    controls.extend(argument_controls(ProfileTarget::Draft, arguments));
    controls.extend([
        SettingsControl::Toggle(SettingsToggle::ProfileDraftVisibility),
        SettingsControl::Dropdown(SettingsDropdown::ProfileDraftIcon),
        SettingsControl::Dropdown(SettingsDropdown::ProfileDraftTheme),
        SettingsControl::Dropdown(SettingsDropdown::ProfileDraftDarkTheme),
        SettingsControl::CloseModal,
        SettingsControl::CreateProfile,
    ]);
    controls
}

#[cfg(test)]
#[path = "../tests/settings_ui/controls.rs"]
pub(crate) mod tests;
