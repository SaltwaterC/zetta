use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

#[cfg(windows)]
use std::{os::windows::process::CommandExt as _, process::Command};

use anyhow::{Context as _, Result};
use serde_json::Value;
use task::Shell;
use terminal::MAX_SCROLL_HISTORY_LINES;

const DEFAULT_TERMINAL_FONT_FAMILY: &str = "MesloLGS NF";
const DEFAULT_MAX_SCROLL_HISTORY_LINES: usize = MAX_SCROLL_HISTORY_LINES;
const DEFAULT_INACTIVE_PANE_OPACITY: f32 = 0.8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellProfile {
    pub name: String,
    pub shell: Shell,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub config_path: PathBuf,
    pub keymap_override: Option<PathBuf>,
    pub profiles: Vec<ShellProfile>,
    pub default_profile: usize,
    pub working_directory: Option<PathBuf>,
    pub keymap_path: PathBuf,
    pub theme: Option<String>,
    pub terminal_font_size: Option<f32>,
    pub terminal_font_family: String,
    pub max_scroll_history_lines: usize,
    pub inactive_pane_opacity: f32,
}

impl Config {
    pub fn load(config_path: Option<&Path>, keymap_path: Option<PathBuf>) -> Result<Self> {
        let config_dir = platform_config_dir();
        let has_keymap_override = keymap_path.is_some();
        let config_path = config_path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| config_dir.join("config.json"));
        let mut config = Self {
            config_path: config_path.clone(),
            keymap_override: keymap_path.clone(),
            profiles: discovered_profiles(),
            default_profile: 0,
            working_directory: None,
            keymap_path: keymap_path.unwrap_or_else(|| config_dir.join("keymap.json")),
            theme: None,
            terminal_font_size: None,
            terminal_font_family: DEFAULT_TERMINAL_FONT_FAMILY.to_owned(),
            max_scroll_history_lines: DEFAULT_MAX_SCROLL_HISTORY_LINES,
            inactive_pane_opacity: DEFAULT_INACTIVE_PANE_OPACITY,
        };

        let content = match fs::read_to_string(&config_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(config),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", config_path.display()));
            }
        };
        let root: Value = serde_json::from_str(&content)
            .with_context(|| format!("parsing {}", config_path.display()))?;

        if let Some(directory) = root.get("working_directory").and_then(Value::as_str) {
            config.working_directory = Some(expand_home(directory));
        }
        if !has_keymap_override && let Some(path) = root.get("keymap").and_then(Value::as_str) {
            config.keymap_path = expand_home(path);
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

        if let Some(profiles) = root.get("shells").and_then(Value::as_array) {
            let parsed = profiles
                .iter()
                .map(parse_profile)
                .collect::<Result<Vec<_>>>()?;
            if !parsed.is_empty() {
                config.profiles = parsed;
            }
        }

        if let Some(default_name) = root.get("default_shell").and_then(Value::as_str) {
            config.default_profile = config
                .profiles
                .iter()
                .position(|profile| profile.name.eq_ignore_ascii_case(default_name))
                .with_context(|| format!("default_shell {default_name:?} is not in shells"))?;
        }

        Ok(config)
    }
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

fn parse_profile(value: &Value) -> Result<ShellProfile> {
    let object = value.as_object().context("each shell must be an object")?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .context("shell.name must be a string")?
        .to_owned();
    let program = object
        .get("program")
        .and_then(Value::as_str)
        .context("shell.program must be a string")?
        .to_owned();
    let args = object
        .get("args")
        .map(|args| {
            args.as_array()
                .context("shell.args must be an array")?
                .iter()
                .map(|arg| {
                    arg.as_str()
                        .map(str::to_owned)
                        .context("shell args must be strings")
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();

    let shell = if args.is_empty() {
        Shell::Program(program)
    } else {
        Shell::WithArguments {
            program,
            args,
            title_override: Some(name.clone()),
        }
    };
    Ok(ShellProfile { name, shell })
}

fn discovered_profiles() -> Vec<ShellProfile> {
    let mut profiles = vec![ShellProfile {
        name: "System".to_owned(),
        shell: Shell::System,
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
            profiles.push(ShellProfile {
                name: (*name).to_owned(),
                shell: Shell::Program((*program).to_owned()),
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
fn discovered_wsl_profiles(program: &str) -> Vec<ShellProfile> {
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

fn wsl_profiles_from_output(program: &str, output: &[u8]) -> Vec<ShellProfile> {
    parse_wsl_distribution_names(output)
        .into_iter()
        .map(|distribution| {
            let name = format!("WSL: {distribution}");
            ShellProfile {
                name: name.clone(),
                shell: Shell::WithArguments {
                    program: program.to_owned(),
                    args: vec!["--distribution".to_owned(), distribution],
                    title_override: Some(name),
                },
            }
        })
        .collect()
}

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

fn platform_config_dir() -> PathBuf {
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
mod tests {
    use super::*;

    #[test]
    fn parses_shell_profile_with_arguments() {
        let profile = parse_profile(&serde_json::json!({
            "name": "WSL Ubuntu",
            "program": "wsl.exe",
            "args": ["-d", "Ubuntu"]
        }))
        .unwrap();
        assert_eq!(profile.name, "WSL Ubuntu");
        assert!(matches!(
            profile.shell,
            Shell::WithArguments { ref program, ref args, .. }
                if program == "wsl.exe" && args == &["-d", "Ubuntu"]
        ));
    }

    #[test]
    fn parses_utf8_wsl_distribution_names() {
        assert_eq!(
            parse_wsl_distribution_names(b"Ubuntu\r\nDocker-Desktop\r\nDebian\r\nubuntu\r\n\r\n"),
            ["Ubuntu", "Debian"]
        );
    }

    #[test]
    fn parses_utf16_wsl_distribution_names() {
        let output = "Ubuntu-24.04\r\nopenSUSE Tumbleweed\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();

        assert_eq!(
            parse_wsl_distribution_names(&output),
            ["Ubuntu-24.04", "openSUSE Tumbleweed"]
        );
    }

    #[test]
    fn parses_big_endian_utf16_wsl_distribution_names() {
        let mut output = vec![0xfe, 0xff];
        output.extend("Debian\r\n".encode_utf16().flat_map(u16::to_be_bytes));

        assert_eq!(parse_wsl_distribution_names(&output), ["Debian"]);
    }

    #[test]
    fn creates_a_profile_for_each_wsl_distribution() {
        let profiles = wsl_profiles_from_output("wsl.exe", b"Ubuntu\r\nDebian\r\n");

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "WSL: Ubuntu");
        assert!(matches!(
            profiles[0].shell,
            Shell::WithArguments {
                ref program,
                ref args,
                ref title_override,
            } if program == "wsl.exe"
                && args == &["--distribution", "Ubuntu"]
                && title_override.as_deref() == Some("WSL: Ubuntu")
        ));
    }

    #[test]
    fn validates_max_scroll_history_lines() {
        assert_eq!(
            parse_max_scroll_history_lines(&serde_json::json!(0)).unwrap(),
            0
        );
        assert_eq!(
            parse_max_scroll_history_lines(&serde_json::json!(2_147_483_647)).unwrap(),
            2_147_483_647
        );
        assert!(parse_max_scroll_history_lines(&serde_json::json!(-1)).is_err());
        assert!(parse_max_scroll_history_lines(&serde_json::json!(2_147_483_648_u64)).is_err());
        assert!(parse_max_scroll_history_lines(&serde_json::json!(1.5)).is_err());
    }

    #[test]
    fn validates_inactive_pane_opacity() {
        assert_eq!(DEFAULT_INACTIVE_PANE_OPACITY, 0.8);
        assert_eq!(
            parse_inactive_pane_opacity(&serde_json::json!(0.8)).unwrap(),
            0.8
        );
        assert!(parse_inactive_pane_opacity(&serde_json::json!(-0.1)).is_err());
        assert!(parse_inactive_pane_opacity(&serde_json::json!(1.1)).is_err());
        assert!(parse_inactive_pane_opacity(&serde_json::json!("dim")).is_err());
    }
}
