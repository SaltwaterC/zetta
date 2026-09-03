//! The git plumbing the worktree commands are built from.
//!
//! Everything here shells out to `git` and reads its output: repository and
//! worktree discovery, branch and destination validation, gitlink and
//! submodule handling, the rebase state machine, and the metadata a managed
//! worktree records about its source branch.

use super::*;

pub(super) fn discover_repository(current_directory: Option<&Path>) -> Result<Repository> {
    let current_dir = current_directory
        .map(Path::to_path_buf)
        .or_else(test_current_directory)
        .or_else(|| env::current_dir().ok())
        .context("reading the current directory")?;
    let inside = git_text(
        Some(&current_dir),
        &[os("rev-parse"), os("--is-inside-work-tree")],
    )?;
    anyhow::ensure!(
        inside.trim() == "true",
        "the current directory is not inside a Git worktree"
    );

    let current_worktree = PathBuf::from(git_text(
        Some(&current_dir),
        &[os("rev-parse"), os("--show-toplevel")],
    )?);
    let worktrees = parse_worktrees(&git_text(
        Some(&current_worktree),
        &[os("worktree"), os("list"), os("--porcelain")],
    )?)?;
    let root = worktrees
        .first()
        .filter(|entry| !entry.bare)
        .map(|entry| entry.path.clone())
        .context("could not determine the repository main worktree root")?;
    let current_entry = worktrees
        .iter()
        .find(|entry| same_path(&entry.path, &current_worktree))
        .cloned()
        .context("the current directory is not a registered Git worktree")?;
    let current_branch = nonempty_git_text(
        Some(&current_worktree),
        &[os("branch"), os("--show-current")],
    )?
    .or(rebase_branch_name(&current_worktree)?);

    Ok(Repository {
        root,
        current_worktree,
        current_branch,
        current_entry,
        worktrees,
    })
}

pub(super) fn parse_worktrees(output: &str) -> Result<Vec<WorktreeEntry>> {
    let mut entries = Vec::new();
    let mut current = None;
    for line in output.lines() {
        if line.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(path),
                branch: None,
                bare: false,
            });
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(entry) = current.as_mut() {
                entry.branch = Some(branch.to_owned());
            }
        } else if line == "bare"
            && let Some(entry) = current.as_mut()
        {
            entry.bare = true;
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    anyhow::ensure!(!entries.is_empty(), "Git did not report any worktrees");
    Ok(entries)
}

pub(super) fn resolved_worktree_root(
    config_directory: &Path,
    repository_root: &Path,
) -> Result<ResolvedRoot> {
    let arguments = [os("config"), os("--get"), os("wt.root")];
    let output = run_git(Some(config_directory), &arguments)?;
    if output.status.success() {
        let value = text_without_newline(&output.stdout);
        anyhow::ensure!(!value.is_empty(), "Git configuration wt.root is empty");
        let configured_path = PathBuf::from(value);
        let path = if configured_path.is_absolute() {
            configured_path
        } else {
            repository_root.join(configured_path)
        };
        return Ok(ResolvedRoot {
            path: normalize_path(&path),
            configured: true,
        });
    }
    if output.status.code() != Some(1) {
        return Err(git_error("git config --get wt.root", &output));
    }

    let parent = repository_root
        .parent()
        .context("the repository root has no sibling directory")?;
    let name = repository_root
        .file_name()
        .context("the repository root has no name")?;
    Ok(ResolvedRoot {
        path: parent.join(format!("{}-worktrees", name.to_string_lossy())),
        configured: false,
    })
}

pub(super) fn destination_path(root: &Path, name: &str) -> PathBuf {
    normalize_path(&root.join(name))
}

pub(super) fn validate_branch_name(repository: &Path, branch: &str) -> Result<()> {
    let arguments = vec![os("check-ref-format"), os("--branch"), os(branch)];
    let output = run_git(Some(repository), &arguments)?;
    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "invalid worktree name {branch:?}: {}",
            git_diagnostic(&output)
        )
    }
}

pub(super) fn ensure_branch_is_available(repository: &Path, branch: &str) -> Result<()> {
    let reference = format!("refs/heads/{branch}");
    let arguments = vec![
        os("show-ref"),
        os("--verify"),
        os("--quiet"),
        os(&reference),
    ];
    let output = run_git(Some(repository), &arguments)?;
    if output.status.success() {
        anyhow::bail!("worktree branch {branch:?} already exists")
    }
    anyhow::ensure!(
        output.status.code() == Some(1),
        "could not check whether worktree branch {branch:?} exists: {}",
        git_diagnostic(&output)
    );
    Ok(())
}

pub(super) fn validate_destination(root: &Path, name: &str, destination: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(root) {
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "wt.root {} is a symlink; choose a real directory root",
            root.display()
        );
        anyhow::ensure!(
            metadata.is_dir(),
            "wt.root {} is not a directory",
            root.display()
        );
    }

    let mut component_path = root.to_path_buf();
    for component in Path::new(name).components() {
        let Component::Normal(component) = component else {
            anyhow::bail!("worktree name {name:?} contains an invalid path component")
        };
        component_path.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&component_path) {
            anyhow::ensure!(
                !metadata.file_type().is_symlink(),
                "worktree destination component {} is a symlink",
                component_path.display()
            );
            if component_path != destination {
                anyhow::ensure!(
                    metadata.is_dir(),
                    "worktree destination component {} is not a directory",
                    component_path.display()
                );
            }
        }
    }

    if fs::symlink_metadata(destination).is_ok() {
        anyhow::bail!(
            "worktree destination {} already exists",
            destination.display()
        );
    }
    Ok(())
}

pub(super) fn create_destination_parent(destination: &Path) -> Result<Vec<PathBuf>> {
    let parent = destination
        .parent()
        .context("worktree destination has no parent directory")?;
    let mut missing = Vec::new();
    let mut path = parent.to_path_buf();
    while fs::symlink_metadata(&path).is_err() {
        missing.push(path.clone());
        path = path
            .parent()
            .map(Path::to_path_buf)
            .context("could not find an existing parent for the worktree root")?;
    }
    for directory in missing.iter().rev() {
        fs::create_dir(directory)
            .with_context(|| format!("creating worktree directory {}", directory.display()))?;
    }
    Ok(missing)
}

pub(super) fn remove_empty_directories(directories: &[PathBuf]) {
    for directory in directories {
        let _ = fs::remove_dir(directory);
    }
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn gitlink_paths(repository: &Path, treeish: &str) -> Result<Vec<PathBuf>> {
    Ok(gitlink_entries(repository, treeish)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

#[cfg(feature = "recursive-submodules")]
pub(super) struct GitlinkEntry {
    path: PathBuf,
    commit: String,
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn gitlink_entries(repository: &Path, treeish: &str) -> Result<Vec<GitlinkEntry>> {
    let arguments = vec![
        os("ls-tree"),
        os("-r"),
        os("-z"),
        os("--full-tree"),
        os(treeish),
        os("--"),
    ];
    let output = run_git(Some(repository), &arguments)?;
    anyhow::ensure!(
        output.status.success(),
        "could not inspect Git tree for submodules: {}",
        git_diagnostic(&output)
    );
    parse_gitlink_entries(&output.stdout)
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn all_gitlink_paths(repository: &Path, treeish: &str) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_gitlink_paths(repository, treeish, Path::new(""), &mut paths)?;
    Ok(paths)
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn collect_gitlink_paths(
    repository: &Path,
    treeish: &str,
    prefix: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in gitlink_entries(repository, treeish)? {
        let full_path = prefix.join(&entry.path);
        paths.push(full_path.clone());

        let submodule = repository.join(&entry.path);
        if is_valid_reference_repository(&submodule) {
            collect_gitlink_paths(&submodule, &entry.commit, &full_path, paths)?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix, feature = "recursive-submodules"))]
pub(super) fn parse_gitlink_paths(output: &[u8]) -> Result<Vec<PathBuf>> {
    Ok(parse_gitlink_entries(output)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn parse_gitlink_entries(output: &[u8]) -> Result<Vec<GitlinkEntry>> {
    let mut paths = Vec::new();
    for record in output.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .context("Git returned a malformed tree entry while finding submodules")?;
        if !record.starts_with(b"160000 ") {
            continue;
        }
        let mut header = record[..tab].split(|byte| byte.is_ascii_whitespace());
        anyhow::ensure!(
            header.next() == Some(b"160000".as_slice())
                && header.next() == Some(b"commit".as_slice()),
            "Git returned a malformed gitlink entry while finding submodules"
        );
        let commit = header
            .next()
            .context("Git returned a gitlink without a commit while finding submodules")?;
        anyhow::ensure!(
            header.next().is_none(),
            "Git returned a malformed gitlink entry while finding submodules"
        );
        let path = &record[tab + 1..];
        anyhow::ensure!(
            !path.is_empty(),
            "Git returned an empty submodule path while finding submodules"
        );
        paths.push(GitlinkEntry {
            path: git_path_from_bytes(path),
            commit: String::from_utf8(commit.to_vec())
                .context("Git returned a non-text gitlink commit while finding submodules")?,
        });
    }
    Ok(paths)
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn git_path_from_bytes(path: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from(OsString::from_vec(path.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(path).into_owned())
    }
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn initialize_submodules(
    destination: &Path,
    source_worktree: &Path,
    paths: &[PathBuf],
) -> Result<()> {
    for path in paths {
        initialize_submodule(destination, source_worktree, path)?;
    }
    Ok(())
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn initialize_submodule(
    destination_parent: &Path,
    source_parent: &Path,
    path: &Path,
) -> Result<()> {
    let source_path = source_parent.join(path);
    let mut arguments = vec![
        os("submodule"),
        os("update"),
        os("--init"),
        os("--checkout"),
    ];
    if is_valid_reference_repository(&source_path) {
        arguments.push(os("--reference"));
        arguments.push(source_path.as_os_str().to_os_string());
    }
    arguments.push(os("--"));
    arguments.push(path.as_os_str().to_os_string());

    let output = run_git(Some(destination_parent), &arguments)?;
    anyhow::ensure!(
        output.status.success(),
        "{} failed: {}",
        display_arguments(&arguments),
        git_diagnostic(&output)
    );

    let destination_path = destination_parent.join(path);
    let nested_paths = gitlink_paths(&destination_path, "HEAD")?;
    for nested_path in nested_paths {
        initialize_submodule(&destination_path, &source_path, &nested_path)?;
    }
    Ok(())
}

#[cfg(feature = "recursive-submodules")]
pub(super) fn is_valid_reference_repository(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let arguments = [os("rev-parse"), os("--git-dir")];
    run_git(Some(path), &arguments)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub(super) fn rollback_new_worktree(
    repository: &Path,
    destination: &Path,
    branch: &str,
    metadata_key: &str,
    created_directories: &[PathBuf],
) -> Vec<String> {
    let mut errors = Vec::new();
    let remove_arguments = vec![
        os("worktree"),
        os("remove"),
        os("--force"),
        os("--"),
        destination.as_os_str().to_os_string(),
    ];
    let remove_output = run_git(Some(repository), &remove_arguments);
    if let Ok(output) = remove_output {
        if !output.status.success() {
            errors.push(git_diagnostic(&output));
        }
    } else if let Err(error) = remove_output {
        errors.push(error.to_string());
    }

    let delete_arguments = vec![os("branch"), os("-d"), os("--"), os(branch)];
    match run_git(Some(repository), &delete_arguments) {
        Ok(output) if output.status.success() => {}
        Ok(output) => errors.push(git_diagnostic(&output)),
        Err(error) => errors.push(error.to_string()),
    }

    let unset_arguments = vec![
        os("config"),
        os("--local"),
        os("--unset-all"),
        os(metadata_key),
    ];
    match run_git(Some(repository), &unset_arguments) {
        Ok(output) if output.status.success() || output.status.code() == Some(5) => {}
        Ok(output) => errors.push(git_diagnostic(&output)),
        Err(error) => errors.push(error.to_string()),
    }
    remove_empty_directories(created_directories);
    errors
}

pub(super) fn source_worktree(
    repository: &Repository,
    source_branch: &str,
) -> Result<WorktreeEntry> {
    let source = source_worktree_without_cleanliness(repository, source_branch)?;
    ensure_clean(&source.path, "source worktree")?;
    Ok(source)
}

pub(super) fn source_worktree_without_cleanliness(
    repository: &Repository,
    source_branch: &str,
) -> Result<WorktreeEntry> {
    let reference = format!("refs/heads/{source_branch}");
    let reference_arguments = vec![os("rev-parse"), os("--verify"), os(&reference)];
    let reference_output = run_git(Some(&repository.current_worktree), &reference_arguments)?;
    anyhow::ensure!(
        reference_output.status.success(),
        "recorded source branch {source_branch:?} does not exist"
    );

    let source = repository
        .worktrees
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(source_branch))
        .cloned()
        .context("recorded source branch is not attached to a Git worktree")?;
    anyhow::ensure!(
        !source.bare,
        "recorded source branch is attached to a bare repository"
    );
    anyhow::ensure!(
        !same_path(&source.path, &repository.current_worktree),
        "recorded source branch resolves to the current worktree"
    );
    anyhow::ensure!(
        source.path.is_dir(),
        "recorded source worktree {} is missing",
        source.path.display()
    );
    let actual_branch =
        nonempty_git_text(Some(&source.path), &[os("branch"), os("--show-current")])?;
    anyhow::ensure!(
        actual_branch.as_deref() == Some(source_branch),
        "source worktree {} is no longer on branch {source_branch:?}",
        source.path.display()
    );
    Ok(source)
}

pub(super) fn ensure_clean(path: &Path, description: &str) -> Result<()> {
    let arguments = vec![
        os("status"),
        os("--porcelain=v1"),
        os("--untracked-files=all"),
        os("--ignore-submodules=none"),
    ];
    let output = run_git(Some(path), &arguments)?;
    anyhow::ensure!(
        output.status.success(),
        "could not inspect {description}: {}",
        git_diagnostic(&output)
    );
    anyhow::ensure!(
        text_without_newline(&output.stdout).is_empty(),
        "{description} is dirty; commit or discard its changes before continuing"
    );
    Ok(())
}

pub(super) fn resolve_commit(path: &Path, commitish: &str, description: &str) -> Result<String> {
    let revision = format!("{commitish}^{{commit}}");
    let arguments = vec![
        os("rev-parse"),
        os("--verify"),
        os("--end-of-options"),
        OsString::from(revision),
    ];
    let output = run_git(Some(path), &arguments)?;
    anyhow::ensure!(
        output.status.success(),
        "{description} does not resolve to a commit: {}",
        git_diagnostic(&output)
    );
    let commit = text_without_newline(&output.stdout);
    anyhow::ensure!(
        !commit.is_empty(),
        "{description} resolved to an empty commit ID"
    );
    Ok(commit)
}

pub(super) fn merge_base(path: &Path, left: &str, right: &str) -> Result<String> {
    let arguments = vec![os("merge-base"), os(left), os(right)];
    let output = run_git(Some(path), &arguments)?;
    anyhow::ensure!(
        output.status.success(),
        "could not determine the current split point: {}",
        git_diagnostic(&output)
    );
    let split_point = text_without_newline(&output.stdout);
    anyhow::ensure!(
        !split_point.is_empty(),
        "the current worktree branch and recorded source branch have no common ancestor"
    );
    Ok(split_point)
}

pub(super) fn is_ancestor(path: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let arguments = vec![
        os("merge-base"),
        os("--is-ancestor"),
        os(ancestor),
        os(descendant),
    ];
    let output = run_git(Some(path), &arguments)?;
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    Err(git_error("git merge-base --is-ancestor", &output))
}

pub(super) fn rebase_onto_commit(path: &Path) -> Result<Option<String>> {
    for marker in ["rebase-merge", "rebase-apply"] {
        for marker_path in rebase_marker_paths(path, marker)? {
            let marker_path = if marker_path.is_absolute() {
                marker_path
            } else {
                path.join(marker_path)
            };
            let onto_path = marker_path.join("onto");
            let contents = match fs::read(&onto_path) {
                Ok(contents) => contents,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("reading the active rebase target {}", onto_path.display())
                    });
                }
            };
            let onto = text_without_newline(&contents);
            if onto.is_empty() {
                continue;
            }
            return resolve_commit(path, &onto, "the active rebase target").map(Some);
        }
    }
    Ok(None)
}

pub(super) fn rebase_in_progress(path: &Path) -> Result<bool> {
    for marker in ["rebase-merge", "rebase-apply"] {
        for marker_path in rebase_marker_paths(path, marker)? {
            if marker_path.exists() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn rebase_branch_name(path: &Path) -> Result<Option<String>> {
    for marker in ["rebase-merge", "rebase-apply"] {
        for marker_path in rebase_marker_paths(path, marker)? {
            let head_name = marker_path.join("head-name");
            let Ok(head_name) = fs::read_to_string(head_name) else {
                continue;
            };
            let head_name = head_name.trim();
            if let Some(branch) = head_name.strip_prefix("refs/heads/")
                && !branch.is_empty()
            {
                return Ok(Some(branch.to_owned()));
            }
        }
    }
    Ok(None)
}

pub(super) fn rebase_marker_paths(path: &Path, marker: &str) -> Result<Vec<PathBuf>> {
    let path_arguments = vec![os("rev-parse"), os("--git-path"), os(marker)];
    let path_output = run_git(Some(path), &path_arguments)?;
    if !path_output.status.success() {
        return Ok(Vec::new());
    }
    let marker_path = PathBuf::from(text_without_newline(&path_output.stdout));
    let absolute_git_dir = PathBuf::from(git_text(
        Some(path),
        &[os("rev-parse"), os("--absolute-git-dir")],
    )?);
    let mut paths = vec![marker_path.clone(), absolute_git_dir.join(marker)];
    if !marker_path.is_absolute() {
        paths.push(path.join(marker_path));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(super) fn continue_rebase(path: &Path, invocation: WorktreeInvocation) -> Result<()> {
    for _ in 0..1024 {
        if !rebase_in_progress(path)? {
            return Ok(());
        }
        let arguments = vec![os("rebase"), os("--continue")];
        let output = run_git_with_editor(path, &arguments)?;
        if !output.status.success() {
            if rebase_in_progress(path)? {
                return Err(conflict_error(&output, invocation));
            }
            return Err(git_error("git rebase --continue", &output));
        }
    }
    anyhow::bail!("git rebase did not finish after 1024 continuation attempts")
}

pub(super) fn continue_sync_rebase(path: &Path, invocation: WorktreeInvocation) -> Result<()> {
    for _ in 0..1024 {
        if !rebase_in_progress(path)? {
            return Ok(());
        }
        let arguments = vec![os("rebase"), os("--continue")];
        let output = run_git_with_editor(path, &arguments)?;
        if !output.status.success() {
            if rebase_in_progress(path)? {
                return Err(sync_conflict_error(&output, invocation));
            }
            if autostash_conflict(&output) {
                return Err(autostash_conflict_error(&output, invocation));
            }
            return Err(git_error("git rebase --continue", &output));
        }
    }
    anyhow::bail!("git rebase did not finish after 1024 continuation attempts")
}

pub(super) fn conflict_error(output: &Output, invocation: WorktreeInvocation) -> anyhow::Error {
    anyhow::anyhow!(
        "rebase stopped with conflicts: {}. Resolve the conflicts, stage the resolutions with git add, and rerun {} done.",
        git_diagnostic(output),
        invocation.command()
    )
}

pub(super) fn sync_conflict_error(
    output: &Output,
    invocation: WorktreeInvocation,
) -> anyhow::Error {
    anyhow::anyhow!(
        "sync rebase stopped with conflicts: {}. Resolve the conflicts, stage the resolutions with git add, and rerun {} sync.",
        git_diagnostic(output),
        invocation.command()
    )
}

pub(super) fn autostash_conflict(output: &Output) -> bool {
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    diagnostic.contains("autostash")
        && (diagnostic.contains("conflict") || diagnostic.contains("applying"))
}

pub(super) fn autostash_conflict_error(
    output: &Output,
    invocation: WorktreeInvocation,
) -> anyhow::Error {
    anyhow::anyhow!(
        "sync rebase completed, but applying the autostash resulted in conflicts: {}. The rebase is no longer active; resolve the working-tree conflicts manually and do not rerun {} sync for this autostash.",
        git_diagnostic(output),
        invocation.command()
    )
}

pub(super) fn autostash_conflict_after_success_error(
    invocation: WorktreeInvocation,
) -> anyhow::Error {
    anyhow::anyhow!(
        "sync rebase completed, but applying the autostash resulted in conflicts. The rebase is no longer active; resolve the working-tree conflicts manually and do not rerun {} sync for this autostash.",
        invocation.command()
    )
}

pub(super) fn has_unmerged_entries(path: &Path) -> Result<bool> {
    let checks = [
        vec![os("diff"), os("--name-only"), os("--diff-filter=U")],
        vec![
            os("diff"),
            os("--cached"),
            os("--name-only"),
            os("--diff-filter=U"),
        ],
    ];
    for arguments in checks {
        let output = run_git(Some(path), &arguments)?;
        anyhow::ensure!(
            output.status.success(),
            "could not inspect unmerged worktree entries: {}",
            git_diagnostic(&output)
        );
        if !text_without_newline(&output.stdout).is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn read_metadata(repository: &Path, key: &str) -> Result<Option<String>> {
    let arguments = vec![os("config"), os("--local"), os("--get"), os(key)];
    let output = run_git(Some(repository), &arguments)?;
    if output.status.success() {
        return Ok(Some(text_without_newline(&output.stdout)));
    }
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    Err(git_error("git config --get", &output))
}

pub(super) fn metadata_key(branch: &str) -> String {
    format!("{WORKTREE_METADATA_SECTION}.{branch}.base")
}

pub(super) fn git_text(path: Option<&Path>, arguments: &[OsString]) -> Result<String> {
    let output = run_git(path, arguments)?;
    if !output.status.success() {
        return Err(git_error("git command", &output));
    }
    Ok(text_without_newline(&output.stdout))
}

pub(super) fn nonempty_git_text(
    path: Option<&Path>,
    arguments: &[OsString],
) -> Result<Option<String>> {
    let text = git_text(path, arguments)?;
    Ok((!text.is_empty()).then_some(text))
}

pub(super) fn run_git(path: Option<&Path>, arguments: &[OsString]) -> Result<Output> {
    let mut command = Command::new("git");
    command.args(arguments);
    if let Some(path) = path {
        command.current_dir(path);
    }
    #[cfg(test)]
    apply_test_git_config(&mut command);
    command
        .output()
        .with_context(|| format!("failed to start git {}", display_arguments(arguments)))
}

pub(super) fn run_git_with_editor(path: &Path, arguments: &[OsString]) -> Result<Output> {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(path);
    command.env("GIT_EDITOR", "true");
    command.env("GIT_SEQUENCE_EDITOR", "true");
    #[cfg(test)]
    apply_test_git_config(&mut command);
    command
        .output()
        .with_context(|| format!("failed to start git {}", display_arguments(arguments)))
}

#[cfg(test)]
pub(super) fn apply_test_git_config(command: &mut Command) {
    TEST_GIT_CONFIG.with(|config| {
        if let Some((global, system)) = config.borrow().as_ref() {
            command
                .env("GIT_CONFIG_GLOBAL", global)
                .env("GIT_CONFIG_SYSTEM", system);
        }
    });
}

pub(super) fn git_error(operation: &str, output: &Output) -> anyhow::Error {
    anyhow::anyhow!("{operation} failed: {}", git_diagnostic(output))
}

pub(super) fn git_diagnostic(output: &Output) -> String {
    let stderr = text_without_newline(&output.stderr);
    if stderr.is_empty() {
        let stdout = text_without_newline(&output.stdout);
        if stdout.is_empty() {
            format!("exit status {}", output.status)
        } else {
            stdout
        }
    } else {
        stderr
    }
}

pub(super) fn display_arguments(arguments: &[OsString]) -> String {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn text_without_newline(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

pub(super) fn os(value: &str) -> OsString {
    OsString::from(value)
}

pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| normalize_path(left));
    let right = fs::canonicalize(right).unwrap_or_else(|_| normalize_path(right));
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(component) => normalized.push(component),
        }
    }
    normalized
}

#[cfg(test)]
#[path = "../tests/worktree_cli/git.rs"]
mod tests;
