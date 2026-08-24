use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[cfg(windows)]
use std::{os::windows::process::CommandExt as _, process::Command};

use anyhow::{Context as _, Result, anyhow};
use serde_json::Value;
use task::Shell;
use terminal::MAX_SCROLL_HISTORY_LINES;
use ui::IconName;

use crate::pane::MAX_PANES_PER_TAB;
use crate::profile_icon::ProfileIcon;

pub(crate) const DEFAULT_TERMINAL_FONT_FAMILY: &str = "MesloLGS NF";
const DEFAULT_MAX_SCROLL_HISTORY_LINES: usize = MAX_SCROLL_HISTORY_LINES;
pub(crate) const DEFAULT_INACTIVE_PANE_OPACITY: f32 = 0.8;
pub(crate) const DEFAULT_HTTP_PORT: u16 = 8000;
pub(crate) const DEFAULT_TFTP_SERVER_PORT: u16 = 69;
pub(crate) const DEFAULT_SESSION_RING_BYTES: usize = zmux::retention::DEFAULT_RING_BYTES;
pub(crate) const MAX_SESSION_RING_BYTES: usize = zmux::retention::MAX_RING_BYTES;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionRetention {
    None,
    #[default]
    Memory,
    /// Encrypt detached-session state on disk when recipients are configured.
    Disk,
}

impl SessionRetention {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Memory => "memory",
            Self::Disk => "disk",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Memory => "Memory",
            Self::Disk => "Disk",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "memory" => Ok(Self::Memory),
            "disk" => Ok(Self::Disk),
            "persist" => {
                anyhow::bail!("sessions.retention=\"persist\" is no longer supported; use \"disk\"")
            }
            _ => anyhow::bail!("sessions.retention must be \"none\", \"memory\", or \"disk\""),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPersistenceConfig {
    pub recipients: Vec<String>,
    pub identity: Option<PathBuf>,
    /// Protect background sessions with a key sealed to `recipients` instead of
    /// asking for a secret.
    ///
    /// Only meaningful with both a recipient and an identity: the recipient is
    /// what a session key is sealed to, and the identity is the only thing that
    /// can open it again. Set without them it would mint sessions nobody can
    /// reattach, which is why nothing reads this flag on its own — see
    /// [`Self::auto_protect_is_configured`].
    pub auto_protect: bool,
}

/// Automatic protection is an age feature, so these have no callers in a build
/// without one; the configuration itself is still parsed and round-tripped, so
/// the fields stay.
#[cfg_attr(not(feature = "session-persistence"), allow(dead_code))]
impl SessionPersistenceConfig {
    /// Whether automatic protection is asked for *and* has the two things it
    /// needs. Deliberately free of I/O, because the settings page asks it while
    /// deciding what to draw; whether the identity file is actually there is
    /// settled once, when the recipients are resolved.
    pub fn auto_protect_is_configured(&self) -> bool {
        self.auto_protect && !self.recipients.is_empty() && self.identity.is_some()
    }

    /// The configured identity with a leading `~/` expanded, which is how it is
    /// written in configuration and how every reader of it has to resolve it.
    pub fn resolved_identity(&self) -> Option<PathBuf> {
        let path = self.identity.clone()?;
        let Some(relative) = path.to_str().and_then(|path| path.strip_prefix("~/")) else {
            return Some(path);
        };
        Some(util::paths::home_dir().join(relative))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionsConfig {
    pub retention: SessionRetention,
    pub ring_bytes: usize,
    pub persistence: SessionPersistenceConfig,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            retention: SessionRetention::default(),
            ring_bytes: DEFAULT_SESSION_RING_BYTES,
            persistence: SessionPersistenceConfig::default(),
        }
    }
}

impl SessionsConfig {
    pub(crate) fn to_zmux_retention(&self) -> Result<zmux::retention::Retention> {
        let retention = match self.retention {
            SessionRetention::None => zmux::retention::Retention::None,
            SessionRetention::Memory => zmux::retention::Retention::Memory {
                bytes: self.ring_bytes,
            },
            SessionRetention::Disk => zmux::retention::Retention::Disk,
        };
        retention.validate()?;
        Ok(retention)
    }

    #[cfg(feature = "session-persistence")]
    pub(crate) fn to_zmux_persistence(&self) -> zmux::persistence::PersistenceOptions {
        zmux::persistence::PersistenceOptions {
            recipients: self.persistence.recipients.clone(),
            identity: self.persistence.identity.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneControlsPosition {
    Left,
    #[default]
    Right,
}

impl PaneControlsPosition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewTabProfile {
    #[default]
    Default,
    Inherit,
}

impl NewTabProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Inherit => "inherit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Inherit => "Inherit",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkingDirectoryScope {
    None,
    Pane,
    #[default]
    Tab,
}

impl WorkingDirectoryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pane => "pane",
            Self::Tab => "tab",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Pane => "Pane",
            Self::Tab => "Tab",
        }
    }

    pub fn inherits_for_new_tab(self) -> bool {
        matches!(self, Self::Tab)
    }

    pub fn inherits_for_new_pane(self) -> bool {
        matches!(self, Self::Pane | Self::Tab)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneSplitTemplate {
    Pane(Box<PaneSplitPane>),
    Split {
        axis: PaneSplitAxis,
        first: Box<PaneSplitTemplate>,
        second: Box<PaneSplitTemplate>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSplitTemplateConfig {
    pub layout: PaneSplitTemplate,
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneSplitPane {
    pub label: Option<String>,
    pub profile: Option<Profile>,
    pub command: Option<PaneSplitCommand>,
    pub theme: Option<String>,
    pub dark_theme: Option<String>,
    pub env: HashMap<String, String>,
    pub overlay: Option<PaneSplitOverlay>,
    /// Commands seeded as stacked entries in this pane, sharing its layout
    /// region with the interactive shell.
    pub stack: Vec<PaneSplitCommand>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSplitCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl PaneSplitCommand {
    pub fn shell(&self) -> Shell {
        Shell::WithArguments {
            program: self.program.clone(),
            args: self.args.clone(),
            title_override: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSplitOverlay {
    pub text: Option<String>,
    pub size: Option<PaneSplitOverlaySize>,
    pub opacity: Option<u8>,
    pub color: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneSplitOverlaySize {
    Small,
    Base,
    Large,
    ExtraLarge,
    ExtraExtraLarge,
    ExtraExtraExtraLarge,
}

impl PaneSplitOverlaySize {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "sm" => Some(Self::Small),
            "base" => Some(Self::Base),
            "lg" => Some(Self::Large),
            "xl" => Some(Self::ExtraLarge),
            "2xl" => Some(Self::ExtraExtraLarge),
            "3xl" => Some(Self::ExtraExtraExtraLarge),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Small => "sm",
            Self::Base => "base",
            Self::Large => "lg",
            Self::ExtraLarge => "xl",
            Self::ExtraExtraLarge => "2xl",
            Self::ExtraExtraExtraLarge => "3xl",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Small => "Small (sm)",
            Self::Base => "Base",
            Self::Large => "Large (lg)",
            Self::ExtraLarge => "Extra large (xl)",
            Self::ExtraExtraLarge => "2x large (2xl)",
            Self::ExtraExtraExtraLarge => "3x large (3xl)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneSplitAxis {
    Horizontal,
    Vertical,
}

impl PaneSplitAxis {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Horizontal => "Horizontal",
            Self::Vertical => "Vertical",
        }
    }
}

impl PaneSplitTemplate {
    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    /// The stacked commands the whole template seeds. Each one becomes its own
    /// terminal, so it counts against the same budget as a layout pane.
    pub fn stacked_command_count(&self) -> usize {
        match self {
            Self::Pane(pane) => pane.stack.len(),
            Self::Split { first, second, .. } => {
                first.stacked_command_count() + second.stacked_command_count()
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pane_labels(&self) -> Vec<Option<String>> {
        let mut labels = Vec::with_capacity(self.pane_count());
        self.collect_pane_labels(&mut labels);
        labels
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn collect_pane_labels(&self, labels: &mut Vec<Option<String>>) {
        match self {
            Self::Pane(pane) => labels.push(pane.label.clone()),
            Self::Split { first, second, .. } => {
                first.collect_pane_labels(labels);
                second.collect_pane_labels(labels);
            }
        }
    }

    pub fn pane_specifications(&self) -> Vec<PaneSplitPane> {
        let mut panes = Vec::with_capacity(self.pane_count());
        self.collect_pane_specifications(&mut panes);
        panes
    }

    fn collect_pane_specifications(&self, panes: &mut Vec<PaneSplitPane>) {
        match self {
            Self::Pane(pane) => panes.push((**pane).clone()),
            Self::Split { first, second, .. } => {
                first.collect_pane_specifications(panes);
                second.collect_pane_specifications(panes);
            }
        }
    }
}

impl PaneSplitTemplateConfig {
    pub fn pane_count(&self) -> usize {
        self.layout.pane_count()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pane_labels(&self) -> Vec<Option<String>> {
        self.layout.pane_labels()
    }

    pub fn pane_specifications(&self) -> Vec<PaneSplitPane> {
        self.layout
            .pane_specifications()
            .into_iter()
            .map(|mut pane| {
                let pane_environment = std::mem::take(&mut pane.env);
                pane.env = self.env.clone();
                pane.env.extend(pane_environment);
                pane
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub command: Shell,
    pub theme: Option<String>,
    pub dark_theme: Option<String>,
    pub icon: ProfileIcon,
}

struct ProfileConfig {
    name: String,
    command: Option<Shell>,
    theme: Option<String>,
    dark_theme: Option<String>,
    icon: Option<ProfileIcon>,
    hidden: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub config_path: PathBuf,
    pub keymap_override: Option<PathBuf>,
    pub profiles: Vec<Profile>,
    pub(crate) hidden_profiles: HashSet<String>,
    pub default_profile: usize,
    pub new_tab_profile: NewTabProfile,
    pub working_directory: Option<PathBuf>,
    pub working_directory_configured: bool,
    pub working_directory_scope: WorkingDirectoryScope,
    pub keymap_path: PathBuf,
    pub theme: Option<String>,
    pub dark_theme: Option<String>,
    pub default_tab_icon: Option<IconName>,
    pub terminal_font_size: Option<f32>,
    pub terminal_font_family: String,
    pub max_scroll_history_lines: usize,
    pub inactive_pane_opacity: f32,
    pub compact_mode: bool,
    pub hide_pane_size: bool,
    pub hide_title_bar_labels: bool,
    pub hide_title_bar_buttons: bool,
    pub hide_title_bar_menus: bool,
    pub pane_controls_position: PaneControlsPosition,
    pub pane_controls_hidden_by_default: bool,
    pub http_server_port: u16,
    pub tftp_server_port: u16,
    pub sessions: SessionsConfig,
    pub pane_split_templates: HashMap<String, PaneSplitTemplateConfig>,
}

impl Config {
    pub fn defaults(config_path: Option<&Path>, keymap_path: Option<PathBuf>) -> Self {
        let config_dir = platform_config_dir();
        let config_path = config_path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| config_dir.join("config.json"));
        Self {
            config_path: config_path.clone(),
            keymap_override: keymap_path.clone(),
            profiles: discovered_profiles(),
            hidden_profiles: HashSet::new(),
            default_profile: 0,
            new_tab_profile: NewTabProfile::default(),
            working_directory: Some(home_dir()),
            working_directory_configured: false,
            working_directory_scope: WorkingDirectoryScope::default(),
            keymap_path: keymap_path.unwrap_or_else(|| config_dir.join("keymap.json")),
            theme: None,
            dark_theme: None,
            default_tab_icon: Some(IconName::Terminal),
            terminal_font_size: None,
            terminal_font_family: DEFAULT_TERMINAL_FONT_FAMILY.to_owned(),
            max_scroll_history_lines: DEFAULT_MAX_SCROLL_HISTORY_LINES,
            inactive_pane_opacity: DEFAULT_INACTIVE_PANE_OPACITY,
            compact_mode: false,
            hide_pane_size: true,
            hide_title_bar_labels: false,
            hide_title_bar_buttons: false,
            hide_title_bar_menus: cfg!(target_os = "macos"),
            pane_controls_position: PaneControlsPosition::default(),
            pane_controls_hidden_by_default: false,
            http_server_port: DEFAULT_HTTP_PORT,
            tftp_server_port: DEFAULT_TFTP_SERVER_PORT,
            sessions: SessionsConfig::default(),
            pane_split_templates: default_pane_split_templates(),
        }
    }

    pub fn load(config_path: Option<&Path>, keymap_path: Option<PathBuf>) -> Result<Self> {
        let config = Self::defaults(config_path, keymap_path.clone());

        let content = match fs::read_to_string(&config.config_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(config),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading {}", config.config_path.display()));
            }
        };
        Self::parse_into(&content, config)
    }

    /// Parses configuration text using the same defaults and path resolution as [`Self::load`].
    /// This lets the settings UI reject invalid edits before replacing the user's file.
    pub fn parse(
        content: &str,
        config_path: Option<&Path>,
        keymap_path: Option<PathBuf>,
    ) -> Result<Self> {
        Self::parse_into(content, Self::defaults(config_path, keymap_path))
    }

    /// Parses a configuration fragment on top of an already resolved base.
    ///
    /// Project configuration uses this after enforcing its smaller field set.
    /// Keeping the actual profile and pane-template parsing here means global
    /// and project configuration cannot drift apart in their validation rules.
    pub(crate) fn parse_overlay(content: &str, mut base: Self, config_path: &Path) -> Result<Self> {
        base.config_path = config_path.to_path_buf();
        Self::parse_into(content, base)
    }

    fn parse_into(content: &str, mut config: Self) -> Result<Self> {
        let root: Value = serde_json::from_str(content)
            .with_context(|| format!("parsing {}", config.config_path.display()))?;
        validate_config_fields(&root)?;

        if let Some(directory) = root.get("working_directory") {
            let directory = directory
                .as_str()
                .context("working_directory must be a string")?;
            config.working_directory = Some(expand_home(directory));
            // An explicit home alias is equivalent to omitting the setting.
            // This distinction matters for WSL: an actual override is a
            // Windows-side cwd, while the default must be passed as `--cd ~`
            // so WSL resolves the Linux user's home directory.
            config.working_directory_configured = !matches!(directory, "~" | "~/");
        }
        if let Some(scope) = root.get("working_directory_scope") {
            config.working_directory_scope = match scope
                .as_str()
                .context("working_directory_scope must be \"none\", \"pane\", or \"tab\"")?
            {
                "none" => WorkingDirectoryScope::None,
                "pane" => WorkingDirectoryScope::Pane,
                "tab" => WorkingDirectoryScope::Tab,
                _ => {
                    anyhow::bail!("working_directory_scope must be \"none\", \"pane\", or \"tab\"")
                }
            };
        }
        if let Some(profile) = root.get("new_tab_profile") {
            config.new_tab_profile = match profile
                .as_str()
                .context("new_tab_profile must be \"default\" or \"inherit\"")?
            {
                "default" => NewTabProfile::Default,
                "inherit" => NewTabProfile::Inherit,
                _ => anyhow::bail!("new_tab_profile must be \"default\" or \"inherit\""),
            };
        }
        if let Some(theme) = root.get("theme") {
            config.theme = Some(theme.as_str().context("theme must be a string")?.to_owned());
        }
        if let Some(dark_theme) = root.get("dark_theme") {
            config.dark_theme = Some(
                dark_theme
                    .as_str()
                    .context("dark_theme must be a string")?
                    .to_owned(),
            );
        }
        if let Some(icon) = root.get("default_tab_icon") {
            config.default_tab_icon = if icon.is_null() {
                None
            } else {
                let name = icon
                    .as_str()
                    .context("default_tab_icon must be an icon name or null")?;
                Some(name.parse().map_err(|_| {
                    anyhow!("default_tab_icon must be a built-in icon name, got {name:?}")
                })?)
            };
        }
        if let Some(font_size) = root.get("terminal_font_size") {
            let font_size = font_size
                .as_f64()
                .context("terminal_font_size must be a number")? as f32;
            anyhow::ensure!(
                (6.0..=100.0).contains(&font_size),
                "terminal_font_size must be between 6 and 100"
            );
            config.terminal_font_size = Some(font_size);
        }
        if let Some(font_family) = root.get("terminal_font_family") {
            config.terminal_font_family = font_family
                .as_str()
                .context("terminal_font_family must be a string")?
                .to_owned();
            anyhow::ensure!(
                !config.terminal_font_family.trim().is_empty(),
                "terminal_font_family must not be empty"
            );
        }
        if let Some(history_lines) = root.get("max_scroll_history_lines") {
            config.max_scroll_history_lines = parse_max_scroll_history_lines(history_lines)?;
        }
        if let Some(opacity) = root.get("inactive_pane_opacity") {
            config.inactive_pane_opacity = parse_inactive_pane_opacity(opacity)?;
        }
        if let Some(compact) = root.get("compact_mode") {
            config.compact_mode = compact
                .as_bool()
                .context("compact_mode must be a boolean")?;
        }
        if let Some(hidden) = root.get("hide_pane_size") {
            config.hide_pane_size = hidden
                .as_bool()
                .context("hide_pane_size must be a boolean")?;
        }
        if let Some(hidden) = root.get("hide_title_bar_labels") {
            config.hide_title_bar_labels = hidden
                .as_bool()
                .context("hide_title_bar_labels must be a boolean")?;
        }
        if let Some(hidden) = root.get("hide_title_bar_buttons") {
            config.hide_title_bar_buttons = hidden
                .as_bool()
                .context("hide_title_bar_buttons must be a boolean")?;
        }
        if let Some(hidden) = root.get("hide_title_bar_menus") {
            config.hide_title_bar_menus = hidden
                .as_bool()
                .context("hide_title_bar_menus must be a boolean")?;
        }
        if let Some(position) = root.get("pane_controls_position") {
            config.pane_controls_position = match position
                .as_str()
                .context("pane_controls_position must be \"left\" or \"right\"")?
            {
                "left" => PaneControlsPosition::Left,
                "right" => PaneControlsPosition::Right,
                _ => anyhow::bail!("pane_controls_position must be \"left\" or \"right\""),
            };
        }
        if let Some(hidden) = root.get("pane_controls_hidden_by_default") {
            config.pane_controls_hidden_by_default = hidden
                .as_bool()
                .context("pane_controls_hidden_by_default must be a boolean")?;
        }
        if let Some(port) = root.get("http_server_port") {
            config.http_server_port = parse_server_port(port, "http_server_port")?;
        }
        if let Some(port) = root.get("tftp_server_port") {
            config.tftp_server_port = parse_server_port(port, "tftp_server_port")?;
        }
        if let Some(sessions) = root.get("sessions") {
            let sessions = sessions.as_object().context("sessions must be an object")?;
            if let Some(field) = sessions
                .keys()
                .find(|field| !matches!(field.as_str(), "retention" | "ring_bytes" | "persistence"))
            {
                anyhow::bail!("unrecognized sessions configuration field {field:?}");
            }
            if let Some(retention) = sessions.get("retention") {
                config.sessions.retention = SessionRetention::parse(
                    retention
                        .as_str()
                        .context("sessions.retention must be a string")?,
                )?;
            }
            if let Some(ring_bytes) = sessions.get("ring_bytes") {
                let ring_bytes = ring_bytes
                    .as_u64()
                    .context("sessions.ring_bytes must be a positive integer")?;
                config.sessions.ring_bytes = usize::try_from(ring_bytes)
                    .ok()
                    .filter(|bytes| (4 * 1024..=MAX_SESSION_RING_BYTES).contains(bytes))
                    .with_context(|| {
                        format!(
                            "sessions.ring_bytes must be between 4096 and {MAX_SESSION_RING_BYTES} bytes"
                        )
                    })?;
            }
            if let Some(persistence) = sessions.get("persistence") {
                let persistence = persistence
                    .as_object()
                    .context("sessions.persistence must be an object")?;
                if let Some(field) = persistence.keys().find(|field| {
                    !matches!(field.as_str(), "recipients" | "identity" | "auto_protect")
                }) {
                    anyhow::bail!("unrecognized sessions.persistence field {field:?}");
                }
                if let Some(recipients) = persistence.get("recipients") {
                    config.sessions.persistence.recipients = recipients
                        .as_array()
                        .context("sessions.persistence.recipients must be an array")?
                        .iter()
                        .map(|recipient| {
                            recipient
                                .as_str()
                                .map(str::trim)
                                .filter(|recipient| !recipient.is_empty())
                                .map(str::to_owned)
                                .context("sessions.persistence.recipients must contain strings")
                        })
                        .collect::<Result<Vec<_>>>()?;
                }
                if let Some(identity) = persistence.get("identity") {
                    config.sessions.persistence.identity = match identity {
                        Value::Null => None,
                        Value::String(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
                        _ => anyhow::bail!(
                            "sessions.persistence.identity must be a path string or null"
                        ),
                    };
                }
                if let Some(auto_protect) = persistence.get("auto_protect") {
                    config.sessions.persistence.auto_protect = auto_protect
                        .as_bool()
                        .context("sessions.persistence.auto_protect must be a boolean")?;
                }
            }
            config.sessions.to_zmux_retention()?;
        }

        if let Some(profiles) = root.get("profiles") {
            let profiles = profiles.as_array().context("profiles must be an array")?;
            let parsed = profiles
                .iter()
                .map(parse_profile)
                .collect::<Result<Vec<_>>>()?;
            merge_profiles(&mut config.profiles, &parsed)?;
            merge_profile_visibility(&mut config.hidden_profiles, &parsed);
        }

        if let Some(templates) = root.get("pane_split_templates") {
            let templates = templates
                .as_object()
                .context("pane_split_templates must be an object")?;
            for (name, value) in templates {
                anyhow::ensure!(
                    !name.trim().is_empty(),
                    "pane split template names must not be empty"
                );
                let template = parse_pane_split_template_config(value, &config.profiles)
                    .with_context(|| format!("parsing pane split template {name:?}"))?;
                anyhow::ensure!(
                    (2..=64).contains(&template.pane_count()),
                    "pane split template {name:?} must contain between 2 and 64 panes"
                );
                config.pane_split_templates.insert(name.clone(), template);
            }
        }

        if let Some(default_profile) = root.get("default_profile") {
            let default_name = default_profile
                .as_str()
                .context("default_profile must be a string")?;
            config.default_profile = resolve_default_profile(&config.profiles, default_name)?;
        }

        Ok(config)
    }
}

fn validate_config_fields(root: &Value) -> Result<()> {
    const FIELDS: &[&str] = &[
        "default_profile",
        "new_tab_profile",
        "working_directory",
        "working_directory_scope",
        "theme",
        "dark_theme",
        "default_tab_icon",
        "terminal_font_size",
        "terminal_font_family",
        "max_scroll_history_lines",
        "inactive_pane_opacity",
        "compact_mode",
        "hide_pane_size",
        "hide_title_bar_labels",
        "hide_title_bar_buttons",
        "hide_title_bar_menus",
        "pane_controls_position",
        "pane_controls_hidden_by_default",
        "http_server_port",
        "tftp_server_port",
        "sessions",
        "pane_split_templates",
        "profiles",
    ];
    let object = root
        .as_object()
        .context("configuration root must be an object")?;
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized configuration field {field:?}");
    }
    Ok(())
}

fn parse_server_port(value: &Value, field: &str) -> Result<u16> {
    let message = || format!("{field} must be an integer from 1 to 65535");
    let port = value.as_u64().with_context(message)?;
    u16::try_from(port)
        .ok()
        .filter(|port| *port != 0)
        .with_context(message)
}

pub(crate) fn built_in_pane_split_templates() -> HashMap<String, PaneSplitTemplateConfig> {
    let labeled_pane = |label: &str| {
        Box::new(PaneSplitTemplate::Pane(Box::new(PaneSplitPane {
            label: Some(label.to_owned()),
            ..PaneSplitPane::default()
        })))
    };
    HashMap::from([
        (
            "three-right".to_owned(),
            PaneSplitTemplateConfig {
                env: HashMap::new(),
                layout: PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Vertical,
                    first: labeled_pane("left"),
                    second: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Horizontal,
                        first: labeled_pane("top-right"),
                        second: labeled_pane("bottom-right"),
                    }),
                },
            },
        ),
        (
            "three-left".to_owned(),
            PaneSplitTemplateConfig {
                env: HashMap::new(),
                layout: PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Vertical,
                    first: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Horizontal,
                        first: labeled_pane("top-left"),
                        second: labeled_pane("bottom-left"),
                    }),
                    second: labeled_pane("right"),
                },
            },
        ),
        (
            "quarters".to_owned(),
            PaneSplitTemplateConfig {
                env: HashMap::new(),
                layout: PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Vertical,
                    first: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Horizontal,
                        first: labeled_pane("top-left"),
                        second: labeled_pane("bottom-left"),
                    }),
                    second: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Horizontal,
                        first: labeled_pane("top-right"),
                        second: labeled_pane("bottom-right"),
                    }),
                },
            },
        ),
        (
            "four-vertical".to_owned(),
            PaneSplitTemplateConfig {
                env: HashMap::new(),
                layout: PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Vertical,
                    first: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Vertical,
                        first: labeled_pane("left"),
                        second: labeled_pane("left-center"),
                    }),
                    second: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Vertical,
                        first: labeled_pane("right-center"),
                        second: labeled_pane("right"),
                    }),
                },
            },
        ),
    ])
}

fn default_pane_split_templates() -> HashMap<String, PaneSplitTemplateConfig> {
    built_in_pane_split_templates()
}

fn parse_pane_split_template_config(
    value: &Value,
    profiles: &[Profile],
) -> Result<PaneSplitTemplateConfig> {
    let object = value
        .as_object()
        .context("pane split templates must be objects")?;
    const FIELDS: &[&str] = &["layout", "env"];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized pane split template field {field:?}");
    }
    let layout = object
        .get("layout")
        .context("pane split template layout is required")?;
    let env = object
        .get("env")
        .map(parse_pane_split_environment)
        .transpose()?
        .unwrap_or_default();
    let layout = parse_pane_split_template(layout, profiles)?;
    // Panes and stacked commands are both terminals, so stacked commands cannot
    // push a template past what a tab can hold. The pane count on its own is
    // validated by the caller, which owns the name for that error.
    let stacked_commands = layout.stacked_command_count();
    anyhow::ensure!(
        stacked_commands == 0 || layout.pane_count() + stacked_commands <= MAX_PANES_PER_TAB,
        "pane split templates must not declare more than {MAX_PANES_PER_TAB} panes and stacked commands combined"
    );
    Ok(PaneSplitTemplateConfig { layout, env })
}

fn parse_pane_split_template(value: &Value, profiles: &[Profile]) -> Result<PaneSplitTemplate> {
    let object = value
        .as_object()
        .context("template nodes must be a pane object or a split object")?;

    if let Some(axis_name) = object
        .keys()
        .find(|key| matches!(key.as_str(), "horizontal" | "vertical"))
    {
        anyhow::ensure!(
            object.len() == 1,
            "split objects must contain exactly one axis"
        );
        let axis = match axis_name.as_str() {
            "horizontal" => PaneSplitAxis::Horizontal,
            "vertical" => PaneSplitAxis::Vertical,
            _ => unreachable!(),
        };
        let children = object
            .get(axis_name)
            .expect("the selected split axis must be present")
            .as_array()
            .context("split children must be a two-element array")?;
        anyhow::ensure!(children.len() == 2, "splits must have exactly two children");
        return Ok(PaneSplitTemplate::Split {
            axis,
            first: Box::new(parse_pane_split_template(&children[0], profiles)?),
            second: Box::new(parse_pane_split_template(&children[1], profiles)?),
        });
    }

    parse_pane_split_pane(object, profiles).map(|pane| PaneSplitTemplate::Pane(Box::new(pane)))
}

fn parse_pane_split_pane(
    object: &serde_json::Map<String, Value>,
    profiles: &[Profile],
) -> Result<PaneSplitPane> {
    const FIELDS: &[&str] = &[
        "label",
        "profile",
        "command",
        "theme",
        "dark_theme",
        "env",
        "overlay",
        "stack",
    ];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized pane template field {field:?}");
    }

    let label = object
        .get("label")
        .map(|label| {
            let label = label
                .as_str()
                .context("pane template label must be a string")?;
            anyhow::ensure!(
                is_valid_pane_split_label(label),
                "pane labels must be lowercase kebab-case"
            );
            Ok(label.to_owned())
        })
        .transpose()?;

    let profile = object
        .get("profile")
        .map(|profile| {
            let name = profile
                .as_str()
                .context("pane template profile must be a string")?;
            profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(name))
                .cloned()
                .with_context(|| format!("pane template profile {name:?} is not available"))
        })
        .transpose()?;

    let command = object
        .get("command")
        .map(parse_pane_split_command)
        .transpose()?;
    anyhow::ensure!(
        profile.is_none() || command.is_none(),
        "pane template profile and command are mutually exclusive"
    );

    let theme = object
        .get("theme")
        .map(|theme| {
            let theme = theme
                .as_str()
                .context("pane template theme must be a string")?;
            anyhow::ensure!(!theme.is_empty(), "pane template theme must not be empty");
            Ok(theme.to_owned())
        })
        .transpose()?;

    let dark_theme = object
        .get("dark_theme")
        .map(|dark_theme| {
            let dark_theme = dark_theme
                .as_str()
                .context("pane template dark_theme must be a string")?;
            anyhow::ensure!(
                !dark_theme.is_empty(),
                "pane template dark_theme must not be empty"
            );
            Ok(dark_theme.to_owned())
        })
        .transpose()?;

    let env = object
        .get("env")
        .map(parse_pane_split_environment)
        .transpose()?
        .unwrap_or_default();

    let overlay = object
        .get("overlay")
        .map(parse_pane_split_overlay)
        .transpose()?;

    let stack = object
        .get("stack")
        .map(parse_pane_split_stack)
        .transpose()?
        .unwrap_or_default();

    Ok(PaneSplitPane {
        label,
        profile,
        command,
        theme,
        dark_theme,
        env,
        overlay,
        stack,
    })
}

fn parse_pane_split_stack(value: &Value) -> Result<Vec<PaneSplitCommand>> {
    let entries = value
        .as_array()
        .context("pane template stack must be an array of command objects")?;
    // A pane hosts its interactive shell plus its stacked entries, which is the
    // same limit `PaneStack::push` enforces at runtime.
    anyhow::ensure!(
        entries.len() < MAX_PANES_PER_TAB,
        "pane template stack must not contain more than {} commands",
        MAX_PANES_PER_TAB - 1
    );
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            parse_pane_split_command(entry)
                .with_context(|| format!("parsing pane template stack entry {}", index + 1))
        })
        .collect()
}

fn parse_pane_split_command(value: &Value) -> Result<PaneSplitCommand> {
    let object = value
        .as_object()
        .context("pane template command must be an object")?;
    const FIELDS: &[&str] = &["program", "args"];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized pane template command field {field:?}");
    }
    let program = object
        .get("program")
        .and_then(Value::as_str)
        .context("pane template command.program must be a string")?
        .to_owned();
    anyhow::ensure!(
        !program.is_empty(),
        "pane template command.program must not be empty"
    );
    let args = object
        .get("args")
        .map(|args| {
            args.as_array()
                .context("pane template command.args must be an array")?
                .iter()
                .map(|arg| {
                    arg.as_str()
                        .map(str::to_owned)
                        .context("pane template command arguments must be strings")
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(PaneSplitCommand { program, args })
}

fn parse_pane_split_environment(value: &Value) -> Result<HashMap<String, String>> {
    let object = value
        .as_object()
        .context("pane template env must be an object of strings")?;
    object
        .iter()
        .map(|(name, value)| {
            anyhow::ensure!(
                !name.is_empty() && !name.contains(['=', '\0']),
                "pane template environment variable names must not be empty or contain '='"
            );
            let value = value
                .as_str()
                .context("pane template environment values must be strings")?;
            anyhow::ensure!(
                !value.contains('\0'),
                "pane template environment values must not contain NUL"
            );
            Ok((name.clone(), value.to_owned()))
        })
        .collect()
}

fn parse_pane_split_overlay(value: &Value) -> Result<PaneSplitOverlay> {
    let object = value
        .as_object()
        .context("pane template overlay must be an object")?;
    const FIELDS: &[&str] = &["text", "size", "opacity", "color"];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized pane template overlay field {field:?}");
    }
    let text = object
        .get("text")
        .map(|text| {
            text.as_str()
                .map(str::to_owned)
                .context("pane template overlay.text must be a string")
        })
        .transpose()?;
    let size = object
        .get("size")
        .map(|size| {
            let size = size
                .as_str()
                .context("pane template overlay.size must be a string")?;
            PaneSplitOverlaySize::parse(size).with_context(
                || "pane template overlay.size must be one of sm, base, lg, xl, 2xl, or 3xl",
            )
        })
        .transpose()?;
    let opacity = object
        .get("opacity")
        .map(|opacity| {
            let opacity = opacity
                .as_u64()
                .context("pane template overlay.opacity must be an integer from 0 to 100")?;
            u8::try_from(opacity)
                .ok()
                .filter(|opacity| *opacity <= 100)
                .context("pane template overlay.opacity must be an integer from 0 to 100")
        })
        .transpose()?;
    let color = object
        .get("color")
        .map(|color| {
            let color = color
                .as_str()
                .context("pane template overlay.color must be a string")?;
            anyhow::ensure!(
                !color.trim().is_empty(),
                "pane template overlay.color must not be empty"
            );
            anyhow::ensure!(
                crate::pane::overlay_color_from_value(color).is_some(),
                "pane template overlay.color must be a named color or a valid hex color"
            );
            Ok(color.to_owned())
        })
        .transpose()?;
    Ok(PaneSplitOverlay {
        text,
        size,
        opacity,
        color,
    })
}

pub(crate) fn is_valid_pane_split_label(label: &str) -> bool {
    let mut segment_has_character = false;
    for byte in label.bytes() {
        if byte == b'-' {
            if !segment_has_character {
                return false;
            }
            segment_has_character = false;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            segment_has_character = true;
        } else {
            return false;
        }
    }
    segment_has_character
}

fn resolve_default_profile(profiles: &[Profile], name: &str) -> Result<usize> {
    profiles
        .iter()
        .position(|profile| profile.name.eq_ignore_ascii_case(name))
        .with_context(|| format!("default profile {name:?} is not available"))
}

fn merge_profiles(
    profiles: &mut Vec<Profile>,
    configured: impl AsRef<[ProfileConfig]>,
) -> Result<()> {
    let configured = configured.as_ref();
    for profile in configured {
        if let Some(index) = profiles
            .iter()
            .position(|existing| existing.name.eq_ignore_ascii_case(&profile.name))
        {
            if let Some(command) = profile.command.clone() {
                profiles[index].command = command;
            }
            if let Some(theme) = profile.theme.clone() {
                profiles[index].theme = Some(theme);
            }
            if let Some(dark_theme) = profile.dark_theme.clone() {
                profiles[index].dark_theme = Some(dark_theme);
            }
            profiles[index].icon = profile.icon.clone().unwrap_or_else(|| {
                ProfileIcon::automatic_for_profile(&profiles[index].name, &profiles[index].command)
            });
        } else {
            let command = profile.command.clone().with_context(|| {
                format!(
                    "profile {:?} must specify program because it was not detected",
                    profile.name
                )
            })?;
            profiles.push(Profile {
                name: profile.name.clone(),
                command: command.clone(),
                theme: profile.theme.clone(),
                dark_theme: profile.dark_theme.clone(),
                icon: profile
                    .icon
                    .clone()
                    .unwrap_or_else(|| ProfileIcon::automatic_for_profile(&profile.name, &command)),
            });
        }
    }
    Ok(())
}

fn profile_name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

fn merge_profile_visibility(hidden_profiles: &mut HashSet<String>, configured: &[ProfileConfig]) {
    for profile in configured {
        let Some(hidden) = profile.hidden else {
            continue;
        };
        let key = profile_name_key(&profile.name);
        if hidden {
            hidden_profiles.insert(key);
        } else {
            hidden_profiles.remove(&key);
        }
    }
}

pub(crate) fn profile_is_hidden(profile: &Profile, hidden_profiles: &HashSet<String>) -> bool {
    hidden_profiles.contains(&profile_name_key(&profile.name))
}

pub(crate) fn visible_profile_count(
    profiles: &[Profile],
    hidden_profiles: &HashSet<String>,
) -> usize {
    profiles
        .iter()
        .filter(|profile| !profile_is_hidden(profile, hidden_profiles))
        .count()
}

pub(crate) fn visible_profile_index(
    profiles: &[Profile],
    hidden_profiles: &HashSet<String>,
    slot: usize,
) -> Option<usize> {
    profiles
        .iter()
        .enumerate()
        .filter(|(_, profile)| !profile_is_hidden(profile, hidden_profiles))
        .nth(slot.checked_sub(1)?)
        .map(|(index, _)| index)
}

fn parse_inactive_pane_opacity(value: &Value) -> Result<f32> {
    let opacity = value
        .as_f64()
        .context("inactive_pane_opacity must be a number")?;
    anyhow::ensure!(
        (0.0..=1.0).contains(&opacity),
        "inactive_pane_opacity must be between 0 and 1"
    );
    Ok(opacity as f32)
}

fn parse_max_scroll_history_lines(value: &Value) -> Result<usize> {
    let history_lines = value
        .as_u64()
        .context("max_scroll_history_lines must be a non-negative integer")?;
    anyhow::ensure!(
        history_lines <= MAX_SCROLL_HISTORY_LINES as u64,
        "max_scroll_history_lines must not exceed {MAX_SCROLL_HISTORY_LINES}"
    );
    Ok(history_lines as usize)
}

fn parse_profile(value: &Value) -> Result<ProfileConfig> {
    let object = value
        .as_object()
        .context("each profile must be an object")?;
    const FIELDS: &[&str] = &[
        "name",
        "program",
        "args",
        "theme",
        "dark_theme",
        "icon",
        "hidden",
    ];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized profile field {field:?}");
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .context("profile.name must be a string")?
        .to_owned();
    let program = object
        .get("program")
        .map(|program| {
            program
                .as_str()
                .context("profile.program must be a string")
                .map(str::to_owned)
        })
        .transpose()?;
    let args = object
        .get("args")
        .map(|args| {
            args.as_array()
                .context("profile.args must be an array")?
                .iter()
                .map(|arg| {
                    arg.as_str()
                        .map(str::to_owned)
                        .context("profile args must be strings")
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    anyhow::ensure!(
        program.is_some() || args.is_empty(),
        "profile.args requires program"
    );
    let command = program.map(|program| {
        if args.is_empty() {
            Shell::Program(program)
        } else {
            Shell::WithArguments {
                program,
                args,
                title_override: Some(name.clone()),
            }
        }
    });
    let theme = object
        .get("theme")
        .map(|theme| {
            theme
                .as_str()
                .context("profile.theme must be a string")
                .map(str::to_owned)
        })
        .transpose()?;
    let dark_theme = object
        .get("dark_theme")
        .map(|dark_theme| {
            dark_theme
                .as_str()
                .context("profile.dark_theme must be a string")
                .map(str::to_owned)
        })
        .transpose()?;
    let icon = object
        .get("icon")
        .map(ProfileIcon::parse)
        .transpose()?
        .flatten();
    let hidden = object
        .get("hidden")
        .map(|hidden| hidden.as_bool().context("profile.hidden must be a boolean"))
        .transpose()?;
    Ok(ProfileConfig {
        name,
        command,
        theme,
        dark_theme,
        icon,
        hidden,
    })
}

fn discovered_profiles() -> Vec<Profile> {
    static DISCOVERED_PROFILES: OnceLock<Vec<Profile>> = OnceLock::new();
    DISCOVERED_PROFILES.get_or_init(discover_profiles).clone()
}

fn discover_profiles() -> Vec<Profile> {
    let mut profiles = vec![Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::automatic_for_shell(&Shell::System),
    }];
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let homebrew_prefixes = homebrew_prefixes();
    let candidates: &[(&str, &str)] = if cfg!(windows) {
        &[
            ("PowerShell", "powershell.exe"),
            ("PowerShell 7", "pwsh.exe"),
            ("Command Prompt", "cmd.exe"),
        ]
    } else {
        &[
            ("Zsh", "zsh"),
            ("Bash", "bash"),
            ("Fish", "fish"),
            ("Nushell", "nu"),
        ]
    };
    let mut seen = HashSet::new();
    for (name, program) in candidates {
        if let Some(path) = command_path(program)
            && seen.insert(path.clone())
        {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let profile =
                homebrew_profile_for_path(&path, &homebrew_prefixes).unwrap_or_else(|| Profile {
                    name: (*name).to_owned(),
                    command: Shell::Program((*program).to_owned()),
                    theme: None,
                    dark_theme: None,
                    icon: ProfileIcon::automatic_for_program(program),
                });
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let profile = Profile {
                name: (*name).to_owned(),
                command: Shell::Program((*program).to_owned()),
                theme: None,
                dark_theme: None,
                icon: ProfileIcon::automatic_for_program(program),
            };
            profiles.push(profile);
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    profiles.extend(
        homebrew_shell_profiles(homebrew_prefixes)
            .into_iter()
            .filter(|profile| {
                let Shell::Program(program) = &profile.command else {
                    return true;
                };
                seen.insert(PathBuf::from(program))
            }),
    );
    #[cfg(windows)]
    if let Some(root) = msys2_installation_root() {
        profiles.extend(msys2_profiles(&root));
    }
    #[cfg(windows)]
    if let Some(program) = wsl_program() {
        profiles.extend(discovered_wsl_profiles(&program));
    }
    profiles
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
const POSIX_PROFILE_CANDIDATES: &[(&str, &str)] = &[
    ("Zsh", "zsh"),
    ("Bash", "bash"),
    ("Fish", "fish"),
    ("Nushell", "nu"),
];

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn homebrew_prefixes() -> Vec<PathBuf> {
    let mut prefixes = env::var_os("HOMEBREW_PREFIX")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    #[cfg(target_os = "macos")]
    prefixes.extend([PathBuf::from("/opt/homebrew"), PathBuf::from("/usr/local")]);
    #[cfg(target_os = "linux")]
    prefixes.push(PathBuf::from("/home/linuxbrew/.linuxbrew"));

    prefixes.dedup();
    prefixes
        .into_iter()
        .filter(|prefix| prefix.join("bin/brew").is_file())
        .collect()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn homebrew_profile_for_path(path: &Path, prefixes: &[PathBuf]) -> Option<Profile> {
    POSIX_PROFILE_CANDIDATES.iter().find_map(|(name, program)| {
        prefixes.iter().find_map(|prefix| {
            let homebrew_path = prefix.join("bin").join(program);
            (homebrew_path == path).then(|| homebrew_profile(name, homebrew_path))
        })
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn homebrew_profile(name: &str, path: PathBuf) -> Profile {
    let command = path.to_string_lossy().into_owned();
    Profile {
        name: format!("{name} (Homebrew)"),
        command: Shell::Program(command.clone()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::automatic_for_program(&command),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn homebrew_shell_profiles(prefixes: impl IntoIterator<Item = PathBuf>) -> Vec<Profile> {
    prefixes
        .into_iter()
        .flat_map(|prefix| {
            let bin = prefix.join("bin");
            POSIX_PROFILE_CANDIDATES
                .iter()
                .filter_map(move |(name, program)| {
                    let path = bin.join(program);
                    path.is_file().then(|| homebrew_profile(name, path))
                })
        })
        .collect()
}

#[cfg(windows)]
fn msys2_installation_root() -> Option<PathBuf> {
    msys2_start_menu_installation_roots()
        .into_iter()
        .chain(msys2_registered_installation_roots())
        .chain([PathBuf::from(r"C:\msys64")])
        .find(|root| root.join("msys2_shell.cmd").is_file())
}

#[cfg(windows)]
fn msys2_start_menu_installation_roots() -> Vec<PathBuf> {
    use windows::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
    };

    let shortcuts = [
        env::var_os("APPDATA").map(PathBuf::from),
        env::var_os("ProgramData").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    .map(|start_menu| {
        start_menu
            .join("Microsoft/Windows/Start Menu/Programs/MSYS2")
            .join("MSYS2 MSYS.lnk")
    })
    .filter(|shortcut| shortcut.is_file())
    .collect::<Vec<_>>();
    if shortcuts.is_empty() {
        return Vec::new();
    }

    // SAFETY: COM is initialized for this thread before creating Shell Link objects,
    // and is uninitialized only when this call initialized it successfully.
    unsafe {
        let result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let initialized = if result.is_ok() {
            true
        } else if result == RPC_E_CHANGED_MODE {
            false
        } else {
            return Vec::new();
        };
        let roots = shortcuts
            .into_iter()
            .filter_map(|shortcut| shortcut_working_directory(&shortcut))
            .collect();
        if initialized {
            CoUninitialize();
        }
        roots
    }
}

#[cfg(windows)]
fn shortcut_working_directory(shortcut: &Path) -> Option<PathBuf> {
    use windows::{
        Win32::{
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ},
            UI::Shell::{IShellLinkW, ShellLink},
        },
        core::{HSTRING, Interface},
    };

    // SAFETY: The caller initializes COM before this helper is used. All output
    // buffers remain alive for the duration of the corresponding COM calls.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist
            .Load(&HSTRING::from(shortcut.as_os_str()), STGM_READ)
            .ok()?;
        let mut directory = vec![0_u16; 32_768];
        link.GetWorkingDirectory(&mut directory).ok()?;
        let len = directory.iter().position(|character| *character == 0)?;
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&directory[..len])))
    }
}

#[cfg(windows)]
fn msys2_registered_installation_roots() -> Vec<PathBuf> {
    use windows::{
        Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegEnumKeyW, RegOpenKeyExW,
            },
        },
        core::w,
    };

    fn read_string(key: HKEY, name: windows::core::PCWSTR) -> Option<String> {
        use windows::Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{REG_SZ, RegQueryValueExW},
        };

        let mut value_type = REG_SZ;
        let mut byte_len = 0;
        // SAFETY: The first query requests only the value size and writes to valid local
        // variables. The second query uses a buffer sized from that result.
        if unsafe {
            RegQueryValueExW(
                key,
                name,
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_len),
            )
        } != ERROR_SUCCESS
            || value_type != REG_SZ
            || byte_len < 2
        {
            return None;
        }
        let mut value = vec![0_u16; byte_len as usize / 2];
        if unsafe {
            RegQueryValueExW(
                key,
                name,
                None,
                Some(&mut value_type),
                Some(value.as_mut_ptr().cast()),
                Some(&mut byte_len),
            )
        } != ERROR_SUCCESS
        {
            return None;
        }
        let len = value.iter().position(|character| *character == 0)?;
        Some(String::from_utf16_lossy(&value[..len]))
    }

    let mut uninstall = HKEY::default();
    // The official MSYS2 installer registers its per-user uninstall entry here.
    // SAFETY: `uninstall` is a valid output pointer and is closed below.
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
            None,
            KEY_READ,
            &mut uninstall,
        )
    } != ERROR_SUCCESS
    {
        return Vec::new();
    }

    let mut roots = Vec::new();
    for index in 0.. {
        let mut name = [0_u16; 256];
        // RegEnumKeyW writes at most the supplied buffer length.
        let result = unsafe { RegEnumKeyW(uninstall, index, Some(&mut name)) };
        if result != ERROR_SUCCESS {
            break;
        }
        if !name.contains(&0) {
            continue;
        }
        let mut entry = HKEY::default();
        // SAFETY: The enumerated name is terminated in the local buffer, and `entry`
        // is a valid output pointer.
        if unsafe {
            RegOpenKeyExW(
                uninstall,
                windows::core::PCWSTR(name.as_ptr()),
                None,
                KEY_READ,
                &mut entry,
            )
        } != ERROR_SUCCESS
        {
            continue;
        }
        let is_msys2 = read_string(entry, w!("DisplayName")).is_some_and(|display_name| {
            display_name.eq_ignore_ascii_case("MSYS2")
                || display_name.to_ascii_lowercase().starts_with("msys2 ")
        });
        if is_msys2 && let Some(location) = read_string(entry, w!("InstallLocation")) {
            roots.push(PathBuf::from(location));
        }
        // SAFETY: `entry` was opened successfully in this iteration.
        unsafe {
            let _ = RegCloseKey(entry);
        }
    }
    // SAFETY: `uninstall` was opened successfully above.
    unsafe {
        let _ = RegCloseKey(uninstall);
    }
    roots
}

#[cfg(any(windows, test))]
fn msys2_profiles(root: &Path) -> Vec<Profile> {
    let launcher = root.join("msys2_shell.cmd");
    [("MSYS2", "bash"), ("MSYS2: Zsh", "zsh")]
        .into_iter()
        .filter(|(_, shell)| root.join("usr/bin").join(format!("{shell}.exe")).is_file())
        .map(|(name, shell)| {
            let command = format!(
                "\"\"{}\" -defterm -here -no-start -msys -use-full-path -shell {shell}\"",
                launcher.display()
            );
            Profile {
                name: name.to_owned(),
                command: Shell::WithArguments {
                    program: "cmd.exe".to_owned(),
                    args: vec!["/d".to_owned(), "/s".to_owned(), "/c".to_owned(), command],
                    title_override: Some(name.to_owned()),
                },
                theme: None,
                dark_theme: None,
                icon: match shell {
                    "bash" => ProfileIcon::Bash,
                    "zsh" => ProfileIcon::Zsh,
                    _ => ProfileIcon::Zetta,
                },
            }
        })
        .collect()
}

#[cfg(windows)]
fn wsl_program() -> Option<String> {
    let system_root = env::var_os("SystemRoot").or_else(|| env::var_os("WINDIR"));
    let system_wsl = system_root.map(PathBuf::from).map(|root| {
        root.join("System32")
            .join("wsl.exe")
            .to_string_lossy()
            .into_owned()
    });

    system_wsl
        .filter(|program| Path::new(program).is_file())
        .or_else(|| command_exists("wsl.exe").then(|| "wsl.exe".to_owned()))
}

#[cfg(windows)]
fn discovered_wsl_profiles(program: &str) -> Vec<Profile> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let output = Command::new(program)
        .args(["--list", "--quiet"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    wsl_profiles_from_output(program, &output.stdout)
}

#[cfg(any(windows, test))]
fn wsl_profiles_from_output(program: &str, output: &[u8]) -> Vec<Profile> {
    parse_wsl_distribution_names(output)
        .into_iter()
        .map(|distribution| {
            let name = format!("WSL: {distribution}");
            Profile {
                name: name.clone(),
                command: Shell::WithArguments {
                    program: program.to_owned(),
                    args: vec!["--distribution".to_owned(), distribution],
                    title_override: Some(name),
                },
                theme: None,
                dark_theme: None,
                icon: ProfileIcon::Tux,
            }
        })
        .collect()
}

#[cfg(any(windows, test))]
fn parse_wsl_distribution_names(output: &[u8]) -> Vec<String> {
    let decoded = if let Some(bytes) = output.strip_prefix(&[0xfe, 0xff]) {
        let code_units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&code_units)
    } else if output.starts_with(&[0xff, 0xfe]) || output.contains(&0) {
        let bytes = output.strip_prefix(&[0xff, 0xfe]).unwrap_or(output);
        let code_units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&code_units)
    } else {
        String::from_utf8_lossy(output).into_owned()
    };

    let mut seen = HashSet::new();
    decoded
        .lines()
        .map(|line| line.trim_matches(['\0', '\u{feff}', ' ', '\t', '\r']))
        .filter(|name| !name.is_empty())
        .filter(|name| is_user_wsl_distribution(name))
        .filter(|name| seen.insert(name.to_lowercase()))
        .map(str::to_owned)
        .collect()
}

#[cfg(any(windows, test))]
fn is_user_wsl_distribution(name: &str) -> bool {
    ![
        "docker-desktop",
        "docker-desktop-data",
        "rancher-desktop",
        "rancher-desktop-data",
    ]
    .iter()
    .any(|service| name.eq_ignore_ascii_case(service))
}

fn command_path(program: &str) -> Option<PathBuf> {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.is_file().then(|| program_path.to_path_buf());
    }
    env::var_os("PATH").and_then(|path| command_path_in(program, &path))
}

fn command_path_in(program: &str, path: &OsStr) -> Option<PathBuf> {
    env::split_paths(path).find_map(|directory| {
        if cfg!(windows) {
            if directory.join(program).is_file() {
                Some(directory.join(program))
            } else if !program.to_ascii_lowercase().ends_with(".exe")
                && directory.join(format!("{program}.exe")).is_file()
            {
                Some(directory.join(format!("{program}.exe")))
            } else {
                None
            }
        } else {
            directory
                .join(program)
                .is_file()
                .then(|| directory.join(program))
        }
    })
}

#[cfg(windows)]
fn command_exists(program: &str) -> bool {
    command_path(program).is_some()
}

/// Resolved by `zmux`, which needs the same directory without depending on
/// this module: it holds the session catalogs and the control endpoint.
pub(crate) fn platform_config_dir() -> PathBuf {
    zmux::paths::platform_config_dir()
}

pub fn themes_dir() -> PathBuf {
    platform_config_dir().join("themes")
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        home_dir()
    } else if let Some(relative) = path.strip_prefix("~/") {
        home_dir().join(relative)
    } else {
        PathBuf::from(path)
    }
}

fn home_dir() -> PathBuf {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
