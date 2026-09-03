use super::*;

#[test]
fn parses_profile_icon_values() {
    assert_eq!(ProfileIcon::parse(&Value::Null).unwrap(), None);
    assert_eq!(
        ProfileIcon::parse(&Value::String("auto".into())).unwrap(),
        None
    );
    assert_eq!(
        ProfileIcon::parse(&Value::String("BASH".into())).unwrap(),
        Some(ProfileIcon::Bash)
    );
    assert!(ProfileIcon::parse(&Value::String("terminal".into())).is_err());
}

#[test]
fn maps_shell_executable_basenames() {
    assert_eq!(
        ProfileIcon::automatic_for_program_name("/opt/homebrew/bin/bash"),
        ProfileIcon::Bash
    );
    assert_eq!(
        ProfileIcon::automatic_for_program_name(r"C:\\tools\\zsh.exe"),
        ProfileIcon::Zsh
    );
    assert_eq!(
        ProfileIcon::automatic_for_program_name("custom-shell"),
        ProfileIcon::Zetta
    );
}

#[test]
fn automatic_programs_fall_back_to_bundled_shell_icons() {
    assert_eq!(
        ProfileIcon::automatic_for_program(r"C:\missing\fish.exe"),
        ProfileIcon::Fish
    );
    assert_eq!(
        ProfileIcon::automatic_for_program("/missing/zsh"),
        ProfileIcon::Zsh
    );
}

#[test]
fn special_profiles_use_their_platform_icon() {
    assert_eq!(
        ProfileIcon::automatic_for_profile("WSL: Ubuntu", &Shell::Program("wsl.exe".into())),
        ProfileIcon::Tux
    );
    assert_eq!(
        ProfileIcon::automatic_for_profile("WSL: Ubuntu (Zsh)", &Shell::Program("wsl.exe".into()),),
        ProfileIcon::Tux
    );
    assert_eq!(
        ProfileIcon::automatic_for_profile(
            "MSYS2: Zsh",
            &Shell::WithArguments {
                program: "cmd.exe".into(),
                args: Vec::new(),
                title_override: None,
            },
        ),
        ProfileIcon::Zsh
    );
    assert_eq!(
        ProfileIcon::automatic_for_profile(
            "Cygwin",
            &Shell::Program(r"C:\cygwin64\bin\bash.exe".into()),
        ),
        ProfileIcon::Bash
    );
    assert_eq!(
        ProfileIcon::automatic_for_profile(
            "Cygwin: Fish",
            &Shell::Program(r"C:\cygwin64\bin\fish.exe".into()),
        ),
        ProfileIcon::Fish
    );
    assert_eq!(
        ProfileIcon::automatic_for_profile(
            "Cygwin: Nushell",
            &Shell::Program(r"C:\cygwin64\bin\nu.exe".into()),
        ),
        ProfileIcon::Zetta
    );
}
