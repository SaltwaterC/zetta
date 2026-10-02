//! The typed form behind the Configuration page, and its serialization.
//!
//! The form is built over the file's parsed root object and written back into
//! it, so keys Zetta does not know about survive a round trip. Values equal to
//! the default are stripped on write rather than spelled out, so the file
//! stays a list of what the user actually chose.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigTextField {
    /// The field of a number or text setting; see `settings_table`.
    Setting(ConfigSetting),
    ProfileName(usize),
    ProfileProgram(usize),
    /// One argument of a profile: the profile, then the argument.
    ProfileArgument(usize, usize),
}

/// A value the Configuration page cannot save, naming the field it is in so
/// the dialog can take the keyboard there. Reported in the page's own words
/// ("Font size must be…"), not the file's key names.
#[derive(Debug)]
pub(crate) struct InvalidField {
    pub(crate) field: ConfigTextField,
    message: String,
}

impl std::fmt::Display for InvalidField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for InvalidField {}

fn invalid(field: ConfigTextField, message: impl Into<String>) -> anyhow::Error {
    InvalidField {
        field,
        message: message.into(),
    }
    .into()
}

#[derive(Clone, Debug)]
pub struct ConfigurationForm {
    root: Map<String, Value>,
    pub default_profile: String,
    pub new_tab_profile: NewTabProfile,
    pub working_directory: TextField,
    pub working_directory_scope: WorkingDirectoryScope,
    pub theme: String,
    pub dark_theme: String,
    pub default_tab_icon: Option<IconName>,
    pub terminal_font_size: TextField,
    pub terminal_font_family: String,
    pub max_scroll_history_lines: TextField,
    pub inactive_pane_opacity: f32,
    pub compact_mode: bool,
    pub hide_pane_size: bool,
    pub hide_title_bar_labels: bool,
    pub hide_title_bar_buttons: bool,
    #[cfg(target_os = "macos")]
    pub hide_title_bar_menus: bool,
    pub pane_controls_position: PaneControlsPosition,
    pub pane_controls_hidden_by_default: bool,
    pub session_retention: SessionRetention,
    pub session_ring_bytes: TextField,
    /// What carries a remote session's panes. Its control traffic is OpenSSH
    /// whichever this is.
    pub remote_session_protocol: crate::config::RemoteSessionProtocol,
    /// The Zosh keep-alive interval in milliseconds. Empty leaves the link on
    /// Mosh's own heartbeat.
    pub remote_session_keep_alive: TextField,
    /// Whether a Zosh pane may forward the local SSH agent.
    pub remote_session_forward_agent: bool,
    #[cfg(feature = "session-persistence")]
    pub session_persistence_recipients: TextField,
    #[cfg(feature = "session-persistence")]
    pub session_persistence_identity: TextField,
    /// Whether background sessions are protected with the configured age key
    /// instead of a secret typed into a dialog. Only meaningful, and only
    /// offered, alongside a recipient and an effective identity.
    #[cfg(feature = "session-persistence")]
    pub session_persistence_auto_protect: bool,
    #[cfg(feature = "http-server")]
    pub http_server_port: TextField,
    #[cfg(feature = "tftp-server")]
    pub tftp_server_port: TextField,
    pub profiles: Vec<ProfileForm>,
    pub pane_templates: PaneTemplatesForm,
}

impl ConfigurationForm {
    /// Whether the automatic-protection toggle should be shown at all.
    ///
    /// Read from the form rather than the loaded configuration, so the toggle
    /// appears as soon as a recipient and an identity have been typed, or when
    /// the conventional `~/.ssh/id_ed25519` identity is available. Both the
    /// page and the tab order ask this, so a control that is not drawn is never
    /// a stop the focus ring lands on.
    #[cfg(feature = "session-persistence")]
    pub fn session_auto_protect_is_offered(&self) -> bool {
        !self.session_persistence_recipients.text.trim().is_empty()
            && (!self.session_persistence_identity.text.trim().is_empty()
                || crate::config::default_session_identity_path().is_some())
    }

    pub fn load(path: &Path, config: &Config) -> Result<Self> {
        Self::from_value(read_json_or(path, json!({}))?, config)
    }

    /// [`Self::load`] from text already read, `None` meaning the file does not
    /// exist. A configuration reload reads the file once, on its worker, and
    /// builds both the configuration and this form from that one read.
    pub fn parse(source: Option<&str>, path: &Path, config: &Config) -> Result<Self> {
        let value = match source {
            Some(source) => serde_json::from_str(source)
                .with_context(|| format!("parsing {}", path.display()))?,
            None => json!({}),
        };
        Self::from_value(value, config)
    }

    fn from_value(value: Value, config: &Config) -> Result<Self> {
        let root = value
            .as_object()
            .context("configuration root must be an object")?
            .clone();
        let configured_profiles = root
            .get("profiles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let profiles = config
            .profiles
            .iter()
            .map(|resolved| -> Result<ProfileForm> {
                let configured = configured_profiles.iter().find_map(|profile| {
                    let profile = profile.as_object()?;
                    profile
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(&resolved.name))
                        .then_some(profile)
                });
                let icon = configured
                    .and_then(|profile| profile.get("icon"))
                    .map(ProfileIcon::parse)
                    .transpose()?
                    .flatten();
                let detected = configured.is_none_or(|profile| !profile.contains_key("program"));
                Ok(ProfileForm {
                    name: TextField::new(resolved.name.clone()),
                    program: TextField::new(
                        configured
                            .and_then(|profile| profile.get("program"))
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ),
                    arguments: profile_arguments_from_json(
                        configured.and_then(|profile| profile.get("args")),
                    ),
                    theme: configured
                        .and_then(|profile| profile.get("theme"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| resolved.theme.clone()),
                    dark_theme: configured
                        .and_then(|profile| profile.get("dark_theme"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| resolved.dark_theme.clone()),
                    icon,
                    automatic_icon: ProfileIcon::automatic_for_profile(
                        &resolved.name,
                        &resolved.command,
                    ),
                    hidden: configured
                        .and_then(|profile| profile.get("hidden"))
                        .and_then(Value::as_bool)
                        .unwrap_or_else(|| profile_is_hidden(resolved, &config.hidden_profiles)),
                    detected,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let pane_templates = PaneTemplatesForm::load(root.get("pane_split_templates"), config)?;
        // Every scalar starts blank and is then filled from the file by the
        // settings table, so how a setting is read lives in one place.
        let mut form = Self {
            root,
            default_profile: String::new(),
            new_tab_profile: NewTabProfile::default(),
            working_directory: TextField::default(),
            working_directory_scope: WorkingDirectoryScope::default(),
            theme: String::new(),
            dark_theme: String::new(),
            default_tab_icon: None,
            terminal_font_size: TextField::default(),
            terminal_font_family: String::new(),
            max_scroll_history_lines: TextField::default(),
            inactive_pane_opacity: crate::config::DEFAULT_INACTIVE_PANE_OPACITY,
            compact_mode: false,
            hide_pane_size: false,
            hide_title_bar_labels: false,
            hide_title_bar_buttons: false,
            #[cfg(target_os = "macos")]
            hide_title_bar_menus: false,
            pane_controls_position: PaneControlsPosition::default(),
            pane_controls_hidden_by_default: false,
            session_retention: SessionRetention::default(),
            session_ring_bytes: TextField::default(),
            remote_session_protocol: crate::config::RemoteSessionProtocol::default(),
            remote_session_keep_alive: TextField::default(),
            remote_session_forward_agent: false,
            #[cfg(feature = "session-persistence")]
            session_persistence_recipients: TextField::default(),
            #[cfg(feature = "session-persistence")]
            session_persistence_identity: TextField::default(),
            #[cfg(feature = "session-persistence")]
            session_persistence_auto_protect: false,
            #[cfg(feature = "http-server")]
            http_server_port: TextField::default(),
            #[cfg(feature = "tftp-server")]
            tftp_server_port: TextField::default(),
            profiles,
            pane_templates,
        };
        for &setting in ALL_SETTINGS {
            let value = get_path(&form.root, setting.spec().key).cloned();
            setting.decode(&mut form, value.as_ref(), config);
        }
        Ok(form)
    }

    pub fn text_mut(&mut self, field: ConfigTextField) -> Option<&mut TextField> {
        match field {
            ConfigTextField::Setting(setting) => setting.text_mut(self),
            ConfigTextField::ProfileName(index) => {
                self.profiles.get_mut(index).map(|p| &mut p.name)
            }
            ConfigTextField::ProfileProgram(index) => {
                self.profiles.get_mut(index).map(|p| &mut p.program)
            }
            ConfigTextField::ProfileArgument(index, argument) => self
                .profiles
                .get_mut(index)
                .and_then(|p| p.arguments.get_mut(argument)),
        }
    }

    /// Why `setting`'s field cannot be saved as it stands, if it cannot: what
    /// the page checks as the keyboard leaves a field, as well as on Save.
    pub(crate) fn check(&self, setting: ConfigSetting) -> Option<String> {
        setting.encode(self).err()
    }

    pub fn to_json(&self) -> Result<String> {
        let mut root = self.root.clone();
        for &setting in ALL_SETTINGS {
            setting
                .write(self, &mut root)
                .map_err(|message| invalid(ConfigTextField::Setting(setting), message))?;
        }
        if let Err(problem) = check_profiles(&self.profiles) {
            let index = problem.profile();
            let field = if problem.is_in_program() {
                ConfigTextField::ProfileProgram(index)
            } else {
                ConfigTextField::ProfileName(index)
            };
            return Err(invalid(field, problem.to_string()));
        }
        if !self.profiles.is_empty() || root.contains_key("profiles") {
            root.insert(
                "profiles".into(),
                Value::Array(
                    self.profiles
                        .iter()
                        .filter(|profile| {
                            !profile.detected
                                || profile.theme.is_some()
                                || profile.dark_theme.is_some()
                                || profile.icon.is_some()
                                || profile.hidden
                        })
                        .map(ProfileForm::to_entry)
                        .collect(),
                ),
            );
        }
        let pane_templates = self.pane_templates.to_value()?;
        if pane_templates
            .as_object()
            .is_some_and(|templates| !templates.is_empty())
        {
            root.insert("pane_split_templates".into(), pane_templates);
        } else {
            root.remove("pane_split_templates");
        }
        serde_json::to_string_pretty(&Value::Object(root)).context("serializing configuration")
    }
}

#[cfg(test)]
#[path = "../tests/settings_editor/configuration.rs"]
mod tests;
