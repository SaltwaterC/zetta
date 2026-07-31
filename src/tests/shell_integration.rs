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
                "terminal-size sessions init serial http tftp --help --version --config --keymap --profile'"
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
