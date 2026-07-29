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
