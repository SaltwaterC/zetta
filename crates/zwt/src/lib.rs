//! The `zwt` worktree command as a library: the entry point the standalone
//! binary and the root package's copy of it both call.

mod process_control;
mod worktree_cli;
mod worktree_copy;

pub(crate) use zetta_cli_help::format_help_table;

pub use process_control::{WorktreeNameRequest, request_process_worktree_name};
pub use worktree_cli::{
    CreatedWorktree, FinishedWorktree, LinkedWorktree, RepositoryWorktrees, SyncedWorktree,
    WorktreeCommand, WorktreeInvocation, create_worktree, discard_worktree, integrate_worktree,
    list_worktrees, parse_worktree_args, parse_worktree_args_for, run, run_for, sync_worktree,
    worktree_abort_help, worktree_abort_help_for, worktree_config_help, worktree_config_help_for,
    worktree_done_help, worktree_done_help_for, worktree_help, worktree_help_for,
    worktree_new_help, worktree_new_help_for, worktree_status_help, worktree_status_help_for,
    worktree_sync_help, worktree_sync_help_for,
};

/// Runs the standalone `zwt` command with the supplied arguments.
pub fn run_standalone<I>(arguments: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let command = parse_worktree_args(arguments.into_iter().collect::<Vec<_>>().as_slice())?;
    run(&command)
}

/// Entry point used by the standalone and root-produced `zwt` binaries.
pub fn standalone_main() {
    if let Err(error) = run_standalone(std::env::args_os().skip(1)) {
        eprintln!("zwt failed: {error:#}");
        std::process::exit(1);
    }
}
