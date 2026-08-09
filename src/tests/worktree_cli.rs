use super::*;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
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
                copy_paths: Vec::new(),
            })
            .unwrap();
        });
        self.worktree_path(name)
    }

    fn create_repository(&self, name: &str) -> PathBuf {
        let repository = self._tempdir.path().join(name);
        fs::create_dir(&repository).unwrap();
        self.git(&repository, &["init", "-q", "-b", "main"]);
        self.git(
            &repository,
            &["config", "user.email", "test@example.invalid"],
        );
        self.git(&repository, &["config", "user.name", "Zetta Test"]);
        self.commit(&repository, "file", &format!("{name}\n"), "initial");
        repository
    }

    fn add_submodule(&self, parent: &Path, repository: &Path, path: &str) -> String {
        let url = repository.to_str().unwrap();
        let output = self.git_output(
            parent,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                url,
                path,
            ],
        );
        assert!(
            output.status.success(),
            "git submodule add failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.git(parent, &["add", "-A"]);
        self.git(
            parent,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "add submodule",
            ],
        );
        self.git(repository, &["rev-parse", "HEAD"])
            .trim()
            .to_owned()
    }

    fn initialize_submodule(&self, parent: &Path, path: &str) {
        let output = self.git_output(
            parent,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "--",
                path,
            ],
        );
        assert!(
            output.status.success(),
            "git submodule update failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn allow_file_protocol(&self) {
        self.git(
            &self.root,
            &["config", "--global", "protocol.file.allow", "always"],
        );
    }

    fn remove_source_submodule(&self, path: &str) {
        let submodule = self.root.join(path);
        if submodule.exists() {
            fs::remove_dir_all(&submodule).unwrap();
        }
        let module_path = PathBuf::from(
            self.git(
                &self.root,
                &["rev-parse", "--git-path", &format!("modules/{path}")],
            )
            .trim(),
        );
        let module_path = if module_path.is_absolute() {
            module_path
        } else {
            self.root.join(module_path)
        };
        if module_path.exists() {
            fs::remove_dir_all(module_path).unwrap();
        }
    }

    fn git_dir(&self, repository: &Path) -> PathBuf {
        let git_dir = PathBuf::from(self.git(repository, &["rev-parse", "--git-dir"]).trim());
        if git_dir.is_absolute() {
            git_dir
        } else {
            repository.join(git_dir)
        }
    }

    fn uses_reference(&self, repository: &Path, reference: &Path) -> bool {
        let alternates = self.git_dir(repository).join("objects/info/alternates");
        let Ok(contents) = fs::read_to_string(alternates) else {
            return false;
        };
        let reference_objects = fs::canonicalize(self.git_dir(reference).join("objects")).unwrap();
        contents.lines().any(|line| {
            fs::canonicalize(line)
                .map(|path| path == reference_objects)
                .unwrap_or(false)
        })
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
            copy_paths: Vec::new(),
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
            copy_paths: Vec::new(),
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
fn parses_repeatable_copy_options_and_propagates_them() {
    assert_eq!(
        parse_worktree_args(&[
            OsString::from("new"),
            OsString::from("--copy"),
            OsString::from("config/settings"),
            OsString::from("-c"),
            OsString::from("cache"),
            OsString::from("-P"),
            OsString::from("feature/api"),
        ])
        .unwrap(),
        WorktreeCommand::New {
            name: "feature/api".to_owned(),
            path_only: true,
            copy_paths: vec![PathBuf::from("config/settings"), PathBuf::from("cache")],
        }
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
    assert!(parse_worktree_args(&[OsString::from("new"), OsString::from("--copy")]).is_err());
    assert!(
        parse_worktree_args(&[
            OsString::from("new"),
            OsString::from("--copy"),
            OsString::from("--path-only"),
            OsString::from("name"),
        ])
        .is_err()
    );
    assert!(
        parse_worktree_args(&[
            OsString::from("new"),
            OsString::from("--copy"),
            OsString::from("../outside"),
            OsString::from("name"),
        ])
        .is_err()
    );
    assert!(
        parse_worktree_args(&[
            OsString::from("new"),
            OsString::from("--copy"),
            OsString::from("config"),
            OsString::from("-c"),
            OsString::from("config/dev"),
            OsString::from("name"),
        ])
        .is_err()
    );
    assert!(
        parse_worktree_args(&[OsString::from("status"), OsString::from("--path-only")]).is_err()
    );
}

#[test]
fn worktree_help_covers_the_workflow() {
    assert!(worktree_help().contains("wt.root"));
    assert!(worktree_help().contains("zetta wt rerere"));
    assert!(worktree_new_help().contains("--copy"));
    assert!(worktree_new_help().contains("--path-only"));
    assert!(worktree_done_help().contains("stage"));
    assert!(worktree_status_help().contains("never creates"));
    assert!(worktree_rerere_help().contains("rerere.autoupdate"));
}

#[cfg(unix)]
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
