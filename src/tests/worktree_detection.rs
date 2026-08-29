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
    let metadata = detect_worktree_metadata(&nested).unwrap().unwrap();
    assert_eq!(metadata.name, "feature/api");
    assert_eq!(metadata.main_root, fs::canonicalize(&fixture.main).unwrap());
}

#[test]
fn ignores_the_main_worktree() {
    let fixture = Fixture::new();
    assert_eq!(detect_worktree_name(&fixture.main).unwrap(), None);
    assert_eq!(detect_worktree_metadata(&fixture.main).unwrap(), None);
}

#[test]
fn ignores_detached_linked_worktrees() {
    let fixture = Fixture::new();
    let linked = fixture.linked("wt/detached", "detached");
    Fixture::git(&linked, &["checkout", "-q", "--detach", "HEAD"]);

    assert_eq!(detect_worktree_name(&linked).unwrap(), None);
    assert_eq!(detect_worktree_metadata(&linked).unwrap(), None);
}

#[test]
fn ignores_linked_worktrees_on_non_worktree_branches() {
    let fixture = Fixture::new();
    let linked = fixture.linked("feature/api", "ordinary");

    assert_eq!(detect_worktree_name(&linked).unwrap(), None);
    assert_eq!(detect_worktree_metadata(&linked).unwrap(), None);
}

#[test]
fn reported_shell_directory_wins_while_a_child_is_foreground() {
    let reported = std::path::PathBuf::from("/shell/worktree");
    let child = std::path::PathBuf::from("/child/switched-source");

    assert_eq!(
        select_current_directory(Some(reported.clone()), Some(child), false, false),
        Some((reported, true))
    );
}

#[test]
fn process_directory_is_used_as_a_non_authoritative_fallback_while_a_child_is_foreground() {
    let child = std::path::PathBuf::from("/child/switched-source");
    assert_eq!(
        select_current_directory(None, Some(child.clone()), false, false),
        Some((child, false))
    );
}

#[test]
fn process_directory_is_used_while_the_shell_is_foreground() {
    let shell = std::path::PathBuf::from("/shell/worktree");
    assert_eq!(
        select_current_directory(None, Some(shell.clone()), true, false),
        Some((shell, true))
    );
}

#[test]
fn process_directory_supersedes_a_stale_report_while_the_shell_is_foreground() {
    let reported = std::path::PathBuf::from("/old/main");
    let shell = std::path::PathBuf::from("/shell/worktree");

    assert_eq!(
        select_current_directory(Some(reported), Some(shell.clone()), true, false),
        Some((shell, true))
    );
}

#[test]
fn msys2_reported_directories_are_normalized_before_selection() {
    let root = Path::new(r"D:\Applications\MSYS2");
    let reported = msys2_path_to_windows(root, "/c/Users/saltw/source/repos/zetta")
        .expect("the MSYS2 path should be native-convertible");

    assert_eq!(
        select_current_directory(Some(reported.clone()), None, false, true),
        Some((reported, true))
    );
}

#[cfg(windows)]
#[test]
fn cygwin_reported_directories_are_normalized_before_selection() {
    let root = Path::new(r"D:\Applications\Cygwin");
    let reported = cygwin_path_to_windows(root, "/cygdrive/c/Users/saltw/source/repos/zetta")
        .expect("the Cygwin path should be native-convertible");
    let process = PathBuf::from(r"C:\Users\saltw");

    assert_eq!(
        select_current_directory(Some(reported.clone()), Some(process), true, true),
        Some((reported, true))
    );
}

#[test]
fn tracked_shell_directory_wins_over_a_stale_process_directory() {
    let reported = PathBuf::from(r"C:\Users\saltw\source\repos\zetta");
    let process = PathBuf::from(r"C:\Users\saltw");

    assert_eq!(
        select_current_directory(Some(reported.clone()), Some(process), true, true),
        Some((reported, true))
    );
}

#[test]
fn title_and_breadcrumb_events_refresh_worktree_detection() {
    assert!(terminal_event_requires_worktree_detection(
        &TerminalEvent::TitleChanged
    ));
    assert!(terminal_event_requires_worktree_detection(
        &TerminalEvent::BreadcrumbsChanged
    ));
    assert!(!terminal_event_requires_worktree_detection(
        &TerminalEvent::Wakeup
    ));
}

#[test]
fn scheduled_shell_directory_remains_current_while_a_child_hides_the_cwd() {
    let shell = Path::new("/shell/worktree");

    assert!(worktree_detection_directory_is_current(
        Some(shell),
        None,
        shell,
    ));
}

#[test]
fn a_new_shell_directory_invalidates_an_old_detection() {
    assert!(!worktree_detection_directory_is_current(
        Some(Path::new("/shell/old")),
        Some(Path::new("/shell/new")),
        Path::new("/shell/old"),
    ));
}
