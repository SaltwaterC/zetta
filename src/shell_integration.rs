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
    "Configure or generate shell integration\n\nUsage: zetta init [SHELL]\n\nWithout SHELL, detects the current shell from SHELL and adds the integration command to its startup file. On Windows, Unix-style HOME paths from MSYS2 and similar environments are resolved with cygpath; when SHELL is unavailable, Zetta detects the launching PowerShell and writes to its $PROFILE. Running it again leaves an existing integration unchanged. With SHELL, prints the integration script for use in a shell startup file.\n\nSupported shells:\n  bash        Bash\n  fish        Fish\n  powershell  PowerShell (also accepted as pwsh)\n  zsh         Z shell\n\nThe generated script adds command completion, including live serial-device completion, the ztftp shortcut when the TFTP client is enabled, and the zntfy and zcopy/zpaste shortcuts when desktop notifications and clipboard access are enabled. zcopy/zpaste are also available as pbcopy/pbpaste on platforms other than macOS, taking priority over any existing pbcopy/pbpaste alias so pbcopy/pbpaste muscle memory keeps working there too."
}

const BASH_INTEGRATION: &str = r#"# Zetta shell integration for Bash.
_zetta_complete() {
    local current previous command
    local -a profiles=(ZETTA_PROFILES)
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}
    command=${COMP_WORDS[1]}

    _zetta_complete_profiles() {
        COMPREPLY=()
        local profile
        for profile in "${profiles[@]}"; do
            [[ $profile == "$current"* ]] && COMPREPLY+=("$profile")
        done
    }

    case "$previous" in
        --profile)
            _zetta_complete_profiles
            return
            ;;
        -p)
            if [[ $command == serial ]]; then
                COMPREPLY=( $(compgen -W 'none odd even' -- "$current") )
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
                COMPREPLY=( $(compgen -W '5 6 7 8' -- "$current") )
            else
                COMPREPLY=()
            fi
            return
            ;;
        --parity)
            COMPREPLY=( $(compgen -W 'none odd even' -- "$current") )
            return
            ;;
        --stop-bits|-s)
            if [[ $command == serial ]]; then
                COMPREPLY=( $(compgen -W '1 2' -- "$current") )
            elif [[ $command == notify ]]; then
                _zetta_complete_sound_names
            else
                COMPREPLY=()
            fi
            return
            ;;
        --flow-control|-f)
            COMPREPLY=( $(compgen -W 'none software hardware' -- "$current") )
            return
            ;;
        --pboard|-pboard)
            COMPREPLY=( $(compgen -W 'general ruler find font' -- "$current") )
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            COMPREPLY=( $(compgen -W 'txt rtf ps' -- "$current") )
            return
            ;;
        --app-name|-a)
            COMPREPLY=()
            return
            ;;
        --icon|-i)
            COMPREPLY=( $(compgen -f -- "$current") )
            return
            ;;
        --sound)
            _zetta_complete_sound_names
            return
            ;;
        --timeout)
            COMPREPLY=( $(compgen -W 'default never' -- "$current") )
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
        --output-type|-t)
            if [[ $command == notify ]]; then
                COMPREPLY=( $(compgen -W 'default never' -- "$current") )
            else
                COMPREPLY=( $(compgen -W 'repeated unique' -- "$current") )
            fi
            return
            ;;
        --port|-p|--baud-rate|-b|--size|--profile-duration|--columns|--rows|-R)
            COMPREPLY=()
            return
            ;;
    esac

    if (( COMP_CWORD == 1 )); then
        COMPREPLY=( $(compgen -W 'benchmark benchmark-output terminal-size sessions init serial http tftp notify copy paste --help --version --config --keymap --profile' -- "$current") )
        return
    fi

    case "$command" in
        benchmark)
            COMPREPLY=( $(compgen -W '--terminal-render-workload --terminal-checkerboard-workload --terminal-sparse-update-workload --profile-report --profile-duration --profile-pane-stress --profile-background-stress --profile-sparse-updates --profile-external-terminal --help' -- "$current") )
            ;;
        benchmark-output)
            COMPREPLY=( $(compgen -W '--size --output-type --help' -- "$current") )
            ;;
        terminal-size)
            COMPREPLY=( $(compgen -W '--json --resize --columns --rows --help' -- "$current") )
            ;;
        sessions)
            COMPREPLY=( $(compgen -W '--json --help' -- "$current") )
            ;;
        init)
            COMPREPLY=( $(compgen -W 'bash fish powershell pwsh zsh --help' -- "$current") )
            ;;
        serial)
            if (( COMP_CWORD == 2 )); then
                COMPREPLY=( $(compgen -W 'console list --help' -- "$current") )
            elif [[ ${COMP_WORDS[2]} == console ]]; then
                COMPREPLY=( $(compgen -W '--device --baud-rate --data-bits --parity --stop-bits --flow-control --help' -- "$current") )
            fi
            ;;
        http)
            if (( COMP_CWORD == 2 )); then
                COMPREPLY=( $(compgen -W 'server --help' -- "$current") )
            else
                COMPREPLY=( $(compgen -W '--root --port --config --help' -- "$current") )
            fi
            ;;
        tftp)
            _zetta_tftp_complete 2
            ;;
        notify)
            COMPREPLY=( $(compgen -W '--app-name --icon --sound --timeout --help' -- "$current") )
            ;;
        copy)
            COMPREPLY=( $(compgen -W '--pboard --help' -- "$current") )
            ;;
        paste)
            COMPREPLY=( $(compgen -W '--pboard --prefer --help' -- "$current") )
            ;;
    esac
}

_zetta_tftp_complete() {
    local operation_index=$1 current previous operation argument
    local index positional=0 skip_port=0
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}

    if (( COMP_CWORD == operation_index )); then
        COMPREPLY=( $(compgen -W 'get put server --help' -- "$current") )
        return
    fi
    operation=${COMP_WORDS[operation_index]}
    if [[ $operation == server ]]; then
        if [[ $current == -* || -z $current ]]; then
            COMPREPLY=( $(compgen -W '--root --port --config --help' -- "$current") )
        fi
        return
    fi
    if [[ $current == -* ]]; then
        COMPREPLY=( $(compgen -W '--port --help' -- "$current") )
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
    COMPREPLY=( $(compgen -W '--app-name --icon --sound --timeout --help' -- "$current") )
}

_zcopy_complete() {
    local current=${COMP_WORDS[COMP_CWORD]} previous=${COMP_WORDS[COMP_CWORD-1]}
    case "$previous" in
        --pboard|-pboard)
            COMPREPLY=( $(compgen -W 'general ruler find font' -- "$current") )
            return
            ;;
    esac
    COMPREPLY=( $(compgen -W '--pboard --help' -- "$current") )
}

_zpaste_complete() {
    local current=${COMP_WORDS[COMP_CWORD]} previous=${COMP_WORDS[COMP_CWORD-1]}
    case "$previous" in
        --pboard|-pboard)
            COMPREPLY=( $(compgen -W 'general ruler find font' -- "$current") )
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            COMPREPLY=( $(compgen -W 'txt rtf ps' -- "$current") )
            return
            ;;
    esac
    COMPREPLY=( $(compgen -W '--pboard --prefer --help' -- "$current") )
}

ztftp() { zetta tftp "$@"; }
zntfy() { zetta notify "$@"; }
zcopy() { zetta copy "$@"; }
zpaste() { zetta paste "$@"; }
complete -F _zetta_complete zetta
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

complete -c zetta -f
complete -c zetta -n '__fish_use_subcommand' -a benchmark -d 'Profile terminal rendering'
complete -c zetta -n '__fish_use_subcommand' -a benchmark-output -d 'Write and time a text payload'
complete -c zetta -n '__fish_use_subcommand' -a terminal-size -d 'Print the current terminal size'
complete -c zetta -n '__fish_use_subcommand' -a sessions -d 'List detached background sessions'
complete -c zetta -n '__fish_use_subcommand' -a init -d 'Generate shell integration'
complete -c zetta -n '__fish_use_subcommand' -a serial -d 'List or connect to serial devices'
complete -c zetta -n '__fish_use_subcommand' -a http -d 'Serve static files over HTTP'
complete -c zetta -n '__fish_use_subcommand' -a tftp -d 'Transfer a file with TFTP'
complete -c zetta -n '__fish_use_subcommand' -a notify -d 'Show a desktop notification'
complete -c zetta -n '__fish_use_subcommand' -a copy -d 'Copy standard input to the clipboard'
complete -c zetta -n '__fish_use_subcommand' -a paste -d "Print the clipboard's contents"
complete -c zetta -n '__fish_use_subcommand' -l help -d 'Print help'
complete -c zetta -n '__fish_use_subcommand' -l version -d 'Print version'
complete -c zetta -n '__fish_use_subcommand' -l config -r -d 'Use a configuration file'
complete -c zetta -n '__fish_use_subcommand' -l keymap -r -d 'Use a keymap file'
complete -c zetta -n '__fish_use_subcommand' -l profile -r -a '(__zetta_profiles)' -d 'Select a profile'
complete -c zetta -n '__fish_seen_subcommand_from init' -a 'bash fish powershell pwsh zsh'
complete -c zetta -n '__fish_seen_subcommand_from serial' -a 'console list'
complete -c zetta -n '__fish_seen_subcommand_from http' -a server
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l json -d 'Print machine-readable JSON'
complete -c zetta -n '__fish_seen_subcommand_from sessions' -l json -d 'Print machine-readable JSON'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l resize -d 'Resize the current pane'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l columns -r -d 'Set pane width in columns'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l rows -r -d 'Set pane height in rows'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from sessions' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l size -r -d 'Set the output size in MiB'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l output-type -r -a 'repeated unique'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l help -d 'Print help'
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
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l device -r -a '(__zetta_serial_devices)' -d 'Serial device'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l baud-rate -r -d 'Baud rate'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l data-bits -r -a '5 6 7 8'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l parity -r -a 'none odd even'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l stop-bits -r -a '1 2'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l flow-control -r -a 'none software hardware'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l root -r -a '(__fish_complete_directories)' -d 'Directory to serve'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l port -r -d 'TCP port'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l config -r -d 'Configuration file'
complete -c zetta -n '__fish_seen_subcommand_from tftp' -a 'get put server'
complete -c zetta -n '__fish_seen_subcommand_from tftp' -l port -r -d 'Server port'
complete -c zetta -n '__fish_seen_subcommand_from tftp; and __fish_seen_subcommand_from server' -l root -r -a '(__fish_complete_directories)' -d 'Directory to serve'
complete -c zetta -n '__fish_seen_subcommand_from tftp; and __fish_seen_subcommand_from server' -l config -r -d 'Configuration file'
complete -c zetta -n '__fish_seen_subcommand_from tftp' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l app-name -r -d 'Application name'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l icon -r -d 'Image to show with the notification'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l sound -r -a '(__zetta_sound_names)' -d 'Sound name'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l timeout -r -a 'default never' -d 'Timeout'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from copy' -l pboard -r -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from copy' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l pboard -r -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l prefer -r -a 'txt rtf ps'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l help -d 'Print help'
complete -c ztftp -f -a 'get put'
complete -c ztftp -l port -r -d 'Server port'
complete -c ztftp -l help -d 'Print help'
complete -c zcopy -f -l pboard -r -a 'general ruler find font'
complete -c zcopy -l help -d 'Print help'
complete -c zpaste -f -l pboard -r -a 'general ruler find font'
complete -c zpaste -l prefer -r -a 'txt rtf ps'
complete -c zpaste -l help -d 'Print help'
if test (uname) != Darwin
    complete -c pbcopy -f -l pboard -r -a 'general ruler find font'
    complete -c pbcopy -l help -d 'Print help'
    complete -c pbpaste -f -l pboard -r -a 'general ruler find font'
    complete -c pbpaste -l prefer -r -a 'txt rtf ps'
    complete -c pbpaste -l help -d 'Print help'
end
"#;

const POWERSHELL_INTEGRATION: &str = r#"# Zetta shell integration for PowerShell.
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

$zettaCompletions = {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandName = $commandAst.CommandElements[0].Value
    $words = @($commandAst.CommandElements | ForEach-Object { $_.Value })
    $previous = if ($words.Count -gt 1) { $words[$words.Count - 2] } else { '' }
    $last = if ($words.Count -gt 1) { $words[$words.Count - 1] } else { '' }
    $subcommand = $words | Where-Object {
        $_ -in 'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'init', 'serial', 'http', 'tftp', 'notify', 'copy', 'paste'
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
    } elseif ($previous -in '--output-type', '-t') {
        if ($subcommand -eq 'notify') { 'default', 'never' } else { 'repeated', 'unique' }
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
    } elseif (
        $previous -in '--columns', '--rows', '-R' -or
        ($previous -eq '-c' -and $subcommand -eq 'terminal-size')
    ) {
        @()
    } elseif ($null -eq $subcommand) {
        'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'init', 'serial', 'http', 'tftp', 'notify', 'copy', 'paste', '--help', '--version', '--config', '--keymap', '--profile'
    } else {
        switch ($subcommand) {
            'benchmark' { '--terminal-render-workload', '--terminal-checkerboard-workload', '--terminal-sparse-update-workload', '--profile-report', '--profile-duration', '--profile-pane-stress', '--profile-background-stress', '--profile-sparse-updates', '--profile-external-terminal', '--help' }
            'benchmark-output' { '--size', '--output-type', '--help' }
            'terminal-size' { '--json', '--resize', '--columns', '--rows', '--help' }
            'sessions' { '--json', '--help' }
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
        }
    }

    $candidates | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }
}

Register-ArgumentCompleter -Native -CommandName zetta -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName ztftp -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zntfy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zcopy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zpaste -ScriptBlock $zettaCompletions
if (-not $IsMacOS) {
    Register-ArgumentCompleter -CommandName pbcopy -ScriptBlock $zettaCompletions
    Register-ArgumentCompleter -CommandName pbpaste -ScriptBlock $zettaCompletions
}
"#;

const ZSH_INTEGRATION: &str = r#"# Zetta shell integration for Zsh.
if ! (( $+functions[compdef] )); then
    autoload -Uz compinit
    compinit
fi

ztftp() { zetta tftp "$@"; }
zntfy() { zetta notify "$@"; }
zcopy() { zetta copy "$@"; }
zpaste() { zetta paste "$@"; }

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste alias (eg. one pointing at xclip) is
# removed first so Zetta's functions take priority over it.
case "$OSTYPE" in
    darwin*) ;;
    *)
        unalias pbcopy pbpaste 2>/dev/null
        pbcopy() { zetta copy "$@"; }
        pbpaste() { zetta paste "$@"; }
        ;;
esac

_zetta_profiles() {
    compadd -- ZETTA_PROFILES
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

    if (( CURRENT == 2 )); then
        compadd -S ' ' -- benchmark benchmark-output terminal-size sessions init serial http tftp notify copy paste
        compadd -- --help --version --config --keymap --profile
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
            _files
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
        --output-type|-t)
            if [[ $words[2] == notify ]]; then
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

    case $words[2] in
        benchmark)
            compadd -- --terminal-render-workload --terminal-checkerboard-workload \
                --terminal-sparse-update-workload --profile-report --profile-duration \
                --profile-pane-stress --profile-background-stress --profile-sparse-updates \
                --profile-external-terminal --help
            ;;
        benchmark-output)
            compadd -- --size --output-type --help
            ;;
        terminal-size)
            compadd -- --json --resize --columns --rows --help
            ;;
        sessions)
            compadd -- --json --help
            ;;
        init)
            compadd -- bash fish powershell pwsh zsh --help
            ;;
        serial)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- console list
                compadd -- --help
            elif [[ $words[3] == console ]]; then
                compadd -- --device --baud-rate --data-bits --parity --stop-bits --flow-control --help
            fi
            ;;
        http)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- server
                compadd -- --help
            else
                compadd -- --root --port --config --help
            fi
            ;;
        tftp)
            _zetta_tftp
            ;;
        notify)
            compadd -- --app-name --icon --sound --timeout --help
            ;;
        copy)
            compadd -- --pboard --help
            ;;
        paste)
            compadd -- --pboard --prefer --help
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
        compadd -- --help
        return
    fi

    operation=${words[operation_index]}
    if [[ $operation == server ]]; then
        if [[ $current == -* || -z $current ]]; then
            compadd -- --root --port --config --help
        fi
        return
    fi

    if [[ $current == -* ]]; then
        compadd -- --port --help
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
    compadd -- --app-name --icon --sound --timeout --help
}

_zcopy() {
    local previous=${words[CURRENT-1]}
    case $previous in
        --pboard|-pboard)
            compadd -- general ruler find font
            return
            ;;
    esac
    compadd -- --pboard --help
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
    compadd -- --pboard --prefer --help
}

compdef _zetta zetta
compdef _ztftp ztftp
compdef _zntfy zntfy
compdef _zcopy zcopy
compdef _zpaste zpaste
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
