//! Shell startup configuration and rendering of startup and completion payloads.

use super::*;
use crate::startup::format_help_table;
use std::ffi::OsStr;
use std::io::Write as _;
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
#[cfg(windows)]
use std::process::Command;

mod command_tree;
#[cfg(windows)]
mod windows_shells;
#[cfg(windows)]
use windows_shells::*;

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
        self.render_script(false)
    }

    pub(crate) fn completion_script(self) -> String {
        self.render_script(true)
    }

    #[cfg(test)]
    fn eager_script(self) -> String {
        // Existing regression drivers exercise the implementation directly.
        let initialize = if self == Self::Zsh {
            "if ! (( $+functions[compdef] )); then autoload -Uz compinit; compinit; fi\n"
        } else {
            ""
        };
        // Fish adds a second `complete -c` source rather than replacing the
        // first, so the lazy loader would stay registered beside the code under
        // test and source whatever `zetta init fish --completions` on PATH
        // prints: the installed binary's completions, not these.
        let loaded = if self == Self::Fish {
            "set -g __zetta_completions_loaded 1\n"
        } else {
            ""
        };
        format!(
            "{loaded}{}\n{initialize}{}",
            self.script(),
            self.completion_script()
        )
    }

    fn render_script(self, completions: bool) -> String {
        let template = match (self, completions) {
            (Self::Bash, false) => BASH_INTEGRATION,
            (Self::Fish, false) => FISH_INTEGRATION,
            (Self::PowerShell, false) => POWERSHELL_INTEGRATION,
            (Self::Zsh, false) => ZSH_INTEGRATION,
            (Self::Bash, true) => include_str!("shell_integration/bash_completions.sh"),
            (Self::Fish, true) => include_str!("shell_integration/fish_completions.fish"),
            (Self::PowerShell, true) => {
                include_str!("shell_integration/powershell_completions.ps1")
            }
            (Self::Zsh, true) => include_str!("shell_integration/zsh_completions.zsh"),
        };
        let script = render_integration(template).replace(
            "__ZETTA_LIFECYCLE_VERSION__",
            &zetta_profiles::ZSH_LIFECYCLE_TRACKING_VERSION.to_string(),
        );
        if completions {
            command_tree::expand(self, &script)
        } else {
            command_tree::expand_lazy(self, &script)
        }
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

/// The optional sections of a template, by their begin and end markers, and
/// whether this build keeps each one.
const fn integration_sections() -> [(&'static str, &'static str, bool); 5] {
    [
        (
            "ZETTA_WORKTREE_INTEGRATION_BEGIN",
            "ZETTA_WORKTREE_INTEGRATION_END",
            cfg!(feature = "worktree"),
        ),
        (
            "ZETTA_ZMUX_INTEGRATION_BEGIN",
            "ZETTA_ZMUX_INTEGRATION_END",
            cfg!(feature = "zmux"),
        ),
        (
            "ZETTA_PAGER_INTEGRATION_BEGIN",
            "ZETTA_PAGER_INTEGRATION_END",
            cfg!(feature = "zpager"),
        ),
        (
            "ZETTA_CLIPBOARD_INTEGRATION_BEGIN",
            "ZETTA_CLIPBOARD_INTEGRATION_END",
            cfg!(feature = "clipboard"),
        ),
        (
            "ZETTA_ZOSH_INTEGRATION_BEGIN",
            "ZETTA_ZOSH_INTEGRATION_END",
            cfg!(feature = "zosh-client"),
        ),
    ]
}

fn render_integration(template: &str) -> String {
    let sections = integration_sections();
    let mut open = sections.map(|_| false);
    let mut rendered = String::with_capacity(template.len());
    'lines: for line in template.split_inclusive('\n') {
        for (index, (begin, end, _)) in sections.iter().enumerate() {
            if line.contains(begin) || line.contains(end) {
                open[index] = line.contains(begin);
                continue 'lines;
            }
        }
        if sections
            .iter()
            .zip(open)
            .any(|((_, _, built), open)| open && !built)
        {
            continue;
        }
        rendered.push_str(line);
    }
    rendered
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

pub(crate) fn shell_integration_help() -> String {
    let supported_shells = format_help_table([
        ("bash", "Bash"),
        ("fish", "Fish"),
        ("powershell", "PowerShell (also accepted as pwsh)"),
        ("zsh", "Z shell"),
    ]);
    let options = format_help_table([
        (
            "-c, --completions",
            "Emit completion implementation (requires SHELL)",
        ),
        ("-h, --help", "Print help"),
    ]);
    let help = format!(
        "Configure or generate shell integration\n\nUsage: zetta init [SHELL] [OPTIONS]\n\nWithout SHELL, detects the active supported shell process (falling back to SHELL when process inspection cannot identify it) and adds the integration command to its startup file. On Windows, Unix-style HOME paths from MSYS2 and Cygwin are resolved with cygpath; when neither an active shell nor SHELL identifies a POSIX shell, Zetta detects the launching PowerShell and writes to its $PROFILE. Running it again leaves an existing integration unchanged. With SHELL, prints the integration script for use in a shell startup file.\n\nOptions:\n{options}\n\nCompletion code loads once on the first request. That request includes generation and evaluation cost; later requests reuse the code while refreshing dynamic values. Zsh also defers automatic compinit until completion.\n\nSupported shells:\n{supported_shells}\n\nThe generated script adds completion, including dynamic profile and theme values from `zetta profile list` and `zetta profile themes`, registered project command names from `zetta cmd --list`, live serial-device, tab-icon (including `zetta tabicon --reset`), pane-split, pane-label, run-dependency-label, and --replace-pane completion, the root --new-window and --command options (which pass their launch behavior or remaining command arguments respectively), the attention command's notification options, the zvi shortcut for the built-in vi editor, ztftp client and server completions, zhttp server completions, and the zntfy shortcut when desktop notifications are enabled, and zcopy/zpaste completions when clipboard access is enabled. `zetta pane --direction` completes left, right, up, and down, while `zetta pane --pane` fetches labels from the active process, and new-pane overlay sizes and colors are offered as fixed values. Past the `--` in `zetta pane wait ... -- COMMAND`, completion hands off to COMMAND's own shell completion exactly as if `zetta pane wait ... --` were not on the command line. zcopy/zpaste are also available as pbcopy/pbpaste on platforms other than macOS, taking priority over any existing pbcopy/pbpaste alias so pbcopy/pbpaste muscle memory keeps working there too. Project commands are raw shell code and should only be used from trusted project configurations."
    );
    let worktree_help = if cfg!(feature = "worktree") {
        "\n\nThe generated integration also provides the zwt wrapper for the standalone Git worktree command; it changes directory only after successful new, done, or abort operations. Worktree completion includes new, done, abort, status, sync, and config operations, dynamic source-branch commit targets for sync, the repeatable --copy path option, and filesystem path arguments."
    } else {
        ""
    };
    let pager_help = if cfg!(feature = "zpager") {
        " The zpager Git diff pager sets GIT_PAGER=zpager and completes its columns, width, color, and help options."
    } else {
        ""
    };
    let mosh_help = "\n\nThe generated integration also defines a mosh wrapper only when no real mosh command, alias, function, or builtin already exists; it routes through zetta mosh and completes Mosh options and SSH targets. The bundled zosh command is the full Mosh-compatible launcher and terminal client, with the same option surface available through zosh --help.";
    format!(
        "{help}{mosh_help}{worktree_help}{pager_help}\n\nProfile administration also completes the fixed icon values auto, zetta, bash, zsh, and fish."
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
