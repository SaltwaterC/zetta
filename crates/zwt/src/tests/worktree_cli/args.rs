use super::*;

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
        parse_worktree_args(&[OsString::from("abort"), OsString::from("-P")]).unwrap(),
        WorktreeCommand::Abort { path_only: true }
    );
    assert_eq!(
        parse_worktree_args(&[OsString::from("status")]).unwrap(),
        WorktreeCommand::Status
    );
    assert_eq!(
        parse_worktree_args(&[OsString::from("sync")]).unwrap(),
        WorktreeCommand::Sync { commit: None }
    );
    assert_eq!(
        parse_worktree_args(&[OsString::from("sync"), OsString::from("main~2")]).unwrap(),
        WorktreeCommand::Sync {
            commit: Some("main~2".to_owned())
        }
    );
    assert_eq!(
        parse_worktree_args(&[OsString::from("config")]).unwrap(),
        WorktreeCommand::Config
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
    assert!(parse_worktree_args(&[OsString::from("abort"), OsString::from("--unknown")]).is_err());
    assert!(
        parse_worktree_args(&[
            OsString::from("sync"),
            OsString::from("HEAD"),
            OsString::from("HEAD~1"),
        ])
        .is_err()
    );
    assert!(parse_worktree_args(&[OsString::from("sync"), OsString::from("--onto")]).is_err());
    assert!(parse_worktree_args(&[OsString::from("config"), OsString::from("extra")]).is_err());
}
