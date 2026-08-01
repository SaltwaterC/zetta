use super::*;

fn profiles() -> [Profile; 2] {
    [
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
        },
        Profile {
            name: "WSL: Ubuntu".to_owned(),
            command: Shell::Program("zsh".to_owned()),
            theme: None,
        },
    ]
}

#[test]
fn supported_shells_generate_completion_and_tftp_shortcut() {
    let profiles = profiles();
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

#[test]
fn sound_completion_calls_a_shared_helper_from_every_call_site() {
    let profiles = profiles();

    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(bash.contains("--sound)\n            _zetta_complete_sound_names"));
    assert!(bash.contains("--sound|-s)\n            _zetta_complete_sound_names"));
    assert!(bash.contains(
        "elif [[ $command == notify ]]; then\n                _zetta_complete_sound_names"
    ));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(zsh.contains("--sound)\n            _zetta_sound_names"));
    assert!(zsh.contains("--sound|-s)\n            _zetta_sound_names"));
    assert!(
        zsh.contains("elif [[ $words[2] == notify ]]; then\n                _zetta_sound_names")
    );

    let fish = ShellIntegration::Fish.script(&profiles);
    assert!(fish.contains("-l sound -r -a '(__zetta_sound_names)'"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
    assert!(powershell.contains("elseif ($previous -in '--sound', '-s') { $zettaSoundNames }"));
    assert!(powershell.contains("elseif ($previous -eq '--sound') {\n        $zettaSoundNames"));
    assert!(powershell.contains("elseif ($subcommand -eq 'notify') { $zettaSoundNames }"));
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
// benchmark-output's --output-type. Completion after -t/--output-type must
// stay scoped to the active subcommand instead of always suggesting
// benchmark-output's repeated/unique values.
#[test]
fn notify_timeout_completion_does_not_leak_into_other_short_t_flags() {
    let profiles = profiles();

    let bash = ShellIntegration::Bash.script(&profiles);
    assert!(bash.contains(
        "--output-type|-t)\n            if [[ $command == notify ]]; then\n                COMPREPLY=( $(compgen -W 'default never'"
    ));
    assert!(bash.contains("COMPREPLY=( $(compgen -W 'repeated unique'"));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(zsh.contains(
        "--output-type|-t)\n            if [[ $words[2] == notify ]]; then\n                compadd -- default never"
    ));
    assert!(zsh.contains("compadd -- repeated unique"));

    let powershell = ShellIntegration::PowerShell.script(&profiles);
    assert!(powershell.contains(
        "elseif ($previous -in '--output-type', '-t') {\n        if ($subcommand -eq 'notify') { 'default', 'never' } else { 'repeated', 'unique' }"
    ));
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
    assert!(bash.contains("if [[ $command == serial ]]; then\n                COMPREPLY=( $(compgen -W 'none odd even'"));
    assert!(bash.contains(
        "if [[ $command == http || ( $command == tftp && ${COMP_WORDS[2]} == server ) ]]; then"
    ));

    let fish = ShellIntegration::Fish.script(&profiles);
    assert!(fish.contains("-l device"));
    assert!(fish.contains("-l data-bits"));
    assert!(fish.contains("-l parity"));
    assert!(
        fish.contains("subcommand_from tftp; and __fish_seen_subcommand_from server' -l config")
    );

    let powershell = ShellIntegration::PowerShell.script(&profiles);
    assert!(powershell.contains("'--device', '-d'"));
    assert!(powershell.contains("'--data-bits', '-D'"));
    assert!(powershell.contains("$previous -eq '-p' -and $subcommand -eq 'serial'"));
    assert!(powershell.contains("{ '--root', '--port', '--config', '--help' }"));

    let zsh = ShellIntegration::Zsh.script(&profiles);
    assert!(zsh.contains("--data-bits|-D)"));
    assert!(zsh.contains("$words[2] == serial"));
    assert!(zsh.contains("compadd -- --root --port --config --help"));
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
        "compadd -- --device --baud-rate --data-bits --parity --stop-bits --flow-control --help"
    ));
    assert!(zsh.contains("compadd -- --root --port --config --help"));

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
    assert!(zsh.contains("compadd -- --app-name --icon --sound --timeout --help"));
    assert!(!powershell.contains("'-a', '--app-name'"));
    assert!(powershell.contains("'--app-name', '--icon', '--sound', '--timeout', '--help'"));
    assert!(!fish.contains("-s a -l app-name"));
    assert!(fish.contains("subcommand_from notify' -l app-name"));
}

#[test]
fn powershell_profiles_are_comma_separated() {
    assert_eq!(
        render_profiles(ShellIntegration::PowerShell, &profiles()),
        "'System', 'WSL: Ubuntu'"
    );
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
        assert!(script.contains("WSL: Ubuntu"));
        assert!(script.contains("profile-report"));
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
            ShellIntegration::Bash => assert!(script.contains(
                "terminal-size sessions init serial http tftp notify copy paste --help --version --config --keymap --profile'"
            )),
            ShellIntegration::Fish => {
                assert!(script.contains("-l profile -r"));
                assert!(!script.contains("-s p -l profile"));
            }
            ShellIntegration::PowerShell => assert!(
                script.contains("'--help', '--version', '--config', '--keymap', '--profile'")
            ),
            ShellIntegration::Zsh => {
                assert!(script.contains("compadd -- --help --version --config --keymap --profile"))
            }
        }
    }
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
