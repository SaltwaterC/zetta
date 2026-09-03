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

pub(super) struct GitFixture {
    pub(super) _tempdir: TempDir,
    pub(super) root: PathBuf,
    pub(super) global_config: PathBuf,
}

pub(super) struct EnvironmentGuard {
    test_directory: Option<PathBuf>,
    test_git_config: Option<(OsString, OsString)>,
}

pub(super) fn empty_system_config() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        replace_test_current_directory(self.test_directory.take());
        replace_test_git_config(self.test_git_config.take());
    }
}

impl GitFixture {
    pub(super) fn new() -> Self {
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

    pub(super) fn enter(&self, path: &Path) -> EnvironmentGuard {
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

    pub(super) fn git(&self, path: &Path, arguments: &[&str]) -> String {
        let output = self.git_output(path, arguments);
        assert!(
            output.status.success(),
            "git {:?} failed:\n{}",
            arguments,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub(super) fn git_output(&self, path: &Path, arguments: &[&str]) -> Output {
        Command::new("git")
            .current_dir(path)
            .env("GIT_CONFIG_GLOBAL", &self.global_config)
            .env("GIT_CONFIG_SYSTEM", empty_system_config())
            .args(arguments)
            .output()
            .unwrap()
    }

    pub(super) fn commit(&self, path: &Path, file: &str, contents: &str, message: &str) {
        fs::write(path.join(file), contents).unwrap();
        self.git(path, &["add", file]);
        self.git(
            path,
            &["-c", "commit.gpgsign=false", "commit", "-qm", message],
        );
    }

    pub(super) fn default_root(&self) -> PathBuf {
        self.root.parent().unwrap().join(format!(
            "{}-worktrees",
            self.root.file_name().unwrap().to_string_lossy()
        ))
    }

    pub(super) fn worktree_path(&self, name: &str) -> PathBuf {
        self.default_root().join(name)
    }

    pub(super) fn create_worktree(&self, name: &str) -> PathBuf {
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

    #[cfg(feature = "recursive-submodules")]
    pub(super) fn create_repository(&self, name: &str) -> PathBuf {
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

    #[cfg(feature = "recursive-submodules")]
    pub(super) fn add_submodule(&self, parent: &Path, repository: &Path, path: &str) -> String {
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

    #[cfg(feature = "recursive-submodules")]
    pub(super) fn initialize_submodule(&self, parent: &Path, path: &str) {
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

    #[cfg(feature = "recursive-submodules")]
    pub(super) fn allow_file_protocol(&self) {
        self.git(
            &self.root,
            &["config", "--global", "protocol.file.allow", "always"],
        );
    }

    #[cfg(feature = "recursive-submodules")]
    pub(super) fn remove_source_submodule(&self, path: &str) {
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

    #[cfg(feature = "recursive-submodules")]
    pub(super) fn git_dir(&self, repository: &Path) -> PathBuf {
        let git_dir = PathBuf::from(self.git(repository, &["rev-parse", "--git-dir"]).trim());
        if git_dir.is_absolute() {
            git_dir
        } else {
            repository.join(git_dir)
        }
    }

    #[cfg(feature = "recursive-submodules")]
    pub(super) fn uses_reference(&self, repository: &Path, reference: &Path) -> bool {
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

pub(super) fn test_lock() -> MutexGuard<'static, ()> {
    WORKTREE_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap()
}

pub(super) fn in_directory<T>(
    fixture: &GitFixture,
    path: &Path,
    operation: impl FnOnce() -> T,
) -> T {
    let _lock = test_lock();
    let _environment = fixture.enter(path);
    operation()
}

pub(super) fn capture_worktree_name_requests<T>(
    operation: impl FnOnce() -> T,
) -> (T, Vec<Option<String>>) {
    let previous = replace_test_worktree_name_requests(Some(Vec::new()));
    let result = operation();
    let requests = replace_test_worktree_name_requests(previous).unwrap();
    (result, requests)
}

#[test]
fn originating_tab_target_requires_positive_numeric_ids() {
    assert_eq!(parse_originating_tab_target("123", "456"), Some((123, 456)));
    assert_eq!(parse_originating_tab_target("0", "456"), None);
    assert_eq!(parse_originating_tab_target("123", "0"), None);
    assert_eq!(parse_originating_tab_target("not-a-pid", "456"), None);
}
