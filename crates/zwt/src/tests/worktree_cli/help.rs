use super::*;

#[test]
fn worktree_help_covers_the_workflow() {
    assert!(worktree_help().contains("wt.root"));
    assert!(worktree_help().contains("zwt sync [COMMIT]"));
    assert!(worktree_help().contains("zwt config"));
    assert!(!worktree_help().contains("zwt rerere"));
    assert!(worktree_help().contains("zwt abort [OPTIONS]"));
    assert!(worktree_new_help().contains("--copy"));
    assert!(worktree_new_help().contains("--path-only"));
    assert!(worktree_new_help().contains("phase progress"));
    assert!(worktree_done_help().contains("stage"));
    assert!(worktree_abort_help().contains("never rebases"));
    assert!(worktree_abort_help().contains("source worktree\nmay be dirty"));
    assert!(worktree_abort_help().contains("--path-only"));
    assert!(worktree_status_help().contains("never creates"));
    assert!(worktree_status_help().contains("nested paths"));
    assert!(worktree_status_help().contains("copy-on-write"));
    assert!(worktree_sync_help().contains("merge-base"));
    assert!(worktree_config_help().contains("rebase.autoStash"));
}

#[test]
fn help_and_diagnostics_use_the_selected_invocation_name() {
    assert!(worktree_help_for(WorktreeInvocation::Standalone).contains("Usage: zwt <COMMAND>"));
    assert!(worktree_help_for(WorktreeInvocation::Zetta).contains("Usage: zetta wt <COMMAND>"));
    assert_eq!(WorktreeInvocation::Standalone.command(), "zwt");
    assert_eq!(WorktreeInvocation::Zetta.command(), "zetta wt");
    assert!(
        parse_worktree_args_for(&[OsString::from("unknown")], WorktreeInvocation::Zetta,)
            .unwrap_err()
            .to_string()
            .contains("run zetta wt --help")
    );
}
