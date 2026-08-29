use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;

const WORKTREE_BRANCH_PREFIX: &str = "refs/heads/wt/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreeMetadata {
    pub(crate) name: String,
    pub(crate) main_root: PathBuf,
}

pub(crate) fn terminal_event_requires_worktree_detection(event: &TerminalEvent) -> bool {
    matches!(
        event,
        TerminalEvent::TitleChanged | TerminalEvent::BreadcrumbsChanged
    )
}

/// Inspect a native directory for a linked Git worktree whose branch is
/// `wt/<name>`.
///
/// The inspection deliberately reads Git's worktree metadata instead of
/// spawning Git. Callers run this function on the background executor; the
/// `Result` distinguishes an unavailable/unreadable directory from a normal
/// directory that simply is not a matching worktree, so an explicit WSL or
/// remote title is not cleared by an inspection that the host cannot perform.
pub(crate) fn detect_worktree_metadata(path: &Path) -> Result<Option<WorktreeMetadata>> {
    let Some((commondir, branch)) = inspect_linked_worktree(path)? else {
        return Ok(None);
    };
    let common_gitdir = read_gitdir_metadata_pointer(&commondir)?;
    let common_gitdir = canonicalize_gitdir(&commondir, &common_gitdir)?;
    let main_root = common_gitdir
        .parent()
        .context("Git common directory has no main-worktree parent")?;
    let main_root = fs::canonicalize(main_root).with_context(|| {
        format!(
            "canonicalizing Git main worktree root {}",
            main_root.display()
        )
    })?;
    anyhow::ensure!(
        main_root.is_dir(),
        "Git main worktree root {} is not a directory",
        main_root.display()
    );
    Ok(Some(WorktreeMetadata {
        name: branch,
        main_root,
    }))
}

fn inspect_linked_worktree(path: &Path) -> Result<Option<(PathBuf, String)>> {
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
    anyhow::ensure!(
        !branch.is_empty() && !branch.contains(['\r', '\n', '\0']),
        "linked worktree HEAD contains an invalid wt/* branch"
    );
    Ok(Some((commondir, branch.to_owned())))
}

/// Inspect a native directory for a matching linked worktree and return only
/// its title. Project resolution uses [`detect_worktree_metadata`] when it
/// also needs the canonical main-worktree root.
pub(crate) fn detect_worktree_name(path: &Path) -> Result<Option<String>> {
    Ok(inspect_linked_worktree(path)?.map(|(_, branch)| branch))
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

fn read_gitdir_metadata_pointer(path: &Path) -> Result<PathBuf> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("reading Git common directory pointer {}", path.display()))?;
    let pointer = contents.trim();
    anyhow::ensure!(
        !pointer.is_empty() && !pointer.contains(['\r', '\n', '\0']),
        "Git common directory pointer is invalid"
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

fn worktree_detection_directory_is_current(
    scheduled_directory: Option<&Path>,
    current_directory: Option<&Path>,
    directory: &Path,
) -> bool {
    scheduled_directory == Some(directory)
        && current_directory.is_none_or(|current| current == directory)
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
                let is_wsl = is_wsl_shell(&pane.profile.command);
                let directory = (!is_wsl).then(|| pane.current_directory(cx)).flatten();
                Some((directory, is_wsl))
            })
        else {
            return;
        };
        if is_wsl {
            // The reported WSL path belongs to a different filesystem. The
            // explicit `wt new`/`wt done` control path remains authoritative.
            return;
        }
        let Some((directory, can_clear)) = directory else {
            // A native process tree reports the foreground child, not the
            // shell that launched it. Keep the last shell directory and any
            // in-flight detection while the child owns the foreground; a
            // later shell report will replace it if the shell actually moved.
            return;
        };
        self.schedule_worktree_detection(tab_id, pane_id, directory, can_clear, cx);
    }

    fn schedule_worktree_detection(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        directory: PathBuf,
        can_clear: bool,
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
            .and_then(|tab| {
                let pane = tab.pane_mut(pane_id)?;
                if pane.worktree_detection_directory.as_deref() == Some(directory.as_path())
                    && (!can_clear || pane.worktree_detection_can_clear)
                {
                    return None;
                }
                pane.worktree_detection_generation =
                    pane.worktree_detection_generation.wrapping_add(1);
                pane.worktree_detection_directory = Some(directory.clone());
                pane.worktree_detection_can_clear = can_clear;
                Some(pane.worktree_detection_generation)
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
        let current_state = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .or_else(|| self.background_sessions.iter().find(|tab| tab.id == tab_id))
            .and_then(|tab| {
                let pane = tab.pane(pane_id)?;
                Some((
                    pane.worktree_detection_generation,
                    pane.worktree_detection_directory.as_deref(),
                    pane.worktree_detection_can_clear,
                    pane.current_directory(cx).map(|(directory, _)| directory),
                ))
            });
        let Some((current_generation, scheduled_directory, can_clear, current_directory)) =
            current_state
        else {
            return;
        };
        if current_generation != generation
            || !worktree_detection_directory_is_current(
                scheduled_directory,
                current_directory.as_deref(),
                &directory,
            )
        {
            return;
        }

        let Ok(worktree_title) = result else {
            return;
        };
        // A missing process CWD means that a child currently hides the shell
        // from process inspection. Do not let that transient state erase a
        // known worktree title; the next shell CWD report will schedule the
        // authoritative replacement.
        if worktree_title.is_none() && !can_clear {
            return;
        }
        let mut changed = false;
        let mut background = false;
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            let Some(pane) = tab.pane_mut(pane_id) else {
                return;
            };
            if pane.worktree_detection_generation != generation {
                return;
            }
            changed = pane.detected_worktree_title != worktree_title;
            pane.detected_worktree_title = worktree_title;
        } else if let Some(tab) = self
            .background_sessions
            .iter_mut()
            .find(|tab| tab.id == tab_id)
        {
            let Some(pane) = tab.pane_mut(pane_id) else {
                return;
            };
            if pane.worktree_detection_generation != generation {
                return;
            }
            changed = pane.detected_worktree_title != worktree_title;
            pane.detected_worktree_title = worktree_title;
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
