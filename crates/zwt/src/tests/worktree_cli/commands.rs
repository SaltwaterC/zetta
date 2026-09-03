use super::*;
use crate::worktree_cli::tests::*;

#[test]
fn creates_nested_worktrees_and_records_the_source_branch() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("feature/api");
    assert!(worktree.is_dir());
    assert_eq!(
        fixture.git(
            &fixture.root,
            &["config", "--get", "wtbranch.wt/feature/api.base"]
        ),
        "main\n"
    );
    assert_eq!(
        fixture.git(&worktree, &["branch", "--show-current"]),
        "wt/feature/api\n"
    );
}

#[test]
fn successful_new_requests_the_exact_worktree_name_after_metadata_setup() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    let (result, requests) = in_directory(&fixture, &root, || {
        capture_worktree_name_requests(|| {
            run(&WorktreeCommand::New {
                name: "feature/api".to_owned(),
                path_only: true,
                copy_paths: Vec::new(),
            })
        })
    });

    result.unwrap();
    assert_eq!(requests, vec![Some("feature/api".to_owned())]);
}

#[test]
fn failed_new_does_not_request_a_worktree_name() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    let (result, requests) = in_directory(&fixture, &root, || {
        capture_worktree_name_requests(|| {
            run(&WorktreeCommand::New {
                name: "copy-failure".to_owned(),
                path_only: true,
                copy_paths: vec![PathBuf::from("missing")],
            })
        })
    });

    assert!(result.is_err());
    assert!(requests.is_empty());
}

#[test]
fn copies_untracked_files_and_directories_into_the_new_worktree() {
    let fixture = GitFixture::new();
    let local = fixture.root.join("local settings");
    fs::create_dir_all(local.join("nested")).unwrap();
    fs::write(local.join("settings.json"), "source settings\n").unwrap();
    fs::write(local.join("nested/value"), "nested source\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("settings.json", local.join("settings-link")).unwrap();

    let root = fixture.root.clone();
    let destination = fixture.worktree_path("copied");
    in_directory(&fixture, &root, || {
        run(&WorktreeCommand::New {
            name: "copied".to_owned(),
            path_only: true,
            copy_paths: vec![PathBuf::from("local settings")],
        })
        .unwrap();
    });

    assert_eq!(
        fs::read_to_string(destination.join("local settings/settings.json")).unwrap(),
        "source settings\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("local settings/nested/value")).unwrap(),
        "nested source\n"
    );
    #[cfg(unix)]
    assert!(
        fs::symlink_metadata(destination.join("local settings/settings-link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );

    fs::write(
        destination.join("local settings/settings.json"),
        "destination settings\n",
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(local.join("settings.json")).unwrap(),
        "source settings\n"
    );
}

#[test]
fn copy_failures_roll_back_the_new_worktree_branch_and_metadata() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    let destination = fixture.worktree_path("copy-conflict");
    let error = in_directory(&fixture, &root, || {
        run(&WorktreeCommand::New {
            name: "copy-conflict".to_owned(),
            path_only: false,
            copy_paths: vec![PathBuf::from("file")],
        })
        .unwrap_err()
    });

    assert!(error.to_string().contains("copying worktree paths"));
    assert!(!destination.exists());
    assert_eq!(
        fixture.git(&root, &["branch", "--list", "wt/copy-conflict"]),
        ""
    );
    assert_eq!(
        fixture
            .git_output(
                &root,
                &["config", "--get", "wtbranch.wt/copy-conflict.base"]
            )
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn missing_copy_sources_are_rejected_before_creating_a_worktree() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    let destination = fixture.worktree_path("missing-copy");
    let error = in_directory(&fixture, &root, || {
        run(&WorktreeCommand::New {
            name: "missing-copy".to_owned(),
            path_only: false,
            copy_paths: vec![PathBuf::from("does-not-exist")],
        })
        .unwrap_err()
    });

    assert!(error.to_string().contains("does not exist"));
    assert!(!destination.exists());
    assert_eq!(
        fixture.git(&root, &["branch", "--list", "wt/missing-copy"]),
        ""
    );
}

#[cfg(feature = "recursive-submodules")]
#[test]
fn initializes_top_level_and_nested_submodules_at_recorded_commits() {
    let fixture = GitFixture::new();
    fixture.allow_file_protocol();
    let leaf = fixture.create_repository("leaf");
    let top = fixture.create_repository("top");
    let leaf_commit = fixture.add_submodule(&top, &leaf, "nested");
    let top_commit = fixture.git(&top, &["rev-parse", "HEAD"]).trim().to_owned();
    fixture.add_submodule(&fixture.root, &top, "vendor/top");
    fixture.initialize_submodule(&fixture.root.join("vendor/top"), "nested");

    let worktree = fixture.create_worktree("submodules");
    assert_eq!(
        fixture
            .git(&worktree.join("vendor/top"), &["rev-parse", "HEAD"])
            .trim(),
        top_commit
    );
    assert_eq!(
        fixture
            .git(&worktree.join("vendor/top/nested"), &["rev-parse", "HEAD"],)
            .trim(),
        leaf_commit
    );
    assert!(fixture.uses_reference(
        &worktree.join("vendor/top"),
        &fixture.root.join("vendor/top")
    ));
    assert!(fixture.uses_reference(
        &worktree.join("vendor/top/nested"),
        &fixture.root.join("vendor/top/nested")
    ));
}

#[cfg(feature = "recursive-submodules")]
#[test]
fn falls_back_to_a_submodule_remote_when_the_source_checkout_is_missing() {
    let fixture = GitFixture::new();
    fixture.allow_file_protocol();
    let remote = fixture.create_repository("remote");
    let remote_commit = fixture.add_submodule(&fixture.root, &remote, "vendor/remote");
    fixture.remove_source_submodule("vendor/remote");

    let worktree = fixture.create_worktree("remote-fallback");
    assert_eq!(
        fixture
            .git(&worktree.join("vendor/remote"), &["rev-parse", "HEAD"])
            .trim(),
        remote_commit
    );
    assert!(!fixture.uses_reference(
        &worktree.join("vendor/remote"),
        &fixture.root.join("vendor/remote")
    ));
}

#[cfg(feature = "recursive-submodules")]
#[test]
fn failed_submodule_initialization_rolls_back_worktree_branch_metadata_and_modules() {
    let fixture = GitFixture::new();
    let remote = fixture.create_repository("remote");
    fixture.add_submodule(&fixture.root, &remote, "vendor/broken");
    fixture.remove_source_submodule("vendor/broken");
    fixture.git(
        &fixture.root,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.vendor/broken.url",
            "/path/that/does/not/exist",
        ],
    );
    fixture.git(&fixture.root, &["add", ".gitmodules"]);
    fixture.git(
        &fixture.root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "break submodule",
        ],
    );

    let root = fixture.root.clone();
    let destination = fixture.worktree_path("broken");
    let error = in_directory(&fixture, &root, || {
        run(&WorktreeCommand::New {
            name: "broken".to_owned(),
            path_only: false,
            copy_paths: Vec::new(),
        })
        .unwrap_err()
    });
    assert!(
        error
            .to_string()
            .contains("initializing worktree submodules")
    );
    assert!(!destination.exists());
    assert_eq!(fixture.git(&root, &["branch", "--list", "wt/broken"]), "");
    assert_eq!(
        fixture
            .git_output(&root, &["config", "--get", "wtbranch.wt/broken.base"])
            .status
            .code(),
        Some(1)
    );
    assert!(!destination.join("vendor/broken").exists());
}

#[test]
fn rejects_branch_path_and_name_collisions() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    let first = fixture.create_worktree("same");
    assert!(
        in_directory(&fixture, &root, || {
            run(&WorktreeCommand::New {
                name: "same".to_owned(),
                path_only: false,
                copy_paths: Vec::new(),
            })
        })
        .is_err()
    );

    let symlink = fixture.default_root().join("link");
    fs::create_dir_all(symlink.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&first, &symlink).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&first, &symlink).unwrap();
    assert!(
        in_directory(&fixture, &root, || {
            run(&WorktreeCommand::New {
                name: "link".to_owned(),
                path_only: false,
                copy_paths: Vec::new(),
            })
        })
        .is_err()
    );
    assert!(
        in_directory(&fixture, &root, || {
            run(&WorktreeCommand::New {
                name: "bad..name".to_owned(),
                path_only: false,
                copy_paths: Vec::new(),
            })
        })
        .is_err()
    );
}

#[test]
fn integrates_clean_worktrees_and_removes_branch_and_metadata() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("feature");
    fixture.commit(&worktree, "work", "done\n", "work");
    let root = fixture.root.clone();
    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: true }).unwrap();
    });
    assert!(!worktree.exists());
    assert_eq!(fixture.git(&root, &["branch", "--list", "wt/feature"]), "");
    assert!(
        fixture
            .git_output(&root, &["config", "--get", "wtbranch.wt/feature.base"])
            .status
            .code()
            == Some(1)
    );
    assert!(root.join("work").is_file());
}

#[test]
fn successful_done_clears_the_originating_worktree_name_after_cleanup() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("clear-name");
    fixture.commit(&worktree, "work", "done\n", "work");

    let (result, requests) = in_directory(&fixture, &worktree, || {
        capture_worktree_name_requests(|| run(&WorktreeCommand::Done { path_only: true }))
    });

    result.unwrap();
    assert_eq!(requests, vec![None]);
}

#[test]
fn abort_discards_dirty_current_worktree_without_changing_dirty_source() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("abort-dirty");
    fixture.commit(&worktree, "committed", "temporary\n", "temporary commit");
    fs::write(worktree.join("untracked"), "discard me\n").unwrap();

    fs::write(fixture.root.join("file"), "source dirty\n").unwrap();
    fixture.git(&fixture.root, &["add", "file"]);
    fs::write(fixture.root.join("source-untracked"), "keep me\n").unwrap();
    let source_head = fixture.git(&fixture.root, &["rev-parse", "refs/heads/main"]);
    let source_file = fs::read_to_string(fixture.root.join("file")).unwrap();
    let source_status = fixture.git(
        &fixture.root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    );
    let root = fixture.root.clone();

    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Abort { path_only: true }).unwrap();
    });

    assert!(!worktree.exists());
    assert_eq!(
        fixture.git(&root, &["branch", "--list", "wt/abort-dirty"]),
        ""
    );
    assert_eq!(
        fixture
            .git_output(&root, &["config", "--get", "wtbranch.wt/abort-dirty.base"],)
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        fixture.git(&root, &["rev-parse", "refs/heads/main"]),
        source_head
    );
    assert_eq!(fs::read_to_string(root.join("file")).unwrap(), source_file);
    assert_eq!(
        fixture.git(
            &root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
        ),
        source_status
    );
}

#[test]
fn successful_abort_clears_the_originating_worktree_name_after_cleanup() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("abort-name");

    let (result, requests) = in_directory(&fixture, &worktree, || {
        capture_worktree_name_requests(|| run(&WorktreeCommand::Abort { path_only: true }))
    });

    result.unwrap();
    assert_eq!(requests, vec![None]);
    assert!(!worktree.exists());
}

#[test]
fn failed_abort_does_not_clear_the_originating_worktree_name_or_remove_worktree() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("abort-invalid-source");
    fixture.git(&fixture.root, &["switch", "-c", "other"]);

    let (result, requests) = in_directory(&fixture, &worktree, || {
        capture_worktree_name_requests(|| run(&WorktreeCommand::Abort { path_only: true }))
    });

    assert!(result.is_err());
    assert!(requests.is_empty());
    assert!(worktree.exists());
    assert!(
        fixture
            .git(
                &fixture.root,
                &["branch", "--list", "wt/abort-invalid-source"]
            )
            .contains("wt/abort-invalid-source")
    );
}

#[test]
fn failed_done_does_not_clear_the_originating_worktree_name() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("keep-name");
    fs::write(worktree.join("untracked"), "dirty\n").unwrap();

    let (result, requests) = in_directory(&fixture, &worktree, || {
        capture_worktree_name_requests(|| run(&WorktreeCommand::Done { path_only: true }))
    });

    assert!(result.is_err());
    assert!(requests.is_empty());
}

#[cfg(feature = "recursive-submodules")]
#[test]
fn integrates_and_forcibly_removes_a_worktree_containing_submodules() {
    let fixture = GitFixture::new();
    fixture.allow_file_protocol();
    let remote = fixture.create_repository("remote");
    fixture.add_submodule(&fixture.root, &remote, "vendor/remote");
    let worktree = fixture.create_worktree("submodule-done");
    let root = fixture.root.clone();

    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: true }).unwrap();
    });

    assert!(!worktree.exists());
    assert_eq!(
        fixture.git(&root, &["branch", "--list", "wt/submodule-done"]),
        ""
    );
    assert_eq!(
        fixture
            .git_output(
                &root,
                &["config", "--get", "wtbranch.wt/submodule-done.base"],
            )
            .status
            .code(),
        Some(1)
    );
}

#[cfg(feature = "recursive-submodules")]
#[test]
fn abort_forcibly_removes_dirty_submodules_and_preserves_dirty_source() {
    let fixture = GitFixture::new();
    fixture.allow_file_protocol();
    let remote = fixture.create_repository("remote");
    fixture.add_submodule(&fixture.root, &remote, "vendor/remote");
    let worktree = fixture.create_worktree("abort-submodule");

    fs::write(worktree.join("vendor/remote/untracked"), "discard me\n").unwrap();
    fs::write(
        fixture.root.join("vendor/remote/source-untracked"),
        "keep me\n",
    )
    .unwrap();
    let source_status = fixture.git(
        &fixture.root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    );
    let root = fixture.root.clone();

    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Abort { path_only: true }).unwrap();
    });

    assert!(!worktree.exists());
    assert_eq!(
        fixture.git(&root, &["branch", "--list", "wt/abort-submodule"]),
        ""
    );
    assert_eq!(
        fixture.git(
            &root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
        ),
        source_status
    );
}

#[cfg(feature = "recursive-submodules")]
#[test]
fn submodule_changes_are_included_in_done_cleanliness_checks() {
    let fixture = GitFixture::new();
    fixture.allow_file_protocol();
    let remote = fixture.create_repository("remote");
    fixture.add_submodule(&fixture.root, &remote, "vendor/remote");
    let worktree = fixture.create_worktree("dirty-submodule");
    fs::write(worktree.join("vendor/remote/untracked"), "dirty\n").unwrap();

    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap_err()
    });
    assert!(error.to_string().contains("current worktree is dirty"));
}

#[test]
fn rebases_after_source_advancement_before_integrating() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("advance");
    fixture.commit(&worktree, "work", "work\n", "work");
    fixture.commit(&fixture.root, "source", "source\n", "source advancement");
    let root = fixture.root.clone();
    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap();
    });
    assert!(!worktree.exists());
    assert_eq!(fixture.git(&root, &["branch", "--list", "wt/advance"]), "");
}

#[test]
fn rejects_dirty_current_and_source_worktrees() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("dirty-current");
    fs::write(worktree.join("untracked"), "dirty\n").unwrap();
    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap_err()
    });
    assert!(error.to_string().contains("current worktree is dirty"));

    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("dirty-source");
    fs::write(fixture.root.join("untracked"), "dirty\n").unwrap();
    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap_err()
    });
    assert!(error.to_string().contains("source worktree is dirty"));
}

#[test]
fn rejects_detached_missing_metadata_and_switched_sources() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("detached");
    fixture.git(&worktree, &["checkout", "--detach", "HEAD"]);
    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap_err()
    });
    assert!(error.to_string().contains("attached branch"));

    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("missing-base");
    fixture.git(
        &worktree,
        &[
            "config",
            "--local",
            "--unset-all",
            "wtbranch.wt/missing-base.base",
        ],
    );
    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap_err()
    });
    assert!(error.to_string().contains("no recorded source branch"));

    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("switched-source");
    fixture.git(&fixture.root, &["switch", "-c", "other"]);
    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap_err()
    });
    assert!(error.to_string().contains("not attached"));
}

#[test]
fn rejects_a_recorded_source_branch_that_no_longer_exists() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("missing-source");
    fixture.git(
        &worktree,
        &[
            "config",
            "--local",
            "wtbranch.wt/missing-source.base",
            "missing-source-branch",
        ],
    );
    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap_err()
    });
    assert!(error.to_string().contains("does not exist"));
}

#[test]
fn rejects_an_in_progress_rebase_on_a_non_worktree_branch() {
    let fixture = GitFixture::new();
    let ordinary = fixture._tempdir.path().join("ordinary worktree");
    fixture.git(
        &fixture.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "ordinary",
            ordinary.to_str().unwrap(),
            "main",
        ],
    );
    fixture.commit(&ordinary, "file", "ordinary\n", "ordinary change");
    fixture.commit(&fixture.root, "file", "source\n", "source change");
    let rebase = fixture.git_output(&ordinary, &["rebase", "main"]);
    assert!(!rebase.status.success());

    let error = in_directory(&fixture, &ordinary, || {
        run(&WorktreeCommand::Done { path_only: false }).unwrap_err()
    });
    assert!(error.to_string().contains("wt/*"));
    fixture.git(&ordinary, &["rebase", "--abort"]);
}

#[test]
fn rollback_removes_the_worktree_branch_and_metadata() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("rollback");
    let root = fixture.root.clone();
    let errors = in_directory(&fixture, &root, || {
        rollback_new_worktree(
            &root,
            &worktree,
            "wt/rollback",
            "wtbranch.wt/rollback.base",
            &[],
        )
    });
    assert!(errors.is_empty(), "rollback failed: {errors:?}");
    assert!(!worktree.exists());
    assert_eq!(fixture.git(&root, &["branch", "--list", "wt/rollback"]), "");
    assert_eq!(
        fixture
            .git_output(&root, &["config", "--get", "wtbranch.wt/rollback.base"])
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn continues_a_conflicting_rebase_after_staging_resolution() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("conflict");
    fixture.commit(&worktree, "file", "work\n", "work change");
    fixture.commit(&fixture.root, "file", "source\n", "source change");

    let first_error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: true }).unwrap_err()
    });
    assert!(first_error.to_string().contains("stage"));
    fs::write(worktree.join("file"), "resolved\n").unwrap();
    fixture.git(&worktree, &["add", "file"]);
    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Done { path_only: true }).unwrap();
    });
    assert!(!worktree.exists());
    assert_eq!(
        fs::read_to_string(fixture.root.join("file")).unwrap(),
        "resolved\n"
    );
}

#[test]
fn abort_removes_a_worktree_with_an_in_progress_rebase() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("abort-rebase");
    fixture.commit(&worktree, "file", "temporary\n", "temporary change");
    fixture.commit(&fixture.root, "file", "source\n", "source change");
    let source_file = fs::read_to_string(fixture.root.join("file")).unwrap();
    let rebase = fixture.git_output(&worktree, &["rebase", "main"]);
    assert!(!rebase.status.success());

    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Abort { path_only: false }).unwrap();
    });

    assert!(!worktree.exists());
    assert_eq!(
        fixture.git(&fixture.root, &["branch", "--list", "wt/abort-rebase"]),
        ""
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("file")).unwrap(),
        source_file
    );
}

#[test]
fn status_report_includes_branch_source_and_root_kind() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("status");
    let report = in_directory(&fixture, &worktree, || {
        let repository = discover_repository(None).unwrap();
        let root = resolved_worktree_root(&repository.current_worktree, &repository.root).unwrap();
        status_report(&repository, &root).unwrap()
    });
    assert!(report.contains("Repository root:"));
    assert!(report.contains("Current worktree:"));
    assert!(report.contains("Current branch: wt/status"));
    assert!(report.contains("Current branch state: attached"));
    assert!(report.contains("Recorded source branch: main"));
    assert!(report.contains("(default)"));
    assert!(report.contains("Submodules: none"));
    assert!(report.contains("Native CoW copying: "));
}

#[cfg(feature = "recursive-submodules")]
#[test]
fn status_report_lists_top_level_and_nested_submodule_paths() {
    let fixture = GitFixture::new();
    fixture.allow_file_protocol();
    let leaf = fixture.create_repository("leaf");
    let top = fixture.create_repository("top");
    let top_base = fixture.git(&top, &["rev-parse", "HEAD"]).trim().to_owned();
    fixture.add_submodule(&top, &leaf, "nested");
    fixture.add_submodule(&fixture.root, &top, "vendor/top");
    fixture.initialize_submodule(&fixture.root.join("vendor/top"), "nested");
    fixture.git(
        &fixture.root.join("vendor/top"),
        &["checkout", "-q", "--detach", &top_base],
    );

    let report = in_directory(&fixture, &fixture.root, || {
        let repository = discover_repository(None).unwrap();
        let root = resolved_worktree_root(&repository.current_worktree, &repository.root).unwrap();
        status_report(&repository, &root).unwrap()
    });

    assert!(report.contains("Submodules: present (2)"));
    assert!(report.contains("  vendor/top\n"));
    assert!(report.contains("  vendor/top/nested\n"));
}

#[test]
fn status_report_checks_existing_and_missing_configured_roots_without_creating_them() {
    let fixture = GitFixture::new();
    let missing_root = fixture._tempdir.path().join("missing worktree root");
    fixture.git(
        &fixture.root,
        &[
            "config",
            "--local",
            "wt.root",
            missing_root.to_str().unwrap(),
        ],
    );

    let missing_report = in_directory(&fixture, &fixture.root, || {
        let repository = discover_repository(None).unwrap();
        let root = resolved_worktree_root(&repository.current_worktree, &repository.root).unwrap();
        status_report(&repository, &root).unwrap()
    });
    assert!(!missing_root.exists());
    assert!(missing_report.contains("Native CoW copying: "));

    let existing_root = fixture._tempdir.path().join("existing worktree root");
    fs::create_dir_all(&existing_root).unwrap();
    fixture.git(
        &fixture.root,
        &[
            "config",
            "--local",
            "wt.root",
            existing_root.to_str().unwrap(),
        ],
    );
    let existing_report = in_directory(&fixture, &fixture.root, || {
        let repository = discover_repository(None).unwrap();
        let root = resolved_worktree_root(&repository.current_worktree, &repository.root).unwrap();
        status_report(&repository, &root).unwrap()
    });
    assert!(existing_report.contains("Native CoW copying: "));
}

#[test]
fn sync_rebases_to_the_latest_source_tip_and_preserves_work() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("sync-latest");
    fixture.commit(&worktree, "work", "work\n", "work");
    fixture.commit(&fixture.root, "source", "source\n", "source");
    let source_tip = fixture.git(&fixture.root, &["rev-parse", "main"]);
    let root = fixture.root.clone();

    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync { commit: None }).unwrap();
    });

    assert_eq!(fixture.git(&worktree, &["rev-parse", "HEAD^1"]), source_tip);
    assert_eq!(
        fixture.git(&worktree, &["merge-base", "wt/sync-latest", "main"]),
        source_tip
    );
    assert_eq!(fixture.git(&root, &["rev-parse", "main"]), source_tip);
    assert_eq!(fs::read_to_string(worktree.join("work")).unwrap(), "work\n");
}

#[test]
fn sync_accepts_an_intermediary_source_commit_then_uses_the_advanced_split_point() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("sync-intermediary");
    fixture.commit(&fixture.root, "source-one", "one\n", "source one");
    let source_one = fixture.git(&fixture.root, &["rev-parse", "main"]);
    fixture.commit(&worktree, "work-one", "work one\n", "work one");

    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync {
            commit: Some(source_one.trim().to_owned()),
        })
        .unwrap();
    });
    assert_eq!(
        fixture.git(&worktree, &["merge-base", "wt/sync-intermediary", "main"]),
        source_one
    );

    fixture.commit(&fixture.root, "source-two", "two\n", "source two");
    let source_two = fixture.git(&fixture.root, &["rev-parse", "main"]);
    fixture.commit(&worktree, "work-two", "work two\n", "work two");
    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync { commit: None }).unwrap();
    });

    assert_eq!(
        fixture.git(&worktree, &["merge-base", "wt/sync-intermediary", "main"]),
        source_two
    );
    assert!(worktree.join("work-one").is_file());
    assert!(worktree.join("work-two").is_file());
}

#[test]
fn sync_allows_dirty_current_and_source_worktrees() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("sync-dirty");
    fixture.commit(&worktree, "work", "work\n", "work");
    fixture.commit(&fixture.root, "source", "source\n", "source");
    fs::write(worktree.join("file"), "local edit\n").unwrap();
    fs::write(fixture.root.join("file"), "source local edit\n").unwrap();
    fs::write(fixture.root.join("source-untracked"), "keep\n").unwrap();
    let source_status = fixture.git(
        &fixture.root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    );

    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync { commit: None }).unwrap();
    });

    assert_eq!(
        fs::read_to_string(worktree.join("file")).unwrap(),
        "local edit\n"
    );
    assert_eq!(
        fixture.git(
            &fixture.root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
        ),
        source_status
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("source-untracked")).unwrap(),
        "keep\n"
    );
}

#[test]
fn sync_accepts_the_split_point_as_a_noop_target() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("sync-split-point");
    let split_point = fixture.git(&fixture.root, &["rev-parse", "main"]);
    fixture.commit(&fixture.root, "source", "source\n", "source");
    fixture.commit(&worktree, "work", "work\n", "work");
    let worktree_tip = fixture.git(&worktree, &["rev-parse", "HEAD"]);
    let source_tip = fixture.git(&fixture.root, &["rev-parse", "main"]);

    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync {
            commit: Some(split_point.trim().to_owned()),
        })
        .unwrap();
    });

    assert_eq!(fixture.git(&worktree, &["rev-parse", "HEAD"]), worktree_tip);
    assert_eq!(
        fixture.git(&fixture.root, &["rev-parse", "main"]),
        source_tip
    );
}

#[test]
fn sync_validates_targets_against_the_current_source_range() {
    let fixture = GitFixture::new();
    fixture.commit(&fixture.root, "pre-split", "pre-split\n", "pre-split");
    let worktree = fixture.create_worktree("sync-validation");
    let split_point = fixture.git(&fixture.root, &["rev-parse", "main~1"]);
    fixture.commit(&fixture.root, "source", "source\n", "source");
    let source_tip = fixture.git(&fixture.root, &["rev-parse", "main"]);
    fixture.commit(&worktree, "work", "work\n", "work");
    fixture.git(&fixture.root, &["switch", "-c", "other"]);
    fixture.commit(&fixture.root, "other", "other\n", "other");
    let off_source = fixture.git(&fixture.root, &["rev-parse", "other"]);
    fixture.git(&fixture.root, &["switch", "main"]);

    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync {
            commit: Some(split_point.trim().to_owned()),
        })
        .unwrap_err()
    });
    assert!(error.to_string().contains("before the current split point"));

    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync {
            commit: Some(off_source.trim().to_owned()),
        })
        .unwrap_err()
    });
    assert!(error.to_string().contains("not at or before"));

    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync {
            commit: Some("not-a-commit".to_owned()),
        })
        .unwrap_err()
    });
    assert!(error.to_string().contains("does not resolve to a commit"));
    assert_eq!(
        fixture.git(&fixture.root, &["rev-parse", "main"]),
        source_tip
    );
}

#[test]
fn sync_conflicts_continue_with_the_same_target_or_without_one() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("sync-conflict");
    let split_point = fixture.git(&fixture.root, &["rev-parse", "main"]);
    fixture.commit(&worktree, "file", "work\n", "work change");
    fixture.commit(&fixture.root, "file", "source\n", "source change");
    let source_tip = fixture.git(&fixture.root, &["rev-parse", "main"]);

    let first_error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync {
            commit: Some(source_tip.trim().to_owned()),
        })
        .unwrap_err()
    });
    assert!(
        first_error
            .to_string()
            .contains("sync rebase stopped with conflicts")
    );

    let mismatch = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync {
            commit: Some(split_point.trim().to_owned()),
        })
        .unwrap_err()
    });
    assert!(
        mismatch
            .to_string()
            .contains("does not match the active rebase target")
    );
    assert!(rebase_in_progress(&worktree).unwrap());

    fs::write(worktree.join("file"), "resolved\n").unwrap();
    fixture.git(&worktree, &["add", "file"]);
    in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync { commit: None }).unwrap();
    });
    assert!(!rebase_in_progress(&worktree).unwrap());
    assert_eq!(
        fs::read_to_string(worktree.join("file")).unwrap(),
        "resolved\n"
    );
}

#[test]
fn sync_distinguishes_conflicts_while_reapplying_the_autostash() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("sync-autostash-conflict");
    fixture.commit(&worktree, "work", "work\n", "work change");
    fs::write(worktree.join("file"), "local edit\n").unwrap();
    fixture.commit(&fixture.root, "file", "source edit\n", "source change");

    let error = in_directory(&fixture, &worktree, || {
        run(&WorktreeCommand::Sync { commit: None }).unwrap_err()
    });
    let message = error.to_string();
    assert!(message.contains("applying the autostash"), "{message}");
    assert!(!rebase_in_progress(&worktree).unwrap());
    assert!(
        fixture
            .git(&worktree, &["status", "--porcelain"])
            .contains("UU file")
    );
}

#[test]
fn config_uses_the_isolated_global_git_config() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    in_directory(&fixture, &root, || {
        run(&WorktreeCommand::Config).unwrap();
    });
    assert_eq!(
        fixture.git(&root, &["config", "--global", "--get", "rerere.enabled"]),
        "true\n"
    );
    assert_eq!(
        fixture.git(&root, &["config", "--global", "--get", "rerere.autoupdate"]),
        "true\n"
    );
    assert_eq!(
        fixture.git(&root, &["config", "--global", "--get", "pull.rebase"]),
        "true\n"
    );
    assert_eq!(
        fixture.git(&root, &["config", "--global", "--get", "rebase.autoStash"]),
        "true\n"
    );
    assert_eq!(
        fixture.git(&root, &["config", "--global", "--get", "alias.up"]),
        "pull --rebase --autostash\n"
    );
}

#[test]
fn config_preserves_unrelated_entries_and_is_idempotent() {
    let fixture = GitFixture::new();
    fs::write(
        &fixture.global_config,
        "[rerere]\n\tstat = true\n[core]\n\teditor = test-editor\n[alias]\n\tdown = log --oneline\n[pull]\n\tff = only\n[rebase]\n\trebaseMerges = true\n",
    )
    .unwrap();
    let root = fixture.root.clone();

    in_directory(&fixture, &root, || {
        run(&WorktreeCommand::Config).unwrap();
        run(&WorktreeCommand::Config).unwrap();
    });

    for (key, value) in [
        ("core.editor", "test-editor"),
        ("alias.down", "log --oneline"),
        ("pull.ff", "only"),
        ("rebase.rebaseMerges", "true"),
        ("rerere.stat", "true"),
        ("pull.rebase", "true"),
        ("rebase.autoStash", "true"),
        ("alias.up", "pull --rebase --autostash"),
        ("rerere.enabled", "true"),
        ("rerere.autoupdate", "true"),
    ] {
        assert_eq!(
            fixture.git(&root, &["config", "--global", "--get", key]),
            format!("{value}\n")
        );
    }
}
