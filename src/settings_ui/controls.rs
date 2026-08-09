use super::keymap::{
    KeymapRow, invalidate_keymap_cache, keymap_filtered_indices, keymap_rows,
    rebuild_keymap_search_cache,
};
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
            return vec![
                SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Name)),
                SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Program)),
                SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Arguments)),
                SettingsControl::Dropdown(SettingsDropdown::ProfileDraftTheme),
                SettingsControl::CreateProfile,
            ];
        }

        if editor.keymap_capture.is_some() {
            return Vec::new();
        }

        let mut controls = vec![
            SettingsControl::Tab(SettingsPage::Configuration),
            SettingsControl::Tab(SettingsPage::Themes),
            SettingsControl::Tab(SettingsPage::Keymap),
            SettingsControl::Save,
        ];
        match editor.page {
            SettingsPage::Configuration => {
                controls.extend([
                    SettingsControl::Dropdown(SettingsDropdown::DefaultProfile),
                    SettingsControl::Dropdown(SettingsDropdown::NewTabProfile),
                    SettingsControl::Dropdown(SettingsDropdown::Theme),
                    SettingsControl::DefaultTabIconPicker,
                    SettingsControl::Numeric(NumericSetting::FontSize),
                    SettingsControl::FontPicker,
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::WorkingDirectory,
                    )),
                    SettingsControl::Dropdown(SettingsDropdown::WorkingDirectoryScope),
                    SettingsControl::Numeric(NumericSetting::ScrollHistory),
                    SettingsControl::Opacity,
                    SettingsControl::Toggle(SettingsToggle::CompactMode),
                    SettingsControl::Toggle(SettingsToggle::PaneSize),
                    SettingsControl::Toggle(SettingsToggle::TitleBarLabels),
                    SettingsControl::Toggle(SettingsToggle::TitleBarButtons),
                    #[cfg(target_os = "macos")]
                    SettingsControl::Toggle(SettingsToggle::TitleBarMenus),
                    SettingsControl::Dropdown(SettingsDropdown::PaneControlsPosition),
                    SettingsControl::Dropdown(SettingsDropdown::PaneControlsDefaultVisibility),
                ]);
                #[cfg(feature = "http-server")]
                controls.push(SettingsControl::Numeric(NumericSetting::HttpServerPort));
                #[cfg(feature = "tftp-server")]
                controls.push(SettingsControl::Numeric(NumericSetting::TftpServerPort));
                for (index, profile) in editor.configuration.profiles.iter().enumerate() {
                    if !profile.detected {
                        controls.extend([
                            SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileName(index),
                            )),
                            SettingsControl::RemoveProfile(index),
                            SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileProgram(index),
                            )),
                            SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileArguments(index),
                            )),
                        ]);
                    } else {
                        controls.push(SettingsControl::Toggle(SettingsToggle::ProfileVisibility(
                            index,
                        )));
                    }
                    controls.push(SettingsControl::Dropdown(SettingsDropdown::ProfileTheme(
                        index,
                    )));
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
        if editor.profile_draft.is_some() {
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
        let controls = Self::settings_controls(editor);
        let Some(index) = controls.iter().position(|candidate| candidate == control) else {
            return;
        };
        const FORM_START: usize = 5;
        if index < FORM_START {
            return;
        }
        let form_index = index - FORM_START;
        let form_count = controls.len().saturating_sub(FORM_START);
        let progress = form_index as f32 / form_count.saturating_sub(1).max(1) as f32;
        let maximum = editor.settings_scroll.max_offset().y;
        let offset = editor.settings_scroll.offset();
        editor
            .settings_scroll
            .set_offset(point(offset.x, -(maximum * progress)));
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
        }
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
        // Only one dropdown is ever open at a time, so the previous dropdown's
        // filtered-options entry is dead weight the moment a different one opens.
        editor.dropdown_filtered_options.clear();
        editor
            .dropdown_scroll
            .scroll_to_item(editor.dropdown_index, ScrollStrategy::Nearest);
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
        let Some(dropdown) = editor.open_dropdown else {
            return false;
        };
        let (_, options) = Self::settings_dropdown_options(editor, dropdown);
        let matching_indices = fuzzy_match_indices(&options, &editor.dropdown_query);
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
        editor
            .dropdown_scroll
            .scroll_to_item(editor.dropdown_index, ScrollStrategy::Nearest);
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
            editor
                .dropdown_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
        if query.is_empty() {
            editor.dropdown_filtered_options.remove(&dropdown);
        } else {
            editor
                .dropdown_filtered_options
                .insert(dropdown, fuzzy_match_indices(&options, &query));
        }
        cx.notify();
        true
    }

    pub(crate) fn commit_open_settings_dropdown(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((dropdown, value)) = self.settings_editor.as_mut().and_then(|editor| {
            let dropdown = editor.open_dropdown.take()?;
            let (_, options) = Self::settings_dropdown_options(editor, dropdown);
            if !editor.dropdown_query.is_empty()
                && fuzzy_match_indices(&options, &editor.dropdown_query).is_empty()
            {
                editor.open_dropdown = Some(dropdown);
                return None;
            }
            options
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
        match control {
            SettingsControl::Tab(page) => self.select_settings_page(page, window, cx),
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
                    SettingsToggle::ProfileVisibility(index) => editor
                        .configuration
                        .profiles
                        .get(index)
                        .is_some_and(|profile| !profile.hidden),
                    #[cfg(target_os = "macos")]
                    SettingsToggle::TitleBarMenus => editor.configuration.hide_title_bar_menus,
                });
                if let Some(value) = value {
                    self.set_settings_toggle(toggle, !value, window, cx);
                }
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
                    editor.font_query = None;
                    editor.focused_input = None;
                    editor.focused_control = None;
                    editor.message = None;
                    cx.notify();
                }
            }
            SettingsControl::CreateProfile => {
                let valid = self.settings_editor.as_ref().is_some_and(|editor| {
                    editor.profile_draft.as_ref().is_some_and(|draft| {
                        !draft.name.text.trim().is_empty() && !draft.program.text.trim().is_empty()
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
                    editor
                        .configuration
                        .profiles
                        .push(editor.profile_draft.take().unwrap());
                    editor.configuration_dirty = true;
                    editor.focused_input = None;
                    editor.focused_control = None;
                    editor.message = None;
                    cx.notify();
                }
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
        editor.open_dropdown = None;
        editor.dropdown_query.clear();
        let Some(input) = editor.focused_input else {
            return;
        };
        let field = match input {
            SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
            SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
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
        match event.keystroke.key.as_str() {
            "backspace" => field.backspace(),
            "delete" => field.delete(),
            "left" => field.move_left(),
            "right" => field.move_right(),
            "home" => {
                field.cursor = 0;
                field.select_all = false;
            }
            "end" => {
                field.cursor = field.text.len();
                field.select_all = false;
            }
            "a" if command => field.select_all(),
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    field.insert(&text);
                }
            }
            _ if !command && !event.keystroke.modifiers.alt => {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    field.insert(text);
                }
            }
            _ => {}
        }
        match input {
            SettingsInput::Configuration(_) => {
                editor.configuration_dirty = true;
                invalidate_controls_cache(editor);
            }
            SettingsInput::Keymap(_) => {
                editor.keymap_dirty = true;
                invalidate_keymap_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::ThemeSearch => {}
            SettingsInput::FontSearch => {
                Self::rebuild_font_search_cache(editor);
            }
            SettingsInput::KeymapSearch => {
                rebuild_keymap_search_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::ProfileDraft(_) => {}
        }
        editor.message = None;
        cx.notify();
    }

    pub(crate) fn set_settings_dropdown(
        &mut self,
        dropdown: SettingsDropdown,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        editor.open_dropdown = None;
        editor.dropdown_query.clear();
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
            SettingsDropdown::ProfileTheme(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::ProfileDraftTheme => {
                if let Some(profile) = editor.profile_draft.as_mut() {
                    profile.theme = (value != "Use application theme").then_some(value);
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
        }
        match dropdown {
            SettingsDropdown::BindingAction(_, _) | SettingsDropdown::BindingTemplate(_, _) => {
                editor.keymap_dirty = true;
                invalidate_keymap_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsDropdown::ProfileDraftTheme => {}
            _ => editor.configuration_dirty = true,
        }
        editor.message = None;
        cx.notify();
    }

    pub(crate) fn set_settings_toggle(
        &mut self,
        toggle: SettingsToggle,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        match toggle {
            SettingsToggle::CompactMode => editor.configuration.compact_mode = value,
            SettingsToggle::PaneSize => editor.configuration.hide_pane_size = value,
            SettingsToggle::TitleBarLabels => editor.configuration.hide_title_bar_labels = value,
            SettingsToggle::TitleBarButtons => editor.configuration.hide_title_bar_buttons = value,
            SettingsToggle::ProfileVisibility(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.hidden = !value;
                }
            }
            #[cfg(target_os = "macos")]
            SettingsToggle::TitleBarMenus => editor.configuration.hide_title_bar_menus = value,
        }
        editor.configuration_dirty = true;
        editor.message = None;
        self.focus_settings_control(SettingsControl::Toggle(toggle), window, cx);
        cx.notify();
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

#[cfg(test)]
#[path = "../tests/settings_ui/controls.rs"]
mod tests;
