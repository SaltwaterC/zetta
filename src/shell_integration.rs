use super::*;

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
}

fn render_profiles(shell: ShellIntegration, profiles: &[Profile]) -> String {
    profiles
        .iter()
        .map(|profile| match shell {
            ShellIntegration::Bash | ShellIntegration::Zsh | ShellIntegration::Fish => {
                shell_single_quote(&profile.name)
            }
            ShellIntegration::PowerShell => format!("'{}'", profile.name.replace('\'', "''")),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn shell_integration_help() -> &'static str {
    "Generate shell integration\n\nUsage: zetta init SHELL\n\nSupported shells:\n  bash        Bash\n  fish        Fish\n  powershell  PowerShell (also accepted as pwsh)\n  zsh         Z shell\n\nThe generated script adds command completion and the ztftp shortcut when the TFTP client is enabled."
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
