//! The worktree workflow for a caller that is not a command line.
//!
//! Zetta's sidebar drives the same `new`, `done`, `abort` and `sync` the CLI
//! does, from inside the application process. Two things about the CLI are
//! wrong there, which is why these exist rather than `run_for`:
//!
//! - The CLI prints its results and progress. These return them.
//! - The CLI tells the tab it was run from about the worktree it entered or
//!   left, through `ZETTA_PROCESS_ID`/`ZETTA_ATTENTION_ID`. The application
//!   process has no originating tab — and when Zetta was itself started from a
//!   Zetta pane, those variables name the *parent* window's tab — so nothing
//!   here sends that request.
//!
//! Every function runs `git` and can block for as long as it does, so a GUI
//! calls them off its main thread.

use super::*;

/// A repository's worktrees, as the sidebar lists them under a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryWorktrees {
    /// The main worktree.
    pub root: PathBuf,
    /// The main worktree's branch, which a new worktree is based on. `None`
    /// when it is detached, in which case `new` refuses.
    pub current_branch: Option<String>,
    /// Where `new` would place a worktree, from `wt.root` or its default.
    pub worktree_root: Option<PathBuf>,
    /// Every linked, non-bare worktree, in the order Git lists them.
    pub linked: Vec<LinkedWorktree>,
}

/// One linked worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedWorktree {
    pub path: PathBuf,
    /// `None` for a detached worktree.
    pub branch: Option<String>,
    /// The `NAME` of a `wt/NAME` branch.
    pub name: Option<String>,
    /// The source branch `new` recorded, which `done`, `abort` and `sync`
    /// need. A `wt/*` branch made by hand has none.
    pub source_branch: Option<String>,
}

impl LinkedWorktree {
    /// Whether `done`, `abort` and `sync` can operate on this worktree.
    pub fn is_managed(&self) -> bool {
        self.name.is_some() && self.source_branch.is_some()
    }
}

/// Lists the worktrees of the repository containing `directory`.
pub fn list_worktrees(directory: &Path) -> Result<RepositoryWorktrees> {
    let repository = discover_repository(Some(directory))?;
    let worktree_root = resolved_worktree_root(&repository.current_worktree, &repository.root)
        .ok()
        .map(|root| root.path);
    let current_branch = repository
        .worktrees
        .first()
        .and_then(|entry| entry.branch.clone());
    let linked = repository
        .worktrees
        .iter()
        .skip(1)
        .filter(|entry| !entry.bare)
        .map(|entry| {
            let name = entry
                .branch
                .as_deref()
                .and_then(|branch| branch.strip_prefix(WORKTREE_BRANCH_PREFIX))
                .filter(|name| !name.is_empty())
                .map(str::to_owned);
            // Unreadable metadata only makes the worktree unmanaged: the
            // listing is still worth showing.
            let source_branch = entry
                .branch
                .as_deref()
                .filter(|_| name.is_some())
                .and_then(|branch| read_metadata(&repository.root, &metadata_key(branch)).ok())
                .flatten()
                .filter(|source| !source.is_empty());
            LinkedWorktree {
                path: entry.path.clone(),
                branch: entry.branch.clone(),
                name,
                source_branch,
            }
        })
        .collect();
    Ok(RepositoryWorktrees {
        root: repository.root,
        current_branch,
        worktree_root,
        linked,
    })
}

/// `new NAME` from `directory`, which has to be on an attached branch.
pub fn create_worktree(
    directory: &Path,
    name: &str,
    copy_paths: &[PathBuf],
) -> Result<CreatedWorktree> {
    create_worktree_at(
        name,
        copy_paths,
        Some(directory),
        WorktreeInvocation::Zetta,
        &mut |_| {},
    )
}

/// `done` for the managed worktree at `worktree`.
pub fn integrate_worktree(worktree: &Path) -> Result<FinishedWorktree> {
    integrate_worktree_at(Some(worktree), WorktreeInvocation::Zetta)
}

/// `abort` for the managed worktree at `worktree`.
pub fn discard_worktree(worktree: &Path) -> Result<FinishedWorktree> {
    discard_worktree_at(Some(worktree), WorktreeInvocation::Zetta)
}

/// `sync` for the managed worktree at `worktree`, onto its source's tip.
pub fn sync_worktree(worktree: &Path) -> Result<SyncedWorktree> {
    sync_worktree_at(None, Some(worktree), WorktreeInvocation::Zetta)
}

#[cfg(test)]
#[path = "../tests/worktree_cli/application.rs"]
mod tests;
