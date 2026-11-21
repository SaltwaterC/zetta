//! Fish's completion: the candidates it displays, options it does not repeat,
//! and what a shell without a local multiplexer leaves out.

use super::*;

#[cfg(feature = "zmux")]
#[test]
fn fish_displays_long_option_candidates_and_supports_short_option_values() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let script = ShellIntegration::Fish.eager_script();
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
        ("zetta cmd ", &["build", "check"][..]),
        ("zetta cmd -", &["--list", "--help", "--"][..]),
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
        (
            "zetta tabicon -",
            &["--icon", "--reset", "--queue", "--list", "--help"][..],
        ),
        ("zetta tabicon --reset ", &[][..]),
        ("zetta tabicon -r ", &[][..]),
        ("zetta tabicon -i ", &[][..]),
        ("zetta theme ", &["pane", "tab"][..]),
        ("zetta theme pane ", &["Gruvbox Light Hard"][..]),
        (
            "zetta theme pane -",
            &["--theme", "--reset", "--list", "--help"][..],
        ),
        ("zetta theme pane -t ", &["Gruvbox Light Hard"][..]),
        ("zetta theme tab ", &["Gruvbox Light Hard"][..]),
        (
            "zetta theme tab -",
            &["--theme", "--reset", "--list", "--help"][..],
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
        ("zetta vi ", &["Cargo.toml"][..]),
        ("zetta vi -", &["--help"][..]),
        ("zetta vi Carg", &["Cargo.toml"][..]),
        // A shell is an operand worth offering, so options wait for a `-`.
        ("zetta init -", &["--completions", "--help"][..]),
        ("zetta init fish -", &["--completions", "--help"][..]),
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
            &[
                "--root",
                "--port",
                "--config",
                "--serve-dotfiles",
                "--allow-host",
                "--allow-any-host",
                "--help",
            ][..],
        ),
        ("zetta tftp ", &["--help"][..]),
        ("zetta tftp get ", &["--port", "--help"][..]),
        #[cfg(tftp_setcap)]
        ("zetta tftp ", &["setcap"][..]),
        #[cfg(tftp_setcap)]
        ("zetta tftp setcap ", &["--check", "--help"][..]),
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
        ("ztftp ", &["--help"][..]),
        ("ztftp get ", &["--port", "--help"][..]),
        (
            "zhttp server ",
            &[
                "--root",
                "--port",
                "--serve-dotfiles",
                "--allow-host",
                "--allow-any-host",
                "--help",
            ][..],
        ),
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

#[cfg(feature = "zmux")]
#[test]
fn fish_omits_daemon_only_mux_candidates_in_no_mux_shells() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let script = ShellIntegration::Fish.eager_script();
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

    let script = ShellIntegration::Fish.eager_script();
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

    let script = ShellIntegration::Fish.eager_script();
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
