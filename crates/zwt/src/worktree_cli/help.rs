//! The help text for each worktree command, in the name it was invoked by.
//!
//! Every table goes through `format_help_table`, so option labels and their
//! descriptions are stored apart and aligned by the formatter rather than by
//! hand-counted padding.

use super::*;

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

#[cfg(test)]
#[path = "../tests/worktree_cli/help.rs"]
mod tests;
