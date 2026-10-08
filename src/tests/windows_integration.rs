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

#[cfg(windows)]
#[test]
fn jump_list_profile_actions_open_fresh_profile_windows() {
    assert_eq!(
        profile_jump_list_arguments("PowerShell"),
        "--new-window --profile PowerShell"
    );
    assert_eq!(
        profile_jump_list_arguments("WSL: Ubuntu"),
        r#"--new-window --profile "WSL: Ubuntu""#
    );
}

#[cfg(windows)]
#[test]
fn jump_list_omits_hidden_profiles() {
    let profile = |name: &str| Profile {
        name: name.to_owned(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: crate::profile_icon::ProfileIcon::Zetta,
    };
    let profiles = [profile("Visible"), profile("Hidden")];
    let hidden_profiles = std::collections::HashSet::from(["hidden".to_owned()]);
    let names = visible_profile_jump_list_profiles(&profiles, &hidden_profiles)
        .map(|profile| profile.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["Visible"]);
}

#[cfg(windows)]
#[test]
fn handoff_pipe_names_are_random() {
    let first = handoff_pipe_name().unwrap();
    let second = handoff_pipe_name().unwrap();
    assert_ne!(first, second);
    for name in [&first, &second] {
        let nonce = name
            .strip_prefix(r"\\.\pipe\ZettaTerminalHandoff-")
            .expect("the handoff pipe keeps its prefix");
        assert_eq!(nonce.len(), 32);
        assert!(nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}

#[cfg(windows)]
#[test]
fn handoff_pipe_refuses_a_name_another_creator_already_holds() {
    use windows::Win32::{Foundation::CloseHandle, System::Pipes::PIPE_UNLIMITED_INSTANCES};

    let name = handoff_pipe_name().unwrap();
    // A squatter's instance, made the way any other account could: default
    // security and room for more instances, which a later creator would join.
    let squatter = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(name.as_str()),
            PIPE_ACCESS_DUPLEX,
            NAMED_PIPE_MODE(PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0),
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            None,
        )
    };
    assert!(!squatter.is_invalid());
    let result = create_handoff_pipe_named(&name);
    unsafe {
        let _ = CloseHandle(squatter);
    }
    assert!(
        result.is_err(),
        "the handoff pipe must not join an instance somebody else created"
    );
}

#[cfg(windows)]
#[test]
fn handoff_pipe_admits_only_this_account_and_the_client_allows_identification_only() {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{
            ACL,
            Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT},
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, PSECURITY_DESCRIPTOR,
            SE_DACL_PROTECTED,
        },
    };

    let flags = handoff_client_flags();
    assert_eq!(flags.0 & SECURITY_SQOS_PRESENT.0, SECURITY_SQOS_PRESENT.0);
    assert_eq!(
        flags.0 & SECURITY_IDENTIFICATION.0,
        SECURITY_IDENTIFICATION.0
    );
    assert_eq!(flags.0 & FILE_FLAG_OVERLAPPED.0, FILE_FLAG_OVERLAPPED.0);

    let (server, _client) = create_handoff_pipe().unwrap();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let status = unsafe {
        GetSecurityInfo(
            HANDLE(server.as_raw_handle()),
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor),
        )
    };
    assert!(
        status.is_ok(),
        "reading the handoff pipe's DACL: {status:?}"
    );
    let mut control = 0_u16;
    let mut revision = 0_u32;
    let (ace_count, controlled) = unsafe {
        let controlled = GetSecurityDescriptorControl(descriptor, &mut control, &mut revision);
        let ace_count = (!dacl.is_null()).then(|| (*dacl).AceCount);
        (ace_count, controlled)
    };
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    controlled.unwrap();
    // This account and SYSTEM, and nothing inherited from a default DACL.
    assert_eq!(ace_count, Some(2));
    assert_ne!(control & SE_DACL_PROTECTED.0, 0);
}
