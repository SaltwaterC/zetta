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

#[test]
fn catalog_directory_and_manifest_deletion_and_recreation_change_stamps() {
    let directory = tempfile::tempdir().unwrap();
    let catalog = directory.path().join("sessions");
    let missing = session_catalog_stamp(&catalog);
    assert_eq!(missing.catalog, None);
    let persistence = catalog.join("persistence");
    fs::create_dir_all(&persistence).unwrap();
    let created = session_catalog_stamp(&catalog);
    assert_ne!(created, missing);

    let manifest = persistence.join("manifest.json");
    fs::write(&manifest, "{}").unwrap();
    let published = session_catalog_stamp(&catalog);
    assert_ne!(published, created);
    fs::remove_file(&manifest).unwrap();
    assert_eq!(session_catalog_stamp(&catalog), created);
    fs::write(&manifest, "{}").unwrap();
    assert_ne!(session_catalog_stamp(&catalog), created);

    fs::remove_dir_all(&catalog).unwrap();
    assert_eq!(session_catalog_stamp(&catalog), missing);
    fs::create_dir_all(&catalog).unwrap();
    assert_ne!(session_catalog_stamp(&catalog), missing);
}
