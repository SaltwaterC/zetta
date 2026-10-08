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

/// Writes a self-contained linked-worktree lookalike in `directory` whose
/// `commondir` names `common_gitdir`, with every other file it needs kept
/// inside `directory` itself.
fn plant_fake_linked_worktree(directory: &Path, common_gitdir: &Path) {
    let gitdir = directory.join("metadata");
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(
        directory.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    fs::write(
        gitdir.join("commondir"),
        format!("{}\n", common_gitdir.display()),
    )
    .unwrap();
    fs::write(
        gitdir.join("gitdir"),
        format!("{}\n", directory.join(".git").display()),
    )
    .unwrap();
    fs::write(gitdir.join("HEAD"), "ref: refs/heads/wt/borrowed\n").unwrap();
}

#[test]
fn a_fake_worktree_pointing_at_another_repository_is_not_detected() {
    let fixture = Fixture::new();
    let outsider = TempDir::new().unwrap();
    let fake = outsider.path().join("fake");
    plant_fake_linked_worktree(&fake, &fixture.main.join(".git"));

    assert_eq!(detect_worktree_metadata(&fake).unwrap(), None);
    assert_eq!(detect_worktree_name(&fake).unwrap(), None);
}

#[test]
fn a_fake_worktree_gitdir_beside_the_common_worktrees_directory_is_not_detected() {
    // The gitdir sits inside the registered repository's own `.git`, but not
    // under `worktrees/`, so Git never created it as a linked worktree.
    let fixture = Fixture::new();
    let outsider = TempDir::new().unwrap();
    let fake = outsider.path().join("fake");
    let common_gitdir = fixture.main.join(".git");
    plant_fake_linked_worktree(&fake, &common_gitdir);
    let planted = common_gitdir.join("not-worktrees").join("borrowed");
    fs::create_dir_all(planted.parent().unwrap()).unwrap();
    fs::rename(fake.join("metadata"), &planted).unwrap();
    fs::write(
        fake.join(".git"),
        format!("gitdir: {}\n", planted.display()),
    )
    .unwrap();

    assert_eq!(detect_worktree_metadata(&fake).unwrap(), None);
}

#[test]
fn a_worktree_added_by_git_with_a_relative_commondir_is_detected() {
    let fixture = Fixture::new();
    let linked = fixture.linked("wt/relative", "relative");
    let gitdir = fs::canonicalize(fixture.main.join(".git/worktrees/relative")).unwrap();
    // Git writes `../..`; pin that here so the containment check is exercised
    // against Git's own relative layout, whatever this Git version wrote.
    fs::write(gitdir.join("commondir"), "../..\n").unwrap();

    let metadata = detect_worktree_metadata(&linked).unwrap().unwrap();
    assert_eq!(metadata.name, "relative");
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
