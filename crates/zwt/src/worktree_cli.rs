use std::{
    env,
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

#[cfg(test)]
use std::cell::RefCell;
#[cfg(all(unix, feature = "recursive-submodules"))]
use std::os::unix::ffi::OsStringExt;

use crate::format_help_table;
use crate::process_control::{WorktreeNameRequest, request_process_worktree_name};
use crate::worktree_copy::{
    copy_paths as copy_worktree_paths, cow_copy_supported, validate_copy_path, validate_copy_paths,
    validate_copy_sources,
};
use anyhow::{Context as _, Result};

const WORKTREE_BRANCH_PREFIX: &str = "wt/";
const WORKTREE_METADATA_SECTION: &str = "wtbranch";
const PATH_ONLY_OPTION: &str = "--path-only";

/// Identifies the command name used in diagnostics and help text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeInvocation {
    /// The independently installed `zwt` command.
    Standalone,
    /// The command exposed as `zetta wt`.
    Zetta,
}

impl WorktreeInvocation {
    pub fn command(self) -> &'static str {
        match self {
            Self::Standalone => "zwt",
            Self::Zetta => "zetta wt",
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_CURRENT_DIRECTORY: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static TEST_GIT_CONFIG: RefCell<Option<(OsString, OsString)>> = const { RefCell::new(None) };
    static TEST_WORKTREE_NAME_REQUESTS: RefCell<Option<Vec<Option<String>>>> = const { RefCell::new(None) };
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
pub(crate) fn replace_test_worktree_name_requests(
    requests: Option<Vec<Option<String>>>,
) -> Option<Vec<Option<String>>> {
    TEST_WORKTREE_NAME_REQUESTS.with(|current| current.replace(requests))
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
pub enum WorktreeCommand {
    New {
        name: String,
        path_only: bool,
        copy_paths: Vec<PathBuf>,
    },
    Done {
        path_only: bool,
    },
    Abort {
        path_only: bool,
    },
    Status,
    Sync {
        commit: Option<String>,
    },
    Config,
}

pub fn parse_worktree_args(arguments: &[OsString]) -> Result<WorktreeCommand> {
    parse_worktree_args_for(arguments, WorktreeInvocation::Standalone)
}

pub fn parse_worktree_args_for(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    if arguments.is_empty() {
        anyhow::bail!(
            "{} requires an operation; run {} --help for usage",
            invocation.command(),
            invocation.command()
        );
    }
    let operation = arguments.first().map(|argument| argument.to_string_lossy());
    if operation.as_deref() == Some("--help") || operation.as_deref() == Some("-h") {
        println!("{}", worktree_help_for(invocation));
        std::process::exit(0);
    }

    let operation = operation.expect("worktree operation was checked above");
    match operation.as_ref() {
        "new" => parse_new_args(&arguments[1..], invocation),
        "done" => parse_done_args(&arguments[1..], invocation),
        "abort" => parse_abort_args(&arguments[1..], invocation),
        "status" => parse_no_arguments(
            "status",
            &arguments[1..],
            WorktreeCommand::Status,
            invocation,
        ),
        "sync" => parse_sync_args(&arguments[1..], invocation),
        "config" => parse_no_arguments(
            "config",
            &arguments[1..],
            WorktreeCommand::Config,
            invocation,
        ),
        unknown => {
            anyhow::bail!(
                "unknown {} operation {unknown:?}; run {} --help for usage",
                invocation.command(),
                invocation.command()
            )
        }
    }
}

fn parse_new_args(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    let mut path_only = false;
    let mut name = None;
    let mut copy_paths = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", worktree_new_help_for(invocation));
                std::process::exit(0);
            }
            PATH_ONLY_OPTION | "-P" => {
                anyhow::ensure!(!path_only, "{PATH_ONLY_OPTION} may only be specified once");
                path_only = true;
            }
            "--copy" | "-c" => {
                let path = arguments
                    .next()
                    .context("--copy requires a relative PATH")?;
                anyhow::ensure!(
                    !path.to_string_lossy().starts_with('-'),
                    "--copy requires a relative PATH"
                );
                copy_paths.push(validate_copy_path(Path::new(path))?);
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown {} new option {value:?}", invocation.command())
            }
            value => {
                anyhow::ensure!(
                    name.is_none(),
                    "{} new accepts exactly one NAME",
                    invocation.command()
                );
                name = Some(value.to_owned());
            }
        }
    }
    let name = name.with_context(|| {
        format!(
            "{} new requires NAME; run {} new --help for usage",
            invocation.command(),
            invocation.command()
        )
    })?;
    anyhow::ensure!(
        !name.is_empty(),
        "{} new requires a non-empty NAME",
        invocation.command()
    );
    let copy_paths = validate_copy_paths(&copy_paths)?;
    Ok(WorktreeCommand::New {
        name,
        path_only,
        copy_paths,
    })
}

fn parse_done_args(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    let path_only = parse_path_only_args(arguments, "done", invocation, worktree_done_help_for)?;
    Ok(WorktreeCommand::Done { path_only })
}

fn parse_abort_args(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    let path_only = parse_path_only_args(arguments, "abort", invocation, worktree_abort_help_for)?;
    Ok(WorktreeCommand::Abort { path_only })
}

fn parse_sync_args(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    let mut commit = None;
    for argument in arguments {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", worktree_sync_help_for(invocation));
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown {} sync argument {value:?}", invocation.command())
            }
            value => {
                anyhow::ensure!(
                    commit.is_none(),
                    "{} sync accepts at most one COMMIT",
                    invocation.command()
                );
                commit = Some(value.to_owned());
            }
        }
    }
    Ok(WorktreeCommand::Sync { commit })
}

fn parse_path_only_args(
    arguments: &[OsString],
    operation: &str,
    invocation: WorktreeInvocation,
    help: fn(WorktreeInvocation) -> String,
) -> Result<bool> {
    let mut path_only = false;
    for argument in arguments {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", help(invocation));
                std::process::exit(0);
            }
            PATH_ONLY_OPTION | "-P" => {
                anyhow::ensure!(!path_only, "{PATH_ONLY_OPTION} may only be specified once");
                path_only = true;
            }
            value => anyhow::bail!(
                "unknown {} {operation} argument {value:?}",
                invocation.command()
            ),
        }
    }
    Ok(path_only)
}

fn parse_no_arguments(
    operation: &str,
    arguments: &[OsString],
    command: WorktreeCommand,
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!(
            "{}",
            match operation {
                "status" => worktree_status_help_for(invocation),
                "config" => worktree_config_help_for(invocation),
                _ => worktree_help_for(invocation),
            }
        );
        std::process::exit(0);
    }
    anyhow::ensure!(
        arguments.is_empty(),
        "{} {operation} does not accept arguments; run {} {operation} --help for usage",
        invocation.command(),
        invocation.command()
    );
    Ok(command)
}

pub fn worktree_help() -> String {
    worktree_help_for(WorktreeInvocation::Standalone)
}

pub fn worktree_help_for(invocation: WorktreeInvocation) -> String {
    let command = invocation.command();
    format!(
        "Zetta Git worktree workflow\n\nUsage: {command} <COMMAND>\n       {command} new [OPTIONS] NAME\n       {command} done [OPTIONS]\n       {command} abort [OPTIONS]\n       {command} status\n       {command} sync [COMMIT]\n       {command} config\n\nCommands:\n{}\n\nThe direct CLI never changes the caller directory. Generated shell integration provides\nzwt, which changes directory after successful new, done, or abort operations; sync and\nconfig pass through without changing directory.\n\nWorktree roots:\n  Git reads effective wt.root configuration. Configure a repository with:\n    git config --local wt.root ../project-worktrees\n  Relative values resolve from the repository main worktree root. Without wt.root, Zetta\n  uses sibling directory <repository>-worktrees. NAME may contain nested components such\n  as feature/api, which creates <wt.root>/feature/api.\n\nRecommended setup:\n  {command} config\n  This configures Git's pull/rebase, autostash, update alias, and recorded\n  conflict-resolution helpers globally.",
        format_help_table([
            ("new", "Create a wt/NAME worktree from the current branch"),
            (
                "done",
                "Rebase, integrate, and remove the current wt/* worktree",
            ),
            ("abort", "Discard and remove the current wt/* worktree"),
            ("status", "Show the current worktree workflow state"),
            ("sync", "Rebase the current worktree onto the source branch",),
            ("config", "Install the recommended global Git configuration"),
        ]),
        command = command,
    )
}

pub fn worktree_abort_help() -> String {
    worktree_abort_help_for(WorktreeInvocation::Standalone)
}

pub fn worktree_abort_help_for(invocation: WorktreeInvocation) -> String {
    let command = invocation.command();
    let options = format_help_table([
        (
            "-P, --path-only",
            "Print exactly the preserved source worktree path",
        ),
        ("-h, --help", "Print help"),
    ]);
    format!(
        "Discard and remove the current temporary worktree\n\nUsage: {command} abort [OPTIONS]\n\nThe current worktree must be a linked, non-bare worktree on a wt/* branch created by\n{command} new, with recorded source metadata. The recorded source branch must still\nexist and be checked out in its original separate non-bare worktree. The source worktree\nmay be dirty. Abort force-removes the current worktree, deletes its temporary branch with\ngit branch -D, and clears the recorded metadata. It discards all changes in the current\nworktree, including untracked files and an in-progress rebase. It never rebases, merges,\nchecks out, or fast-forwards the source branch. Validation completes before removal.\n\nOptions:\n{options}\n\nThe direct CLI does not change directory. Generated shell integration makes zwt abort\nchange into the preserved source worktree after successful cleanup. The path-only output\nis emitted only after the worktree, branch, and metadata have all been cleaned up.",
        command = command,
    )
}

pub fn worktree_new_help() -> String {
    worktree_new_help_for(WorktreeInvocation::Standalone)
}

pub fn worktree_new_help_for(invocation: WorktreeInvocation) -> String {
    let command = invocation.command();
    let options = format_help_table([
        (
            "-c, --copy PATH",
            "Copy a source-worktree path (repeatable)",
        ),
        ("-P, --path-only", "Print exactly the created worktree path"),
        ("-h, --help", "Print help"),
    ]);
    format!(
        "Create a Git worktree for a temporary wt/NAME branch\n\nUsage: {command} new [OPTIONS] NAME\n\nThe current worktree must be on an attached branch. Zetta creates branch wt/NAME,\nrecords that branch source in wtbranch.wt/NAME.base, and places the worktree at\n<wt.root>/NAME. Nested NAME values are supported. The default root is sibling\n<repository>-worktrees; configure a repository root with git config --local wt.root PATH.\nFor example, use git config --local wt.root ../project-worktrees. Relative PATH values\nresolve from the repository root. Existing paths, symlinks, and branches are rejected.\n\nIf the source commit contains submodules, new recursively initializes them at their\nrecorded commits. An initialized matching submodule checkout in the source worktree\nis reused as a local Git object reference when possible; otherwise Git uses the\nsubmodule's configured remote. If initialization fails, Zetta force-removes the\npartial worktree, deletes its branch, and clears its metadata.\n\nThe repeatable --copy PATH (or -c PATH) option copies a relative file, directory,\nor symlink from the current source worktree to the identical location in the new\nworktree. Paths may not be absolute, traverse a parent directory, or traverse an\nintermediate symlink. Existing destination paths and overlapping copy requests are\nrejected. Native copy-on-write cloning is used when the filesystem supports it, with\na regular recursive-copy fallback elsewhere. A copy failure removes the new\nworktree, branch, metadata, and directories created for its root.\n\nnew reports phase progress on standard error while creating the worktree,\ninitializing submodules, copying paths, and recording metadata.\n\nOptions:\n{options}\n\nUse zwt new NAME from generated shell integration to create the worktree and cd into\nit. Run {command} config before the first conflict.",
        command = command,
    )
}

pub fn worktree_done_help() -> String {
    worktree_done_help_for(WorktreeInvocation::Standalone)
}

pub fn worktree_done_help_for(invocation: WorktreeInvocation) -> String {
    let command = invocation.command();
    let options = format_help_table([
        (
            "-P, --path-only",
            "Print exactly the integrated source worktree path",
        ),
        ("-h, --help", "Print help"),
    ]);
    format!(
        "Integrate and remove the current temporary worktree\n\nUsage: {command} done [OPTIONS]\n\nThe current worktree must be a clean, attached wt/* branch created by {command} new.\nZetta rebases it onto the recorded source branch, verifies that the source worktree is\nstill attached to a clean worktree, fast-forwards that source worktree, removes the\ntemporary worktree and branch, and clears the source metadata. Submodule changes are\nincluded in the cleanliness checks. Worktrees whose current commit contains submodules\nare removed with Git's forced worktree cleanup after successful integration. If a rebase\nconflicts, resolve the files, stage the resolutions with git add, and rerun {command} done.\n\nOptions:\n{options}\n\nThe direct CLI does not change directory. zwt done changes into the source worktree\nafter success. The worktree destination uses the configured wt.root, or the sibling\n<repository>-worktrees default when wt.root is unset. For example, use git config --local\nwt.root ../project-worktrees. Run {command} config to install Git's recorded conflict-resolution\nand autostash helpers.",
        command = command,
    )
}

pub fn worktree_status_help() -> String {
    worktree_status_help_for(WorktreeInvocation::Standalone)
}

pub fn worktree_status_help_for(invocation: WorktreeInvocation) -> String {
    format!(
        concat!(
            "Show Git worktree workflow state\n\n",
            "Usage: {} status\n\n",
            "Prints repository root, current worktree, attached or detached branch state, recorded\n",
            "source branch, resolved wt.root, whether the current HEAD contains submodules, the\n",
            "detected submodule paths (including nested paths), and whether native copy-on-write\n",
            "copying is available between the current worktree and wt.root. The root is marked\n",
            "configured or default and status never creates it; a missing root is checked through\n",
            "its nearest existing ancestor. For example, configure it with git config --local\n",
            "wt.root ../project-worktrees; relative values resolve from the repository root. If it\n",
            "is unset, Zetta uses sibling <repository>-worktrees.\n\n",
            "Run {} config before integrating worktrees to install Git's recorded conflict\n",
            "resolution and autostash helpers. The direct CLI never changes directory; generated zwt new,\n",
            "zwt done, and zwt abort wrappers enter worktrees only after successful operations."
        ),
        invocation.command(),
        invocation.command(),
    )
}

pub fn worktree_sync_help() -> String {
    worktree_sync_help_for(WorktreeInvocation::Standalone)
}

pub fn worktree_sync_help_for(invocation: WorktreeInvocation) -> String {
    let command = invocation.command();
    format!(
        "Synchronize the current temporary worktree with its source branch\n\nUsage: {command} sync [COMMIT]\n\nThe current worktree must be a linked, non-bare worktree on a managed wt/* branch\ncreated by {command} new, with recorded source metadata. Zetta finds the current\nmerge-base of the worktree branch and recorded source branch. Without COMMIT, sync\nrebases onto the latest source-branch tip. With COMMIT, the commit-ish must resolve to\na commit on the recorded source branch at or after that merge-base and at or before the\ncurrent source tip; the bounds are inclusive. This permits synchronizing to an\nintermediary source commit before syncing again after the split point advances. The\nsource worktree may be dirty and is never changed.\n\nSync runs git rebase --autostash --onto TARGET SPLIT_POINT with pinned commit IDs, so\nlocal tracked edits are preserved through the rebase. Untracked files are left to Git's\nnormal collision handling. If the rebase stops with conflicts, resolve them, stage the\nresolutions with git add, and rerun {command} sync, with or without COMMIT. If applying\nthe post-rebase autostash conflicts, the rebase is already finished; resolve the\nworking-tree conflicts manually. If a rebase is already active, sync continues it. A\nsupplied COMMIT must match Git's recorded rebase target. Sync never changes directory.\n\nThe optional wt.root setting is unrelated to sync; configure it per repository with\ngit config --local wt.root PATH, where a relative PATH is resolved from the repository\nroot and the default is sibling <repository>-worktrees.",
        command = command,
    )
}

pub fn worktree_config_help() -> String {
    worktree_config_help_for(WorktreeInvocation::Standalone)
}

pub fn worktree_config_help_for(invocation: WorktreeInvocation) -> String {
    let command = invocation.command();
    format!(
        "Install the recommended global Git configuration for the worktree workflow\n\nUsage: {command} config\n\nRuns idempotent git config --global --replace-all operations for pull.rebase=true,\nrebase.autoStash=true, alias.up=pull --rebase --autostash, rerere.enabled=true, and\nrerere.autoupdate=true. Only those five keys are changed; unrelated keys and other\nentries in their Git configuration sections are preserved. Configuration is validated\nthrough Git key lookups. Running {command} config repeatedly is safe and does not\nchange directory. The optional wt.root setting is separate and remains a per-repository\nsetting; configure it with git config --local wt.root PATH, where a relative PATH is\nresolved from the repository root and the default is sibling <repository>-worktrees.",
        command = command,
    )
}

pub fn run(command: &WorktreeCommand) -> Result<()> {
    run_for(command, WorktreeInvocation::Standalone)
}

pub fn run_for(command: &WorktreeCommand, invocation: WorktreeInvocation) -> Result<()> {
    run_at(command, None, invocation)
}

fn run_at(
    command: &WorktreeCommand,
    current_directory: Option<&Path>,
    invocation: WorktreeInvocation,
) -> Result<()> {
    match command {
        WorktreeCommand::New {
            name,
            path_only,
            copy_paths,
        } => run_new(name, *path_only, copy_paths, current_directory, invocation),
        WorktreeCommand::Done { path_only } => run_done(*path_only, current_directory, invocation),
        WorktreeCommand::Abort { path_only } => {
            run_abort(*path_only, current_directory, invocation)
        }
        WorktreeCommand::Status => run_status(current_directory, invocation),
        WorktreeCommand::Sync { commit } => {
            run_sync(commit.as_deref(), current_directory, invocation)
        }
        WorktreeCommand::Config => run_config(current_directory),
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

fn request_originating_worktree_name(name: Option<&str>) {
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

fn originating_tab_target() -> Option<(u32, u64)> {
    parse_originating_tab_target(
        &env::var("ZETTA_PROCESS_ID").ok()?,
        &env::var("ZETTA_ATTENTION_ID").ok()?,
    )
}

fn parse_originating_tab_target(process_id: &str, attention_id: &str) -> Option<(u32, u64)> {
    let process_id = process_id.parse().ok()?;
    let attention_id = attention_id.parse().ok()?;
    (process_id != 0 && attention_id != 0).then_some((process_id, attention_id))
}

fn run_new(
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

fn run_done(
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

fn run_abort(
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

fn run_status(current_directory: Option<&Path>, _invocation: WorktreeInvocation) -> Result<()> {
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

fn display_git_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn run_sync(
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

const GLOBAL_GIT_CONFIGURATION: [(&str, &str); 5] = [
    ("pull.rebase", "true"),
    ("rebase.autoStash", "true"),
    ("alias.up", "pull --rebase --autostash"),
    ("rerere.enabled", "true"),
    ("rerere.autoupdate", "true"),
];

fn run_config(current_directory: Option<&Path>) -> Result<()> {
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

#[cfg(feature = "recursive-submodules")]
fn gitlink_paths(repository: &Path, treeish: &str) -> Result<Vec<PathBuf>> {
    Ok(gitlink_entries(repository, treeish)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

#[cfg(feature = "recursive-submodules")]
struct GitlinkEntry {
    path: PathBuf,
    commit: String,
}

#[cfg(feature = "recursive-submodules")]
fn gitlink_entries(repository: &Path, treeish: &str) -> Result<Vec<GitlinkEntry>> {
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
fn all_gitlink_paths(repository: &Path, treeish: &str) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_gitlink_paths(repository, treeish, Path::new(""), &mut paths)?;
    Ok(paths)
}

#[cfg(feature = "recursive-submodules")]
fn collect_gitlink_paths(
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
fn parse_gitlink_paths(output: &[u8]) -> Result<Vec<PathBuf>> {
    Ok(parse_gitlink_entries(output)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

#[cfg(feature = "recursive-submodules")]
fn parse_gitlink_entries(output: &[u8]) -> Result<Vec<GitlinkEntry>> {
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

#[cfg(feature = "recursive-submodules")]
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

#[cfg(feature = "recursive-submodules")]
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

#[cfg(feature = "recursive-submodules")]
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
    let source = source_worktree_without_cleanliness(repository, source_branch)?;
    ensure_clean(&source.path, "source worktree")?;
    Ok(source)
}

fn source_worktree_without_cleanliness(
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

fn resolve_commit(path: &Path, commitish: &str, description: &str) -> Result<String> {
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

fn merge_base(path: &Path, left: &str, right: &str) -> Result<String> {
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

fn is_ancestor(path: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
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

fn rebase_onto_commit(path: &Path) -> Result<Option<String>> {
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

fn continue_rebase(path: &Path, invocation: WorktreeInvocation) -> Result<()> {
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

fn continue_sync_rebase(path: &Path, invocation: WorktreeInvocation) -> Result<()> {
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

fn conflict_error(output: &Output, invocation: WorktreeInvocation) -> anyhow::Error {
    anyhow::anyhow!(
        "rebase stopped with conflicts: {}. Resolve the conflicts, stage the resolutions with git add, and rerun {} done.",
        git_diagnostic(output),
        invocation.command()
    )
}

fn sync_conflict_error(output: &Output, invocation: WorktreeInvocation) -> anyhow::Error {
    anyhow::anyhow!(
        "sync rebase stopped with conflicts: {}. Resolve the conflicts, stage the resolutions with git add, and rerun {} sync.",
        git_diagnostic(output),
        invocation.command()
    )
}

fn autostash_conflict(output: &Output) -> bool {
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    diagnostic.contains("autostash")
        && (diagnostic.contains("conflict") || diagnostic.contains("applying"))
}

fn autostash_conflict_error(output: &Output, invocation: WorktreeInvocation) -> anyhow::Error {
    anyhow::anyhow!(
        "sync rebase completed, but applying the autostash resulted in conflicts: {}. The rebase is no longer active; resolve the working-tree conflicts manually and do not rerun {} sync for this autostash.",
        git_diagnostic(output),
        invocation.command()
    )
}

fn autostash_conflict_after_success_error(invocation: WorktreeInvocation) -> anyhow::Error {
    anyhow::anyhow!(
        "sync rebase completed, but applying the autostash resulted in conflicts. The rebase is no longer active; resolve the working-tree conflicts manually and do not rerun {} sync for this autostash.",
        invocation.command()
    )
}

fn has_unmerged_entries(path: &Path) -> Result<bool> {
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
