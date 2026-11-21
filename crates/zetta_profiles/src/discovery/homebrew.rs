//! Homebrew's shells on macOS and Linux: the prefixes it installs under, and
//! the profile each shell found there becomes.

use super::*;

pub(super) const POSIX_PROFILE_CANDIDATES: &[(&str, &str)] = &[
    ("Zsh", "zsh"),
    ("Bash", "bash"),
    ("Fish", "fish"),
    ("Nushell", "nu"),
];

/// The Homebrew installations on this machine.
///
/// `HOMEBREW_PREFIX` is only set by `brew shellenv`, which a user typically
/// runs from an interactive shell's rc file. A process started from a desktop
/// launcher never sees it, so relying on it alone made the profile list depend
/// on how Zetta was started. The candidates below do not need it.
pub(super) fn homebrew_prefixes() -> Vec<PathBuf> {
    homebrew_prefixes_from(
        env::var_os("HOMEBREW_PREFIX").as_deref(),
        env::var_os("HOME").as_deref().map(Path::new),
        env::var_os("PATH").as_deref(),
        SYSTEM_HOMEBREW_PREFIXES,
    )
}

/// The default prefixes of a system-wide Homebrew install.
#[cfg(target_os = "macos")]
pub(super) const SYSTEM_HOMEBREW_PREFIXES: &[&str] = &["/opt/homebrew", "/usr/local"];
#[cfg(target_os = "linux")]
pub(super) const SYSTEM_HOMEBREW_PREFIXES: &[&str] = &["/home/linuxbrew/.linuxbrew"];

/// Every place a Homebrew prefix is conventionally found, in priority order,
/// kept only where `bin/brew` exists.
///
/// In order: the environment's own answer, the prefix of any `brew` on `PATH`,
/// the per-user prefixes Homebrew documents for installs without root
/// (`~/.brew`, and `~/.linuxbrew` on Linux), then the `system` prefixes, which
/// are a parameter so that tests do not see the host's own installation.
pub(super) fn homebrew_prefixes_from(
    environment_prefix: Option<&OsStr>,
    home: Option<&Path>,
    path: Option<&OsStr>,
    system: &[&str],
) -> Vec<PathBuf> {
    let mut prefixes = environment_prefix
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    prefixes.extend(
        path.into_iter()
            .flat_map(env::split_paths)
            .filter(|directory| directory.is_absolute() && directory.join("brew").is_file())
            .filter_map(|directory| directory.parent().map(Path::to_path_buf)),
    );
    if let Some(home) = home.filter(|home| home.is_absolute()) {
        prefixes.push(home.join(".brew"));
        #[cfg(target_os = "linux")]
        prefixes.push(home.join(".linuxbrew"));
    }
    prefixes.extend(system.iter().map(PathBuf::from));

    let mut seen = HashSet::new();
    prefixes
        .into_iter()
        .filter(|prefix| seen.insert(prefix.clone()))
        .filter(|prefix| prefix.join("bin/brew").is_file())
        .collect()
}

pub(super) fn homebrew_profile_for_path(
    path: &Path,
    prefixes: &[PathBuf],
) -> Option<ProfileDefinition> {
    POSIX_PROFILE_CANDIDATES.iter().find_map(|(name, program)| {
        prefixes.iter().find_map(|prefix| {
            let homebrew_path = prefix.join("bin").join(program);
            (homebrew_path == path).then(|| homebrew_profile(name, homebrew_path))
        })
    })
}

pub(super) fn homebrew_profile(name: &str, path: PathBuf) -> ProfileDefinition {
    ProfileDefinition {
        name: format!("{name} (Homebrew)"),
        command: ProfileCommand::program(path.to_string_lossy().into_owned()),
    }
}

pub(super) fn homebrew_shell_profiles(
    prefixes: impl IntoIterator<Item = PathBuf>,
) -> Vec<ProfileDefinition> {
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
