//! The lazy startup payloads: their size budget, and completion that loads on
//! first use, replays, and retries after a failure.

use super::*;

#[test]
fn lazy_startup_payloads_fit_budget_and_keep_completion_separate() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let startup = shell.script();
        assert!(
            startup.len() <= 16 * 1024,
            "{shell:?}: {} bytes",
            startup.len()
        );
        assert!(startup.contains("--completions"));
        assert!(!startup.contains("ZETTA_OVERLAY_COLORS"));
        let completions = shell.completion_script();
        assert!(!completions.contains("tracking-ready"));
        assert!(completions.contains("--completions") || completions.contains("-l completions"));
    }
}

#[test]
fn bash_lazy_completion_replays_retries_and_loads_once() {
    if !bash_available() {
        return;
    }
    let _guard = lock_bash_tests();
    let temp = tempfile::tempdir().unwrap();
    let startup = temp.path().join("startup.sh");
    let completion = temp.path().join("completion.sh");
    std::fs::write(&startup, ShellIntegration::Bash.script()).unwrap();
    std::fs::write(&completion, ShellIntegration::Bash.completion_script()).unwrap();
    let driver = r#"
source "$STARTUP"
source "$STARTUP"
count=0
zetta() {
    (( count++ ))
    printf '%s\n' "$count" >> "$COUNT"
    case $(( $(wc -l < "$COUNT") )) in
        1) return 1 ;;
        2) return 0 ;;
        *) cat "$COMPLETION" ;;
    esac
}
COMP_WORDS=(zetta init --comp); COMP_CWORD=2
__zetta_lazy_complete
[[ ${__ZETTA_COMPLETIONS_LOADED:-0} == 0 ]] || exit 10
__zetta_lazy_complete
[[ ${__ZETTA_COMPLETIONS_LOADED:-0} == 0 ]] || exit 11
__zetta_lazy_complete
[[ ${COMPREPLY[*]} == --completions ]] || exit 12
_zetta_entry_zetta
[[ ${COMPREPLY[*]} == --completions ]] || exit 13
source "$STARTUP"
[[ $(complete -p zetta) == *'-F _zetta_entry_zetta '* ]] || exit 14
(( $(wc -l < "$COUNT") == 3 )) || exit 15
"#;
    let output = bash_command()
        .arg("--noprofile")
        .arg("--norc")
        .arg("-c")
        .arg(driver)
        .env("STARTUP", startup)
        .env("COMPLETION", completion)
        .env("COUNT", temp.path().join("count"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn fish_lazy_completion_replays_retries_and_loads_once() {
    if !std::process::Command::new("fish")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
    {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("startup"), ShellIntegration::Fish.script()).unwrap();
    std::fs::write(
        temp.path().join("completion"),
        ShellIntegration::Fish.completion_script(),
    )
    .unwrap();
    let driver = r#"
source "$STARTUP"
source "$STARTUP"
function zetta
    echo load >> "$COUNT"
    switch (count (cat "$COUNT"))
        case 1
            return 1
        case 2
            return 0
        case '*'
            cat "$COMPLETION"
    end
end
function zetta_test_wrapper --wraps zetta_test_target
end
complete -c zetta_test_target -a inherited
complete -c zetta_test_wrapper -a retained
complete -c zetta_test_wrapper -a '(__zetta_lazy_complete)'
set -l cleanup_output (__zetta_remove_lazy_registration zetta_test_wrapper)
test (count $cleanup_output) = 0; or exit 15
set -l preserved (complete -C 'zetta_test_wrapper ')
contains -- retained $preserved; or exit 16
contains -- inherited $preserved; or exit 17
string match -q '*__zetta_lazy_complete*' (complete -c zetta_test_wrapper); and exit 18
complete -C 'zetta init --comp' >/dev/null
set -q __zetta_completions_loaded; and exit 10
complete -C 'zetta init --comp' >/dev/null
set -q __zetta_completions_loaded; and exit 11
set -l first (complete -C 'zetta init --comp')
string match -q -- '--completions*' $first; or exit 12
set -l second (complete -C 'zetta init --comp')
string match -q -- '--completions*' $second; or exit 13
source "$STARTUP"
test (count (cat "$COUNT")) = 3; or exit 14
"#;
    let output = clean_shell_command("fish")
        .arg("--no-config")
        .arg("-c")
        .arg(driver)
        .env("STARTUP", temp.path().join("startup"))
        .env("COMPLETION", temp.path().join("completion"))
        .env("COUNT", temp.path().join("count"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(not(windows))]
#[test]
fn zsh_lazy_widgets_complete_interactively_before_and_after_compinit() {
    if !std::process::Command::new("zsh")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
    {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("startup"), ShellIntegration::Zsh.script()).unwrap();
    std::fs::write(
        temp.path().join("completion"),
        ShellIntegration::Zsh.completion_script(),
    )
    .unwrap();
    for phase in ["before", "after", "later"] {
        let driver = r#"
if [[ $PHASE == after ]]; then autoload -Uz compinit; compinit; fi
source "$STARTUP"
source "$STARTUP"
if [[ $PHASE != after ]]; then (( ! $+functions[compdef] )) || exit 21; fi
if [[ $PHASE == later ]]; then
    autoload -Uz compinit; compinit
    function custom_complete { zle .expand-or-complete }
    zle -N menu-complete custom_complete
    __zetta_completion_prompt_hook
    [[ $widgets[menu-complete] == user:custom_complete ]] || exit 22
fi
function zetta {
    if [[ $3 == --completions ]]; then
        print load >> "$COUNT"
        cat "$COMPLETION"
    else
        print "PROBE:$2"
    fi
}
PS1='READY> '
"#;
        std::fs::write(temp.path().join("driver"), driver).unwrap();
        let parent = r#"
zmodload zsh/zpty
zpty -b probe zsh -dfi
zpty -w probe 'source "$DRIVER"'
# Read boundedly: an assertion failure in the child must not hang the suite.
function read_until {
    local chunk all='' i
    for (( i=0; i<300; i++ )); do
        while zpty -r -t probe chunk; do
            all+=$chunk
            if [[ $all == *$1* ]]; then return 0; fi
        done
        sleep 0.02
    done
    print -u2 -- "$all"
    return 1
}
read_until 'READY> ' || { zpty -d probe; exit 30; }
zpty -w -n probe $'zetta init zs\t\n'
read_until 'PROBE:zsh' || { zpty -d probe; exit 31; }
zpty -w -n probe $'zetta init zs\t\n'
read_until 'PROBE:zsh' || { zpty -d probe; exit 32; }
zpty -d probe
(( $(wc -l < "$COUNT") == 1 )) || exit 33
"#;
        let count = temp.path().join(format!("count-{phase}"));
        let output = clean_shell_command("zsh")
            .arg("-df")
            .arg("-c")
            .arg(parent)
            .env("STARTUP", temp.path().join("startup"))
            .env("COMPLETION", temp.path().join("completion"))
            .env("DRIVER", temp.path().join("driver"))
            .env("COUNT", count)
            .env("PHASE", phase)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{phase}: {:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(windows)]
#[test]
fn powershell_startup_preserves_commands_without_autoloading_optional_modules() {
    let driver = include_str!("../../shell_integration/tests/command_discovery.ps1");
    for executable in ["powershell.exe", "pwsh.exe"] {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("startup.ps1"),
            ShellIntegration::PowerShell.script(),
        )
        .unwrap();
        let output = clean_shell_command(executable)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                driver,
            ])
            .env("FIXTURE", temp.path())
            .output();
        match output {
            Err(error)
                if executable == "pwsh.exe" && error.kind() == std::io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(error) => panic!("{executable}: {error}"),
            Ok(output) => assert!(
                output.status.success(),
                "{executable}: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }
}

#[test]
fn powershell_lazy_completion_replays_retries_and_retains_closure() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("startup.ps1"),
        ShellIntegration::PowerShell.script(),
    )
    .unwrap();
    std::fs::write(
        temp.path().join("completion.ps1"),
        ShellIntegration::PowerShell.completion_script(),
    )
    .unwrap();
    let driver = r#"
$ErrorActionPreference = 'Stop'
. (Join-Path $env:FIXTURE 'startup.ps1')
. (Join-Path $env:FIXTURE 'startup.ps1')
$global:Loads = 0
function global:zetta {
    if ($args[0] -eq 'profile') { 'Project Shell'; return }
    $global:Loads++
    $global:LASTEXITCODE = 0
    if ($global:Loads -eq 1) { $global:LASTEXITCODE = 1; return }
    if ($global:Loads -eq 2) { return }
    Get-Content -Raw (Join-Path $env:FIXTURE 'completion.ps1')
}
function Probe {
    [System.Management.Automation.CommandCompletion]::CompleteInput('zetta init --comp', 17, $null)
}
Probe | Out-Null
if ($null -ne $global:__ZettaCompletionState.Handler) { throw 'failure marked loaded' }
Probe | Out-Null
if ($null -ne $global:__ZettaCompletionState.Handler) { throw 'empty output marked loaded' }
$result = Probe
if ($result.CompletionMatches.CompletionText -notcontains '--completions') { throw 'first result missing' }
$result = Probe
if ($result.CompletionMatches.CompletionText -notcontains '--completions') { throw 'subsequent result missing' }
. (Join-Path $env:FIXTURE 'startup.ps1')
$result = Probe
if ($result.CompletionMatches.CompletionText -notcontains '--completions') { throw 'reinit lost result' }
$result = [System.Management.Automation.CommandCompletion]::CompleteInput('zetta --profile Pro', 19, $null)
if ($result.CompletionMatches.CompletionText -notcontains "'Project Shell'") { throw 'closure lost profile catalog' }
$result = [System.Management.Automation.CommandCompletion]::CompleteInput('zetta -p Pro', 12, $null)
if ($result.CompletionMatches.CompletionText -notcontains "'Project Shell'") { throw 'short option lost catalog' }
foreach ($line in @('zetta -p ', 'zetta --profile ""', "zetta --profile 'Pro'")) {
    $result = [System.Management.Automation.CommandCompletion]::CompleteInput($line, $line.Length, $null)
    if ($result.CompletionMatches.CompletionText -notcontains "'Project Shell'") { throw "empty profile value lost: $line" }
}
foreach ($line in @('zetta pane --direction l', 'zetta pane -d l')) {
    $result = [System.Management.Automation.CommandCompletion]::CompleteInput($line, $line.Length, $null)
    if ($result.CompletionMatches.CompletionText -notcontains 'left') { throw "direction value lost: $line" }
}
$result = [System.Management.Automation.CommandCompletion]::CompleteInput('zetta init -c zs', 16, $null)
if ($result.CompletionMatches.CompletionText -notcontains 'zsh') { throw 'init short boolean took a value' }
if ($global:Loads -ne 3) { throw "generated $global:Loads times" }
"#;
    for executable in ["powershell.exe", "pwsh.exe", "pwsh"] {
        let output = clean_shell_command(executable)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                driver,
            ])
            .env("FIXTURE", temp.path())
            .output();
        match output {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("{executable}: {error}"),
            Ok(output) => assert!(
                output.status.success(),
                "{executable}: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }
}

#[test]
fn zsh_lazy_completion_retries_failure_and_empty_output() {
    if !std::process::Command::new("zsh")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
    {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("startup"), ShellIntegration::Zsh.script()).unwrap();
    std::fs::write(
        temp.path().join("completion"),
        ShellIntegration::Zsh.completion_script(),
    )
    .unwrap();
    let driver = r#"
autoload -Uz compinit; compinit
source "$STARTUP"
function zetta {
    print load >> "$COUNT"
    case $(( $(wc -l < "$COUNT") )) in
        1) return 1 ;;
        2) return 0 ;;
        *) cat "$COMPLETION" ;;
    esac
}
function compadd { reply=("$@") }
words=(zetta pane --direction ''); CURRENT=4
__zetta_lazy_complete
(( ${__zetta_completions_loaded:-0} == 0 )) || exit 11
__zetta_lazy_complete
(( ${__zetta_completions_loaded:-0} == 0 )) || exit 12
__zetta_lazy_complete
[[ ${reply[*]} == *left* ]] || exit 13
__zetta_lazy_complete
source "$STARTUP"
[[ $_comps[zetta] == _zetta_entry_zetta ]] || exit 14
(( $(wc -l < "$COUNT") == 3 )) || exit 15
"#;
    let output = clean_shell_command("zsh")
        .arg("-df")
        .arg("-c")
        .arg(driver)
        .env("STARTUP", temp.path().join("startup"))
        .env("COMPLETION", temp.path().join("completion"))
        .env("COUNT", temp.path().join("count"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
