use super::*;
use crate::process_control::{TabNameRequest, WorktreeNameRequest};

/// Resolve the title shown for a tab.
///
/// Resolve a tab title by source priority. Terminal-side title requests are
/// deliberately below the current worktree name: they remain compatible when
/// no worktree is active, but cannot replace a worktree title.
pub(crate) fn resolve_tab_title(
    tab: &Tab,
    automatic_title: impl FnOnce() -> SharedString,
) -> SharedString {
    tab.custom_title
        .as_ref()
        .map(|title| title.clone().into())
        .or_else(|| {
            tab.active_pane()
                .and_then(|pane| pane.detected_worktree_title.as_ref())
                .map(|title| title.clone().into())
        })
        .or_else(|| {
            tab.worktree_seed_title
                .as_ref()
                .map(|title| title.clone().into())
        })
        .or_else(|| tab.process_title.as_ref().map(|title| title.clone().into()))
        .unwrap_or_else(automatic_title)
}

/// Set the title entered through the manual tab rename UI.
pub(crate) fn set_tab_title(tab: &mut Tab, title: Option<String>) {
    tab.custom_title = title;
}

pub(crate) fn set_tab_process_title(tab: &mut Tab, title: Option<String>) {
    tab.process_title = title;
}

pub(crate) fn set_tab_worktree_title(tab: &mut Tab, title: Option<String>) {
    for pane in &mut tab.panes {
        pane.worktree_detection_generation = pane.worktree_detection_generation.wrapping_add(1);
        pane.worktree_detection_directory = None;
        pane.worktree_detection_can_clear = false;
        pane.detected_worktree_title = None;
    }
    tab.worktree_seed_title = title;
}

pub(crate) fn set_tab_name_on_tabs<'a, I>(tabs: I, request: &TabNameRequest) -> bool
where
    I: IntoIterator<Item = &'a mut Tab>,
{
    let Some(tab) = tabs
        .into_iter()
        .find(|tab| tab.attention_id == request.attention_id)
    else {
        return false;
    };
    set_tab_process_title(tab, request.name.clone());
    true
}

pub(crate) fn set_worktree_name_on_tabs<'a, I>(tabs: I, request: &WorktreeNameRequest) -> bool
where
    I: IntoIterator<Item = &'a mut Tab>,
{
    let Some(tab) = tabs
        .into_iter()
        .find(|tab| tab.attention_id == request.attention_id)
    else {
        return false;
    };
    set_tab_worktree_title(tab, request.name.clone());
    true
}

impl Zetta {
    pub(crate) fn set_tab_name(&mut self, request: TabNameRequest, cx: &mut Context<Self>) -> bool {
        let found_in_visible = set_tab_name_on_tabs(self.tabs.iter_mut(), &request);
        let found_in_background = !found_in_visible
            && set_tab_name_on_tabs(self.background_sessions.iter_unprotected_mut(), &request);
        let found = found_in_visible || found_in_background;
        if found {
            // A process-side title request is also a useful signal that the
            // foreground process changed. Refresh the worktree identity here
            // because this path does not necessarily produce a terminal CWD
            // or title event of its own.
            let target = self
                .tabs
                .iter()
                .find(|tab| tab.attention_id == request.attention_id)
                .or_else(|| {
                    self.background_sessions
                        .iter()
                        .find(|tab| tab.attention_id == request.attention_id)
                })
                .map(|tab| {
                    (
                        tab.id,
                        tab.panes.iter().map(|pane| pane.id).collect::<Vec<_>>(),
                    )
                });
            if let Some((tab_id, pane_ids)) = target {
                for pane_id in pane_ids {
                    self.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                }
            }
            cx.notify();
            if found_in_background {
                self.publish_background_session_catalog(cx);
            }
        }
        found
    }

    pub(crate) fn set_worktree_name(
        &mut self,
        request: WorktreeNameRequest,
        cx: &mut Context<Self>,
    ) -> bool {
        let found_in_visible = set_worktree_name_on_tabs(self.tabs.iter_mut(), &request);
        let found_in_background = !found_in_visible
            && set_worktree_name_on_tabs(self.background_sessions.iter_unprotected_mut(), &request);
        let found = found_in_visible || found_in_background;
        if found {
            cx.notify();
            if found_in_background {
                self.publish_background_session_catalog(cx);
            }
        }
        found
    }

    pub(crate) fn rename_tab(
        &mut self,
        _: &RenameTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_tab_rename(self.active_tab, window, cx);
    }

    pub(crate) fn begin_tab_rename(
        &mut self,
        tab_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let automatic_title = self
            .tabs
            .get(tab_index)
            .and_then(Tab::active_view)
            .map(|view| view.read(cx).tab_content_text(0, cx).to_string())
            .or_else(|| {
                self.tabs
                    .get(tab_index)
                    .and_then(Tab::active_pane)
                    .map(|pane| pane.profile.name.clone())
            })
            .unwrap_or_else(|| "Terminal".to_owned());
        self.active_tab = tab_index;
        self.begin_rename_with_title(tab_index, automatic_title, window, cx);
    }

    pub(crate) fn begin_rename(
        &mut self,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let automatic_title = view.read(cx).tab_content_text(0, cx).to_string();
        self.begin_rename_with_title(self.active_tab, automatic_title, window, cx);
    }

    fn begin_rename_with_title(
        &mut self,
        tab_index: usize,
        automatic_title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tabs.get_mut(tab_index) {
            let title = resolve_tab_title(tab, || automatic_title.into()).to_string();
            tab.renaming_pane = None;
            tab.rename_buffer = Some(TextField::new(title));
        }
        self.rename_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn rename_pane(
        &mut self,
        _: &RenamePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.tabs.get(self.active_tab).map(|tab| tab.active_pane) else {
            return;
        };
        self.begin_pane_rename(pane_id, window, cx);
    }

    pub(crate) fn begin_pane_rename(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(label) = tab.pane(pane_id).map(TerminalPane::label) else {
            return;
        };
        tab.activate_pane(pane_id);
        tab.renaming_pane = Some(pane_id);
        tab.rename_buffer = Some(TextField::selected(label));
        self.rename_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn is_renaming(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.rename_buffer.is_some())
    }

    pub(crate) fn is_editing_pane_overlay(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.overlay_buffer.is_some())
    }
}

#[cfg(test)]
#[path = "tests/rename.rs"]
mod tests;
