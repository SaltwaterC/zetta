use super::*;

#[test]
fn notify_cleanup_parser_accepts_dry_run_and_rejects_duplicates_and_unknowns() {
    assert_eq!(
        parse_notify_cleanup_args([]).unwrap(),
        NotifyCleanupCommand { dry_run: false }
    );
    assert_eq!(
        parse_notify_cleanup_args([OsString::from("--dry-run")]).unwrap(),
        NotifyCleanupCommand { dry_run: true }
    );
    assert_eq!(
        parse_notify_cleanup_args([OsString::from("-n")]).unwrap(),
        NotifyCleanupCommand { dry_run: true }
    );
    assert!(
        parse_notify_cleanup_args([OsString::from("-n"), OsString::from("--dry-run")]).is_err()
    );
    assert!(parse_notify_cleanup_args([OsString::from("--unknown")]).is_err());
}

#[test]
fn worker_notification_timeout_reparses_the_workers_own_argv() {
    let cmd =
        |arguments: &[&str]| -> Vec<OsString> { arguments.iter().map(OsString::from).collect() };

    // A worker relaunched with no explicit --timeout defaults like `notify` itself.
    assert_eq!(
        worker_notification_timeout(&cmd(&[
            "/path/to/zntfy",
            "--sound",
            "zetta-ok",
            "Build finished",
        ])),
        Some(None)
    );

    // An explicit --timeout survives the round trip through its own argv.
    assert_eq!(
        worker_notification_timeout(&cmd(&[
            "/path/to/zntfy",
            "--timeout",
            "5000",
            "--",
            "Build finished",
        ])),
        Some(Some(NotificationTimeout::Milliseconds(5000)))
    );
    assert_eq!(
        worker_notification_timeout(&cmd(&[
            "/path/to/zetta",
            "notify",
            "--timeout",
            "never",
            "Build finished",
            "All tests passed",
        ])),
        Some(Some(NotificationTimeout::Never))
    );

    // Anything that isn't recognizably a `notify` worker's own argv is left
    // alone rather than guessed at.
    assert_eq!(worker_notification_timeout(&cmd(&["/path/to/zntfy"])), None);
    assert_eq!(
        worker_notification_timeout(&cmd(&["/path/to/zetta", "attention", "Build finished"])),
        None
    );
    assert_eq!(
        worker_notification_timeout(&cmd(&["/path/to/zetta", "notify"])),
        None
    );
}
