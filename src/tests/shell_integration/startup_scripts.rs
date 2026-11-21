//! What the startup scripts do in a live shell: lifecycle and directory
//! markers, history filtering, the vi wrapper, and routing `zetta` to the
//! process that should answer it.

use super::*;

#[test]
fn shell_integration_reports_the_shell_cwd_while_children_run() {
    assert!(
        ShellIntegration::Bash
            .eager_script()
            .contains("__zetta_report_cwd")
    );
    assert!(
        ShellIntegration::Fish
            .eager_script()
            .contains("--on-event fish_prompt")
    );
    assert!(
        ShellIntegration::PowerShell
            .eager_script()
            .contains("zetta-cwd:$zettaDirectory")
    );
    assert!(
        ShellIntegration::Zsh
            .eager_script()
            .contains("add-zsh-hook precmd __zetta_report_cwd")
    );
}

#[test]
fn zsh_lifecycle_tracker_does_not_assign_to_read_only_status() {
    let zsh = ShellIntegration::Zsh.eager_script();

    assert!(zsh.contains(&format!(
        "__ZETTA_LIFECYCLE_TRACKING_VERSION={}",
        zetta_profiles::ZSH_LIFECYCLE_TRACKING_VERSION
    )));
    assert!(!zsh.contains("__ZETTA_LIFECYCLE_VERSION__"));
    assert!(zsh.contains("local zetta_status=$?"));
    assert!(!zsh.contains("local status=$?"));
    assert!(zsh.contains("__ZETTA_LIFECYCLE_TRACKING_VERSION:-0} != 1"));
}

#[test]
fn zsh_integration_filters_zetta_startup_history() {
    let zsh = ShellIntegration::Zsh.eager_script();

    assert!(zsh.contains("function __zetta_filter_startup_history()"));
    assert!(zsh.contains("__zed_init_command_history_"));
    assert!(zsh.contains("fc -p"));
    assert!(zsh.contains("add-zsh-hook zshaddhistory __zetta_filter_startup_history"));
}

#[test]
fn fish_integration_filters_zetta_startup_history() {
    let fish = ShellIntegration::Fish.eager_script();

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

    let script = ShellIntegration::Zsh.eager_script();
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
        .env(
            "__ZETTA_COMMAND_MARKER_NONCE",
            "0123456789abcdef0123456789abcdef",
        )
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
        stdout.contains("zetta-event;0123456789abcdef0123456789abcdef:command-finished:1"),
        "{stdout:?}"
    );
}

const MARKER_NONCE: &str = "00112233445566778899aabbccddeeff";

/// Runs `driver` — the integration followed by a few commands — under `shell`
/// in a pane-like environment that holds [`MARKER_NONCE`], and returns stdout.
/// `None` when the shell is not installed.
fn run_integration_with_nonce(mut command: std::process::Command, driver: &str) -> Option<String> {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = match command
        .env("ZETTA_PANE_ID", "7")
        .env("__ZETTA_COMMAND_MARKER_NONCE", MARKER_NONCE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => panic!("failed to launch the shell: {error}"),
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
        "the integration failed: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 3.17: what a POSIX shell integration prints — lifecycle and directory
/// markers alike — carries the nonce Zetta gave the shell, and nothing the
/// shell runs inherits that nonce — so neither a child's output nor a
/// recording of a nested shell can carry a valid one.
fn assert_markers_carry_the_nonce_and_children_do_not(shell: &str, stdout: &str) {
    assert!(
        stdout.contains(&format!("zetta-event;{MARKER_NONCE}:tracking-ready")),
        "{shell}: {stdout:?}"
    );
    assert!(
        stdout.contains(&format!("zetta-event;{MARKER_NONCE}:command-finished:1")),
        "{shell}: {stdout:?}"
    );
    assert_directory_markers_carry_the_nonce(shell, stdout);
    assert!(!stdout.contains("zetta-event:"), "{shell}: {stdout:?}");
    assert!(stdout.contains("child:unset"), "{shell}: {stdout:?}");
}

/// A shell's directory report decides where splits start, what a file server
/// serves and which project a pane enters, so it carries the nonce too.
fn assert_directory_markers_carry_the_nonce(shell: &str, stdout: &str) {
    assert!(
        stdout.contains(&format!("zetta-cwd;{MARKER_NONCE}:")),
        "{shell}: {stdout:?}"
    );
    assert!(!stdout.contains("zetta-cwd:"), "{shell}: {stdout:?}");
}

const CHILD_ENVIRONMENT_PROBE: &str =
    r#"sh -c 'echo "child:${__ZETTA_COMMAND_MARKER_NONCE-unset}"'"#;

#[test]
fn bash_lifecycle_markers_carry_the_pane_nonce_and_children_do_not_inherit_it() {
    let _guard = lock_bash_tests();
    if !bash_available() || cfg!(windows) {
        return;
    }
    let mut command = bash_command();
    command.args(["--noprofile", "--norc"]);
    let driver = format!(
        "{script}\n__ZETTA_COMMAND_STARTED=1\nfalse\n__zetta_report_cwd\n{CHILD_ENVIRONMENT_PROBE}\n",
        script = ShellIntegration::Bash.eager_script(),
    );
    let stdout = run_integration_with_nonce(command, &driver).expect("bash is available");
    assert_markers_carry_the_nonce_and_children_do_not("bash", &stdout);
}

#[test]
fn zsh_lifecycle_markers_carry_the_pane_nonce_and_children_do_not_inherit_it() {
    if cfg!(windows) {
        return;
    }
    let mut command = clean_shell_command("zsh");
    command.arg("-f");
    let driver = format!(
        "{script}\n__ZETTA_COMMAND_STARTED=1\nfalse\n__zetta_report_cwd\n{CHILD_ENVIRONMENT_PROBE}\nexit 0\n",
        script = ShellIntegration::Zsh.eager_script(),
    );
    let Some(stdout) = run_integration_with_nonce(command, &driver) else {
        return;
    };
    assert_markers_carry_the_nonce_and_children_do_not("zsh", &stdout);
}

#[test]
fn zsh_marks_its_prompt_last_and_keeps_the_status_for_later_hooks() {
    if cfg!(windows) {
        return;
    }
    let mut command = clean_shell_command("zsh");
    command.arg("-f");
    let script = ShellIntegration::Zsh.eager_script();
    // Loaded twice, as a reload does: the hooks must not be registered twice.
    let driver = format!(
        "{script}\n{script}\nTTY=/dev/stdout\nfalse\n__zetta_mark_prompt\nprint -rn -- \"status:$?;\"\n\
         print -rn -- \"precmd:${{(j:,:)precmd_functions}};preexec:${{(j:,:)preexec_functions}};\"\n\
         exit 0\n",
    );
    let Some(stdout) = run_integration_with_nonce(command, &driver) else {
        return;
    };
    let hooks = |name: &str| {
        let start = stdout.find(&format!("{name}:")).expect(name) + name.len() + 1;
        stdout[start..]
            .split(';')
            .next()
            .unwrap()
            .split(',')
            .collect::<Vec<_>>()
    };

    // The marks go to `$TTY`, which a shell without a terminal leaves empty.
    assert!(stdout.contains("\x1b]133;A\x1b\\"), "{stdout:?}");
    assert!(stdout.contains("\x1b]133;C\x1b\\"), "{stdout:?}");
    assert!(stdout.contains("status:1;"), "{stdout:?}");
    let precmd = hooks("precmd");
    assert_eq!(precmd.last(), Some(&"__zetta_mark_prompt"), "{precmd:?}");
    assert_eq!(
        precmd
            .iter()
            .filter(|hook| **hook == "__zetta_mark_prompt")
            .count(),
        1
    );
    let preexec = hooks("preexec");
    assert_eq!(
        preexec
            .iter()
            .filter(|hook| **hook == "__zetta_mark_command")
            .count(),
        1
    );
}

#[test]
fn fish_lifecycle_markers_carry_the_pane_nonce_and_children_do_not_inherit_it() {
    if cfg!(windows) {
        return;
    }
    let mut command = clean_shell_command("fish");
    command.arg("--no-config");
    let driver = format!(
        "{script}\nset -g __ZETTA_COMMAND_STARTED 1\nfalse\n__zetta_report_cwd\n{CHILD_ENVIRONMENT_PROBE}\n",
        script = ShellIntegration::Fish.eager_script(),
    );
    let Some(stdout) = run_integration_with_nonce(command, &driver) else {
        return;
    };
    assert_markers_carry_the_nonce_and_children_do_not("fish", &stdout);
}

#[test]
fn powershell_lifecycle_markers_carry_the_pane_nonce_and_leave_the_environment() {
    let mut command = clean_shell_command(if cfg!(windows) { "pwsh.exe" } else { "pwsh" });
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$source = [Console]::In.ReadToEnd(); . ([scriptblock]::Create($source))",
    ]);
    let driver = format!(
        "{script}\n$null = prompt\nWrite-Output \"child:$(if (Test-Path Env:__ZETTA_COMMAND_MARKER_NONCE) {{ 'set' }} else {{ 'unset' }})\"\n",
        script = ShellIntegration::PowerShell.eager_script(),
    );
    let Some(stdout) = run_integration_with_nonce(command, &driver) else {
        return;
    };
    assert!(
        stdout.contains(&format!("zetta-event;{MARKER_NONCE}:tracking-ready")),
        "{stdout:?}"
    );
    assert_directory_markers_carry_the_nonce("pwsh", &stdout);
    assert!(!stdout.contains("zetta-event:"), "{stdout:?}");
    assert!(stdout.contains("child:unset"), "{stdout:?}");
}

#[test]
fn powershell_cwd_tracker_uses_the_shared_idempotence_guard() {
    let script = ShellIntegration::PowerShell.eager_script();

    assert!(script.contains("Get-Variable -Name __ZettaCwdTrackerInstalled -Scope Global"));
    assert!(script.contains("$global:__ZettaCwdTrackerInstalled = $true"));
    assert!(script.contains("$global:__ZettaOriginalPrompt = $function:prompt"));
    assert!(script.contains("& $global:__ZettaOriginalPrompt"));
    assert!(!script.contains("__ZettaShellIntegrationOriginalPrompt"));
}

#[test]
fn powershell_cwd_trackers_reset_before_the_saved_prompt() {
    const TERMINAL_TRACKER: &str = zetta_profiles::POWERSHELL_CWD_TRACKER;
    // The tokened marker when the shell holds its nonce, else the untokened
    // one: either way, one write the reset must follow at once.
    const MARKER_WRITE: &str = r#"if ($global:__ZettaMarkerNonce) {
                [Console]::Write("$([char]27)]2;zetta-cwd;$($global:__ZettaMarkerNonce):$zettaDirectory$([char]27)\")
            } else {
                [Console]::Write("$([char]27)]2;zetta-cwd:$zettaDirectory$([char]27)\")
            }"#;
    const RESET_WRITE: &str = r#"[Console]::Write("$([char]27)[0m")"#;

    for (name, script) in [
        (
            "shell integration",
            ShellIntegration::PowerShell.eager_script(),
        ),
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
            "{name} must have one CWD marker block"
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
    let bash = ShellIntegration::Bash.eager_script();
    assert!(bash.contains("if ! type -t vi >/dev/null 2>&1"));
    assert!(bash.contains("eval 'vi() { zetta vi \"$@\"; }'"));
    assert!(bash.contains("zvi() { zetta vi \"$@\"; }"));
    assert!(bash.contains("ZETTA_HOST_EXECUTABLE"));
    assert!(bash.contains("complete -F _zetta_entry_zvi zvi"));
    assert!(bash.contains("complete -F _zetta_entry_zvi vi"));

    let fish = ShellIntegration::Fish.eager_script();
    assert!(fish.contains("if not type -q vi"));
    assert!(fish.contains("function vi --wraps 'zetta vi'"));
    assert!(fish.contains("function zvi --wraps 'zetta vi'"));
    // The tree completes files where `vi` takes them; forcing them in would
    // offer them everywhere else too.
    assert!(!fish.contains("complete -c vi -F"));
    assert!(!fish.contains("complete -c zvi -F"));
    assert!(fish.contains("complete -c zvi -f -a '(__zetta_tree_complete zvi)'"));
    assert!(fish.contains("ZETTA_HOST_EXECUTABLE"));

    let powershell = ShellIntegration::PowerShell.eager_script();
    assert!(powershell.contains("$zettaViMissing = -not (__zetta_command_available 'vi')"));
    assert!(powershell.contains("if ($zettaViMissing)"));
    assert!(powershell.contains("function zvi { & zetta vi @args }"));
    assert!(powershell.contains("Register-ArgumentCompleter -CommandName zvi"));
    assert!(powershell.contains("Get-ChildItem -Path $pattern"));
    assert!(powershell.contains("$zettaTreeRoots['vi'] = 'zvi'"));

    let zsh = ShellIntegration::Zsh.eager_script();
    assert!(zsh.contains("$+commands[vi]"));
    assert!(zsh.contains("compdef _zetta_entry_zvi vi"));
    assert!(zsh.contains("function zvi { zetta vi \"$@\"; }"));
    assert!(zsh.contains("ZETTA_HOST_EXECUTABLE"));
    #[cfg(feature = "worktree")]
    {
        assert!(zsh.contains("local worktree_path path_only_arg"));
        assert!(!zsh.contains("local path path_only_arg"));
    }
    assert!(zsh.contains("compdef _zetta_entry_zvi zvi"));
    assert!(zsh.contains("_files"));
}

#[cfg(unix)]
#[test]
fn posix_zetta_routes_standalone_commands_to_path_and_live_commands_to_owner() {
    use std::{io::Write as _, os::unix::fs::PermissionsExt, process::Stdio};

    let _bash_test_lock = lock_bash_tests();
    let temporary = tempfile::tempdir().unwrap();
    let owner = temporary.path().join("owner-zetta");
    let path_zetta = temporary.path().join("zetta");
    let log = temporary.path().join("routing.log");
    let fake_zetta = |path: &std::path::Path, label: &str| {
        fs::write(
            path,
            format!(
                r#"#!/bin/sh
printf '%s:%s\n' '{label}' "$*" >> "$ZETTA_TEST_LOG"
"#
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    };
    fake_zetta(&owner, "owner");
    fake_zetta(&path_zetta, "path");

    let mut path =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
    path.insert(0, temporary.path().to_owned());
    let path = std::env::join_paths(path).unwrap();

    let cases = [
        ("path", "zetta vi README.md"),
        ("path", "zvi README.md"),
        ("path", "zetta edit README.md"),
        ("path", "zetta init bash"),
        ("path", "zetta splits"),
        ("path", "zetta mux list"),
        ("path", "zetta benchmark output"),
        ("path", "zetta terminal-size"),
        ("path", "zetta profile list"),
        ("path", "zetta profile themes"),
        ("path", "zetta profile --config config.json list"),
        ("path", "zetta project list"),
        ("path", "zetta cmd --list"),
        ("path", "zetta tabicon --list"),
        ("path", "zetta serial list"),
        ("path", "zetta http server --help"),
        ("path", "zetta tftp get --help"),
        ("path", "zetta copy --help"),
        ("path", "zetta paste --help"),
        ("path", "zetta wt status"),
        ("path", "zetta --profile Demo"),
        ("path", "zetta --profile Demo --split layout --theme Dark"),
        ("path", "zetta --config config.json --keymap keymap.json"),
        ("owner", "zetta"),
        ("owner", "zetta --new-window"),
        ("owner", "zetta --profile Demo --new-window"),
        ("owner", "zetta --command sh -c true"),
        ("owner", "zetta --replace-pane --split layout"),
        ("owner", "zetta --profile Demo --replace-pane"),
        ("owner", "zetta pane --list"),
        ("owner", "zetta pane wait api -- true"),
        ("owner", "zetta cmd build -- arg"),
        ("owner", "zetta attention summary"),
        ("owner", "zetta notify summary"),
        ("owner", "zetta theme pane Dark"),
        ("owner", "zetta overlay text"),
        ("owner", "zetta tabicon star"),
        ("owner", "zetta tabicon --reset"),
        ("owner", "zetta tabicon -r"),
        ("owner", "zetta project open"),
        ("owner", "zetta project add"),
        ("owner", "zetta project remove"),
        ("owner", "zetta profile disable Demo"),
        (
            "owner",
            "zetta profile --config config.json theme Demo Dark",
        ),
        ("owner", "zetta -c config.json profile remove Demo"),
    ];
    let owner_cases = cases
        .iter()
        .filter(|(route, _)| *route == "owner")
        .copied()
        .collect::<Vec<_>>();
    let deleted_owner_cases = owner_cases
        .iter()
        .map(|(_, command)| ("path", *command))
        .collect::<Vec<_>>();

    let run_shell = |shell: &str, host: &std::path::Path, selected_cases: &[(&str, &str)]| {
        fs::write(&log, "").unwrap();
        let commands = selected_cases
            .iter()
            .map(|(_, command)| *command)
            .collect::<Vec<_>>()
            .join("\n");
        let script = ShellIntegration::parse(shell).unwrap().eager_script();
        let prefix = if shell == "zsh" {
            "function compdef { :; }\n"
        } else {
            ""
        };
        let driver = format!("{prefix}{script}\n{commands}\n");

        let mut command = match shell {
            "bash" => bash_completion_command(),
            "zsh" => {
                let mut command = clean_shell_command("zsh");
                command.arg("-f");
                command
            }
            "fish" => {
                let mut command = clean_shell_command("fish");
                command.arg("--no-config");
                command
            }
            _ => unreachable!(),
        };
        let mut child = command
            .env("PATH", &path)
            .env("ZETTA_HOST_EXECUTABLE", host)
            .env("ZETTA_TEST_LOG", &log)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
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
            "{shell} routing script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let actual = String::from_utf8_lossy(&fs::read(&log).unwrap())
            .lines()
            .map(|line| line.split_once(':').unwrap().0.to_owned())
            .collect::<Vec<_>>();
        let expected = selected_cases
            .iter()
            .map(|(route, _)| (*route).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            expected,
            "{shell} routing did not match: {}",
            fs::read_to_string(&log).unwrap()
        );
    };

    for shell in ["bash", "zsh", "fish"] {
        if (shell == "bash" && !bash_available())
            || (shell != "bash"
                && clean_shell_command(shell)
                    .arg("--version")
                    .output()
                    .is_err())
        {
            continue;
        }
        run_shell(shell, &owner, &cases);
    }

    fs::remove_file(&owner).unwrap();
    let deleted_owner = temporary.path().join("zetta (deleted)");
    for shell in ["bash", "zsh", "fish"] {
        if (shell == "bash" && !bash_available())
            || (shell != "bash"
                && clean_shell_command(shell)
                    .arg("--version")
                    .output()
                    .is_err())
        {
            continue;
        }
        run_shell(shell, &deleted_owner, &deleted_owner_cases);
    }
}
