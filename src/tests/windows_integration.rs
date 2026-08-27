use super::*;

#[test]
fn quotes_profile_names_for_windows_command_lines() {
    assert_eq!(quote_windows_argument("PowerShell"), "PowerShell");
    assert_eq!(quote_windows_argument("WSL: Ubuntu"), r#""WSL: Ubuntu""#);
    assert_eq!(
        quote_windows_argument(r#"A "quoted" profile"#),
        r#""A \"quoted\" profile""#
    );
    assert_eq!(quote_windows_argument(r#"A\"B"#), r#""A\\\"B""#);
    assert_eq!(
        quote_windows_argument(r"Trailing slash\"),
        r#""Trailing slash\\""#
    );
}

#[cfg(windows)]
#[test]
fn terminal_handoff_registration_uses_a_stable_clsid_and_quoted_server() {
    assert_eq!(
        ZETTA_TERMINAL_HANDOFF_CLSID,
        GUID::from_u128(0x7f6f0d2e_0b8c_4a36_9c71_42b3c6d89e10)
    );
    let executable = Path::new(r"C:\Program Files\Zetta\zetta-gui.exe");
    let command = local_server_command(executable);
    assert_eq!(
        command,
        r#""C:\Program Files\Zetta\zetta-gui.exe" -Embedding"#
    );
    assert_eq!(
        server_path_from_command(&command),
        Some(executable.to_path_buf())
    );
}

#[cfg(windows)]
#[test]
fn jump_list_icons_use_embedded_resources_or_executable_icons() {
    use std::path::PathBuf;

    use crate::profile_icon::ProfileIcon;

    let target = PathBuf::from(r"C:\Program Files\Zetta\zetta-gui.exe");
    assert_eq!(
        ProfileIcon::Zetta.jump_list_icon_location(&target),
        (target.clone(), 1)
    );
    assert_eq!(
        ProfileIcon::Tux.jump_list_icon_location(&target),
        (target.clone(), 5)
    );
    assert_eq!(
        ProfileIcon::Bash.jump_list_icon_location(&target),
        (target.clone(), 2)
    );
    assert_eq!(
        ProfileIcon::Zsh.jump_list_icon_location(&target),
        (target.clone(), 3)
    );
    assert_eq!(
        ProfileIcon::Fish.jump_list_icon_location(&target),
        (target.clone(), 4)
    );

    let executable = tempfile::NamedTempFile::new().unwrap();
    let executable_path = executable.path().to_path_buf();
    assert_eq!(
        ProfileIcon::Executable(executable_path.clone()).jump_list_icon_location(&target),
        (executable_path, 0)
    );
}
