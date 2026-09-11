//! Mirrors `src/config/discovery.rs`.
//!
//! Detection itself is tested in `zetta_profiles`, which owns it. What is left
//! here is the half the application adds back: the icon a detected profile is
//! drawn with, and the round trip between a [`Shell`] and the plain command
//! that crosses a machine boundary.

use super::*;

/// The icons the detection used to choose for itself, before it moved to a
/// crate that has never heard of an icon. Deriving them from the name and
/// command has to produce the same answers, or a shared profile set would come
/// back drawn differently.
#[test]
fn a_detected_profiles_icon_is_derived_from_its_name_and_command() {
    let cases: &[(&str, Shell, ProfileIcon)] = &[
        (
            "WSL: Ubuntu",
            Shell::WithArguments {
                program: "wsl.exe".to_owned(),
                args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
                title_override: Some("WSL: Ubuntu".to_owned()),
            },
            ProfileIcon::Tux,
        ),
        (
            "MSYS2",
            Shell::WithArguments {
                program: "cmd.exe".to_owned(),
                args: vec!["/c".to_owned()],
                title_override: None,
            },
            ProfileIcon::Bash,
        ),
        (
            "MSYS2: Zsh",
            Shell::WithArguments {
                program: "cmd.exe".to_owned(),
                args: vec!["/c".to_owned()],
                title_override: None,
            },
            ProfileIcon::Zsh,
        ),
        (
            "Cygwin",
            Shell::Program(r"C:\cygwin64\bin\bash.exe".to_owned()),
            ProfileIcon::Bash,
        ),
        (
            "Cygwin: Zsh",
            Shell::Program(r"C:\cygwin64\bin\zsh.exe".to_owned()),
            ProfileIcon::Zsh,
        ),
        (
            "Cygwin: Fish",
            Shell::Program(r"C:\cygwin64\bin\fish.exe".to_owned()),
            ProfileIcon::Fish,
        ),
        (
            "Cygwin: Nushell",
            Shell::Program(r"C:\cygwin64\bin\nu.exe".to_owned()),
            ProfileIcon::Zetta,
        ),
    ];

    for (name, command, expected) in cases {
        assert_eq!(
            ProfileIcon::automatic_for_profile(name, command),
            *expected,
            "icon for {name}"
        );
    }
}

#[cfg(feature = "zmux")]
#[test]
fn a_command_survives_the_round_trip_through_a_machine_boundary() {
    let cases = [
        Shell::System,
        Shell::Program("/opt/homebrew/bin/zsh".to_owned()),
        Shell::WithArguments {
            program: "wsl.exe".to_owned(),
            args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
            title_override: Some("WSL: Ubuntu".to_owned()),
        },
    ];

    for shell in cases {
        let name = match &shell {
            Shell::WithArguments { title_override, .. } => title_override.clone().unwrap(),
            _ => "System".to_owned(),
        };
        assert_eq!(profile_shell(&name, profile_command(&shell)), shell);
    }
}

/// A profile with arguments keeps its name as the title override. A pane's
/// title comes from it, so losing it on the way back renames every pane the
/// profile opens.
#[test]
fn arguments_carry_the_profile_name_as_the_title_override() {
    let shell = profile_shell(
        "Cygwin: Zsh",
        zetta_profiles::ProfileCommand::with_args(
            r"C:\cygwin64\bin\zsh.exe".to_owned(),
            vec!["-l".to_owned()],
        ),
    );

    assert!(matches!(
        shell,
        Shell::WithArguments { title_override, .. } if title_override.as_deref() == Some("Cygwin: Zsh")
    ));
}
