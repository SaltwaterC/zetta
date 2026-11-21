use super::*;

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

#[test]
fn native_home_does_not_require_cygpath() {
    let home =
        resolve_windows_posix_shell_home(Some(PathBuf::from(r"D:\homes\alice")), None, |_| {
            panic!("native HOME should not be converted")
        })
        .unwrap();

    assert_eq!(home, PathBuf::from(r"D:\homes\alice"));
}

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

#[test]
fn cygwin_startup_file_links_are_resolved_before_native_file_access() {
    let startup_file = Path::new(r"C:\cygwin64\home\alice\.zshrc");
    let target = PathBuf::from(r"C:\cygwin64\home\alice\.zprezto\runcoms\zshrc");

    let resolved = resolve_cygwin_link_startup_file_with(
        startup_file,
        |_| false,
        |_| true,
        |_| Ok(Some(target.clone())),
    )
    .unwrap();

    assert_eq!(resolved, target);
}

#[test]
fn cygwin_tools_use_the_installation_root_containing_the_startup_file() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("cygwin1.dll"), []).unwrap();
    fs::write(bin.join("cygpath.exe"), []).unwrap();
    fs::write(bin.join("readlink.exe"), []).unwrap();
    let startup_file = root.path().join("home").join("alice").join(".zshrc");

    assert_eq!(
        cygwin_root_for_path(&startup_file),
        Some(root.path().to_path_buf())
    );
    assert_eq!(
        cygwin_tool_for_path(&startup_file, "cygpath.exe"),
        bin.join("cygpath.exe")
    );
    assert_eq!(
        cygwin_tool_for_path(&startup_file, "readlink.exe"),
        bin.join("readlink.exe")
    );
}

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
