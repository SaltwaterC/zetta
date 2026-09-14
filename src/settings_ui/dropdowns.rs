//! The settings dropdowns: what a dropdown offers, and what choosing an
//! option does.
//!
//! A dropdown's options are a snapshot rather than a live list, so typing to
//! filter and arrowing through them cannot be invalidated by a background
//! refresh — a theme extension finishing its download, say — while the
//! dropdown is open.

use super::*;

use super::controls::scroll_open_dropdown_to_selection;

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
            SettingsDropdown::RemoteSessionProtocol => (
                remote_protocol_label(editor.configuration.remote_session_protocol).to_owned(),
                Arc::from([
                    String::from(REMOTE_PROTOCOL_SSH),
                    String::from(REMOTE_PROTOCOL_ZOSH),
                ]),
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
            SettingsDropdown::ProfileIcon(index) => {
                let profile = editor.configuration.profiles.get(index);
                (
                    profile
                        .and_then(|profile| profile.icon.as_ref())
                        .map_or("Automatic", ProfileIcon::label)
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
                    .map_or("Automatic", ProfileIcon::label)
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
        editor.dropdown.set_options(options);
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
        let selected_index = options
            .iter()
            .position(|option| option == &selected)
            .unwrap_or(0);
        editor.dropdown.open(options, selected_index, anchor);
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
        let moved = editor.dropdown.move_selection(direction);
        if moved {
            cx.notify();
        }
        moved
    }

    pub(crate) fn commit_open_settings_dropdown_value(
        &mut self,
        value: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(dropdown) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.open_dropdown)
        else {
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
        apply_settings_dropdown_value(editor, dropdown, value);
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

/// Writes a dropdown's chosen option into the form behind it.
///
/// Separate from [`Zetta::set_settings_dropdown`] because that also has to
/// close the popup, refresh caches and notify; this is only the assignment, and
/// it is one arm per dropdown.
fn apply_settings_dropdown_value(
    editor: &mut SettingsEditor,
    dropdown: SettingsDropdown,
    value: String,
) {
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
        SettingsDropdown::RemoteSessionProtocol => {
            editor.configuration.remote_session_protocol = if value == REMOTE_PROTOCOL_ZOSH {
                crate::config::RemoteSessionProtocol::Zosh
            } else {
                crate::config::RemoteSessionProtocol::Ssh
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
            if !pane_templates::set_pane_template_dropdown(editor, dropdown, &value) {}
        }
        SettingsDropdown::ProjectTheme
        | SettingsDropdown::ProjectDarkTheme
        | SettingsDropdown::ProjectDefaultProfile
        | SettingsDropdown::ProjectInitialSplit
        | SettingsDropdown::ProjectProfileTheme(_)
        | SettingsDropdown::ProjectProfileDarkTheme(_)
        | SettingsDropdown::ProjectProfileIcon(_) => {
            if !projects::set_project_dropdown(editor, dropdown, &value) {}
        }
    }
}

/// How the two remote protocols are labelled. The names the configuration file
/// uses are lowercase; these are what a reader of the dialog expects to see.
const REMOTE_PROTOCOL_SSH: &str = "SSH";
const REMOTE_PROTOCOL_ZOSH: &str = "Zosh";

fn remote_protocol_label(protocol: crate::config::RemoteSessionProtocol) -> &'static str {
    match protocol {
        crate::config::RemoteSessionProtocol::Ssh => REMOTE_PROTOCOL_SSH,
        crate::config::RemoteSessionProtocol::Zosh => REMOTE_PROTOCOL_ZOSH,
    }
}
