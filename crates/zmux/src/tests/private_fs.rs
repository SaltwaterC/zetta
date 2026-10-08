//! Compiled into both `zmux` and a no-`zmux` Zetta, like the module it tests,
//! so it may use only `super::*`, `std`, `libc` and `tempfile`.

use super::*;

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};

    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    const UMASK_CHILD: &str = "ZETTA_PRIVATE_FS_TEST_UMASK_CHILD";

    /// The umask is process-wide, so clearing it here would widen whatever the
    /// tests running alongside create. The test runs itself again, alone, in
    /// a child process, and does the work there.
    #[test]
    fn missing_components_are_created_private_even_under_a_permissive_umask() {
        if std::env::var_os(UMASK_CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "private_fs::tests::unix::missing_components_are_created_private_even_under_a_permissive_umask",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env(UMASK_CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success(), "the umask child failed: {status}");
            return;
        }

        let root = tempfile::tempdir().unwrap();
        // SAFETY: umask only swaps the process's file creation mask, and this
        // process runs nothing else.
        unsafe { libc::umask(0) };
        let sessions = root.path().join("fallback").join("zetta").join("sessions");
        let file = sessions.join("zmux.json");
        create_private_dir(&sessions).unwrap();
        write_private_file(&file, b"{}").unwrap();
        for directory in [
            root.path().join("fallback"),
            root.path().join("fallback/zetta"),
            sessions.clone(),
        ] {
            assert_eq!(
                mode(&directory),
                0o700,
                "{} is not private",
                directory.display()
            );
        }
        assert_eq!(mode(&file), 0o600);
    }

    #[test]
    fn an_existing_private_directory_is_tightened_and_accepted() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        fs::create_dir(&sessions).unwrap();
        chmod(&sessions, 0o755);

        create_private_dir(&sessions).unwrap();
        assert_eq!(mode(&sessions), 0o700);
        validate_private_dir(&sessions).unwrap();
    }

    #[test]
    fn a_world_writable_ancestor_is_refused() {
        // The shape of a fallback prefix somebody else could rename: anybody
        // may replace what is inside a world-writable, non-sticky directory.
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        fs::create_dir(&shared).unwrap();
        chmod(&shared, 0o777);
        let sessions = shared.join("zetta-1000").join("zetta").join("sessions");

        let error = create_private_dir(&sessions).unwrap_err();
        assert!(
            format!("{error:#}").contains("writable by every user"),
            "{error:#}"
        );
        assert!(
            !shared.join("zetta-1000").exists(),
            "nothing is created below it"
        );

        // A directory that already sits below it is refused to readers too.
        chmod(&shared, 0o700);
        create_private_dir(&sessions).unwrap();
        chmod(&shared, 0o777);
        assert_eq!(
            validate_private_dir(&sessions).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn a_sticky_world_writable_ancestor_is_accepted() {
        let root = tempfile::tempdir().unwrap();
        let temporary = root.path().join("tmp");
        fs::create_dir(&temporary).unwrap();
        chmod(&temporary, 0o1777);

        create_private_dir(&temporary.join("zetta-1000/zetta/sessions")).unwrap();
    }

    #[test]
    fn a_private_directory_that_is_a_symlink_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        chmod(&elsewhere, 0o700);
        let sessions = root.path().join("sessions");
        symlink(&elsewhere, &sessions).unwrap();

        let error = create_private_dir(&sessions).unwrap_err();
        assert!(format!("{error:#}").contains("symbolic link"), "{error:#}");
        assert_eq!(
            validate_private_dir(&sessions).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn a_symlinked_prefix_is_checked_where_it_leads() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        fs::create_dir(&shared).unwrap();
        chmod(&shared, 0o777);
        let prefix = root.path().join("zetta-1000");
        symlink(&shared, &prefix).unwrap();

        assert!(create_private_dir(&prefix.join("zetta/sessions")).is_err());

        // A link this user made to a directory that is safe is followed.
        let private = root.path().join("private");
        fs::create_dir(&private).unwrap();
        let linked = root.path().join("linked");
        symlink(&private, &linked).unwrap();
        create_private_dir(&linked.join("zetta/sessions")).unwrap();
        assert!(private.join("zetta/sessions").is_dir());
    }

    #[test]
    fn a_missing_directory_is_reported_as_missing() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            validate_private_dir(&root.path().join("absent"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn writing_replaces_a_symlink_instead_of_writing_through_it() {
        let root = tempfile::tempdir().unwrap();
        let victim = root.path().join("victim");
        fs::write(&victim, b"keep").unwrap();
        let endpoint = root.path().join("zmux.json");
        symlink(&victim, &endpoint).unwrap();

        write_private_file(&endpoint, b"endpoint").unwrap();

        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        assert!(
            !fs::symlink_metadata(&endpoint)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&endpoint).unwrap(), b"endpoint");
        // No temporary file is left behind.
        let leftovers = fs::read_dir(root.path())
            .unwrap()
            .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
            .filter(|name| name.ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn reading_refuses_links_special_files_shared_files_and_oversized_files() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real.json");
        write_private_file(&real, b"{}").unwrap();
        assert_eq!(read_private_file(&real, 16).unwrap(), b"{}");

        let link = root.path().join("link.json");
        symlink(&real, &link).unwrap();
        assert!(
            read_private_file(&link, 16).is_err(),
            "a link is not followed"
        );

        let fifo = root.path().join("fifo.json");
        let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: the path is a valid null-terminated string.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
        assert!(
            read_private_file(&fifo, 16).is_err(),
            "a FIFO neither blocks nor reads"
        );

        let shared = root.path().join("shared.json");
        fs::write(&shared, b"{}").unwrap();
        chmod(&shared, 0o666);
        assert!(read_private_file(&shared, 16).is_err());

        let large = root.path().join("large.json");
        write_private_file(&large, &[b' '; 17]).unwrap();
        assert!(read_private_file(&large, 16).is_err());
    }

    #[test]
    fn only_a_socket_is_removed_as_a_stale_socket() {
        let root = tempfile::tempdir().unwrap();
        let regular = root.path().join("control-1.sock");
        fs::write(&regular, b"not a socket").unwrap();
        assert!(remove_stale_socket(&regular).is_err());
        assert!(regular.exists());

        let link = root.path().join("control-2.sock");
        symlink(&regular, &link).unwrap();
        assert!(remove_stale_socket(&link).is_err());
        assert!(regular.exists() && fs::symlink_metadata(&link).is_ok());

        let socket = root.path().join("control-3.sock");
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        remove_stale_socket(&socket).unwrap();
        assert!(fs::symlink_metadata(&socket).is_err());

        remove_stale_socket(&root.path().join("absent.sock")).unwrap();
    }

    #[test]
    fn the_fallback_prefix_is_per_user_under_the_temporary_directory() {
        let fallback = private_fallback_dir();
        assert!(fallback.is_absolute());
        assert_eq!(fallback.parent(), Some(std::env::temp_dir().as_path()));
        // SAFETY: geteuid cannot fail.
        let euid = unsafe { libc::geteuid() };
        assert_eq!(
            fallback.file_name().and_then(|name| name.to_str()),
            Some(format!("zetta-{euid}").as_str())
        );
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn missing_components_are_created_and_usable() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("Zetta").join("sessions");
        create_private_dir(&sessions).unwrap();
        validate_private_dir(&sessions).unwrap();
        write_private_file(&sessions.join("zmux.json"), b"{}").unwrap();
        assert_eq!(
            read_private_file(&sessions.join("zmux.json"), 16).unwrap(),
            b"{}"
        );
    }

    #[test]
    fn user_only_security_builds_for_directories_and_objects() {
        UserOnlySecurity::new(UserOnlySecurityTarget::Directory).unwrap();
        UserOnlySecurity::new(UserOnlySecurityTarget::Object).unwrap();
    }
}
