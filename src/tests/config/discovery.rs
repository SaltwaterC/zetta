//! Mirrors `src/config/discovery.rs`.
//!
//! Every test here builds a fake installation on disk — a Homebrew prefix, an
//! MSYS2 or Cygwin root, a `wsl.exe --list` transcript — and asserts the
//! profiles detected from it, so none of them depends on what the host machine
//! actually has installed.

use super::*;

use std::fs;

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn homebrew_shells_are_profiles_with_their_installed_program_paths() {
    let prefix = tempfile::tempdir().unwrap();
    let bin = prefix.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("brew"), "").unwrap();
    fs::write(bin.join("fish"), "").unwrap();
    fs::write(bin.join("bash"), "").unwrap();

    let profiles = homebrew_shell_profiles([prefix.path().to_path_buf()]);

    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].name, "Bash (Homebrew)");
    assert_eq!(
        profiles[0].command,
        Shell::Program(bin.join("bash").to_string_lossy().into_owned())
    );
    assert_eq!(profiles[1].name, "Fish (Homebrew)");
    assert_eq!(
        profiles[1].command,
        Shell::Program(bin.join("fish").to_string_lossy().into_owned())
    );
    assert_eq!(profiles[0].icon, ProfileIcon::Bash);
    assert_eq!(profiles[1].icon, ProfileIcon::Fish);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn path_resolved_homebrew_shells_match_direct_discovery() {
    let prefix = tempfile::tempdir().unwrap();
    let bin = prefix.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("brew"), "").unwrap();
    fs::write(bin.join("bash"), "").unwrap();
    fs::write(bin.join("fish"), "").unwrap();

    let prefix_path = prefix.path().to_path_buf();
    let direct_profiles = homebrew_shell_profiles([prefix_path.clone()]);
    for program in ["bash", "fish"] {
        let path = command_path_in(program, bin.as_os_str()).unwrap();
        let path_profile =
            homebrew_profile_for_path(&path, std::slice::from_ref(&prefix_path)).unwrap();
        let direct_profile = direct_profiles
            .iter()
            .find(|profile| profile.command == Shell::Program(path.to_string_lossy().into_owned()))
            .cloned()
            .unwrap();

        assert_eq!(path_profile, direct_profile);
    }
}

#[test]
fn parses_utf8_wsl_distribution_names() {
    assert_eq!(
        parse_wsl_distribution_names(b"Ubuntu\r\nDocker-Desktop\r\nDebian\r\nubuntu\r\n\r\n"),
        ["Ubuntu", "Debian"]
    );
}

#[test]
fn parses_utf16_wsl_distribution_names() {
    let output = "Ubuntu-24.04\r\nopenSUSE Tumbleweed\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();

    assert_eq!(
        parse_wsl_distribution_names(&output),
        ["Ubuntu-24.04", "openSUSE Tumbleweed"]
    );
}

#[test]
fn parses_big_endian_utf16_wsl_distribution_names() {
    let mut output = vec![0xfe, 0xff];
    output.extend("Debian\r\n".encode_utf16().flat_map(u16::to_be_bytes));

    assert_eq!(parse_wsl_distribution_names(&output), ["Debian"]);
}

#[test]
fn creates_a_profile_for_each_wsl_distribution() {
    let profiles = wsl_profiles_from_output("wsl.exe", b"Ubuntu\r\nDebian\r\n");

    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].name, "WSL: Ubuntu");
    assert_eq!(profiles[0].icon, ProfileIcon::Tux);
    assert!(matches!(
        profiles[0].command,
        Shell::WithArguments {
            ref program,
            ref args,
            ref title_override,
        } if program == "wsl.exe"
            && args == &["--distribution", "Ubuntu"]
            && title_override.as_deref() == Some("WSL: Ubuntu")
    ));
}

#[test]
fn creates_msys2_profiles_for_installed_shells_using_the_launcher() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("msys2_shell.cmd"), "").unwrap();
    fs::create_dir_all(root.path().join("usr/bin")).unwrap();
    fs::write(root.path().join("usr/bin/bash.exe"), "").unwrap();
    fs::write(root.path().join("usr/bin/zsh.exe"), "").unwrap();

    let profiles = msys2_profiles(root.path());

    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["MSYS2", "MSYS2: Zsh"]
    );
    for (profile, shell) in profiles.iter().zip(["bash", "zsh"]) {
        assert_eq!(
            profile.icon,
            if shell == "bash" {
                ProfileIcon::Bash
            } else {
                ProfileIcon::Zsh
            }
        );
        assert!(matches!(
            profile.command,
            Shell::WithArguments {
                ref program,
                ref args,
                ..
            } if program == "cmd.exe"
                && args[..3] == ["/d", "/s", "/c"]
                && args[3].starts_with("\"\"")
                && args[3].contains("msys2_shell.cmd\" -defterm")
                && args[3].contains("-defterm -here -no-start -msys -use-full-path")
                && args[3].ends_with(&format!("-shell {shell}\""))
        ));
    }
}

#[test]
fn omits_msys2_zsh_profile_when_zsh_is_not_installed() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("usr/bin")).unwrap();
    fs::write(root.path().join("usr/bin/bash.exe"), "").unwrap();

    let profiles = msys2_profiles(root.path());

    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].name, "MSYS2");
}

#[test]
fn creates_cygwin_profiles_for_installed_shells_with_direct_commands() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("bin")).unwrap();
    fs::write(root.path().join("bin/cygwin1.dll"), "").unwrap();
    for shell in ["bash", "zsh", "fish", "nu"] {
        fs::write(root.path().join("bin").join(format!("{shell}.exe")), "").unwrap();
    }

    let profiles = cygwin_profiles(root.path());

    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["Cygwin", "Cygwin: Zsh", "Cygwin: Fish", "Cygwin: Nushell"]
    );
    for (profile, (shell, icon)) in profiles.iter().zip([
        ("bash", ProfileIcon::Bash),
        ("zsh", ProfileIcon::Zsh),
        ("fish", ProfileIcon::Fish),
        ("nu", ProfileIcon::Zetta),
    ]) {
        assert_eq!(profile.icon, icon);
        assert!(matches!(
            &profile.command,
            Shell::WithArguments {
                program,
                args,
                title_override,
            } if program == &root.path().join("bin").join(format!("{shell}.exe")).display().to_string()
                && args == &["-l"]
                && title_override.as_deref() == Some(profile.name.as_str())
        ));
    }
}

#[test]
fn omits_cygwin_profiles_for_missing_shell_executables() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("bin")).unwrap();
    fs::write(root.path().join("bin/cygwin1.dll"), "").unwrap();
    fs::write(root.path().join("bin/bash.exe"), "").unwrap();
    fs::write(root.path().join("bin/nu.exe"), "").unwrap();

    let profiles = cygwin_profiles(root.path());

    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["Cygwin", "Cygwin: Nushell"]
    );
}

#[test]
fn skips_incomplete_cygwin_roots_when_a_later_root_has_shells() {
    let incomplete = tempfile::tempdir().unwrap();
    fs::create_dir_all(incomplete.path().join("bin")).unwrap();
    fs::write(incomplete.path().join("bin/cygwin1.dll"), "").unwrap();

    let complete = tempfile::tempdir().unwrap();
    fs::create_dir_all(complete.path().join("bin")).unwrap();
    fs::write(complete.path().join("bin/cygwin1.dll"), "").unwrap();
    fs::write(complete.path().join("bin/bash.exe"), "").unwrap();

    assert_eq!(
        select_cygwin_installation_root([
            incomplete.path().to_path_buf(),
            complete.path().to_path_buf(),
        ]),
        Some(complete.path().to_path_buf())
    );
}

#[test]
fn normalizes_cygwin_registry_installation_paths() {
    assert_eq!(
        normalize_cygwin_registry_path(r"\??\C:\cygwin64"),
        Some(PathBuf::from(r"C:\cygwin64"))
    );
    assert_eq!(
        normalize_cygwin_registry_path(r"\\??\D:\Tools\cygwin"),
        Some(PathBuf::from(r"D:\Tools\cygwin"))
    );
    assert_eq!(
        normalize_cygwin_registry_path(r"\??\UNC\server\share\cygwin"),
        Some(PathBuf::from(r"\\server\share\cygwin"))
    );
    assert_eq!(normalize_cygwin_registry_path("relative\npath"), None);
}

#[cfg(windows)]
#[test]
fn msys2_launcher_command_supports_custom_paths_with_spaces() {
    use std::os::windows::process::CommandExt as _;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("custom MSYS2 installation");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::write(
        root.join("msys2_shell.cmd"),
        "@echo off\r\nif \"%7\"==\"zsh\" exit /b 0\r\nexit /b 1\r\n",
    )
    .unwrap();
    fs::write(root.join("usr/bin/zsh.exe"), "").unwrap();
    let profile = msys2_profiles(&root).pop().unwrap();
    let Shell::WithArguments { program, args, .. } = profile.command else {
        panic!("MSYS2 profile did not include launcher arguments");
    };

    let status = Command::new(program)
        .raw_arg(args.join(" "))
        .status()
        .unwrap();

    assert!(status.success());
}

#[cfg(windows)]
#[test]
fn reads_custom_msys2_root_from_an_installer_shortcut() {
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, IPersistFile,
            },
            UI::Shell::{IShellLinkW, ShellLink},
        },
        core::{HSTRING, Interface},
    };

    let temporary = tempfile::tempdir().unwrap();
    let shortcut = temporary.path().join("MSYS2 MSYS.lnk");
    let root = temporary.path().join("custom MSYS2 installation");
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(&HSTRING::from(root.join("msys2.exe").as_os_str()))
                .unwrap();
            link.SetWorkingDirectory(&HSTRING::from(root.as_os_str()))
                .unwrap();
            let persist: IPersistFile = link.cast().unwrap();
            persist
                .Save(&HSTRING::from(shortcut.as_os_str()), true)
                .unwrap();

            assert_eq!(shortcut_working_directory(&shortcut), Some(root));
        }
        CoUninitialize();
    }
}
