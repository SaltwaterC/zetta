use std::{
    env,
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

#[cfg(test)]
use std::cell::RefCell;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

use anyhow::{Context as _, Result};

const WORKTREE_BRANCH_PREFIX: &str = "wt/";
const WORKTREE_METADATA_SECTION: &str = "wtbranch";
const PATH_ONLY_OPTION: &str = "--path-only";

#[cfg(test)]
thread_local! {
    static TEST_CURRENT_DIRECTORY: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static TEST_GIT_CONFIG: RefCell<Option<(OsString, OsString)>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn replace_test_current_directory(path: Option<PathBuf>) -> Option<PathBuf> {
    TEST_CURRENT_DIRECTORY.with(|current| current.replace(path))
}

#[cfg(test)]
pub(crate) fn replace_test_git_config(
    config: Option<(OsString, OsString)>,
) -> Option<(OsString, OsString)> {
    TEST_GIT_CONFIG.with(|current| current.replace(config))
}

#[cfg(test)]
fn test_current_directory() -> Option<PathBuf> {
    TEST_CURRENT_DIRECTORY.with(|current| current.borrow().clone())
}

#[cfg(not(test))]
fn test_current_directory() -> Option<PathBuf> {
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeCommand {
    New { name: String, path_only: bool },
    Done { path_only: bool },
    Status,
    Rerere,
}

pub(crate) fn parse_worktree_args(arguments: &[OsString]) -> Result<WorktreeCommand> {
    if arguments.is_empty() {
        anyhow::bail!("zetta wt requires an operation; run zetta wt --help for usage");
    }
    let operation = arguments.first().map(|argument| argument.to_string_lossy());
    if operation.as_deref() == Some("--help") || operation.as_deref() == Some("-h") {
        println!("{}", worktree_help());
        std::process::exit(0);
    }

    let operation = operation.expect("worktree operation was checked above");
    match operation.as_ref() {
        "new" => parse_new_args(&arguments[1..]),
        "done" => parse_done_args(&arguments[1..]),
        "status" => parse_no_arguments("status", &arguments[1..], WorktreeCommand::Status),
        "rerere" => parse_no_arguments("rerere", &arguments[1..], WorktreeCommand::Rerere),
        unknown => {
            anyhow::bail!("unknown zetta wt operation {unknown:?}; run zetta wt --help for usage")
        }
    }
}

fn parse_new_args(arguments: &[OsString]) -> Result<WorktreeCommand> {
    let mut path_only = false;
    let mut name = None;
    for argument in arguments {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", worktree_new_help());
                std::process::exit(0);
            }
            PATH_ONLY_OPTION | "-P" => {
                anyhow::ensure!(!path_only, "{PATH_ONLY_OPTION} may only be specified once");
                path_only = true;
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown zetta wt new option {value:?}")
            }
            value => {
                anyhow::ensure!(name.is_none(), "zetta wt new accepts exactly one NAME");
                name = Some(value.to_owned());
            }
        }
    }
    let name = name.context("zetta wt new requires NAME; run zetta wt new --help for usage")?;
    anyhow::ensure!(!name.is_empty(), "zetta wt new requires a non-empty NAME");
    Ok(WorktreeCommand::New { name, path_only })
}

fn parse_done_args(arguments: &[OsString]) -> Result<WorktreeCommand> {
    let mut path_only = false;
    for argument in arguments {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", worktree_done_help());
                std::process::exit(0);
            }
            PATH_ONLY_OPTION | "-P" => {
                anyhow::ensure!(!path_only, "{PATH_ONLY_OPTION} may only be specified once");
                path_only = true;
            }
            value => anyhow::bail!("unknown zetta wt done argument {value:?}"),
        }
    }
    Ok(WorktreeCommand::Done { path_only })
}

fn parse_no_arguments(
    operation: &str,
    arguments: &[OsString],
    command: WorktreeCommand,
) -> Result<WorktreeCommand> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!(
            "{}",
            match operation {
                "status" => worktree_status_help(),
                "rerere" => worktree_rerere_help(),
                _ => worktree_help(),
            }
        );
        std::process::exit(0);
    }
    anyhow::ensure!(
        arguments.is_empty(),
        "zetta wt {operation} does not accept arguments; run zetta wt {operation} --help for usage"
    );
    Ok(command)
}

pub(crate) fn worktree_help() -> &'static str {
    "Zetta Git worktree workflow\n\nUsage: zetta wt <COMMAND>\n       zetta wt new [OPTIONS] NAME\n       zetta wt done [OPTIONS]\n       zetta wt status\n       zetta wt rerere\n\nCommands:\n  new                                 Create a wt/NAME worktree from the current branch\n  done                                Rebase, integrate, and remove the current wt/* worktree\n  status                              Show the current worktree workflow state\n  rerere                              Enable Git recorded conflict-resolution helpers\n\nThe direct CLI never changes the caller directory. Generated shell integration provides\nzwt, which changes directory after successful new or done operations.\n\nWorktree roots:\n  Git reads effective wt.root configuration. Configure a repository with:\n    git config --local wt.root ../project-worktrees\n  Relative values resolve from the repository main worktree root. Without wt.root, Zetta\n  uses sibling directory <repository>-worktrees. NAME may contain nested components such\n  as feature/api, which creates <wt.root>/feature/api.\n\nRecommended setup:\n  zetta wt rerere\n  This enables rerere.enabled and rerere.autoupdate globally so repeated conflicts can\n  be resolved automatically after you resolve and stage them once."
}

pub(crate) fn worktree_new_help() -> &'static str {
    "Create a Git worktree for a temporary wt/NAME branch\n\nUsage: zetta wt new [OPTIONS] NAME\n\nThe current worktree must be on an attached branch. Zetta creates branch wt/NAME,\nrecords that branch source in wtbranch.wt/NAME.base, and places the worktree at\n<wt.root>/NAME. Nested NAME values are supported. The default root is sibling\n<repository>-worktrees; configure a repository root with git config --local wt.root PATH.\nFor example, use git config --local wt.root ../project-worktrees. Relative PATH values\nresolve from the repository root. Existing paths, symlinks, and branches are rejected.\n\nIf the source commit contains submodules, new recursively initializes them at their\nrecorded commits. An initialized matching submodule checkout in the source worktree\nis reused as a local Git object reference when possible; otherwise Git uses the\nsubmodule's configured remote. If initialization fails, Zetta force-removes the\npartial worktree, deletes its branch, and clears its metadata.\n\nOptions:\n  -P, --path-only                   Print exactly the created worktree path\n  -h, --help                        Print help\n\nUse zwt new NAME from generated shell integration to create the worktree and cd into\nit. The zetta wt rerere shortcut is recommended before the first conflict."
}

pub(crate) fn worktree_done_help() -> &'static str {
    "Integrate and remove the current temporary worktree\n\nUsage: zetta wt done [OPTIONS]\n\nThe current worktree must be a clean, attached wt/* branch created by zetta wt new.\nZetta rebases it onto the recorded source branch, verifies that the source worktree is\nstill attached to a clean worktree, fast-forwards that source worktree, removes the\ntemporary worktree and branch, and clears the source metadata. Submodule changes are\nincluded in the cleanliness checks. Worktrees whose current commit contains submodules\nare removed with Git's forced worktree cleanup after successful integration. If a rebase\nconflicts, resolve the files, stage the resolutions with git add, and rerun zetta wt done.\n\nOptions:\n  -P, --path-only                   Print exactly the integrated source worktree path\n  -h, --help                        Print help\n\nThe direct CLI does not change directory. zwt done changes into the source worktree\nafter success. The worktree destination uses the configured wt.root, or the sibling\n<repository>-worktrees default when wt.root is unset. For example, use git config --local\nwt.root ../project-worktrees. Run zetta wt rerere to enable Git recorded conflict-resolution\nhelpers."
}

pub(crate) fn worktree_status_help() -> &'static str {
    "Show Git worktree workflow state\n\nUsage: zetta wt status\n\nPrints repository root, current worktree, attached or detached branch state, recorded\nsource branch, and resolved wt.root. The root is marked configured or default and\nstatus never creates the root directory. For example, configure it with\ngit config --local wt.root ../project-worktrees; relative values resolve from the\nrepository root. If it is unset, Zetta uses sibling <repository>-worktrees.\n\nRun zetta wt rerere before integrating worktrees to enable Git's recorded conflict\nresolution helpers. The direct CLI never changes directory; generated zwt new and\nzwt done wrappers enter worktrees only after successful operations."
}

pub(crate) fn worktree_rerere_help() -> &'static str {
    "Enable Git rerere for the worktree workflow\n\nUsage: zetta wt rerere\n\nRuns git config --global rerere.enabled true and git config --global\nrerere.autoupdate true. This is the recommended shortcut before using zetta wt done,\nespecially when the same conflicts recur. The optional wt.root setting does not affect\nrerere; configure it per repository with git config --local wt.root PATH, where a\nrelative PATH is resolved from the repository root and the default is sibling\n<repository>-worktrees. Generated shell integration provides zwt new and zwt done,\nwhich enter the resulting worktrees after successful operations."
}

pub(crate) fn run(command: &WorktreeCommand) -> Result<()> {
    run_at(command, None)
}

fn run_at(command: &WorktreeCommand, current_directory: Option<&Path>) -> Result<()> {
    match command {
        WorktreeCommand::New { name, path_only } => run_new(name, *path_only, current_directory),
        WorktreeCommand::Done { path_only } => run_done(*path_only, current_directory),
        WorktreeCommand::Status => run_status(current_directory),
        WorktreeCommand::Rerere => run_rerere(current_directory),
    }
}

#[derive(Clone, Debug)]
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
    bare: bool,
}

#[derive(Debug)]
struct Repository {
    root: PathBuf,
    current_worktree: PathBuf,
    current_branch: Option<String>,
    current_entry: WorktreeEntry,
    worktrees: Vec<WorktreeEntry>,
}

#[derive(Clone, Debug)]
struct ResolvedRoot {
    path: PathBuf,
    configured: bool,
}

fn run_new(name: &str, path_only: bool, current_directory: Option<&Path>) -> Result<()> {
    let repository = discover_repository(current_directory)?;
    anyhow::ensure!(
        !rebase_in_progress(&repository.current_worktree)?,
        "zetta wt new cannot run while the current worktree has a rebase in progress"
    );
    let source_branch = repository
        .current_branch
        .as_deref()
        .context("zetta wt new requires the current worktree to have an attached branch")?;
    anyhow::ensure!(
        !repository.current_entry.bare,
        "zetta wt new requires a non-bare Git worktree"
    );

    let branch = format!("{WORKTREE_BRANCH_PREFIX}{name}");
    validate_branch_name(&repository.current_worktree, &branch)?;
    ensure_branch_is_available(&repository.current_worktree, &branch)?;
    let source_submodules = gitlink_paths(&repository.current_worktree, source_branch)?;

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
    let add_output = run_git(Some(&repository.current_worktree), &add_arguments)?;
    if !add_output.status.success() {
        remove_empty_directories(&created_directories);
        return Err(git_error("git worktree add", &add_output));
    }

    let metadata_key = metadata_key(&branch);
    if !source_submodules.is_empty()
        && let Err(error) = initialize_submodules(
            &destination,
            &repository.current_worktree,
            &source_submodules,
        )
    {
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

    let metadata_arguments = vec![
        os("config"),
        os("--local"),
        os(&metadata_key),
        os(source_branch),
    ];
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

fn run_done(path_only: bool, current_directory: Option<&Path>) -> Result<()> {
    let repository = discover_repository(current_directory)?;
    let current_branch = repository.current_branch.clone().context(
        "zetta wt done requires an attached branch; detached worktrees cannot be integrated",
    )?;
    anyhow::ensure!(
        current_branch.starts_with(WORKTREE_BRANCH_PREFIX)
            && current_branch.len() > WORKTREE_BRANCH_PREFIX.len(),
        "zetta wt done only operates on wt/* worktree branches"
    );
    anyhow::ensure!(
        !repository.current_entry.bare
            && !same_path(&repository.current_worktree, &repository.root),
        "zetta wt done must be run from a linked Git worktree, not the repository main worktree"
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
        continue_rebase(&repository.current_worktree)?;
    } else {
        ensure_clean(&repository.current_worktree, "current worktree")?;
        let rebase_arguments = vec![os("rebase"), os(&source_branch)];
        let rebase_output = run_git(Some(&repository.current_worktree), &rebase_arguments)?;
        if !rebase_output.status.success() {
            if rebase_in_progress(&repository.current_worktree)? {
                return Err(conflict_error(&rebase_output));
            }
            return Err(git_error("git rebase", &rebase_output));
        }
    }

    anyhow::ensure!(
        !rebase_in_progress(&repository.current_worktree)?,
        "the worktree rebase is still in progress; resolve conflicts, stage the resolutions with git add, and rerun zetta wt done"
    );
    ensure_clean(
        &repository.current_worktree,
        "current worktree after rebase",
    )?;
    let current_commit_has_submodules =
        !gitlink_paths(&repository.current_worktree, "HEAD")?.is_empty();

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

fn run_status(current_directory: Option<&Path>) -> Result<()> {
    let repository = discover_repository(current_directory)?;
    let resolved_root = resolved_worktree_root(&repository.current_worktree, &repository.root)?;
    print!("{}", status_report(&repository, &resolved_root)?);
    Ok(())
}

fn status_report(repository: &Repository, resolved_root: &ResolvedRoot) -> Result<String> {
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

    Ok(format!(
        "Repository root: {}\nCurrent worktree: {}\nCurrent branch: {branch}\nCurrent branch state: {branch_state}\nRecorded source branch: {source_branch}\nResolved wt.root: {} ({root_kind})\n",
        repository.root.display(),
        repository.current_worktree.display(),
        resolved_root.path.display()
    ))
}

fn run_rerere(current_directory: Option<&Path>) -> Result<()> {
    let current_directory = current_directory
        .map(Path::to_path_buf)
        .or_else(test_current_directory)
        .or_else(|| env::current_dir().ok());
    for (key, value) in [("rerere.enabled", "true"), ("rerere.autoupdate", "true")] {
        let arguments = vec![os("config"), os("--global"), os(key), os(value)];
        let output = run_git(current_directory.as_deref(), &arguments)?;
        if !output.status.success() {
            return Err(git_error("git config --global", &output));
        }
    }
    println!("Enabled Git rerere and rerere.autoupdate globally.");
    Ok(())
}

fn discover_repository(current_directory: Option<&Path>) -> Result<Repository> {
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

fn parse_worktrees(output: &str) -> Result<Vec<WorktreeEntry>> {
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

fn resolved_worktree_root(config_directory: &Path, repository_root: &Path) -> Result<ResolvedRoot> {
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

fn destination_path(root: &Path, name: &str) -> PathBuf {
    normalize_path(&root.join(name))
}

fn validate_branch_name(repository: &Path, branch: &str) -> Result<()> {
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

fn ensure_branch_is_available(repository: &Path, branch: &str) -> Result<()> {
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

fn validate_destination(root: &Path, name: &str, destination: &Path) -> Result<()> {
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

fn create_destination_parent(destination: &Path) -> Result<Vec<PathBuf>> {
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

fn remove_empty_directories(directories: &[PathBuf]) {
    for directory in directories {
        let _ = fs::remove_dir(directory);
    }
}

fn gitlink_paths(repository: &Path, treeish: &str) -> Result<Vec<PathBuf>> {
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
    parse_gitlink_paths(&output.stdout)
}

fn parse_gitlink_paths(output: &[u8]) -> Result<Vec<PathBuf>> {
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
        let path = &record[tab + 1..];
        anyhow::ensure!(
            !path.is_empty(),
            "Git returned an empty submodule path while finding submodules"
        );
        paths.push(git_path_from_bytes(path));
    }
    Ok(paths)
}

fn git_path_from_bytes(path: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from(OsString::from_vec(path.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(path).into_owned())
    }
}

fn initialize_submodules(
    destination: &Path,
    source_worktree: &Path,
    paths: &[PathBuf],
) -> Result<()> {
    for path in paths {
        initialize_submodule(destination, source_worktree, path)?;
    }
    Ok(())
}

fn initialize_submodule(
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

fn is_valid_reference_repository(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let arguments = [os("rev-parse"), os("--git-dir")];
    run_git(Some(path), &arguments)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn rollback_new_worktree(
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

fn source_worktree(repository: &Repository, source_branch: &str) -> Result<WorktreeEntry> {
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
    ensure_clean(&source.path, "source worktree")?;
    Ok(source)
}

fn ensure_clean(path: &Path, description: &str) -> Result<()> {
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

fn rebase_in_progress(path: &Path) -> Result<bool> {
    for marker in ["rebase-merge", "rebase-apply"] {
        for marker_path in rebase_marker_paths(path, marker)? {
            if marker_path.exists() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn rebase_branch_name(path: &Path) -> Result<Option<String>> {
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

fn rebase_marker_paths(path: &Path, marker: &str) -> Result<Vec<PathBuf>> {
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

fn continue_rebase(path: &Path) -> Result<()> {
    for _ in 0..1024 {
        if !rebase_in_progress(path)? {
            return Ok(());
        }
        let arguments = vec![os("rebase"), os("--continue")];
        let output = run_git_with_editor(path, &arguments)?;
        if !output.status.success() {
            if rebase_in_progress(path)? {
                return Err(conflict_error(&output));
            }
            return Err(git_error("git rebase --continue", &output));
        }
    }
    anyhow::bail!("git rebase did not finish after 1024 continuation attempts")
}

fn conflict_error(output: &Output) -> anyhow::Error {
    anyhow::anyhow!(
        "rebase stopped with conflicts: {}. Resolve the conflicts, stage the resolutions with git add, and rerun zetta wt done.",
        git_diagnostic(output)
    )
}

fn read_metadata(repository: &Path, key: &str) -> Result<Option<String>> {
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

fn metadata_key(branch: &str) -> String {
    format!("{WORKTREE_METADATA_SECTION}.{branch}.base")
}

fn git_text(path: Option<&Path>, arguments: &[OsString]) -> Result<String> {
    let output = run_git(path, arguments)?;
    if !output.status.success() {
        return Err(git_error("git command", &output));
    }
    Ok(text_without_newline(&output.stdout))
}

fn nonempty_git_text(path: Option<&Path>, arguments: &[OsString]) -> Result<Option<String>> {
    let text = git_text(path, arguments)?;
    Ok((!text.is_empty()).then_some(text))
}

fn run_git(path: Option<&Path>, arguments: &[OsString]) -> Result<Output> {
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

fn run_git_with_editor(path: &Path, arguments: &[OsString]) -> Result<Output> {
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
fn apply_test_git_config(command: &mut Command) {
    TEST_GIT_CONFIG.with(|config| {
        if let Some((global, system)) = config.borrow().as_ref() {
            command
                .env("GIT_CONFIG_GLOBAL", global)
                .env("GIT_CONFIG_SYSTEM", system);
        }
    });
}

fn git_error(operation: &str, output: &Output) -> anyhow::Error {
    anyhow::anyhow!("{operation} failed: {}", git_diagnostic(output))
}

fn git_diagnostic(output: &Output) -> String {
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

fn display_arguments(arguments: &[OsString]) -> String {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn text_without_newline(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

fn os(value: &str) -> OsString {
    OsString::from(value)
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| normalize_path(left));
    let right = fs::canonicalize(right).unwrap_or_else(|_| normalize_path(right));
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn normalize_path(path: &Path) -> PathBuf {
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
#[path = "tests/worktree_cli.rs"]
mod tests;
