//! Detecting the shells this machine can offer as profiles.
//!
//! Everything here answers one question — what is installed — and answers it
//! per platform: Homebrew prefixes on macOS and Linux, the MSYS2 and Cygwin
//! installation roots on Windows (from the Start Menu shortcut, the registry,
//! and `PATH`), and the WSL distribution list. The result is the profile set
//! the application's `Config::defaults` starts from, and the set the daemon
//! resolves a shared pane's profile name against, before any file is read.
//!
//! It produces commands only. Which icon a discovered profile is drawn with is
//! decided in the application from the profile's name and command, by
//! `ProfileIcon::automatic_for_profile`, which already reproduces every special
//! case this module used to spell out for itself.

use super::*;

use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod homebrew;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use homebrew::*;
#[cfg(windows)]
mod windows_shells;
#[cfg(windows)]
use windows_shells::*;

/// What this machine has installed, probed once. The answer cannot change
/// while the process runs without something being installed under it, and the
/// Windows half of the probe reads the registry and the Start Menu.
pub fn discovered_profiles() -> Vec<ProfileDefinition> {
    static DISCOVERED_PROFILES: OnceLock<Vec<ProfileDefinition>> = OnceLock::new();
    DISCOVERED_PROFILES.get_or_init(discover_profiles).clone()
}

fn discover_profiles() -> Vec<ProfileDefinition> {
    let mut profiles = vec![ProfileDefinition {
        name: "System".to_owned(),
        command: ProfileCommand::system(),
    }];
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let homebrew_prefixes = homebrew_prefixes();
    #[cfg(windows)]
    let system_directory = system_directory();
    #[cfg(windows)]
    profiles.extend(windows_shell_profiles(
        system_directory.as_deref(),
        env::var_os("PATH").as_deref(),
    ));
    #[cfg(not(windows))]
    let mut seen = HashSet::new();
    #[cfg(not(windows))]
    for (name, program) in [
        ("Zsh", "zsh"),
        ("Bash", "bash"),
        ("Fish", "fish"),
        ("Nushell", "nu"),
    ] {
        if let Some(path) = command_path(program)
            && seen.insert(path.clone())
        {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let profile =
                homebrew_profile_for_path(&path, &homebrew_prefixes).unwrap_or_else(|| {
                    ProfileDefinition {
                        name: name.to_owned(),
                        command: ProfileCommand::program(program),
                    }
                });
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let profile = ProfileDefinition {
                name: name.to_owned(),
                command: ProfileCommand::program(program),
            };
            profiles.push(profile);
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    profiles.extend(
        homebrew_shell_profiles(homebrew_prefixes)
            .into_iter()
            .filter(|profile| {
                let Some(program) = &profile.command.program else {
                    return true;
                };
                seen.insert(PathBuf::from(program))
            }),
    );
    #[cfg(windows)]
    if let Some(root) = msys2_installation_root()
        && let Some(cmd) = system_directory
            .as_deref()
            .map(|system| system.join("cmd.exe"))
    {
        profiles.extend(msys2_profiles(&root, &cmd));
    }
    #[cfg(windows)]
    if let Some(root) = cygwin_installation_root() {
        profiles.extend(cygwin_profiles(&root));
    }
    #[cfg(windows)]
    if let Some(program) = wsl_program() {
        profiles.extend(discovered_wsl_profiles(&program));
    }
    profiles
}

/// Where one of Windows' own shells is installed.
#[cfg(any(windows, test))]
enum WindowsShell {
    /// Under the system directory, which is where Windows keeps it whatever
    /// `PATH` says.
    System(&'static [&'static str]),
    /// Found on `PATH`, because it is installed separately.
    Path(&'static str),
}

#[cfg(any(windows, test))]
const WINDOWS_PROFILE_CANDIDATES: &[(&str, WindowsShell)] = &[
    (
        "PowerShell",
        WindowsShell::System(&["WindowsPowerShell", "v1.0", "powershell.exe"]),
    ),
    ("PowerShell 7", WindowsShell::Path("pwsh.exe")),
    ("Command Prompt", WindowsShell::System(&["cmd.exe"])),
];

/// Windows' own shells, each spelled as the absolute path it was found at.
///
/// A bare `cmd.exe` is searched for when the pane starts, and Windows' search
/// includes the current directory — the pane's, for the palette bootstrap —
/// before the system one. The path found here is the one that should run, so
/// it is the one the profile carries.
#[cfg(any(windows, test))]
fn windows_shell_profiles(
    system_directory: Option<&Path>,
    path: Option<&OsStr>,
) -> Vec<ProfileDefinition> {
    let mut seen = HashSet::new();
    WINDOWS_PROFILE_CANDIDATES
        .iter()
        .filter_map(|(name, shell)| {
            let program = match shell {
                WindowsShell::System(components) => {
                    let program = system_directory?.join(components.iter().collect::<PathBuf>());
                    program.is_file().then_some(program)?
                }
                WindowsShell::Path(program) => command_path_in(program, path?)?,
            };
            seen.insert(program.clone()).then(|| ProfileDefinition {
                name: (*name).to_owned(),
                command: ProfileCommand::program(program.display().to_string()),
            })
        })
        .collect()
}

#[cfg(any(windows, test))]
fn msys2_profiles(root: &Path, cmd: &Path) -> Vec<ProfileDefinition> {
    let launcher = root.join("msys2_shell.cmd");
    [("MSYS2", "bash"), ("MSYS2: Zsh", "zsh")]
        .into_iter()
        .filter(|(_, shell)| root.join("usr/bin").join(format!("{shell}.exe")).is_file())
        .map(|(name, shell)| {
            let command = format!(
                "\"\"{}\" -defterm -here -no-start -msys -use-full-path -shell {shell}\"",
                launcher.display()
            );
            ProfileDefinition {
                name: name.to_owned(),
                command: ProfileCommand::with_args(
                    cmd.display().to_string(),
                    vec!["/d".to_owned(), "/s".to_owned(), "/c".to_owned(), command],
                ),
            }
        })
        .collect()
}

#[cfg(any(windows, test))]
fn select_cygwin_installation_root(roots: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    let mut first_valid_root = None;
    for root in roots {
        if !root.join("bin").join("cygwin1.dll").is_file() {
            continue;
        }
        if first_valid_root.is_none() {
            first_valid_root = Some(root.clone());
        }
        // A stale registry entry may retain cygwin1.dll after the supported
        // shell executables have been removed. Prefer the next complete
        // installation so that one such entry cannot hide a working root.
        if !cygwin_profiles(&root).is_empty() {
            return Some(root);
        }
    }
    first_valid_root
}

#[cfg(any(windows, test))]
fn normalize_cygwin_registry_path(value: &str) -> Option<PathBuf> {
    let value = value.trim().trim_end_matches('\0');
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }

    let value = value.replace('/', "\\");
    let value = ["\\\\??\\", "\\??\\", "\\\\?\\"]
        .iter()
        .find_map(|prefix| {
            value
                .get(..prefix.len())
                .filter(|value| value.eq_ignore_ascii_case(prefix))
                .map(|_| &value[prefix.len()..])
        })
        .unwrap_or(&value);
    let value = if value
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("UNC\\"))
    {
        format!("\\\\{}", &value[4..])
    } else {
        value.to_owned()
    };
    (!value.is_empty()).then(|| PathBuf::from(value))
}

#[cfg(any(windows, test))]
fn cygwin_profiles(root: &Path) -> Vec<ProfileDefinition> {
    [
        ("Cygwin", "bash"),
        ("Cygwin: Zsh", "zsh"),
        ("Cygwin: Fish", "fish"),
        ("Cygwin: Nushell", "nu"),
    ]
    .into_iter()
    .filter_map(|(name, shell)| {
        let program = root.join("bin").join(format!("{shell}.exe"));
        program.is_file().then(|| ProfileDefinition {
            name: name.to_owned(),
            command: ProfileCommand::with_args(
                program.display().to_string(),
                vec!["-l".to_owned()],
            ),
        })
    })
    .collect()
}

#[cfg(any(windows, test))]
fn wsl_profiles_from_output(program: &str, output: &[u8]) -> Vec<ProfileDefinition> {
    parse_wsl_distribution_names(output)
        .into_iter()
        .map(|distribution| ProfileDefinition {
            name: format!("WSL: {distribution}"),
            command: ProfileCommand::with_args(
                program,
                vec!["--distribution".to_owned(), distribution],
            ),
        })
        .collect()
}

#[cfg(any(windows, test))]
fn parse_wsl_distribution_names(output: &[u8]) -> Vec<String> {
    let decoded = if let Some(bytes) = output.strip_prefix(&[0xfe, 0xff]) {
        let code_units = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&code_units)
    } else if output.starts_with(&[0xff, 0xfe]) || output.contains(&0) {
        let bytes = output.strip_prefix(&[0xff, 0xfe]).unwrap_or(output);
        let code_units = bytes
            .as_chunks::<2>()
            .0
            .iter()
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

/// Looks `program` up in the absolute entries of `path`.
///
/// An empty or relative entry names the current directory, which is wherever
/// the process happened to start: a program found there is not installed, it
/// is just nearby, and on Windows it would be run in place of the real one.
fn command_path_in(program: &str, path: &OsStr) -> Option<PathBuf> {
    env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .find_map(|directory| {
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

#[cfg(test)]
#[path = "tests/discovery.rs"]
mod tests;
