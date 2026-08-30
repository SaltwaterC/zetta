use super::*;
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

struct UnsupportedBackend;

impl FileCopyBackend for UnsupportedBackend {
    fn copy_file(&self, _source: &Path, _destination: &Path) -> Result<CopyFileResult> {
        Ok(CopyFileResult::Unsupported)
    }
}

struct FailingBackend {
    copied_files: Cell<usize>,
}

impl FileCopyBackend for FailingBackend {
    fn copy_file(&self, source: &Path, destination: &Path) -> Result<CopyFileResult> {
        if self.copied_files.get() == 1 {
            anyhow::bail!("injected copy failure");
        }
        self.copied_files.set(self.copied_files.get() + 1);
        copy_regular_file(source, destination)?;
        Ok(CopyFileResult::Copied)
    }
}

#[test]
fn validates_relative_paths_and_overlaps() {
    assert_eq!(
        validate_copy_path(Path::new("./config/settings")).unwrap(),
        PathBuf::from("config/settings")
    );
    assert!(validate_copy_path(Path::new("../config")).is_err());
    assert!(validate_copy_path(Path::new("/config")).is_err());
    assert!(validate_copy_path(Path::new(".")).is_err());

    assert!(validate_copy_paths(&[PathBuf::from("config"), PathBuf::from("config/dev")]).is_err());
    assert!(validate_copy_paths(&[PathBuf::from("config"), PathBuf::from("config")]).is_err());
    assert!(validate_copy_paths(&[PathBuf::from("config"), PathBuf::from("cache")]).is_ok());
}

#[test]
fn copies_files_directories_and_uses_regular_fallback_when_cow_is_unavailable() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    let destination = temporary.path().join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("file"), "source\n").unwrap();
    fs::write(source.join("nested/value"), "nested\n").unwrap();

    let backend = UnsupportedBackend;
    copy_paths_with_backend(
        &source,
        &destination,
        &[PathBuf::from("file"), PathBuf::from("nested")],
        &backend,
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("file")).unwrap(),
        "source\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("nested/value")).unwrap(),
        "nested\n"
    );
    fs::write(destination.join("file"), "destination\n").unwrap();
    assert_eq!(fs::read_to_string(source.join("file")).unwrap(), "source\n");
}

#[test]
fn cow_capability_query_handles_supported_unsupported_and_missing_destinations() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    let destination = temporary.path().join("destination");
    let missing_destination = destination.join("future/nested");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();

    let expected = !matches!(
        detect_cow_filesystem(&source, &destination),
        CowFilesystem::Unsupported
    );
    assert_eq!(cow_copy_supported(&source, &destination), expected);
    assert_eq!(cow_copy_supported(&source, &missing_destination), expected);
    assert!(!cow_copy_supported(
        &temporary.path().join("missing-source"),
        &destination
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn cow_capability_query_rejects_different_filesystems() {
    let source_temporary = TempDir::new().unwrap();
    let Ok(destination_temporary) = TempDir::new_in("/dev/shm") else {
        return;
    };
    let Some(source_type) = linux_filesystem_type(source_temporary.path()) else {
        return;
    };
    let Some(destination_type) = linux_filesystem_type(destination_temporary.path()) else {
        return;
    };
    if source_type == destination_type {
        return;
    }

    assert!(!cow_copy_supported(
        source_temporary.path(),
        &destination_temporary.path().join("missing")
    ));
}

#[cfg(unix)]
#[test]
fn preserves_symlinks_without_traversing_them() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    let destination = temporary.path().join("destination");
    fs::create_dir_all(source.join("tree")).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("tree/value"), "value\n").unwrap();
    symlink("value", source.join("tree/link")).unwrap();

    copy_paths(&source, &destination, &[PathBuf::from("tree")]).unwrap();

    let link = destination.join("tree/link");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(link).unwrap(), PathBuf::from("value"));
}

#[cfg(unix)]
#[test]
fn rejects_intermediate_source_symlink_and_existing_destination() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    let destination = temporary.path().join("destination");
    let outside = temporary.path().join("outside");
    fs::create_dir_all(source.join("real")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(outside.join("value"), "outside\n").unwrap();
    symlink(&outside, source.join("link")).unwrap();
    fs::write(source.join("existing"), "source\n").unwrap();
    fs::write(destination.join("existing"), "destination\n").unwrap();

    assert!(copy_paths(&source, &destination, &[PathBuf::from("link/value")]).is_err());
    assert!(copy_paths(&source, &destination, &[PathBuf::from("existing")]).is_err());
    assert_eq!(
        fs::read_to_string(destination.join("existing")).unwrap(),
        "destination\n"
    );
}

#[test]
fn injectable_backend_can_fail_after_a_partial_copy() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("source");
    let destination = temporary.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("one"), "one\n").unwrap();
    fs::write(source.join("two"), "two\n").unwrap();

    let backend = FailingBackend {
        copied_files: Cell::new(0),
    };
    let error = copy_paths_with_backend(
        &source,
        &destination,
        &[PathBuf::from("one"), PathBuf::from("two")],
        &backend,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("injected copy failure"));
    assert_eq!(
        fs::read_to_string(destination.join("one")).unwrap(),
        "one\n"
    );
    assert!(!destination.join("two").exists());
}
