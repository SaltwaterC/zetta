use super::*;
use crate::worktree_cli::tests::*;
#[cfg(all(unix, feature = "recursive-submodules"))]
use std::os::unix::ffi::OsStrExt;

#[cfg(all(unix, feature = "recursive-submodules"))]
#[test]
fn parses_gitlink_paths_without_lossy_path_conversion() {
    let paths = parse_gitlink_paths(b"160000 commit abc\tdeps/\xff-module\0").unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].as_os_str().as_bytes(), b"deps/\xff-module");
}

#[test]
fn resolves_default_relative_and_absolute_roots_without_creating_them() {
    let fixture = GitFixture::new();
    let default_root = in_directory(&fixture, &fixture.root, || {
        resolved_worktree_root(&fixture.root, &fixture.root).unwrap()
    });
    assert_eq!(default_root.path, fixture.default_root());
    assert!(!default_root.configured);
    assert!(!default_root.path.exists());

    fixture.git(
        &fixture.root,
        &["config", "--local", "wt.root", "nested roots"],
    );
    let relative_root = in_directory(&fixture, &fixture.root, || {
        resolved_worktree_root(&fixture.root, &fixture.root).unwrap()
    });
    assert_eq!(relative_root.path, fixture.root.join("nested roots"));
    assert!(relative_root.configured);
    assert!(!relative_root.path.exists());

    let absolute = fixture._tempdir.path().join("absolute roots");
    fixture.git(
        &fixture.root,
        &["config", "--local", "wt.root", absolute.to_str().unwrap()],
    );
    let absolute_root = in_directory(&fixture, &fixture.root, || {
        resolved_worktree_root(&fixture.root, &fixture.root).unwrap()
    });
    assert_eq!(absolute_root.path, absolute);
    assert!(!absolute_root.path.exists());
}
