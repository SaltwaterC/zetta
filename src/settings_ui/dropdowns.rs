//! The settings dropdowns: what a dropdown offers, and what choosing an
//! option does.
//!
//! A dropdown's options are a snapshot rather than a live list, so typing to
//! filter and arrowing through them cannot be invalidated by a background
//! refresh — a theme extension finishing its download, say — while the
//! dropdown is open.

use super::*;

use super::controls::{dropdown_snapshot_rows, scroll_open_dropdown_to_selection};

impl Zetta {
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
}

impl Zetta {
    /// Re-snapshots the open dropdown (if any) after its underlying options
    /// changed while it was open, for example when installing or removing a
    /// theme extension rebuilds the theme list.
    pub(crate) fn refresh_open_dropdown_options(editor: &mut SettingsEditor) {
        let Some(dropdown) = editor.open_dropdown else {
            return;
        };
        let (_, options) = Self::settings_dropdown_options(editor, dropdown);
        Self::refresh_open_dropdown_snapshot(editor, options);
        scroll_open_dropdown_to_selection(editor);
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
}

impl Zetta {
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
}
