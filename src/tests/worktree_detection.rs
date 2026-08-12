use super::*;
use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

struct Fixture {
    _temporary: TempDir,
    main: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let main = temporary.path().join("project");
        fs::create_dir(&main).unwrap();
        Self::git(&main, &["init", "-q", "-b", "main"]);
        Self::git(&main, &["config", "user.email", "test@example.invalid"]);
        Self::git(&main, &["config", "user.name", "Zetta Test"]);
        fs::write(main.join("file"), "base\n").unwrap();
        Self::git(&main, &["add", "file"]);
        Self::git(
            &main,
            &["-c", "commit.gpgsign=false", "commit", "-qm", "initial"],
        );
        Self {
            _temporary: temporary,
            main,
        }
    }

    fn linked(&self, branch: &str, directory_name: &str) -> std::path::PathBuf {
        let directory = self.main.parent().unwrap().join(directory_name);
        Self::git(
            &self.main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                directory.to_str().unwrap(),
            ],
        );
        directory
    }

    fn git(directory: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .current_dir(directory)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn detects_nested_worktree_names_from_subdirectories() {
    let fixture = Fixture::new();
    let linked = fixture.linked("wt/feature/api", "linked");
    let nested = linked.join("src").join("api");
    fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        detect_worktree_name(&nested).unwrap(),
        Some("feature/api".to_owned())
    );
}

#[test]
fn ignores_the_main_worktree() {
    let fixture = Fixture::new();
    assert_eq!(detect_worktree_name(&fixture.main).unwrap(), None);
}

#[test]
fn ignores_detached_linked_worktrees() {
    let fixture = Fixture::new();
    let linked = fixture.linked("wt/detached", "detached");
    Fixture::git(&linked, &["checkout", "-q", "--detach", "HEAD"]);

    assert_eq!(detect_worktree_name(&linked).unwrap(), None);
}

#[test]
fn ignores_linked_worktrees_on_non_worktree_branches() {
    let fixture = Fixture::new();
    let linked = fixture.linked("feature/api", "ordinary");

    assert_eq!(detect_worktree_name(&linked).unwrap(), None);
}

#[test]
fn reported_shell_directory_wins_while_a_child_is_foreground() {
    let reported = std::path::PathBuf::from("/shell/worktree");
    let child = std::path::PathBuf::from("/child/switched-source");

    assert_eq!(
        select_worktree_detection_directory(Some(reported.clone()), Some(child), false),
        Some(reported)
    );
}

#[test]
fn process_directory_is_ignored_while_a_child_is_foreground() {
    assert_eq!(
        select_worktree_detection_directory(
            None,
            Some(std::path::PathBuf::from("/child/switched-source")),
            false,
        ),
        None
    );
}

#[test]
fn process_directory_is_used_while_the_shell_is_foreground() {
    let shell = std::path::PathBuf::from("/shell/worktree");
    assert_eq!(
        select_worktree_detection_directory(None, Some(shell.clone()), true),
        Some(shell)
    );
}
