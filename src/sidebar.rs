//! The project sidebar: Main, the registered projects, and each project's
//! worktrees, listed as the tab groups the tab bar switches between.
//!
//! What a group *is* belongs to `tab_groups`; this module owns what the
//! sidebar shows of them: whether it is open, which projects are collapsed,
//! what each project's repository last reported about its worktrees, and the
//! entry list built from those for one frame. Rendering is
//! `sidebar_render.rs`, and every worktree operation — listing included, since
//! it runs `git` — is `worktree_ui.rs`, which a build without the `worktree`
//! feature replaces with a stub that lists nothing.
//!
//! The entry list is rebuilt from scratch for each render rather than kept in
//! step with the tabs and the registry, because every input to it changes
//! somewhere else: tabs open and close everywhere, the registry is reloaded by
//! the settings page and by other windows, and a listing lands whenever `git`
//! finishes. Building it reads only memory, so a stale entry list is never
//! worth the bookkeeping.

use super::*;
use crate::project::{paths_equal, project_display_name};
use crate::tab_groups::TabGroup;

/// The sidebar's width. Fixed, because a terminal reflows whenever the body
/// beside it changes width, and a sidebar that grew with its longest name
/// would reflow every pane each time a worktree was created.
pub(crate) const SIDEBAR_WIDTH: Pixels = px(240.);

/// One linked worktree, as the sidebar keeps it. Plain data so a build without
/// the `worktree` feature, which has no `zwt`, still names the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SidebarWorktree {
    pub(crate) path: PathBuf,
    /// The `wt/NAME` name, the branch, or the directory name, in that order.
    pub(crate) label: String,
    pub(crate) branch: Option<String>,
    /// The branch `zwt new` based it on; present only for a worktree that
    /// `done`, `abort` and `sync` can operate on.
    pub(crate) source_branch: Option<String>,
}

impl SidebarWorktree {
    pub(crate) fn is_managed(&self) -> bool {
        self.source_branch.is_some()
    }
}

/// What a project's repository last reported.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProjectWorktrees {
    /// Bumped by every refresh, so a listing that took longer than a later one
    /// cannot replace it.
    #[cfg_attr(
        not(feature = "worktree"),
        allow(dead_code, reason = "only worktree_ui.rs refreshes listings")
    )]
    pub(crate) generation: u64,
    pub(crate) loading: bool,
    pub(crate) worktrees: Vec<SidebarWorktree>,
    /// The branch a new worktree would be based on.
    pub(crate) current_branch: Option<String>,
    /// Where a new worktree would be created.
    #[cfg_attr(
        not(feature = "worktree"),
        allow(dead_code, reason = "only worktree_ui.rs creates worktrees")
    )]
    pub(crate) worktree_root: Option<PathBuf>,
    /// Why the project has no listing — usually that it is not a Git
    /// repository. Shown in place of the worktrees rather than as an error,
    /// because a project does not have to be one.
    pub(crate) unavailable: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct SidebarState {
    pub(crate) open: bool,
    /// Projects whose worktrees are hidden. Expanded is the default, so a
    /// worktree the user just created is visible without another click.
    pub(crate) collapsed: HashSet<PathBuf>,
    pub(crate) worktrees: HashMap<PathBuf, ProjectWorktrees>,
    pub(crate) scroll: ScrollHandle,
}

/// A row the sidebar draws, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SidebarEntry {
    Main {
        tab_count: usize,
        active: bool,
    },
    Project {
        root: PathBuf,
        label: String,
        tab_count: usize,
        active: bool,
        expanded: bool,
        /// Whether any worktree rows sit under it, shown or collapsed.
        has_worktrees: bool,
        /// Whether the repository can take a new worktree, as far as the last
        /// listing knows.
        can_create_worktree: bool,
        loading: bool,
        /// Why the project lists no worktrees, when it cannot.
        unavailable: Option<String>,
    },
    Worktree {
        project: PathBuf,
        worktree: SidebarWorktree,
        tab_count: usize,
        active: bool,
    },
    /// A group whose tabs are open but which the registry no longer lists: a
    /// project that was removed, say. Kept so its tabs stay reachable.
    Unlisted {
        group: TabGroup,
        tab_count: usize,
        active: bool,
    },
}

impl SidebarEntry {
    pub(crate) fn group(&self) -> TabGroup {
        match self {
            Self::Main { .. } => TabGroup::Main,
            Self::Project { root, .. } => TabGroup::Project(root.clone()),
            Self::Worktree {
                project, worktree, ..
            } => TabGroup::Worktree {
                project: project.clone(),
                root: worktree.path.clone(),
            },
            Self::Unlisted { group, .. } => group.clone(),
        }
    }

    pub(crate) fn tab_count(&self) -> usize {
        match self {
            Self::Main { tab_count, .. }
            | Self::Project { tab_count, .. }
            | Self::Worktree { tab_count, .. }
            | Self::Unlisted { tab_count, .. } => *tab_count,
        }
    }
}

/// The group after (or before) `active` that cycling moves to, wrapping, in
/// the sidebar's order. `None` when there is nowhere else to go.
///
/// Only groups with open tabs are stops, and Main, so there is always a way
/// back. Visiting an empty group opens a tab in it, and cycling past a project
/// on the way to another would otherwise leave a shell behind in each one;
/// the sidebar and the palette reach an empty group deliberately.
pub(crate) fn adjacent_tab_group(
    entries: &[SidebarEntry],
    active: &TabGroup,
    forward: bool,
) -> Option<TabGroup> {
    let stops = entries
        .iter()
        .filter(|entry| matches!(entry, SidebarEntry::Main { .. }) || entry.tab_count() > 0)
        .map(SidebarEntry::group)
        .collect::<Vec<_>>();
    let position = stops.iter().position(|group| group == active)?;
    if stops.len() < 2 {
        return None;
    }
    let next = if forward {
        (position + 1) % stops.len()
    } else {
        (position + stops.len() - 1) % stops.len()
    };
    Some(stops[next].clone())
}

/// The inputs one frame's entry list is built from.
pub(crate) struct SidebarSources<'a> {
    pub(crate) roots: &'a [PathBuf],
    pub(crate) listings: &'a HashMap<PathBuf, ProjectWorktrees>,
    pub(crate) collapsed: &'a HashSet<PathBuf>,
    pub(crate) counts: &'a HashMap<&'a TabGroup, usize>,
    pub(crate) active: &'a TabGroup,
}

/// Main, then each registered project followed by its worktrees, then any
/// group with open tabs that none of those accounts for.
///
/// A worktree group with open tabs is listed under its project even when the
/// last listing does not include it — the listing may not have arrived yet,
/// or the worktree may have been removed from a shell — because the tabs are
/// what the user is looking for.
pub(crate) fn sidebar_entries(sources: &SidebarSources<'_>) -> Vec<SidebarEntry> {
    let SidebarSources {
        roots,
        listings,
        collapsed,
        counts,
        active,
    } = *sources;
    let count = |group: &TabGroup| counts.get(group).copied().unwrap_or_default();
    let mut listed = HashSet::new();
    let mut entries = vec![SidebarEntry::Main {
        tab_count: count(&TabGroup::Main),
        active: *active == TabGroup::Main,
    }];
    listed.insert(TabGroup::Main);
    for root in roots {
        let group = TabGroup::Project(root.clone());
        let listing = listings.get(root);
        let expanded = !collapsed.contains(root);
        let mut worktrees = listing
            .map(|listing| listing.worktrees.clone())
            .unwrap_or_default();
        for group in counts.keys() {
            if let TabGroup::Worktree {
                project,
                root: path,
            } = group
                && paths_equal(project, root)
                && !worktrees
                    .iter()
                    .any(|worktree| paths_equal(&worktree.path, path))
            {
                worktrees.push(SidebarWorktree {
                    path: path.clone(),
                    label: project_display_name(path).to_owned(),
                    branch: None,
                    source_branch: None,
                });
            }
        }
        entries.push(SidebarEntry::Project {
            root: root.clone(),
            label: project_display_name(root).to_owned(),
            tab_count: count(&group),
            active: *active == group,
            expanded,
            has_worktrees: !worktrees.is_empty(),
            can_create_worktree: listing.is_some_and(|listing| {
                listing.unavailable.is_none() && listing.current_branch.is_some()
            }),
            loading: listing.is_some_and(|listing| listing.loading),
            unavailable: listing.and_then(|listing| listing.unavailable.clone()),
        });
        listed.insert(group);

        for worktree in worktrees {
            let group = TabGroup::Worktree {
                project: root.clone(),
                root: worktree.path.clone(),
            };
            let tab_count = count(&group);
            let active = *active == group;
            listed.insert(group);
            if expanded || active {
                entries.push(SidebarEntry::Worktree {
                    project: root.clone(),
                    worktree,
                    tab_count,
                    active,
                });
            }
        }
    }
    let mut unlisted = counts
        .iter()
        .filter(|(group, _)| !listed.contains(**group))
        .map(|(group, tab_count)| SidebarEntry::Unlisted {
            group: (*group).clone(),
            tab_count: *tab_count,
            active: **group == *active,
        })
        .collect::<Vec<_>>();
    // `counts` is a map, so its order is arbitrary; sort for a stable list.
    unlisted.sort_by_key(|entry| entry.group().label());
    entries.extend(unlisted);
    entries
}

impl Zetta {
    pub(crate) fn sidebar_visible(&self) -> bool {
        self.sidebar.open && self.tab_groups_enabled()
    }

    pub(crate) fn toggle_sidebar(
        &mut self,
        _: &ToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tab_groups_enabled() {
            return;
        }
        self.sidebar.open = !self.sidebar.open;
        if self.sidebar.open {
            self.refresh_all_project_worktrees(cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn activate_tab_group_action(
        &mut self,
        action: &ActivateTabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_tab_group(action.group.clone(), window, cx);
    }

    pub(crate) fn show_main_tabs(
        &mut self,
        _: &ShowMainTabs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_tab_group(TabGroup::Main, window, cx);
    }

    pub(crate) fn next_tab_group(
        &mut self,
        _: &NextTabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_tab_group(true, window, cx);
    }

    pub(crate) fn previous_tab_group(
        &mut self,
        _: &PreviousTabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_tab_group(false, window, cx);
    }

    /// Moves to the adjacent group with tabs. Collapsed projects are not
    /// skipped: collapsing hides rows, it does not close what is in them.
    fn cycle_tab_group(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tab_groups_enabled() {
            return;
        }
        let active = self.active_tab_group();
        let target = {
            let counts = self.tab_group_counts();
            let entries = sidebar_entries(&SidebarSources {
                roots: self.projects.registry.roots(),
                listings: &self.sidebar.worktrees,
                collapsed: &HashSet::new(),
                counts: &counts,
                active: &active,
            });
            adjacent_tab_group(&entries, &active, forward)
        };
        if let Some(group) = target {
            self.activate_tab_group(group, window, cx);
        }
    }

    pub(crate) fn toggle_sidebar_project(&mut self, root: &Path, cx: &mut Context<Self>) {
        if !self.sidebar.collapsed.remove(root) {
            self.sidebar.collapsed.insert(root.to_path_buf());
        }
        cx.notify();
    }

    /// Lists every registered project's worktrees again. Run when the sidebar
    /// opens and when the registry changes, never per frame: each listing is
    /// a few `git` processes.
    pub(crate) fn refresh_all_project_worktrees(&mut self, cx: &mut Context<Self>) {
        let roots = self.projects.registry.roots().to_vec();
        self.sidebar
            .worktrees
            .retain(|root, _| roots.iter().any(|candidate| paths_equal(candidate, root)));
        for root in roots {
            self.refresh_project_worktrees(root, cx);
        }
    }

    /// The registry changed under the sidebar: the settings page added or
    /// removed a project, or another window did.
    pub(crate) fn sidebar_registry_changed(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_visible() {
            self.refresh_all_project_worktrees(cx);
        }
    }

    /// The sidebar's palette commands: one per group the user can move to,
    /// so the sidebar has a keyboard route that does not need it open.
    pub(crate) fn tab_group_palette_commands(&self) -> Vec<PaletteCommand> {
        if !self.tab_groups_enabled() {
            return Vec::new();
        }
        let counts = self.tab_group_counts();
        let active = self.active_tab_group();
        let entries = sidebar_entries(&SidebarSources {
            roots: self.projects.registry.roots(),
            listings: &self.sidebar.worktrees,
            collapsed: &HashSet::new(),
            counts: &counts,
            active: &active,
        });
        entries
            .iter()
            .filter(|entry| !matches!(entry, SidebarEntry::Main { .. }))
            .map(|entry| {
                let group = entry.group();
                let name = match &group {
                    TabGroup::Main => unreachable!("Main has its own action"),
                    TabGroup::Project(root) => format!(
                        "zetta: show project tabs: {} ({})",
                        group.label(),
                        root.display()
                    ),
                    TabGroup::Worktree { project, root } => format!(
                        "zetta: show worktree tabs: {} · {} ({})",
                        project_display_name(project),
                        group.label(),
                        root.display()
                    ),
                };
                PaletteCommand {
                    name,
                    shortcut: None,
                    action: Box::new(ActivateTabGroup { group }),
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "tests/sidebar.rs"]
mod tests;
