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
        assert!(script.contains("init"));
    }
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
                "terminal-size sessions init tftp --help --version --config --keymap --profile'"
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
