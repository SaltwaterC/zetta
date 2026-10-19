//! Tab groups: which of the window's tabs the tab bar shows.
//!
//! Every tab belongs to one group — Main, a registered project, or one of a
//! project's worktrees — and the tab bar, tab cycling and tab moves work on
//! the active group's tabs only. The sidebar is how the user moves between
//! groups.
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
//! is placed by where its active pane is, not by the group it was in, because
//! nothing outside this window stores groups: a tab's state goes to the daemon
//! as opaque JSON, and growing it would lock out every running Zetta that
//! cannot parse the new field. So a tab inside a worktree returns to that
//! worktree's group, one inside a project to the project's, and anything else
//! to Main — which is also where a project tab the user `cd`'d out of returns.
//! A pane whose directory has been detected already is placed at once; one
//! that has not keeps the group the user is in until its first detection lands
//! ([`TabGroupState::arriving`]), and detections after that leave it alone.
//!
//! `hide_project_sidebar` turns the whole thing off: every tab is in view,
//! cycling covers all of them, and new tabs join Main. Tabs keep the group
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
    /// A registered project, by its registry root.
    Project(PathBuf),
    /// A linked worktree of a registered project.
    Worktree { project: PathBuf, root: PathBuf },
}

impl TabGroup {
    /// The registered project this group belongs to.
    #[cfg_attr(
        not(feature = "worktree"),
        allow(dead_code, reason = "only worktree_ui.rs asks which project is active")
    )]
    pub(crate) fn project_root(&self) -> Option<&Path> {
        match self {
            Self::Main => None,
            Self::Project(root) | Self::Worktree { project: root, .. } => Some(root),
        }
    }

    /// The worktree a worktree group's tabs start in.
    pub(crate) fn worktree_root(&self) -> Option<&Path> {
        match self {
            Self::Worktree { root, .. } => Some(root),
            Self::Main | Self::Project(_) => None,
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Main => "Main".to_owned(),
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

/// The group a tab whose active pane is in `directory` belongs to.
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
    /// Tabs that arrived before their active pane's directory was detected,
    /// waiting for that detection to say which group they belong to.
    arriving: HashSet<u64>,
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
    /// tab — in pin order, into the group its active pane's directory belongs
    /// to; see the module docs. Until that is known, the group the user is
    /// looking at.
    pub(crate) fn insert_tab_into_active_group(&mut self, mut tab: Tab) -> usize {
        tab.group = if !self.tab_groups_enabled() {
            TabGroup::Main
        } else if let Some(group) = self.projects.group_for_pane(tab.active_pane) {
            group.clone()
        } else {
            self.tab_groups.arriving.insert(tab.id);
            self.active_tab_group()
        };
        insert_tab_in_pin_order(&mut self.tabs, tab)
    }

    /// Moves a tab still waiting to be placed into `group`, once a detection
    /// of its active pane has reported which group that is. A detection of any
    /// other pane, or of a tab already placed, changes nothing.
    pub(crate) fn place_arriving_tab(&mut self, tab_id: u64, pane_id: u64, group: &TabGroup) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id && tab.active_pane == pane_id)
        else {
            return;
        };
        if self.tab_groups.arriving.remove(&tab_id) {
            self.tabs[index].group = group.clone();
        }
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
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if tab.group == group {
                self.focus_active(window, cx);
                return;
            }
            self.tab_groups.remember(tab.group.clone(), tab.id);
        }
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        let remembered = self
            .tab_groups
            .remembered(&group)
            .and_then(|tab_id| self.tabs.iter().position(|tab| tab.id == tab_id))
            .filter(|index| self.tabs[*index].group == group);
        let first = || self.tabs.iter().position(|tab| tab.group == group);
        if let Some(index) = remembered.or_else(first) {
            self.active_tab = index;
            self.tab_overflow_selection_side = None;
            self.dismiss_tab_overflow_menus(cx);
            self.disable_tab_move_mode_if_unavailable(cx);
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        self.open_first_tab_in_group(group, window, cx);
    }

    /// Opens another tab in `group`, which becomes the active group: the
    /// sidebar's "New Tab" on a row that is not the one the user is in.
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

    /// A group with no tabs gets one: a plain tab for Main, a project tab
    /// for a project, and a project tab in the worktree for a worktree.
    fn open_first_tab_in_group(
        &mut self,
        group: TabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &group {
            TabGroup::Main => {
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

    /// Moves every tab of `from` into `to`, for a worktree that no longer
    /// exists. Its tabs keep running; they only stop being listed under it.
    #[cfg_attr(
        not(feature = "worktree"),
        allow(dead_code, reason = "only a removed worktree regroups its tabs")
    )]
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
