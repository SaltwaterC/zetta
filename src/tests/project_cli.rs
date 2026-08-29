use super::*;
use std::{path::Path, process::Command};

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

fn os(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn parses_every_project_operation_and_both_path_forms() {
    assert_eq!(
        parse_project_args(&os(&["add", "workspace"])).unwrap(),
        ProjectCommand::Add {
            path: Some(PathBuf::from("workspace"))
        }
    );
    assert_eq!(
        parse_project_args(&os(&["remove", "-p", "workspace"])).unwrap(),
        ProjectCommand::Remove {
            path: Some(PathBuf::from("workspace"))
        }
    );
    assert_eq!(
        parse_project_args(&os(&["open", "--path", "workspace"])).unwrap(),
        ProjectCommand::Open {
            path: Some(PathBuf::from("workspace"))
        }
    );
    assert_eq!(
        parse_project_args(&os(&["list"])).unwrap(),
        ProjectCommand::List
    );
}

#[test]
fn project_parser_rejects_ambiguous_or_unknown_arguments() {
    assert!(parse_project_args(&[]).is_err());
    assert!(parse_project_args(&os(&["unknown"])).is_err());
    assert!(parse_project_args(&os(&["list", "extra"])).is_err());
    assert!(parse_project_args(&os(&["add", "one", "two"])).is_err());
    assert!(parse_project_args(&os(&["open", "--unknown"])).is_err());
}

#[test]
fn project_help_documents_registry_and_preserved_configuration() {
    assert!(project_help(None).contains("zetta project <COMMAND>"));
    assert!(project_help(Some("add")).contains(".zetta/config.json"));
    assert!(project_help(Some("remove")).contains("never deleted"));
    assert!(project_help(Some("open")).contains("new active tab"));
}

#[test]
fn project_open_target_uses_the_main_root_and_preserves_a_managed_worktree_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let main = temporary.path().join("project");
    let linked = temporary.path().join("project-worktree");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["config", "user.email", "test@example.invalid"]);
    git(&main, &["config", "user.name", "Zetta Test"]);
    fs::write(main.join("file"), "base\n").unwrap();
    git(&main, &["add", "file"]);
    git(
        &main,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "initial"],
    );
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt/feature",
            linked.to_str().unwrap(),
        ],
    );
    let child = linked.join("src");
    fs::create_dir(&child).unwrap();

    let mut registry = ProjectRegistry::load_from(temporary.path().join("registry.json")).unwrap();
    registry.add(&main).unwrap();
    let main = fs::canonicalize(main).unwrap();
    let child = fs::canonicalize(child).unwrap();

    let target = resolve_open_target_in_registry(&child, &registry).unwrap();
    assert_eq!(target.root, main);
    assert_eq!(target.working_directory, Some(child));
}
