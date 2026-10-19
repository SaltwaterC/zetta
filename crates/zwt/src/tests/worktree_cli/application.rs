use super::*;
use crate::worktree_cli::tests::*;

#[test]
fn lists_managed_and_unmanaged_linked_worktrees() {
    let fixture = GitFixture::new();
    let managed = fixture.create_worktree("feature/api");
    let unmanaged = fixture.root.parent().unwrap().join("by-hand");
    fixture.git(
        &fixture.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "by-hand",
            unmanaged.to_str().unwrap(),
        ],
    );

    let root = fixture.root.clone();
    let listing = in_directory(&fixture, &root, || list_worktrees(&root)).unwrap();

    assert!(same_path(&listing.root, &fixture.root));
    assert_eq!(listing.current_branch.as_deref(), Some("main"));
    assert_eq!(listing.worktree_root, Some(fixture.default_root()));
    assert_eq!(listing.linked.len(), 2);
    let api = listing
        .linked
        .iter()
        .find(|worktree| same_path(&worktree.path, &managed))
        .unwrap();
    assert_eq!(api.name.as_deref(), Some("feature/api"));
    assert_eq!(api.source_branch.as_deref(), Some("main"));
    assert!(api.is_managed());
    let by_hand = listing
        .linked
        .iter()
        .find(|worktree| same_path(&worktree.path, &unmanaged))
        .unwrap();
    assert_eq!(by_hand.branch.as_deref(), Some("by-hand"));
    assert!(!by_hand.is_managed());
}

#[test]
fn a_hand_made_wt_branch_without_metadata_is_not_managed() {
    let fixture = GitFixture::new();
    let path = fixture.worktree_path("manual");
    fixture.git(
        &fixture.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt/manual",
            path.to_str().unwrap(),
        ],
    );

    let root = fixture.root.clone();
    let listing = in_directory(&fixture, &root, || list_worktrees(&root)).unwrap();

    let [worktree] = listing.linked.as_slice() else {
        panic!("expected one linked worktree, got {:?}", listing.linked);
    };
    assert_eq!(worktree.name.as_deref(), Some("manual"));
    assert_eq!(worktree.source_branch, None);
    assert!(!worktree.is_managed());
}

#[test]
fn the_application_lifecycle_never_requests_an_originating_tab_title() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    let (results, requests) = in_directory(&fixture, &root, || {
        capture_worktree_name_requests(|| {
            let created = create_worktree(&root, "lifecycle", &[]).unwrap();
            fixture.commit(&created.path, "change", "work\n", "work");
            let synced = sync_worktree(&created.path).unwrap();
            let finished = integrate_worktree(&created.path).unwrap();
            (created, synced, finished)
        })
    });
    let (created, synced, finished) = results;

    assert!(requests.is_empty());
    assert_eq!(created.name, "lifecycle");
    assert_eq!(created.branch, "wt/lifecycle");
    assert_eq!(created.source_branch, "main");
    assert!(same_path(
        &created.path,
        &fixture.worktree_path("lifecycle")
    ));
    assert!(matches!(synced, SyncedWorktree::Synchronized { .. }));
    assert_eq!(finished.branch, "wt/lifecycle");
    assert!(same_path(&finished.source_path, &fixture.root));
    assert!(!created.path.exists());
    assert_eq!(
        fixture.git(&fixture.root, &["log", "-1", "--format=%s"]),
        "work\n"
    );
}

#[test]
fn discarding_removes_the_worktree_and_keeps_the_source() {
    let fixture = GitFixture::new();
    let worktree = fixture.create_worktree("throwaway");
    fixture.commit(&worktree, "change", "discarded\n", "discarded");

    let root = fixture.root.clone();
    let finished = in_directory(&fixture, &root, || discard_worktree(&worktree)).unwrap();

    assert_eq!(finished.source_branch, "main");
    assert!(!worktree.exists());
    assert_eq!(
        fixture.git(&fixture.root, &["log", "-1", "--format=%s"]),
        "initial\n"
    );
}

#[test]
fn creating_reports_the_reason_a_name_is_refused() {
    let fixture = GitFixture::new();
    let root = fixture.root.clone();
    let error = in_directory(&fixture, &root, || {
        create_worktree(&root, "bad..name", &[]).unwrap_err()
    });

    assert!(!fixture.worktree_path("bad..name").exists());
    assert!(!format!("{error:#}").is_empty());
}
