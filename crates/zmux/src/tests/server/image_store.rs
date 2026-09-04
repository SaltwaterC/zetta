use super::*;

#[test]
#[cfg(all(unix, not(target_os = "macos")))]
fn startup_sweep_removes_numeric_stale_directories() {
    let directory = tempfile::tempdir().unwrap();
    let image_directory = directory.path().join("clipboard");
    create_private_dir(&image_directory).unwrap();
    create_private_dir(&image_directory.join("41")).unwrap();
    std::fs::write(image_directory.join("41/image.png"), b"stale").unwrap();
    std::fs::create_dir_all(image_directory.join("not-a-session")).unwrap();

    let daemon = Arc::new(Daemon::new(
        directory.path(),
        Retention::None,
        image_directory.clone(),
        #[cfg(feature = "session-persistence")]
        None,
        1,
        1,
        -1,
    ));
    sweep_image_staging(&daemon).unwrap();

    assert!(!image_directory.join("41").exists());
    assert!(image_directory.join("not-a-session").is_dir());
}
