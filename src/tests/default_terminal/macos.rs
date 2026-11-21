use super::*;

#[test]
fn accepts_only_registered_script_extensions() {
    assert!(accepted_script_extension(Path::new("build.command")));
    assert!(accepted_script_extension(Path::new("script.ZSH")));
    assert!(accepted_script_extension(Path::new("notes.sh")));
    assert!(!accepted_script_extension(Path::new("notes.txt")));
}

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
