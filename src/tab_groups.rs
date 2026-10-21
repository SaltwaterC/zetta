//! Tab groups: which of the window's tabs the tab bar shows.
//!
//! Every tab belongs to one group — Main, a group the user named, a registered
//! project, or one of a project's worktrees — and the tab bar, tab cycling and
//! tab moves work on the active group's tabs only. The sidebar is how the user
//! moves between groups.
//!
//! A named group is this window's alone and lasts as long as the window does:
//! the registry gives projects and worktrees a life outside it, but nothing
//! records a name, for the same reason given below for tabs.
//!
//! Hidden tabs stay in `Zetta::tabs`. Everything that reaches a tab from
//! outside the tab bar — terminal events and exits, attention routing, shared
//! session snapshots, preserving tabs when the window closes — finds it there
//! by id, and a tab kept anywhere else would silently miss all of it. So a
//! group is a field on the tab and a filter on the view, not a separate list.
//!
//! The active group is the active tab's group. There is no second selection
//! that could disagree with it: anything that activates a tab, from a click to
//! a `zetta tab focus` request, also selects that tab's group. The one moment
//! a group is chosen without a tab is when the user selects an empty one, and
//! that opens a tab in it.
//!
//! A tab arriving from elsewhere — reattached from the multiplexer, resumed
//! from disk, joined from a shared session, reopened after its window closed —
//! is placed by where all its panes are, because nothing outside this window
//! stores groups: a tab's state goes to the daemon
//! as opaque JSON, and growing it would lock out every running Zetta that
//! cannot parse the new field. A tab whose panes all resolve to the same
//! worktree returns to that group, or to a project when all resolve to that
//! project. Mixed directories return to Main, which is also where a project
//! tab the user `cd`'d out of returns. A tab from a named group follows the
//! same directory rule.
//! Saved directories resolved on restore, or earlier detections, place a tab
//! at once. Unknown directories keep it in Main until all panes are known
//! ([`TabGroupState::arriving`]); detections after placement leave it alone.
//! Restoring into a project or worktree reveals the sidebar, enabling grouping
//! for this window even when the saved configuration hid it.
//!
//! `hide_project_sidebar` turns the whole thing off: every tab is in view,
//! cycling covers all of them, and new tabs join Main, until a project restore
//! enables it again. Tabs keep the group
//! they had, so turning the sidebar back on restores the grouping.

use super::*;
use crate::project::{path_is_within_lexically, paths_equal, project_display_name};
use crate::sidebar::ProjectWorktrees;

/// The group a tab belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum TabGroup {
    /// The tabs that belong to no project: what the window held before groups.
    #[default]
    Main,
    /// A group the user created and named, listed under Main. The name is the
    /// identity, so names are unique within a window.
    Custom(String),
    /// A registered project, by its registry root.
    Project(PathBuf),
    /// A linked worktree of a registered project.
    Worktree { project: PathBuf, root: PathBuf },
}

impl TabGroup {
    /// The registered project this group belongs to.
    pub(crate) fn project_root(&self) -> Option<&Path> {
        match self {
            Self::Main | Self::Custom(_) => None,
            Self::Project(root) | Self::Worktree { project: root, .. } => Some(root),
        }
    }

    /// The worktree a worktree group's tabs start in.
    pub(crate) fn worktree_root(&self) -> Option<&Path> {
        match self {
            Self::Worktree { root, .. } => Some(root),
            Self::Main | Self::Custom(_) | Self::Project(_) => None,
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Main => "Main".to_owned(),
            Self::Custom(name) => name.clone(),
            Self::Project(root) => project_display_name(root).to_owned(),
            Self::Worktree { root, .. } => project_display_name(root).to_owned(),
        }
    }
}

/// The positions in `items` the view shows, in order. `None` shows everything.
pub(crate) fn indices_in_group<T>(
    items: &[T],
    group: Option<&TabGroup>,
    group_of: impl Fn(&T) -> &TabGroup,
) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| group.is_none_or(|group| group_of(item) == group))
        .map(|(index, _)| index)
        .collect()
}

/// The index after (or before) `active` among `view`, wrapping. `None` when
/// `view` does not hold `active`, which leaves the selection where it is.
pub(crate) fn cycle_in_view(view: &[usize], active: usize, forward: bool) -> Option<usize> {
    let position = view.iter().position(|&index| index == active)?;
    let next = if forward {
        (position + 1) % view.len()
    } else {
        (position + view.len() - 1) % view.len()
    };
    Some(view[next])
}

/// Which tab becomes active after the tab at `removed_index` left `items`
/// (which no longer holds it), given the active index before the removal.
///
/// Removing a tab other than the active one only shifts the index. Removing
/// the active one keeps the selection in its group — the tab that slid into
/// its place, else the one before it — so closing a project tab does not land
/// the user in some other project. Only when the group is empty does the
/// selection leave it, for `fallback` if that still exists, else Main, else
/// whatever is nearest.
pub(crate) fn active_index_after_removal<T>(
    items: &[T],
    removed_index: usize,
    previous_active: usize,
    removed_group: Option<&TabGroup>,
    fallback: Option<usize>,
    group_of: impl Fn(&T) -> &TabGroup,
) -> usize {
    if items.is_empty() {
        return 0;
    }
    if removed_index < previous_active {
        return previous_active - 1;
    }
    if removed_index > previous_active {
        return previous_active;
    }
    let in_group =
        |index: &usize| removed_group.is_none_or(|group| group_of(&items[*index]) == group);
    (removed_index..items.len())
        .find(in_group)
        .or_else(|| (0..removed_index.min(items.len())).rev().find(in_group))
        .or(fallback.filter(|index| *index < items.len()))
        .or_else(|| (0..items.len()).find(|index| *group_of(&items[*index]) == TabGroup::Main))
        .unwrap_or_else(|| removed_index.min(items.len() - 1))
}

/// What clicking a group's row in the sidebar does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupEntry {
    /// The active tab is already in the group: nothing changes, beyond
    /// returning focus to the terminal.
    AlreadyThere,
    /// The group has tabs; the one at this index becomes active.
    Switch(usize),
    /// The group has no tabs, so entering it opens its first.
    OpenFirstTab,
}

/// What entering `group` does, given the active index and the tab the group
/// remembers as last active there. A remembered tab that has since left the
/// group is ignored for the group's first tab.
pub(crate) fn group_entry<T>(
    items: &[T],
    active: usize,
    remembered: Option<usize>,
    group: &TabGroup,
    group_of: impl Fn(&T) -> &TabGroup,
) -> GroupEntry {
    let in_group = |index: &usize| {
        items
            .get(*index)
            .is_some_and(|item| group_of(item) == group)
    };
    if in_group(&active) {
        return GroupEntry::AlreadyThere;
    }
    remembered
        .filter(in_group)
        .or_else(|| (0..items.len()).find(in_group))
        .map_or(GroupEntry::OpenFirstTab, GroupEntry::Switch)
}

/// The name a new or renamed group is given, trimmed, or why it cannot have
/// it. `renaming` is the group's current name, which it may keep.
///
/// Names are compared ignoring case, so the sidebar never shows two rows a
/// reader could not tell apart, and Main's name is taken.
pub(crate) fn validate_tab_group_name(
    existing: &[String],
    name: &str,
    renaming: Option<&str>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name for the tab group.".to_owned());
    }
    let folded = name.to_lowercase();
    let taken = |other: &str| {
        other.to_lowercase() == folded && renaming.is_none_or(|current| current != other)
    };
    if taken("Main") || existing.iter().any(|other| taken(other)) {
        return Err(format!("A tab group named {name} already exists."));
    }
    Ok(name.to_owned())
}

/// What a detection learned about a pane's directory, for
/// [`group_for_detected_directory`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct DetectedDirectory<'a> {
    /// The directory as the shell reported it.
    pub(crate) reported: &'a Path,
    /// The same directory as the filesystem spells it, when it resolved.
    pub(crate) canonical: Option<&'a Path>,
    /// The registry root of the project the directory is in.
    pub(crate) registered_root: Option<&'a Path>,
    /// The root of the managed worktree the directory is in. Only meaningful
    /// alongside `registered_root`, the project the worktree belongs to.
    pub(crate) managed_worktree: Option<&'a Path>,
}

/// The group a pane in `directory` belongs to.
///
/// A managed worktree knows its project from its own metadata. Any other
/// linked worktree is recognised only from the sidebar's listings, which the
/// project's repository reported; the innermost listed worktree containing the
/// directory wins, so one nested inside its main repository is not mistaken for
/// the project. Without either, the directory's project, else Main.
pub(crate) fn group_for_detected_directory(
    directory: &DetectedDirectory<'_>,
    listings: &HashMap<PathBuf, ProjectWorktrees>,
) -> TabGroup {
    if let (Some(project), Some(worktree)) = (directory.registered_root, directory.managed_worktree)
    {
        // The listing's spelling where it has one, so the group is the very
        // one the sidebar lists rather than a second entry beside it.
        let root = listings
            .get(project)
            .and_then(|listing| {
                listing
                    .worktrees
                    .iter()
                    .find(|listed| paths_equal(&listed.path, worktree))
            })
            .map_or_else(|| worktree.to_path_buf(), |listed| listed.path.clone());
        return TabGroup::Worktree {
            project: project.to_path_buf(),
            root,
        };
    }
    let contains = |path: &Path| {
        path_is_within_lexically(directory.reported, path)
            || directory
                .canonical
                .is_some_and(|canonical| path_is_within_lexically(canonical, path))
    };
    let listed = listings
        .iter()
        .filter(|(project, _)| {
            directory
                .registered_root
                .is_none_or(|registered| paths_equal(registered, project))
        })
        .flat_map(|(project, listing)| {
            listing
                .worktrees
                .iter()
                .map(move |worktree| (project, &worktree.path))
        })
        .filter(|(_, path)| contains(path))
        .max_by_key(|(_, path)| path.components().count());
    if let Some((project, root)) = listed {
        return TabGroup::Worktree {
            project: project.clone(),
            root: root.clone(),
        };
    }
    directory
        .registered_root
        .map_or(TabGroup::Main, |root| TabGroup::Project(root.to_path_buf()))
}

/// Places a tab only when every directory belongs to the same group. A known
/// mismatch settles it in Main; otherwise unknown directories defer placement.
pub(crate) fn common_tab_group<'a>(
    groups: impl IntoIterator<Item = Option<&'a TabGroup>>,
) -> Option<TabGroup> {
    let mut common = None;
    let mut unknown = false;
    for group in groups {
        let Some(group) = group else {
            unknown = true;
            continue;
        };
        if *group == TabGroup::Main || common.is_some_and(|common| common != group) {
            return Some(TabGroup::Main);
        }
        common = Some(group);
    }
    if unknown {
        None
    } else {
        Some(common.cloned().unwrap_or_default())
    }
}

fn group_for_arriving_tab(tab: &Tab, projects: &ProjectState) -> Option<TabGroup> {
    common_tab_group(tab.panes.iter().flat_map(|pane| {
        std::iter::once(projects.group_for_pane(pane.id)).chain(
            pane.stack
                .entries
                .iter()
                .map(|entry| projects.group_for_pane(entry.id)),
        )
    }))
}

/// What the window remembers about groups between selections.
#[derive(Debug, Default)]
pub(crate) struct TabGroupState {
    /// The tab each group last had active, so returning to a group returns to
    /// that tab rather than to its first one.
    remembered: HashMap<TabGroup, u64>,
    /// The group the tab being opened right now belongs to, when the opener
    /// chose one. Set and consumed within the same synchronous call, so a tab
    /// opened by anything else cannot take it.
    opening: Option<TabGroup>,
    /// Tabs waiting for enough pane directories to determine a common group.
    arriving: HashSet<u64>,
    /// The names of the groups the user created, in the order they were
    /// created, which is the order the sidebar lists them in.
    custom: Vec<String>,
}

impl TabGroupState {
    pub(crate) fn remember(&mut self, group: TabGroup, tab_id: u64) {
        self.remembered.insert(group, tab_id);
    }

    pub(crate) fn remembered(&self, group: &TabGroup) -> Option<u64> {
        self.remembered.get(group).copied()
    }

    pub(crate) fn forget_tab(&mut self, tab_id: u64) {
        self.remembered
            .retain(|_, remembered| *remembered != tab_id);
        self.arriving.remove(&tab_id);
    }

    /// Stops waiting to place `tab_id`, returning whether it was waiting.
    pub(crate) fn arrived(&mut self, tab_id: u64) -> bool {
        self.arriving.remove(&tab_id)
    }

    pub(crate) fn custom_groups(&self) -> &[String] {
        &self.custom
    }

    /// Adds a group named `name`, which the caller has validated.
    pub(crate) fn add_custom(&mut self, name: String) {
        self.custom.push(name);
    }

    /// Renames a group in place, keeping its position and the tab it
    /// remembers. Tabs carry their group, so the caller moves those.
    pub(crate) fn rename_custom(&mut self, from: &str, to: String) {
        if let Some(name) = self.custom.iter_mut().find(|name| *name == from) {
            *name = to.clone();
        }
        if let Some(tab_id) = self.remembered.remove(&TabGroup::Custom(from.to_owned())) {
            self.remembered.insert(TabGroup::Custom(to), tab_id);
        }
    }

    pub(crate) fn remove_custom(&mut self, name: &str) {
        self.custom.retain(|other| other != name);
        self.remembered.remove(&TabGroup::Custom(name.to_owned()));
    }
}

impl Zetta {
    /// Whether tabs are grouped at all; see the module docs.
    pub(crate) fn tab_groups_enabled(&self) -> bool {
        !self.launch_config.hide_project_sidebar
    }

    /// The group the view shows: the active tab's, or `None` for every tab.
    pub(crate) fn tab_group_filter(&self) -> Option<&TabGroup> {
        if !self.tab_groups_enabled() {
            return None;
        }
        self.tabs.get(self.active_tab).map(|tab| &tab.group)
    }

    /// The group the user is in, for the sidebar's selection. Main while
    /// grouping is off or the window has no tab.
    pub(crate) fn active_tab_group(&self) -> TabGroup {
        self.tab_group_filter().cloned().unwrap_or_default()
    }

    /// The indices of the tabs the tab bar shows, in order.
    pub(crate) fn tabs_in_view(&self) -> Vec<usize> {
        indices_in_group(&self.tabs, self.tab_group_filter(), |tab| &tab.group)
    }

    /// The group a tab being opened now joins.
    ///
    /// An explicit choice wins. Entering a project lands in that project's
    /// group — however it was entered, from the palette or `zetta project
    /// open` as much as the sidebar. Anything else continues the session the
    /// user is in, so a new tab opens next to the one they were looking at.
    pub(crate) fn group_for_new_tab(
        &mut self,
        origin: NewTabOrigin,
        project: Option<&ProjectConfig>,
    ) -> TabGroup {
        let explicit = self.tab_groups.opening.take();
        if !self.tab_groups_enabled() {
            return TabGroup::Main;
        }
        if let Some(group) = explicit {
            return group;
        }
        match (origin, project) {
            (NewTabOrigin::ProjectEntry, Some(project)) => TabGroup::Project(
                self.projects
                    .registry
                    .matching_root_lexically(&project.root)
                    .cloned()
                    .unwrap_or_else(|| project.root.clone()),
            ),
            (NewTabOrigin::GroupEntry, _) => TabGroup::Main,
            _ => self.active_tab_group(),
        }
    }

    /// Runs `open` with `group` as the group of whatever tab it opens. An open
    /// that gives up before opening one leaves nothing behind for the next.
    pub(crate) fn with_next_tab_in_group(&mut self, group: TabGroup, open: impl FnOnce(&mut Self)) {
        self.tab_groups.opening = Some(group);
        open(self);
        self.tab_groups.opening = None;
    }

    /// Inserts a tab arriving from elsewhere — a reconnected session, a shared
    /// tab — in pin order, into the group all its directories belong to;
    /// see the module docs. Until that is known, Main.
    pub(crate) fn insert_tab_into_active_group(
        &mut self,
        mut tab: Tab,
        cx: &mut Context<Self>,
    ) -> usize {
        tab.group = if let Some(group) = group_for_arriving_tab(&tab, &self.projects) {
            group
        } else {
            self.tab_groups.arriving.insert(tab.id);
            TabGroup::Main
        };
        self.reveal_restored_tab_group(&tab.group, cx);
        insert_tab_in_pin_order(&mut self.tabs, tab)
    }

    /// Reconsiders a waiting tab after a pane detection. One pane alone cannot
    /// place a multi-pane tab; detections after placement leave it alone.
    pub(crate) fn place_arriving_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        if !self.tab_groups.arriving.contains(&tab_id) {
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(group) = group_for_arriving_tab(tab, &self.projects) else {
            return;
        };
        self.tab_groups.arriving.remove(&tab_id);
        tab.group = group.clone();
        self.reveal_restored_tab_group(&group, cx);
    }

    /// Settles `active_tab` once the tab at `removed_index`, which belonged to
    /// `removed_group`, has left `self.tabs`.
    pub(crate) fn settle_active_tab_after_removal(
        &mut self,
        removed_index: usize,
        removed_group: &TabGroup,
    ) {
        let filter = self.tab_groups_enabled().then_some(removed_group);
        let fallback = self
            .tab_groups
            .remembered(&TabGroup::Main)
            .and_then(|tab_id| self.tabs.iter().position(|tab| tab.id == tab_id));
        self.active_tab = active_index_after_removal(
            &self.tabs,
            removed_index,
            self.active_tab,
            filter,
            fallback,
            |tab| &tab.group,
        );
    }

    /// Moves the view to `group`: back to the tab it last had active, or to
    /// its first tab, or — for a group with no tabs yet — to a new tab in it.
    pub(crate) fn activate_tab_group(
        &mut self,
        group: TabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tab_groups_enabled() {
            return;
        }
        let remembered = self
            .tab_groups
            .remembered(&group)
            .and_then(|tab_id| self.tabs.iter().position(|tab| tab.id == tab_id));
        let entry = group_entry(&self.tabs, self.active_tab, remembered, &group, |tab| {
            &tab.group
        });
        if entry == GroupEntry::AlreadyThere {
            self.focus_active(window, cx);
            return;
        }
        if let Some(tab) = self.tabs.get(self.active_tab) {
            self.tab_groups.remember(tab.group.clone(), tab.id);
        }
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        match entry {
            GroupEntry::AlreadyThere => {}
            GroupEntry::Switch(index) => {
                self.active_tab = index;
                self.tab_overflow_selection_side = None;
                self.dismiss_tab_overflow_menus(cx);
                self.disable_tab_move_mode_if_unavailable(cx);
                self.focus_active(window, cx);
                cx.notify();
            }
            GroupEntry::OpenFirstTab => self.open_first_tab_in_group(group, window, cx),
        }
    }

    /// Opens another tab in `group`, which becomes the active group: the
    /// "New Tab" entry of a sidebar row's context menu.
    pub(crate) fn open_new_tab_in_group(
        &mut self,
        group: TabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tab_groups_enabled() {
            return;
        }
        if let Some(tab) = self.tabs.get(self.active_tab) {
            self.tab_groups.remember(tab.group.clone(), tab.id);
        }
        self.open_first_tab_in_group(group, window, cx);
    }

    /// A group with no tabs gets one: a plain tab for Main or a named group, a project tab
    /// for a project, and a project tab in the worktree for a worktree.
    fn open_first_tab_in_group(
        &mut self,
        group: TabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &group {
            TabGroup::Main | TabGroup::Custom(_) => {
                let Some(profile) = new_tab_profile(
                    None,
                    &self.profiles,
                    self.launch_config.default_profile,
                    self.launch_config.new_tab_profile,
                ) else {
                    return;
                };
                self.with_next_tab_in_group(group, |this| {
                    this.open_tab_with_profile_context(
                        profile,
                        None,
                        NewTabOrigin::GroupEntry,
                        None,
                        None,
                        TerminalLaunch::Spawn,
                        window,
                        cx,
                    );
                });
            }
            TabGroup::Project(root) => {
                let root = root.clone();
                self.open_project_tab_in_group(root, None, group, window, cx);
            }
            TabGroup::Worktree { project, root } => {
                let (project, root) = (project.clone(), root.clone());
                self.open_project_tab_in_group(project, Some(root), group, window, cx);
            }
        }
    }

    /// Moves every tab of `from` into `to`, for a group that no longer
    /// exists or has a new name. Its tabs keep running; they only stop being
    /// listed under it.
    pub(crate) fn regroup_tabs(&mut self, from: &TabGroup, to: TabGroup, cx: &mut Context<Self>) {
        let mut moved = false;
        for tab in self.tabs.iter_mut().filter(|tab| tab.group == *from) {
            tab.group = to.clone();
            moved = true;
        }
        self.tab_groups.remembered.remove(from);
        if moved {
            cx.notify();
        }
    }

    /// Creates a group named `name`, which the caller has validated, and
    /// enters it — which opens its first tab.
    pub(crate) fn create_tab_group(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_groups.add_custom(name.clone());
        self.activate_tab_group(TabGroup::Custom(name), window, cx);
    }

    /// Renames a group, which the caller has validated, taking its tabs along.
    pub(crate) fn rename_tab_group(&mut self, from: &str, to: String, cx: &mut Context<Self>) {
        self.tab_groups.rename_custom(from, to.clone());
        self.regroup_tabs(&TabGroup::Custom(from.to_owned()), TabGroup::Custom(to), cx);
        cx.notify();
    }

    /// Removes a group. Its tabs keep running and move to Main, like a removed
    /// worktree's move to its project.
    pub(crate) fn remove_tab_group(&mut self, name: &str, cx: &mut Context<Self>) {
        self.tab_groups.remove_custom(name);
        self.regroup_tabs(&TabGroup::Custom(name.to_owned()), TabGroup::Main, cx);
        cx.notify();
    }

    /// How many tabs each group holds, for the sidebar's counts.
    pub(crate) fn tab_group_counts(&self) -> HashMap<&TabGroup, usize> {
        let mut counts = HashMap::new();
        for tab in &self.tabs {
            *counts.entry(&tab.group).or_default() += 1;
        }
        counts
    }
}

#[cfg(test)]
#[path = "tests/tab_groups.rs"]
mod tests;
