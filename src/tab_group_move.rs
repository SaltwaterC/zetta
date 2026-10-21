//! Moving an existing tab between groups and the destination picker shared by
//! the tab context menu and command palette. Only membership changes: panes,
//! their project context, and running processes stay with the tab. The picker
//! uses stable tab IDs so asynchronous tab changes cannot move a different tab.

use super::*;
use crate::command_palette::PaletteKind;
use crate::project::project_display_name;
use crate::sidebar::SidebarEntry;

/// Labels include the kind and project paths, so identically named projects,
/// worktrees and custom groups remain distinct in the searchable list.
fn destination_label(group: &TabGroup) -> String {
    match group {
        TabGroup::Main => "Main".to_owned(),
        TabGroup::Custom(name) => format!("Tab group: {name}"),
        TabGroup::Project(root) => {
            format!("Project: {} ({})", group.label(), root.display())
        }
        TabGroup::Worktree { project, root } => format!(
            "Worktree: {} · {} ({})",
            project_display_name(project),
            group.label(),
            root.display()
        ),
    }
}

fn destination_palette(
    entries: &[SidebarEntry],
    tab_id: u64,
    current: &TabGroup,
) -> CommandPalette {
    let commands = entries
        .iter()
        .map(SidebarEntry::group)
        .filter(|group| group != current)
        .map(|group| PaletteCommand {
            name: destination_label(&group),
            shortcut: None,
            action: Box::new(MoveTabToGroupDestination { tab_id, group }),
        })
        .collect();
    let mut palette = CommandPalette::new(commands);
    palette.kind = PaletteKind::TabGroupDestination;
    palette
}

impl Zetta {
    pub(crate) fn move_tab_to_group(
        &mut self,
        _: &MoveTabToGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tab_groups_enabled()
            || self.close_tab_confirmation.is_some()
            || self.projects.trust_prompt.is_some()
        {
            return;
        }
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let picker = destination_palette(&self.all_tab_group_entries(), tab.id, &tab.group);
        if self.multi_command.is_some() {
            self.dismiss_multi_command(window, cx);
        }
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if self.is_picking_overlay_style() {
            self.cancel_overlay_style_picker(window, cx);
        }
        self.command_palette = Some(picker);
        self.command_palette_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn move_tab_to_group_destination(
        &mut self,
        action: &MoveTabToGroupDestination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tab_groups_enabled()
            || !self
                .all_tab_group_entries()
                .iter()
                .any(|entry| entry.group() == action.group)
        {
            return;
        }
        let Some(index) = self.tabs.iter().position(|tab| tab.id == action.tab_id) else {
            return;
        };
        if self.tabs[index].group == action.group {
            return;
        }
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        self.tab_groups.forget_tab(action.tab_id);
        self.tabs[index].group = action.group.clone();
        self.tab_groups
            .remember(action.group.clone(), action.tab_id);
        self.active_tab = index;
        self.tab_overflow_selection_side = None;
        self.dismiss_tab_overflow_menus(cx);
        self.disable_tab_move_mode_if_unavailable(cx);
        if let Some(project) = action.group.project_root() {
            self.sidebar.collapsed.remove(project);
        }
        self.focus_active(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
#[path = "tests/tab_group_move.rs"]
mod tests;
