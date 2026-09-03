use super::*;

#[test]
fn unchanged_user_themes_are_not_reloaded() {
    let themes_dir = env::temp_dir().join(format!(
        "zetta-theme-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&themes_dir).unwrap();
    let theme_path = themes_dir.join("test.json");
    fs::write(&theme_path, "one").unwrap();
    let mut cache = HashMap::new();

    assert_eq!(
        changed_theme_files(&themes_dir, &mut cache).unwrap(),
        std::slice::from_ref(&theme_path)
    );
    assert!(
        changed_theme_files(&themes_dir, &mut cache)
            .unwrap()
            .is_empty()
    );

    fs::write(&theme_path, "a longer theme").unwrap();
    assert_eq!(
        changed_theme_files(&themes_dir, &mut cache).unwrap(),
        [theme_path]
    );
    fs::remove_dir_all(themes_dir).unwrap();
}

#[test]
fn persistence_manifest_changes_invalidate_the_session_catalog_stamp() {
    let directory = tempfile::tempdir().unwrap();
    let persistence = directory.path().join("persistence");
    fs::create_dir_all(&persistence).unwrap();
    let manifest = persistence.join("manifest.json");
    fs::write(&manifest, "{}").unwrap();
    let before = session_catalog_stamp(directory.path());

    fs::write(&manifest, "{\"records\": []}").unwrap();

    assert_ne!(before, session_catalog_stamp(directory.path()));
}
