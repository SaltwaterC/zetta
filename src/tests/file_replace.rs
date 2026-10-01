use super::*;

fn scratch_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "zetta-file-replace-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn a_replaced_file_gains_a_trailing_newline_once() {
    let directory = scratch_directory("newline");
    let path = directory.join("config.json");

    replace_file(&path, "{}").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    replace_file(&path, "{}\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn missing_parent_directories_are_created() {
    let directory = scratch_directory("parents");
    let path = directory.join("zetta").join("keymap.json");

    replace_file(&path, "[]").unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), "[]\n");
    fs::remove_dir_all(directory).unwrap();
}

/// A configuration kept in a dotfiles repository is usually a symlink to it.
/// Renaming over the link would silently fork the file from the repository.
#[cfg(unix)]
#[test]
fn a_symlinked_file_is_replaced_where_the_link_points() {
    let directory = scratch_directory("symlink");
    let real = directory.join("dotfiles-config.json");
    let link = directory.join("config.json");
    fs::write(&real, "{}\n").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    replace_file(&link, r#"{"compact_mode":true}"#).unwrap();

    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is still a link"
    );
    assert_eq!(
        fs::read_to_string(&real).unwrap(),
        "{\"compact_mode\":true}\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn an_existing_file_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = scratch_directory("permissions");
    let path = directory.join("config.json");
    fs::write(&path, "{}\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    replace_file(&path, "[]").unwrap();

    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    fs::remove_dir_all(directory).unwrap();
}

/// Every file is staged before any is renamed, so a failure staging the second
/// leaves the first untouched.
#[cfg(unix)]
#[test]
fn a_failure_staging_one_file_changes_none_of_them() {
    let directory = scratch_directory("all-or-none");
    let first = directory.join("keymap.json");
    fs::write(&first, "old\n").unwrap();
    // A regular file where the second target's directory would have to be.
    let blocker = directory.join("not-a-directory");
    fs::write(&blocker, "").unwrap();
    let second = blocker.join("config.json");

    let result = replace_files(&[(&first, "new"), (&second, "new")]);

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&first).unwrap(), "old\n");
    fs::remove_dir_all(directory).unwrap();
}
