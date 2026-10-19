use super::*;

#[test]
fn copy_paths_are_comma_separated_and_trimmed() {
    assert_eq!(
        parse_copy_paths(" .env , local-settings/,,  "),
        [PathBuf::from(".env"), PathBuf::from("local-settings/")]
    );
    assert!(parse_copy_paths("").is_empty());
    assert!(parse_copy_paths(" , ").is_empty());
}

#[test]
fn a_managed_worktree_is_labelled_by_its_name_and_keeps_its_source() {
    let worktree = sidebar_worktree(zwt::LinkedWorktree {
        path: PathBuf::from("/src/api.worktrees/fix"),
        branch: Some("wt/fix".to_owned()),
        name: Some("fix".to_owned()),
        source_branch: Some("main".to_owned()),
    });

    assert_eq!(worktree.label, "fix");
    assert_eq!(worktree.source_branch.as_deref(), Some("main"));
    assert!(worktree.is_managed());
}

#[test]
fn an_unmanaged_worktree_is_labelled_by_its_branch_or_directory() {
    let by_branch = sidebar_worktree(zwt::LinkedWorktree {
        path: PathBuf::from("/src/by-hand"),
        branch: Some("experiment".to_owned()),
        name: None,
        source_branch: None,
    });
    assert_eq!(by_branch.label, "experiment");
    assert!(!by_branch.is_managed());

    let detached = sidebar_worktree(zwt::LinkedWorktree {
        path: PathBuf::from("/src/detached"),
        branch: None,
        name: None,
        source_branch: None,
    });
    assert_eq!(detached.label, "detached");
    assert!(!detached.is_managed());
}

#[test]
fn a_wt_branch_without_a_recorded_source_offers_no_done_or_abort() {
    let worktree = sidebar_worktree(zwt::LinkedWorktree {
        path: PathBuf::from("/src/api.worktrees/manual"),
        branch: Some("wt/manual".to_owned()),
        name: Some("manual".to_owned()),
        source_branch: None,
    });

    assert_eq!(worktree.label, "manual");
    assert!(!worktree.is_managed());
}

#[test]
fn the_create_dialog_types_into_the_selected_field_and_not_while_working() {
    let mut prompt = WorktreePrompt {
        project: PathBuf::from("/src/api"),
        kind: WorktreePromptKind::Create {
            name: TextField::default(),
            copy_paths: TextField::default(),
            field: WorktreeField::CopyPaths,
        },
        working: false,
        error: None,
        generation: 1,
    };

    prompt.text_field_mut().unwrap().insert(".env");
    let WorktreePromptKind::Create {
        name, copy_paths, ..
    } = &prompt.kind
    else {
        unreachable!();
    };
    assert_eq!(name.text, "");
    assert_eq!(copy_paths.text, ".env");

    prompt.working = true;
    assert!(prompt.text_field_mut().is_none());
}

#[test]
fn finishing_messages_name_both_branches() {
    let finished = zwt::FinishedWorktree {
        worktree: PathBuf::from("/src/api.worktrees/fix"),
        branch: "wt/fix".to_owned(),
        source_branch: "main".to_owned(),
        source_path: PathBuf::from("/src/api"),
    };

    let integrated = finished_message(WorktreeFinish::Integrate, &finished);
    assert!(integrated.contains("wt/fix") && integrated.contains("main"));
    let discarded = finished_message(WorktreeFinish::Discard, &finished);
    assert!(discarded.contains("wt/fix") && discarded.contains("unchanged"));
}
