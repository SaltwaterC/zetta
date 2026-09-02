use super::*;

// On Windows, `bash.exe` is commonly the WSL launcher rather than a native
// Bash binary. Starting several WSL instances concurrently can make the
// launcher fail with no useful stderr, so keep all external Bash tests
// serialized. The lock is harmless on Unix and also covers the tests that
// invoke Bash as one shell among several.
fn lock_bash_tests() -> std::sync::MutexGuard<'static, ()> {
    static BASH_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    BASH_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn bash_command() -> std::process::Command {
    clean_shell_command("bash")
}

fn clean_shell_command(program: &str) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    // Do not let a user's shell startup environment change the exit status or
    // behavior of a generated script under test.
    command
        .env_remove("BASH_ENV")
        .env_remove("BASHOPTS")
        .env_remove("SHELLOPTS")
        .env_remove("ZETTA_PROCESS_ID")
        .env_remove("ZETTA_ATTENTION_ID")
        .env_remove("ZETTA_PANE_ID")
        .env_remove("ZETTA_PANE_ROUTING_ID");
    command
}

#[test]
fn supported_shells_generate_completion_and_tftp_shortcut() {
    let help = shell_integration_help();
    assert!(help.contains("--replace-pane"));
    assert!(help.contains("--new-window"));
    assert!(help.contains("--command"));
    assert!(help.contains("zetta pane"));
    #[cfg(feature = "worktree")]
    assert!(help.contains("standalone Git worktree command"));
    #[cfg(not(feature = "worktree"))]
    assert!(!help.contains("Git worktree command"));
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(script.contains("ztftp"));
        assert!(script.contains("tftp"));
        assert!(script.contains("serial"));
        assert!(script.contains("http"));
        assert!(script.contains("init"));
        assert!(script.contains("EDITOR"));
        assert!(script.contains("zetta vi"));
        assert!(script.contains("zetta tabicon --list"));
        assert!(script.contains("zetta theme"));
        assert!(script.contains("zetta splits"));
        assert!(script.contains("zetta pane --list"));
        assert!(script.contains("zetta cmd --list"));
        assert!(script.contains("ZETTA_NO_MUX"));
        assert!(script.contains("--direction"));
        assert!(script.contains("--pane"));
        assert!(script.contains("zetta-event:tracking-ready"));
        assert!(script.contains("zetta-event:command-started:"));
        assert!(script.contains("zetta-event:command-finished:"));
        assert!(!script.contains("run --wait"));
        assert!(script.contains("allow-failure"));
        assert!(script.contains("replace-pane"));
        assert!(script.contains("--new-window"));
        assert!(!script.contains(" -w"));
        assert!(script.contains("--command"));
        #[cfg(feature = "worktree")]
        {
            assert!(script.contains("zwt"));
            assert!(script.contains("wt"));
        }
        assert!(script.contains("zmux"));
        #[cfg(feature = "worktree")]
        for operation in ["new", "done", "abort", "status", "sync", "config"] {
            assert!(script.contains(operation));
        }
        #[cfg(feature = "worktree")]
        {
            assert!(script.contains("--path-only"));
            assert!(script.contains("--copy"));
        }
    }
}

#[test]
fn bash_pane_wait_completion_fetches_origin_pane_labels_and_preserves_commas() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "{script}\nprintf '\\n'\nzetta() {{ if [[ $1 == pane && $2 == --list ]]; then printf '%s\\n' api db deploy; fi; }}\nCOMP_WORDS=(zetta pane wait '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'first:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta pane wait api,)\nCOMP_CWORD=3\n_zetta_complete\nprintf 'second:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta pane wait api -- '')\nCOMP_CWORD=5\n_zetta_complete\nprintf 'after-delimiter:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash pane wait completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    for label in ["api", "db", "deploy"] {
        assert!(
            completions
                .lines()
                .any(|line| line == format!("first:{label}")),
            "missing first pane wait label {label:?}: {completions}"
        );
    }
    assert!(completions.lines().any(|line| line == "second:api,db"));
    assert!(completions.lines().any(|line| line == "second:api,deploy"));
    assert!(!completions.lines().any(|line| line == "second:api,api"));
    assert!(completions.lines().any(|line| line == "after-delimiter:"));
}

#[test]
fn bash_project_command_completion_is_dynamic_and_stops_at_delimiter() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "{script}\nzetta() {{ if [[ $1 == cmd && $2 == --list ]]; then printf '%s\\n' build check test:unit; fi; }}\nCOMP_WORDS=(zetta cmd '')\nCOMP_CWORD=2\n_zetta_complete\nprintf 'names:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd t)\nCOMP_CWORD=2\n_zetta_complete\nprintf 'prefix:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd build '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'after-name:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd build -- '')\nCOMP_CWORD=4\n_zetta_complete\nprintf 'after-delimiter:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd --list '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'after-list:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .env_remove("ZETTA_HOST_EXECUTABLE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash project command completion failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    for name in ["build", "check", "test:unit"] {
        assert!(
            completions
                .lines()
                .any(|line| line == format!("names:{name}")),
            "missing dynamic project command {name:?}: {completions}"
        );
    }
    for option in ["--help", "--list", "--"] {
        assert!(
            completions
                .lines()
                .any(|line| line == format!("names:{option}")),
            "missing project command option {option:?} at the command-name prompt: {completions}"
        );
    }
    assert!(completions.lines().any(|line| line == "prefix:test:unit"));
    assert!(completions.lines().any(|line| line == "after-name:--help"));
    assert!(completions.lines().any(|line| line == "after-name:--"));
    assert!(!completions.lines().any(|line| line == "after-name:--list"));
    assert!(completions.lines().any(|line| line == "after-delimiter:"));
    assert!(completions.lines().any(|line| line == "after-list:"));
}

#[test]
fn zsh_project_command_completion_offers_long_options_at_each_option_prompt() {
    use std::io::Write as _;
    use std::process::Stdio;

    if clean_shell_command("zsh")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let script = ShellIntegration::Zsh.script();
    let driver = format!(
        "{script}\nfunction zetta {{ if [[ $1 == cmd && $2 == --list ]]; then print -r -- build check; fi; }}\nfunction compadd {{ print -r -- \"${{stage}}:candidates:$*\"; }}\nfunction _zetta_options {{ print -r -- \"${{stage}}:options:$*\"; }}\nstage=empty\nwords=(zetta cmd '')\nCURRENT=3\n_zetta\nstage=name\nwords=(zetta cmd build '')\nCURRENT=4\n_zetta\nstage=list\nwords=(zetta cmd --list '')\nCURRENT=4\n_zetta\n"
    );
    let mut child = clean_shell_command("zsh")
        .arg("-f")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Zsh project command completion failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    assert!(
        completions
            .lines()
            .any(|line| line == "empty:candidates:-- build check"),
        "unexpected Zsh command-name candidates: {completions}"
    );
    assert!(
        completions
            .lines()
            .any(|line| line == "empty:options:--help --list --")
    );
    assert!(
        completions
            .lines()
            .any(|line| line == "name:options:--help --")
    );
    assert!(!completions.lines().any(|line| line.starts_with("list:")));
}

#[cfg(not(feature = "worktree"))]
#[test]
fn generated_shell_scripts_omit_worktree_integration_when_disabled() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(!script.contains("zwt"), "{shell:?} emitted zwt integration");
        assert!(
            !script.contains("ZETTA_WORKTREE"),
            "{shell:?} emitted a worktree template marker"
        );
        assert!(!script.contains("--path-only"));
        assert!(!script.contains("--copy"));
    }
}

#[test]
fn shell_integration_reports_the_shell_cwd_while_children_run() {
    assert!(
        ShellIntegration::Bash
            .script()
            .contains("__zetta_report_cwd")
    );
    assert!(
        ShellIntegration::Fish
            .script()
            .contains("--on-event fish_prompt")
    );
    assert!(
        ShellIntegration::PowerShell
            .script()
            .contains("zetta-cwd:$zettaDirectory")
    );
    assert!(
        ShellIntegration::Zsh
            .script()
            .contains("add-zsh-hook precmd __zetta_report_cwd")
    );
}

#[test]
fn zsh_lifecycle_tracker_does_not_assign_to_read_only_status() {
    let zsh = ShellIntegration::Zsh.script();

    assert!(zsh.contains("local zetta_status=$?"));
    assert!(!zsh.contains("local status=$?"));
    assert!(zsh.contains("__ZETTA_LIFECYCLE_TRACKING_VERSION:-0} != 3"));
}

#[test]
fn zsh_integration_filters_zetta_startup_history() {
    let zsh = ShellIntegration::Zsh.script();

    assert!(zsh.contains("function __zetta_filter_startup_history()"));
    assert!(zsh.contains("__zed_init_command_history_"));
    assert!(zsh.contains("fc -p"));
    assert!(zsh.contains("add-zsh-hook zshaddhistory __zetta_filter_startup_history"));
}

#[test]
fn fish_integration_filters_zetta_startup_history() {
    let fish = ShellIntegration::Fish.script();

    assert!(fish.contains("function __zetta_capture_startup_history"));
    assert!(fish.contains("--on-event fish_postexec"));
    assert!(fish.contains("__zed_init_command_history_"));
    assert!(fish.contains("builtin history delete --case-sensitive --exact"));
    assert!(fish.contains("commandline -f repaint"));
    assert!(fish.contains("function __zetta_remove_startup_history"));
    assert!(fish.contains("--on-event fish_prompt"));
}

#[test]
fn zsh_reload_replaces_an_already_installed_broken_tracker() {
    use std::io::Write as _;
    use std::process::Stdio;

    let script = ShellIntegration::Zsh.script();
    let driver = format!(
        "{prelude}\n{script}\n__ZETTA_COMMAND_STARTED=1\nfalse\n__zetta_report_cwd\nexit 0\n",
        prelude = r#"function __zetta_report_cwd() {
    local status=$?
    return $status
}
typeset -g __ZETTA_LIFECYCLE_TRACKING_INSTALLED=1
typeset -g __ZETTA_LIFECYCLE_TRACKING_ENABLED=1
typeset -g __ZETTA_LIFECYCLE_TRACKING_VERSION=2"#,
        script = script,
    );
    let mut child = match clean_shell_command("zsh")
        .env("ZETTA_PANE_ID", "7")
        .args(["-f"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("failed to launch zsh: {error}"),
    };
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "zsh failed to reload its lifecycle tracker:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("zetta-event:command-finished:1"),
        "{stdout}"
    );
}

#[test]
fn powershell_cwd_tracker_uses_the_shared_idempotence_guard() {
    let script = ShellIntegration::PowerShell.script();

    assert!(script.contains("Get-Variable -Name __ZettaCwdTrackerInstalled -Scope Global"));
    assert!(script.contains("$global:__ZettaCwdTrackerInstalled = $true"));
    assert!(script.contains("$global:__ZettaOriginalPrompt = $function:prompt"));
    assert!(script.contains("& $global:__ZettaOriginalPrompt"));
    assert!(!script.contains("__ZettaShellIntegrationOriginalPrompt"));
}

#[test]
fn powershell_cwd_trackers_reset_before_the_saved_prompt() {
    const TERMINAL_TRACKER: &str =
        include_str!("../../crates/terminal/src/terminal/powershell_cwd_tracker.ps1");
    const MARKER_WRITE: &str =
        r#"[Console]::Write("$([char]27)]2;zetta-cwd:$zettaDirectory$([char]27)\")"#;
    const RESET_WRITE: &str = r#"[Console]::Write("$([char]27)[0m")"#;

    for (name, script) in [
        ("shell integration", ShellIntegration::PowerShell.script()),
        ("terminal tracker", TERMINAL_TRACKER.to_owned()),
    ] {
        let marker = script
            .find(MARKER_WRITE)
            .unwrap_or_else(|| panic!("{name} must write the CWD marker"));
        let reset = script
            .find(RESET_WRITE)
            .unwrap_or_else(|| panic!("{name} must reset the console style"));
        let prompt = script
            .find("if ($null -ne $global:__ZettaOriginalPrompt)")
            .unwrap_or_else(|| panic!("{name} must invoke the saved prompt"));

        assert_eq!(
            script.matches(MARKER_WRITE).count(),
            1,
            "{name} must have one CWD marker write"
        );
        assert_eq!(
            script.matches(RESET_WRITE).count(),
            1,
            "{name} must have one prompt reset write"
        );
        assert_eq!(
            script.matches("function global:prompt").count(),
            1,
            "{name} must install the prompt only once"
        );
        assert!(
            script[marker..].starts_with(&format!("{MARKER_WRITE}\n            {RESET_WRITE}")),
            "{name} must reset immediately after its CWD marker"
        );
        assert!(
            marker < reset && reset < prompt,
            "{name} reset must precede prompt"
        );
    }
}

#[test]
fn vi_integration_is_conditional_and_has_cli_completion() {
    let bash = ShellIntegration::Bash.script();
    assert!(bash.contains("if ! type -t vi >/dev/null 2>&1"));
    assert!(bash.contains("eval 'vi() { zetta vi \"$@\"; }'"));
    assert!(bash.contains("zvi() { zetta vi \"$@\"; }"));
    assert!(bash.contains("ZETTA_HOST_EXECUTABLE"));
    assert!(bash.contains("complete -F _zetta_complete zvi"));
    assert!(bash.contains("vi)\n            if [[ $current == -* ]]; then"));

    let fish = ShellIntegration::Fish.script();
    assert!(fish.contains("if not type -q vi"));
    assert!(fish.contains("complete -c vi -F"));
    assert!(fish.contains("function zvi --wraps 'zetta vi'"));
    assert!(fish.contains("complete -c zvi -F"));
    assert!(fish.contains("function __zetta_option_unused"));
    assert!(fish.contains("ZETTA_HOST_EXECUTABLE"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(powershell.contains("$zettaViMissing = -not (Get-Command vi"));
    assert!(powershell.contains("if ($zettaViMissing)"));
    assert!(powershell.contains("function zvi { & zetta vi @args }"));
    assert!(powershell.contains("Register-ArgumentCompleter -CommandName zvi"));
    assert!(powershell.contains("Get-ChildItem -Name -Path \"$wordToComplete*\""));
    assert!(powershell.contains("$_ -notin $words"));

    let zsh = ShellIntegration::Zsh.script();
    assert!(zsh.contains("$+commands[vi]"));
    assert!(zsh.contains("compdef _zetta vi"));
    assert!(zsh.contains("function zvi { zetta vi \"$@\"; }"));
    assert!(zsh.contains("ZETTA_HOST_EXECUTABLE"));
    #[cfg(feature = "worktree")]
    {
        assert!(zsh.contains("local worktree_path path_only_arg"));
        assert!(!zsh.contains("local path path_only_arg"));
    }
    assert!(zsh.contains("compdef _zetta zvi"));
    assert!(zsh.contains("_zetta_option_unused"));
    assert!(zsh.contains("_zetta_options()"));
    assert!(zsh.contains("_files"));
}

#[test]
fn bash_does_not_repeat_options_and_completes_vi_files() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "{script}\nCOMP_WORDS=(zetta vi --)\nCOMP_CWORD=2\n_zetta_complete\nprintf 'option:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta vi --help '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'file:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta vi Carg)\nCOMP_CWORD=2\n_zetta_complete\nprintf 'file:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    assert!(completions.lines().any(|line| line == "option:--help"));
    assert!(!completions.lines().any(|line| line == "file:--help"));
    assert!(completions.lines().any(|line| line == "file:Cargo.toml"));
}

#[test]
fn bash_color_completion_offers_named_presets_for_long_and_short_flags() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "{script}\nCOMP_WORDS=(zetta overlay --color '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'long:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta overlay -c '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'short:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    for prefix in ["long:", "short:"] {
        for preset in OVERLAY_COLOR_PRESETS {
            assert!(
                completions
                    .lines()
                    .any(|line| line == format!("{prefix}{}", preset.name)),
                "expected {} after {prefix}: {completions}",
                preset.name
            );
        }
    }
}

#[cfg(feature = "worktree")]
#[test]
fn bash_worktree_completion_offers_operations_and_long_worktree_options() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "{script}\ngit() {{ case \"$1 $2\" in branch\\ --show-current) printf '%s\\n' wt/feature ;; config\\ --local) printf '%s\\n' main ;; merge-base*) printf '%s\\n' split ;; rev-list*) printf '%s\\n' commit-one commit-two ;; esac; }}\nCOMP_WORDS=(zetta wt '')\nCOMP_CWORD=2\n_zetta_complete\nprintf 'operation:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new --)\nCOMP_CWORD=3\n_zetta_complete\nprintf 'option:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new --copy Carg)\nCOMP_CWORD=4\n_zetta_complete\nprintf 'copy-path:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new -c Carg)\nCOMP_CWORD=4\n_zetta_complete\nprintf 'short-copy-path:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt sync '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'sync-commit:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zwt '')\nCOMP_CWORD=1\n_zetta_complete_zwt\nprintf 'wrapper:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zwt sync '')\nCOMP_CWORD=2\n_zetta_complete_zwt\nprintf 'wrapper-sync-commit:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    for prefix in ["operation:", "wrapper:"] {
        for operation in ["new", "done", "abort", "status", "sync", "config"] {
            assert!(
                completions
                    .lines()
                    .any(|line| line == format!("{prefix}{operation}")),
                "expected {operation} after {prefix}: {completions}"
            );
        }
    }
    for prefix in ["sync-commit:", "wrapper-sync-commit:"] {
        for commit in ["commit-one", "commit-two"] {
            assert!(
                completions
                    .lines()
                    .any(|line| line == format!("{prefix}{commit}")),
                "expected {commit} after {prefix}: {completions}"
            );
        }
    }
    assert!(completions.lines().any(|line| line == "option:--path-only"));
    assert!(completions.lines().any(|line| line == "option:--copy"));
    assert!(!completions.lines().any(|line| line == "option:-P"));
    assert!(!completions.lines().any(|line| line == "option:-c"));
    assert!(
        completions
            .lines()
            .any(|line| line == "copy-path:Cargo.toml")
    );
    assert!(
        completions
            .lines()
            .any(|line| line == "short-copy-path:Cargo.toml")
    );
}

#[test]
fn bash_zmux_completes_the_same_as_zetta_mux() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = "\
zetta() { if [[ $1 == mux && $2 == list ]]; then printf '%s\\n' '  reconnect id: 12345:7:42 (short: 42)'; fi; }\n\
zmux() { if [[ $1 == list ]]; then printf '%s\\n' '  reconnect id: 12345:7:42 (short: 42)'; fi; }\n\
COMP_WORDS=(zetta mux '')\nCOMP_CWORD=2\n_zetta_complete\nprintf 'mux:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux '')\nCOMP_CWORD=1\n_zetta_complete_zmux\nprintf 'zmux:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zetta mux share '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'mux-share:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux share '')\nCOMP_CWORD=2\n_zetta_complete_zmux\nprintf 'zmux-share:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux unshare '')\nCOMP_CWORD=2\n_zetta_complete_zmux\nprintf 'zmux-unshare:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zetta mux stop --)\nCOMP_CWORD=3\n_zetta_complete\nprintf 'mux-stop:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux stop --)\nCOMP_CWORD=2\n_zetta_complete_zmux\nprintf 'zmux-stop:%s\\n' \"${COMPREPLY[@]}\"\n\
ZETTA_NO_MUX=1\n\
COMP_WORDS=(zetta mux '')\nCOMP_CWORD=2\n_zetta_complete\nprintf 'no-mux:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux '')\nCOMP_CWORD=1\n_zetta_complete_zmux\nprintf 'no-mux-zmux:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zetta mux reconnect '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'no-mux-reconnect:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zetta mux share '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'no-mux-share:%s\\n' \"${COMPREPLY[@]}\"\n";
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{script}\n{driver}").as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    let candidates = |prefix: &str| -> Vec<&str> {
        completions
            .lines()
            .filter_map(|line| line.strip_prefix(prefix))
            .collect()
    };
    let mux = candidates("mux:");
    let zmux = candidates("zmux:");
    assert!(!mux.is_empty());
    assert_eq!(mux, zmux, "zmux should complete the same as zetta mux");
    for prefix in ["mux-share:", "zmux-share:", "zmux-unshare:"] {
        assert_eq!(
            candidates(prefix),
            vec!["12345:7:42"],
            "{prefix} should offer the full reconnect identifier"
        );
    }
    let mux_stop = candidates("mux-stop:");
    let zmux_stop = candidates("zmux-stop:");
    assert!(mux_stop.contains(&"--force"));
    assert_eq!(
        mux_stop, zmux_stop,
        "zmux stop should complete the same as zetta mux stop"
    );
    let no_mux = candidates("no-mux:");
    let no_mux_zmux = candidates("no-mux-zmux:");
    assert_eq!(
        no_mux, no_mux_zmux,
        "no-mux zmux completion should match zetta mux"
    );
    for unavailable in ["stop", "share", "unshare", "kill", "forget", "--upgrade"] {
        assert!(
            !no_mux.contains(&unavailable),
            "no-mux completion offered daemon-only candidate {unavailable:?}: {no_mux:?}"
        );
    }
    assert_eq!(
        candidates("no-mux-reconnect:"),
        vec!["12345:7:42"],
        "reconnect remains available for local sessions"
    );
    assert!(
        candidates("no-mux-share:")
            .iter()
            .all(|candidate| candidate.is_empty()),
        "share should not be completed without a daemon"
    );
}

#[test]
fn zsh_and_powershell_wire_up_zmux_completion() {
    let zsh = ShellIntegration::Zsh.script();
    assert!(zsh.contains("_zmux()"));
    assert!(zsh.contains("words=(zetta mux \"${words[@]:1}\")"));
    assert!(zsh.contains("compdef _zmux zmux"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(powershell.contains("Register-ArgumentCompleter -Native -CommandName zmux"));
    assert!(powershell.contains("if ($commandName -eq 'zmux')"));
}

#[cfg(all(unix, feature = "worktree"))]
#[test]
fn bash_zwt_changes_directory_for_nested_paths_with_spaces() {
    use std::{io::Write as _, os::unix::fs::PermissionsExt as _, process::Stdio};

    let _bash_test_lock = lock_bash_tests();
    let temporary = tempfile::tempdir().unwrap();
    let start = temporary.path().join("start directory");
    let new_path = temporary.path().join("new worktree/feature api");
    let done_path = temporary.path().join("source worktree");
    let abort_path = temporary.path().join("abort source worktree");
    std::fs::create_dir_all(&start).unwrap();
    std::fs::create_dir_all(&new_path).unwrap();
    std::fs::create_dir_all(&done_path).unwrap();
    std::fs::create_dir_all(&abort_path).unwrap();

    let fake_zwt = temporary.path().join("zwt");
    std::fs::write(
        &fake_zwt,
        "#!/bin/sh\ncase \"$1\" in\n  new) printf '%s\\n' \"$ZETTA_TEST_NEW\" ;;\n  done) printf '%s\\n' \"$ZETTA_TEST_DONE\" ;;\n  abort) printf '%s\\n' \"$ZETTA_TEST_ABORT\" ;;\n  *) exit 0 ;;\nesac\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_zwt).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_zwt, permissions).unwrap();

    let mut path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
    paths.insert(0, temporary.path().to_owned());
    path = std::env::join_paths(paths).unwrap();

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "{script}\ncd '{}'\nzwt new --path-only 'feature/api'\nprintf 'new:%s\\n' \"$PWD\"\nzwt done --path-only\nprintf 'done:%s\\n' \"$PWD\"\nzwt abort\nprintf 'abort:%s\\n' \"$PWD\"\n",
        start.display()
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .current_dir(&start)
        .env_remove("ZETTA_HOST_EXECUTABLE")
        .env("PATH", path)
        .env("ZETTA_TEST_NEW", &new_path)
        .env("ZETTA_TEST_DONE", &done_path)
        .env("ZETTA_TEST_ABORT", &abort_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash zwt wrapper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(
        output
            .lines()
            .any(|line| line == format!("new:{}", new_path.display()))
    );
    assert!(
        output
            .lines()
            .any(|line| line == format!("done:{}", done_path.display()))
    );
    assert!(
        output
            .lines()
            .any(|line| line == format!("abort:{}", abort_path.display()))
    );
}

#[cfg(feature = "worktree")]
#[test]
fn worktree_wrappers_pass_help_through_without_capturing_it_as_a_path() {
    let bash = ShellIntegration::Bash.script();
    assert!(bash.contains("$path_only_arg == --help || $path_only_arg == -h"));
    assert!(bash.contains("command zwt abort --path-only"));

    let zsh = ShellIntegration::Zsh.script();
    assert!(zsh.contains("$path_only_arg == --help || $path_only_arg == -h"));
    assert!(zsh.contains("command zwt abort --path-only"));

    let fish = ShellIntegration::Fish.script();
    assert!(fish.contains("contains -- --help $operation_args; or contains -- -h $operation_args"));
    assert!(fish.contains("command zwt abort --path-only"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(
        powershell.contains("$operationArgs -contains '--help' -or $operationArgs -contains '-h'")
    );
    assert!(powershell.contains("& $zwtApplication abort --path-only"));
}

#[cfg(all(unix, feature = "worktree"))]
#[test]
fn posix_zwt_help_does_not_change_directory_or_inject_path_only() {
    use std::{io::Write as _, os::unix::fs::PermissionsExt as _, process::Stdio};

    let _bash_test_lock = lock_bash_tests();
    let temporary = tempfile::tempdir().unwrap();
    let start = temporary.path().join("start directory");
    let args_file = temporary.path().join("zwt arguments");
    std::fs::create_dir_all(&start).unwrap();

    let fake_zwt = temporary.path().join("zwt");
    std::fs::write(
        &fake_zwt,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ZETTA_TEST_ARGS\"\nprintf '%s\\n' 'Create a Git worktree for a temporary wt/NAME branch' 'Usage: zwt new [OPTIONS] NAME'\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_zwt).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_zwt, permissions).unwrap();

    let mut path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
    paths.insert(0, temporary.path().to_owned());
    path = std::env::join_paths(paths).unwrap();

    for shell in ["bash", "zsh"] {
        let version = if shell == "bash" {
            bash_command().arg("--version").output()
        } else {
            clean_shell_command(shell).arg("--version").output()
        };
        if version.is_err() {
            continue;
        }

        let script = ShellIntegration::parse(shell).unwrap().script();
        let prefix = if shell == "zsh" {
            "compdef() { :; }\n"
        } else {
            ""
        };
        let driver = format!(
            "{prefix}{script}\ncd '{}'\nzwt new --help\nprintf 'cwd:%s\\n' \"$PWD\"\n",
            start.display()
        );

        let mut command = if shell == "bash" {
            bash_command()
        } else {
            clean_shell_command(shell)
        };
        if shell == "bash" {
            command.args(["--noprofile", "--norc"]);
        } else {
            command.arg("-f");
        }
        let mut child = command
            .env_remove("ZETTA_HOST_EXECUTABLE")
            .current_dir(&start)
            .env("PATH", &path)
            .env("ZETTA_TEST_ARGS", &args_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(driver.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{shell} zwt help wrapper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout
                .lines()
                .any(|line| line == format!("cwd:{}", start.display())),
            "{shell} zwt help wrapper changed directory: {stdout}"
        );
        assert_eq!(
            std::fs::read_to_string(&args_file).unwrap(),
            "new\n--help\n",
            "{shell} zwt help wrapper changed the CLI arguments"
        );
    }
}

#[cfg(all(unix, feature = "worktree"))]
#[test]
fn posix_zwt_sync_and_config_pass_through_without_changing_directory() {
    use std::{io::Write as _, os::unix::fs::PermissionsExt as _, process::Stdio};

    let _bash_test_lock = lock_bash_tests();
    let temporary = tempfile::tempdir().unwrap();
    let start = temporary.path().join("start directory");
    let args_file = temporary.path().join("zwt arguments");
    std::fs::create_dir_all(&start).unwrap();

    let fake_zwt = temporary.path().join("zwt");
    std::fs::write(
        &fake_zwt,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ZETTA_TEST_ARGS.$1\"\nprintf 'passed:%s\\n' \"$1\"\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_zwt).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_zwt, permissions).unwrap();

    let mut path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
    paths.insert(0, temporary.path().to_owned());
    path = std::env::join_paths(paths).unwrap();

    for shell in ["bash", "zsh"] {
        let version = if shell == "bash" {
            bash_command().arg("--version").output()
        } else {
            clean_shell_command(shell).arg("--version").output()
        };
        if version.is_err() {
            continue;
        }

        let script = ShellIntegration::parse(shell).unwrap().script();
        let prefix = if shell == "zsh" {
            "compdef() { :; }\n"
        } else {
            ""
        };
        let driver = format!(
            "{prefix}{script}\ncd '{}'\nzwt sync target\nprintf 'sync-cwd:%s\\n' \"$PWD\"\nzwt config\nprintf 'config-cwd:%s\\n' \"$PWD\"\n",
            start.display()
        );

        let mut command = if shell == "bash" {
            bash_command()
        } else {
            clean_shell_command(shell)
        };
        if shell == "bash" {
            command.args(["--noprofile", "--norc"]);
        } else {
            command.arg("-f");
        }
        let mut child = command
            .env_remove("ZETTA_HOST_EXECUTABLE")
            .current_dir(&start)
            .env("PATH", &path)
            .env("ZETTA_TEST_ARGS", &args_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(driver.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{shell} zwt pass-through failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.lines().any(|line| line == "passed:sync"));
        assert!(stdout.lines().any(|line| line == "passed:config"));
        assert!(
            stdout
                .lines()
                .any(|line| line == format!("sync-cwd:{}", start.display()))
        );
        assert!(
            stdout
                .lines()
                .any(|line| line == format!("config-cwd:{}", start.display()))
        );
        assert_eq!(
            std::fs::read_to_string(format!("{}.sync", args_file.display())).unwrap(),
            "sync\ntarget\n"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{}.config", args_file.display())).unwrap(),
            "config\n"
        );
    }
}

#[test]
fn supported_shells_generate_notify_completion_and_zntfy_shortcut() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(script.contains("zntfy"));
        assert!(script.contains("notify"));
        if shell == ShellIntegration::Fish {
            assert!(script.contains("-l app-name"));
            assert!(script.contains("-l icon"));
            assert!(script.contains("-l sound"));
            assert!(script.contains("-l timeout"));
        } else {
            assert!(script.contains("--app-name"));
            assert!(script.contains("--icon"));
            assert!(script.contains("--sound"));
            assert!(script.contains("--timeout"));
        }
    }
}

#[test]
fn supported_shells_generate_notify_cleanup_completion() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(script.contains("notify") && script.contains("cleanup"));
        if shell == ShellIntegration::Fish {
            assert!(script.contains("-l dry-run"));
        } else {
            assert!(script.contains("--dry-run"));
        }
    }
}

#[test]
fn supported_shells_generate_attention_completion() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(script.contains("attention"));
        if shell == ShellIntegration::Fish {
            assert!(script.contains("-l notify"));
        } else {
            assert!(script.contains("--notify"));
        }
        assert!(script.contains("--app-name"));
        assert!(script.contains("--timeout"));
    }
}

#[test]
fn supported_shells_generate_copy_paste_completion_and_shortcuts() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(script.contains("zcopy"));
        assert!(script.contains("zpaste"));
        assert!(script.contains("copy"));
        assert!(script.contains("paste"));
        if shell == ShellIntegration::Fish {
            assert!(script.contains("-l pboard"));
            assert!(script.contains("-l prefer"));
        } else {
            assert!(script.contains("--pboard"));
            assert!(script.contains("--prefer"));
        }
    }
}

// Regression guard: pbcopy/pbpaste already exist natively on macOS, so Zetta
// must not shadow them there, but every other platform should get them so
// pbcopy/pbpaste muscle memory keeps working.
#[test]
fn pbcopy_and_pbpaste_are_gated_to_non_macos_platforms() {
    let bash = ShellIntegration::Bash.script();
    assert!(bash.contains("pbcopy"));
    assert!(bash.contains("pbpaste"));
    assert!(bash.contains("unalias pbcopy pbpaste"));
    assert!(bash.contains("darwin*) ;;"));

    let zsh = ShellIntegration::Zsh.script();
    assert!(zsh.contains("pbcopy"));
    assert!(zsh.contains("pbpaste"));
    assert!(zsh.contains("unalias pbcopy pbpaste"));
    assert!(zsh.contains("darwin*) ;;"));

    let fish = ShellIntegration::Fish.script();
    assert!(fish.contains("pbcopy"));
    assert!(fish.contains("pbpaste"));
    assert!(fish.contains("functions -e pbcopy pbpaste"));
    assert!(fish.contains("case Darwin\n    case '*'"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(powershell.contains("pbcopy"));
    assert!(powershell.contains("pbpaste"));
    assert!(powershell.contains("if (-not $IsMacOS) {"));
    assert!(powershell.contains("Remove-Item -Path Alias:pbcopy,Alias:pbpaste"));
}

// Regression test: zsh expands an active alias while parsing a `name() {
// ... }` function definition of the same name, which fails to parse
// ("defining function based on alias") even when a preceding `unalias`
// removes it, because the whole `case` branch is parsed as one unit before
// any of it runs. `zsh -n` (syntax check only) does not catch this, since it
// depends on the alias actually being defined; only executing the script
// with a preexisting pbcopy/pbpaste alias (as a real user's zshrc would
// have) reproduces it. Zetta must use `function name { ... }` there instead.
#[test]
fn zsh_accepts_the_generated_integration_with_a_preexisting_pbcopy_alias() {
    let script = ShellIntegration::Zsh.script();
    let combined = format!(
        "alias pbcopy='xclip -selection clipboard'\nalias pbpaste='xclip -selection clipboard -o'\n{script}"
    );

    let mut child = match clean_shell_command("zsh")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("failed to launch zsh: {error}"),
    };
    child
        .stdin
        .take()
        .unwrap()
        .write_all(combined.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "zsh rejected the generated integration with a preexisting pbcopy/pbpaste alias:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sound_completion_calls_a_shared_helper_from_every_call_site() {
    let bash = ShellIntegration::Bash.script();
    assert!(bash.contains("--sound)\n            _zetta_complete_sound_names"));
    assert!(bash.contains("--sound|-s)\n            _zetta_complete_sound_names"));
    assert!(bash.contains(
        "elif [[ $command == notify || $command == attention ]]; then\n                _zetta_complete_sound_names"
    ));

    let zsh = ShellIntegration::Zsh.script();
    assert!(zsh.contains("--sound)\n            _zetta_sound_names"));
    assert!(zsh.contains("--sound|-s)\n            _zetta_sound_names"));
    assert!(
        zsh.contains(
            "elif [[ $words[2] == notify || $words[2] == attention ]]; then\n                _zetta_sound_names"
        )
    );

    let fish = ShellIntegration::Fish.script();
    assert!(fish.contains("-l sound -r -a '(__zetta_sound_names)'"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(powershell.contains("elseif ($previous -in '--sound', '-s') { $zettaSoundNames }"));
    assert!(powershell.contains("elseif ($previous -eq '--sound') {\n        $zettaSoundNames"));
    assert!(
        powershell.contains("elseif ($subcommand -in 'notify', 'attention') { $zettaSoundNames }")
    );
}

// Regression guard: a flat, unconditional merge of every platform's sound
// names is confusing (e.g. offering macOS's "Glass" while completing on
// Linux, where it does not work). Each shell must detect the actual host
// platform at completion time and only offer that platform's own names,
// alongside the bundled zetta-* tones which work everywhere.
#[test]
fn sound_completion_is_scoped_to_the_detected_platform() {
    let bundled = ["zetta-default", "zetta-ok", "zetta-alarm", "zetta-gong"];
    let linux_only = ["bell", "message-new-instant", "trash-empty"];
    let macos_only = ["Basso", "Glass", "Sosumi"];
    let windows_only = ["IM", "Reminder", "SMS"];

    let bash = ShellIntegration::Bash.script();
    assert!(bash.contains("case \"$OSTYPE\" in"));
    assert!(bash.contains("darwin*)"));
    assert!(bash.contains("msys*|cygwin*|win32*)"));

    let zsh = ShellIntegration::Zsh.script();
    assert!(zsh.contains("case \"$OSTYPE\" in"));
    assert!(zsh.contains("darwin*)"));
    assert!(zsh.contains("msys*|cygwin*|win32*)"));

    let fish = ShellIntegration::Fish.script();
    assert!(fish.contains("switch (uname)"));
    assert!(fish.contains("case Darwin"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(powershell.contains("if ($IsMacOS) {"));
    assert!(powershell.contains("} elseif ($IsLinux) {"));

    // Fish has no Windows branch (fish does not target native Windows here).
    for script in [&bash, &zsh, &fish, &powershell] {
        for name in bundled {
            assert!(
                script.contains(name),
                "expected {name:?} to always be offered"
            );
        }
        for name in linux_only {
            assert!(
                script.contains(name),
                "expected the Linux-only name {name:?} to be gated to a Linux branch"
            );
        }
        for name in macos_only {
            assert!(
                script.contains(name),
                "expected the macOS-only name {name:?} to be gated to a macOS branch"
            );
        }
    }
    for script in [&bash, &zsh, &powershell] {
        for name in windows_only {
            assert!(
                script.contains(name),
                "expected the Windows-only name {name:?} to be gated to a Windows branch"
            );
        }
    }
}

// Regression guard: --timeout shares its short form (-t) with
// benchmark output's --output-type, and the top-level/theme --theme flag
// now shares it too. Completion after -t/--output-type/--theme must stay
// scoped to the active subcommand instead of always suggesting
// benchmark output's repeated/unique values.
#[test]
fn notify_timeout_completion_does_not_leak_into_other_short_t_flags() {
    let bash = ShellIntegration::Bash.script();
    assert!(bash.contains("elif [[ $command == theme && ${COMP_WORDS[2]} == pane ]]; then"));
    assert!(bash.contains("elif [[ $command == theme && ${COMP_WORDS[2]} == tab ]]; then"));
    assert!(bash.contains("elif [[ $command == benchmark && ${COMP_WORDS[2]} == output ]]; then"));
    assert!(bash.contains("_zetta_compgen 'repeated unique'"));

    let zsh = ShellIntegration::Zsh.script();
    assert!(zsh.contains(
        "elif [[ $words[2] == theme && ($words[3] == pane || $words[3] == tab) ]]; then"
    ));
    assert!(zsh.contains("elif [[ $words[2] == benchmark && $words[3] == output ]]; then"));
    assert!(zsh.contains("compadd -- repeated unique"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(powershell.contains(
        "elseif ($subcommand -eq 'theme' -and $words.Count -gt 2 -and $words[2] -in 'pane', 'tab') { & $zettaThemes $words[2] }"
    ));
    assert!(
        powershell.contains("elseif ($previous -in '--output-type', '-t', '--theme', '--text')")
    );
}

// Regression test: --theme requires --profile, so completing one must keep
// offering the other. Both are root flags handled by the same "$command ==
// -*" branch that also stops the script from falling through to a
// subcommand's (empty) completions once any root flag has been typed.
#[test]
fn profile_and_theme_root_flags_keep_completing_each_other_in_bash() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "{script}\nCOMP_WORDS=(zetta --profile System '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'after-profile:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta --theme Dracula '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'after-theme:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    // printf recycles its format string per remaining argument, so each
    // COMPREPLY entry lands on its own "after-profile:"/"after-theme:" line
    // rather than one space-joined line.
    let after_profile = completions
        .lines()
        .filter_map(|line| line.strip_prefix("after-profile:"))
        .collect::<Vec<_>>();
    let after_theme = completions
        .lines()
        .filter_map(|line| line.strip_prefix("after-theme:"))
        .collect::<Vec<_>>();
    assert!(
        after_profile.contains(&"--theme"),
        "expected --theme after --profile: {after_profile:?}"
    );
    assert!(
        !after_profile.contains(&"--profile"),
        "did not expect --profile repeated after --profile: {after_profile:?}"
    );
    assert!(
        !after_profile.contains(&"benchmark"),
        "did not expect a subcommand after a root flag: {after_profile:?}"
    );
    assert!(
        after_theme.contains(&"--profile"),
        "expected --profile after --theme: {after_theme:?}"
    );
    assert!(
        !after_theme.contains(&"--theme"),
        "did not expect --theme repeated after --theme: {after_theme:?}"
    );
}

#[test]
fn bash_root_split_completion_handles_long_short_and_combined_launch_options() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "zetta() {{\n    if [[ $1 == splits ]]; then\n        printf '%s\\n' custom-layout quarters four-vertical three-left three-right\n    elif [[ $1 == profile && $2 == list ]]; then\n        if [[ $3 == --config && $4 == profiles.json ]]; then\n            printf '%s\\n' 'Configured Shell'\n        else\n            printf '%s\\n' 'System' 'WSL: Ubuntu'\n        fi\n    fi\n}}\n{script}\nCOMP_WORDS=(zetta --split '')\nCOMP_CWORD=2\n_zetta_complete\nprintf 'long:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta -s '')\nCOMP_CWORD=2\n_zetta_complete\nprintf 'short:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta --profile System '')\nCOMP_CWORD=3\n_zetta_complete\nprintf 'profile:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta --split quarters --profile '')\nCOMP_CWORD=4\n_zetta_complete\nprintf 'combined:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta -c profiles.json profile disable '')\nCOMP_CWORD=5\n_zetta_complete\nprintf 'profile-config:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .env_remove("ZETTA_HOST_EXECUTABLE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    for prefix in ["long:", "short:"] {
        for name in [
            "custom-layout",
            "quarters",
            "four-vertical",
            "three-left",
            "three-right",
        ] {
            assert!(
                completions
                    .lines()
                    .any(|line| line == format!("{prefix}{name}")),
                "expected {name:?} after {prefix}: {completions}"
            );
        }
    }
    assert!(completions.lines().any(|line| line == "profile:--split"));
    assert!(completions.lines().any(|line| line == "combined:System"));
    assert!(
        completions
            .lines()
            .any(|line| line == "profile-config:Configured Shell")
    );
}

#[test]
fn bash_pane_completion_offers_directions_and_live_labels() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_command()
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let script = ShellIntegration::Bash.script();
    let driver = format!(
        "zetta() {{
    if [[ $1 == pane && $2 == --list ]]; then
        printf '%s\\n' 'Pane 1' api 'Build runner'
    fi
}}
{script}
COMP_WORDS=(zetta pane '')
COMP_CWORD=2
_zetta_complete
printf 'options:%s\\n' \"${{COMPREPLY[@]}}\"
COMP_WORDS=(zetta pane --direction '')
COMP_CWORD=3
_zetta_complete
printf 'directions:%s\\n' \"${{COMPREPLY[@]}}\"
COMP_WORDS=(zetta pane --pane '')
COMP_CWORD=3
_zetta_complete
printf 'labels:%s\\n' \"${{COMPREPLY[@]}}\"
"
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .env_remove("ZETTA_HOST_EXECUTABLE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash pane completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    for direction in ["left", "right", "up", "down"] {
        assert!(
            completions
                .lines()
                .any(|line| line == format!("directions:{direction}")),
            "expected {direction:?} in pane direction completions: {completions}"
        );
    }
    for label in ["Pane 1", "api", "Build runner"] {
        assert!(
            completions
                .lines()
                .any(|line| line == format!("labels:{label}")),
            "expected {label:?} in pane label completions: {completions}"
        );
    }
    assert!(
        completions
            .lines()
            .any(|line| line == "options:--direction")
    );
    assert!(completions.lines().any(|line| line == "options:--stack"));
}

#[test]
fn serial_completion_enumerates_devices_when_completion_is_requested() {
    let scripts = [
        ShellIntegration::Bash.script(),
        ShellIntegration::Fish.script(),
        ShellIntegration::PowerShell.script(),
        ShellIntegration::Zsh.script(),
    ];

    for script in scripts {
        assert!(script.contains("serial list"));
        assert!(script.contains("tftp") && script.contains("server"));
    }
}

#[test]
fn service_completion_uses_command_local_short_options() {
    let bash = ShellIntegration::Bash.script();
    assert!(bash.contains("--device)\n            _zetta_complete_serial_devices"));
    assert!(bash.contains("--data-bits|-D)"));
    assert!(bash.contains(
        "if [[ $command == serial ]]; then\n                _zetta_compgen 'none odd even'"
    ));
    assert!(bash.contains(
        "if [[ $command == http || ( $command == tftp && ${COMP_WORDS[2]} == server ) ]]; then"
    ));

    let fish = ShellIntegration::Fish.script();
    assert!(fish.contains("-l device"));
    assert!(fish.contains("-l data-bits"));
    assert!(fish.contains("-l parity"));
    assert!(fish.contains("__zetta_tftp_server' -l config"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(powershell.contains("'--device', '-d'"));
    assert!(powershell.contains("'--data-bits', '-D'"));
    assert!(powershell.contains("$previous -eq '-p' -and $subcommand -eq 'serial'"));
    assert!(powershell.contains("{ '--root', '--port', '--config', '--help' }"));

    let zsh = ShellIntegration::Zsh.script();
    assert!(zsh.contains("--data-bits|-D)"));
    assert!(zsh.contains("$words[2] == serial"));
    assert!(zsh.contains("_zetta_options --root --port --config --help"));
}

// Regression test: commit 72afe3b ("Add serial console, and HTTP, TFTP
// servers to CLI integration") offered both short and long option names as
// completion candidates for `serial console`, `http server`, and `tftp`,
// contrary to the "long form only in autocomplete" rule in AGENTS.md. Short
// forms must remain valid on the command line (see cli_services.rs and
// tftp.rs parsing) but must not be offered as completion candidates.
#[test]
fn service_subcommand_completions_only_offer_long_form_flags() {
    let bash = ShellIntegration::Bash.script();
    assert!(!bash.contains("'-d --device"));
    assert!(!bash.contains("'-r --root -p --port -c --config -h --help'"));
    assert!(!bash.contains("'-p --port -h --help'"));
    assert!(
        bash.contains(
            "'--device --baud-rate --data-bits --parity --stop-bits --flow-control --help'"
        )
    );
    assert!(bash.contains("'--root --port --config --help'"));

    let zsh = ShellIntegration::Zsh.script();
    assert!(!zsh.contains("-d --device -b --baud-rate"));
    assert!(!zsh.contains("compadd -- -r --root -p --port -c --config -h --help"));
    assert!(!zsh.contains("compadd -- -p --port -h --help"));
    assert!(zsh.contains(
        "_zetta_options --device --baud-rate --data-bits --parity --stop-bits --flow-control --help"
    ));
    assert!(zsh.contains("_zetta_options --root --port --config --help"));

    let powershell = ShellIntegration::PowerShell.script();
    assert!(!powershell.contains("'-d', '--device', '-b', '--baud-rate'"));
    assert!(
        !powershell.contains("'-r', '--root', '-p', '--port', '-c', '--config', '-h', '--help'")
    );
    assert!(!powershell.contains("'-p', '--port', '-h', '--help'"));
    assert!(powershell.contains(
        "'--device', '--baud-rate', '--data-bits', '--parity', '--stop-bits', '--flow-control', '--help'"
    ));
    assert!(powershell.contains("'--root', '--port', '--config', '--help'"));

    let fish = ShellIntegration::Fish.script();
    assert!(!fish.contains("-s d -l device"));
    assert!(!fish.contains("-s r -l root"));
    assert!(!fish.contains("-s p -l port"));
    assert!(!fish.contains("-s c -l config"));
    assert!(fish.contains("subcommand_from console' -l device"));
    assert!(fish.contains("subcommand_from server' -l root"));

    assert!(!bash.contains("'-a --app-name -i --icon -s --sound -t --timeout --help'"));
    assert!(bash.contains("'--app-name --icon --sound --timeout --help'"));
    assert!(!zsh.contains("compadd -- -a --app-name -i --icon -s --sound -t --timeout --help"));
    assert!(zsh.contains("_zetta_options --app-name --icon --sound --timeout --help"));
    assert!(!powershell.contains("'-a', '--app-name'"));
    assert!(powershell.contains("'--app-name', '--icon', '--sound', '--timeout', '--help'"));
    assert!(!fish.contains("-s a -l app-name"));
    assert!(fish.contains("__zetta_notify_root' -l app-name"));
}

#[test]
fn profile_completion_uses_line_oriented_dynamic_endpoints() {
    let scripts = [
        ShellIntegration::Bash.script(),
        ShellIntegration::Fish.script(),
        ShellIntegration::PowerShell.script(),
        ShellIntegration::Zsh.script(),
    ];
    for script in scripts {
        assert!(!script.contains("ZETTA_PROFILES"));
        assert!(script.contains("profile list"));
        assert!(script.contains("profile themes"));
    }
}

// Regression test: values that contain spaces or quote characters are inserted
// by PowerShell into the command line verbatim, splitting arguments. Completing
// a theme name like "Gruvbox Light Hard" must emit a single quoted argument
// (with embedded single quotes doubled), not the raw name, or `theme pane`
// rejects it with "only one theme may be specified".
#[test]
fn powershell_quotes_spaced_completion_values() {
    let powershell = ShellIntegration::PowerShell.script();

    assert!(powershell.contains(r#"$value -match '\s'"#));
    assert!(powershell.contains(r#""'" + $value.Replace("'", "''")"#));
    assert!(powershell.contains(
        "[System.Management.Automation.CompletionResult]::new($text, $value, 'ParameterValue', $value)"
    ));
}

#[cfg(windows)]
#[test]
fn powershell_accepts_the_generated_integration_syntax() {
    let script = ShellIntegration::PowerShell.script();

    for executable in ["powershell.exe", "pwsh.exe"] {
        let mut child = match Command::new(executable)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$source = [Console]::In.ReadToEnd(); \
                 [scriptblock]::Create($source) | Out-Null",
            ])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error)
                if executable == "pwsh.exe" && error.kind() == std::io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(error) => panic!("failed to launch {executable}: {error}"),
        };
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{executable} rejected the generated integration:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn configuring_zsh_writes_the_startup_command_once() {
    let home = tempfile::tempdir().unwrap();
    let startup_file = home.path().join(".zshrc");

    assert_eq!(
        configure_shell_integration(ShellIntegration::Zsh, home.path()).unwrap(),
        ShellIntegrationConfiguration::Written(startup_file.clone())
    );
    assert_eq!(
        fs::read_to_string(&startup_file).unwrap(),
        "eval \"$(zetta init zsh)\"\n"
    );

    assert_eq!(
        configure_shell_integration(ShellIntegration::Zsh, home.path()).unwrap(),
        ShellIntegrationConfiguration::AlreadyPresent(startup_file.clone())
    );
    assert_eq!(
        fs::read_to_string(startup_file).unwrap(),
        "eval \"$(zetta init zsh)\"\n"
    );
}

#[test]
fn shell_detection_uses_the_shell_name_from_shell_environment_path() {
    assert_eq!(
        ShellIntegration::from_shell_path(Path::new("/usr/bin/zsh")).unwrap(),
        ShellIntegration::Zsh
    );
}

#[cfg(not(windows))]
#[test]
fn shell_detection_prefers_the_active_profile_shell_over_shell_environment() {
    assert_eq!(
        ShellIntegration::current_with_active_shell(
            Some(Path::new("/opt/homebrew/bin/fish")),
            Some(OsStr::new("/bin/bash")),
        )
        .unwrap(),
        ShellIntegration::Fish
    );
}

#[cfg(windows)]
#[test]
fn shell_detection_prefers_an_active_cygwin_shell_over_windows_fallback() {
    for (path, shell) in [
        (r"C:\cygwin64\bin\bash.exe", ShellIntegration::Bash),
        (r"C:\cygwin64\bin\fish.exe", ShellIntegration::Fish),
        (r"C:\cygwin64\bin\zsh.exe", ShellIntegration::Zsh),
    ] {
        assert_eq!(
            ShellIntegration::current_with_active_shell(Some(Path::new(path)), None).unwrap(),
            shell
        );
    }
}

#[cfg(windows)]
#[test]
fn shell_detection_prefers_an_active_powershell_process_over_shell_environment() {
    assert_eq!(
        ShellIntegration::current_with_active_shell(
            Some(Path::new(
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
            )),
            Some(OsStr::new("/bin/bash")),
        )
        .unwrap(),
        ShellIntegration::PowerShell
    );
}

#[test]
fn shell_detection_falls_back_to_shell_environment_without_an_active_shell() {
    assert_eq!(
        ShellIntegration::current_with_active_shell(None, Some(OsStr::new("/bin/bash"))).unwrap(),
        ShellIntegration::Bash
    );
}

#[cfg(windows)]
#[test]
fn missing_shell_defaults_to_powershell_on_windows() {
    assert_eq!(
        ShellIntegration::current_with_active_shell(None, None).unwrap(),
        ShellIntegration::PowerShell
    );
}

#[cfg(windows)]
#[test]
fn shell_detection_accepts_powershell_executables() {
    assert_eq!(
        ShellIntegration::from_shell_path(Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe"))
            .unwrap(),
        ShellIntegration::PowerShell
    );
}

#[cfg(windows)]
#[test]
fn msys_home_is_converted_before_selecting_the_startup_file() {
    let home = resolve_windows_posix_shell_home(
        Some(PathBuf::from("/home/alice")),
        Some(PathBuf::from(r"C:\Users\alice")),
        |home| {
            assert_eq!(home, Path::new("/home/alice"));
            Ok(PathBuf::from(r"D:\tools\msys64\home\alice"))
        },
    )
    .unwrap();

    assert_eq!(home, PathBuf::from(r"D:\tools\msys64\home\alice"));
    assert_eq!(
        ShellIntegration::Zsh.startup_file(&home),
        PathBuf::from(r"D:\tools\msys64\home\alice\.zshrc")
    );
}

#[cfg(windows)]
#[test]
fn native_home_does_not_require_cygpath() {
    let home =
        resolve_windows_posix_shell_home(Some(PathBuf::from(r"D:\homes\alice")), None, |_| {
            panic!("native HOME should not be converted")
        })
        .unwrap();

    assert_eq!(home, PathBuf::from(r"D:\homes\alice"));
}

#[cfg(windows)]
#[test]
fn msys2_link_startup_file_is_resolved_without_creating_a_shadow_file() {
    let temporary = tempfile::tempdir().unwrap();
    let startup_file = temporary.path().join(".zshrc");
    let link = temporary.path().join(".zshrc.lnk");
    let target = temporary.path().join("prezto-zshrc");
    fs::write(&link, "MSYS2 shortcut placeholder").unwrap();
    fs::write(&target, "# existing configuration\n").unwrap();

    let resolved = resolve_msys2_link_startup_file(&startup_file, |candidate| {
        assert_eq!(candidate, link);
        Ok(target.clone())
    })
    .unwrap();
    configure_shell_integration_file(ShellIntegration::Zsh, &resolved).unwrap();

    assert!(!startup_file.exists());
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "# existing configuration\neval \"$(zetta init zsh)\"\n"
    );
}

#[cfg(windows)]
#[test]
fn cygwin_startup_file_links_are_resolved_before_native_file_access() {
    let startup_file = Path::new(r"C:\cygwin64\home\alice\.zshrc");
    let target = PathBuf::from(r"C:\cygwin64\home\alice\.zprezto\runcoms\zshrc");

    let resolved = resolve_cygwin_link_startup_file_with(
        startup_file,
        |_| false,
        |_| true,
        |_| Ok(Some(target.clone())),
    )
    .unwrap();

    assert_eq!(resolved, target);
}

#[cfg(windows)]
#[test]
fn cygwin_tools_use_the_installation_root_containing_the_startup_file() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("cygwin1.dll"), []).unwrap();
    fs::write(bin.join("cygpath.exe"), []).unwrap();
    fs::write(bin.join("readlink.exe"), []).unwrap();
    let startup_file = root.path().join("home").join("alice").join(".zshrc");

    assert_eq!(
        cygwin_root_for_path(&startup_file),
        Some(root.path().to_path_buf())
    );
    assert_eq!(
        cygwin_tool_for_path(&startup_file, "cygpath.exe"),
        bin.join("cygpath.exe")
    );
    assert_eq!(
        cygwin_tool_for_path(&startup_file, "readlink.exe"),
        bin.join("readlink.exe")
    );
}

#[cfg(windows)]
#[test]
fn powershell_profile_query_uses_the_requested_shell_edition() {
    let profile = query_powershell_profile(Path::new("powershell.exe")).unwrap();

    assert_eq!(
        profile.file_name().and_then(|name| name.to_str()),
        Some("Microsoft.PowerShell_profile.ps1")
    );
    assert_eq!(
        profile
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str()),
        Some("WindowsPowerShell")
    );
}

#[test]
fn configuring_powershell_writes_the_resolved_profile() {
    let home = tempfile::tempdir().unwrap();
    let profile = home
        .path()
        .join("Redirected Documents")
        .join("WindowsPowerShell")
        .join("Microsoft.PowerShell_profile.ps1");

    assert_eq!(
        configure_shell_integration_file(ShellIntegration::PowerShell, &profile).unwrap(),
        ShellIntegrationConfiguration::Written(profile.clone())
    );
    assert_eq!(
        fs::read_to_string(profile).unwrap(),
        "zetta init powershell | Out-String | Invoke-Expression\n"
    );
}

#[test]
fn configuring_powershell_migrates_the_broken_pipeline() {
    let home = tempfile::tempdir().unwrap();
    let profile = home.path().join("Microsoft.PowerShell_profile.ps1");
    fs::write(
        &profile,
        "# Keep this comment unchanged.\r\nzetta init powershell | Invoke-Expression\r\n",
    )
    .unwrap();

    assert_eq!(
        configure_shell_integration_file(ShellIntegration::PowerShell, &profile).unwrap(),
        ShellIntegrationConfiguration::Written(profile.clone())
    );
    assert_eq!(
        fs::read_to_string(profile).unwrap(),
        "# Keep this comment unchanged.\r\nzetta init powershell | Out-String | Invoke-Expression\r\n"
    );
}

#[test]
fn commented_integration_does_not_prevent_configuration() {
    let home = tempfile::tempdir().unwrap();
    let startup_file = home.path().join(".zshrc");
    fs::write(&startup_file, "# eval \"$(zetta init zsh)\"\n").unwrap();

    assert_eq!(
        configure_shell_integration(ShellIntegration::Zsh, home.path()).unwrap(),
        ShellIntegrationConfiguration::Written(startup_file.clone())
    );
    assert_eq!(
        fs::read_to_string(startup_file).unwrap(),
        "# eval \"$(zetta init zsh)\"\neval \"$(zetta init zsh)\"\n"
    );
}

#[test]
fn configuring_fish_creates_its_startup_directory_and_preserves_existing_content() {
    let home = tempfile::tempdir().unwrap();
    let startup_file = home.path().join(".config/fish/config.fish");
    fs::create_dir_all(startup_file.parent().unwrap()).unwrap();
    fs::write(&startup_file, "set -gx EDITOR vim").unwrap();

    assert_eq!(
        configure_shell_integration(ShellIntegration::Fish, home.path()).unwrap(),
        ShellIntegrationConfiguration::Written(startup_file.clone())
    );
    assert_eq!(
        fs::read_to_string(startup_file).unwrap(),
        "set -gx EDITOR vim\nzetta init fish | source\n"
    );
}

#[test]
fn generated_shell_syntax_uses_the_native_powershell_completer_signature() {
    assert!(
        ShellIntegration::PowerShell
            .script()
            .contains("param($wordToComplete, $commandAst, $cursorPosition)")
    );
    assert!(ShellIntegration::Zsh.script().contains("terminal-size)"));
    assert!(
        ShellIntegration::Zsh
            .script()
            .contains("compadd -S ' ' -- benchmark")
    );
}

#[test]
fn terminal_size_completions_include_pane_resize_options() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        match shell {
            ShellIntegration::Fish => {
                assert!(script.contains("-l resize"));
                assert!(script.contains("-l columns"));
                assert!(script.contains("-l rows"));
                assert!(!script.contains("-s r -l resize"));
                assert!(!script.contains("-s c -l columns"));
                assert!(!script.contains("-s R -l rows"));
            }
            _ => {
                assert!(script.contains("--resize"));
                assert!(script.contains("--columns"));
                assert!(script.contains("--rows"));
                assert!(!script.contains("--resize -r"));
            }
        }
    }
}

#[test]
fn edit_completions_offer_managed_cleanup_by_its_long_name() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(script.contains("--delete-after"));
        assert!(!script.contains("-d --delete-after"));
    }
}

#[test]
fn generated_scripts_include_root_flags_and_configured_profiles() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(script.contains("profile"));
        assert!(script.contains("config"));
        assert!(!script.contains("WSL: Ubuntu"));
        assert!(script.contains("zetta profile list"));
        assert!(script.contains("zetta profile themes"));
        assert!(script.contains("profile-report"));
        assert!(script.contains("split"));
        assert!(script.contains("zetta splits"));
        assert!(script.contains("project"));
        assert!(script.contains("zetta project list"));
        #[cfg(feature = "worktree")]
        assert!(script.contains("--path"));
        assert!(!script.contains("quarters four-vertical three-left three-right"));
    }
}

#[test]
fn generated_scripts_offer_the_shared_overlay_colour_catalogue() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        assert!(!script.contains("ZETTA_OVERLAY_COLORS"));
        for preset in OVERLAY_COLOR_PRESETS {
            assert!(
                script.contains(preset.name),
                "{shell:?} script omitted {}",
                preset.name
            );
        }
    }
}

#[cfg(feature = "worktree")]
#[test]
fn generated_scripts_only_offer_long_form_flags() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script();
        match shell {
            ShellIntegration::Bash => {
                assert!(script.contains(
                    "terminal-size mux pane profile project cmd edit vi init serial http tftp notify attention copy paste splits tabicon theme overlay wt --help --version --config --keymap --profile --split --replace-pane --theme --no-mux --new-window --command'"
                ));
                assert!(script.contains("auto zetta bash zsh fish"));
                assert!(script.contains("_zetta_complete_project_commands"));
                assert!(
                    script
                        .contains("_zetta_complete_project_command_options '--help' '--list' '--'")
                );
            }
            ShellIntegration::Fish => {
                assert!(script.contains("-l profile -r"));
                assert!(script.contains("-l replace-pane"));
                assert!(!script.contains("-s p -l profile"));
                assert!(script.contains(
                    "-s c -r -n '__zetta_has_profile_subcommand; and __zetta_short_option -c'"
                ));
                assert!(script.contains("auto zetta bash zsh fish"));
                assert!(script.contains("__zetta_project_commands"));
            }
            ShellIntegration::PowerShell => {
                assert!(script.contains(
                    "'--help', '--version', '--config', '--keymap', '--profile', '--split', '--replace-pane', '--theme', '--no-mux', '--new-window', '--command'"
                ));
                assert!(script.contains("'overlay', 'wt', '--help'"));
                assert!(script.contains("'auto', 'zetta', 'bash', 'zsh', 'fish'"));
                assert!(script.contains("$zettaProjectCommands"));
                assert!(script.contains("@(& $zettaProjectCommands) + '--help', '--list', '--'"));
            }
            ShellIntegration::Zsh => {
                assert!(
                    script.contains("_zetta_options --help --version --config --keymap --profile")
                );
                assert!(script.contains("auto zetta bash zsh fish"));
                assert!(script.contains("_zetta_project_commands"));
                assert!(script.contains("_zetta_options --help --list --"));
            }
        }
    }
}

#[test]
fn fish_script_emits_long_option_candidates_for_every_command_context() {
    let script = ShellIntegration::Fish.script();

    for context in [
        "root",
        "init",
        "cmd",
        "serial",
        "http",
        "terminal-size",
        "mux",
        "benchmark_output",
        "benchmark",
        "serial-console",
        "http-server",
        "tftp",
        "tftp-client",
        "tftp-server",
        "notify",
        "notify_cleanup",
        "attention",
        "copy",
        "paste",
        "tabicon",
        "theme_pane",
        "theme_tab",
        "splits",
        "pane",
        "pane_wait",
        "overlay",
        "ztftp",
        "zntfy",
        "zcopy",
        "zpaste",
        "pbcopy",
        "pbpaste",
    ] {
        assert!(
            script.contains(&format!("(__zetta_long_options {context})")),
            "missing Fish long-option candidates for {context}"
        );
    }
}

#[test]
fn fish_displays_long_option_candidates_and_supports_short_option_values() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let script = ShellIntegration::Fish.script();
    let script_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(script_file.path(), script).unwrap();
    let overlay_color_names = OVERLAY_COLOR_PRESETS
        .iter()
        .map(|preset| preset.name)
        .collect::<Vec<_>>();
    for (line, expected) in [
        (
            "zetta ",
            &[
                "--help",
                "--version",
                "--config",
                "--keymap",
                "--profile",
                "--split",
                "--replace-pane",
                "--theme",
                "--no-mux",
                "--new-window",
            ][..],
        ),
        (
            "zetta --split ",
            &[
                "custom-layout",
                "quarters",
                "four-vertical",
                "three-left",
                "three-right",
            ][..],
        ),
        (
            "zetta -s ",
            &[
                "custom-layout",
                "quarters",
                "four-vertical",
                "three-left",
                "three-right",
            ][..],
        ),
        ("zetta --profile System ", &["--split"][..]),
        ("zetta --split quarters --profile ", &["System"][..]),
        (
            "zetta profile ",
            &[
                "list",
                "themes",
                "disable",
                "enable",
                "theme",
                "dark-theme",
                "icon",
                "default",
                "add",
                "remove",
            ][..],
        ),
        ("zetta profile disable ", &["System", "WSL: Ubuntu"][..]),
        ("zetta profile theme ", &["System", "WSL: Ubuntu"][..]),
        ("zetta profile theme System ", &["Gruvbox Light Hard"][..]),
        (
            "zetta cmd ",
            &["build", "check", "--list", "--help", "--"][..],
        ),
        ("zetta cmd b", &["build"][..]),
        ("zetta cmd build ", &["--help", "--"][..]),
        ("zetta cmd build -- ", &[][..]),
        ("zetta cmd --list ", &[][..]),
        (
            "zetta -c profiles.json profile disable ",
            &["Configured Shell"][..],
        ),
        (
            "zetta benchmark ",
            &[
                "--profile-report",
                "--profile-duration",
                "--profile-pane-stress",
                "--profile-background-stress",
                "--profile-sparse-updates",
                "--profile-external-terminal",
                "--help",
            ][..],
        ),
        (
            "zetta benchmark output ",
            &["--size", "--output-type", "--help"][..],
        ),
        (
            "zetta terminal-size ",
            &["--json", "--resize", "--columns", "--rows", "--help"][..],
        ),
        (
            "zetta mux ",
            &[
                "list",
                "stop",
                "reconnect",
                "resume",
                "share",
                "unshare",
                "kill",
                "forget",
                "--json",
                "--upgrade",
                "--help",
                "--version",
            ][..],
        ),
        (
            "zmux ",
            &[
                "list",
                "stop",
                "reconnect",
                "resume",
                "share",
                "unshare",
                "kill",
                "forget",
                "--json",
                "--upgrade",
                "--help",
                "--version",
            ][..],
        ),
        ("zetta splits ", &["--help"][..]),
        (
            "zetta pane ",
            &[
                "wait",
                "--direction",
                "--label",
                "--pane",
                "--overlay",
                "--overlay-size",
                "--overlay-opacity",
                "--overlay-color",
                "--stack",
                "--list",
                "--help",
            ][..],
        ),
        (
            "zetta pane --direction ",
            &["left", "right", "up", "down"][..],
        ),
        ("zetta pane -d ", &["left", "right", "up", "down"][..]),
        ("zetta pane --overlay ", &[][..]),
        (
            "zetta pane --overlay-size ",
            &["sm", "base", "lg", "xl", "2xl", "3xl"][..],
        ),
        ("zetta pane --overlay-opacity ", &[][..]),
        (
            "zetta pane --overlay-color ",
            overlay_color_names.as_slice(),
        ),
        (
            "zetta pane -S ",
            &["sm", "base", "lg", "xl", "2xl", "3xl"][..],
        ),
        ("zetta pane -O ", &[][..]),
        ("zetta pane -c ", overlay_color_names.as_slice()),
        ("zetta tabicon ", &["--icon", "--list", "--help"][..]),
        ("zetta tabicon -i ", &[][..]),
        ("zetta theme ", &["pane", "tab"][..]),
        (
            "zetta theme pane ",
            &[
                "--theme",
                "--reset",
                "--list",
                "--help",
                "Gruvbox Light Hard",
            ][..],
        ),
        ("zetta theme pane -t ", &["Gruvbox Light Hard"][..]),
        (
            "zetta theme tab ",
            &[
                "--theme",
                "--reset",
                "--list",
                "--help",
                "Gruvbox Light Hard",
            ][..],
        ),
        ("zetta theme tab -t ", &["Gruvbox Light Hard"][..]),
        (
            "zetta overlay ",
            &[
                "--text",
                "--size",
                "--opacity",
                "--color",
                "--reset",
                "--help",
            ][..],
        ),
        ("zetta overlay -t ", &[][..]),
        (
            "zetta overlay -s ",
            &["sm", "base", "lg", "xl", "2xl", "3xl"][..],
        ),
        ("zetta overlay -o ", &[][..]),
        ("zetta overlay --color ", overlay_color_names.as_slice()),
        ("zetta overlay -c ", overlay_color_names.as_slice()),
        ("zetta vi ", &["--help", "Cargo.toml"][..]),
        ("zetta vi Carg", &["Cargo.toml"][..]),
        ("zetta init ", &["--help"][..]),
        ("zetta init fish ", &["--help"][..]),
        ("zetta serial ", &["--help"][..]),
        ("zetta serial list ", &["--help"][..]),
        (
            "zetta serial console ",
            &[
                "--device",
                "--baud-rate",
                "--data-bits",
                "--parity",
                "--stop-bits",
                "--flow-control",
                "--help",
            ][..],
        ),
        ("zetta http ", &["--help"][..]),
        (
            "zetta http server ",
            &["--root", "--port", "--config", "--help"][..],
        ),
        ("zetta tftp ", &["--help"][..]),
        ("zetta tftp get ", &["--port", "--help"][..]),
        (
            "zetta tftp server ",
            &["--root", "--port", "--config", "--help"][..],
        ),
        ("zetta serial console -p ", &["none", "odd", "even"][..]),
        (
            "zetta notify ",
            &["--app-name", "--icon", "--sound", "--timeout", "--help"][..],
        ),
        ("zetta notify cleanup ", &["--dry-run", "--help"][..]),
        ("zetta notify -s ", &["zetta-default"][..]),
        (
            "zetta attention ",
            &[
                "--notify",
                "--app-name",
                "--icon",
                "--sound",
                "--timeout",
                "--help",
            ][..],
        ),
        ("zetta attention -s ", &["zetta-default"][..]),
        ("zetta copy ", &["--pboard", "--help"][..]),
        ("zetta paste ", &["--pboard", "--prefer", "--help"][..]),
        (
            "zetta copy -pboard ",
            &["general", "ruler", "find", "font"][..],
        ),
        ("ztftp ", &["--port", "--help"][..]),
        (
            "zntfy ",
            &["--app-name", "--icon", "--sound", "--timeout", "--help"][..],
        ),
        ("zntfy -s ", &["zetta-default"][..]),
        ("zcopy ", &["--pboard", "--help"][..]),
        ("zpaste ", &["--pboard", "--prefer", "--help"][..]),
    ] {
        let output = clean_shell_command("fish")
            .args([
                "--no-config",
                "-c",
                "function zetta; if test \"$argv[1]\" = cmd; and test \"$argv[2]\" = --list; printf '%s\\n' build check; else if test \"$argv[1]\" = splits; printf '%s\\n' custom-layout quarters four-vertical three-left three-right; else if test \"$argv[1]\" = profile; and test \"$argv[2]\" = list; if test \"$argv[3]\" = --config; and test \"$argv[4]\" = profiles.json; printf '%s\\n' 'Configured Shell'; else; printf '%s\\n' System 'WSL: Ubuntu'; end; else if test \"$argv[1]\" = profile; and test \"$argv[2]\" = themes; printf '%s\\n' 'Gruvbox Light Hard'; else if test \"$argv[1]\" = theme; and test \"$argv[3]\" = --list; printf '%s\\n' 'Gruvbox Light Hard'; end; end; source $argv[1]; complete -C \"$argv[2]\"",
                "--",
                script_file.path().to_str().unwrap(),
                line,
            ])
            .env_remove("ZETTA_HOST_EXECUTABLE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Fish rejected generated completion for {line:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let completions = String::from_utf8_lossy(&output.stdout);
        let candidates = completions
            .lines()
            .map(|completion| {
                completion
                    .split_once('\t')
                    .map_or(completion, |(name, _)| name)
            })
            .collect::<Vec<_>>();
        for expected in expected {
            assert!(
                candidates.contains(expected),
                "expected {expected:?} in Fish completions for {line:?}: {completions}"
            );
        }
        assert!(
            !candidates
                .iter()
                .any(|candidate| candidate.starts_with('-') && !candidate.starts_with("--")),
            "did not expect short-form options in Fish completions for {line:?}: {completions}"
        );
    }
}

#[test]
fn fish_omits_daemon_only_mux_candidates_in_no_mux_shells() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let script = ShellIntegration::Fish.script();
    let script_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(script_file.path(), script).unwrap();
    for line in ["zetta mux ", "zmux "] {
        let output = clean_shell_command("fish")
            .args([
                "--no-config",
                "-c",
                "function zetta; end; source $argv[1]; complete -C \"$argv[2]\"",
                "--",
                script_file.path().to_str().unwrap(),
                line,
            ])
            .env_remove("ZETTA_HOST_EXECUTABLE")
            .env("ZETTA_NO_MUX", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Fish rejected no-mux completion for {line:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let completions = String::from_utf8_lossy(&output.stdout);
        let candidates = completions
            .lines()
            .map(|completion| {
                completion
                    .split_once('\t')
                    .map_or(completion, |(name, _)| name)
            })
            .collect::<Vec<_>>();
        for expected in ["list", "reconnect", "--json", "--help", "--version"] {
            assert!(
                candidates.contains(&expected),
                "expected {expected:?} in Fish no-mux completions for {line:?}: {candidates:?}"
            );
        }
        for unavailable in [
            "stop",
            "share",
            "unshare",
            "kill",
            "forget",
            "--force",
            "--upgrade",
        ] {
            assert!(
                !candidates.contains(&unavailable),
                "no-mux Fish completion offered {unavailable:?} for {line:?}: {candidates:?}"
            );
        }
    }
}

// Regression test: --theme requires --profile, but each is a plain root
// flag that fish's builtin __fish_use_subcommand cannot tell apart from a
// subcommand once its value has been typed (it treats any non-flag word as
// proof a subcommand was given). Without __zetta_use_subcommand accounting
// for consumed flag values, --profile NAME would stop completing --theme
// and vice versa, and typing either would also incorrectly keep offering
// subcommand names, which are only valid as the very first argument.
#[test]
fn profile_and_theme_root_flags_keep_completing_each_other() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let script = ShellIntegration::Fish.script();
    let script_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(script_file.path(), script).unwrap();

    for (line, expected, unexpected) in [
        (
            "zetta --profile System ",
            &["--theme"][..],
            &["--profile", "benchmark"][..],
        ),
        (
            "zetta --theme Dracula ",
            &["--profile"][..],
            &["--theme", "benchmark"][..],
        ),
    ] {
        let output = clean_shell_command("fish")
            .args([
                "--no-config",
                "-c",
                "source $argv[1]; complete -C \"$argv[2]\"",
                "--",
                script_file.path().to_str().unwrap(),
                line,
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let completions = String::from_utf8_lossy(&output.stdout);
        let candidates = completions
            .lines()
            .map(|completion| {
                completion
                    .split_once('\t')
                    .map_or(completion, |(name, _)| name)
            })
            .collect::<Vec<_>>();
        for name in expected {
            assert!(
                candidates.contains(name),
                "expected {name:?} in Fish completions for {line:?}: {completions}"
            );
        }
        for name in unexpected {
            assert!(
                !candidates.contains(name),
                "did not expect {name:?} in Fish completions for {line:?}: {completions}"
            );
        }
    }
}

#[test]
fn fish_does_not_repeat_options_and_completes_vi_files() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let script = ShellIntegration::Fish.script();
    let script_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(script_file.path(), script).unwrap();

    for line in ["zetta vi --help ", "zetta copy --help ", "zcopy --help "] {
        let output = clean_shell_command("fish")
            .args([
                "--no-config",
                "-c",
                "source $argv[1]; complete -C \"$argv[2]\"",
                "--",
                script_file.path().to_str().unwrap(),
                line,
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let completions = String::from_utf8_lossy(&output.stdout);
        assert!(
            !completions.lines().any(|line| line.starts_with("--help\t")),
            "repeated --help completion for {line:?}: {completions}"
        );
    }

    let output = clean_shell_command("fish")
        .args([
            "--no-config",
            "-c",
            "source $argv[1]; complete -C \"$argv[2]\"",
            "--",
            script_file.path().to_str().unwrap(),
            "zetta vi --",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let completions = String::from_utf8_lossy(&output.stdout);
    assert!(
        completions.lines().any(|line| line.starts_with("--help\t")),
        "missing --help completion for zetta vi --: {completions}"
    );

    let output = clean_shell_command("fish")
        .args([
            "--no-config",
            "-c",
            "source $argv[1]; complete -C \"$argv[2]\"",
            "--",
            script_file.path().to_str().unwrap(),
            "zetta vi Carg",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.starts_with("Cargo.toml"))
    );
}

#[test]
fn tftp_completion_uses_only_the_upload_local_file_argument_position() {
    assert!(
        ShellIntegration::Bash
            .script()
            .contains("(( positional == 1 )) && COMPREPLY=( $(compgen -f")
    );
    assert!(
        ShellIntegration::Zsh
            .script()
            .contains("(( position == 1 )) && _files")
    );
    assert!(!ShellIntegration::Bash.script().contains("positional >= 2"));
    assert!(!ShellIntegration::Zsh.script().contains("position >= 2"));
}

#[test]
fn shell_names_are_case_insensitive_and_pwsh_is_supported() {
    assert_eq!(
        ShellIntegration::parse("BASH").unwrap(),
        ShellIntegration::Bash
    );
    assert_eq!(
        ShellIntegration::parse("pwsh").unwrap(),
        ShellIntegration::PowerShell
    );
    assert!(ShellIntegration::parse("sh").is_err());
}
