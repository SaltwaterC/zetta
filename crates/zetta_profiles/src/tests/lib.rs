use super::*;

use std::io::Write as _;

fn config_with(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(contents.as_bytes()).unwrap();
    (directory, path)
}

#[test]
fn a_configured_profile_replaces_the_discovered_command_of_the_same_name() {
    let (_directory, path) = config_with(
        r#"{ "profiles": [ { "name": "system", "program": "/opt/custom/zsh", "args": ["-l"] } ] }"#,
    );

    let resolved = resolve(&path, "System").expect("the discovered System profile still exists");
    assert_eq!(
        resolved,
        ProfileCommand::with_args("/opt/custom/zsh", vec!["-l".to_owned()]),
        "a configured profile overrides the discovered one, matched without regard to case"
    );
}

#[test]
fn a_profile_the_file_adds_is_resolvable_and_an_unknown_one_is_not() {
    let (_directory, path) =
        config_with(r#"{ "profiles": [ { "name": "Build", "program": "/usr/bin/env" } ] }"#);

    assert_eq!(
        resolve(&path, "build"),
        Some(ProfileCommand::program("/usr/bin/env"))
    );
    assert_eq!(
        resolve(&path, "Nothing Named This"),
        None,
        "an unresolvable name is reported as such, so the caller can fall back to the login shell"
    );
}

#[test]
fn the_system_profile_resolves_to_the_hosts_login_shell() {
    let (_directory, path) = config_with("{}");

    assert_eq!(
        resolve(&path, "System"),
        Some(ProfileCommand::system()),
        "an empty command means the host's login shell, decided where the pane is started"
    );
}

#[test]
fn settings_around_the_profiles_are_ignored_rather_than_rejected() {
    // The window that owns the file parses it strictly and reports what is
    // wrong. This reader runs in a daemon starting a shell for somebody else,
    // and a setting it has never heard of must not stop that.
    let (_directory, path) = config_with(
        r#"{ "theme": "Zetta Dark", "some_future_setting": 12,
             "profiles": [ { "name": "Fish", "program": "/usr/bin/fish", "hidden": true } ] }"#,
    );

    assert_eq!(
        resolve(&path, "Fish"),
        Some(ProfileCommand::program("/usr/bin/fish"))
    );
}

#[test]
fn an_unparseable_file_leaves_the_discovered_profiles_standing() {
    let (_directory, path) = config_with("{ this is not json");

    assert_eq!(
        resolve(&path, "System"),
        Some(ProfileCommand::system()),
        "a broken configuration file must not leave a host unable to open a shell"
    );
}

#[test]
fn a_missing_file_leaves_the_discovered_profiles_standing() {
    let directory = tempfile::tempdir().unwrap();

    assert_eq!(
        resolve(&directory.path().join("absent.json"), "System"),
        Some(ProfileCommand::system())
    );
}

#[test]
fn a_shell_kind_is_read_from_the_programs_basename() {
    assert_eq!(ShellKind::of("/bin/zsh"), ShellKind::Zsh);
    assert_eq!(ShellKind::of("/opt/homebrew/bin/bash"), ShellKind::Bash);
    assert_eq!(
        ShellKind::of(r"C:\msys64\usr\bin\fish.exe"),
        ShellKind::Fish
    );
    assert_eq!(ShellKind::of("pwsh.exe"), ShellKind::PowerShell);
    assert_eq!(ShellKind::of("/usr/bin/nu"), ShellKind::Other);
}

#[test]
fn a_shell_running_a_command_gets_no_shell_integration() {
    assert!(
        shell_integration_startup_command(ShellKind::Bash, &["-c".to_owned(), "ls".to_owned()])
            .is_none(),
        "there is no interactive session to install anything into"
    );
    assert!(shell_integration_startup_command(ShellKind::Other, &[]).is_none());
}

#[cfg(not(windows))]
#[test]
fn an_interactive_posix_shell_is_sent_its_integration() {
    let command = shell_integration_startup_command(ShellKind::Zsh, &[])
        .expect("zsh has a shell integration");
    let command = String::from_utf8(command).unwrap();

    assert!(command.contains("zetta init zsh"));
    assert!(
        command.ends_with('\r'),
        "the line is delivered as if typed, so it has to be entered"
    );
}

#[test]
fn the_terminal_environment_advertises_zettas_own_capabilities() {
    let environment = terminal_environment(TerminalEnvironmentOptions { version: "9.9.9" })
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();

    // Fixed rather than inherited: it describes what Zetta's emulator
    // implements, not what the process that started the pty happens to have.
    // A daemon-started pane inheriting the daemon's empty TERM is what made a
    // shell draw a monochrome prompt beside identical panes that had colour.
    assert_eq!(environment["TERM"], "xterm-256color");
    assert_eq!(environment["COLORTERM"], "truecolor");
    assert_eq!(environment["TERM_PROGRAM"], "zetta");
    assert_eq!(environment["TERM_PROGRAM_VERSION"], "9.9.9");
    assert_eq!(environment["ZETTA_TERM"], "true");
}

#[test]
fn discovery_always_offers_the_system_profile() {
    let profiles = discovered_profiles();

    assert_eq!(
        profiles.first().map(|profile| profile.name.as_str()),
        Some("System"),
        "every host can start its own login shell, whatever else it has"
    );
    assert_eq!(profiles[0].command, ProfileCommand::system());
}
