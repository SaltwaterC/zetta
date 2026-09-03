use super::keymap::{KeymapRow, keymap_filtered_indices, keymap_rows};
use super::pane_templates;
use super::projects;
use super::*;

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

/// The rows an open dropdown displays for `query` — every option, or its fuzzy
/// matches, in display order — paired with the row `uniform_list` has to measure.
///
/// `uniform_list` derives the whole list's width from a single measured row, so
/// that row has to be the longest option; measuring the first one instead leaves
/// every longer option wrapping inside a row whose height is pinned to the
/// measured row's single line.
pub(crate) fn dropdown_snapshot_rows(
    options: &[String],
    query: &str,
) -> (Arc<[usize]>, Option<usize>) {
    let rows: Arc<[usize]> = if query.is_empty() {
        (0..options.len()).collect::<Vec<_>>().into()
    } else {
        fuzzy_match_indices(options, query).into()
    };
    let widest_row = rows
        .iter()
        .enumerate()
        .max_by_key(|(_, index)| options[**index].chars().count())
        .map(|(row, _)| row);
    (rows, widest_row)
}

pub(super) fn dropdown_row_for_option(rows: &[usize], option_index: usize) -> Option<usize> {
    rows.iter().position(|index| *index == option_index)
}

pub(super) fn scroll_open_dropdown_to_selection(editor: &mut SettingsEditor) {
    let Some(row) = dropdown_row_for_option(&editor.open_dropdown_rows, editor.dropdown_index)
    else {
        return;
    };
    editor
        .dropdown_scroll
        .scroll_to_item(row, ScrollStrategy::Nearest);
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
    fn settings_controls(editor: &mut SettingsEditor) -> Vec<SettingsControl> {
        // Check cache first
        if let Some(ref cache) = editor.controls_cache {
            return cache.clone();
        }

        let controls = Self::build_settings_controls(editor);
        editor.controls_cache = Some(controls.clone());
        controls
    }

    fn build_settings_controls(editor: &SettingsEditor) -> Vec<SettingsControl> {
        if let Some(query) = editor.font_query.as_ref() {
            let mut controls = vec![SettingsControl::Input(SettingsInput::FontSearch)];
            controls.extend(
                matching_font_indices(&editor.normalized_fonts, &query.text)
                    .iter()
                    .copied()
                    .map(SettingsControl::Font),
            );
            return controls;
        }
        if editor.profile_draft.is_some() {
            return profile_draft_controls().to_vec();
        }

        if editor.keymap_capture.is_some() {
            return Vec::new();
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
                controls.extend([
                    SettingsControl::Dropdown(SettingsDropdown::DefaultProfile),
                    SettingsControl::Dropdown(SettingsDropdown::NewTabProfile),
                    SettingsControl::Dropdown(SettingsDropdown::Theme),
                    SettingsControl::Dropdown(SettingsDropdown::DarkTheme),
                    SettingsControl::DefaultTabIconPicker,
                    SettingsControl::Numeric(NumericSetting::FontSize),
                    SettingsControl::FontPicker,
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::WorkingDirectory,
                    )),
                    SettingsControl::Dropdown(SettingsDropdown::WorkingDirectoryScope),
                    SettingsControl::Numeric(NumericSetting::ScrollHistory),
                    // The background-session block, in the order the page draws
                    // it: between the scrollback setting and the appearance
                    // toggles. It used to be listed after the pane-control
                    // dropdowns, which put it late in the tab order and early on
                    // the page — and since `scroll_settings_control_into_view`
                    // maps tab position onto the scroll range, focusing one of
                    // these fields scrolled it out of view.
                    SettingsControl::Dropdown(SettingsDropdown::SessionRetention),
                    SettingsControl::Numeric(NumericSetting::SessionRingBytes),
                    #[cfg(feature = "session-persistence")]
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::SessionPersistenceRecipients,
                    )),
                    #[cfg(feature = "session-persistence")]
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::SessionPersistenceIdentity,
                    )),
                ]);
                // Only in the tab order while the page is actually drawing it —
                // a control that cannot be seen but can still be tabbed to is a
                // stop where the focus ring lands on nothing.
                #[cfg(feature = "session-persistence")]
                if editor.configuration.session_auto_protect_is_offered() {
                    controls.push(SettingsControl::Toggle(SettingsToggle::SessionAutoProtect));
                }
                controls.extend([
                    SettingsControl::Opacity,
                    SettingsControl::Toggle(SettingsToggle::CompactMode),
                    SettingsControl::Toggle(SettingsToggle::PaneSize),
                    SettingsControl::Toggle(SettingsToggle::TitleBarLabels),
                    SettingsControl::Toggle(SettingsToggle::TitleBarButtons),
                    #[cfg(target_os = "macos")]
                    SettingsControl::Toggle(SettingsToggle::TitleBarMenus),
                    #[cfg(target_os = "macos")]
                    SettingsControl::RequestFocusStatusAccess,
                    SettingsControl::Dropdown(SettingsDropdown::PaneControlsPosition),
                    SettingsControl::Dropdown(SettingsDropdown::PaneControlsDefaultVisibility),
                ]);
                #[cfg(feature = "http-server")]
                controls.push(SettingsControl::Numeric(NumericSetting::HttpServerPort));
                #[cfg(feature = "tftp-server")]
                controls.push(SettingsControl::Numeric(NumericSetting::TftpServerPort));
                for (index, profile) in editor.configuration.profiles.iter().enumerate() {
                    controls.extend(profile_controls(index, profile.detected));
                }
                controls.push(SettingsControl::AddProfile);
            }
            SettingsPage::Themes => {
                controls.extend([
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
                            controls
                                .push(SettingsControl::RemoveBinding(section_index, binding_index));
                        }
                    }
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

pub(crate) fn profile_controls(index: usize, detected: bool) -> Vec<SettingsControl> {
    let mut controls = Vec::with_capacity(if detected { 4 } else { 8 });
    if !detected {
        controls.extend([
            SettingsControl::Input(SettingsInput::Configuration(ConfigTextField::ProfileName(
                index,
            ))),
            SettingsControl::RemoveProfile(index),
            SettingsControl::Input(SettingsInput::Configuration(
                ConfigTextField::ProfileProgram(index),
            )),
            SettingsControl::Input(SettingsInput::Configuration(
                ConfigTextField::ProfileArguments(index),
            )),
        ]);
    }
    controls.extend([
        SettingsControl::Toggle(SettingsToggle::ProfileVisibility(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProfileIcon(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProfileTheme(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProfileDarkTheme(index)),
    ]);
    controls
}

pub(crate) fn project_profile_controls(index: usize) -> [SettingsControl; 8] {
    [
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileName(index))),
        SettingsControl::RemoveProjectProfile(index),
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileProgram(
            index,
        ))),
        SettingsControl::Input(SettingsInput::Project(ProjectTextField::ProfileArguments(
            index,
        ))),
        SettingsControl::Toggle(SettingsToggle::ProjectProfileVisibility(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProjectProfileIcon(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProjectProfileTheme(index)),
        SettingsControl::Dropdown(SettingsDropdown::ProjectProfileDarkTheme(index)),
    ]
}

pub(crate) fn profile_draft_controls() -> [SettingsControl; 9] {
    [
        SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Name)),
        SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Program)),
        SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Arguments)),
        SettingsControl::Toggle(SettingsToggle::ProfileDraftVisibility),
        SettingsControl::Dropdown(SettingsDropdown::ProfileDraftIcon),
        SettingsControl::Dropdown(SettingsDropdown::ProfileDraftTheme),
        SettingsControl::Dropdown(SettingsDropdown::ProfileDraftDarkTheme),
        SettingsControl::Close,
        SettingsControl::CreateProfile,
    ]
}

#[cfg(test)]
#[path = "../tests/settings_ui/controls.rs"]
mod tests;
