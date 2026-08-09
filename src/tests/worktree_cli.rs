use super::*;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Mutex, MutexGuard, OnceLock},
};

use tempfile::TempDir;

static WORKTREE_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct GitFixture {
    _tempdir: TempDir,
    root: PathBuf,
    global_config: PathBuf,
}

struct EnvironmentGuard {
    test_directory: Option<PathBuf>,
    test_git_config: Option<(OsString, OsString)>,
}

fn empty_system_config() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        replace_test_current_directory(self.test_directory.take());
        replace_test_git_config(self.test_git_config.take());
    }
}

impl GitFixture {
    fn new() -> Self {
        let tempdir = TempDir::new().unwrap();
        let root = tempdir.path().join("project with space");
        fs::create_dir(&root).unwrap();
        let global_config = tempdir.path().join("global-config");
        let fixture = Self {
            _tempdir: tempdir,
            root,
            global_config,
        };
        fixture.git(&fixture.root, &["init", "-q", "-b", "main"]);
        fixture.git(
            &fixture.root,
            &["config", "user.email", "test@example.invalid"],
        );
        fixture.git(&fixture.root, &["config", "user.name", "Zetta Test"]);
        fixture.commit(&fixture.root, "file", "base\n", "initial");
        fixture
    }

    fn enter(&self, path: &Path) -> EnvironmentGuard {
        let test_directory = replace_test_current_directory(Some(path.to_owned()));
        let test_git_config = replace_test_git_config(Some((
            self.global_config.as_os_str().to_os_string(),
            OsString::from(empty_system_config()),
        )));
        EnvironmentGuard {
            test_directory,
            test_git_config,
        }
    }

    fn git(&self, path: &Path, arguments: &[&str]) -> String {
        let output = self.git_output(path, arguments);
        assert!(
            output.status.success(),
            "git {:?} failed:\n{}",
            arguments,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn git_output(&self, path: &Path, arguments: &[&str]) -> Output {
        Command::new("git")
            .current_dir(path)
            .env("GIT_CONFIG_GLOBAL", &self.global_config)
            .env("GIT_CONFIG_SYSTEM", empty_system_config())
            .args(arguments)
            .output()
            .unwrap()
    }

    fn commit(&self, path: &Path, file: &str, contents: &str, message: &str) {
        fs::write(path.join(file), contents).unwrap();
        self.git(path, &["add", file]);
        self.git(
            path,
            &["-c", "commit.gpgsign=false", "commit", "-qm", message],
        );
    }

    fn default_root(&self) -> PathBuf {
        self.root.parent().unwrap().join(format!(
            "{}-worktrees",
            self.root.file_name().unwrap().to_string_lossy()
        ))
    }

    fn worktree_path(&self, name: &str) -> PathBuf {
        self.default_root().join(name)
    }

    fn create_worktree(&self, name: &str) -> PathBuf {
        let root = self.root.clone();
        in_directory(self, &root, || {
            run(&WorktreeCommand::New {
                name: name.to_owned(),
                path_only: true,
            })
            .unwrap();
        });
        self.worktree_path(name)
    }
}

fn test_lock() -> MutexGuard<'static, ()> {
    WORKTREE_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap()
}

fn in_directory<T>(fixture: &GitFixture, path: &Path, operation: impl FnOnce() -> T) -> T {
    let _lock = test_lock();
    let _environment = fixture.enter(path);
    operation()
}

#[test]
fn parses_worktree_commands_and_path_only_aliases() {
    assert_eq!(
        parse_worktree_args(&[OsString::from("new"), OsString::from("feature/api")]).unwrap(),
        WorktreeCommand::New {
            name: "feature/api".to_owned(),
            path_only: false,
        }
    );
    assert_eq!(
        parse_worktree_args(&[
            OsString::from("new"),
            OsString::from("-P"),
            OsString::from("feature/api"),
        ])
        .unwrap(),
        WorktreeCommand::New {
            name: "feature/api".to_owned(),
            path_only: true,
        }
    );
    assert_eq!(
        parse_worktree_args(&[OsString::from("done"), OsString::from("--path-only")]).unwrap(),
        WorktreeCommand::Done { path_only: true }
    );
    assert_eq!(
        parse_worktree_args(&[OsString::from("status")]).unwrap(),
        WorktreeCommand::Status
    );
    assert_eq!(
        parse_worktree_args(&[OsString::from("rerere")]).unwrap(),
        WorktreeCommand::Rerere
    );
}

#[test]
fn rejects_invalid_worktree_arguments() {
    assert!(parse_worktree_args(&[]).is_err());
    assert!(parse_worktree_args(&[OsString::from("unknown")]).is_err());
    assert!(
        parse_worktree_args(&[
            OsString::from("new"),
            OsString::from("one"),
            OsString::from("two")
        ])
        .is_err()
    );
    assert!(parse_worktree_args(&[OsString::from("new"), OsString::from("--path-only")]).is_err());
    assert!(
        parse_worktree_args(&[OsString::from("status"), OsString::from("--path-only")]).is_err()
    );
}

#[test]
fn worktree_help_covers_the_workflow() {
    assert!(worktree_help().contains("wt.root"));
    assert!(worktree_help().contains("zetta wt rerere"));
    assert!(worktree_new_help().contains("--path-only"));
    assert!(worktree_done_help().contains("stage"));
    assert!(worktree_status_help().contains("never creates"));
    assert!(worktree_rerere_help().contains("rerere.autoupdate"));
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
fn rejects_branch_path_and_name_collisions() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    let first = fixture.create_worktree("same");
    assert!(
        in_directory(&fixture, &root, || {
            run(&WorktreeCommand::New {
                name: "same".to_owned(),
                path_only: false,
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
            })
        })
        .is_err()
    );
    assert!(
        in_directory(&fixture, &root, || {
            run(&WorktreeCommand::New {
                name: "bad..name".to_owned(),
                path_only: false,
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
}

#[test]
fn rerere_uses_the_isolated_global_git_config() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    in_directory(&fixture, &root, || {
        run(&WorktreeCommand::Rerere).unwrap();
    });
    assert_eq!(
        fixture.git(&root, &["config", "--global", "--get", "rerere.enabled"]),
        "true\n"
    );
    assert_eq!(
        fixture.git(&root, &["config", "--global", "--get", "rerere.autoupdate"]),
        "true\n"
    );
}
