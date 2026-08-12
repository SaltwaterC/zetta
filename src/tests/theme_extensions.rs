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

#[test]
fn rejects_url_path_segments_that_could_escape_the_download_path() {
    assert_eq!(url_path_segment("catppuccin"), Some("catppuccin"));
    assert_eq!(url_path_segment("1.2.3-beta_4"), Some("1.2.3-beta_4"));

    for hostile in [
        "",
        ".",
        "..",
        "../../admin",
        "catppuccin/download",
        "catppuccin?a=b",
        "catppuccin#fragment",
        "%2e%2e",
        "catppuccin theme",
        "café",
    ] {
        assert_eq!(
            url_path_segment(hostile),
            None,
            "{hostile:?} must not be spliced into the download URL"
        );
    }
}

#[test]
fn an_archive_that_expands_past_the_limit_is_rejected() {
    use futures::AsyncReadExt as _;

    // A gzip bomb is small on the wire and enormous once expanded, so the cap
    // has to sit on the decompressed side. Drive the reader directly rather
    // than building a 64 MiB fixture.
    let payload = vec![0u8; 1024];
    let mut reader = LimitedReader::new(futures::io::Cursor::new(payload), 512);
    let mut unpacked = Vec::new();
    let error = futures::executor::block_on(reader.read_to_end(&mut unpacked)).unwrap_err();

    assert!(error.to_string().contains("expands past 512 bytes"));

    // A stream that stays inside the budget is untouched.
    let mut reader = LimitedReader::new(futures::io::Cursor::new(vec![7u8; 512]), 512);
    let mut unpacked = Vec::new();
    futures::executor::block_on(reader.read_to_end(&mut unpacked)).unwrap();
    assert_eq!(unpacked, vec![7u8; 512]);
}
