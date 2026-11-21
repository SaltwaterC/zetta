//! The generated completion scripts: what each shell offers for the root
//! options, panes, profiles, services and the commands that hand off to
//! another.

use super::*;

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
        let script = shell.eager_script();
        assert!(script.contains("ztftp"));
        assert!(script.contains("zhttp"));
        assert!(script.contains("tftp"));
        assert!(script.contains("serial"));
        assert!(script.contains("http"));
        assert!(script.contains("init"));
        assert!(script.contains("EDITOR"));
        assert!(script.contains("zetta vi"));
        assert!(script.contains("tabicon --list"));
        assert!(script.contains("--reset"));
        assert!(script.contains("theme pane --list"));
        assert!(script.contains("zetta:splits"));
        assert!(script.contains("zetta pane --list"));
        assert!(script.contains("cmd --list"));
        #[cfg(feature = "zmux")]
        assert!(script.contains("ZETTA_NO_MUX"));
        assert!(script.contains("--direction"));
        assert!(script.contains("--pane"));
        // Every lifecycle marker carries the pane's nonce (3.17); none is
        // emitted in the untokened form any printed output can produce.
        assert!(script.contains("zetta-event;"));
        assert!(!script.contains("zetta-event:"));
        assert!(script.contains(":tracking-ready"));
        assert!(script.contains(":command-started:"));
        assert!(script.contains(":command-finished:"));
        assert!(!script.contains("codex"));
        assert!(!script.contains("tabicon ai_open_ai"));
        assert!(!script.contains("run --wait"));
        assert!(script.contains("allow-failure"));
        assert!(script.contains("replace-pane"));
        assert!(script.contains("--new-window"));
        // `-w` is `--new-window`'s short form, and zpager's `--width` and
        // the TFTP server's `--writable`; it is only ever recognised.
        assert!(
            script.match_indices(" -w").all(|(index, _)| {
                ["--new-window", "--width", "--writable"]
                    .iter()
                    .any(|long| script[..index].ends_with(long))
            }),
            "{shell:?} offers a -w that is not a recognised short form"
        );
        assert!(script.contains("--command"));
        #[cfg(feature = "worktree")]
        {
            assert!(script.contains("zwt"));
            assert!(script.contains("wt"));
        }
        #[cfg(feature = "zmux")]
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

#[cfg(not(feature = "zmux"))]
#[test]
fn generated_shell_integrations_omit_the_zmux_cli_surface() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.eager_script();
        assert!(!script.contains("--no-mux"));
        assert!(!script.contains("zetta mux"));
        match shell {
            ShellIntegration::Bash => {
                assert!(!script.contains("complete -F _zetta_complete_zmux zmux"));
            }
            ShellIntegration::Fish => assert!(!script.contains("complete -c zmux")),
            ShellIntegration::PowerShell => {
                assert!(!script.contains("CommandName zmux"));
            }
            ShellIntegration::Zsh => assert!(!script.contains("compdef _zmux zmux")),
        }
    }
}

#[test]
fn bash_pane_wait_completion_labels_then_delegates_to_the_wrapped_command() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "{script}\nprintf '\\n'\nzetta() {{ if [[ $1 == pane && $2 == --list ]]; then printf '%s\\n' api db deploy; fi; }}\nCOMP_WORDS=(zetta pane wait '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'first:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta pane wait api,)\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'second:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta pane wait api -- tru)\nCOMP_CWORD=5\n_zetta_entry_zetta\nprintf 'after-delimiter-command:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta pane wait api -- git Carg)\nCOMP_CWORD=6\n_zetta_entry_zetta\nprintf 'after-delimiter-arg:%s\\n' \"${{COMPREPLY[@]}}\"\n"
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
    // Past the `--`, completion must behave as if `zetta pane wait ... --`
    // were not on the command line at all: the wrapped command's own name
    // completes normally (here, against Bash's builtin/PATH commands)...
    assert!(
        completions
            .lines()
            .any(|line| line == "after-delimiter-command:true"),
        "the wrapped command's own name should complete normally past the delimiter: {completions}"
    );
    assert!(
        !completions
            .lines()
            .any(|line| line == "after-delimiter-command:api"),
        "pane labels must not be offered past the delimiter: {completions}"
    );
    // ...and, once that command is named, its own arguments fall back to
    // plain file completion rather than going silent.
    assert!(
        completions
            .lines()
            .any(|line| line == "after-delimiter-arg:Cargo.toml"),
        "the wrapped command's arguments should fall back to file completion past the delimiter: {completions}"
    );
}

// zsh's `_normal` dispatch can only run inside a real completion context (it
// panics with "can only be called from completion function" otherwise), so
// unlike the Bash and Fish variants this is asserted statically rather than
// by driving a live completion, matching how `_files` is covered elsewhere
// in this file (for example `vi_integration_is_conditional_and_has_cli_completion`).
#[test]
fn zsh_pane_wait_completion_shifts_words_and_delegates_past_the_delimiter() {
    let zsh = ShellIntegration::Zsh.eager_script();
    let command_block = zsh
        .split("        command)\n")
        .nth(1)
        .expect("missing command-line completion block");
    let command_block = &command_block[..command_block
        .find(";;")
        .expect("missing end of command-line completion block")];
    assert!(command_block.contains(r#"words=("${(@)words[rest,-1]}")"#));
    assert!(command_block.contains("(( CURRENT -= rest - 1 ))"));
    assert!(command_block.contains("_normal"));
    // `rest` is where the engine saw `--`, and `pane wait` hands it a
    // command line.
    assert!(zsh.contains("ended=1 rest=$(( index + 1 ))"));
    assert!(zsh.contains("spec_rest='command'"));
}

#[test]
fn fish_pane_wait_completion_labels_then_delegates_to_the_wrapped_command() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let script = ShellIntegration::Fish.eager_script();
    let script_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(script_file.path(), &script).unwrap();

    let complete = |line: &str| {
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
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let completions = complete("zetta pane wait a");
    assert!(
        completions.lines().any(|line| line.starts_with("--")),
        "missing pane wait options before the delimiter: {completions}"
    );

    let completions = complete("zetta pane wait api -- tru");
    assert!(
        completions.lines().any(|line| line.starts_with("true")),
        "the wrapped command's own name should complete normally past the delimiter: {completions}"
    );
    assert!(
        !completions.lines().any(|line| line.starts_with("api")),
        "pane labels must not be offered past the delimiter: {completions}"
    );

    // `cat` (rather than `git`) here deliberately: git's own fish completion
    // fuzzy-matches subcommand names since fish 4 and, unlike fish 3, that
    // now suppresses the file-completion fallback for a token that doesn't
    // match any subcommand, which made this assert on the delegation target's
    // completion quirks rather than on zetta's own delegation.
    let completions = complete("zetta pane wait api -- cat Carg");
    assert!(
        completions
            .lines()
            .any(|line| line.starts_with("Cargo.toml")),
        "the wrapped command's arguments should fall back to file completion past the delimiter: {completions}"
    );
}

#[test]
fn bash_project_command_completion_is_dynamic_and_stops_at_delimiter() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "{script}\nzetta() {{ if [[ $1 == cmd && $2 == --list ]]; then printf '%s\\n' build check test:unit; fi; }}\nCOMP_WORDS=(zetta cmd '')\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'names:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd t)\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'prefix:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd -)\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'dash:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd build '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'after-name:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd build -- '')\nCOMP_CWORD=4\n_zetta_entry_zetta\nprintf 'after-delimiter:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta cmd --list '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'after-list:%s\\n' \"${{COMPREPLY[@]}}\"\n"
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
    // Options wait for a `-` where the name has candidates of its own.
    assert!(!completions.lines().any(|line| line == "names:--list"));
    for option in ["--help", "--list", "--"] {
        assert!(
            completions
                .lines()
                .any(|line| line == format!("dash:{option}")),
            "missing project command option {option:?} at a `-`: {completions}"
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

    let script = ShellIntegration::Zsh.eager_script();
    let driver = format!(
        "{script}\nfunction zetta {{ if [[ $1 == cmd && $2 == --list ]]; then print -r -- build check; fi; }}\nfunction compadd {{ print -r -- \"${{stage}}:candidates:$*\"; }}\nfunction _describe {{ local -a names=(${{(P)argv[-1]}}); print -r -- \"${{stage}}:options:${{(j: :)${{(@)names%%:*}}}}\"; }}\nstage=empty\nwords=(zetta cmd '')\nCURRENT=3\n_zetta_entry_zetta\nstage=dash\nwords=(zetta cmd -)\nCURRENT=3\n_zetta_entry_zetta\nstage=name\nwords=(zetta cmd build '')\nCURRENT=4\n_zetta_entry_zetta\nstage=list\nwords=(zetta cmd --list '')\nCURRENT=4\n_zetta_entry_zetta\n"
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
        !completions
            .lines()
            .any(|line| line.starts_with("empty:options:-"))
    );
    assert!(
        completions
            .lines()
            .any(|line| line == "dash:options:--list --help --"),
        "unexpected Zsh options at a `-`: {completions}"
    );
    assert!(
        completions
            .lines()
            .any(|line| line == "name:options:--help --"),
        "unexpected Zsh options after the name: {completions}"
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
        let script = shell.eager_script();
        assert!(!script.contains("zwt"), "{shell:?} emitted zwt integration");
        assert!(
            !script.contains("ZETTA_WORKTREE"),
            "{shell:?} emitted a worktree template marker"
        );
        assert!(!script.contains("--path-only"));
        assert!(!script.contains("--copy"));
    }
}

// Regression guard: the bash template is embedded verbatim (include_str!, no
// template expansion), so `\${...}` reaches Bash as a literal backslash
// escape that prevents `__ZETTA_MOSH_WRAPPER` and `COMP_WORDS` from ever
// expanding, which breaks the generated `vi` and `mosh` wrapper completions
// and makes the tree engine see nothing to complete.
#[test]
fn bash_template_has_no_literal_escaped_expansions() {
    let bash = ShellIntegration::Bash.eager_script();
    assert!(bash.contains("if [[ ${__ZETTA_VI_WRAPPER:-0} == 1 ]]; then"));
    assert!(bash.contains("local node=$1 current=${COMP_WORDS[COMP_CWORD]}"));
    assert!(
        !bash.contains("\\${"),
        "generated Bash must not contain literal \\${{...}}"
    );
}

// Regression guard: every SSH-host helper runs `for (index = 2; ...)`, but
// `index` is a builtin awk function and the awk shipped on macOS rejects it
// as a loop variable (`awk: syntax error at source line 1`), so no host was
// ever offered there. The loop variable must be a name awk everywhere
// accepts.
#[test]
fn ssh_target_helpers_use_an_awk_portable_loop_variable() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::Zsh,
    ] {
        let script = shell.eager_script();
        assert!(
            script.contains("for (field = 2; field <= NF; field++)"),
            "{shell:?} SSH-target helper must use a portable awk loop variable"
        );
        assert!(!script.contains("for (index = 2;"));
    }
}

#[test]
fn bash_does_not_repeat_options_and_completes_vi_files() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "{script}\nCOMP_WORDS=(zetta vi --)\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'option:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta vi --help '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'file:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta vi Carg)\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'file:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let completions = run_bash_driver(bash_completion_command(), &driver);
    assert!(completions.lines().any(|line| line == "option:--help"));
    assert!(!completions.lines().any(|line| line == "file:--help"));
    assert!(completions.lines().any(|line| line == "file:Cargo.toml"));
}

#[test]
fn bash_color_completion_offers_named_presets_for_long_and_short_flags() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "{script}\nCOMP_WORDS=(zetta overlay --color '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'long:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta overlay -c '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'short:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let completions = run_bash_driver(bash_completion_command(), &driver);
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
fn sound_completion_calls_a_shared_helper_from_every_call_site() {
    // Every command taking a sound completes from its command tree, which
    // names the helper.
    let bash = ShellIntegration::Bash.eager_script();
    assert!(bash.contains(
        "--sound|-s) spec_option=--sound spec_value='call:_zetta_complete_sound_names' ;;"
    ));

    let zsh = ShellIntegration::Zsh.eager_script();
    assert!(
        zsh.contains("(--sound|-s) spec_option=--sound spec_value='call:_zetta_sound_names' ;;")
    );

    let fish = ShellIntegration::Fish.eager_script();
    assert!(fish.contains("printf '%s\\n' --sound 'call:__zetta_sound_names'"));

    let powershell = ShellIntegration::PowerShell.eager_script();
    assert!(powershell.contains("'-s' { '--sound', 'call:zettaSoundNames' }"));
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

    let bash = ShellIntegration::Bash.eager_script();
    assert!(bash.contains("case \"$OSTYPE\" in"));
    assert!(bash.contains("darwin*)"));
    assert!(bash.contains("msys*|cygwin*|win32*)"));

    let zsh = ShellIntegration::Zsh.eager_script();
    assert!(zsh.contains("case \"$OSTYPE\" in"));
    assert!(zsh.contains("darwin*)"));
    assert!(zsh.contains("msys*|cygwin*|win32*)"));

    let fish = ShellIntegration::Fish.eager_script();
    assert!(fish.contains("switch $__zetta_platform"));
    assert!(fish.contains("case Darwin"));

    let powershell = ShellIntegration::PowerShell.eager_script();
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

// Regression test: --theme requires --profile, so completing one must keep
// offering the other. Both are root flags handled by the same "$command ==
// -*" branch that also stops the script from falling through to a
// subcommand's (empty) completions once any root flag has been typed.
#[test]
fn profile_and_theme_root_flags_keep_completing_each_other_in_bash() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "{script}\nCOMP_WORDS=(zetta --profile System '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'after-profile:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta --theme Dracula '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'after-theme:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let completions = run_bash_driver(bash_completion_command(), &driver);
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
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "zetta() {{\n    if [[ $1 == splits ]]; then\n        printf '%s\\n' custom-layout quarters four-vertical three-left three-right\n    elif [[ $1 == profile && $2 == list ]]; then\n        if [[ $3 == --config && $4 == profiles.json ]]; then\n            printf '%s\\n' 'Configured Shell'\n        else\n            printf '%s\\n' 'System' 'WSL: Ubuntu'\n        fi\n    fi\n}}\n{script}\nCOMP_WORDS=(zetta --split '')\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'long:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta -s '')\nCOMP_CWORD=2\n_zetta_entry_zetta\nprintf 'short:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta --profile System '')\nCOMP_CWORD=3\n_zetta_entry_zetta\nprintf 'profile:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta --split quarters --profile '')\nCOMP_CWORD=4\n_zetta_entry_zetta\nprintf 'combined:%s\\n' \"${{COMPREPLY[@]}}\"\nCOMP_WORDS=(zetta -c profiles.json profile disable '')\nCOMP_CWORD=5\n_zetta_entry_zetta\nprintf 'profile-config:%s\\n' \"${{COMPREPLY[@]}}\"\n"
    );
    let mut command = bash_completion_command();
    command.env_remove("ZETTA_HOST_EXECUTABLE");
    let completions = run_bash_driver(command, &driver);
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
    let configured = format!("profile-config:{}", bash_offered("Configured Shell"));
    assert!(
        completions.lines().any(|line| line == configured),
        "expected {configured:?}: {completions}"
    );
}

#[test]
fn bash_pane_completion_offers_directions_and_live_labels() {
    use std::io::Write as _;
    use std::process::Stdio;

    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }

    let script = ShellIntegration::Bash.eager_script();
    let driver = format!(
        "zetta() {{
    if [[ $1 == pane && $2 == --list ]]; then
        printf '%s\\n' 'Pane 1' api 'Build runner'
    fi
}}
{script}
COMP_WORDS=(zetta pane '')
COMP_CWORD=2
_zetta_entry_zetta
printf 'options:%s\\n' \"${{COMPREPLY[@]}}\"
COMP_WORDS=(zetta pane --direction '')
COMP_CWORD=3
_zetta_entry_zetta
printf 'directions:%s\\n' \"${{COMPREPLY[@]}}\"
COMP_WORDS=(zetta pane --pane '')
COMP_CWORD=3
_zetta_entry_zetta
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
        let offered = bash_offered(label);
        assert!(
            completions
                .lines()
                .any(|line| line == format!("labels:{offered}")),
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
        ShellIntegration::Bash.eager_script(),
        ShellIntegration::Fish.eager_script(),
        ShellIntegration::PowerShell.eager_script(),
        ShellIntegration::Zsh.eager_script(),
    ];

    for script in scripts {
        assert!(script.contains("serial list"));
        assert!(script.contains("tftp") && script.contains("server"));
    }
}

// Regression test: commit 72afe3b ("Add serial console, and HTTP, TFTP
// servers to CLI integration") offered both short and long option names as
// completion candidates for `serial console`, `http server`, and `tftp`,
// contrary to the "long form only in autocomplete" rule in AGENTS.md. Short
// forms must remain valid on the command line (see cli_services.rs and
// tftp.rs parsing) but must not be offered as completion candidates.
#[test]
fn service_subcommand_completions_only_offer_long_form_flags() {
    let bash = ShellIntegration::Bash.eager_script();
    assert!(!bash.contains("'-d --device"));
    assert!(!bash.contains("'-r --root -p --port -c --config -h --help'"));
    assert!(!bash.contains("'-p --port -h --help'"));

    let zsh = ShellIntegration::Zsh.eager_script();
    assert!(!zsh.contains("-d --device -b --baud-rate"));
    assert!(!zsh.contains("compadd -- -r --root -p --port -c --config -h --help"));
    assert!(!zsh.contains("compadd -- -p --port -h --help"));

    let powershell = ShellIntegration::PowerShell.eager_script();
    assert!(!powershell.contains("'-d', '--device', '-b', '--baud-rate'"));
    assert!(
        !powershell.contains("'-r', '--root', '-p', '--port', '-c', '--config', '-h', '--help'")
    );
    assert!(!powershell.contains("'-p', '--port', '-h', '--help'"));

    let fish = ShellIntegration::Fish.eager_script();
    assert!(!fish.contains("-s d -l device"));
    assert!(!fish.contains("-s r -l root"));
    assert!(!fish.contains("-s p -l port"));
    assert!(!fish.contains("-s c -l config"));

    // `serial`, `http`, `notify` and `attention` complete from their command trees,
    // which offer only long options by construction: see
    // `only_long_options_are_offered`.
}

#[test]
fn profile_completion_uses_line_oriented_dynamic_endpoints() {
    let scripts = [
        ShellIntegration::Bash.eager_script(),
        ShellIntegration::Fish.eager_script(),
        ShellIntegration::PowerShell.eager_script(),
        ShellIntegration::Zsh.eager_script(),
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
    let powershell = ShellIntegration::PowerShell.eager_script();

    assert!(powershell.contains(r#"$value -match '\s'"#));
    assert!(powershell.contains(r#""'" + $value.Replace("'", "''")"#));
    assert!(powershell.contains(
        "[System.Management.Automation.CompletionResult]::new($text, $value, 'ParameterValue', $value)"
    ));
}

#[cfg(windows)]
#[test]
fn powershell_accepts_the_generated_integration_syntax() {
    let script = ShellIntegration::PowerShell.eager_script();

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
fn generated_shell_syntax_uses_the_native_powershell_completer_signature() {
    assert!(
        ShellIntegration::PowerShell
            .eager_script()
            .contains("param($wordToComplete, $commandAst, $cursorPosition)")
    );
    assert!(
        ShellIntegration::Zsh
            .eager_script()
            .contains("_zetta_node_zetta__tabicon()")
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
        let script = shell.eager_script();
        match shell {
            ShellIntegration::Fish => {
                assert!(!script.contains("-s r -l resize"));
                assert!(!script.contains("-s c -l columns"));
                assert!(!script.contains("-s R -l rows"));
            }
            _ => assert!(!script.contains("--resize -r")),
        }
        assert!(script.contains("--resize"));
        assert!(script.contains("--columns"));
        assert!(script.contains("--rows"));
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
        let script = shell.eager_script();
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
        let script = shell.eager_script();
        assert!(script.contains("profile"));
        assert!(script.contains("config"));
        assert!(!script.contains("WSL: Ubuntu"));
        assert!(script.contains("configured:profile list"));
        assert!(script.contains("configured:profile themes"));
        assert!(script.contains("profile-report"));
        assert!(script.contains("split"));
        assert!(script.contains("zetta:splits"));
        assert!(script.contains("project"));
        assert!(script.contains("project list"));
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
        let script = shell.eager_script();
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
fn tftp_completion_uses_only_the_local_file_argument_position() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }
    // `get HOST REMOTE [LOCAL]` and `put HOST LOCAL [REMOTE]`: only LOCAL
    // is a file here.
    let script = ShellIntegration::Bash.eager_script();
    let mut driver = script.clone();
    for (case, words) in [
        "ztftp get host Carg",
        "ztftp get host remote Carg",
        "ztftp put host Carg",
        "ztftp put host local Carg",
        "zetta tftp put host Carg",
    ]
    .into_iter()
    .enumerate()
    {
        let words = words.split(' ').collect::<Vec<_>>();
        let entry = format!("_zetta_entry_{}", words[0]);
        let _ = write!(
            driver,
            "\nCOMP_WORDS=({})\nCOMP_CWORD={}\n{entry}\nprintf 'case{case}:%s\\n' \"${{COMPREPLY[@]}}\"\n",
            words.join(" "),
            words.len() - 1,
        );
    }
    let completions = run_bash_driver(bash_completion_command(), &driver);
    let offers_cargo = |case: usize| {
        completions
            .lines()
            .any(|line| line == format!("case{case}:Cargo.toml"))
    };
    assert!(!offers_cargo(0), "{completions}");
    assert!(offers_cargo(1), "{completions}");
    assert!(offers_cargo(2), "{completions}");
    assert!(!offers_cargo(3), "{completions}");
    assert!(offers_cargo(4), "{completions}");
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

#[cfg(not(feature = "clipboard"))]
#[test]
fn generated_integrations_omit_clipboard_shortcuts() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.eager_script();
        assert!(!script.contains("ZETTA_CLIPBOARD_INTEGRATION"));
        assert!(!script.contains("pbcopy"));
        assert!(!script.contains("zcopy"));
        assert!(!script.contains("zpaste"));
    }
}
