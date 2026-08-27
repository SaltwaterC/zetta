#![cfg(any(target_os = "linux", target_os = "macos"))]

use super::*;

#[cfg(target_os = "linux")]
#[test]
fn detects_supported_desktops_from_session_values() {
    assert_eq!(
        desktop_environment_from_values(Some("GNOME:GNOME-Classic"), None, false),
        Some(DesktopEnvironment::Gnome)
    );
    assert_eq!(
        desktop_environment_from_values(Some("KDE"), None, false),
        Some(DesktopEnvironment::Kde)
    );
    assert_eq!(
        desktop_environment_from_values(Some("X-Cinnamon"), None, false),
        Some(DesktopEnvironment::Cinnamon)
    );
    assert_eq!(desktop_environment_from_values(None, None, false), None);
}

#[cfg(target_os = "linux")]
#[test]
fn selects_the_xdg_terminal_config_for_the_active_desktop() {
    assert_eq!(
        xdg_terminal_config_filename(Some("GNOME:GNOME-Classic")),
        "gnome-xdg-terminals.list"
    );
    assert_eq!(
        xdg_terminal_config_filename(Some("Ubuntu:GNOME")),
        "ubuntu-xdg-terminals.list"
    );
    assert_eq!(xdg_terminal_config_filename(None), "xdg-terminals.list");
    assert_eq!(
        xdg_terminal_config_filename(Some("../unsafe")),
        "xdg-terminals.list"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn declares_zetta_as_an_xdg_terminal_emulator() {
    let desktop_entry = include_str!("../../resources/linux/Zetta.desktop");
    assert!(
        desktop_entry
            .lines()
            .any(|line| line == "X-TerminalArgExec=-e")
    );
    assert!(
        desktop_entry
            .lines()
            .any(|line| line == "Categories=System;TerminalEmulator;")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn makes_zetta_the_first_xdg_terminal_without_removing_other_entries() {
    let original =
        "# User terminal order\r\nother.desktop\r\nZetta.desktop:old-action\r\n-Zetta.desktop\r\n";
    assert_eq!(
        update_xdg_terminal_list(original),
        "Zetta.desktop\r\n# User terminal order\r\nother.desktop\r\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn recognizes_zetta_as_the_selected_xdg_terminal_only_when_first() {
    assert!(xdg_terminal_list_prefers_zetta(
        "# comment\n+Zetta.desktop:default\nother.desktop\n"
    ));
    assert!(!xdg_terminal_list_prefers_zetta(
        "other.desktop\nZetta.desktop\n"
    ));
    assert!(!xdg_terminal_list_prefers_zetta(
        "-Zetta.desktop\nother.desktop\n"
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn uses_the_desktop_specific_gsettings_schema() {
    assert_eq!(
        gsettings_terminal_schema(DesktopEnvironment::Gnome),
        Some("org.gnome.desktop.default-applications.terminal")
    );
    assert_eq!(
        gsettings_terminal_schema(DesktopEnvironment::Cinnamon),
        Some("org.cinnamon.desktop.default-applications.terminal")
    );
    assert_eq!(
        gsettings_terminal_schema(DesktopEnvironment::Mate),
        Some("org.mate.applications-terminal")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn updates_only_the_requested_ini_keys() {
    let original = "[General]\nTerminalApplication=konsole\nKeep=me\n\n[Other]\nValue=1";
    let updated = update_ini_section(
        original,
        "General",
        &[
            ("TerminalApplication", "/opt/zetta"),
            ("TerminalService", "Zetta.desktop"),
        ],
    );
    assert!(updated.contains("TerminalApplication=/opt/zetta"));
    assert!(updated.contains("TerminalService=Zetta.desktop"));
    assert!(updated.contains("Keep=me"));
    assert!(updated.contains("[Other]\nValue=1"));
}

#[cfg(target_os = "linux")]
#[test]
fn preserves_ini_line_endings_and_replaces_duplicate_keys() {
    let original =
        "[General]\r\nTerminalApplication=old\r\nTerminalApplication=stale\r\nKeep=me\r\n";
    let updated = update_ini_section(
        original,
        "General",
        &[
            ("TerminalApplication", "/opt/zetta"),
            ("TerminalService", "Zetta.desktop"),
        ],
    );
    assert_eq!(
        updated,
        "[General]\r\nTerminalApplication=/opt/zetta\r\nTerminalApplication=/opt/zetta\r\nKeep=me\r\nTerminalService=Zetta.desktop\r\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn updates_xfce_terminal_without_touching_other_helpers() {
    let updated = update_xfce_helpers("WebBrowser=firefox\nTerminalEmulator=old", "/opt/zetta");
    assert_eq!(updated, "WebBrowser=firefox\nTerminalEmulator=/opt/zetta");
}

#[cfg(target_os = "linux")]
#[test]
fn preserves_xfce_line_endings_when_adding_the_terminal_helper() {
    let updated = update_xfce_helpers("WebBrowser=firefox\r\n", "/opt/zetta");
    assert_eq!(
        updated,
        "WebBrowser=firefox\r\nTerminalEmulator=/opt/zetta\r\n"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn accepts_only_registered_script_extensions() {
    assert!(accepted_script_extension(Path::new("build.command")));
    assert!(accepted_script_extension(Path::new("script.ZSH")));
    assert!(accepted_script_extension(Path::new("notes.sh")));
    assert!(!accepted_script_extension(Path::new("notes.txt")));
}

#[cfg(target_os = "macos")]
#[test]
fn filters_file_urls_and_preserves_multiple_files() {
    let paths = script_paths_from_urls(&[
        "file:///tmp/a.command".to_owned(),
        "https://example.com/b.command".to_owned(),
        "file:///tmp/notes.txt".to_owned(),
        "file:///tmp/b.tool".to_owned(),
    ]);
    assert_eq!(
        paths,
        [
            PathBuf::from("/tmp/a.command"),
            PathBuf::from("/tmp/b.tool")
        ]
    );
}
