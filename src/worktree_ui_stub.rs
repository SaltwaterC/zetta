//! Stand-in for `worktree_ui.rs` in a build without the `worktree` feature,
//! which has no `zwt`. The sidebar still lists Main and the projects; nothing
//! lists a worktree, so no worktree row or action is ever shown.

use super::*;
use crate::sidebar::SidebarWorktree;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeFinish {
    Integrate,
    Discard,
}

/// Uninhabited: there is no worktree dialog to open.
#[derive(Debug)]
pub(crate) enum WorktreePrompt {}

impl WorktreePrompt {
    pub(crate) fn text_field_mut(&mut self) -> Option<&mut TextField> {
        match *self {}
    }
}

impl Zetta {
    pub(crate) fn refresh_project_worktrees(&mut self, _root: PathBuf, _cx: &mut Context<Self>) {}

    pub(crate) fn new_worktree(&mut self, _: &NewWorktree, _: &mut Window, _: &mut Context<Self>) {}

    pub(crate) fn prompt_new_worktree(
        &mut self,
        _project: PathBuf,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(crate) fn prompt_finish_worktree(
        &mut self,
        _project: PathBuf,
        _worktree: SidebarWorktree,
        _finish: WorktreeFinish,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(crate) fn sync_sidebar_worktree(
        &mut self,
        _project: PathBuf,
        _worktree: SidebarWorktree,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(crate) fn worktree_prompt_key_down(
        &mut self,
        _event: &KeyDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }

    pub(crate) fn render_worktree_prompt_overlay(
        &self,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        None
    }
}
