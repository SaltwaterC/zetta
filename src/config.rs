use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use task::Shell;
use terminal::MAX_SCROLL_HISTORY_LINES;
use ui::IconName;

use crate::pane::MAX_PANES_PER_TAB;
use crate::profile_icon::ProfileIcon;

mod discovery;

use discovery::discovered_profiles;

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
    /// needs. A missing identity field may resolve to the conventional SSH
    /// identity, but only when that file exists.
    pub fn auto_protect_is_configured(&self) -> bool {
        self.auto_protect && !self.recipients.is_empty() && self.resolved_identity().is_some()
    }

    /// The effective identity with a leading `~/` expanded.
    ///
    /// When the setting is absent or blank, an existing `~/.ssh/id_ed25519` is
    /// used as the default age identity. An explicitly configured path wins,
    /// even when that path does not exist, so callers can report its useful
    /// name in an error.
    pub fn resolved_identity(&self) -> Option<PathBuf> {
        let path = self
            .identity
            .clone()
            .or_else(default_session_identity_path)?;
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

    fn parse(value: &str) -> Result<Self> {
        match value {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            _ => anyhow::bail!("pane_controls_position must be \"left\" or \"right\""),
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

    fn parse(value: &str) -> Result<Self> {
        match value {
            "default" => Ok(Self::Default),
            "inherit" => Ok(Self::Inherit),
            _ => anyhow::bail!("new_tab_profile must be \"default\" or \"inherit\""),
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

    fn parse(value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "pane" => Ok(Self::Pane),
            "tab" => Ok(Self::Tab),
            _ => anyhow::bail!("working_directory_scope must be \"none\", \"pane\", or \"tab\""),
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

/// A setting that is absent unless its key appears in the file.
///
/// Deliberately not `Option<T>`: serde reads an explicit `null` into an
/// `Option<T>` field as `None`, which would silently accept `"theme": null`
/// where this file format has always reported it as a type error. Here the
/// key's presence and the value's type are separate questions, and only the
/// value's type is serde's to decide.
enum Setting<T> {
    Unset,
    Set(T),
}

/// Written out rather than derived: `#[derive(Default)]` on a generic enum
/// bounds the impl on `T: Default`, which has nothing to do with whether a key
/// was present.
impl<T> Default for Setting<T> {
    fn default() -> Self {
        Self::Unset
    }
}

impl<T> Setting<T> {
    fn get(self) -> Option<T> {
        match self {
            Self::Unset => None,
            Self::Set(value) => Some(value),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Setting<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Set)
    }
}

/// The configuration file's shape.
///
/// Every field is a [`Setting`] because a configuration file is an overlay:
/// only the keys it actually names are applied over [`Config::defaults`], which
/// is what lets a project's configuration layer over the user's. The container's
/// `default` is what makes an omitted key [`Setting::Unset`].
///
/// `deny_unknown_fields` is the whole of the misspelled-setting check. It used
/// to be a hand-maintained list of the same names, which could drift from the
/// fields it was meant to describe without anything noticing.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    default_profile: Setting<String>,
    new_tab_profile: Setting<String>,
    working_directory: Setting<String>,
    working_directory_scope: Setting<String>,
    theme: Setting<String>,
    dark_theme: Setting<String>,
    /// `null` clears the icon, so this one distinguishes three states: absent,
    /// explicitly cleared, and named.
    default_tab_icon: Setting<Option<String>>,
    terminal_font_size: Setting<f64>,
    terminal_font_family: Setting<String>,
    max_scroll_history_lines: Setting<u64>,
    inactive_pane_opacity: Setting<f64>,
    compact_mode: Setting<bool>,
    hide_pane_size: Setting<bool>,
    hide_title_bar_labels: Setting<bool>,
    hide_title_bar_buttons: Setting<bool>,
    hide_title_bar_menus: Setting<bool>,
    pane_controls_position: Setting<String>,
    pane_controls_hidden_by_default: Setting<bool>,
    http_server_port: Setting<u64>,
    tftp_server_port: Setting<u64>,
    sessions: Setting<SessionsFile>,
    /// Template nodes are polymorphic — a pane object or a one-key split object
    /// — which a derive cannot describe without losing the errors that say what
    /// was wrong with a layout. They stay hand-parsed, from here.
    pane_split_templates: Setting<serde_json::Map<String, Value>>,
    profiles: Setting<Vec<ProfileFile>>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SessionsFile {
    retention: Setting<String>,
    ring_bytes: Setting<u64>,
    persistence: Setting<SessionPersistenceFile>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SessionPersistenceFile {
    recipients: Setting<Vec<String>>,
    /// `null` and an empty string both mean "use the conventional identity",
    /// which is why this is not simply a path.
    identity: Setting<Option<String>>,
    auto_protect: Setting<bool>,
}

/// A profile as the file spells it. `name` is the only required key, because a
/// profile entry may exist only to re-theme or hide a detected shell.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    name: String,
    #[serde(default)]
    program: Setting<String>,
    #[serde(default)]
    args: Setting<Vec<String>>,
    #[serde(default)]
    theme: Setting<String>,
    #[serde(default)]
    dark_theme: Setting<String>,
    /// Kept as a raw value: `"auto"`, `null` and a name are all accepted, and
    /// [`ProfileIcon::parse`] is what tells them apart.
    #[serde(default)]
    icon: Setting<Value>,
    #[serde(default)]
    hidden: Setting<bool>,
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
        let config_path =
            config_path.map_or_else(|| config_dir.join("config.json"), Path::to_path_buf);
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

    fn parse_into(content: &str, config: Self) -> Result<Self> {
        let file = parse_config_file(content)
            .with_context(|| format!("parsing {}", config.config_path.display()))?;
        config.apply(file)
    }

    /// Applies the keys a file actually named, leaving the rest at the value
    /// they already had. The order matters at the end: pane templates name
    /// profiles, and `default_profile` names one the file may have just added.
    fn apply(mut self, file: ConfigFile) -> Result<Self> {
        if let Some(directory) = file.working_directory.get() {
            self.working_directory = Some(expand_home(&directory));
            // An explicit home alias is equivalent to omitting the setting.
            // This distinction matters for WSL: an actual override is a
            // Windows-side cwd, while the default must be passed as `--cd ~`
            // so WSL resolves the Linux user's home directory.
            self.working_directory_configured = !matches!(directory.as_str(), "~" | "~/");
        }
        if let Some(scope) = file.working_directory_scope.get() {
            self.working_directory_scope = WorkingDirectoryScope::parse(&scope)?;
        }
        if let Some(profile) = file.new_tab_profile.get() {
            self.new_tab_profile = NewTabProfile::parse(&profile)?;
        }
        if let Some(theme) = file.theme.get() {
            self.theme = Some(theme);
        }
        if let Some(dark_theme) = file.dark_theme.get() {
            self.dark_theme = Some(dark_theme);
        }
        if let Some(icon) = file.default_tab_icon.get() {
            self.default_tab_icon = icon
                .map(|name| {
                    name.parse().map_err(|_| {
                        anyhow!("default_tab_icon must be a built-in icon name, got {name:?}")
                    })
                })
                .transpose()?;
        }
        if let Some(font_size) = file.terminal_font_size.get() {
            let font_size = font_size as f32;
            anyhow::ensure!(
                (6.0..=100.0).contains(&font_size),
                "terminal_font_size must be between 6 and 100"
            );
            self.terminal_font_size = Some(font_size);
        }
        if let Some(font_family) = file.terminal_font_family.get() {
            anyhow::ensure!(
                !font_family.trim().is_empty(),
                "terminal_font_family must not be empty"
            );
            self.terminal_font_family = font_family;
        }
        if let Some(history_lines) = file.max_scroll_history_lines.get() {
            self.max_scroll_history_lines = parse_max_scroll_history_lines(history_lines)?;
        }
        if let Some(opacity) = file.inactive_pane_opacity.get() {
            self.inactive_pane_opacity = parse_inactive_pane_opacity(opacity)?;
        }
        if let Some(compact) = file.compact_mode.get() {
            self.compact_mode = compact;
        }
        if let Some(hidden) = file.hide_pane_size.get() {
            self.hide_pane_size = hidden;
        }
        if let Some(hidden) = file.hide_title_bar_labels.get() {
            self.hide_title_bar_labels = hidden;
        }
        if let Some(hidden) = file.hide_title_bar_buttons.get() {
            self.hide_title_bar_buttons = hidden;
        }
        if let Some(hidden) = file.hide_title_bar_menus.get() {
            self.hide_title_bar_menus = hidden;
        }
        if let Some(position) = file.pane_controls_position.get() {
            self.pane_controls_position = PaneControlsPosition::parse(&position)?;
        }
        if let Some(hidden) = file.pane_controls_hidden_by_default.get() {
            self.pane_controls_hidden_by_default = hidden;
        }
        if let Some(port) = file.http_server_port.get() {
            self.http_server_port = parse_server_port(port, "http_server_port")?;
        }
        if let Some(port) = file.tftp_server_port.get() {
            self.tftp_server_port = parse_server_port(port, "tftp_server_port")?;
        }
        if let Some(sessions) = file.sessions.get() {
            sessions.apply(&mut self.sessions)?;
            // Only a file that named `sessions` is held to a retention this
            // build can actually serve.
            self.sessions.to_zmux_retention()?;
        }

        if let Some(profiles) = file.profiles.get() {
            let parsed = profiles
                .into_iter()
                .map(ProfileFile::into_config)
                .collect::<Result<Vec<_>>>()?;
            merge_profiles(&mut self.profiles, &parsed)?;
            merge_profile_visibility(&mut self.hidden_profiles, &parsed);
        }

        if let Some(templates) = file.pane_split_templates.get() {
            for (name, value) in &templates {
                anyhow::ensure!(
                    !name.trim().is_empty(),
                    "pane split template names must not be empty"
                );
                let template = parse_pane_split_template_config(value, &self.profiles)
                    .with_context(|| format!("parsing pane split template {name:?}"))?;
                anyhow::ensure!(
                    (2..=64).contains(&template.pane_count()),
                    "pane split template {name:?} must contain between 2 and 64 panes"
                );
                self.pane_split_templates.insert(name.clone(), template);
            }
        }

        if let Some(default_profile) = file.default_profile.get() {
            self.default_profile = resolve_default_profile(&self.profiles, &default_profile)?;
        }

        Ok(self)
    }
}

impl SessionsFile {
    fn apply(self, sessions: &mut SessionsConfig) -> Result<()> {
        if let Some(retention) = self.retention.get() {
            sessions.retention = SessionRetention::parse(&retention)?;
        }
        if let Some(ring_bytes) = self.ring_bytes.get() {
            sessions.ring_bytes = usize::try_from(ring_bytes)
                .ok()
                .filter(|bytes| (4 * 1024..=MAX_SESSION_RING_BYTES).contains(bytes))
                .with_context(|| {
                    format!(
                        "sessions.ring_bytes must be between 4096 and {MAX_SESSION_RING_BYTES} bytes"
                    )
                })?;
        }
        if let Some(persistence) = self.persistence.get() {
            persistence.apply(&mut sessions.persistence)?;
        }
        Ok(())
    }
}

impl SessionPersistenceFile {
    fn apply(self, persistence: &mut SessionPersistenceConfig) -> Result<()> {
        if let Some(recipients) = self.recipients.get() {
            persistence.recipients = recipients
                .into_iter()
                .map(|recipient| {
                    let recipient = recipient.trim();
                    (!recipient.is_empty())
                        .then(|| recipient.to_owned())
                        .context("sessions.persistence.recipients must contain strings")
                })
                .collect::<Result<Vec<_>>>()?;
        }
        if let Some(identity) = self.identity.get() {
            persistence.identity = identity
                .filter(|path| !path.trim().is_empty())
                .map(PathBuf::from);
        }
        if let Some(auto_protect) = self.auto_protect.get() {
            persistence.auto_protect = auto_protect;
        }
        Ok(())
    }
}

/// Reads the file's shape, reporting which setting was wrong rather than only
/// what was wrong with it.
///
/// Serde names the expected type and the offending line, but not the field; on
/// a nested value — a profile's `hidden`, a template's `env` — that is most of
/// the answer missing. `serde_path_to_error` supplies the rest, so a message
/// reads `profiles[1].hidden: invalid type: string "yes", expected a boolean`.
fn parse_config_file(content: &str) -> Result<ConfigFile> {
    let mut deserializer = serde_json::Deserializer::from_str(content);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let path = error.path().to_string();
        let error = error.into_inner();
        if path.is_empty() || path == "." {
            anyhow!("{error}")
        } else {
            anyhow!("{path}: {error}")
        }
    })
}

fn parse_server_port(port: u64, field: &str) -> Result<u16> {
    u16::try_from(port)
        .ok()
        .filter(|port| *port != 0)
        .with_context(|| format!("{field} must be an integer from 1 to 65535"))
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

fn parse_inactive_pane_opacity(opacity: f64) -> Result<f32> {
    anyhow::ensure!(
        (0.0..=1.0).contains(&opacity),
        "inactive_pane_opacity must be between 0 and 1"
    );
    Ok(opacity as f32)
}

fn parse_max_scroll_history_lines(history_lines: u64) -> Result<usize> {
    anyhow::ensure!(
        history_lines <= MAX_SCROLL_HISTORY_LINES as u64,
        "max_scroll_history_lines must not exceed {MAX_SCROLL_HISTORY_LINES}"
    );
    Ok(history_lines as usize)
}

impl ProfileFile {
    fn into_config(self) -> Result<ProfileConfig> {
        let args = self.args.get().unwrap_or_default();
        let program = self.program.get();
        anyhow::ensure!(
            program.is_some() || args.is_empty(),
            "profile.args requires program"
        );
        let name = self.name;
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
        Ok(ProfileConfig {
            name,
            command,
            theme: self.theme.get(),
            dark_theme: self.dark_theme.get(),
            icon: self
                .icon
                .get()
                .map(|icon| ProfileIcon::parse(&icon))
                .transpose()?
                .flatten(),
            hidden: self.hidden.get(),
        })
    }
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
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// The conventional local SSH identity used as the age fallback when no
/// `sessions.persistence.identity` is configured. The file check keeps an
/// absent convention from changing the identity set or enabling a setting
/// that cannot be used.
#[cfg_attr(not(feature = "session-persistence"), allow(dead_code))]
pub(crate) fn default_session_identity_path() -> Option<PathBuf> {
    let home =
        env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)?;
    let path = home.join(".ssh").join("id_ed25519");
    path.is_file().then_some(path)
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
