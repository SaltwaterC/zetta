use super::*;

fn profiles() -> [Profile; 2] {
    [
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::Program("zsh".to_owned()),
            theme: None,
            icon: ProfileIcon::Zsh,
        },
    ]
}

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
    let mut command = std::process::Command::new("bash");
    // Do not let a user's shell startup environment change the exit status or
    // behavior of a generated script under test.
    command
        .env_remove("BASH_ENV")
        .env_remove("BASHOPTS")
        .env_remove("SHELLOPTS");
    command
}

#[test]
fn supported_shells_generate_completion_and_tftp_shortcut() {
    let profiles = profiles();
    assert!(shell_integration_help().contains("--replace-pane"));
    assert!(shell_integration_help().contains("zetta pane"));
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
        assert!(script.contains("ztftp"));
        assert!(script.contains("tftp"));
        assert!(script.contains("serial"));
        assert!(script.contains("http"));
        assert!(script.contains("init"));
        assert!(script.contains("EDITOR"));
        assert!(script.contains("zetta vi"));
        assert!(script.contains("zetta tabicon --list"));
        assert!(script.contains("zetta panetheme --list"));
        assert!(script.contains("zetta splits"));
        assert!(script.contains("zetta pane --list"));
        assert!(script.contains("--direction"));
        assert!(script.contains("--pane"));
        assert!(script.contains("replace-pane"));
        assert!(script.contains("zwt"));
        assert!(script.contains("wt"));
        for operation in ["new", "done", "status", "rerere"] {
            assert!(script.contains(operation));
        }
        assert!(script.contains("--path-only"));
        assert!(script.contains("--copy"));
    }
}

#[test]
fn shell_integration_reports_the_shell_cwd_while_children_run() {
    let profiles = profiles();

    assert!(
        ShellIntegration::Bash
            .script(&profiles)
            .contains("__zetta_report_cwd")
    );
    assert!(
        ShellIntegration::Fish
            .script(&profiles)
            .contains("--on-event fish_prompt")
    );
    assert!(
        ShellIntegration::PowerShell
            .script(&profiles)
            .contains("zetta-cwd:$zettaDirectory")
    );
    assert!(
        ShellIntegration::Zsh
            .script(&profiles)
            .contains("add-zsh-hook precmd __zetta_report_cwd")
    );
}

#[test]
fn vi_integration_is_conditional_and_has_cli_completion() {
    let profiles = profiles();

    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(bash.contains("if ! type -t vi >/dev/null 2>&1"));
    assert!(bash.contains("eval 'vi() { zetta vi \"$@\"; }'"));
    assert!(bash.contains("zvi() { zetta vi \"$@\"; }"));
    assert!(bash.contains("ZETTA_HOST_EXECUTABLE"));
    assert!(bash.contains("complete -F _zetta_complete zvi"));
    assert!(bash.contains("vi)\n            if [[ $current == -* ]]; then"));

    let fish = ShellIntegration::Fish.script(&profiles);
    assert!(fish.contains("if not type -q vi"));
    assert!(fish.contains("complete -c vi -F"));
    assert!(fish.contains("function zvi --wraps 'zetta vi'"));
    assert!(fish.contains("complete -c zvi -F"));
    assert!(fish.contains("function __zetta_option_unused"));
    assert!(fish.contains("ZETTA_HOST_EXECUTABLE"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
    assert!(powershell.contains("$zettaViMissing = -not (Get-Command vi"));
    assert!(powershell.contains("if ($zettaViMissing)"));
    assert!(powershell.contains("function zvi { & zetta vi @args }"));
    assert!(powershell.contains("Register-ArgumentCompleter -CommandName zvi"));
    assert!(powershell.contains("Get-ChildItem -Name -Path \"$wordToComplete*\""));
    assert!(powershell.contains("$_ -notin $words"));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(zsh.contains("$+commands[vi]"));
    assert!(zsh.contains("compdef _zetta vi"));
    assert!(zsh.contains("function zvi { zetta vi \"$@\"; }"));
    assert!(zsh.contains("ZETTA_HOST_EXECUTABLE"));
    assert!(zsh.contains("local worktree_path path_only_arg"));
    assert!(!zsh.contains("local path path_only_arg"));
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

    let script = ShellIntegration::Bash.script(&profiles());
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

    let script = ShellIntegration::Bash.script(&profiles());
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

    let script = ShellIntegration::Bash.script(&profiles());
    let driver = format!(
        "{script}\nCOMP_WORDS=(zetta wt '')\nCOMP_CWORD=2\n_zetta_complete\nprintf 'operation:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new --)\nCOMP_CWORD=3\n_zetta_complete\nprintf 'option:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new --copy Carg)\nCOMP_CWORD=4\n_zetta_complete\nprintf 'copy-path:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new -c Carg)\nCOMP_CWORD=4\n_zetta_complete\nprintf 'short-copy-path:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zwt '')\nCOMP_CWORD=1\n_zetta_complete_zwt\nprintf 'wrapper:%s\\n' \"${{COMPREPLY[@]}}\"\n"
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
        for operation in ["new", "done", "status", "rerere"] {
            assert!(
                completions
                    .lines()
                    .any(|line| line == format!("{prefix}{operation}")),
                "expected {operation} after {prefix}: {completions}"
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

#[cfg(unix)]
#[test]
fn bash_zwt_changes_directory_for_nested_paths_with_spaces() {
    use std::{io::Write as _, os::unix::fs::PermissionsExt as _, process::Stdio};

    let _bash_test_lock = lock_bash_tests();
    let temporary = tempfile::tempdir().unwrap();
    let start = temporary.path().join("start directory");
    let new_path = temporary.path().join("new worktree/feature api");
    let done_path = temporary.path().join("source worktree");
    std::fs::create_dir_all(&start).unwrap();
    std::fs::create_dir_all(&new_path).unwrap();
    std::fs::create_dir_all(&done_path).unwrap();

    let fake_zetta = temporary.path().join("zetta");
    std::fs::write(
        &fake_zetta,
        "#!/bin/sh\ncase \"$2\" in\n  new) printf '%s\\n' \"$ZETTA_TEST_NEW\" ;;\n  done) printf '%s\\n' \"$ZETTA_TEST_DONE\" ;;\n  *) exit 0 ;;\nesac\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_zetta).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_zetta, permissions).unwrap();

    let mut path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
    paths.insert(0, temporary.path().to_owned());
    path = std::env::join_paths(paths).unwrap();

    let script = ShellIntegration::Bash.script(&profiles());
    let driver = format!(
        "{script}\ncd '{}'\nzwt new --path-only 'feature/api'\nprintf 'new:%s\\n' \"$PWD\"\nzwt done --path-only\nprintf 'done:%s\\n' \"$PWD\"\n",
        start.display()
    );
    let mut child = bash_command()
        .args(["--noprofile", "--norc"])
        .current_dir(&start)
        .env_remove("ZETTA_HOST_EXECUTABLE")
        .env("PATH", path)
        .env("ZETTA_TEST_NEW", &new_path)
        .env("ZETTA_TEST_DONE", &done_path)
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
}

#[test]
fn worktree_wrappers_pass_help_through_without_capturing_it_as_a_path() {
    let bash = ShellIntegration::Bash.script(&profiles());
    assert!(bash.contains("$path_only_arg == --help || $path_only_arg == -h"));

    let zsh = ShellIntegration::Zsh.script(&profiles());
    assert!(zsh.contains("$path_only_arg == --help || $path_only_arg == -h"));

    let fish = ShellIntegration::Fish.script(&profiles());
    assert!(fish.contains("contains -- --help $operation_args; or contains -- -h $operation_args"));

    let powershell = ShellIntegration::PowerShell.script(&profiles());
    assert!(
        powershell.contains("$operationArgs -contains '--help' -or $operationArgs -contains '-h'")
    );
}

#[cfg(unix)]
#[test]
fn posix_zwt_help_does_not_change_directory_or_inject_path_only() {
    use std::{
        io::Write as _,
        os::unix::fs::PermissionsExt as _,
        process::{Command, Stdio},
    };

    let _bash_test_lock = lock_bash_tests();
    let temporary = tempfile::tempdir().unwrap();
    let start = temporary.path().join("start directory");
    let args_file = temporary.path().join("zetta arguments");
    std::fs::create_dir_all(&start).unwrap();

    let fake_zetta = temporary.path().join("zetta");
    std::fs::write(
        &fake_zetta,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ZETTA_TEST_ARGS\"\nprintf '%s\\n' 'Create a Git worktree for a temporary wt/NAME branch' 'Usage: zetta wt new [OPTIONS] NAME'\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_zetta).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_zetta, permissions).unwrap();

    let mut path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
    paths.insert(0, temporary.path().to_owned());
    path = std::env::join_paths(paths).unwrap();

    for shell in ["bash", "zsh"] {
        let version = if shell == "bash" {
            bash_command().arg("--version").output()
        } else {
            Command::new(shell).arg("--version").output()
        };
        if version.is_err() {
            continue;
        }

        let script = ShellIntegration::parse(shell).unwrap().script(&profiles());
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
            Command::new(shell)
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
            "wt\nnew\n--help\n",
            "{shell} zwt help wrapper changed the CLI arguments"
        );
    }
}

#[test]
fn supported_shells_generate_notify_completion_and_zntfy_shortcut() {
    let profiles = profiles();
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
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
fn supported_shells_generate_attention_completion() {
    let profiles = profiles();
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
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
    let profiles = profiles();
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
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
    let profiles = profiles();

    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(bash.contains("pbcopy"));
    assert!(bash.contains("pbpaste"));
    assert!(bash.contains("unalias pbcopy pbpaste"));
    assert!(bash.contains("darwin*) ;;"));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(zsh.contains("pbcopy"));
    assert!(zsh.contains("pbpaste"));
    assert!(zsh.contains("unalias pbcopy pbpaste"));
    assert!(zsh.contains("darwin*) ;;"));

    let fish = ShellIntegration::Fish.script(&profiles);
    assert!(fish.contains("pbcopy"));
    assert!(fish.contains("pbpaste"));
    assert!(fish.contains("functions -e pbcopy pbpaste"));
    assert!(fish.contains("case Darwin\n    case '*'"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
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
    let script = ShellIntegration::Zsh.script(&profiles());
    let combined = format!(
        "alias pbcopy='xclip -selection clipboard'\nalias pbpaste='xclip -selection clipboard -o'\n{script}"
    );

    let mut child = match std::process::Command::new("zsh")
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
    let profiles = profiles();

    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(bash.contains("--sound)\n            _zetta_complete_sound_names"));
    assert!(bash.contains("--sound|-s)\n            _zetta_complete_sound_names"));
    assert!(bash.contains(
        "elif [[ $command == notify || $command == attention ]]; then\n                _zetta_complete_sound_names"
    ));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(zsh.contains("--sound)\n            _zetta_sound_names"));
    assert!(zsh.contains("--sound|-s)\n            _zetta_sound_names"));
    assert!(
        zsh.contains(
            "elif [[ $words[2] == notify || $words[2] == attention ]]; then\n                _zetta_sound_names"
        )
    );

    let fish = ShellIntegration::Fish.script(&profiles);
    assert!(fish.contains("-l sound -r -a '(__zetta_sound_names)'"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
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
    let profiles = profiles();
    let bundled = ["zetta-default", "zetta-ok", "zetta-alarm"];
    let linux_only = ["bell", "message-new-instant", "trash-empty"];
    let macos_only = ["Basso", "Glass", "Sosumi"];
    let windows_only = ["IM", "Reminder", "SMS"];

    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(bash.contains("case \"$OSTYPE\" in"));
    assert!(bash.contains("darwin*)"));
    assert!(bash.contains("msys*|cygwin*|win32*)"));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(zsh.contains("case \"$OSTYPE\" in"));
    assert!(zsh.contains("darwin*)"));
    assert!(zsh.contains("msys*|cygwin*|win32*)"));

    let fish = ShellIntegration::Fish.script(&profiles);
    assert!(fish.contains("switch (uname)"));
    assert!(fish.contains("case Darwin"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
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
// benchmark-output's --output-type, and the top-level/panetheme --theme flag
// now shares it too. Completion after -t/--output-type/--theme must stay
// scoped to the active subcommand instead of always suggesting
// benchmark-output's repeated/unique values.
#[test]
fn notify_timeout_completion_does_not_leak_into_other_short_t_flags() {
    let profiles = profiles();

    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(bash.contains(
        "--output-type|-t|--theme|--text)\n            if [[ $command == profile ]]; then\n                _zetta_complete_profile_themes\n            elif [[ $command == -* ]]; then\n                _zetta_complete_profile_themes\n            elif [[ $command == panetheme ]]; then"
    ));
    assert!(bash.contains("_zetta_compgen 'repeated unique'"));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(zsh.contains(
        "--output-type|-t|--theme|--text)\n            if [[ $words[2] == profile || $words[2] == -* ]]; then\n                _zetta_profile_themes \"${config_args[@]}\"\n            elif [[ $words[2] == panetheme ]]; then"
    ));
    assert!(zsh.contains("compadd -- repeated unique"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
    assert!(powershell.contains(
        "elseif ($previous -in '--output-type', '-t', '--theme', '--text') {\n        if ($subcommand -eq 'profile' -or $null -eq $subcommand) { & $zettaProfileThemes $configArguments }\n        elseif ($subcommand -eq 'panetheme') { & $zettaPaneThemes }"
    ));
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

    let script = ShellIntegration::Bash.script(&profiles());
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

    let script = ShellIntegration::Bash.script(&profiles());
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

    let script = ShellIntegration::Bash.script(&profiles());
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
    let profiles = profiles();
    let scripts = [
        ShellIntegration::Bash.script(&profiles),
        ShellIntegration::Fish.script(&profiles),
        ShellIntegration::PowerShell.script(&profiles),
        ShellIntegration::Zsh.script(&profiles),
    ];

    for script in scripts {
        assert!(script.contains("serial list"));
        assert!(script.contains("tftp") && script.contains("server"));
    }
}

#[test]
fn service_completion_uses_command_local_short_options() {
    let profiles = profiles();
    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(bash.contains("--device)\n            _zetta_complete_serial_devices"));
    assert!(bash.contains("--data-bits|-D)"));
    assert!(bash.contains(
        "if [[ $command == serial ]]; then\n                _zetta_compgen 'none odd even'"
    ));
    assert!(bash.contains(
        "if [[ $command == http || ( $command == tftp && ${COMP_WORDS[2]} == server ) ]]; then"
    ));

    let fish = ShellIntegration::Fish.script(&profiles);
    assert!(fish.contains("-l device"));
    assert!(fish.contains("-l data-bits"));
    assert!(fish.contains("-l parity"));
    assert!(fish.contains("__zetta_tftp_server' -l config"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
    assert!(powershell.contains("'--device', '-d'"));
    assert!(powershell.contains("'--data-bits', '-D'"));
    assert!(powershell.contains("$previous -eq '-p' -and $subcommand -eq 'serial'"));
    assert!(powershell.contains("{ '--root', '--port', '--config', '--help' }"));

    let zsh = ShellIntegration::Zsh.script(&profiles);
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
    let profiles = profiles();

    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(!bash.contains("'-d --device"));
    assert!(!bash.contains("'-r --root -p --port -c --config -h --help'"));
    assert!(!bash.contains("'-p --port -h --help'"));
    assert!(
        bash.contains(
            "'--device --baud-rate --data-bits --parity --stop-bits --flow-control --help'"
        )
    );
    assert!(bash.contains("'--root --port --config --help'"));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(!zsh.contains("-d --device -b --baud-rate"));
    assert!(!zsh.contains("compadd -- -r --root -p --port -c --config -h --help"));
    assert!(!zsh.contains("compadd -- -p --port -h --help"));
    assert!(zsh.contains(
        "_zetta_options --device --baud-rate --data-bits --parity --stop-bits --flow-control --help"
    ));
    assert!(zsh.contains("_zetta_options --root --port --config --help"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
    assert!(!powershell.contains("'-d', '--device', '-b', '--baud-rate'"));
    assert!(
        !powershell.contains("'-r', '--root', '-p', '--port', '-c', '--config', '-h', '--help'")
    );
    assert!(!powershell.contains("'-p', '--port', '-h', '--help'"));
    assert!(powershell.contains(
        "'--device', '--baud-rate', '--data-bits', '--parity', '--stop-bits', '--flow-control', '--help'"
    ));
    assert!(powershell.contains("'--root', '--port', '--config', '--help'"));

    let fish = ShellIntegration::Fish.script(&profiles);
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
    assert!(fish.contains("subcommand_from notify' -l app-name"));
}

#[test]
fn profile_completion_uses_line_oriented_dynamic_endpoints() {
    let scripts = [
        ShellIntegration::Bash.script(&profiles()),
        ShellIntegration::Fish.script(&profiles()),
        ShellIntegration::PowerShell.script(&profiles()),
        ShellIntegration::Zsh.script(&profiles()),
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
// (with embedded single quotes doubled), not the raw name, or `panetheme`
// rejects it with "only one theme may be specified".
#[test]
fn powershell_quotes_spaced_completion_values() {
    let powershell = ShellIntegration::PowerShell.script(&profiles());

    assert!(powershell.contains(r#"$value -match '\s'"#));
    assert!(powershell.contains(r#""'" + $value.Replace("'", "''")"#));
    assert!(powershell.contains(
        "[System.Management.Automation.CompletionResult]::new($text, $value, 'ParameterValue', $value)"
    ));
}

#[cfg(windows)]
#[test]
fn powershell_accepts_the_generated_integration_syntax() {
    let script = ShellIntegration::PowerShell.script(&profiles());

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
        ShellIntegration::current(None).unwrap(),
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
    let profiles = profiles();
    assert!(
        ShellIntegration::PowerShell
            .script(&profiles)
            .contains("param($wordToComplete, $commandAst, $cursorPosition)")
    );
    assert!(
        ShellIntegration::Zsh
            .script(&profiles)
            .contains("terminal-size)")
    );
    assert!(
        ShellIntegration::Zsh
            .script(&profiles)
            .contains("compadd -S ' ' -- benchmark")
    );
}

#[test]
fn terminal_size_completions_include_pane_resize_options() {
    let profiles = profiles();
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
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
    let profiles = profiles();
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
        assert!(script.contains("--delete-after"));
        assert!(!script.contains("-d --delete-after"));
    }
}

#[test]
fn generated_scripts_include_root_flags_and_configured_profiles() {
    let profiles = profiles();
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
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
        assert!(script.contains("--path"));
        assert!(!script.contains("quarters four-vertical three-left three-right"));
    }
}

#[test]
fn generated_scripts_offer_the_shared_overlay_colour_catalogue() {
    let profiles = profiles();
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
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

#[test]
fn generated_scripts_only_offer_long_form_flags() {
    let profiles = profiles();
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.script(&profiles);
        match shell {
            ShellIntegration::Bash => {
                assert!(script.contains(
                    "terminal-size sessions pane profile project edit vi init serial http tftp notify attention copy paste splits tabicon panetheme overlay wt --help --version --config --keymap --profile --split --replace-pane --theme'"
                ));
                assert!(script.contains("auto zetta bash zsh fish"));
            }
            ShellIntegration::Fish => {
                assert!(script.contains("-l profile -r"));
                assert!(script.contains("-l replace-pane"));
                assert!(!script.contains("-s p -l profile"));
                assert!(script.contains(
                    "-s c -r -n '__zetta_has_profile_subcommand; and __zetta_short_option -c'"
                ));
                assert!(script.contains("auto zetta bash zsh fish"));
            }
            ShellIntegration::PowerShell => {
                assert!(script.contains(
                    "'--help', '--version', '--config', '--keymap', '--profile', '--split', '--replace-pane', '--theme'"
                ));
                assert!(script.contains("'overlay', 'wt', '--help'"));
                assert!(script.contains("'auto', 'zetta', 'bash', 'zsh', 'fish'"));
            }
            ShellIntegration::Zsh => {
                assert!(
                    script.contains("_zetta_options --help --version --config --keymap --profile")
                );
                assert!(script.contains("auto zetta bash zsh fish"));
            }
        }
    }
}

#[test]
fn fish_script_emits_long_option_candidates_for_every_command_context() {
    let script = ShellIntegration::Fish.script(&profiles());

    for context in [
        "root",
        "init",
        "serial",
        "http",
        "terminal-size",
        "sessions",
        "benchmark-output",
        "benchmark",
        "serial-console",
        "http-server",
        "tftp",
        "tftp-client",
        "tftp-server",
        "notify",
        "attention",
        "copy",
        "paste",
        "tabicon",
        "panetheme",
        "splits",
        "pane",
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
    use std::process::Command;

    if Command::new("fish").arg("--version").output().is_err() {
        return;
    }

    let script = ShellIntegration::Fish.script(&profiles());
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
                "list", "themes", "disable", "enable", "theme", "icon", "default", "add", "remove",
            ][..],
        ),
        ("zetta profile disable ", &["System", "WSL: Ubuntu"][..]),
        ("zetta profile theme ", &["System", "WSL: Ubuntu"][..]),
        ("zetta profile theme System ", &["Gruvbox Light Hard"][..]),
        (
            "zetta -c profiles.json profile disable ",
            &["Configured Shell"][..],
        ),
        (
            "zetta benchmark ",
            &[
                "--terminal-render-workload",
                "--terminal-checkerboard-workload",
                "--terminal-sparse-update-workload",
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
            "zetta benchmark-output ",
            &["--size", "--output-type", "--help"][..],
        ),
        (
            "zetta terminal-size ",
            &["--json", "--resize", "--columns", "--rows", "--help"][..],
        ),
        ("zetta sessions ", &["--json", "--help"][..]),
        ("zetta splits ", &["--help"][..]),
        (
            "zetta pane ",
            &[
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
        (
            "zetta panetheme ",
            &["--theme", "--reset", "--list", "--help"][..],
        ),
        ("zetta panetheme -t ", &[][..]),
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
        let output = Command::new("fish")
            .args([
                "--no-config",
                "-c",
                "function zetta; if test \"$argv[1]\" = splits; printf '%s\\n' custom-layout quarters four-vertical three-left three-right; else if test \"$argv[1]\" = profile; and test \"$argv[2]\" = list; if test \"$argv[3]\" = --config; and test \"$argv[4]\" = profiles.json; printf '%s\\n' 'Configured Shell'; else; printf '%s\\n' System 'WSL: Ubuntu'; end; else if test \"$argv[1]\" = profile; and test \"$argv[2]\" = themes; printf '%s\\n' 'Gruvbox Light Hard'; end; end; source $argv[1]; complete -C \"$argv[2]\"",
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

// Regression test: --theme requires --profile, but each is a plain root
// flag that fish's builtin __fish_use_subcommand cannot tell apart from a
// subcommand once its value has been typed (it treats any non-flag word as
// proof a subcommand was given). Without __zetta_use_subcommand accounting
// for consumed flag values, --profile NAME would stop completing --theme
// and vice versa, and typing either would also incorrectly keep offering
// subcommand names, which are only valid as the very first argument.
#[test]
fn profile_and_theme_root_flags_keep_completing_each_other() {
    use std::process::Command;

    if Command::new("fish").arg("--version").output().is_err() {
        return;
    }

    let script = ShellIntegration::Fish.script(&profiles());
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
        let output = Command::new("fish")
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
    use std::process::Command;

    if Command::new("fish").arg("--version").output().is_err() {
        return;
    }

    let script = ShellIntegration::Fish.script(&profiles());
    let script_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(script_file.path(), script).unwrap();

    for line in ["zetta vi --help ", "zetta copy --help ", "zcopy --help "] {
        let output = Command::new("fish")
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

    let output = Command::new("fish")
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

    let output = Command::new("fish")
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
    let profiles = profiles();
    assert!(
        ShellIntegration::Bash
            .script(&profiles)
            .contains("(( positional == 1 )) && COMPREPLY=( $(compgen -f")
    );
    assert!(
        ShellIntegration::Zsh
            .script(&profiles)
            .contains("(( position == 1 )) && _files")
    );
    assert!(
        !ShellIntegration::Bash
            .script(&profiles)
            .contains("positional >= 2")
    );
    assert!(
        !ShellIntegration::Zsh
            .script(&profiles)
            .contains("position >= 2")
    );
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
