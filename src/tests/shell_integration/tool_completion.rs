//! Completion for the commands a feature brings: `mosh` and its SSH targets,
//! `zmux`, `zwt`, `zntfy` and the clipboard shortcuts.

use super::*;

#[cfg(feature = "zosh-client")]
#[test]
fn mosh_integration_is_conditional_and_has_full_client_completion() {
    let bash = ShellIntegration::Bash.eager_script();
    assert!(bash.contains("if ! type -t mosh >/dev/null 2>&1"));
    assert!(bash.contains("mosh() { zetta mosh \"$@\"; }"));
    assert!(bash.contains("complete -F _zetta_entry_zosh zosh"));
    assert!(bash.contains("complete -F _zetta_entry_zosh mosh"));
    assert!(bash.contains("_zetta_complete_ssh_targets"));

    let fish = ShellIntegration::Fish.eager_script();
    assert!(fish.contains("if not type -q mosh"));
    assert!(fish.contains("function mosh --wraps 'zetta mosh'"));
    assert!(fish.contains("complete -c mosh -f -a '(__zetta_tree_complete zosh)'"));
    assert!(fish.contains("complete -c zosh -f -a '(__zetta_tree_complete zosh)'"));
    assert!(fish.contains("function __zetta_ssh_targets"));

    let powershell = ShellIntegration::PowerShell.eager_script();
    assert!(powershell.contains("$zettaMoshMissing = -not (__zetta_command_available 'mosh')"));
    assert!(powershell.contains("function global:mosh { & zetta mosh @args }"));
    assert!(powershell.contains("Register-ArgumentCompleter -Native -CommandName zosh"));
    assert!(powershell.contains("$zettaSshTargets"));

    let zsh = ShellIntegration::Zsh.eager_script();
    assert!(zsh.contains("$+commands[mosh]"));
    assert!(zsh.contains("function mosh { zetta mosh \"$@\"; }"));
    assert!(zsh.contains("compdef _zetta_entry_zosh zosh"));
    assert!(zsh.contains("compdef _zetta_entry_zosh mosh"));
    assert!(zsh.contains("_zetta_ssh_targets() {"));
}

// Regression guard: `zetta mosh` (subcommand at words[2]) used to fall
// through the whole of `_zetta` because only the standalone wrapper
// (`words[1] == mosh`) had a branch, so neither SSH targets nor Mosh options
// were ever offered. A bare `mosh <TAB>` offered nothing either, because the
// host position was tested at CURRENT == 3 (the second positional). The
// first positional is the host for both forms.
#[cfg(feature = "zosh-client")]
#[test]
fn zsh_mosh_completion_offers_ssh_targets_and_options_for_wrapper_and_zetta_mosh() {
    use std::io::Write as _;
    use std::process::Stdio;

    if clean_shell_command("zsh")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let temporary = tempfile::tempdir().unwrap();
    let ssh_dir = temporary.path().join(".ssh");
    fs::create_dir_all(&ssh_dir).unwrap();
    fs::write(
        ssh_dir.join("config"),
        "Host web01 pi\n    User testuser\nHost *\n    StrictHostKeyChecking no\n",
    )
    .unwrap();

    let script = ShellIntegration::Zsh.eager_script();
    let driver = format!(
        "{script}\nfunction zetta {{ :; }}\nfunction compadd {{ print -r -- \"${{stage}}:candidates:$*\"; }}\nfunction _describe {{ local -a items=(\"${{(@P)4}}\"); print -r -- \"${{stage}}:options:${{(j: :)${{(@)items%%:*}}}}\"; }}\nstage=wrapper-host\nwords=(mosh '')\nCURRENT=2\n_zetta_entry_zosh\nstage=wrapper-option\nwords=(mosh '--')\nCURRENT=2\n_zetta_entry_zosh\nstage=zetta-host\nwords=(zetta mosh '')\nCURRENT=3\n_zetta_entry_zetta\nstage=zetta-option\nwords=(zetta mosh '--')\nCURRENT=3\n_zetta_entry_zetta\nstage=wrapper-after-host\nwords=(mosh 'web01' '')\nCURRENT=3\n_zetta_entry_zosh\nstage=zetta-after-host\nwords=(zetta mosh 'web01' '')\nCURRENT=4\n_zetta_entry_zetta\nstage=wrapper-after-flags\nwords=(mosh '--predict' 'always' '')\nCURRENT=4\n_zetta_entry_zosh\nstage=zosh-host\nwords=(zosh '')\nCURRENT=2\n_zetta_entry_zosh\nstage=zosh-after-host\nwords=(zosh 'web01' '')\nCURRENT=3\n_zetta_entry_zosh\nstage=wrapper-bind\nwords=(mosh '--bind-server' '')\nCURRENT=3\n_zetta_entry_zosh\nstage=zetta-bind\nwords=(zetta mosh '--bind-server' '')\nCURRENT=4\n_zetta_entry_zetta\n"
    );
    let mut child = clean_shell_command("zsh")
        .arg("-f")
        .env("HOME", temporary.path())
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
        "Zsh mosh completion failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let completions = String::from_utf8_lossy(&output.stdout);
    // An empty word offers the hosts: the options wait for a `-`.
    for stage in [
        "wrapper-host",
        "zetta-host",
        "wrapper-after-flags",
        "zosh-host",
    ] {
        assert!(
            completions
                .lines()
                .any(|line| line == format!("{stage}:candidates:-- web01 pi")),
            "hosts must be offered at {stage}: {completions}"
        );
    }
    for stage in ["wrapper-option", "zetta-option"] {
        assert!(
            completions.lines().any(|line| {
                line.starts_with(&format!("{stage}:options:--client --server --predict"))
                    && line.contains("--no-predict-overwrite")
                    && line.contains("--port")
                    && line.contains("--help --version")
            }),
            "missing full Mosh option surface at {stage}: {completions}"
        );
        assert!(
            !completions
                .lines()
                .any(|line| line.starts_with(&format!("{stage}:candidates:"))),
            "hosts must not be offered while completing an option at {stage}: {completions}"
        );
    }
    for stage in ["wrapper-after-host", "zetta-after-host", "zosh-after-host"] {
        assert!(
            !completions
                .lines()
                .any(|line| line.starts_with(&format!("{stage}:"))),
            "nothing may be offered once the host is given at {stage}: {completions}"
        );
    }
    for stage in ["wrapper-bind", "zetta-bind"] {
        assert!(
            completions
                .lines()
                .any(|line| line == format!("{stage}:candidates:-- ssh any")),
            "missing bind-server values at {stage}: {completions}"
        );
    }
}

// Regression guard: the SSH-target completion must actually read the user's
// SSH config and list its hosts; on macOS this is also where the awk `index`
// loop-variable bug surfaced (`awk: syntax error at source line 1`).
#[cfg(feature = "zosh-client")]
#[test]
fn bash_ssh_target_completion_reads_the_ssh_config() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let temporary = tempfile::tempdir().unwrap();
    let ssh_dir = temporary.path().join(".ssh");
    fs::create_dir_all(&ssh_dir).unwrap();
    fs::write(
        ssh_dir.join("config"),
        "Host web01 pi\n    User testuser\nHost *\n    StrictHostKeyChecking no\n",
    )
    .unwrap();

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "{script}\nCOMP_WORDS=(zetta mosh '')\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'zetta-mosh:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(mosh '')\nCOMP_CWORD=1\n_zetta_entry_zosh\nprintf 'wrapper:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zosh '')\nCOMP_CWORD=1\n_zetta_entry_zosh\nprintf 'zosh:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zosh 'web01' '')\nCOMP_CWORD=2\n_zetta_entry_zosh\nprintf 'zosh-after-host:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta mosh 'web01' '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'zetta-after-host:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zosh '--')\nCOMP_CWORD=1\n_zetta_entry_zosh\nprintf 'no-options:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zosh '--bind-server' '')\nCOMP_CWORD=2\n_zetta_entry_zosh\nprintf 'bind:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta mosh '--bind-server' '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'zetta-bind:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut command = bash_completion_command();
    command.env("HOME", temporary.path());
    let output = run_bash_driver(command, &driver);
    for label in ["zetta-mosh", "wrapper", "zosh"] {
        for host in ["web01", "pi"] {
            assert!(
                output.lines().any(|line| line == format!("{label}:{host}")),
                "missing {host} at {label}: {output}"
            );
        }
    }
    for label in ["zosh-after-host", "zetta-after-host"] {
        assert!(
            !output.lines().any(|line| {
                line.strip_prefix(&format!("{label}:"))
                    .is_some_and(|rest| !rest.is_empty())
            }),
            "nothing may be offered once the host is given at {label}: {output}"
        );
    }
    for option in ["--no-init", "--no-predict-overwrite", "--bind-server"] {
        assert!(
            output
                .lines()
                .any(|line| line == format!("no-options:{option}")),
            "missing option {option} while completing a dash word: {output}"
        );
    }
    assert!(
        !output.lines().any(|line| line == "no-options:web01"),
        "hosts must not be offered while completing an option: {output}"
    );
    for label in ["bind", "zetta-bind"] {
        for value in ["ssh", "any"] {
            assert!(
                output
                    .lines()
                    .any(|line| line == format!("{label}:{value}")),
                "missing bind-server value {value} at {label}: {output}"
            );
        }
    }
}

// Regression guard: the fish `zetta mosh` and `zosh` completions both feed on
// `__zetta_ssh_targets`, so the awk loop-variable fix has to be alive at
// runtime there too.
#[cfg(feature = "zosh-client")]
#[test]
fn fish_mosh_and_zosh_completion_offer_ssh_targets() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let temporary = tempfile::tempdir().unwrap();
    let ssh_dir = temporary.path().join(".ssh");
    fs::create_dir_all(&ssh_dir).unwrap();
    fs::write(
        ssh_dir.join("config"),
        "Host web01 pi\n    User testuser\nHost *\n    StrictHostKeyChecking no\n",
    )
    .unwrap();

    let script = ShellIntegration::Fish.eager_script();
    let script_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(script_file.path(), script).unwrap();
    for line in ["zetta mosh ", "zosh "] {
        let output = clean_shell_command("fish")
            .args([
                "--no-config",
                "-c",
                "source $argv[1]; complete -C \"$argv[2]\"",
                "--",
                script_file.path().to_str().unwrap(),
                line,
            ])
            .env_remove("ZETTA_HOST_EXECUTABLE")
            .env("HOME", temporary.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Fish rejected SSH-target completion for {line:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let completions = String::from_utf8_lossy(&output.stdout);
        for host in ["web01", "pi"] {
            assert!(
                completions
                    .lines()
                    .any(|candidate| candidate == host
                        || candidate.starts_with(&format!("{host}\t"))),
                "expected {host:?} in Fish completions for {line:?}: {completions}"
            );
        }
    }
    for line in ["zetta mosh web01 ", "zosh web01 "] {
        let output = clean_shell_command("fish")
            .args([
                "--no-config",
                "-c",
                "source $argv[1]; complete -C \"$argv[2]\"",
                "--",
                script_file.path().to_str().unwrap(),
                line,
            ])
            .env_remove("ZETTA_HOST_EXECUTABLE")
            .env("HOME", temporary.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Fish rejected post-host completion for {line:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let completions = String::from_utf8_lossy(&output.stdout);
        assert!(
            completions.trim().is_empty(),
            "nothing may be offered once the host is given for {line:?}: {completions}"
        );
    }
}

#[cfg(feature = "worktree")]
#[test]
fn bash_worktree_completion_offers_operations_and_long_worktree_options() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "{script}\ngit() {{ case \"$1 $2\" in branch\\ --show-current) printf '%s\\n' wt/feature ;; config\\ --local) printf '%s\\n' main ;; merge-base*) printf '%s\\n' split ;; rev-list*) printf '%s\\n' commit-one commit-two ;; esac; }}\nCOMP_WORDS=(zetta wt '')\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'operation:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new --)\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'option:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new --copy Carg)\nCOMP_CWORD=4\n_zetta_entry_zetta\nprintf 'copy-path:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt new -c Carg)\nCOMP_CWORD=4\n_zetta_entry_zetta\nprintf 'short-copy-path:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta wt sync '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'sync-commit:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zwt '')\nCOMP_CWORD=1\n_zetta_entry_zwt\nprintf 'wrapper:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zwt sync '')\nCOMP_CWORD=2\n_zetta_entry_zwt\nprintf 'wrapper-sync-commit:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let completions = run_bash_driver(bash_completion_command(), &driver);
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

#[cfg(feature = "zmux")]
#[test]
fn bash_zmux_completes_the_same_as_zetta_mux() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = "\
zetta() { if [[ $1 == mux && $2 == list ]]; then printf '%s\\n' '  reconnect id: 12345:7:42 (short: 42)'; fi; }\n\
zmux() { if [[ $1 == list ]]; then printf '%s\\n' '  reconnect id: 12345:7:42 (short: 42)'; fi; }\n\
COMP_WORDS=(zetta mux '')\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'mux:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux '')\nCOMP_CWORD=1\n_zetta_complete_zmux\nprintf 'zmux:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zetta mux share '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'mux-share:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux share '')\nCOMP_CWORD=2\n_zetta_complete_zmux\nprintf 'zmux-share:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux unshare '')\nCOMP_CWORD=2\n_zetta_complete_zmux\nprintf 'zmux-unshare:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zetta mux stop --)\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'mux-stop:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux stop --)\nCOMP_CWORD=2\n_zetta_complete_zmux\nprintf 'zmux-stop:%s\\n' \"${COMPREPLY[@]}\"\n\
ZETTA_NO_MUX=1\n\
COMP_WORDS=(zetta mux '')\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'no-mux:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zmux '')\nCOMP_CWORD=1\n_zetta_complete_zmux\nprintf 'no-mux-zmux:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zetta mux reconnect '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'no-mux-reconnect:%s\\n' \"${COMPREPLY[@]}\"\n\
COMP_WORDS=(zetta mux share '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'no-mux-share:%s\\n' \"${COMPREPLY[@]}\"\n";
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

#[cfg(feature = "zmux")]
#[test]
fn zsh_and_powershell_wire_up_zmux_completion() {
    let zsh = ShellIntegration::Zsh.eager_script();
    // `zmux` completes from `zetta mux`'s tree, under its own name.
    assert!(zsh.contains("_zetta_entry_zmux() { _zetta_tree_complete zmux; }"));
    assert!(zsh.contains("compdef _zetta_entry_zmux zmux"));

    let powershell = ShellIntegration::PowerShell.eager_script();
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

    let script = ShellIntegration::Bash.eager_script();
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
    let bash = ShellIntegration::Bash.eager_script();
    assert!(bash.contains("$path_only_arg == --help || $path_only_arg == -h"));
    assert!(bash.contains("command zwt abort --path-only"));

    let zsh = ShellIntegration::Zsh.eager_script();
    assert!(zsh.contains("$path_only_arg == --help || $path_only_arg == -h"));
    assert!(zsh.contains("command zwt abort --path-only"));

    let fish = ShellIntegration::Fish.eager_script();
    assert!(fish.contains("contains -- --help $operation_args; or contains -- -h $operation_args"));
    assert!(fish.contains("command zwt abort --path-only"));

    let powershell = ShellIntegration::PowerShell.eager_script();
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
        if shell == "bash" && !bash_available() {
            continue;
        }
        let version = if shell == "bash" {
            bash_command().arg("--version").output()
        } else {
            clean_shell_command(shell).arg("--version").output()
        };
        if version.is_err() {
            continue;
        }

        let script = ShellIntegration::parse(shell).unwrap().eager_script();
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
        if shell == "bash" && !bash_available() {
            continue;
        }
        let version = if shell == "bash" {
            bash_command().arg("--version").output()
        } else {
            clean_shell_command(shell).arg("--version").output()
        };
        if version.is_err() {
            continue;
        }

        let script = ShellIntegration::parse(shell).unwrap().eager_script();
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
        let script = shell.eager_script();
        assert!(script.contains("zntfy"));
        assert!(script.contains("notify"));
        assert!(script.contains("--app-name"));
        assert!(script.contains("--icon"));
        assert!(script.contains("--sound"));
        assert!(script.contains("--timeout"));
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
        let script = shell.eager_script();
        assert!(script.contains("notify") && script.contains("cleanup"));
        assert!(script.contains("--dry-run"));
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
        let script = shell.eager_script();
        assert!(script.contains("attention"));
        assert!(script.contains("--notify"));
        assert!(script.contains("--app-name"));
        assert!(script.contains("--timeout"));
    }
}

#[cfg(feature = "clipboard")]
#[test]
fn supported_shells_generate_copy_paste_completion_and_shortcuts() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.eager_script();
        assert!(script.contains("zcopy"));
        assert!(script.contains("zpaste"));
        assert!(!script.contains("zcopy() { zetta copy"));
        assert!(!script.contains("function zcopy { & zetta copy"));
        assert!(!script.contains("function zcopy --wraps"));
        assert!(script.contains("copy"));
        assert!(script.contains("paste"));
        assert!(script.contains("--pboard"));
        assert!(script.contains("--prefer"));
    }
}

// Regression guard: pbcopy/pbpaste already exist natively on macOS, so Zetta
// must not shadow them there, but every other platform should get them so
// pbcopy/pbpaste muscle memory keeps working.
#[cfg(feature = "clipboard")]
#[test]
fn pbcopy_and_pbpaste_are_gated_to_non_macos_platforms() {
    let bash = ShellIntegration::Bash.eager_script();
    assert!(bash.contains("pbcopy"));
    assert!(bash.contains("pbpaste"));
    assert!(bash.contains("unalias pbcopy pbpaste"));
    assert!(bash.contains("pbcopy() { command zcopy"));
    assert!(bash.contains("darwin*) ;;"));

    let zsh = ShellIntegration::Zsh.eager_script();
    assert!(zsh.contains("pbcopy"));
    assert!(zsh.contains("pbpaste"));
    assert!(zsh.contains("unalias pbcopy pbpaste"));
    assert!(zsh.contains("function pbcopy { command zcopy"));
    assert!(zsh.contains("darwin*) ;;"));

    let fish = ShellIntegration::Fish.eager_script();
    assert!(fish.contains("pbcopy"));
    assert!(fish.contains("pbpaste"));
    assert!(fish.contains("functions -e pbcopy pbpaste"));
    assert!(fish.contains("command zcopy $argv"));
    assert!(fish.contains("case Darwin\n    case '*'"));

    let powershell = ShellIntegration::PowerShell.eager_script();
    assert!(powershell.contains("pbcopy"));
    assert!(powershell.contains("pbpaste"));
    assert!(powershell.contains("if (-not $IsMacOS) {"));
    assert!(powershell.contains("Remove-Item -Path Alias:pbcopy,Alias:pbpaste"));
    assert!(powershell.contains("function pbcopy { & zcopy @args }"));
}

// Regression test: zsh expands an active alias while parsing a `name() {
// ... }` function definition of the same name, which fails to parse
// ("defining function based on alias") even when a preceding `unalias`
// removes it, because the whole `case` branch is parsed as one unit before
// any of it runs. `zsh -n` (syntax check only) does not catch this, since it
// depends on the alias actually being defined; only executing the script
// with a preexisting pbcopy/pbpaste alias (as a real user's zshrc would
// have) reproduces it. Zetta must use `function name { ... }` there instead.
#[cfg(feature = "clipboard")]
#[test]
fn zsh_accepts_the_generated_integration_with_a_preexisting_pbcopy_alias() {
    let script = ShellIntegration::Zsh.eager_script();
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
