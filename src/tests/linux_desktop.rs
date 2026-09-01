use super::*;
use crate::profile_icon::ProfileIcon;
use task::Shell;

fn profile(name: &str) -> Profile {
    Profile {
        name: name.to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    }
}

#[test]
fn renders_visible_profile_actions_and_preserves_new_window() {
    let profiles = vec![
        profile("PowerShell"),
        profile("WSL: Ubuntu"),
        profile("Hidden"),
    ];
    let hidden_profiles = HashSet::from(["hidden".to_owned()]);
    let source = include_str!("../../resources/linux/Zetta.desktop");
    let rendered = render_managed_desktop_entry(
        source,
        Path::new("/usr/bin/zetta"),
        &profiles,
        &hidden_profiles,
    )
    .unwrap();

    assert!(rendered.contains("Actions=new-window;profile-1;profile-2;"));
    assert!(rendered.contains("Exec=/usr/bin/zetta --zetta-profile-actions-generation "));
    assert!(
        !rendered.contains("Exec=/usr/bin/zetta --new-window --zetta-profile-actions-generation ")
    );
    assert!(
        rendered.contains("[Desktop Action new-window]\nName=New Window\nExec=zetta --new-window")
    );
    assert!(rendered.contains(
        "[Desktop Action profile-1]\nName=PowerShell\nExec=/usr/bin/zetta --new-window --profile PowerShell"
    ));
    assert!(rendered.contains(
        "[Desktop Action profile-2]\nName=WSL: Ubuntu\nExec=/usr/bin/zetta --new-window --profile \"WSL: Ubuntu\""
    ));
    assert!(!rendered.contains("Hidden"));
}

#[test]
fn omits_names_that_cannot_be_represented_in_exec_fields() {
    let profiles = [profile("Visible"), profile("line\nbreak"), profile("")];
    let hidden_profiles = HashSet::new();
    let source = include_str!("../../resources/linux/Zetta.desktop");
    let rendered = render_managed_desktop_entry(
        source,
        Path::new("/usr/bin/zetta"),
        &profiles,
        &hidden_profiles,
    )
    .unwrap();

    assert!(rendered.contains("Actions=new-window;profile-1;"));
    assert!(!rendered.contains("profile-2"));
    assert!(!rendered.contains("Name=line"));
    assert!(
        render_managed_desktop_entry(
            source,
            Path::new("/usr/bin/\nzetta"),
            &profiles,
            &hidden_profiles,
        )
        .is_none()
    );
}

#[test]
fn quotes_desktop_exec_profile_names_according_to_the_spec() {
    assert_eq!(quote_exec_argument("PowerShell"), "PowerShell");
    assert_eq!(
        quote_exec_argument(r##"A "quoted" \ $ 100%"##),
        r##""A \"quoted\" \\ \$ 100%%""##
    );
    assert_eq!(escape_desktop_string("A;B\\C"), r##"A\;B\\C"##);
}

#[test]
fn changing_visible_profiles_changes_the_main_exec_revision() {
    let source = include_str!("../../resources/linux/Zetta.desktop");
    let first = render_managed_desktop_entry(
        source,
        Path::new("/usr/bin/zetta"),
        &[profile("First")],
        &HashSet::new(),
    )
    .unwrap();
    let second = render_managed_desktop_entry(
        source,
        Path::new("/usr/bin/zetta"),
        &[profile("Second")],
        &HashSet::new(),
    )
    .unwrap();

    assert_ne!(first, second);
    assert_eq!(
        first.matches("--zetta-profile-actions-generation").count(),
        1
    );
    assert_eq!(
        second.matches("--zetta-profile-actions-generation").count(),
        1
    );
}

#[test]
fn ignores_unmanaged_or_ambiguous_desktop_entries() {
    let profiles = [profile("PowerShell")];
    let hidden_profiles = HashSet::new();
    assert!(
        render_managed_desktop_entry(
            "[Desktop Entry]\nActions=new-window;\n",
            Path::new("/usr/bin/zetta"),
            &profiles,
            &hidden_profiles,
        )
        .is_none()
    );

    let source = include_str!("../../resources/linux/Zetta.desktop");
    let duplicate = format!("{source}\n{ACTIONS_BEGIN}\n");
    assert!(
        render_managed_desktop_entry(
            &duplicate,
            Path::new("/usr/bin/zetta"),
            &profiles,
            &hidden_profiles,
        )
        .is_none()
    );
}

#[test]
fn generated_desktop_entry_is_accepted_by_desktop_tools() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Zetta.desktop");
    let profiles = [profile("A;B"), profile("WSL: Ubuntu")];
    let rendered = render_managed_desktop_entry(
        include_str!("../../resources/linux/Zetta.desktop"),
        Path::new("/usr/bin/zetta"),
        &profiles,
        &HashSet::new(),
    )
    .unwrap();
    std::fs::write(&path, rendered).unwrap();

    let validation = match std::process::Command::new("desktop-file-validate")
        .arg(&path)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("running desktop-file-validate: {error}"),
    };
    assert!(
        validation.status.success(),
        "desktop-file-validate failed: {}",
        String::from_utf8_lossy(&validation.stderr)
    );

    let gio = match std::process::Command::new("gio")
        .args(["info", "--attributes=standard::content-type"])
        .arg(&path)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("running gio: {error}"),
    };
    assert!(gio.status.success());
    assert!(String::from_utf8_lossy(&gio.stdout).contains("application/x-desktop"));
}

#[test]
fn replaces_the_desktop_entry_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Zetta.desktop");
    std::fs::write(&path, "old").unwrap();
    let permissions = std::fs::metadata(&path).unwrap().permissions();

    replace_desktop_file(&path, "new", &permissions).unwrap();

    assert_eq!(std::fs::read_to_string(path).unwrap(), "new");
}
