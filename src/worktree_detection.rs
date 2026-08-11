use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;

const WORKTREE_BRANCH_PREFIX: &str = "refs/heads/wt/";

/// Inspect a native directory for a linked Git worktree whose branch is
/// `wt/<name>`.
///
/// The inspection deliberately reads Git's worktree metadata instead of
/// spawning Git. Callers run this function on the background executor; the
/// `Result` distinguishes an unavailable/unreadable directory from a normal
/// directory that simply is not a matching worktree, so an explicit WSL or
/// remote title is not cleared by an inspection that the host cannot perform.
pub(crate) fn detect_worktree_name(path: &Path) -> Result<Option<String>> {
    let directory = fs::canonicalize(path)
        .with_context(|| format!("canonicalizing terminal directory {}", path.display()))?;
    anyhow::ensure!(
        fs::metadata(&directory)
            .with_context(|| format!("reading terminal directory {}", directory.display()))?
            .is_dir(),
        "terminal working directory {} is not a directory",
        directory.display()
    );

    let Some(git_marker) = find_git_marker(&directory)? else {
        return Ok(None);
    };
    let marker_metadata = fs::metadata(&git_marker)
        .with_context(|| format!("reading Git marker {}", git_marker.display()))?;
    if !marker_metadata.is_file() {
        // A directory marker is the repository's main worktree. It is not a
        // linked temporary worktree even if somebody manually names its branch
        // `wt/*`.
        return Ok(None);
    }

    let gitdir = read_gitdir_pointer(&git_marker)?;
    let gitdir = canonicalize_gitdir(&git_marker, &gitdir)?;

    // Linked worktrees have both of these metadata files. Requiring the
    // back-pointer also avoids treating a submodule's `.git` file as a linked
    // worktree if that submodule happens to use a `wt/*` branch.
    let commondir = gitdir.join("commondir");
    let back_pointer = gitdir.join("gitdir");
    if !commondir.is_file() || !back_pointer.is_file() {
        return Ok(None);
    }
    let linked_marker = read_gitdir_back_pointer(&back_pointer)?;
    let linked_marker = canonicalize_gitdir(&back_pointer, &linked_marker)?;
    if linked_marker != git_marker {
        return Ok(None);
    }

    let head = fs::read_to_string(gitdir.join("HEAD"))
        .with_context(|| format!("reading linked worktree HEAD {}", gitdir.display()))?;
    let Some(reference) = head.trim().strip_prefix("ref: ") else {
        return Ok(None);
    };
    let Some(branch) = reference.strip_prefix(WORKTREE_BRANCH_PREFIX) else {
        return Ok(None);
    };
    (!branch.is_empty() && !branch.contains(['\r', '\n', '\0']))
        .then(|| branch.to_owned())
        .ok_or_else(|| anyhow::anyhow!("linked worktree HEAD contains an invalid wt/* branch"))
        .map(Some)
}

fn find_git_marker(directory: &Path) -> Result<Option<PathBuf>> {
    for ancestor in directory.ancestors() {
        let marker = ancestor.join(".git");
        match fs::symlink_metadata(&marker) {
            Ok(_) => return Ok(Some(marker)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading Git marker {}", marker.display()));
            }
        }
    }
    Ok(None)
}

fn read_gitdir_pointer(path: &Path) -> Result<PathBuf> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("reading Git directory pointer {}", path.display()))?;
    let pointer = contents
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .filter(|pointer| !pointer.is_empty())
        .context("Git directory pointer is missing the gitdir prefix")?;
    anyhow::ensure!(
        !pointer.contains(['\r', '\n', '\0']),
        "Git directory pointer contains control characters"
    );
    Ok(PathBuf::from(pointer))
}

fn read_gitdir_back_pointer(path: &Path) -> Result<PathBuf> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("reading linked worktree pointer {}", path.display()))?;
    let pointer = contents.trim().strip_suffix('/').unwrap_or(contents.trim());
    anyhow::ensure!(
        !pointer.is_empty() && !pointer.contains(['\r', '\n', '\0']),
        "linked worktree pointer is invalid"
    );
    Ok(PathBuf::from(pointer))
}

fn canonicalize_gitdir(pointer_file: &Path, gitdir: &Path) -> Result<PathBuf> {
    let gitdir = if gitdir.is_absolute() {
        gitdir.to_path_buf()
    } else {
        pointer_file
            .parent()
            .context("Git directory pointer has no parent")?
            .join(gitdir)
    };
    fs::canonicalize(&gitdir)
        .with_context(|| format!("canonicalizing Git directory {}", gitdir.display()))
}

impl Zetta {
    pub(crate) fn schedule_worktree_detection_for_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some((directory, is_wsl)) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .or_else(|| self.background_sessions.iter().find(|tab| tab.id == tab_id))
            .and_then(|tab| {
                let pane = tab.pane(pane_id)?;
                Some((
                    pane.working_directory(cx),
                    is_wsl_shell(&pane.profile.command),
                ))
            })
        else {
            return;
        };
        if is_wsl {
            // The reported WSL path belongs to a different filesystem. The
            // explicit `wt new`/`wt done` control path remains authoritative.
            return;
        }
        let Some(directory) = directory else {
            return;
        };
        self.schedule_worktree_detection(tab_id, pane_id, directory, cx);
    }

    fn schedule_worktree_detection(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        directory: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(generation) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .or_else(|| {
                self.background_sessions
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
            })
            .map(|tab| {
                tab.worktree_detection_generation =
                    tab.worktree_detection_generation.wrapping_add(1);
                tab.worktree_detection_generation
            })
        else {
            return;
        };

        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let detection_directory = directory.clone();
            let result = executor
                .spawn(async move { detect_worktree_name(&detection_directory) })
                .await;
            this.update(cx, |this, cx| {
                this.apply_worktree_detection(tab_id, pane_id, directory, generation, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn apply_worktree_detection(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        directory: PathBuf,
        generation: u64,
        result: Result<Option<String>>,
        cx: &mut Context<Self>,
    ) {
        let current_directory = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .or_else(|| self.background_sessions.iter().find(|tab| tab.id == tab_id))
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.working_directory(cx));
        if current_directory.as_deref() != Some(directory.as_path()) {
            return;
        }

        let Ok(worktree_title) = result else {
            return;
        };
        let mut changed = false;
        let mut background = false;
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            if tab.worktree_detection_generation != generation {
                return;
            }
            changed = tab.worktree_title != worktree_title;
            tab.worktree_title = worktree_title;
        } else if let Some(tab) = self
            .background_sessions
            .iter_mut()
            .find(|tab| tab.id == tab_id)
        {
            if tab.worktree_detection_generation != generation {
                return;
            }
            changed = tab.worktree_title != worktree_title;
            tab.worktree_title = worktree_title;
            background = true;
        }

        if !changed {
            return;
        }
        if background {
            self.publish_background_session_catalog(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
#[path = "tests/worktree_detection.rs"]
mod tests;
