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

mod args;
mod commands;
mod git;
mod help;

// The module was one file before it was split by responsibility. These keep
// every name reachable as `zwt::worktree_cli::…`, which is what `lib.rs`
// re-exports to Zetta and to the `zwt` binary.
pub use args::*;
use commands::*;
use git::*;
pub use help::*;

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

#[cfg(test)]
#[path = "tests/worktree_cli.rs"]
mod tests;
