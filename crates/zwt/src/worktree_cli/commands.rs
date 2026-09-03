//! One function per worktree command.
//!
//! Each is a sequence of git operations with a rollback for what it already
//! did, because a half-created worktree — a branch made, a directory left
//! behind, submodules half-initialized — is worse than a failure that leaves
//! the repository as it was found.

use super::*;

pub(super) fn request_originating_worktree_name(name: Option<&str>) {
    #[cfg(test)]
    if TEST_WORKTREE_NAME_REQUESTS.with(|requests| {
        let mut requests = requests.borrow_mut();
        let Some(requests) = requests.as_mut() else {
            return false;
        };
        requests.push(name.map(str::to_owned));
        true
    }) {
        return;
    }

    let Some((process_id, attention_id)) = originating_tab_target() else {
        return;
    };
    let _ = request_process_worktree_name(
        process_id,
        WorktreeNameRequest {
            attention_id,
            name: name.map(str::to_owned),
        },
    );
}

pub(super) fn originating_tab_target() -> Option<(u32, u64)> {
    parse_originating_tab_target(
        &env::var("ZETTA_PROCESS_ID").ok()?,
        &env::var("ZETTA_ATTENTION_ID").ok()?,
    )
}

pub(super) fn parse_originating_tab_target(
    process_id: &str,
    attention_id: &str,
) -> Option<(u32, u64)> {
    let process_id = process_id.parse().ok()?;
    let attention_id = attention_id.parse().ok()?;
    (process_id != 0 && attention_id != 0).then_some((process_id, attention_id))
}

pub(super) fn run_new(
    name: &str,
    path_only: bool,
    copy_paths: &[PathBuf],
    current_directory: Option<&Path>,
    invocation: WorktreeInvocation,
) -> Result<()> {
    let command = invocation.command();
    let repository = discover_repository(current_directory)?;
    anyhow::ensure!(
        !rebase_in_progress(&repository.current_worktree)?,
        "{command} new cannot run while the current worktree has a rebase in progress"
    );
    let source_branch = repository.current_branch.as_deref().with_context(|| {
        format!("{command} new requires the current worktree to have an attached branch")
    })?;
    anyhow::ensure!(
        !repository.current_entry.bare,
        "{command} new requires a non-bare Git worktree"
    );

    let branch = format!("{WORKTREE_BRANCH_PREFIX}{name}");
    validate_branch_name(&repository.current_worktree, &branch)?;
    ensure_branch_is_available(&repository.current_worktree, &branch)?;
    #[cfg(feature = "recursive-submodules")]
    let source_submodules = gitlink_paths(&repository.current_worktree, source_branch)?;
    validate_copy_sources(&repository.current_worktree, copy_paths)?;

    let resolved_root = resolved_worktree_root(&repository.current_worktree, &repository.root)?;
    let destination = destination_path(&resolved_root.path, name);
    validate_destination(&resolved_root.path, name, &destination)?;
    let created_directories = create_destination_parent(&destination)?;

    let add_arguments = vec![
        os("worktree"),
        os("add"),
        os("-b"),
        os(&branch),
        destination.as_os_str().to_os_string(),
        os(source_branch),
    ];
    eprintln!("Creating worktree at {}...", destination.display());
    let add_output = run_git(Some(&repository.current_worktree), &add_arguments)?;
    if !add_output.status.success() {
        remove_empty_directories(&created_directories);
        return Err(git_error("git worktree add", &add_output));
    }

    let metadata_key = metadata_key(&branch);
    #[cfg(feature = "recursive-submodules")]
    if !source_submodules.is_empty() {
        eprintln!("Initializing submodules...");
        if let Err(error) = initialize_submodules(
            &destination,
            &repository.current_worktree,
            &source_submodules,
        ) {
            let rollback_errors = rollback_new_worktree(
                &repository.current_worktree,
                &destination,
                &branch,
                &metadata_key,
                &created_directories,
            );
            let mut message = format!("initializing worktree submodules failed: {error}");
            if !rollback_errors.is_empty() {
                message.push_str("; rollback also failed: ");
                message.push_str(&rollback_errors.join("; "));
            }
            return Err(anyhow::anyhow!(message));
        }
    }

    if !copy_paths.is_empty() {
        eprintln!("Copying requested paths...");
    }
    if let Err(error) = copy_worktree_paths(&repository.current_worktree, &destination, copy_paths)
    {
        let rollback_errors = rollback_new_worktree(
            &repository.current_worktree,
            &destination,
            &branch,
            &metadata_key,
            &created_directories,
        );
        let mut message = format!("copying worktree paths failed: {error}");
        if !rollback_errors.is_empty() {
            message.push_str("; rollback also failed: ");
            message.push_str(&rollback_errors.join("; "));
        }
        return Err(anyhow::anyhow!(message));
    }

    let metadata_arguments = vec![
        os("config"),
        os("--local"),
        os(&metadata_key),
        os(source_branch),
    ];
    eprintln!("Recording worktree metadata...");
    let metadata_output = run_git(Some(&repository.current_worktree), &metadata_arguments)?;
    if !metadata_output.status.success() {
        let rollback_errors = rollback_new_worktree(
            &repository.current_worktree,
            &destination,
            &branch,
            &metadata_key,
            &created_directories,
        );
        let mut message = format!(
            "recording worktree source metadata failed: {}",
            git_diagnostic(&metadata_output)
        );
        if !rollback_errors.is_empty() {
            message.push_str("; rollback also failed: ");
            message.push_str(&rollback_errors.join("; "));
        }
        return Err(anyhow::anyhow!(message));
    }

    request_originating_worktree_name(Some(name));
    if path_only {
        println!("{}", destination.display());
    } else {
        println!(
            "Created worktree {} at {} on branch {} (based on {}).",
            name,
            destination.display(),
            branch,
            source_branch
        );
    }
    Ok(())
}

pub(super) fn run_done(
    path_only: bool,
    current_directory: Option<&Path>,
    invocation: WorktreeInvocation,
) -> Result<()> {
    let command = invocation.command();
    let repository = discover_repository(current_directory)?;
    let current_branch = repository.current_branch.clone().context(format!(
        "{command} done requires an attached branch; detached worktrees cannot be integrated"
    ))?;
    anyhow::ensure!(
        current_branch.starts_with(WORKTREE_BRANCH_PREFIX)
            && current_branch.len() > WORKTREE_BRANCH_PREFIX.len(),
        "{command} done only operates on wt/* worktree branches"
    );
    anyhow::ensure!(
        !repository.current_entry.bare
            && !same_path(&repository.current_worktree, &repository.root),
        "{command} done must be run from a linked Git worktree, not the repository main worktree"
    );

    let metadata_key = metadata_key(&current_branch);
    let source_branch = read_metadata(&repository.current_worktree, &metadata_key)?
        .context("the current wt/* branch has no recorded source branch metadata")?;
    anyhow::ensure!(
        !source_branch.is_empty(),
        "the current wt/* branch has an empty recorded source branch"
    );

    // Validate the source before touching the current branch, then repeat the
    // check after rebasing to catch a source-worktree change during the
    // operation.
    source_worktree(&repository, &source_branch)?;

    let rebasing = rebase_in_progress(&repository.current_worktree)?;
    if rebasing {
        continue_rebase(&repository.current_worktree, invocation)?;
    } else {
        ensure_clean(&repository.current_worktree, "current worktree")?;
        let rebase_arguments = vec![os("rebase"), os(&source_branch)];
        let rebase_output = run_git(Some(&repository.current_worktree), &rebase_arguments)?;
        if !rebase_output.status.success() {
            if rebase_in_progress(&repository.current_worktree)? {
                return Err(conflict_error(&rebase_output, invocation));
            }
            return Err(git_error("git rebase", &rebase_output));
        }
    }

    anyhow::ensure!(
        !rebase_in_progress(&repository.current_worktree)?,
        "the worktree rebase is still in progress; resolve conflicts, stage the resolutions with git add, and rerun {command} done"
    );
    ensure_clean(
        &repository.current_worktree,
        "current worktree after rebase",
    )?;
    #[cfg(feature = "recursive-submodules")]
    let current_commit_has_submodules =
        !gitlink_paths(&repository.current_worktree, "HEAD")?.is_empty();
    #[cfg(not(feature = "recursive-submodules"))]
    let current_commit_has_submodules = false;

    // Re-read the source worktree after rebasing. This catches a source branch
    // switch or newly-created changes before the integration mutates it.
    let source = source_worktree(&repository, &source_branch)?;
    let source_path = source.path.clone();
    let merge_arguments = vec![os("merge"), os("--ff-only"), os("--"), os(&current_branch)];
    let merge_output = run_git(Some(&source_path), &merge_arguments)?;
    if !merge_output.status.success() {
        return Err(git_error("git merge --ff-only", &merge_output));
    }

    let mut remove_arguments = vec![os("worktree"), os("remove")];
    if current_commit_has_submodules {
        remove_arguments.push(os("--force"));
    }
    remove_arguments.extend([
        os("--"),
        repository.current_worktree.as_os_str().to_os_string(),
    ]);
    let remove_output = run_git(Some(&source_path), &remove_arguments)?;
    if !remove_output.status.success() {
        return Err(git_error("git worktree remove", &remove_output));
    }

    let delete_arguments = vec![os("branch"), os("-d"), os("--"), os(&current_branch)];
    let delete_output = run_git(Some(&source_path), &delete_arguments)?;
    if !delete_output.status.success() {
        return Err(git_error("git branch -d", &delete_output));
    }

    let unset_arguments = vec![
        os("config"),
        os("--local"),
        os("--unset-all"),
        os(&metadata_key),
    ];
    let unset_output = run_git(Some(&source_path), &unset_arguments)?;
    if !unset_output.status.success() && unset_output.status.code() != Some(5) {
        return Err(git_error("git config --unset-all", &unset_output));
    }

    request_originating_worktree_name(None);
    if path_only {
        println!("{}", source_path.display());
    } else {
        println!(
            "Integrated {} into {} and removed the temporary worktree {}.",
            current_branch,
            source_branch,
            repository.current_worktree.display()
        );
    }
    Ok(())
}

pub(super) fn run_abort(
    path_only: bool,
    current_directory: Option<&Path>,
    invocation: WorktreeInvocation,
) -> Result<()> {
    let command = invocation.command();
    let repository = discover_repository(current_directory)?;
    let current_branch = repository.current_branch.clone().context(format!(
        "{command} abort requires an attached branch; detached worktrees cannot be aborted"
    ))?;
    anyhow::ensure!(
        current_branch.starts_with(WORKTREE_BRANCH_PREFIX)
            && current_branch.len() > WORKTREE_BRANCH_PREFIX.len(),
        "{command} abort only operates on wt/* worktree branches"
    );
    anyhow::ensure!(
        !repository.current_entry.bare
            && !same_path(&repository.current_worktree, &repository.root),
        "{command} abort must be run from a linked Git worktree, not the repository main worktree"
    );

    let metadata_key = metadata_key(&current_branch);
    let source_branch = read_metadata(&repository.current_worktree, &metadata_key)?
        .context("the current wt/* branch has no recorded source branch metadata")?;
    anyhow::ensure!(
        !source_branch.is_empty(),
        "the current wt/* branch has an empty recorded source branch"
    );

    // Validate every source-worktree invariant before removing the current
    // worktree. Unlike done, abort deliberately permits source dirtiness.
    let source = source_worktree_without_cleanliness(&repository, &source_branch)?;
    let source_path = source.path.clone();

    let remove_arguments = vec![
        os("worktree"),
        os("remove"),
        os("--force"),
        os("--"),
        repository.current_worktree.as_os_str().to_os_string(),
    ];
    let remove_output = run_git(Some(&source_path), &remove_arguments)?;
    if !remove_output.status.success() {
        return Err(git_error("git worktree remove --force", &remove_output));
    }

    let delete_arguments = vec![os("branch"), os("-D"), os("--"), os(&current_branch)];
    let delete_output = run_git(Some(&source_path), &delete_arguments)?;
    if !delete_output.status.success() {
        return Err(git_error("git branch -D", &delete_output));
    }

    let unset_arguments = vec![
        os("config"),
        os("--local"),
        os("--unset-all"),
        os(&metadata_key),
    ];
    let unset_output = run_git(Some(&source_path), &unset_arguments)?;
    if !unset_output.status.success() && unset_output.status.code() != Some(5) {
        return Err(git_error("git config --unset-all", &unset_output));
    }

    request_originating_worktree_name(None);
    if path_only {
        println!("{}", source_path.display());
    } else {
        println!(
            "Aborted {} and removed the temporary worktree {} without changing {}.",
            current_branch,
            repository.current_worktree.display(),
            source_branch
        );
    }
    Ok(())
}

pub(super) fn run_status(
    current_directory: Option<&Path>,
    _invocation: WorktreeInvocation,
) -> Result<()> {
    let repository = discover_repository(current_directory)?;
    let resolved_root = resolved_worktree_root(&repository.current_worktree, &repository.root)?;
    print!("{}", status_report(&repository, &resolved_root)?);
    Ok(())
}

pub(super) fn status_report(
    repository: &Repository,
    resolved_root: &ResolvedRoot,
) -> Result<String> {
    let branch = repository.current_branch.as_deref().unwrap_or("<detached>");
    let source_branch = if let Some(branch) = repository.current_branch.as_deref() {
        read_metadata(&repository.current_worktree, &metadata_key(branch))?
            .unwrap_or_else(|| "<none>".to_owned())
    } else {
        "<none>".to_owned()
    };
    let branch_state = if repository.current_branch.is_some() {
        "attached"
    } else {
        "detached"
    };
    let root_kind = if resolved_root.configured {
        "configured"
    } else {
        "default"
    };
    #[cfg(feature = "recursive-submodules")]
    let submodule_paths = all_gitlink_paths(&repository.current_worktree, "HEAD")?;
    #[cfg(not(feature = "recursive-submodules"))]
    let submodule_paths = Vec::<PathBuf>::new();
    let cow_status = if cow_copy_supported(&repository.current_worktree, &resolved_root.path) {
        "available"
    } else {
        "unavailable"
    };

    let mut report = format!(
        "Repository root: {}\nCurrent worktree: {}\nCurrent branch: {branch}\nCurrent branch state: {branch_state}\nRecorded source branch: {source_branch}\nResolved wt.root: {} ({root_kind})\n",
        repository.root.display(),
        repository.current_worktree.display(),
        resolved_root.path.display()
    );
    if submodule_paths.is_empty() {
        report.push_str("Submodules: none\n");
    } else {
        report.push_str(&format!(
            "Submodules: present ({})\nSubmodule paths:\n",
            submodule_paths.len()
        ));
        for path in submodule_paths {
            report.push_str(&format!("  {}\n", display_git_path(&path)));
        }
    }
    report.push_str(&format!("Native CoW copying: {cow_status}\n"));
    Ok(report)
}

pub(super) fn display_git_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

pub(super) fn run_sync(
    requested_commit: Option<&str>,
    current_directory: Option<&Path>,
    invocation: WorktreeInvocation,
) -> Result<()> {
    let command = invocation.command();
    let repository = discover_repository(current_directory)?;
    let current_branch = repository.current_branch.clone().context(format!(
        "{command} sync requires an attached branch; detached worktrees cannot be synchronized"
    ))?;
    anyhow::ensure!(
        current_branch.starts_with(WORKTREE_BRANCH_PREFIX)
            && current_branch.len() > WORKTREE_BRANCH_PREFIX.len(),
        "{command} sync only operates on wt/* worktree branches"
    );
    anyhow::ensure!(
        !repository.current_entry.bare
            && !same_path(&repository.current_worktree, &repository.root),
        "{command} sync must be run from a linked Git worktree, not the repository main worktree"
    );

    let metadata_key = metadata_key(&current_branch);
    let source_branch = read_metadata(&repository.current_worktree, &metadata_key)?
        .context("the current wt/* branch has no recorded source branch metadata")?;
    anyhow::ensure!(
        !source_branch.is_empty(),
        "the current wt/* branch has an empty recorded source branch"
    );

    if rebase_in_progress(&repository.current_worktree)? {
        if let Some(requested_commit) = requested_commit {
            let requested_target = resolve_commit(
                &repository.current_worktree,
                requested_commit,
                &format!("{command} sync target {requested_commit:?}"),
            )?;
            let recorded_target = rebase_onto_commit(&repository.current_worktree)?
                .context("the active rebase has no recorded target; rerun sync without a COMMIT")?;
            anyhow::ensure!(
                requested_target == recorded_target,
                "{command} sync target {requested_commit:?} does not match the active rebase target {recorded_target}"
            );
        }

        continue_sync_rebase(&repository.current_worktree, invocation)?;
        if has_unmerged_entries(&repository.current_worktree)? {
            return Err(autostash_conflict_after_success_error(invocation));
        }
        println!(
            "Continued synchronization of {} with {}.",
            current_branch, source_branch
        );
        return Ok(());
    }

    let _source = source_worktree_without_cleanliness(&repository, &source_branch)?;
    let source_ref = format!("refs/heads/{source_branch}");
    let source_tip = resolve_commit(
        &repository.current_worktree,
        &source_ref,
        &format!("recorded source branch {source_branch:?}"),
    )?;
    let current_ref = format!("refs/heads/{current_branch}");
    let split_point = merge_base(&repository.current_worktree, &current_ref, &source_ref)?;
    let target = requested_commit
        .map(|commit| {
            resolve_commit(
                &repository.current_worktree,
                commit,
                &format!("{command} sync target {commit:?}"),
            )
        })
        .transpose()?
        .unwrap_or_else(|| source_tip.clone());

    if let Some(requested_commit) = requested_commit {
        anyhow::ensure!(
            is_ancestor(&repository.current_worktree, &split_point, &target)?,
            "{command} sync target {requested_commit:?} is before the current split point {split_point}; the split point is an inclusive lower bound"
        );
        anyhow::ensure!(
            is_ancestor(&repository.current_worktree, &target, &source_tip)?,
            "{command} sync target {requested_commit:?} is not at or before the current tip of recorded source branch {source_branch:?}"
        );
    }

    let rebase_arguments = vec![
        os("rebase"),
        os("--autostash"),
        os("--onto"),
        os(&target),
        os(&split_point),
    ];
    let rebase_output = run_git(Some(&repository.current_worktree), &rebase_arguments)?;
    if !rebase_output.status.success() {
        if rebase_in_progress(&repository.current_worktree)? {
            return Err(sync_conflict_error(&rebase_output, invocation));
        }
        if autostash_conflict(&rebase_output) {
            return Err(autostash_conflict_error(&rebase_output, invocation));
        }
        return Err(git_error("git rebase --autostash", &rebase_output));
    }

    if has_unmerged_entries(&repository.current_worktree)? {
        return Err(autostash_conflict_after_success_error(invocation));
    }
    anyhow::ensure!(
        !rebase_in_progress(&repository.current_worktree)?,
        "the worktree rebase is still in progress; resolve conflicts, stage the resolutions with git add, and rerun {command} sync"
    );
    println!(
        "Synchronized {} with {} at {}.",
        current_branch, source_branch, target
    );
    Ok(())
}

pub(super) const GLOBAL_GIT_CONFIGURATION: [(&str, &str); 5] = [
    ("pull.rebase", "true"),
    ("rebase.autoStash", "true"),
    ("alias.up", "pull --rebase --autostash"),
    ("rerere.enabled", "true"),
    ("rerere.autoupdate", "true"),
];

pub(super) fn run_config(current_directory: Option<&Path>) -> Result<()> {
    let current_directory = current_directory
        .map(Path::to_path_buf)
        .or_else(test_current_directory)
        .or_else(|| env::current_dir().ok());
    for (key, value) in GLOBAL_GIT_CONFIGURATION {
        let arguments = vec![
            os("config"),
            os("--global"),
            os("--replace-all"),
            os(key),
            os(value),
        ];
        let output = run_git(current_directory.as_deref(), &arguments)?;
        if !output.status.success() {
            return Err(git_error("git config --global --replace-all", &output));
        }
    }

    for (key, expected) in GLOBAL_GIT_CONFIGURATION {
        let arguments = vec![os("config"), os("--global"), os("--get"), os(key)];
        let actual = git_text(current_directory.as_deref(), &arguments)?;
        anyhow::ensure!(
            actual == expected,
            "git config --global {key} was set to {actual:?}, expected {expected:?}"
        );
    }
    println!("Installed the recommended global Git configuration.");
    Ok(())
}

#[cfg(test)]
#[path = "../tests/worktree_cli/commands.rs"]
mod tests;
