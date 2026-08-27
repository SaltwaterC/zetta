use super::*;

#[test]
fn pane_directions_map_to_split_geometry() {
    assert_eq!(
        pane_direction_split(PaneDirection::Left),
        (SplitAxis::Vertical, SplitPosition::Before)
    );
    assert_eq!(
        pane_direction_split(PaneDirection::Right),
        (SplitAxis::Vertical, SplitPosition::After)
    );
    assert_eq!(
        pane_direction_split(PaneDirection::Up),
        (SplitAxis::Horizontal, SplitPosition::Before)
    );
    assert_eq!(
        pane_direction_split(PaneDirection::Down),
        (SplitAxis::Horizontal, SplitPosition::After)
    );
}

#[test]
fn pane_command_size_counts_separators() {
    assert_eq!(
        pane_command_byte_len(&["echo".to_owned(), "hello world".to_owned()]),
        16
    );
}

#[test]
fn pane_commands_are_quoted_as_shell_arguments() {
    let shell = Shell::Program("bash".to_owned());
    let command = quote_pane_command_for_shell(
        &shell,
        &[
            "printf".to_owned(),
            "%s %s".to_owned(),
            "hello world".to_owned(),
        ],
    )
    .unwrap();
    assert_eq!(command, "printf '%s %s' 'hello world'");
}

#[test]
fn pane_command_quoting_keeps_shell_metacharacters_literal() {
    let arguments = vec![
        "echo".to_owned(),
        "$HOME".to_owned(),
        "a;b".to_owned(),
        "$(touch marker)".to_owned(),
    ];
    let posix =
        quote_pane_command_for_shell(&Shell::Program("bash".to_owned()), &arguments).unwrap();
    assert!(posix.contains("'$HOME'"));
    assert!(posix.contains("'a;b'"));
    assert!(posix.contains("'$(touch marker)'"));

    let powershell =
        quote_pane_command_for_shell(&Shell::Program("pwsh".to_owned()), &arguments).unwrap();
    assert!(powershell.starts_with("echo "));
    assert!(powershell.contains("'$HOME'"));
    assert!(powershell.contains("'a;b'"));
    assert!(powershell.contains("'$(touch marker)'"));
}

#[test]
fn new_split_overlays_resolve_text_style_and_color() {
    let overlay = resolve_pane_overlay(Some(PaneOverlayRequest {
        text: Some("API".to_owned()),
        font_size: Some(OverlayFontSize::Large),
        opacity: Some(70),
        color: Some("cyan".to_owned()),
    }))
    .unwrap()
    .unwrap();
    assert_eq!(overlay.text.as_deref(), Some("API"));
    assert_eq!(overlay.font_size, Some(OverlayFontSize::Large));
    assert_eq!(overlay.opacity, Some(0.7));
    assert_eq!(overlay.color, overlay_color_from_value("cyan"));
    assert!(
        resolve_pane_overlay(Some(PaneOverlayRequest {
            text: Some("API".to_owned()),
            font_size: None,
            opacity: None,
            color: Some("not-a-color".to_owned()),
        }))
        .is_err()
    );
}

#[test]
fn split_commands_use_exact_program_arguments() {
    assert_eq!(
        exact_pane_command_shell(
            &Shell::Program("bash".to_owned()),
            &["npm".to_owned(), "run dev".to_owned()],
            None,
        )
        .unwrap(),
        Shell::WithArguments {
            program: "npm".to_owned(),
            args: vec!["run dev".to_owned()],
            title_override: None,
        }
    );
}

#[test]
fn wsl_split_commands_preserve_the_launcher_and_working_directory() {
    let shell = exact_pane_command_shell(
        &Shell::Program("wsl.exe".to_owned()),
        &["cargo".to_owned(), "test".to_owned()],
        Some("/work/project"),
    )
    .unwrap();
    let Shell::WithArguments { program, args, .. } = shell else {
        panic!("expected a WSL launcher");
    };
    assert_eq!(program, "wsl.exe");
    assert_eq!(
        args,
        vec![
            "--cd".to_owned(),
            "/work/project".to_owned(),
            "--exec".to_owned(),
            "cargo".to_owned(),
            "test".to_owned(),
        ]
    );
}

#[cfg(windows)]
fn cygwin_profile(shell: &str, title: &str) -> Shell {
    Shell::WithArguments {
        program: format!(r"C:\cygwin64\bin\{shell}.exe"),
        args: vec!["-l".to_owned()],
        title_override: Some(title.to_owned()),
    }
}

#[cfg(windows)]
#[test]
fn cygwin_split_commands_use_the_direct_profile_shell() {
    let profile = cygwin_profile("bash", "Cygwin");
    let shell = exact_pane_command_shell(
        &profile,
        &["printf".to_owned(), "hello world".to_owned()],
        None,
    )
    .unwrap();

    assert_eq!(
        shell,
        Shell::WithArguments {
            program: r"C:\cygwin64\bin\bash.exe".to_owned(),
            args: vec![
                "-l".to_owned(),
                "-i".to_owned(),
                "-c".to_owned(),
                "exec printf 'hello world'".to_owned(),
            ],
            title_override: None,
        }
    );
}

#[cfg(windows)]
#[test]
fn cygwin_split_commands_use_shell_specific_quoting() {
    let command = [
        "echo".to_owned(),
        "hello world".to_owned(),
        "$HOME".to_owned(),
    ];
    let bash = quote_pane_command_for_shell(&cygwin_profile("bash", "Cygwin"), &command).unwrap();
    let fish =
        quote_pane_command_for_shell(&cygwin_profile("fish", "Cygwin: Fish"), &command).unwrap();
    let nu =
        quote_pane_command_for_shell(&cygwin_profile("nu", "Cygwin: Nushell"), &command).unwrap();

    assert_eq!(bash, "echo 'hello world' '$HOME'");
    assert_eq!(fish, "echo 'hello world' '$HOME'");
    assert_eq!(nu, "echo 'hello world' '$HOME'");
}
