use super::*;
use crate::startup::format_help_table;
use std::ffi::OsStr;
use std::io::Write as _;
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
#[cfg(windows)]
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellIntegration {
    Bash,
    Fish,
    PowerShell,
    Zsh,
}

impl ShellIntegration {
    pub(crate) fn parse(shell: &str) -> Result<Self> {
        match shell.to_ascii_lowercase().as_str() {
            "bash" => Ok(Self::Bash),
            "fish" => Ok(Self::Fish),
            "powershell" | "pwsh" => Ok(Self::PowerShell),
            "zsh" => Ok(Self::Zsh),
            _ => anyhow::bail!(
                "unsupported shell {shell:?}; supported shells: bash, fish, powershell, zsh"
            ),
        }
    }

    pub(crate) fn script(self) -> String {
        let template = match self {
            Self::Bash => BASH_INTEGRATION,
            Self::Fish => FISH_INTEGRATION,
            Self::PowerShell => POWERSHELL_INTEGRATION,
            Self::Zsh => ZSH_INTEGRATION,
        };
        let template = template.replace("ZETTA_OVERLAY_COLORS", &render_overlay_color_names(self));
        render_worktree_integration(&template)
    }

    fn startup_file(self, home: &Path) -> PathBuf {
        match self {
            Self::Bash => home.join(".bashrc"),
            Self::Fish => home.join(".config/fish/config.fish"),
            Self::PowerShell => {
                #[cfg(windows)]
                {
                    home.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1")
                }
                #[cfg(not(windows))]
                {
                    home.join(".config/powershell/Microsoft.PowerShell_profile.ps1")
                }
            }
            Self::Zsh => home.join(".zshrc"),
        }
    }

    fn configuration_command(self) -> &'static str {
        match self {
            Self::Bash => "eval \"$(zetta init bash)\"",
            Self::Fish => "zetta init fish | source",
            Self::PowerShell => "zetta init powershell | Out-String | Invoke-Expression",
            Self::Zsh => "eval \"$(zetta init zsh)\"",
        }
    }

    fn configuration_is_present(self, contents: &str) -> bool {
        contents.lines().any(|line| {
            let line = line.trim_start();
            if line.starts_with('#') {
                return false;
            }
            match self {
                Self::PowerShell => line.contains(self.configuration_command()),
                _ => line.contains(self.configuration_command()),
            }
        })
    }

    fn migrate_configuration(self, contents: &str) -> Option<String> {
        if self != Self::PowerShell {
            return None;
        }

        const LEGACY_COMMANDS: [&str; 2] = [
            "zetta init powershell | Invoke-Expression",
            "zetta init pwsh | Invoke-Expression",
        ];
        let mut changed = false;
        let mut migrated = String::with_capacity(contents.len());
        for line in contents.split_inclusive('\n') {
            if line.trim_start().starts_with('#') {
                migrated.push_str(line);
                continue;
            }

            let mut line = line.to_owned();
            for legacy_command in LEGACY_COMMANDS {
                if line.contains(legacy_command) {
                    line = line.replacen(legacy_command, self.configuration_command(), 1);
                    changed = true;
                    break;
                }
            }
            migrated.push_str(&line);
        }
        changed.then_some(migrated)
    }

    fn from_shell_path(path: &Path) -> Result<Self> {
        let shell_name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .context("could not determine the current shell from SHELL")?;
        Self::parse(shell_name)
    }

    fn current(shell_path: Option<&OsStr>) -> Result<Self> {
        let active_shell_path = active_shell_path();

        Self::current_with_active_shell(active_shell_path.as_deref(), shell_path)
    }

    fn current_with_active_shell(
        active_shell_path: Option<&Path>,
        shell_path: Option<&OsStr>,
    ) -> Result<Self> {
        if let Some(active_shell_path) = active_shell_path
            && let Ok(shell) = Self::from_shell_path(active_shell_path)
        {
            return Ok(shell);
        }

        match shell_path {
            Some(shell_path) => Self::from_shell_path(Path::new(shell_path)),
            None => {
                #[cfg(windows)]
                {
                    Ok(Self::PowerShell)
                }
                #[cfg(not(windows))]
                {
                    anyhow::bail!("could not determine the current shell: SHELL is not set")
                }
            }
        }
    }
}

fn render_worktree_integration(template: &str) -> String {
    const BEGIN: &str = "ZETTA_WORKTREE_INTEGRATION_BEGIN";
    const END: &str = "ZETTA_WORKTREE_INTEGRATION_END";
    let worktree_enabled = cfg!(feature = "worktree");
    let mut rendered = String::with_capacity(template.len());
    let mut in_optional_section = false;

    for line in template.split_inclusive('\n') {
        if line.contains(BEGIN) {
            in_optional_section = true;
            continue;
        }
        if line.contains(END) {
            in_optional_section = false;
            continue;
        }
        if !in_optional_section || worktree_enabled {
            rendered.push_str(line);
        }
    }

    rendered
        .replace(
            "ZETTA_WORKTREE_ROOT_COMMANDS",
            if worktree_enabled { ", 'wt'" } else { "" },
        )
        .replace(
            "ZETTA_WORKTREE_ROOT_COMMAND",
            if worktree_enabled { "wt" } else { "" },
        )
        .replace(
            "ZETTA_WORKTREE_BASH_COPY_CONDITION",
            if worktree_enabled {
                "[[ $command == wt && ${COMP_WORDS[2]} == new ]]"
            } else {
                "false"
            },
        )
        .replace(
            "ZETTA_WORKTREE_BASH_REPEATABLE_COPY",
            if worktree_enabled {
                "[[ $repeatable == 1 && $candidate == --copy ]]"
            } else {
                "false"
            },
        )
        .replace(
            "ZETTA_WORKTREE_ZSH_COPY_OPTION",
            if worktree_enabled {
                "[[ $option == --copy ]]"
            } else {
                "false"
            },
        )
        .replace(
            "ZETTA_WORKTREE_FISH_COPY_OPTION",
            if worktree_enabled {
                "test \"$argv[1]\" = --copy"
            } else {
                "false"
            },
        )
        .replace(
            "ZETTA_WORKTREE_POWERSHELL_COPY_COMPLETION_CHECK",
            if worktree_enabled {
                "($worktreeCommand -and $worktreeOperation -eq 'new' -and $previous -in '--copy', '-c')"
            } else {
                "($false)"
            },
        )
        .replace(
            "ZETTA_WORKTREE_POWERSHELL_REPEATABLE_COPY",
            if worktree_enabled {
                "$_ -eq '--copy'"
            } else {
                "$false"
            },
        )
        .replace(
            "ZETTA_WORKTREE_COMPLETION_CHECK",
            if worktree_enabled {
                "($worktreeCommand)"
            } else {
                "($false)"
            },
        )
        .replace(
            "ZETTA_WORKTREE_STANDALONE_CHECK",
            if worktree_enabled {
                "($commandName -eq 'zwt')"
            } else {
                "($false)"
            },
        )
        .replace(
            "ZETTA_WORKTREE_ROOT_CHECK",
            if worktree_enabled {
                "($subcommand -eq 'wt')"
            } else {
                "($false)"
            },
        )
        .replace(
            "ZETTA_WORKTREE_SWITCH_CASE",
            if worktree_enabled {
                "'wt'"
            } else {
                "'__zetta_worktree_disabled__'"
            },
        )
}

fn active_shell_path() -> Option<PathBuf> {
    let mut system = sysinfo::System::new();
    let mut pid = sysinfo::get_current_pid().ok()?;

    loop {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        let parent_pid = system.process(pid)?.parent()?;
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[parent_pid]), true);
        let parent = system.process(parent_pid)?;

        if let Some(executable) = parent.exe()
            && ShellIntegration::from_shell_path(executable).is_ok()
        {
            return Some(executable.to_path_buf());
        }

        let process_name = parent.name();
        if ShellIntegration::from_shell_path(Path::new(process_name)).is_ok() {
            return Some(PathBuf::from(process_name));
        }

        pid = parent_pid;
    }
}

#[cfg(windows)]
fn cygwin_tool_from_active_shell(tool: &str) -> PathBuf {
    active_shell_path()
        .map(|path| cygwin_tool_for_path(&path, tool))
        .unwrap_or_else(|| PathBuf::from(tool))
}

#[cfg(windows)]
fn cygwin_tool_for_path(path: &Path, tool: &str) -> PathBuf {
    cygwin_root_for_path(path)
        .map(|root| root.join("bin").join(tool))
        .filter(|tool_path| tool_path.is_file())
        .unwrap_or_else(|| PathBuf::from(tool))
}

#[cfg(windows)]
fn cygwin_tool_for_path_or_active_shell(path: &Path, tool: &str) -> PathBuf {
    active_shell_path()
        .map(|shell| cygwin_tool_for_path(&shell, tool))
        .filter(|tool_path| tool_path.is_file())
        .unwrap_or_else(|| cygwin_tool_for_path(path, tool))
}

#[cfg(windows)]
fn cygwin_root_for_path(path: &Path) -> Option<PathBuf> {
    let mut candidate = path;
    loop {
        if candidate.join("bin").join("cygwin1.dll").is_file() {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShellIntegrationConfiguration {
    Written(PathBuf),
    AlreadyPresent(PathBuf),
}

pub(crate) fn configure_current_shell_integration() -> Result<ShellIntegrationConfiguration> {
    let shell = ShellIntegration::current(env::var_os("SHELL").as_deref())?;

    #[cfg(windows)]
    if shell == ShellIntegration::PowerShell {
        let profile = current_powershell_profile()?;
        return configure_shell_integration_file(shell, &profile);
    }

    configure_shell_integration(shell, &current_posix_shell_home()?)
}

fn current_posix_shell_home() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        resolve_windows_posix_shell_home(
            env::var_os("HOME").map(PathBuf::from),
            env::var_os("USERPROFILE").map(PathBuf::from),
            |home| {
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                let output = Command::new(cygwin_tool_from_active_shell("cygpath.exe"))
                    .args(["-w", "--"])
                    .arg(home)
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                    .context("HOME uses a Unix path, but cygpath.exe could not be run")?;
                anyhow::ensure!(
                    output.status.success(),
                    "cygpath.exe could not resolve HOME: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                let home = String::from_utf8(output.stdout)
                    .context("cygpath.exe returned a home path that was not UTF-8")?;
                let home = home.trim().trim_start_matches('\u{feff}');
                anyhow::ensure!(!home.is_empty(), "cygpath.exe returned an empty home path");
                Ok(PathBuf::from(home))
            },
        )
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .context("could not locate the home directory: HOME is not set")
    }
}

#[cfg(windows)]
fn resolve_windows_posix_shell_home(
    home: Option<PathBuf>,
    user_profile: Option<PathBuf>,
    convert_unix_path: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<PathBuf> {
    match home {
        Some(home) if home.is_absolute() => Ok(home),
        Some(home) => convert_unix_path(&home),
        None => user_profile
            .context("could not locate the home directory: HOME and USERPROFILE are not set"),
    }
}

fn configure_shell_integration(
    shell: ShellIntegration,
    home: &Path,
) -> Result<ShellIntegrationConfiguration> {
    let path = shell.startup_file(home);
    #[cfg(windows)]
    let path = resolve_msys2_link_startup_file(&path, resolve_msys2_link)?;
    #[cfg(windows)]
    let path = resolve_cygwin_link_startup_file(&path)?;
    configure_shell_integration_file(shell, &path)
}

#[cfg(windows)]
fn resolve_msys2_link_startup_file(
    path: &Path,
    resolve_link: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    let mut link = path.as_os_str().to_os_string();
    link.push(".lnk");
    let link = PathBuf::from(link);
    if !link.is_file() {
        return Ok(path.to_path_buf());
    }
    resolve_link(&link)
}

#[cfg(windows)]
fn resolve_cygwin_link_startup_file(path: &Path) -> Result<PathBuf> {
    resolve_cygwin_link_startup_file_with(
        path,
        |path| path.exists(),
        |path| fs::symlink_metadata(path).is_ok(),
        resolve_cygwin_link,
    )
}

#[cfg(windows)]
fn resolve_cygwin_link_startup_file_with(
    path: &Path,
    is_accessible: impl Fn(&Path) -> bool,
    has_metadata: impl Fn(&Path) -> bool,
    resolve_link: impl FnOnce(&Path) -> Result<Option<PathBuf>>,
) -> Result<PathBuf> {
    // Cygwin stores POSIX symlinks as a reparse-point format that native
    // Windows file APIs cannot follow. `exists` is false for those paths even
    // though Cygwin can read them, while symlink_metadata still exposes the
    // reparse point without following it.
    if is_accessible(path) || !has_metadata(path) {
        return Ok(path.to_path_buf());
    }

    match resolve_link(path)? {
        Some(path) => Ok(path),
        None => Ok(path.to_path_buf()),
    }
}

#[cfg(windows)]
fn resolve_cygwin_link(path: &Path) -> Result<Option<PathBuf>> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let resolved = match Command::new(cygwin_tool_for_path_or_active_shell(path, "readlink.exe"))
        .args(["-f", "--"])
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to resolve the Cygwin startup-file link {}",
                    path.display()
                )
            });
        }
    };
    if !resolved.status.success() {
        return Ok(None);
    }
    let resolved = String::from_utf8(resolved.stdout)
        .context("readlink.exe returned a Cygwin path that was not UTF-8")?;
    let resolved = resolved.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !resolved.is_empty(),
        "readlink.exe returned an empty path for {}",
        path.display()
    );

    let native = Command::new(cygwin_tool_for_path_or_active_shell(path, "cygpath.exe"))
        .args(["-w", "--", resolved])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| {
            format!(
                "failed to convert the Cygwin startup-file link {}",
                path.display()
            )
        })?;
    anyhow::ensure!(
        native.status.success(),
        "cygpath.exe could not convert {}: {}",
        resolved,
        String::from_utf8_lossy(&native.stderr).trim()
    );
    let native = String::from_utf8(native.stdout)
        .context("cygpath.exe returned a path that was not UTF-8")?;
    let native = native.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !native.is_empty(),
        "cygpath.exe returned an empty path for {}",
        path.display()
    );
    Ok(Some(PathBuf::from(native)))
}

#[cfg(windows)]
fn resolve_msys2_link(link: &Path) -> Result<PathBuf> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let resolved = Command::new("readlink.exe")
        .args(["-f", "--"])
        .arg(link)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("failed to resolve the MSYS2 link {}", link.display()))?;
    anyhow::ensure!(
        resolved.status.success(),
        "readlink.exe could not resolve {}: {}",
        link.display(),
        String::from_utf8_lossy(&resolved.stderr).trim()
    );
    let resolved = String::from_utf8(resolved.stdout)
        .context("readlink.exe returned a path that was not UTF-8")?;
    let resolved = resolved.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !resolved.is_empty(),
        "readlink.exe returned an empty path for {}",
        link.display()
    );

    let native = Command::new("cygpath.exe")
        .args(["-w", "--", resolved])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("failed to convert the MSYS2 link {}", link.display()))?;
    anyhow::ensure!(
        native.status.success(),
        "cygpath.exe could not convert {}: {}",
        link.display(),
        String::from_utf8_lossy(&native.stderr).trim()
    );
    let native = String::from_utf8(native.stdout)
        .context("cygpath.exe returned a path that was not UTF-8")?;
    let native = native.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !native.is_empty(),
        "cygpath.exe returned an empty path for {}",
        link.display()
    );
    Ok(PathBuf::from(native))
}

fn configure_shell_integration_file(
    shell: ShellIntegration,
    path: &Path,
) -> Result<ShellIntegrationConfiguration> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };

    if shell.configuration_is_present(&contents) {
        return Ok(ShellIntegrationConfiguration::AlreadyPresent(
            path.to_path_buf(),
        ));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if let Some(migrated) = shell.migrate_configuration(&contents) {
        fs::write(path, migrated)
            .with_context(|| format!("failed to update {}", path.display()))?;
        return Ok(ShellIntegrationConfiguration::Written(path.to_path_buf()));
    }

    let separator = if contents.is_empty() || contents.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(format!("{separator}{}\n", shell.configuration_command()).as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(ShellIntegrationConfiguration::Written(path.to_path_buf()))
}

#[cfg(windows)]
fn current_powershell_profile() -> Result<PathBuf> {
    let executable =
        parent_powershell_executable().unwrap_or_else(|| PathBuf::from("powershell.exe"));
    query_powershell_profile(&executable)
}

#[cfg(windows)]
fn parent_powershell_executable() -> Option<PathBuf> {
    let mut system = sysinfo::System::new();
    let mut pid = sysinfo::get_current_pid().ok()?;

    loop {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        let parent_pid = system.process(pid)?.parent()?;
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[parent_pid]), true);
        let parent = system.process(parent_pid)?;
        if parent.exe().is_some_and(|executable| {
            executable
                .file_stem()
                .and_then(OsStr::to_str)
                .is_some_and(|name| {
                    name.eq_ignore_ascii_case("powershell") || name.eq_ignore_ascii_case("pwsh")
                })
        }) {
            return parent.exe().map(Path::to_path_buf);
        }
        pid = parent_pid;
    }
}

#[cfg(windows)]
fn query_powershell_profile(executable: &Path) -> Result<PathBuf> {
    let output = Command::new(executable)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OutputEncoding = New-Object Text.UTF8Encoding; \
             [Console]::Out.Write($PROFILE.CurrentUserCurrentHost)",
        ])
        .output()
        .with_context(|| {
            format!(
                "failed to query the PowerShell profile using {}",
                executable.display()
            )
        })?;
    anyhow::ensure!(
        output.status.success(),
        "failed to query the PowerShell profile using {}: {}",
        executable.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );

    let profile = String::from_utf8(output.stdout)
        .context("PowerShell returned a profile path that was not UTF-8")?;
    let profile = profile.trim().trim_start_matches('\u{feff}');
    anyhow::ensure!(
        !profile.is_empty(),
        "{} returned an empty PowerShell profile path",
        executable.display()
    );
    Ok(PathBuf::from(profile))
}

fn render_overlay_color_names(shell: ShellIntegration) -> String {
    let separator = if shell == ShellIntegration::PowerShell {
        ", "
    } else {
        " "
    };
    OVERLAY_COLOR_PRESETS
        .iter()
        .map(|preset| match shell {
            ShellIntegration::PowerShell => format!("'{}'", preset.name),
            ShellIntegration::Bash | ShellIntegration::Fish | ShellIntegration::Zsh => {
                preset.name.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(separator)
}

pub(crate) fn shell_integration_help() -> String {
    let supported_shells = format_help_table([
        ("bash", "Bash"),
        ("fish", "Fish"),
        ("powershell", "PowerShell (also accepted as pwsh)"),
        ("zsh", "Z shell"),
    ]);
    let help = format!(
        "Configure or generate shell integration\n\nUsage: zetta init [SHELL]\n\nWithout SHELL, detects the active supported shell process (falling back to SHELL when process inspection cannot identify it) and adds the integration command to its startup file. On Windows, Unix-style HOME paths from MSYS2 and Cygwin are resolved with cygpath; when neither an active shell nor SHELL identifies a POSIX shell, Zetta detects the launching PowerShell and writes to its $PROFILE. Running it again leaves an existing integration unchanged. With SHELL, prints the integration script for use in a shell startup file.\n\nSupported shells:\n{supported_shells}\n\nThe generated script adds completion, including dynamic profile and theme values from `zetta profile list` and `zetta profile themes`, registered project command names from `zetta cmd --list`, live serial-device, tab-icon, pane-split, pane-label, run-dependency-label, and --replace-pane completion, the root --new-window and --command options (which pass their launch behavior or remaining command arguments respectively), the attention command's notification options, the zvi shortcut for the built-in vi editor, the ztftp shortcut when the TFTP client is enabled, and the zntfy and zcopy/zpaste shortcuts when desktop notifications and clipboard access are enabled. `zetta pane --direction` completes left, right, up, and down, while `zetta pane --pane` fetches labels from the active process, and new-pane overlay sizes and colors are offered as fixed values. zcopy/zpaste are also available as pbcopy/pbpaste on platforms other than macOS, taking priority over any existing pbcopy/pbpaste alias so pbcopy/pbpaste muscle memory keeps working there too. Project commands are raw shell code and should only be used from trusted project configurations."
    );
    let worktree_help = if cfg!(feature = "worktree") {
        "\n\nThe generated integration also provides the zwt wrapper for the standalone Git worktree command; it changes directory only after successful new, done, or abort operations. Worktree completion includes new, done, abort, status, sync, and config operations, dynamic source-branch commit targets for sync, the repeatable --copy path option, and filesystem path arguments."
    } else {
        ""
    };
    format!(
        "{help}{worktree_help}\n\nProfile administration also completes the fixed icon values auto, zetta, bash, zsh, and fish."
    )
}

const BASH_INTEGRATION: &str = include_str!("shell_integration/bash.sh");
const FISH_INTEGRATION: &str = include_str!("shell_integration/fish.fish");
const POWERSHELL_INTEGRATION: &str = include_str!("shell_integration/powershell.ps1");
const ZSH_INTEGRATION: &str = include_str!("shell_integration/zsh.zsh");
#[cfg(not(windows))]
pub(crate) const ZSH_EARLY_HISTORY_INTEGRATION: &str =
    include_str!("shell_integration/zsh_history.zsh");

#[cfg(test)]
#[path = "tests/shell_integration.rs"]
mod tests;
