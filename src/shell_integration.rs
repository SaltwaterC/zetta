use super::*;
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

    pub(crate) fn script(self, profiles: &[Profile]) -> String {
        let template = match self {
            Self::Bash => BASH_INTEGRATION,
            Self::Fish => FISH_INTEGRATION,
            Self::PowerShell => POWERSHELL_INTEGRATION,
            Self::Zsh => ZSH_INTEGRATION,
        };
        template.replace("ZETTA_PROFILES", &render_profiles(self, profiles))
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
        #[cfg(not(windows))]
        let active_shell_path = active_posix_shell_path();
        #[cfg(windows)]
        let active_shell_path: Option<PathBuf> = None;

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

#[cfg(not(windows))]
fn active_posix_shell_path() -> Option<PathBuf> {
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
                let output = Command::new("cygpath.exe")
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

fn render_profiles(shell: ShellIntegration, profiles: &[Profile]) -> String {
    let separator = if shell == ShellIntegration::PowerShell {
        ", "
    } else {
        " "
    };
    profiles
        .iter()
        .map(|profile| match shell {
            ShellIntegration::Bash | ShellIntegration::Zsh | ShellIntegration::Fish => {
                shell_single_quote(&profile.name)
            }
            ShellIntegration::PowerShell => format!("'{}'", profile.name.replace('\'', "''")),
        })
        .collect::<Vec<_>>()
        .join(separator)
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn shell_integration_help() -> &'static str {
    "Configure or generate shell integration\n\nUsage: zetta init [SHELL]\n\nWithout SHELL, detects the active supported shell process (falling back to SHELL when process inspection cannot identify it) and adds the integration command to its startup file. On Windows, Unix-style HOME paths from MSYS2 and similar environments are resolved with cygpath; when SHELL is unavailable, Zetta detects the launching PowerShell and writes to its $PROFILE. Running it again leaves an existing integration unchanged. With SHELL, prints the integration script for use in a shell startup file.\n\nSupported shells:\n  bash        Bash\n  fish        Fish\n  powershell  PowerShell (also accepted as pwsh)\n  zsh         Z shell\n\nThe generated script adds command completion, including live serial-device, tab-icon, and pane-theme completion, the zvi shortcut for the built-in vi editor, the ztftp shortcut when the TFTP client is enabled, and the zntfy and zcopy/zpaste shortcuts when desktop notifications and clipboard access are enabled. zcopy/zpaste are also available as pbcopy/pbpaste on platforms other than macOS, taking priority over any existing pbcopy/pbpaste alias so pbcopy/pbpaste muscle memory keeps working there too."
}

const BASH_INTEGRATION: &str = r#"# Zetta shell integration for Bash.
if [[ -z ${EDITOR+x} ]]; then
    export EDITOR='zetta vi'
fi

if ! type -t vi >/dev/null 2>&1; then
    eval 'vi() { command zetta vi "$@"; }'
    complete -F _zetta_complete vi
fi

zvi() { command zetta vi "$@"; }

_zetta_option_used() {
    local option=$1 index
    for (( index = 1; index < COMP_CWORD; index++ )); do
        [[ ${COMP_WORDS[index]} == "$option" ]] && return 0
    done
    return 1
}

_zetta_compgen() {
    local options=$1 candidate
    local -a available=()
    for candidate in $options; do
        if [[ $candidate != -* ]] || ! _zetta_option_used "$candidate"; then
            available+=("$candidate")
        fi
    done
    COMPREPLY=( $(compgen -W "${available[*]}" -- "$current") )
}

_zetta_complete() {
    local current previous command
    local -a profiles=(ZETTA_PROFILES)
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}
    command=${COMP_WORDS[1]}

    if [[ ${COMP_WORDS[0]} == vi || ${COMP_WORDS[0]} == zvi ]]; then
        if [[ $current == -* ]]; then
            _zetta_compgen '--help'
        else
            COMPREPLY=( $(compgen -f -- "$current") )
        fi
        return
    fi

    _zetta_complete_profiles() {
        COMPREPLY=()
        local profile
        for profile in "${profiles[@]}"; do
            [[ $profile == "$current"* ]] && COMPREPLY+=("$profile")
        done
    }

    _zetta_complete_session_ids() {
        COMPREPLY=()
        local session_id
        while IFS= read -r session_id; do
            [[ $session_id == "$current"* ]] && COMPREPLY+=("$session_id")
        done < <(zetta sessions --json 2>/dev/null | awk '
            /"process_id"[[:space:]]*:/ { match($0, /[0-9]+/); process=substr($0, RSTART, RLENGTH) }
            /"runner_id"[[:space:]]*:/ { match($0, /[0-9]+/); runner=substr($0, RSTART, RLENGTH) }
            /"id"[[:space:]]*:/ { match($0, /[0-9]+/); session=substr($0, RSTART, RLENGTH) }
            /"authentication_required"[[:space:]]*:/ { print process ":" runner ":" session }
        ')
    }

    _zetta_complete_tab_icons() {
        local icons
        icons=$(zetta tabicon --list 2>/dev/null)
        COMPREPLY=( $(compgen -W "$icons" -- "$current") )
    }

    _zetta_complete_pane_themes() {
        COMPREPLY=()
        local theme
        while IFS= read -r theme; do
            [[ $theme == "$current"* ]] && COMPREPLY+=("$theme")
        done < <(zetta panetheme --list 2>/dev/null)
    }

    case "$previous" in
        --profile)
            _zetta_complete_profiles
            return
            ;;
        -p)
            if [[ $command == serial ]]; then
                _zetta_compgen 'none odd even'
            elif [[ $command != tftp && $command != http && $command != notify ]]; then
                _zetta_complete_profiles
            else
                COMPREPLY=()
            fi
            return
            ;;
        --root)
            COMPREPLY=( $(compgen -d -- "$current") )
            return
            ;;
        --device)
            _zetta_complete_serial_devices
            return
            ;;
        -d)
            if [[ $command == serial ]]; then
                _zetta_complete_serial_devices
            else
                COMPREPLY=()
            fi
            return
            ;;
        --data-bits|-D)
            if [[ $command == serial ]]; then
                _zetta_compgen '5 6 7 8'
            else
                COMPREPLY=()
            fi
            return
            ;;
        --parity)
            _zetta_compgen 'none odd even'
            return
            ;;
        --stop-bits|-s)
            if [[ $command == serial ]]; then
                _zetta_compgen '1 2'
            elif [[ $command == notify ]]; then
                _zetta_complete_sound_names
            else
                COMPREPLY=()
            fi
            return
            ;;
        --flow-control|-f)
            _zetta_compgen 'none software hardware'
            return
            ;;
        --pboard|-pboard)
            _zetta_compgen 'general ruler find font'
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            _zetta_compgen 'txt rtf ps'
            return
            ;;
        --app-name|-a)
            COMPREPLY=()
            return
            ;;
        --icon|-i)
            if [[ $command == tabicon ]]; then
                _zetta_complete_tab_icons
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        --sound)
            _zetta_complete_sound_names
            return
            ;;
        --timeout)
            _zetta_compgen 'default never'
            return
            ;;
        --config|--keymap|-k|--profile-report)
            COMPREPLY=( $(compgen -f -- "$current") )
            return
            ;;
        -c)
            if [[ $command == terminal-size ]]; then
                COMPREPLY=()
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        -r)
            if [[ $command == http || ( $command == tftp && ${COMP_WORDS[2]} == server ) ]]; then
                COMPREPLY=( $(compgen -d -- "$current") )
            elif [[ $command == terminal-size ]]; then
                COMPREPLY=()
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        --output-type|-t|--theme)
            if [[ $command == panetheme || $command == -* ]]; then
                _zetta_complete_pane_themes
            elif [[ $command == notify ]]; then
                _zetta_compgen 'default never'
            else
                _zetta_compgen 'repeated unique'
            fi
            return
            ;;
        --port|-p|--baud-rate|-b|--size|--profile-duration|--columns|--rows|-R)
            COMPREPLY=()
            return
            ;;
    esac

    if (( COMP_CWORD == 1 )); then
        _zetta_compgen 'benchmark benchmark-output terminal-size sessions edit vi init serial http tftp notify copy paste tabicon panetheme --help --version --config --keymap --profile --theme'
        return
    fi

    # A leading flag rules out a subcommand for the rest of the command line
    # (subcommands are only recognized as the first argument), so keep
    # offering the remaining top-level flags instead of falling through to
    # the subcommand-specific cases below, which would offer nothing.
    if [[ $command == -* ]]; then
        _zetta_compgen '--help --version --config --keymap --profile --theme'
        return
    fi

    case "$command" in
        benchmark)
            _zetta_compgen '--terminal-render-workload --terminal-checkerboard-workload --terminal-sparse-update-workload --profile-report --profile-duration --profile-pane-stress --profile-background-stress --profile-sparse-updates --profile-external-terminal --help'
            ;;
        benchmark-output)
            _zetta_compgen '--size --output-type --help'
            ;;
        terminal-size)
            _zetta_compgen '--json --resize --columns --rows --help'
            ;;
        edit)
            if [[ $current == -* ]]; then
                _zetta_compgen '--delete-after --help'
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            ;;
        vi)
            if [[ $current == -* ]]; then
                _zetta_compgen '--help'
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            ;;
        sessions)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'reconnect --json --help'
            elif [[ ${COMP_WORDS[2]} == reconnect ]]; then
                if [[ $previous == --session || $previous == -s ]]; then
                    COMPREPLY=()
                elif (( COMP_CWORD == 3 )); then
                    _zetta_complete_session_ids
                else
                    _zetta_compgen '--session --help'
                fi
            else
                _zetta_compgen '--json --help'
            fi
            ;;
        init)
            _zetta_compgen 'bash fish powershell pwsh zsh --help'
            ;;
        serial)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'console list --help'
            elif [[ ${COMP_WORDS[2]} == console ]]; then
                _zetta_compgen '--device --baud-rate --data-bits --parity --stop-bits --flow-control --help'
            fi
            ;;
        http)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'server --help'
            else
                _zetta_compgen '--root --port --config --help'
            fi
            ;;
        tftp)
            _zetta_tftp_complete 2
            ;;
        notify)
            _zetta_compgen '--app-name --icon --sound --timeout --help'
            ;;
        copy)
            _zetta_compgen '--pboard --help'
            ;;
        paste)
            _zetta_compgen '--pboard --prefer --help'
            ;;
        tabicon)
            if [[ $current == -* ]]; then
                _zetta_compgen '--icon --list --help'
            else
                _zetta_complete_tab_icons
            fi
            ;;
        panetheme)
            if [[ $current == -* ]]; then
                _zetta_compgen '--theme --reset --list --help'
            else
                _zetta_complete_pane_themes
            fi
            ;;
    esac
}

_zetta_tftp_complete() {
    local operation_index=$1 current previous operation argument
    local index positional=0 skip_port=0
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}

    if (( COMP_CWORD == operation_index )); then
        _zetta_compgen 'get put server --help'
        return
    fi
    operation=${COMP_WORDS[operation_index]}
    if [[ $operation == server ]]; then
        if [[ $current == -* || -z $current ]]; then
            _zetta_compgen '--root --port --config --help'
        fi
        return
    fi
    if [[ $current == -* ]]; then
        _zetta_compgen '--port --help'
        return
    fi
    if [[ $previous == '--port' || $previous == '-p' ]]; then
        COMPREPLY=()
        return
    fi

    for (( index = operation_index + 1; index < COMP_CWORD; index++ )); do
        argument=${COMP_WORDS[index]}
        if (( skip_port )); then
            skip_port=0
        elif [[ $argument == '--port' || $argument == '-p' ]]; then
            skip_port=1
        elif [[ $argument != -* ]]; then
            (( positional++ ))
        fi
    done

    case $operation in
        put)
            (( positional == 1 )) && COMPREPLY=( $(compgen -f -- "$current") )
            ;;
    esac
}

_zetta_complete_serial_devices() {
    local devices
    devices=$(zetta serial list 2>/dev/null)
    COMPREPLY=( $(compgen -W "$devices" -- "$current") )
}

# zetta-default/zetta-ok/zetta-alarm are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered.
_zetta_complete_sound_names() {
    local platform_sounds
    case "$OSTYPE" in
        darwin*)
            platform_sounds='Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink'
            ;;
        msys*|cygwin*|win32*)
            platform_sounds='Default IM Mail Reminder SMS'
            ;;
        *)
            platform_sounds='bell complete message message-new-instant dialog-information dialog-warning dialog-error trash-empty'
            ;;
    esac
    COMPREPLY=( $(compgen -W "zetta-default zetta-ok zetta-alarm $platform_sounds" -- "$current") )
}

_ztftp_complete() {
    _zetta_tftp_complete 1
}

_zntfy_complete() {
    local current previous
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}

    case "$previous" in
        --app-name|-a)
            COMPREPLY=()
            return
            ;;
        --icon|-i)
            COMPREPLY=( $(compgen -f -- "$current") )
            return
            ;;
        --sound|-s)
            _zetta_complete_sound_names
            return
            ;;
        --timeout|-t)
            COMPREPLY=( $(compgen -W 'default never' -- "$current") )
            return
            ;;
    esac
    _zetta_compgen '--app-name --icon --sound --timeout --help'
}

_zcopy_complete() {
    local current=${COMP_WORDS[COMP_CWORD]} previous=${COMP_WORDS[COMP_CWORD-1]}
    case "$previous" in
        --pboard|-pboard)
            _zetta_compgen 'general ruler find font'
            return
            ;;
    esac
    _zetta_compgen '--pboard --help'
}

_zpaste_complete() {
    local current=${COMP_WORDS[COMP_CWORD]} previous=${COMP_WORDS[COMP_CWORD-1]}
    case "$previous" in
        --pboard|-pboard)
            _zetta_compgen 'general ruler find font'
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            _zetta_compgen 'txt rtf ps'
            return
            ;;
    esac
    _zetta_compgen '--pboard --prefer --help'
}

ztftp() { zetta tftp "$@"; }
zntfy() { zetta notify "$@"; }
zcopy() { zetta copy "$@"; }
zpaste() { zetta paste "$@"; }
complete -F _zetta_complete zetta
complete -F _zetta_complete zvi
complete -F _ztftp_complete ztftp
complete -F _zntfy_complete zntfy
complete -F _zcopy_complete zcopy
complete -F _zpaste_complete zpaste

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone there.
# Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working; any
# preexisting pbcopy/pbpaste alias (eg. one pointing at xclip) is removed
# first so Zetta's functions take priority over it.
case "$OSTYPE" in
    darwin*) ;;
    *)
        unalias pbcopy pbpaste 2>/dev/null
        pbcopy() { zetta copy "$@"; }
        pbpaste() { zetta paste "$@"; }
        complete -F _zcopy_complete pbcopy
        complete -F _zpaste_complete pbpaste
        ;;
esac
"#;

const FISH_INTEGRATION: &str = r#"# Zetta shell integration for Fish.
if not set -q EDITOR
    set -gx EDITOR 'zetta vi'
end

if not type -q vi
    if not abbr --query vi
        function vi --wraps 'zetta vi' --description 'Zetta vi editor'
            command zetta vi $argv
        end
        complete -c vi -F
    end
end

function zvi --wraps 'zetta vi' --description 'Zetta vi editor'
    command zetta vi $argv
end
complete -c zvi -F

function ztftp --wraps 'zetta tftp' --description 'Zetta TFTP client'
    zetta tftp $argv
end

function zntfy --wraps 'zetta notify' --description 'Zetta desktop notifications'
    zetta notify $argv
end

function zcopy --wraps 'zetta copy' --description 'Copy standard input to the clipboard'
    zetta copy $argv
end

function zpaste --wraps 'zetta paste' --description "Print the clipboard's contents"
    zetta paste $argv
end

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste function or abbreviation is erased first so
# Zetta's functions take priority over it.
switch (uname)
    case Darwin
    case '*'
        functions -e pbcopy pbpaste 2>/dev/null
        function pbcopy --wraps 'zetta copy' --description 'Copy standard input to the clipboard'
            zetta copy $argv
        end
        function pbpaste --wraps 'zetta paste' --description "Print the clipboard's contents"
            zetta paste $argv
        end
end

function __zetta_profiles
    printf '%s\n' ZETTA_PROFILES
end

function __zetta_serial_devices
    zetta serial list 2>/dev/null
end

function __zetta_tab_icons
    zetta tabicon --list 2>/dev/null
end

function __zetta_pane_themes
    zetta panetheme --list 2>/dev/null
end

# zetta-default/zetta-ok/zetta-alarm are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered.
function __zetta_sound_names
    switch (uname)
        case Darwin
            printf '%s\n' zetta-default zetta-ok zetta-alarm \
                Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink
        case '*'
            printf '%s\n' zetta-default zetta-ok zetta-alarm bell complete message \
                message-new-instant dialog-information dialog-warning dialog-error trash-empty
    end
end

# Fish completion registrations normally expose every short option as a
# candidate. Keep the short forms out of the candidate list while retaining
# their argument completion by activating these registrations only after the
# user has already entered the short option.
function __zetta_short_option
    set -l words (commandline -opc)
    test (count $words) -gt 0
    and test "$words[-1]" = "$argv[1]"
end

function __zetta_at_subcommand
    set -l words (commandline -opc)
    test (count $words) -eq 2
    and test "$words[2]" = "$argv[1]"
end

# A subcommand is only recognized as the very first argument, unlike root
# flags (--profile, --theme, --config, --keymap), which may combine and
# appear in any order. Subcommand-name candidates use this instead of
# __zetta_use_subcommand so they stop appearing once a root flag is typed.
function __zetta_at_root
    test (count (commandline -opc)) -eq 1
end

# Fish's own __fish_use_subcommand treats any non-flag token as a subcommand,
# so it stops offering root flags after a value-taking one (e.g. --profile
# NAME) even though no subcommand was actually given. Skip known root option
# arguments before applying that same rule, so --profile and --theme keep
# completing each other despite --theme requiring --profile.
function __zetta_use_subcommand
    set -l words (commandline -opc)
    set -e words[1]
    set -l skip_next 0
    for word in $words
        if test $skip_next -eq 1
            set skip_next 0
            continue
        end
        switch $word
            case --config -c --keymap -k --profile -p --theme -t
                set skip_next 1
                continue
            case '-*'
                continue
        end
        return 1
    end
    return 0
end

function __zetta_tftp_client
    set -l words (commandline -opc)
    test (count $words) -ge 3
    and test "$words[2]" = tftp
    and contains -- "$words[3]" get put
end

function __zetta_tftp_server
    set -l words (commandline -opc)
    test (count $words) -ge 3
    and test "$words[2]" = tftp
    and test "$words[3]" = server
end

function __zetta_session_ids
    zetta sessions --json 2>/dev/null | awk '
        /"process_id"[[:space:]]*:/ { match($0, /[0-9]+/); process=substr($0, RSTART, RLENGTH) }
        /"runner_id"[[:space:]]*:/ { match($0, /[0-9]+/); runner=substr($0, RSTART, RLENGTH) }
        /"id"[[:space:]]*:/ { match($0, /[0-9]+/); session=substr($0, RSTART, RLENGTH) }
        /"authentication_required"[[:space:]]*:/ { print process ":" runner ":" session }
    '
end

# Fish only considers options registered with `-l` after the user has typed a
# dash. Emit the same long options as ordinary completion candidates too, so
# they appear alongside subcommands at every valid argument position.
function __zetta_option_unused
    set -l words (commandline -opc)
    not contains -- $argv[1] $words[2..-1]
end

function __zetta_filter_long_options
    while read -l line
        set -l option (string split \t -- $line)[1]
        if __zetta_option_unused $option
            printf '%s\n' "$line"
        end
    end
end

function __zetta_long_options
    begin
        switch $argv[1]
        case root
            printf '%s\t%s\n' \
                --help 'Print help' \
                --version 'Print version' \
                --config 'Use a configuration file' \
                --keymap 'Use a keymap file' \
                --profile 'Select a profile' \
                --theme 'Non-persistently override the profile theme'
        case init serial http tftp
            printf '%s\t%s\n' --help 'Print help'
        case panetheme
            printf '%s\t%s\n' \
                --theme 'Set the pane theme' \
                --reset 'Restore the profile-configured theme' \
                --list 'Print the registered theme names' \
                --help 'Print help'
        case tabicon
            printf '%s\t%s\n' \
                --icon 'Set the tab icon' \
                --list 'Print built-in icon names' \
                --help 'Print help'
        case terminal-size
            printf '%s\t%s\n' \
                --json 'Print machine-readable JSON' \
                --resize 'Resize the current pane' \
                --columns 'Set pane width in columns' \
                --rows 'Set pane height in rows' \
                --help 'Print help'
        case edit
            printf '%s\t%s\n' --delete-after 'Delete a managed buffer after editing' --help 'Print help'
        case vi
            printf '%s\t%s\n' --help 'Print help'
        case sessions
            printf '%s\t%s\n' --json 'Print machine-readable JSON' --help 'Print help'
        case benchmark-output
            printf '%s\t%s\n' \
                --size 'Set the output size in MiB' \
                --output-type 'Select repeated or unique lines' \
                --help 'Print help'
        case benchmark
            printf '%s\n' \
                --terminal-render-workload \
                --terminal-checkerboard-workload \
                --terminal-sparse-update-workload \
                --profile-report \
                --profile-duration \
                --profile-pane-stress \
                --profile-background-stress \
                --profile-sparse-updates \
                --profile-external-terminal \
                --help
        case serial-console
            printf '%s\t%s\n' \
                --device 'Serial device' \
                --baud-rate 'Baud rate' \
                --data-bits 'Data bits' \
                --parity 'Parity' \
                --stop-bits 'Stop bits' \
                --flow-control 'Flow control' \
                --help 'Print help'
        case http-server tftp-server
            printf '%s\t%s\n' \
                --root 'Directory to serve' \
                --port 'Server port' \
                --config 'Configuration file' \
                --help 'Print help'
        case tftp-client ztftp
            printf '%s\t%s\n' --port 'Server port' --help 'Print help'
        case notify zntfy
            printf '%s\t%s\n' \
                --app-name 'Application name' \
                --icon 'Image to show with the notification' \
                --sound 'Sound name' \
                --timeout 'Timeout' \
                --help 'Print help'
        case copy zcopy pbcopy
            printf '%s\t%s\n' --pboard 'Pasteboard to use' --help 'Print help'
        case paste zpaste pbpaste
            printf '%s\t%s\n' \
                --pboard 'Pasteboard to use' \
                --prefer 'Preferred clipboard format' \
                --help 'Print help'
        end
    end | __zetta_filter_long_options
end

complete -c zetta -f
complete -c zetta -n '__zetta_at_root' -a benchmark -d 'Profile terminal rendering'
complete -c zetta -n '__zetta_at_root' -a benchmark-output -d 'Write and time a text payload'
complete -c zetta -n '__zetta_at_root' -a terminal-size -d 'Print the current terminal size'
complete -c zetta -n '__zetta_at_root' -a sessions -d 'List detached background sessions'
complete -c zetta -n '__zetta_at_root' -a edit -d 'Edit files with EDITOR or Zetta vi'
complete -c zetta -n '__zetta_at_root' -a vi -d "Edit files with Zetta's built-in vi"
complete -c zetta -n '__zetta_at_root' -a init -d 'Generate shell integration'
complete -c zetta -n '__zetta_at_root' -a serial -d 'List or connect to serial devices'
complete -c zetta -n '__zetta_at_root' -a http -d 'Serve static files over HTTP'
complete -c zetta -n '__zetta_at_root' -a tftp -d 'Transfer a file with TFTP'
complete -c zetta -n '__zetta_at_root' -a notify -d 'Show a desktop notification'
complete -c zetta -n '__zetta_at_root' -a copy -d 'Copy standard input to the clipboard'
complete -c zetta -n '__zetta_at_root' -a paste -d "Print the clipboard's contents"
complete -c zetta -n '__zetta_at_root' -a tabicon -d 'Set the active tab icon'
complete -c zetta -n '__zetta_at_root' -a panetheme -d "Non-persistently change the active pane's theme"
complete -c zetta -n '__zetta_use_subcommand' -l help -d 'Print help'
complete -c zetta -n '__zetta_use_subcommand' -l version -d 'Print version'
complete -c zetta -n '__zetta_use_subcommand' -l config -r -d 'Use a configuration file'
complete -c zetta -n '__zetta_use_subcommand' -l keymap -r -d 'Use a keymap file'
complete -c zetta -n '__zetta_use_subcommand' -l profile -r -a '(__zetta_profiles)' -d 'Select a profile'
complete -c zetta -n '__zetta_use_subcommand' -l theme -r -a '(__zetta_pane_themes)' -d 'Non-persistently override the profile theme'
complete -c zetta -n '__zetta_use_subcommand' -a '(__zetta_long_options root)'
complete -c zetta -s c -r -n '__zetta_use_subcommand; and __zetta_short_option -c'
complete -c zetta -s k -r -n '__zetta_use_subcommand; and __zetta_short_option -k'
complete -c zetta -s p -r -a '(__zetta_profiles)' -n '__zetta_use_subcommand; and __zetta_short_option -p'
complete -c zetta -s t -r -a '(__zetta_pane_themes)' -n '__zetta_use_subcommand; and __zetta_short_option -t'
complete -c zetta -n '__zetta_at_subcommand init' -a 'bash fish powershell pwsh zsh'
complete -c zetta -n '__fish_seen_subcommand_from init' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from init' -a '(__zetta_long_options init)'
complete -c zetta -n '__zetta_at_subcommand serial' -a 'console list'
complete -c zetta -n '__fish_seen_subcommand_from serial' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from serial' -a '(__zetta_long_options serial)'
complete -c zetta -n '__zetta_at_subcommand http' -a server
complete -c zetta -n '__fish_seen_subcommand_from http' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from http' -a '(__zetta_long_options http)'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l json -d 'Print machine-readable JSON'
complete -c zetta -n '__fish_seen_subcommand_from sessions' -l json -d 'Print machine-readable JSON'
complete -c zetta -n '__zetta_at_subcommand sessions' -a reconnect -d 'Reconnect a detached session'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l resize -d 'Resize the current pane'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l columns -r -d 'Set pane width in columns'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l rows -r -d 'Set pane height in rows'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -a '(__zetta_long_options terminal-size)'
complete -c zetta -s c -r -n '__fish_seen_subcommand_from terminal-size; and __zetta_short_option -c'
complete -c zetta -s R -r -n '__fish_seen_subcommand_from terminal-size; and __zetta_short_option -R'
complete -c zetta -n '__fish_seen_subcommand_from sessions' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from sessions' -a '(__zetta_long_options sessions)'
complete -c zetta -n '__fish_seen_subcommand_from sessions; and __fish_seen_subcommand_from reconnect' -a '(__zetta_session_ids)'
complete -c zetta -n '__fish_seen_subcommand_from sessions; and __fish_seen_subcommand_from reconnect' -l session -r -d 'Session ID to reconnect'
complete -c zetta -n '__fish_seen_subcommand_from edit' -l delete-after -d 'Delete a managed buffer after editing'
complete -c zetta -n '__fish_seen_subcommand_from edit' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from edit' -a '(__zetta_long_options edit)'
complete -c zetta -n '__fish_seen_subcommand_from edit' -F
complete -c zetta -s d -n '__fish_seen_subcommand_from edit; and __zetta_short_option -d'
complete -c zetta -n '__fish_seen_subcommand_from vi' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from vi' -a '(__zetta_long_options vi)'
complete -c zetta -n '__fish_seen_subcommand_from vi' -F
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l size -r -d 'Set the output size in MiB'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l output-type -r -a 'repeated unique'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -a '(__zetta_long_options benchmark-output)'
complete -c zetta -s s -r -n '__fish_seen_subcommand_from benchmark-output; and __zetta_short_option -s'
complete -c zetta -s t -r -a 'repeated unique' -n '__fish_seen_subcommand_from benchmark-output; and __zetta_short_option -t'
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l terminal-render-workload
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l terminal-checkerboard-workload
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l terminal-sparse-update-workload
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-report -r
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-duration -r
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-pane-stress
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-background-stress
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-sparse-updates
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-external-terminal
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -a '(__zetta_long_options benchmark)'
complete -c zetta -s r -r -n '__fish_seen_subcommand_from benchmark; and __zetta_short_option -r'
complete -c zetta -s d -r -n '__fish_seen_subcommand_from benchmark; and __zetta_short_option -d'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l device -r -a '(__zetta_serial_devices)' -d 'Serial device'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l baud-rate -r -d 'Baud rate'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l data-bits -r -a '5 6 7 8'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l parity -r -a 'none odd even'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l stop-bits -r -a '1 2'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l flow-control -r -a 'none software hardware'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -a '(__zetta_long_options serial-console)'
complete -c zetta -s d -r -a '(__zetta_serial_devices)' -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -d'
complete -c zetta -s b -r -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -b'
complete -c zetta -s D -r -a '5 6 7 8' -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -D'
complete -c zetta -s p -r -a 'none odd even' -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -p'
complete -c zetta -s s -r -a '1 2' -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -s'
complete -c zetta -s f -r -a 'none software hardware' -n '__fish_seen_subcommand_from serial; and __zetta_short_option -f'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l root -r -a '(__fish_complete_directories)' -d 'Directory to serve'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l port -r -d 'TCP port'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l config -r -d 'Configuration file'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -a '(__zetta_long_options http-server)'
complete -c zetta -s r -r -a '(__fish_complete_directories)' -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server; and __zetta_short_option -r'
complete -c zetta -s p -r -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server; and __zetta_short_option -p'
complete -c zetta -s c -r -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server; and __zetta_short_option -c'
complete -c zetta -n '__zetta_at_subcommand tftp' -a 'get put server'
complete -c zetta -n '__zetta_tftp_client' -l port -r -d 'Server port'
complete -c zetta -n '__zetta_tftp_server' -l root -r -a '(__fish_complete_directories)' -d 'Directory to serve'
complete -c zetta -n '__zetta_tftp_server' -l config -r -d 'Configuration file'
complete -c zetta -n '__fish_seen_subcommand_from tftp' -l help -d 'Print help'
complete -c zetta -n '__zetta_at_subcommand tftp' -a '(__zetta_long_options tftp)'
complete -c zetta -n '__zetta_tftp_client' -a '(__zetta_long_options tftp-client)'
complete -c zetta -n '__zetta_tftp_server' -a '(__zetta_long_options tftp-server)'
complete -c zetta -s p -r -n '__zetta_tftp_client; and __zetta_short_option -p'
complete -c zetta -s r -r -a '(__fish_complete_directories)' -n '__zetta_tftp_server; and __zetta_short_option -r'
complete -c zetta -s p -r -n '__zetta_tftp_server; and __zetta_short_option -p'
complete -c zetta -s c -r -n '__zetta_tftp_server; and __zetta_short_option -c'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l app-name -r -d 'Application name'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l icon -r -d 'Image to show with the notification'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -l icon -r -a '(__zetta_tab_icons)' -d 'Set the tab icon'
complete -c zetta -s i -r -a '(__zetta_tab_icons)' -n '__fish_seen_subcommand_from tabicon; and __zetta_short_option -i'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l sound -r -a '(__zetta_sound_names)' -d 'Sound name'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l timeout -r -a 'default never' -d 'Timeout'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from notify' -a '(__zetta_long_options notify)'
complete -c zetta -s a -r -n '__fish_seen_subcommand_from notify; and __zetta_short_option -a'
complete -c zetta -s i -r -n '__fish_seen_subcommand_from notify; and __zetta_short_option -i'
complete -c zetta -s s -r -a '(__zetta_sound_names)' -n '__fish_seen_subcommand_from notify; and __zetta_short_option -s'
complete -c zetta -s t -r -a 'default never' -n '__fish_seen_subcommand_from notify; and __zetta_short_option -t'
complete -c zetta -n '__fish_seen_subcommand_from copy' -l pboard -r -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from copy' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from copy' -a '(__zetta_long_options copy)'
complete -c zetta -n '__fish_seen_subcommand_from copy; and __zetta_short_option -pboard' -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l pboard -r -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l prefer -r -a 'txt rtf ps'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from paste' -a '(__zetta_long_options paste)'
complete -c zetta -n '__fish_seen_subcommand_from paste; and __zetta_short_option -pboard' -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from paste; and __zetta_short_option -prefer' -a 'txt rtf ps'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -l list -d 'Print built-in icon names'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -a '(__zetta_long_options tabicon)'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -a '(__zetta_tab_icons)'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -l theme -r -a '(__zetta_pane_themes)' -d 'Set the pane theme'
complete -c zetta -s t -r -a '(__zetta_pane_themes)' -n '__fish_seen_subcommand_from panetheme; and __zetta_short_option -t'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -l reset -d 'Restore the profile-configured theme'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -l list -d 'Print the registered theme names'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -a '(__zetta_long_options panetheme)'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -a '(__zetta_pane_themes)'
complete -c ztftp -f -a 'get put'
complete -c ztftp -l port -r -d 'Server port'
complete -c ztftp -l help -d 'Print help'
complete -c ztftp -a '(__zetta_long_options ztftp)'
complete -c ztftp -s p -r -n '__zetta_short_option -p'
complete -c zntfy -f -l app-name -r
complete -c zntfy -l icon -r
complete -c zntfy -l sound -r -a '(__zetta_sound_names)'
complete -c zntfy -l timeout -r -a 'default never'
complete -c zntfy -l help -d 'Print help'
complete -c zntfy -a '(__zetta_long_options zntfy)'
complete -c zntfy -s a -r -n '__zetta_short_option -a'
complete -c zntfy -s i -r -n '__zetta_short_option -i'
complete -c zntfy -s s -r -a '(__zetta_sound_names)' -n '__zetta_short_option -s'
complete -c zntfy -s t -r -a 'default never' -n '__zetta_short_option -t'
complete -c zcopy -f -l pboard -r -a 'general ruler find font'
complete -c zcopy -l help -d 'Print help'
complete -c zcopy -a '(__zetta_long_options zcopy)'
complete -c zcopy -n '__zetta_short_option -pboard' -a 'general ruler find font'
complete -c zpaste -f -l pboard -r -a 'general ruler find font'
complete -c zpaste -l prefer -r -a 'txt rtf ps'
complete -c zpaste -l help -d 'Print help'
complete -c zpaste -a '(__zetta_long_options zpaste)'
complete -c zpaste -n '__zetta_short_option -pboard' -a 'general ruler find font'
complete -c zpaste -n '__zetta_short_option -prefer' -a 'txt rtf ps'
if test (uname) != Darwin
    complete -c pbcopy -f -l pboard -r -a 'general ruler find font'
    complete -c pbcopy -l help -d 'Print help'
    complete -c pbcopy -a '(__zetta_long_options pbcopy)'
    complete -c pbcopy -n '__zetta_short_option -pboard' -a 'general ruler find font'
    complete -c pbpaste -f -l pboard -r -a 'general ruler find font'
    complete -c pbpaste -l prefer -r -a 'txt rtf ps'
    complete -c pbpaste -l help -d 'Print help'
    complete -c pbpaste -a '(__zetta_long_options pbpaste)'
    complete -c pbpaste -n '__zetta_short_option -pboard' -a 'general ruler find font'
    complete -c pbpaste -n '__zetta_short_option -prefer' -a 'txt rtf ps'
end
"#;

const POWERSHELL_INTEGRATION: &str = r#"# Zetta shell integration for PowerShell.
if (-not (Test-Path Env:EDITOR)) {
    $env:EDITOR = 'zetta vi'
}

$zettaViMissing = -not (Get-Command vi -ErrorAction SilentlyContinue)
if ($zettaViMissing) {
    function vi { & zetta vi @args }
}

function zvi { & zetta vi @args }

function ztftp { & zetta tftp @args }
function zntfy { & zetta notify @args }
function zcopy { & zetta copy @args }
function zpaste { & zetta paste @args }

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste alias (eg. one pointing at a third-party
# tool) is removed first so Zetta's functions take priority over it. As
# above, $IsMacOS is unset (falsy) on Windows PowerShell 5.1.
if (-not $IsMacOS) {
    Remove-Item -Path Alias:pbcopy,Alias:pbpaste -ErrorAction SilentlyContinue
    function pbcopy { & zetta copy @args }
    function pbpaste { & zetta paste @args }
}

$zettaProfiles = @(ZETTA_PROFILES)
$zettaTabIcons = { @(& zetta tabicon --list 2>$null) }
$zettaPaneThemes = { @(& zetta panetheme --list 2>$null) }

# zetta-default/zetta-ok/zetta-alarm are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered. $IsMacOS/$IsLinux are unset on Windows PowerShell 5.1, which only
# runs on Windows, so the Windows branch is also the correct fallback there.
$zettaSoundNames = @('zetta-default', 'zetta-ok', 'zetta-alarm') + $(
    if ($IsMacOS) {
        'Basso', 'Blow', 'Bottle', 'Frog', 'Funk', 'Glass', 'Hero', 'Morse', 'Ping', 'Pop', 'Purr', 'Sosumi', 'Submarine', 'Tink'
    } elseif ($IsLinux) {
        'bell', 'complete', 'message', 'message-new-instant', 'dialog-information', 'dialog-warning', 'dialog-error', 'trash-empty'
    } else {
        'Default', 'IM', 'Mail', 'Reminder', 'SMS'
    }
)

$zettaSessionIds = {
    try {
        $catalogs = @(zetta sessions --json 2>$null | ConvertFrom-Json)
        foreach ($catalog in $catalogs) {
            foreach ($session in @($catalog.sessions)) {
                "{0}:{1}:{2}" -f $catalog.process_id, $catalog.runner_id, $session.id
            }
        }
    } catch {}
}

$zettaCompletions = {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandName = $commandAst.CommandElements[0].Value
    $words = @($commandAst.CommandElements | ForEach-Object { $_.Value })
    $previous = if ($words.Count -gt 1) { $words[$words.Count - 2] } else { '' }
    $last = if ($words.Count -gt 1) { $words[$words.Count - 1] } else { '' }
    $subcommand = $words | Where-Object {
        $_ -in 'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'edit', 'vi', 'init', 'serial', 'http', 'tftp', 'notify', 'copy', 'paste', 'tabicon', 'panetheme'
    } | Select-Object -First 1

    $candidates = if ($commandName -eq 'ztftp') {
        if ($words.Count -le 1) { 'get', 'put', '--help' } else { '--port', '--help' }
    } elseif ($commandName -eq 'zntfy') {
        if ($previous -in '--timeout', '-t') { 'default', 'never' }
        elseif ($previous -in '--sound', '-s') { $zettaSoundNames }
        else { '--app-name', '--icon', '--sound', '--timeout', '--help' }
    } elseif ($commandName -in 'zcopy', 'pbcopy') {
        if ($previous -in '--pboard', '-pboard') { 'general', 'ruler', 'find', 'font' }
        else { '--pboard', '--help' }
    } elseif ($commandName -in 'zpaste', 'pbpaste') {
        if ($previous -in '--pboard', '-pboard') { 'general', 'ruler', 'find', 'font' }
        elseif ($previous -in '--prefer', '-prefer', '--Prefer', '-Prefer') { 'txt', 'rtf', 'ps' }
        else { '--pboard', '--prefer', '--help' }
    } elseif (
        $previous -eq '--profile' -or $last -eq '--profile' -or
        (($previous -eq '-p' -or $last -eq '-p') -and $null -eq $subcommand)
    ) {
        $zettaProfiles
    } elseif ($previous -eq '--timeout') {
        'default', 'never'
    } elseif ($previous -in '--output-type', '-t', '--theme') {
        if ($subcommand -eq 'panetheme' -or $null -eq $subcommand) { & $zettaPaneThemes }
        elseif ($subcommand -eq 'notify') { 'default', 'never' }
        else { 'repeated', 'unique' }
    } elseif ($previous -in '--device', '-d') {
        if ($subcommand -eq 'serial') { @(& zetta serial list 2>$null) } else { @() }
    } elseif ($previous -in '--data-bits', '-D') {
        if ($subcommand -eq 'serial') { '5', '6', '7', '8' } else { @() }
    } elseif ($previous -eq '--parity' -or ($previous -eq '-p' -and $subcommand -eq 'serial')) {
        'none', 'odd', 'even'
    } elseif ($previous -in '--stop-bits', '-s') {
        if ($subcommand -eq 'serial') { '1', '2' }
        elseif ($subcommand -eq 'notify') { $zettaSoundNames }
        else { @() }
    } elseif ($previous -eq '--sound') {
        $zettaSoundNames
    } elseif ($previous -in '--flow-control', '-f') {
        'none', 'software', 'hardware'
    } elseif ($previous -in '--pboard', '-pboard') {
        'general', 'ruler', 'find', 'font'
    } elseif ($previous -in '--prefer', '-prefer', '--Prefer', '-Prefer') {
        'txt', 'rtf', 'ps'
    } elseif ($commandName -in 'vi', 'zvi' -or $subcommand -in 'edit', 'vi') {
        if ($wordToComplete -like '-*') {
            '--help'
        } else {
            @(Get-ChildItem -Name -Path "$wordToComplete*" -ErrorAction SilentlyContinue)
        }
    } elseif (
        $previous -in '--columns', '--rows', '-R' -or
        ($previous -eq '-c' -and $subcommand -eq 'terminal-size')
    ) {
        @()
    } elseif ($subcommand -eq 'tabicon' -and (
        $previous -in '--icon', '-i' -or $wordToComplete -notlike '-*'
    )) {
        & $zettaTabIcons
    } elseif ($subcommand -eq 'panetheme' -and $wordToComplete -notlike '-*') {
        & $zettaPaneThemes
    } elseif ($subcommand -eq 'sessions' -and $words.Count -ge 3 -and $words[2] -eq 'reconnect') {
        if ($previous -in '--session', '-s') { @() } else { & $zettaSessionIds }
    } elseif ($null -eq $subcommand) {
        'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'edit', 'vi', 'init', 'serial', 'http', 'tftp', 'notify', 'copy', 'paste', 'tabicon', 'panetheme', '--help', '--version', '--config', '--keymap', '--profile', '--theme'
    } else {
        switch ($subcommand) {
            'benchmark' { '--terminal-render-workload', '--terminal-checkerboard-workload', '--terminal-sparse-update-workload', '--profile-report', '--profile-duration', '--profile-pane-stress', '--profile-background-stress', '--profile-sparse-updates', '--profile-external-terminal', '--help' }
            'benchmark-output' { '--size', '--output-type', '--help' }
            'terminal-size' { '--json', '--resize', '--columns', '--rows', '--help' }
            'edit' { '--delete-after', '--help' }
            'vi' { '--help' }
            'sessions' {
                if ($words.Count -le 2 -or ($words.Count -eq 3 -and $words[2] -ne 'reconnect')) {
                    'reconnect', '--json', '--help'
                } elseif ($words[2] -eq 'reconnect') {
                    if ($last -eq 'reconnect') { & $zettaSessionIds } else { '--session', '--help' }
                } else { '--json', '--help' }
            }
            'init' { 'bash', 'fish', 'powershell', 'pwsh', 'zsh', '--help' }
            'serial' {
                if ($words.Count -le 2) { 'console', 'list', '--help' }
                elseif ($words[2] -eq 'console') { '--device', '--baud-rate', '--data-bits', '--parity', '--stop-bits', '--flow-control', '--help' }
            }
            'http' {
                if ($words.Count -le 2) { 'server', '--help' } else { '--root', '--port', '--config', '--help' }
            }
            'tftp' {
                if ($words.Count -le 2) { 'get', 'put', 'server', '--help' }
                elseif ($words[2] -eq 'server') { '--root', '--port', '--config', '--help' }
                else { '--port', '--help' }
            }
            'notify' { '--app-name', '--icon', '--sound', '--timeout', '--help' }
            'copy' { '--pboard', '--help' }
            'paste' { '--pboard', '--prefer', '--help' }
            'tabicon' { '--icon', '--list', '--help' }
            'panetheme' { '--theme', '--reset', '--list', '--help' }
        }
    }

    $candidates = @($candidates | Where-Object {
        if ($_ -like '-*') { $_ -notin $words } else { $true }
    })
    $candidates | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }
}

Register-ArgumentCompleter -Native -CommandName zetta -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName ztftp -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zntfy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zcopy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zpaste -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zvi -ScriptBlock $zettaCompletions
if ($zettaViMissing) {
    Register-ArgumentCompleter -CommandName vi -ScriptBlock $zettaCompletions
}
if (-not $IsMacOS) {
    Register-ArgumentCompleter -CommandName pbcopy -ScriptBlock $zettaCompletions
    Register-ArgumentCompleter -CommandName pbpaste -ScriptBlock $zettaCompletions
}
"#;

const ZSH_INTEGRATION: &str = r#"# Zetta shell integration for Zsh.
if (( ! ${+EDITOR} )); then
    export EDITOR='zetta vi'
fi

if (( ! $+commands[vi] && ! $+aliases[vi] && ! $+functions[vi] && ! $+builtins[vi] )); then
    function vi { command zetta vi "$@"; }
    _zetta_vi_missing=1
else
    _zetta_vi_missing=0
fi

function zvi { command zetta vi "$@"; }

if ! (( $+functions[compdef] )); then
    autoload -Uz compinit
    compinit
fi

_zetta_option_unused() {
    local option=$1 index
    for (( index = 2; index < CURRENT; index++ )); do
        [[ ${words[index]} == "$option" ]] && return 1
    done
    return 0
}

_zetta_options() {
    local -a candidates=()
    local candidate
    for candidate in "$@"; do
        if [[ $candidate != -* ]] || _zetta_option_unused "$candidate"; then
            candidates+=("$candidate")
        fi
    done
    builtin compadd -- "${candidates[@]}"
}

ztftp() { zetta tftp "$@"; }
zntfy() { zetta notify "$@"; }
zcopy() { zetta copy "$@"; }
zpaste() { zetta paste "$@"; }

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste alias (eg. one pointing at xclip) is
# removed first so Zetta's functions take priority over it. The `function
# name { ... }` form (rather than `name() { ... }`) is required here: zsh
# expands an active alias while parsing a `name() { ... }` definition of the
# same name, which fails to parse ("defining function based on alias") even
# though the preceding unalias runs first, because the whole case branch is
# parsed as one unit before any of it executes.
case "$OSTYPE" in
    darwin*) ;;
    *)
        unalias pbcopy pbpaste 2>/dev/null
        function pbcopy { zetta copy "$@"; }
        function pbpaste { zetta paste "$@"; }
        ;;
esac

_zetta_profiles() {
    compadd -- ZETTA_PROFILES
}

_zetta_session_ids() {
    compadd -- "${(@f)$(zetta sessions --json 2>/dev/null | awk '
        /"process_id"[[:space:]]*:/ { match($0, /[0-9]+/); process=substr($0, RSTART, RLENGTH) }
        /"runner_id"[[:space:]]*:/ { match($0, /[0-9]+/); runner=substr($0, RSTART, RLENGTH) }
        /"id"[[:space:]]*:/ { match($0, /[0-9]+/); session=substr($0, RSTART, RLENGTH) }
        /"authentication_required"[[:space:]]*:/ { print process ":" runner ":" session }
    ')}"
}

_zetta_tab_icons() {
    compadd -- "${(@f)$(zetta tabicon --list 2>/dev/null)}"
}

_zetta_pane_themes() {
    compadd -- "${(@f)$(zetta panetheme --list 2>/dev/null)}"
}

# zetta-default/zetta-ok/zetta-alarm are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered.
_zetta_sound_names() {
    case "$OSTYPE" in
        darwin*)
            compadd -- zetta-default zetta-ok zetta-alarm \
                Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink
            ;;
        msys*|cygwin*|win32*)
            compadd -- zetta-default zetta-ok zetta-alarm Default IM Mail Reminder SMS
            ;;
        *)
            compadd -- zetta-default zetta-ok zetta-alarm bell complete message \
                message-new-instant dialog-information dialog-warning dialog-error trash-empty
            ;;
    esac
}

_zetta() {
    local previous=${words[CURRENT-1]}

    if [[ $words[1] == edit ]]; then
        if [[ $words[CURRENT] == -* ]]; then
            _zetta_options --delete-after --help
        else
            _files
        fi
        return
    fi

    if [[ $words[1] == vi || $words[1] == zvi ]]; then
        if [[ $words[CURRENT] == -* ]]; then
            _zetta_options --help
        else
            _files
        fi
        return
    fi

    if (( CURRENT == 2 )); then
        compadd -S ' ' -- benchmark benchmark-output terminal-size sessions edit vi init serial http tftp notify copy paste tabicon panetheme
        _zetta_options --help --version --config --keymap --profile --theme
        return
    fi

    case $previous in
        --profile)
            _zetta_profiles
            return
            ;;
        -p)
            if [[ $words[2] == serial ]]; then
                compadd -- none odd even
            elif [[ $words[2] != http && $words[2] != tftp && $words[2] != notify ]]; then
                _zetta_profiles
            fi
            return
            ;;
        --config|--keymap|-k|--profile-report)
            _files
            return
            ;;
        --root)
            _files -/
            return
            ;;
        --device)
            compadd -- "${(@f)$(zetta serial list 2>/dev/null)}"
            return
            ;;
        -d)
            if [[ $words[2] == serial ]]; then
                compadd -- "${(@f)$(zetta serial list 2>/dev/null)}"
            fi
            return
            ;;
        --data-bits|-D)
            if [[ $words[2] == serial ]]; then
                compadd -- 5 6 7 8
            fi
            return
            ;;
        --parity)
            compadd -- none odd even
            return
            ;;
        --stop-bits|-s)
            if [[ $words[2] == serial ]]; then
                compadd -- 1 2
            elif [[ $words[2] == notify ]]; then
                _zetta_sound_names
            fi
            return
            ;;
        --flow-control|-f)
            compadd -- none software hardware
            return
            ;;
        --pboard|-pboard)
            compadd -- general ruler find font
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            compadd -- txt rtf ps
            return
            ;;
        --app-name|-a)
            return
            ;;
        --icon|-i)
            if [[ $words[2] == tabicon ]]; then
                _zetta_tab_icons
            else
                _files
            fi
            return
            ;;
        --sound)
            _zetta_sound_names
            return
            ;;
        --timeout)
            compadd -- default never
            return
            ;;
        -c)
            if [[ $words[2] == terminal-size ]]; then
                return
            fi
            _files
            return
            ;;
        -r)
            if [[ $words[2] == http || ( $words[2] == tftp && $words[3] == server ) ]]; then
                _files -/
                return
            fi
            if [[ $words[2] == terminal-size ]]; then
                return
            fi
            _files
            return
            ;;
        --output-type|-t|--theme)
            if [[ $words[2] == panetheme || $words[2] == -* ]]; then
                _zetta_pane_themes
            elif [[ $words[2] == notify ]]; then
                compadd -- default never
            else
                compadd -- repeated unique
            fi
            return
            ;;
        --port|-p|--baud-rate|-b|--size|--profile-duration|--columns|--rows|-R)
            return
            ;;
    esac

    # A leading flag rules out a subcommand for the rest of the command line
    # (subcommands are only recognized as the first argument), so keep
    # offering the remaining top-level flags instead of falling through to
    # the subcommand-specific cases below, which would offer nothing.
    if [[ $words[2] == -* ]]; then
        _zetta_options --help --version --config --keymap --profile --theme
        return
    fi

    case $words[2] in
        benchmark)
            _zetta_options --terminal-render-workload --terminal-checkerboard-workload \
                --terminal-sparse-update-workload --profile-report --profile-duration \
                --profile-pane-stress --profile-background-stress --profile-sparse-updates \
                --profile-external-terminal --help
            ;;
        benchmark-output)
            _zetta_options --size --output-type --help
            ;;
        terminal-size)
            _zetta_options --json --resize --columns --rows --help
            ;;
        edit)
            if [[ $words[CURRENT] == -* ]]; then
                _zetta_options --delete-after --help
            else
                _files
            fi
            ;;
        vi)
            if [[ $words[CURRENT] == -* ]]; then
                _zetta_options --help
            else
                _files
            fi
            ;;
        sessions)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- reconnect
                _zetta_options --json --help
            elif [[ $words[3] == reconnect ]]; then
                if [[ $previous != --session && $previous != -s ]]; then
                    if (( CURRENT == 4 )); then
                        _zetta_session_ids
                    else
                        _zetta_options --session --help
                    fi
                fi
            else
                _zetta_options --json --help
            fi
            ;;
        init)
            compadd -- bash fish powershell pwsh zsh --help
            ;;
        serial)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- console list
                _zetta_options --help
            elif [[ $words[3] == console ]]; then
                _zetta_options --device --baud-rate --data-bits --parity --stop-bits --flow-control --help
            fi
            ;;
        http)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- server
                _zetta_options --help
            else
                _zetta_options --root --port --config --help
            fi
            ;;
        tftp)
            _zetta_tftp
            ;;
        notify)
            _zetta_options --app-name --icon --sound --timeout --help
            ;;
        copy)
            _zetta_options --pboard --help
            ;;
        paste)
            _zetta_options --pboard --prefer --help
            ;;
        tabicon)
            if [[ $words[CURRENT] == -* ]]; then
                _zetta_options --icon --list --help
            else
                _zetta_tab_icons
            fi
            ;;
        panetheme)
            if [[ $words[CURRENT] == -* ]]; then
                _zetta_options --theme --reset --list --help
            else
                _zetta_pane_themes
            fi
            ;;
    esac
}

_zetta_tftp() {
    local operation_index operation position=0 index argument skip_port=0
    local current=${words[CURRENT]}

    if [[ $words[1] == ztftp ]]; then
        operation_index=2
    else
        operation_index=3
    fi

    if (( CURRENT == operation_index )); then
        compadd -S ' ' -- get put server
        _zetta_options --help
        return
    fi

    operation=${words[operation_index]}
    if [[ $operation == server ]]; then
        if [[ $current == -* || -z $current ]]; then
            _zetta_options --root --port --config --help
        fi
        return
    fi

    if [[ $current == -* ]]; then
        _zetta_options --port --help
        return
    fi
    if [[ $words[CURRENT-1] == --port || $words[CURRENT-1] == -p ]]; then
        return
    fi

    for (( index = operation_index + 1; index < CURRENT; index++ )); do
        argument=${words[index]}
        if (( skip_port )); then
            skip_port=0
        elif [[ $argument == --port || $argument == -p ]]; then
            skip_port=1
        elif [[ $argument != -* ]]; then
            (( position++ ))
        fi
    done

    case $operation in
        put)
            (( position == 1 )) && _files
            ;;
    esac
}

_ztftp() {
    _zetta_tftp
}

_zntfy() {
    local previous=${words[CURRENT-1]}

    case $previous in
        --app-name|-a)
            return
            ;;
        --icon|-i)
            _files
            return
            ;;
        --sound|-s)
            _zetta_sound_names
            return
            ;;
        --timeout|-t)
            compadd -- default never
            return
            ;;
    esac
    _zetta_options --app-name --icon --sound --timeout --help
}

_zcopy() {
    local previous=${words[CURRENT-1]}
    case $previous in
        --pboard|-pboard)
            compadd -- general ruler find font
            return
            ;;
    esac
    _zetta_options --pboard --help
}

_zpaste() {
    local previous=${words[CURRENT-1]}
    case $previous in
        --pboard|-pboard)
            compadd -- general ruler find font
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            compadd -- txt rtf ps
            return
            ;;
    esac
    _zetta_options --pboard --prefer --help
}

compdef _zetta zetta
compdef _ztftp ztftp
compdef _zntfy zntfy
compdef _zcopy zcopy
compdef _zpaste zpaste
compdef _zetta zvi
if (( _zetta_vi_missing )); then
    compdef _zetta vi
fi
case "$OSTYPE" in
    darwin*) ;;
    *)
        compdef _zcopy pbcopy
        compdef _zpaste pbpaste
        ;;
esac
"#;

#[cfg(test)]
#[path = "tests/shell_integration.rs"]
mod tests;
