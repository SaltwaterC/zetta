use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[cfg(windows)]
use std::{os::windows::process::CommandExt as _, process::Command};

use anyhow::{Context as _, Result};
use serde_json::Value;
use task::Shell;
use terminal::MAX_SCROLL_HISTORY_LINES;

use crate::http_server::DEFAULT_HTTP_PORT;

const DEFAULT_TERMINAL_FONT_FAMILY: &str = "MesloLGS NF";
const DEFAULT_MAX_SCROLL_HISTORY_LINES: usize = MAX_SCROLL_HISTORY_LINES;
const DEFAULT_INACTIVE_PANE_OPACITY: f32 = 0.8;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneSplitTemplate {
    Pane,
    Split {
        axis: PaneSplitAxis,
        first: Box<PaneSplitTemplate>,
        second: Box<PaneSplitTemplate>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneSplitAxis {
    Horizontal,
    Vertical,
}

impl PaneSplitTemplate {
    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub command: Shell,
    pub theme: Option<String>,
}

struct ProfileConfig {
    name: String,
    command: Option<Shell>,
    theme: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub config_path: PathBuf,
    pub keymap_override: Option<PathBuf>,
    pub profiles: Vec<Profile>,
    pub default_profile: usize,
    pub working_directory: Option<PathBuf>,
    pub working_directory_configured: bool,
    pub keymap_path: PathBuf,
    pub theme: Option<String>,
    pub terminal_font_size: Option<f32>,
    pub terminal_font_family: String,
    pub max_scroll_history_lines: usize,
    pub inactive_pane_opacity: f32,
    pub pane_controls_position: PaneControlsPosition,
    pub http_server_port: u16,
    pub pane_split_templates: HashMap<String, PaneSplitTemplate>,
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
            default_profile: 0,
            working_directory: Some(home_dir()),
            working_directory_configured: false,
            keymap_path: keymap_path.unwrap_or_else(|| config_dir.join("keymap.json")),
            theme: None,
            terminal_font_size: None,
            terminal_font_family: DEFAULT_TERMINAL_FONT_FAMILY.to_owned(),
            max_scroll_history_lines: DEFAULT_MAX_SCROLL_HISTORY_LINES,
            inactive_pane_opacity: DEFAULT_INACTIVE_PANE_OPACITY,
            pane_controls_position: PaneControlsPosition::default(),
            http_server_port: DEFAULT_HTTP_PORT,
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

    fn parse_into(content: &str, mut config: Self) -> Result<Self> {
        let root: Value = serde_json::from_str(&content)
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
        if let Some(theme) = root.get("theme") {
            config.theme = Some(theme.as_str().context("theme must be a string")?.to_owned());
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
        if let Some(port) = root.get("http_server_port") {
            let port = port
                .as_u64()
                .context("http_server_port must be an integer from 1 to 65535")?;
            config.http_server_port = u16::try_from(port)
                .ok()
                .filter(|port| *port != 0)
                .context("http_server_port must be an integer from 1 to 65535")?;
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
                let template = parse_pane_split_template(value)
                    .with_context(|| format!("parsing pane split template {name:?}"))?;
                anyhow::ensure!(
                    (2..=64).contains(&template.pane_count()),
                    "pane split template {name:?} must contain between 2 and 64 panes"
                );
                config.pane_split_templates.insert(name.clone(), template);
            }
        }

        if let Some(profiles) = root.get("profiles") {
            let profiles = profiles.as_array().context("profiles must be an array")?;
            let parsed = profiles
                .iter()
                .map(parse_profile)
                .collect::<Result<Vec<_>>>()?;
            merge_profiles(&mut config.profiles, parsed)?;
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
        "working_directory",
        "theme",
        "terminal_font_size",
        "terminal_font_family",
        "max_scroll_history_lines",
        "inactive_pane_opacity",
        "pane_controls_position",
        "http_server_port",
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

fn default_pane_split_templates() -> HashMap<String, PaneSplitTemplate> {
    let pane = || Box::new(PaneSplitTemplate::Pane);
    HashMap::from([
        (
            "three-right".to_owned(),
            PaneSplitTemplate::Split {
                axis: PaneSplitAxis::Vertical,
                first: pane(),
                second: Box::new(PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Horizontal,
                    first: pane(),
                    second: pane(),
                }),
            },
        ),
        (
            "three-left".to_owned(),
            PaneSplitTemplate::Split {
                axis: PaneSplitAxis::Vertical,
                first: Box::new(PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Horizontal,
                    first: pane(),
                    second: pane(),
                }),
                second: pane(),
            },
        ),
        (
            "quarters".to_owned(),
            PaneSplitTemplate::Split {
                axis: PaneSplitAxis::Vertical,
                first: Box::new(PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Horizontal,
                    first: pane(),
                    second: pane(),
                }),
                second: Box::new(PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Horizontal,
                    first: pane(),
                    second: pane(),
                }),
            },
        ),
    ])
}

fn parse_pane_split_template(value: &Value) -> Result<PaneSplitTemplate> {
    if value.as_str() == Some("pane") {
        return Ok(PaneSplitTemplate::Pane);
    }

    let object = value
        .as_object()
        .context("template nodes must be \"pane\" or a split object")?;
    anyhow::ensure!(
        object.len() == 1,
        "split objects must have exactly one axis"
    );
    let (axis, children) = object.iter().next().unwrap();
    let axis = match axis.as_str() {
        "horizontal" => PaneSplitAxis::Horizontal,
        "vertical" => PaneSplitAxis::Vertical,
        _ => anyhow::bail!("split axis must be \"horizontal\" or \"vertical\""),
    };
    let children = children
        .as_array()
        .context("split children must be a two-element array")?;
    anyhow::ensure!(children.len() == 2, "splits must have exactly two children");
    Ok(PaneSplitTemplate::Split {
        axis,
        first: Box::new(parse_pane_split_template(&children[0])?),
        second: Box::new(parse_pane_split_template(&children[1])?),
    })
}

fn resolve_default_profile(profiles: &[Profile], name: &str) -> Result<usize> {
    profiles
        .iter()
        .position(|profile| profile.name.eq_ignore_ascii_case(name))
        .with_context(|| format!("default profile {name:?} is not available"))
}

fn merge_profiles(profiles: &mut Vec<Profile>, configured: Vec<ProfileConfig>) -> Result<()> {
    for profile in configured {
        if let Some(index) = profiles
            .iter()
            .position(|existing| existing.name.eq_ignore_ascii_case(&profile.name))
        {
            if let Some(command) = profile.command {
                profiles[index].command = command;
            }
            if let Some(theme) = profile.theme {
                profiles[index].theme = Some(theme);
            }
        } else {
            let command = profile.command.with_context(|| {
                format!(
                    "profile {:?} must specify program because it was not detected",
                    profile.name
                )
            })?;
            profiles.push(Profile {
                name: profile.name,
                command,
                theme: profile.theme,
            });
        }
    }
    Ok(())
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
    const FIELDS: &[&str] = &["name", "program", "args", "theme"];
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
    Ok(ProfileConfig {
        name,
        command,
        theme,
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
    }];
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
        if command_exists(program) && seen.insert(*program) {
            profiles.push(Profile {
                name: (*name).to_owned(),
                command: Shell::Program((*program).to_owned()),
                theme: None,
            });
        }
    }
    #[cfg(windows)]
    if let Some(program) = wsl_program() {
        profiles.extend(discovered_wsl_profiles(&program));
    }
    profiles
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

fn command_exists(program: &str) -> bool {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.is_file();
    }
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|directory| {
            if cfg!(windows) {
                directory.join(program).is_file()
                    || (!program.to_ascii_lowercase().ends_with(".exe")
                        && directory.join(format!("{program}.exe")).is_file())
            } else {
                directory.join(program).is_file()
            }
        })
    })
}

pub(crate) fn platform_config_dir() -> PathBuf {
    if cfg!(windows) {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Zetta")
    } else if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("zetta")
    } else {
        home_dir().join(".config/zetta")
    }
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
