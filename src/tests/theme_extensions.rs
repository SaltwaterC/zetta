use super::*;

#[test]
fn rejects_theme_paths_outside_the_archive() {
    assert!(validate_relative_theme_path(Path::new("themes/one.json")).is_ok());
    assert!(validate_relative_theme_path(Path::new("../one.json")).is_err());
    assert!(validate_relative_theme_path(Path::new("themes/one.toml")).is_err());
}

#[test]
fn lists_and_removes_only_managed_extension_themes() {
    let directory = tempfile::tempdir().unwrap();
    let managed = directory.path().join("catppuccin--0--mauve.json");
    let manual = directory.path().join("my-theme.json");
    let theme = br#"{"themes":[{"name":"Catppuccin Mauve"}]}"#;
    fs::write(&managed, theme).unwrap();
    fs::write(&manual, theme).unwrap();

    let installed = super::installed(directory.path()).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].id, "catppuccin");
    assert_eq!(installed[0].theme_names, ["Catppuccin Mauve"]);
    assert_eq!(installed[0].file_count, 1);

    assert_eq!(super::remove("catppuccin", directory.path()).unwrap(), 1);
    assert!(!managed.exists());
    assert!(manual.exists());
}

#[test]
fn extension_ids_are_safe_as_file_names() {
    assert_eq!(safe_file_component("catppuccin/theme"), "catppuccin-theme");
}
