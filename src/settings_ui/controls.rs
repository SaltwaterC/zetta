use super::keymap::{KeymapRow, keymap_filtered_indices, keymap_rows, refresh_keymap_cache};
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

fn dropdown_row_for_option(rows: &[usize], option_index: usize) -> Option<usize> {
    rows.iter().position(|index| *index == option_index)
}

fn scroll_open_dropdown_to_selection(editor: &mut SettingsEditor) {
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

    pub(crate) fn settings_dropdown_options(
        editor: &SettingsEditor,
        dropdown: SettingsDropdown,
    ) -> (String, Arc<[String]>) {
        match dropdown {
            SettingsDropdown::DefaultProfile => {
                let mut options = editor.profile_names.to_vec();
                options.extend(
                    editor
                        .configuration
                        .profiles
                        .iter()
                        .map(|profile| profile.name.text.clone()),
                );
                options.sort();
                options.dedup();
                (editor.configuration.default_profile.clone(), options.into())
            }
            SettingsDropdown::NewTabProfile => (
                editor.configuration.new_tab_profile.label().to_owned(),
                Arc::from([String::from("Default"), String::from("Inherit")]),
            ),
            SettingsDropdown::Theme => (editor.configuration.theme.clone(), editor.themes.clone()),
            SettingsDropdown::DarkTheme => (
                editor.configuration.dark_theme.clone(),
                editor.themes.clone(),
            ),
            SettingsDropdown::WorkingDirectoryScope => (
                editor
                    .configuration
                    .working_directory_scope
                    .label()
                    .to_owned(),
                Arc::from([
                    String::from("None"),
                    String::from("Pane"),
                    String::from("Tab"),
                ]),
            ),
            SettingsDropdown::PaneControlsPosition => (
                editor
                    .configuration
                    .pane_controls_position
                    .label()
                    .to_owned(),
                Arc::from([String::from("Right"), String::from("Left")]),
            ),
            SettingsDropdown::PaneControlsDefaultVisibility => (
                if editor.configuration.pane_controls_hidden_by_default {
                    "Hidden".to_owned()
                } else {
                    "Visible".to_owned()
                },
                Arc::from([String::from("Visible"), String::from("Hidden")]),
            ),
            SettingsDropdown::SessionRetention => {
                #[cfg(feature = "session-persistence")]
                let options = Arc::from([
                    String::from("None"),
                    String::from("Memory"),
                    String::from("Disk"),
                ]);
                #[cfg(not(feature = "session-persistence"))]
                let options = Arc::from([String::from("None"), String::from("Memory")]);
                (
                    editor.configuration.session_retention.label().to_owned(),
                    options,
                )
            }
            SettingsDropdown::ProfileTheme(index) => (
                editor
                    .configuration
                    .profiles
                    .get(index)
                    .and_then(|profile| profile.theme.clone())
                    .unwrap_or_else(|| "Use application theme".to_owned()),
                std::iter::once("Use application theme".to_owned())
                    .chain(editor.themes.iter().cloned())
                    .collect(),
            ),
            SettingsDropdown::ProfileIcon(index) => {
                let profile = editor.configuration.profiles.get(index);
                (
                    profile
                        .and_then(|profile| profile.icon.as_ref())
                        .map(ProfileIcon::label)
                        .unwrap_or("Automatic")
                        .to_owned(),
                    Arc::from(["Automatic", "Zetta", "Bash", "Zsh", "Fish"].map(str::to_owned)),
                )
            }
            SettingsDropdown::ProfileDarkTheme(index) => (
                editor
                    .configuration
                    .profiles
                    .get(index)
                    .and_then(|profile| profile.dark_theme.clone())
                    .unwrap_or_else(|| "Use application theme".to_owned()),
                std::iter::once("Use application theme".to_owned())
                    .chain(editor.themes.iter().cloned())
                    .collect(),
            ),
            SettingsDropdown::ProfileDraftTheme => (
                editor
                    .profile_draft
                    .as_ref()
                    .and_then(|profile| profile.theme.clone())
                    .unwrap_or_else(|| "Use application theme".to_owned()),
                std::iter::once("Use application theme".to_owned())
                    .chain(editor.themes.iter().cloned())
                    .collect(),
            ),
            SettingsDropdown::ProfileDraftDarkTheme => (
                editor
                    .profile_draft
                    .as_ref()
                    .and_then(|profile| profile.dark_theme.clone())
                    .unwrap_or_else(|| "Use application theme".to_owned()),
                std::iter::once("Use application theme".to_owned())
                    .chain(editor.themes.iter().cloned())
                    .collect(),
            ),
            SettingsDropdown::ProfileDraftIcon => {
                let icon = editor
                    .profile_draft
                    .as_ref()
                    .and_then(|profile| profile.icon.as_ref())
                    .map(ProfileIcon::label)
                    .unwrap_or("Automatic")
                    .to_owned();
                (
                    icon,
                    Arc::from(["Automatic", "Zetta", "Bash", "Zsh", "Fish"].map(str::to_owned)),
                )
            }
            SettingsDropdown::BindingAction(section, binding) => (
                editor
                    .keymap
                    .sections
                    .get(section)
                    .and_then(|section| section.bindings.get(binding))
                    .map(BindingForm::action_name)
                    .unwrap_or_default(),
                editor.actions.clone(),
            ),
            SettingsDropdown::BindingTemplate(section, binding) => (
                editor
                    .keymap
                    .sections
                    .get(section)
                    .and_then(|section| section.bindings.get(binding))
                    .and_then(|binding| binding.action_parameter("name"))
                    .unwrap_or_default(),
                editor.pane_template_names.clone(),
            ),
            SettingsDropdown::BindingProfile(section, binding) => {
                let slot = editor
                    .keymap
                    .sections
                    .get(section)
                    .and_then(|section| section.bindings.get(binding))
                    .and_then(|binding| binding.action_usize_parameter("slot"))
                    .unwrap_or(1);
                (
                    editor
                        .profile_names
                        .get(slot.saturating_sub(1))
                        .cloned()
                        .unwrap_or_default(),
                    editor.profile_names.clone(),
                )
            }
            SettingsDropdown::PaneTemplateAxis(_)
            | SettingsDropdown::PaneTemplateSource(_)
            | SettingsDropdown::PaneTemplateTheme(_)
            | SettingsDropdown::PaneTemplateDarkTheme(_)
            | SettingsDropdown::PaneTemplateOverlaySize(_) => {
                pane_templates::pane_template_dropdown_options(editor, dropdown)
            }
            SettingsDropdown::ProjectTheme
            | SettingsDropdown::ProjectDarkTheme
            | SettingsDropdown::ProjectDefaultProfile
            | SettingsDropdown::ProjectInitialSplit
            | SettingsDropdown::ProjectProfileTheme(_)
            | SettingsDropdown::ProjectProfileDarkTheme(_)
            | SettingsDropdown::ProjectProfileIcon(_) => {
                projects::project_dropdown_options(editor, dropdown)
            }
        }
    }

    /// Re-snapshots the open dropdown (if any) after its underlying options
    /// changed while it was open, for example when installing or removing a
    /// theme extension rebuilds the theme list.
    pub(crate) fn refresh_open_dropdown_options(editor: &mut SettingsEditor) {
        let Some(dropdown) = editor.open_dropdown else {
            return;
        };
        let (_, options) = Self::settings_dropdown_options(editor, dropdown);
        Self::refresh_open_dropdown_snapshot(editor, options);
    }

    /// Refreshes the open dropdown's render snapshot for `options` and the
    /// current query. Called when a dropdown opens and whenever its query
    /// changes, so rendering the popover is `Arc` clones rather than a rebuild
    /// of the option list on every frame.
    pub(crate) fn refresh_open_dropdown_snapshot(
        editor: &mut SettingsEditor,
        options: Arc<[String]>,
    ) {
        let (rows, widest_row) = dropdown_snapshot_rows(&options, &editor.dropdown_query);
        editor.open_dropdown_options = options;
        editor.open_dropdown_rows = rows;
        editor.open_dropdown_widest_row = widest_row;
    }

    pub(crate) fn open_settings_dropdown(
        &mut self,
        dropdown: SettingsDropdown,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let (selected, options) = Self::settings_dropdown_options(editor, dropdown);
        if options.is_empty() {
            return;
        }
        editor.dropdown_index = options
            .iter()
            .position(|option| option == &selected)
            .unwrap_or(0);
        editor.dropdown_query.clear();
        Self::refresh_open_dropdown_snapshot(editor, options);
        scroll_open_dropdown_to_selection(editor);
        editor.dropdown_anchor = anchor;
        editor.open_dropdown = Some(dropdown);
        cx.notify();
    }

    pub(crate) fn move_open_settings_dropdown(
        &mut self,
        direction: i32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.settings_editor.as_mut() else {
            return false;
        };
        if editor.open_dropdown.is_none() {
            return false;
        }
        let matching_indices = editor.open_dropdown_rows.clone();
        if matching_indices.is_empty() {
            return false;
        }
        let current = matching_indices
            .iter()
            .position(|index| *index == editor.dropdown_index)
            .unwrap_or(0);
        let next = if direction < 0 {
            current.checked_sub(1).unwrap_or(matching_indices.len() - 1)
        } else {
            (current + 1) % matching_indices.len()
        };
        editor.dropdown_index = matching_indices[next];
        scroll_open_dropdown_to_selection(editor);
        cx.notify();
        true
    }

    pub(crate) fn type_into_open_settings_dropdown(
        &mut self,
        event: &KeyDownEvent,
        command: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.settings_editor.as_mut() else {
            return false;
        };
        let Some(dropdown) = editor.open_dropdown else {
            return false;
        };

        let changed = if event.keystroke.key == "backspace" {
            editor.dropdown_query.pop().is_some()
        } else if !command
            && !event.keystroke.modifiers.alt
            && let Some(text) = event.keystroke.key_char.as_ref()
            && !text.chars().any(char::is_control)
        {
            editor.dropdown_query.push_str(text);
            true
        } else {
            false
        };
        if !changed {
            return false;
        }

        let (_, options) = Self::settings_dropdown_options(editor, dropdown);
        let query = editor.dropdown_query.clone();
        if let Some(index) = fuzzy_match_index(&options, &query) {
            editor.dropdown_index = index;
        }
        Self::refresh_open_dropdown_snapshot(editor, options);
        scroll_open_dropdown_to_selection(editor);
        cx.notify();
        true
    }

    pub(crate) fn commit_open_settings_dropdown(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((dropdown, value)) = self.settings_editor.as_mut().and_then(|editor| {
            let dropdown = editor.open_dropdown.take()?;
            if !editor.dropdown_query.is_empty() && editor.open_dropdown_rows.is_empty() {
                editor.open_dropdown = Some(dropdown);
                return None;
            }
            editor
                .open_dropdown_options
                .get(editor.dropdown_index)
                .cloned()
                .map(|value| (dropdown, value))
        }) else {
            return false;
        };
        self.set_settings_dropdown(dropdown, value, cx);
        true
    }

    pub(crate) fn activate_settings_control(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .settings_editor
            .as_ref()
            .is_some_and(settings_save_in_flight)
        {
            return;
        }
        match control {
            SettingsControl::Tab(page) => self.select_settings_page(page, window, cx),
            SettingsControl::Close => {
                if self
                    .settings_editor
                    .as_ref()
                    .is_some_and(|editor| editor.profile_draft.is_some())
                {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.dismiss_profile_draft();
                        editor.focused_input = None;
                        editor.focused_control = None;
                        editor.focus_scroll_request = None;
                        editor.message = None;
                        invalidate_controls_cache(editor);
                        cx.notify();
                    }
                } else {
                    self.dismiss_settings(window, cx);
                }
            }
            SettingsControl::Save => self.save_settings(window, cx),
            SettingsControl::Input(input) => self.focus_settings_input(input, window, cx),
            SettingsControl::CaptureKeymap(target) => self.start_keymap_capture(target, window, cx),
            SettingsControl::Dropdown(dropdown) => {
                self.open_settings_dropdown(dropdown, window.mouse_position(), cx)
            }
            SettingsControl::Toggle(toggle) => {
                let value = self.settings_editor.as_ref().map(|editor| match toggle {
                    SettingsToggle::CompactMode => editor.configuration.compact_mode,
                    SettingsToggle::PaneSize => editor.configuration.hide_pane_size,
                    SettingsToggle::TitleBarLabels => editor.configuration.hide_title_bar_labels,
                    SettingsToggle::TitleBarButtons => editor.configuration.hide_title_bar_buttons,
                    #[cfg(feature = "session-persistence")]
                    SettingsToggle::SessionAutoProtect => {
                        editor.configuration.session_persistence_auto_protect
                    }
                    SettingsToggle::ProfileVisibility(index) => editor
                        .configuration
                        .profiles
                        .get(index)
                        .is_some_and(|profile| !profile.hidden),
                    SettingsToggle::ProfileDraftVisibility => editor
                        .profile_draft
                        .as_ref()
                        .is_some_and(|profile| !profile.hidden),
                    #[cfg(target_os = "macos")]
                    SettingsToggle::TitleBarMenus => editor.configuration.hide_title_bar_menus,
                    SettingsToggle::ProjectOpacityOverride => editor
                        .project
                        .as_ref()
                        .is_some_and(|project| project.form.inactive_pane_opacity.is_some()),
                    SettingsToggle::ProjectProfileVisibility(index) => editor
                        .project
                        .as_ref()
                        .and_then(|project| project.form.profiles.get(index))
                        .is_some_and(|profile| !profile.hidden),
                });
                if let Some(value) = value {
                    self.set_settings_toggle(toggle, !value, window, cx);
                }
            }
            #[cfg(target_os = "macos")]
            SettingsControl::RequestFocusStatusAccess => {
                window.dispatch_action(Box::new(RequestFocusStatusAccess), cx);
            }
            SettingsControl::FontPicker => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.font_query = Some(TextField::default());
                    editor.scroll_geometry_initialized = false;
                    Self::rebuild_font_search_cache(editor);
                }
                self.focus_settings_input(SettingsInput::FontSearch, window, cx);
            }
            SettingsControl::DefaultTabIconPicker => {
                self.open_default_tab_icon_picker(window, cx);
            }
            SettingsControl::Numeric(_) | SettingsControl::Opacity => {}
            SettingsControl::AddProfile => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.profile_draft_scroll = ScrollHandle::new();
                    editor.profile_draft = Some(settings_editor::ProfileForm {
                        name: TextField::default(),
                        program: TextField::default(),
                        arguments: TextField::default(),
                        theme: None,
                        dark_theme: None,
                        icon: None,
                        automatic_icon: ProfileIcon::Zetta,
                        hidden: false,
                        detected: false,
                    });
                    editor.message = None;
                    invalidate_controls_cache(editor);
                }
                self.focus_settings_input(
                    SettingsInput::ProfileDraft(ProfileDraftField::Name),
                    window,
                    cx,
                );
            }
            SettingsControl::RemoveProfile(index) => {
                if let Some(editor) = self.settings_editor.as_mut()
                    && index < editor.configuration.profiles.len()
                {
                    editor.configuration.profiles.remove(index);
                    editor.configuration_dirty = true;
                    editor.focused_control = None;
                    invalidate_controls_cache(editor);
                    cx.notify();
                }
            }
            SettingsControl::SearchThemes => self.fetch_theme_extensions(window, cx),
            SettingsControl::InstallTheme(id) => self.download_theme_extension(id, window, cx),
            SettingsControl::RemoveTheme(id) => self.remove_theme_extension(id, window, cx),
            SettingsControl::RemoveBinding(section, binding) => {
                if let Some(editor) = self.settings_editor.as_mut()
                    && let Some(section) = editor.keymap.sections.get_mut(section)
                    && binding < section.bindings.len()
                {
                    section.bindings.remove(binding);
                    editor.keymap_dirty = true;
                    editor.focused_control = None;
                    cx.notify();
                }
            }
            SettingsControl::UnbindBinding(section, binding) => {
                if let Some(editor) = self.settings_editor.as_mut()
                    && let Some(section) = editor.keymap.sections.get_mut(section)
                    && binding < section.bindings.len()
                {
                    let binding = section.bindings.remove(binding);
                    // Add to unbind map
                    section.unbind.insert(
                        keymap_keystroke_storage(&binding.keystroke.text),
                        binding.action_name(),
                    );
                    editor.keymap_dirty = true;
                    editor.focused_control = None;
                    cx.notify();
                }
            }
            SettingsControl::AddBinding(section_index) => {
                if let Some(editor) = self.settings_editor.as_mut()
                    && let Some(section) = editor.keymap.sections.get_mut(section_index)
                {
                    section.bindings.push(BindingForm {
                        keystroke: TextField::new("ctrl-shift-x"),
                        action: serde_json::Value::String("zetta::NewTab".to_owned()),
                    });
                    editor.keymap_dirty = true;
                    cx.notify();
                }
            }
            SettingsControl::AddKeymapSection => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor
                        .keymap
                        .sections
                        .push(KeymapSectionForm::new("Zetta > Terminal"));
                    editor.keymap_dirty = true;
                    cx.notify();
                }
            }
            SettingsControl::Font(index) => {
                if let Some(editor) = self.settings_editor.as_mut()
                    && let Some(font) = editor.fonts.get(index)
                {
                    editor.configuration.terminal_font_family = font.clone();
                    editor.configuration_dirty = true;
                    editor.clear_dropdown();
                    editor.font_query = None;
                    editor.focused_input = None;
                    editor.focused_control = None;
                    editor.message = None;
                    invalidate_controls_cache(editor);
                    cx.notify();
                }
            }
            SettingsControl::CreateProfile => {
                let valid = self.settings_editor.as_ref().is_some_and(|editor| {
                    editor.profile_draft.as_ref().is_some_and(|draft| {
                        Self::profile_draft_has_required_fields(
                            &draft.name.text,
                            &draft.program.text,
                        )
                    })
                });
                if !valid {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.message =
                            Some((true, "Profile name and program are required.".to_owned()));
                    }
                    cx.notify();
                    return;
                }
                if let Some(editor) = self.settings_editor.as_mut() {
                    let mut draft = editor.profile_draft.take().unwrap();
                    draft.automatic_icon = ProfileIcon::automatic_for_program(&draft.program.text);
                    editor.configuration.profiles.push(draft);
                    editor.configuration_dirty = true;
                    editor.clear_dropdown();
                    editor.focused_input = None;
                    editor.focused_control = None;
                    editor.focus_scroll_request = None;
                    editor.message = None;
                    invalidate_controls_cache(editor);
                    cx.notify();
                }
            }
            SettingsControl::SelectPaneTemplate(_)
            | SettingsControl::SelectPaneTemplateNode(_)
            | SettingsControl::NewPaneTemplate
            | SettingsControl::DuplicatePaneTemplate
            | SettingsControl::DeletePaneTemplate
            | SettingsControl::SplitPaneTemplate(_, _)
            | SettingsControl::RemovePaneTemplateNode(_)
            | SettingsControl::SwapPaneTemplateChildren(_)
            | SettingsControl::AddPaneTemplateArgument(_)
            | SettingsControl::RemovePaneTemplateArgument(_, _)
            | SettingsControl::AddPaneTemplateStackEntry(_)
            | SettingsControl::RemovePaneTemplateStackEntry(_, _)
            | SettingsControl::AddPaneTemplateStackArgument(_, _)
            | SettingsControl::RemovePaneTemplateStackArgument(_, _, _)
            | SettingsControl::AddPaneTemplateGlobalEnvironment
            | SettingsControl::RemovePaneTemplateGlobalEnvironment(_)
            | SettingsControl::AddPaneTemplateEnvironment(_)
            | SettingsControl::RemovePaneTemplateEnvironment(_, _)
            | SettingsControl::TogglePaneTemplateOverlay(_) => {
                let _ = pane_templates::activate_pane_template_control(self, control, window, cx);
            }
            SettingsControl::CloseProjectConfig
            | SettingsControl::SaveProjectConfig
            | SettingsControl::OpenProjectConfigFile
            | SettingsControl::ProjectTabIconPicker
            | SettingsControl::ClearProjectTabIcon
            | SettingsControl::AddProjectEnvironment
            | SettingsControl::RemoveProjectEnvironment(_)
            | SettingsControl::AddProjectProfile
            | SettingsControl::RemoveProjectProfile(_) => {
                self.activate_project_config_control(control, window, cx)
            }
            SettingsControl::ProjectOpacity => {}
            SettingsControl::AddProject => self.add_project_from_settings(window, cx),
            SettingsControl::OpenProject(index) => {
                self.open_project_from_settings(index, window, cx)
            }
            SettingsControl::EditProject(index) => {
                self.edit_project_from_settings(index, window, cx)
            }
            SettingsControl::RemoveProject(index) => {
                self.remove_project_from_settings(index, window, cx)
            }
        }
    }

    pub(crate) fn edit_settings_input(
        &mut self,
        event: &KeyDownEvent,
        command: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if settings_save_in_flight(editor) {
            return;
        }
        editor.clear_dropdown();
        let Some(input) = editor.focused_input else {
            return;
        };
        let field = match input {
            SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
            SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
            SettingsInput::PaneTemplate(field) => {
                pane_templates::pane_template_text_mut(editor, field)
            }
            SettingsInput::Project(field) => editor
                .project
                .as_mut()
                .and_then(|project| project.form.text_mut(field)),
            SettingsInput::ThemeSearch => Some(&mut editor.theme_extension_query),
            SettingsInput::FontSearch => editor.font_query.as_mut(),
            SettingsInput::KeymapSearch => Some(&mut editor.keymap_search),
            SettingsInput::ProfileDraft(field) => {
                editor.profile_draft.as_mut().map(|draft| match field {
                    ProfileDraftField::Name => &mut draft.name,
                    ProfileDraftField::Program => &mut draft.program,
                    ProfileDraftField::Arguments => &mut draft.arguments,
                })
            }
        };
        let Some(field) = field else {
            return;
        };
        let key = event.keystroke.key.as_str();
        // Settled before the surface's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        let clipboard = apply_clipboard_shortcut(field.edit(), &event.keystroke, cx);
        // Whether the keystroke changed the text. Moving the cursor, selecting
        // and copying do not, and used to be treated as edits all the same: an
        // arrow key was enough to make the dialog believe it had unsaved changes,
        // rebuild the control cache and clear whatever it was showing.
        let edited = match clipboard {
            ClipboardOutcome::Edited => true,
            ClipboardOutcome::Unchanged => false,
            ClipboardOutcome::Ignored => match key {
                "backspace" => {
                    field.backspace();
                    true
                }
                "delete" => {
                    field.delete();
                    true
                }
                "left" => {
                    field.move_left();
                    false
                }
                "right" => {
                    field.move_right();
                    false
                }
                "home" => {
                    field.cursor = 0;
                    field.select_all = false;
                    false
                }
                "end" => {
                    field.cursor = field.text.len();
                    field.select_all = false;
                    false
                }
                _ if command && key.eq_ignore_ascii_case("a") => {
                    field.select_all();
                    false
                }
                _ if !command && !event.keystroke.modifiers.alt => {
                    match event.keystroke.key_char.as_ref() {
                        Some(text) => {
                            field.insert(text);
                            true
                        }
                        None => false,
                    }
                }
                _ => false,
            },
        };
        if !edited {
            // Copying is otherwise silent, and a clipboard that may or may not
            // have been written is the thing worth saying something about.
            if is_copy_chord(&event.keystroke) {
                editor.message = Some((false, "Copied the field to the clipboard.".to_owned()));
            }
            cx.notify();
            return;
        }
        match input {
            SettingsInput::Configuration(_) => {
                editor.configuration_dirty = true;
                invalidate_controls_cache(editor);
            }
            SettingsInput::Keymap(_) => {
                editor.keymap_dirty = true;
                refresh_keymap_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::ThemeSearch => {}
            SettingsInput::FontSearch => {
                Self::rebuild_font_search_cache(editor);
            }
            SettingsInput::KeymapSearch => {
                refresh_keymap_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::PaneTemplate(_) => {
                editor.configuration_dirty = true;
                if matches!(
                    input,
                    SettingsInput::PaneTemplate(PaneTemplateTextField::Name(_))
                ) {
                    pane_templates::refresh_template_names(editor);
                }
                invalidate_controls_cache(editor);
            }
            SettingsInput::Project(_) => {
                projects::mark_project_dirty(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::ProfileDraft(_) => {}
        }
        editor.message = None;
        if matches!(input, SettingsInput::PaneTemplate(_)) {
            pane_templates::schedule_pane_template_validation(self, cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn set_settings_dropdown(
        &mut self,
        dropdown: SettingsDropdown,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let pane_template_dropdown = matches!(
            dropdown,
            SettingsDropdown::PaneTemplateAxis(_)
                | SettingsDropdown::PaneTemplateSource(_)
                | SettingsDropdown::PaneTemplateTheme(_)
                | SettingsDropdown::PaneTemplateDarkTheme(_)
                | SettingsDropdown::PaneTemplateOverlaySize(_)
        );
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if settings_save_in_flight(editor) {
            return;
        }
        editor.clear_dropdown();
        match dropdown {
            SettingsDropdown::DefaultProfile => {
                editor.configuration.default_profile = value;
            }
            SettingsDropdown::NewTabProfile => {
                editor.configuration.new_tab_profile = if value == "Inherit" {
                    NewTabProfile::Inherit
                } else {
                    NewTabProfile::Default
                };
            }
            SettingsDropdown::Theme => editor.configuration.theme = value,
            SettingsDropdown::DarkTheme => editor.configuration.dark_theme = value,
            SettingsDropdown::WorkingDirectoryScope => {
                editor.configuration.working_directory_scope = match value.as_str() {
                    "None" => WorkingDirectoryScope::None,
                    "Pane" => WorkingDirectoryScope::Pane,
                    _ => WorkingDirectoryScope::Tab,
                };
            }
            SettingsDropdown::PaneControlsPosition => {
                editor.configuration.pane_controls_position = if value == "Left" {
                    PaneControlsPosition::Left
                } else {
                    PaneControlsPosition::Right
                };
            }
            SettingsDropdown::PaneControlsDefaultVisibility => {
                editor.configuration.pane_controls_hidden_by_default = value == "Hidden";
            }
            SettingsDropdown::SessionRetention => {
                editor.configuration.session_retention = match value.as_str() {
                    "None" => crate::config::SessionRetention::None,
                    "Disk" => crate::config::SessionRetention::Disk,
                    _ => crate::config::SessionRetention::Memory,
                };
            }
            SettingsDropdown::ProfileTheme(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::ProfileDarkTheme(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.dark_theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::ProfileIcon(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.icon = if value == "Automatic" {
                        None
                    } else {
                        ProfileIcon::parse_name(&value.to_ascii_lowercase())
                            .ok()
                            .flatten()
                    };
                }
            }
            SettingsDropdown::ProfileDraftTheme => {
                if let Some(profile) = editor.profile_draft.as_mut() {
                    profile.theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::ProfileDraftDarkTheme => {
                if let Some(profile) = editor.profile_draft.as_mut() {
                    profile.dark_theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::ProfileDraftIcon => {
                if let Some(profile) = editor.profile_draft.as_mut() {
                    profile.icon = if value == "Automatic" {
                        None
                    } else {
                        ProfileIcon::parse_name(&value.to_ascii_lowercase())
                            .ok()
                            .flatten()
                    };
                }
            }
            SettingsDropdown::BindingAction(section, binding) => {
                if let Some(binding) = editor
                    .keymap
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.bindings.get_mut(binding))
                {
                    binding.action = if value == ApplyPaneSplitTemplate::name_for_type() {
                        serde_json::json!([
                            value,
                            {
                                "name": editor
                                    .pane_template_names
                                    .first()
                                    .cloned()
                                    .unwrap_or_default()
                            }
                        ])
                    } else if value == OpenProfile::name_for_type() {
                        serde_json::json!([value, { "slot": 1 }])
                    } else {
                        serde_json::Value::String(value)
                    };
                }
            }
            SettingsDropdown::BindingTemplate(section, binding) => {
                if let Some(arguments) = editor
                    .keymap
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.bindings.get_mut(binding))
                    .and_then(|binding| binding.action.as_array_mut())
                    .and_then(|action| action.get_mut(1))
                    .and_then(serde_json::Value::as_object_mut)
                {
                    arguments.insert("name".to_owned(), serde_json::Value::String(value));
                }
            }
            SettingsDropdown::BindingProfile(section, binding) => {
                let Some(slot) = editor
                    .profile_names
                    .iter()
                    .position(|profile| profile == &value)
                    .map(|index| index + 1)
                else {
                    return;
                };
                if let Some(arguments) = editor
                    .keymap
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.bindings.get_mut(binding))
                    .and_then(|binding| binding.action.as_array_mut())
                    .and_then(|action| action.get_mut(1))
                    .and_then(serde_json::Value::as_object_mut)
                {
                    arguments.insert("slot".to_owned(), serde_json::json!(slot));
                }
            }
            SettingsDropdown::PaneTemplateAxis(_)
            | SettingsDropdown::PaneTemplateSource(_)
            | SettingsDropdown::PaneTemplateTheme(_)
            | SettingsDropdown::PaneTemplateDarkTheme(_)
            | SettingsDropdown::PaneTemplateOverlaySize(_) => {
                if !pane_templates::set_pane_template_dropdown(editor, dropdown, &value) {
                    return;
                }
            }
            SettingsDropdown::ProjectTheme
            | SettingsDropdown::ProjectDarkTheme
            | SettingsDropdown::ProjectDefaultProfile
            | SettingsDropdown::ProjectInitialSplit
            | SettingsDropdown::ProjectProfileTheme(_)
            | SettingsDropdown::ProjectProfileDarkTheme(_)
            | SettingsDropdown::ProjectProfileIcon(_) => {
                if !projects::set_project_dropdown(editor, dropdown, &value) {
                    return;
                }
            }
        }
        match dropdown {
            SettingsDropdown::BindingAction(_, _) | SettingsDropdown::BindingTemplate(_, _) => {
                editor.keymap_dirty = true;
                refresh_keymap_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsDropdown::ProfileDraftTheme
            | SettingsDropdown::ProfileDraftDarkTheme
            | SettingsDropdown::ProfileDraftIcon
            | SettingsDropdown::ProjectTheme
            | SettingsDropdown::ProjectDarkTheme
            | SettingsDropdown::ProjectDefaultProfile
            | SettingsDropdown::ProjectInitialSplit
            | SettingsDropdown::ProjectProfileTheme(_)
            | SettingsDropdown::ProjectProfileDarkTheme(_)
            | SettingsDropdown::ProjectProfileIcon(_) => {}
            _ => editor.configuration_dirty = true,
        }
        editor.message = None;
        if pane_template_dropdown {
            pane_templates::schedule_pane_template_validation(self, cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn set_settings_toggle(
        &mut self,
        toggle: SettingsToggle,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inherited_opacity = self.launch_config.inactive_pane_opacity;
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        match toggle {
            SettingsToggle::CompactMode => editor.configuration.compact_mode = value,
            SettingsToggle::PaneSize => editor.configuration.hide_pane_size = value,
            SettingsToggle::TitleBarLabels => editor.configuration.hide_title_bar_labels = value,
            SettingsToggle::TitleBarButtons => editor.configuration.hide_title_bar_buttons = value,
            #[cfg(feature = "session-persistence")]
            SettingsToggle::SessionAutoProtect => {
                editor.configuration.session_persistence_auto_protect = value
            }
            SettingsToggle::ProfileVisibility(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.hidden = !value;
                }
            }
            SettingsToggle::ProfileDraftVisibility => {
                if let Some(profile) = editor.profile_draft.as_mut() {
                    profile.hidden = !value;
                }
            }
            #[cfg(target_os = "macos")]
            SettingsToggle::TitleBarMenus => editor.configuration.hide_title_bar_menus = value,
            SettingsToggle::ProjectOpacityOverride => {
                if let Some(project) = editor.project.as_mut() {
                    // Turning the override on starts from whatever the user
                    // configuration resolves to, so the slider does not jump.
                    project.form.inactive_pane_opacity = value.then_some(inherited_opacity);
                }
            }
            SettingsToggle::ProjectProfileVisibility(index) => {
                if let Some(profile) = editor
                    .project
                    .as_mut()
                    .and_then(|project| project.form.profiles.get_mut(index))
                {
                    profile.hidden = !value;
                }
            }
        }
        if matches!(
            toggle,
            SettingsToggle::ProjectOpacityOverride | SettingsToggle::ProjectProfileVisibility(_)
        ) {
            projects::mark_project_dirty(editor);
            invalidate_controls_cache(editor);
        } else if !matches!(toggle, SettingsToggle::ProfileDraftVisibility) {
            editor.configuration_dirty = true;
        }
        editor.message = None;
        self.focus_settings_control(SettingsControl::Toggle(toggle), window, cx);
        cx.notify();
    }

    /// The inactive-pane opacity a target currently shows. A project that does
    /// not override it has no slider, so the fallback only matters for the
    /// frame in which the override is being switched on.
    pub(crate) fn settings_opacity(editor: &SettingsEditor, target: OpacityTarget) -> Option<f32> {
        match target {
            OpacityTarget::Configuration => Some(editor.configuration.inactive_pane_opacity),
            OpacityTarget::Project => editor
                .project
                .as_ref()
                .and_then(|project| project.form.inactive_pane_opacity),
        }
    }

    pub(crate) fn set_settings_opacity(
        &mut self,
        target: OpacityTarget,
        opacity: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let opacity = opacity.clamp(0., 1.);
        match target {
            OpacityTarget::Configuration => {
                editor.configuration.inactive_pane_opacity = opacity;
                editor.configuration_dirty = true;
                editor.message = None;
            }
            OpacityTarget::Project => {
                let Some(project) = editor
                    .project
                    .as_mut()
                    .filter(|project| project.form.inactive_pane_opacity.is_some())
                else {
                    return;
                };
                project.form.inactive_pane_opacity = Some(opacity);
                projects::mark_project_dirty(editor);
            }
        }
        cx.notify();
    }

    pub(crate) fn adjust_settings_opacity(
        &mut self,
        target: OpacityTarget,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| Self::settings_opacity(editor, target))
        else {
            return;
        };
        self.set_settings_opacity(target, current + direction as f32 / 20., cx);
    }

    pub(crate) fn adjust_numeric_setting(
        &mut self,
        setting: NumericSetting,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let configuration = &mut editor.configuration;
        match setting {
            NumericSetting::FontSize => {
                let current = configuration
                    .terminal_font_size
                    .text
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(14.);
                let value = (current + direction as f32).clamp(6., 100.);
                configuration.terminal_font_size = TextField::new(format!("{value}"));
            }
            NumericSetting::ScrollHistory => {
                let maximum = terminal::MAX_SCROLL_HISTORY_LINES as u64;
                let current = if configuration
                    .max_scroll_history_lines
                    .text
                    .trim()
                    .eq_ignore_ascii_case("max")
                {
                    maximum
                } else {
                    configuration
                        .max_scroll_history_lines
                        .text
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0)
                        .min(maximum)
                };
                let value = adjusted_scroll_history(current, direction, maximum);
                configuration.max_scroll_history_lines = TextField::new(if value == maximum {
                    "Max".to_owned()
                } else {
                    value.to_string()
                });
            }
            #[cfg(feature = "http-server")]
            NumericSetting::HttpServerPort => {
                let current = configuration
                    .http_server_port
                    .text
                    .trim()
                    .parse::<u16>()
                    .unwrap_or(config::DEFAULT_HTTP_PORT);
                configuration.http_server_port = TextField::new(
                    current
                        .saturating_add_signed(direction as i16)
                        .clamp(1, u16::MAX)
                        .to_string(),
                );
            }
            #[cfg(feature = "tftp-server")]
            NumericSetting::TftpServerPort => {
                let current = configuration
                    .tftp_server_port
                    .text
                    .trim()
                    .parse::<u16>()
                    .unwrap_or(config::DEFAULT_TFTP_SERVER_PORT);
                configuration.tftp_server_port = TextField::new(
                    current
                        .saturating_add_signed(direction as i16)
                        .clamp(1, u16::MAX)
                        .to_string(),
                );
            }
            NumericSetting::SessionRingBytes => {
                let current = configuration
                    .session_ring_bytes
                    .text
                    .trim()
                    .parse::<usize>()
                    .unwrap_or(config::DEFAULT_SESSION_RING_BYTES);
                let value = current
                    .saturating_add_signed(direction.saturating_mul(4096) as isize)
                    .clamp(4 * 1024, config::MAX_SESSION_RING_BYTES);
                configuration.session_ring_bytes = TextField::new(value.to_string());
            }
        }
        editor.configuration_dirty = true;
        editor.message = None;
        cx.notify();
    }

    pub(crate) fn begin_numeric_repeat(
        &mut self,
        setting: NumericSetting,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        let generation = editor.numeric_repeat_generation;
        self.adjust_numeric_setting(setting, direction, cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            loop {
                let repeating = this
                    .update(cx, |this, cx| {
                        let repeating = this
                            .settings_editor
                            .as_ref()
                            .is_some_and(|editor| editor.numeric_repeat_generation == generation);
                        if repeating {
                            this.adjust_numeric_setting(setting, direction, cx);
                        }
                        repeating
                    })
                    .unwrap_or(false);
                if !repeating {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(75))
                    .await;
            }
        })
        .detach();
    }

    pub(crate) fn end_numeric_repeat(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        }
        cx.notify();
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
