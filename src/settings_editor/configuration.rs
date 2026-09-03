//! The typed form behind the Configuration page, and its serialization.
//!
//! The form is built over the file's parsed root object and written back into
//! it, so keys Zetta does not know about survive a round trip. Values equal to
//! the default are stripped on write rather than spelled out, so the file
//! stays a list of what the user actually chose.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigTextField {
    WorkingDirectory,
    FontSize,
    ScrollHistory,
    SessionRingBytes,
    #[cfg(feature = "session-persistence")]
    SessionPersistenceRecipients,
    #[cfg(feature = "session-persistence")]
    SessionPersistenceIdentity,
    #[cfg(feature = "http-server")]
    HttpServerPort,
    #[cfg(feature = "tftp-server")]
    TftpServerPort,
    ProfileName(usize),
    ProfileProgram(usize),
    ProfileArguments(usize),
}

#[derive(Clone, Debug)]
pub struct ProfileForm {
    pub name: TextField,
    pub program: TextField,
    pub arguments: TextField,
    pub theme: Option<String>,
    pub dark_theme: Option<String>,
    /// An explicit configuration override. None means automatic inference.
    pub icon: Option<ProfileIcon>,
    pub automatic_icon: ProfileIcon,
    pub hidden: bool,
    pub detected: bool,
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
    pub session_persistence_recipients: TextField,
    pub session_persistence_identity: TextField,
    /// Whether background sessions are protected with the configured age key
    /// instead of a secret typed into a dialog. Only meaningful, and only
    /// offered, alongside a recipient and an effective identity.
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
        let root = read_json_or(path, json!({}))?
            .as_object()
            .context("configuration root must be an object")?
            .clone();
        let string = |name: &str| root.get(name).and_then(Value::as_str).map(str::to_owned);
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
                    arguments: TextField::new(
                        configured
                            .and_then(|profile| profile.get("args"))
                            .and_then(Value::as_array)
                            .map(|args| {
                                args.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
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
        let session_ring_bytes = root
            .get("sessions")
            .and_then(Value::as_object)
            .and_then(|sessions| sessions.get("ring_bytes"))
            .and_then(Value::as_u64)
            .and_then(|bytes| usize::try_from(bytes).ok())
            .unwrap_or(config.sessions.ring_bytes);
        let session_persistence_recipients = root
            .get("sessions")
            .and_then(Value::as_object)
            .and_then(|sessions| sessions.get("persistence"))
            .and_then(Value::as_object)
            .and_then(|persistence| persistence.get("recipients"))
            .and_then(Value::as_array)
            .map(|recipients| {
                recipients
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| config.sessions.persistence.recipients.join(", "));
        let session_persistence_identity = root
            .get("sessions")
            .and_then(Value::as_object)
            .and_then(|sessions| sessions.get("persistence"))
            .and_then(Value::as_object)
            .and_then(|persistence| persistence.get("identity"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                config
                    .sessions
                    .persistence
                    .identity
                    .as_deref()
                    .and_then(Path::to_str)
                    .unwrap_or_default()
            });
        Ok(Self {
            default_profile: config.profiles[config.default_profile].name.clone(),
            new_tab_profile: config.new_tab_profile,
            working_directory: TextField::new(
                string("working_directory").unwrap_or_else(|| "~".to_owned()),
            ),
            working_directory_scope: config.working_directory_scope,
            theme: config
                .theme
                .clone()
                .unwrap_or_else(|| crate::ZETTA_DEFAULT_THEME.to_owned()),
            dark_theme: config
                .dark_theme
                .clone()
                .unwrap_or_else(|| crate::ZETTA_DEFAULT_DARK_THEME.to_owned()),
            default_tab_icon: config.default_tab_icon,
            terminal_font_size: TextField::new(
                config.terminal_font_size.unwrap_or(14.).to_string(),
            ),
            terminal_font_family: config.terminal_font_family.clone(),
            max_scroll_history_lines: TextField::new(
                if config.max_scroll_history_lines == terminal::MAX_SCROLL_HISTORY_LINES {
                    "Max".to_owned()
                } else {
                    config.max_scroll_history_lines.to_string()
                },
            ),
            inactive_pane_opacity: config.inactive_pane_opacity,
            compact_mode: config.compact_mode,
            hide_pane_size: config.hide_pane_size,
            hide_title_bar_labels: config.hide_title_bar_labels,
            hide_title_bar_buttons: config.hide_title_bar_buttons,
            #[cfg(target_os = "macos")]
            hide_title_bar_menus: config.hide_title_bar_menus,
            pane_controls_position: config.pane_controls_position,
            pane_controls_hidden_by_default: config.pane_controls_hidden_by_default,
            session_retention: config.sessions.retention,
            session_ring_bytes: TextField::new(session_ring_bytes.to_string()),
            session_persistence_recipients: TextField::new(session_persistence_recipients),
            session_persistence_identity: TextField::new(session_persistence_identity),
            session_persistence_auto_protect: config.sessions.persistence.auto_protect,
            #[cfg(feature = "http-server")]
            http_server_port: TextField::new(config.http_server_port.to_string()),
            #[cfg(feature = "tftp-server")]
            tftp_server_port: TextField::new(config.tftp_server_port.to_string()),
            root,
            profiles,
            pane_templates,
        })
    }

    pub fn text_mut(&mut self, field: ConfigTextField) -> Option<&mut TextField> {
        match field {
            ConfigTextField::WorkingDirectory => Some(&mut self.working_directory),
            ConfigTextField::FontSize => Some(&mut self.terminal_font_size),
            ConfigTextField::ScrollHistory => Some(&mut self.max_scroll_history_lines),
            ConfigTextField::SessionRingBytes => Some(&mut self.session_ring_bytes),
            #[cfg(feature = "session-persistence")]
            ConfigTextField::SessionPersistenceRecipients => {
                Some(&mut self.session_persistence_recipients)
            }
            #[cfg(feature = "session-persistence")]
            ConfigTextField::SessionPersistenceIdentity => {
                Some(&mut self.session_persistence_identity)
            }
            #[cfg(feature = "http-server")]
            ConfigTextField::HttpServerPort => Some(&mut self.http_server_port),
            #[cfg(feature = "tftp-server")]
            ConfigTextField::TftpServerPort => Some(&mut self.tftp_server_port),
            ConfigTextField::ProfileName(index) => {
                self.profiles.get_mut(index).map(|p| &mut p.name)
            }
            ConfigTextField::ProfileProgram(index) => {
                self.profiles.get_mut(index).map(|p| &mut p.program)
            }
            ConfigTextField::ProfileArguments(index) => {
                self.profiles.get_mut(index).map(|p| &mut p.arguments)
            }
        }
    }

    pub fn to_json(&self) -> Result<String> {
        let mut root = self.root.clone();
        root.insert("default_profile".into(), json!(self.default_profile));
        root.insert(
            "new_tab_profile".into(),
            json!(self.new_tab_profile.as_str()),
        );
        root.insert(
            "working_directory".into(),
            json!(self.working_directory.text),
        );
        root.insert(
            "working_directory_scope".into(),
            json!(self.working_directory_scope.as_str()),
        );
        root.insert("theme".into(), json!(self.theme));
        root.insert("dark_theme".into(), json!(self.dark_theme));
        let default_tab_icon = self.default_tab_icon.map(|icon| {
            let name: &'static str = icon.into();
            Value::String(name.to_owned())
        });
        root.insert(
            "default_tab_icon".into(),
            default_tab_icon.unwrap_or(Value::Null),
        );
        let terminal_font_size = self
            .terminal_font_size
            .text
            .trim()
            .parse::<f32>()
            .context("terminal font size must be a number")?;
        root.insert("terminal_font_size".into(), json!(terminal_font_size));
        root.insert(
            "terminal_font_family".into(),
            json!(self.terminal_font_family),
        );
        let scroll_history = if self
            .max_scroll_history_lines
            .text
            .trim()
            .eq_ignore_ascii_case("max")
        {
            terminal::MAX_SCROLL_HISTORY_LINES as u64
        } else {
            self.max_scroll_history_lines
                .text
                .trim()
                .parse::<u64>()
                .context("scrollback history must be a non-negative integer or Max")?
        };
        root.insert("max_scroll_history_lines".into(), json!(scroll_history));
        let inactive_pane_opacity = format!("{:.2}", self.inactive_pane_opacity)
            .parse::<f64>()
            .context("formatting inactive pane opacity")?;
        root.insert("inactive_pane_opacity".into(), json!(inactive_pane_opacity));
        root.insert("compact_mode".into(), json!(self.compact_mode));
        root.insert("hide_pane_size".into(), json!(self.hide_pane_size));
        root.insert(
            "hide_title_bar_labels".into(),
            json!(self.hide_title_bar_labels),
        );
        root.insert(
            "hide_title_bar_buttons".into(),
            json!(self.hide_title_bar_buttons),
        );
        #[cfg(target_os = "macos")]
        root.insert(
            "hide_title_bar_menus".into(),
            json!(self.hide_title_bar_menus),
        );
        root.insert(
            "pane_controls_position".into(),
            json!(self.pane_controls_position.as_str()),
        );
        root.insert(
            "pane_controls_hidden_by_default".into(),
            json!(self.pane_controls_hidden_by_default),
        );
        let session_ring_bytes = self
            .session_ring_bytes
            .text
            .trim()
            .parse::<usize>()
            .context("session ring bytes must be a positive integer")?;
        anyhow::ensure!(
            (4 * 1024..=crate::config::MAX_SESSION_RING_BYTES).contains(&session_ring_bytes),
            "session ring bytes must be between 4096 and {}",
            crate::config::MAX_SESSION_RING_BYTES
        );
        let recipients = self
            .session_persistence_recipients
            .text
            .split(',')
            .map(str::trim)
            .filter(|recipient| !recipient.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let identity = self.session_persistence_identity.text.trim();
        root.insert(
            "sessions".into(),
            json!({
                "retention": self.session_retention.as_str(),
                "ring_bytes": session_ring_bytes,
                "persistence": {
                    "recipients": recipients,
                    "identity": (!identity.is_empty()).then_some(identity),
                    "auto_protect": self.session_persistence_auto_protect,
                },
            }),
        );
        #[cfg(feature = "http-server")]
        {
            let http_server_port = self
                .http_server_port
                .text
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .context("HTTP server port must be an integer from 1 to 65535")?;
            root.insert("http_server_port".into(), json!(http_server_port));
        }
        #[cfg(feature = "tftp-server")]
        {
            let tftp_server_port = self
                .tftp_server_port
                .text
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .context("TFTP server port must be an integer from 1 to 65535")?;
            root.insert("tftp_server_port".into(), json!(tftp_server_port));
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
                        .map(|profile| {
                            let mut value = Map::new();
                            value.insert("name".into(), json!(profile.name.text));
                            if !profile.program.text.trim().is_empty() {
                                value.insert("program".into(), json!(profile.program.text));
                                value.insert(
                                    "args".into(),
                                    Value::Array(
                                        profile
                                            .arguments
                                            .text
                                            .split(',')
                                            .map(str::trim)
                                            .filter(|arg| !arg.is_empty())
                                            .map(|arg| json!(arg))
                                            .collect(),
                                    ),
                                );
                            }
                            if let Some(theme) = &profile.theme {
                                value.insert("theme".into(), json!(theme));
                            }
                            if let Some(dark_theme) = &profile.dark_theme {
                                value.insert("dark_theme".into(), json!(dark_theme));
                            }
                            if let Some(icon) = &profile.icon
                                && let Some(name) = icon.name()
                            {
                                value.insert("icon".into(), json!(name));
                            }
                            if profile.hidden {
                                value.insert("hidden".into(), json!(true));
                            }
                            Value::Object(value)
                        })
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
        strip_default_configuration_values(&mut root, &self.profiles, &self.working_directory);
        serde_json::to_string_pretty(&Value::Object(root)).context("serializing configuration")
    }
}

fn strip_matching_defaults(root: &mut Map<String, Value>, defaults: &[(&str, Value)]) {
    for (key, default) in defaults {
        if root.get(*key) == Some(default) {
            root.remove(*key);
        }
    }
}

fn strip_default_configuration_values(
    root: &mut Map<String, Value>,
    profiles: &[ProfileForm],
    working_directory: &TextField,
) {
    #[allow(
        unused_mut,
        reason = "the platform- and feature-gated pushes below are the only mutations"
    )]
    let mut defaults: Vec<(&str, Value)> = vec![
        ("new_tab_profile", json!(NewTabProfile::default().as_str())),
        (
            "working_directory_scope",
            json!(WorkingDirectoryScope::default().as_str()),
        ),
        ("theme", json!(crate::ZETTA_DEFAULT_THEME)),
        ("dark_theme", json!(crate::ZETTA_DEFAULT_DARK_THEME)),
        ("default_tab_icon", json!("terminal")),
        (
            "terminal_font_family",
            json!(crate::config::DEFAULT_TERMINAL_FONT_FAMILY),
        ),
        (
            "max_scroll_history_lines",
            json!(terminal::MAX_SCROLL_HISTORY_LINES as u64),
        ),
        (
            "inactive_pane_opacity",
            json!(
                format!("{:.2}", crate::config::DEFAULT_INACTIVE_PANE_OPACITY)
                    .parse::<f64>()
                    .unwrap()
            ),
        ),
        ("compact_mode", json!(false)),
        ("hide_pane_size", json!(true)),
        ("hide_title_bar_labels", json!(false)),
        ("hide_title_bar_buttons", json!(false)),
        (
            "pane_controls_position",
            json!(PaneControlsPosition::default().as_str()),
        ),
        ("pane_controls_hidden_by_default", json!(false)),
        (
            "sessions",
            json!({
                "retention": SessionRetention::default().as_str(),
                "ring_bytes": crate::config::DEFAULT_SESSION_RING_BYTES,
                "persistence": {
                    "recipients": [],
                    "identity": null,
                    "auto_protect": false,
                },
            }),
        ),
    ];
    #[cfg(target_os = "macos")]
    defaults.push(("hide_title_bar_menus", json!(true)));
    #[cfg(feature = "http-server")]
    defaults.push(("http_server_port", json!(crate::config::DEFAULT_HTTP_PORT)));
    #[cfg(feature = "tftp-server")]
    defaults.push((
        "tftp_server_port",
        json!(crate::config::DEFAULT_TFTP_SERVER_PORT),
    ));
    strip_matching_defaults(root, &defaults);

    if root
        .get("default_profile")
        .and_then(Value::as_str)
        .zip(profiles.first())
        .is_some_and(|(name, first)| name.eq_ignore_ascii_case(&first.name.text))
    {
        root.remove("default_profile");
    }
    if matches!(working_directory.text.trim(), "~" | "~/") {
        root.remove("working_directory");
    }
}

#[cfg(test)]
#[path = "../tests/settings_editor/configuration.rs"]
mod tests;
