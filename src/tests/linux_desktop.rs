use super::*;
use crate::profile_icon::ProfileIcon;
use task::Shell;

fn profile(name: &str) -> Profile {
    Profile {
        console_backend: Default::default(),
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

/// A desktop entry whose main `Exec=` launches `launched`.
fn entry_launching(launched: &Path) -> String {
    include_str!("../../resources/linux/Zetta.desktop").replacen(
        "\nExec=zetta\n",
        &format!(
            "\nExec={}\n",
            quote_exec_argument(&launched.to_string_lossy())
        ),
        1,
    )
}

#[test]
fn an_entry_belongs_to_the_executable_it_launches() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("installed zetta");
    std::fs::write(&installed, "").unwrap();

    assert!(entry_launches(&entry_launching(&installed), &installed));
}

#[test]
fn a_symlinked_install_location_still_matches() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("zetta");
    let link = directory.path().join("link");
    std::fs::write(&installed, "").unwrap();
    std::os::unix::fs::symlink(&installed, &link).unwrap();

    assert!(entry_launches(&entry_launching(&link), &installed));
}

#[test]
fn another_build_does_not_own_the_entry() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("installed");
    let debug = directory.path().join("debug");
    std::fs::write(&installed, "").unwrap();
    std::fs::write(&debug, "").unwrap();

    assert!(!entry_launches(&entry_launching(&installed), &debug));
}

#[test]
fn an_executable_that_no_longer_exists_owns_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("zetta");
    std::fs::write(&installed, "").unwrap();
    let entry = entry_launching(&installed);
    let replaced = directory.path().join("zetta (deleted)");

    assert!(!entry_launches(&entry, &replaced));
}

#[test]
fn a_debug_build_leaves_the_installed_entry_alone() {
    let home = tempfile::tempdir().unwrap();
    let applications = home.path().join(".local/share/applications");
    std::fs::create_dir_all(&applications).unwrap();
    let entry = applications.join("Zetta.desktop");
    let installed = home.path().join("installed");
    std::fs::write(&installed, "").unwrap();
    let original = entry_launching(&installed);
    std::fs::write(&entry, &original).unwrap();

    let debug = home.path().join("debug");
    std::fs::write(&debug, "").unwrap();

    let rewritten =
        update_profile_actions_at(&entry, &debug, &[profile("Bash")], &HashSet::new()).unwrap();

    assert!(!rewritten);

    assert_eq!(std::fs::read_to_string(&entry).unwrap(), original);

    // The installed binary, on the other hand, does update it.
    assert!(
        update_profile_actions_at(&entry, &installed, &[profile("Bash")], &HashSet::new()).unwrap()
    );
    assert!(
        std::fs::read_to_string(entry)
            .unwrap()
            .contains("Name=Bash")
    );
}

#[test]
fn quoted_and_bare_main_programs_are_read() {
    assert_eq!(
        main_exec_program("[Desktop Entry]\nExec=/a/zetta --x\n\n[Desktop Action a]\nExec=/b\n"),
        Some("/a/zetta".to_owned())
    );
    assert_eq!(
        main_exec_program("[Desktop Entry]\nExec=\"/a b/zet\\\"ta\" --x\n\n[Desktop Action a]\n"),
        Some("/a b/zet\"ta".to_owned())
    );
}
