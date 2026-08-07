use super::*;

#[test]
fn managed_names_round_trip_the_owner_and_suffix() {
    let owner = parse_owner("zetta-scrollback-42-123456-random.txt").unwrap();
    assert_eq!(owner.pid, 42);
    assert_eq!(owner.start_time, Some(123456));
    assert_eq!(owner.suffix, "random.txt");
    let pending = parse_owner("zetta-scrollback-pending-42-123456-random.txt").unwrap();
    assert_eq!(pending.kind, FileKind::Pending);
    let editor = parse_owner("zetta-scrollback-editor-42-123456-random.txt").unwrap();
    assert_eq!(editor.kind, FileKind::Editor);
    let legacy = parse_owner("zetta-scrollback-42-random.txt").unwrap();
    assert_eq!(legacy.pid, 42);
    assert_eq!(legacy.start_time, None);
    assert_eq!(legacy.suffix, "random.txt");
    assert_eq!(legacy.kind, FileKind::Legacy);
    assert!(parse_owner("unrelated-42-random.txt").is_none());
}

#[test]
fn unclaimed_dumps_expire_but_editor_dumps_do_not() {
    let now = std::time::SystemTime::UNIX_EPOCH + PENDING_FILE_GRACE_PERIOD;
    let pid = Pid::from_u32(42);
    assert!(handoff_expired(
        FileKind::Pending,
        Some(pid),
        pid,
        Some(std::time::SystemTime::UNIX_EPOCH),
        now
    ));
    assert!(!handoff_expired(
        FileKind::Editor,
        Some(pid),
        pid,
        Some(std::time::SystemTime::UNIX_EPOCH),
        now
    ));
}

#[test]
fn managed_file_is_private_and_removable() {
    let path = create("private scrollback").unwrap();
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let _ = remove_managed(&path);
    assert!(!path.exists());
}

#[test]
fn cleanup_removes_a_file_owned_by_a_dead_process() {
    let path = create("stale scrollback").unwrap();
    let suffix = parse_owner(path.file_name().unwrap().to_str().unwrap())
        .unwrap()
        .suffix;
    let stale_path = path
        .parent()
        .unwrap()
        .join(format!("{FILE_PREFIX}{}-1-{suffix}", u32::MAX));
    fs::rename(&path, &stale_path).unwrap();

    let _ = cleanup_stale();

    assert!(!stale_path.exists());
}

#[cfg(windows)]
#[test]
fn windows_scrollback_root_ignores_an_msys_temporary_directory() {
    assert_eq!(
        windows_fallback_root(
            Some(PathBuf::from(r"C:\Users\saltw\AppData\Local")),
            PathBuf::from(r"C:\msys64\tmp"),
        ),
        PathBuf::from(r"C:\Users\saltw\AppData\Local\Temp\zetta"),
    );
}

#[cfg(windows)]
#[test]
fn windows_helper_claims_a_file_created_in_the_gui_scrollback_root() {
    let pending = create("scrollback").unwrap();
    let claimed = claim_for_editor(&pending).unwrap();

    assert!(claimed.exists());
    assert!(
        claimed
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("zetta-scrollback-editor-"))
    );
    assert!(remove_managed(&claimed));
}
