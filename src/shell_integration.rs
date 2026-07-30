use super::*;
use std::ffi::OsStr;
use std::io::Write as _;
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

    let home =
        env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).with_context(|| {
            if cfg!(windows) {
                "could not locate the home directory: USERPROFILE is not set"
            } else {
                "could not locate the home directory: HOME is not set"
            }
        })?;
    configure_shell_integration(shell, Path::new(&home))
}

fn configure_shell_integration(
    shell: ShellIntegration,
    home: &Path,
) -> Result<ShellIntegrationConfiguration> {
    let path = shell.startup_file(home);
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
    "Configure or generate shell integration\n\nUsage: zetta init [SHELL]\n\nWithout SHELL, detects the current shell from SHELL and adds the integration command to its startup file. On Windows, Zetta detects the launching PowerShell and writes to its $PROFILE when SHELL is unavailable. Running it again leaves an existing integration unchanged. With SHELL, prints the integration script for use in a shell startup file.\n\nSupported shells:\n  bash        Bash\n  fish        Fish\n  powershell  PowerShell (also accepted as pwsh)\n  zsh         Z shell\n\nThe generated script adds command completion and the ztftp shortcut when the TFTP client is enabled."
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
            if [[ $command != tftp ]]; then
                _zetta_complete_profiles
            else
                COMPREPLY=()
            fi
            return
            ;;
        --config|--keymap|-k|--profile-report)
            COMPREPLY=( $(compgen -f -- "$current") )
            return
            ;;
        -c|-r)
            if [[ $command == terminal-size ]]; then
                COMPREPLY=()
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        --output-type|-t)
            COMPREPLY=( $(compgen -W 'repeated unique' -- "$current") )
            return
            ;;
        --port|-p|--size|-s|--profile-duration|-d|--columns|-c|--rows|-R)
            COMPREPLY=()
            return
            ;;
    esac

    if (( COMP_CWORD == 1 )); then
        COMPREPLY=( $(compgen -W 'benchmark benchmark-output terminal-size sessions init tftp --help --version --config --keymap --profile' -- "$current") )
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
        tftp)
            _zetta_tftp_complete 2
            ;;
    esac
}

_zetta_tftp_complete() {
    local operation_index=$1 current previous operation argument
    local index positional=0 skip_port=0
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}

    if (( COMP_CWORD == operation_index )); then
        COMPREPLY=( $(compgen -W 'get put --help' -- "$current") )
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

    operation=${COMP_WORDS[operation_index]}
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

_ztftp_complete() {
    _zetta_tftp_complete 1
}

ztftp() { zetta tftp "$@"; }
complete -F _zetta_complete zetta
complete -F _ztftp_complete ztftp
"#;

const FISH_INTEGRATION: &str = r#"# Zetta shell integration for Fish.
function ztftp --wraps 'zetta tftp' --description 'Zetta TFTP client'
    zetta tftp $argv
end

function __zetta_profiles
    printf '%s\n' ZETTA_PROFILES
end

complete -c zetta -f
complete -c zetta -n '__fish_use_subcommand' -a benchmark -d 'Profile terminal rendering'
complete -c zetta -n '__fish_use_subcommand' -a benchmark-output -d 'Write and time a text payload'
complete -c zetta -n '__fish_use_subcommand' -a terminal-size -d 'Print the current terminal size'
complete -c zetta -n '__fish_use_subcommand' -a sessions -d 'List detached background sessions'
complete -c zetta -n '__fish_use_subcommand' -a init -d 'Generate shell integration'
complete -c zetta -n '__fish_use_subcommand' -a tftp -d 'Transfer a file with TFTP'
complete -c zetta -n '__fish_use_subcommand' -l help -d 'Print help'
complete -c zetta -n '__fish_use_subcommand' -l version -d 'Print version'
complete -c zetta -n '__fish_use_subcommand' -l config -r -d 'Use a configuration file'
complete -c zetta -n '__fish_use_subcommand' -l keymap -r -d 'Use a keymap file'
complete -c zetta -n '__fish_use_subcommand' -l profile -r -a '(__zetta_profiles)' -d 'Select a profile'
complete -c zetta -n '__fish_seen_subcommand_from init' -a 'bash fish powershell pwsh zsh'
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
complete -c zetta -n '__fish_seen_subcommand_from tftp' -a 'get put'
complete -c zetta -n '__fish_seen_subcommand_from tftp' -l port -r -d 'Server port'
complete -c zetta -n '__fish_seen_subcommand_from tftp' -l help -d 'Print help'
complete -c ztftp -f -a 'get put'
complete -c ztftp -l port -r -d 'Server port'
complete -c ztftp -l help -d 'Print help'
"#;

const POWERSHELL_INTEGRATION: &str = r#"# Zetta shell integration for PowerShell.
function ztftp { & zetta tftp @args }

$zettaProfiles = @(ZETTA_PROFILES)

$zettaCompletions = {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandName = $commandAst.CommandElements[0].Value
    $words = @($commandAst.CommandElements | ForEach-Object { $_.Value })
    $previous = if ($words.Count -gt 1) { $words[$words.Count - 2] } else { '' }
    $last = if ($words.Count -gt 1) { $words[$words.Count - 1] } else { '' }
    $subcommand = $words | Where-Object {
        $_ -in 'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'init', 'tftp'
    } | Select-Object -First 1

    $candidates = if ($commandName -eq 'ztftp') {
        if ($words.Count -le 1) { 'get', 'put', '--help' } else { '--port', '--help' }
    } elseif ($previous -in '--profile', '-p' -or $last -in '--profile', '-p') {
        $zettaProfiles
    } elseif ($previous -in '--output-type', '-t') {
        'repeated', 'unique'
    } elseif ($previous -in '--columns', '-c', '--rows', '-R') {
        @()
    } elseif ($null -eq $subcommand) {
        'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'init', 'tftp', '--help', '--version', '--config', '--keymap', '--profile'
    } else {
        switch ($subcommand) {
            'benchmark' { '--terminal-render-workload', '--terminal-checkerboard-workload', '--terminal-sparse-update-workload', '--profile-report', '--profile-duration', '--profile-pane-stress', '--profile-background-stress', '--profile-sparse-updates', '--profile-external-terminal', '--help' }
            'benchmark-output' { '--size', '--output-type', '--help' }
            'terminal-size' { '--json', '--resize', '--columns', '--rows', '--help' }
            'sessions' { '--json', '--help' }
            'init' { 'bash', 'fish', 'powershell', 'pwsh', 'zsh', '--help' }
            'tftp' { if ($words.Count -le 2) { 'get', 'put', '--help' } else { '--port', '--help' } }
        }
    }

    $candidates | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }
}

Register-ArgumentCompleter -Native -CommandName zetta -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName ztftp -ScriptBlock $zettaCompletions
"#;

const ZSH_INTEGRATION: &str = r#"# Zetta shell integration for Zsh.
if ! (( $+functions[compdef] )); then
    autoload -Uz compinit
    compinit
fi

ztftp() { zetta tftp "$@"; }

_zetta_profiles() {
    compadd -- ZETTA_PROFILES
}

_zetta() {
    local previous=${words[CURRENT-1]}

    if (( CURRENT == 2 )); then
        compadd -S ' ' -- benchmark benchmark-output terminal-size sessions init tftp
        compadd -- --help --version --config --keymap --profile
        return
    fi

    case $previous in
        --profile|-p)
            _zetta_profiles
            return
            ;;
        --config|--keymap|-k|--profile-report)
            _files
            return
            ;;
        -c|-r)
            if [[ $words[2] == terminal-size ]]; then
                return
            fi
            _files
            return
            ;;
        --output-type|-t)
            compadd -- repeated unique
            return
            ;;
        --port|--size|--profile-duration|-d|--columns|-c|--rows|-R)
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
        tftp)
            _zetta_tftp
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
        compadd -S ' ' -- get put
        compadd -- --help
        return
    fi

    if [[ $current == -* ]]; then
        compadd -- --port --help
        return
    fi
    if [[ $words[CURRENT-1] == --port || $words[CURRENT-1] == -p ]]; then
        return
    fi

    operation=${words[operation_index]}
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

compdef _zetta zetta
compdef _ztftp ztftp
"#;

#[cfg(test)]
#[path = "tests/shell_integration.rs"]
mod tests;
