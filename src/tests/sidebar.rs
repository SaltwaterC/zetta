use super::*;

fn root(name: &str) -> PathBuf {
    PathBuf::from(format!("/src/{name}"))
}

fn worktree_path(project: &str, name: &str) -> PathBuf {
    PathBuf::from(format!("/src/{project}.worktrees/{name}"))
}

fn listed(path: PathBuf, label: &str) -> SidebarWorktree {
    SidebarWorktree {
        path,
        label: label.to_owned(),
        branch: Some(format!("wt/{label}")),
        source_branch: Some("main".to_owned()),
    }
}

fn listing(worktrees: Vec<SidebarWorktree>) -> ProjectWorktrees {
    ProjectWorktrees {
        worktrees,
        current_branch: Some("main".to_owned()),
        ..ProjectWorktrees::default()
    }
}

fn worktree_group(project: &str, name: &str) -> TabGroup {
    TabGroup::Worktree {
        project: root(project),
        root: worktree_path(project, name),
    }
}

struct Fixture {
    roots: Vec<PathBuf>,
    listings: HashMap<PathBuf, ProjectWorktrees>,
    collapsed: HashSet<PathBuf>,
    groups: Vec<TabGroup>,
}

impl Fixture {
    fn entries(&self, active: &TabGroup) -> Vec<SidebarEntry> {
        let mut counts = HashMap::new();
        for group in &self.groups {
            *counts.entry(group).or_default() += 1;
        }
        sidebar_entries(&SidebarSources {
            roots: &self.roots,
            listings: &self.listings,
            collapsed: &self.collapsed,
            counts: &counts,
            active,
        })
    }
}

fn groups_of(entries: &[SidebarEntry]) -> Vec<TabGroup> {
    entries.iter().map(SidebarEntry::group).collect()
}

#[test]
fn main_comes_first_then_each_project_with_its_worktrees() {
    let fixture = Fixture {
        roots: vec![root("api"), root("web")],
        listings: HashMap::from([(
            root("api"),
            listing(vec![listed(worktree_path("api", "fix"), "fix")]),
        )]),
        collapsed: HashSet::new(),
        groups: vec![
            TabGroup::Main,
            TabGroup::Main,
            TabGroup::Project(root("api")),
        ],
    };

    let entries = fixture.entries(&TabGroup::Project(root("api")));

    assert_eq!(
        groups_of(&entries),
        [
            TabGroup::Main,
            TabGroup::Project(root("api")),
            worktree_group("api", "fix"),
            TabGroup::Project(root("web")),
        ]
    );
    assert_eq!(
        entries[0],
        SidebarEntry::Main {
            tab_count: 2,
            active: false
        }
    );
    let SidebarEntry::Project {
        tab_count,
        active,
        has_worktrees,
        can_create_worktree,
        ..
    } = &entries[1]
    else {
        panic!("expected a project, got {:?}", entries[1]);
    };
    assert_eq!(*tab_count, 1);
    assert!(*active);
    assert!(*has_worktrees);
    assert!(*can_create_worktree);
    let SidebarEntry::Project {
        has_worktrees,
        can_create_worktree,
        ..
    } = &entries[3]
    else {
        panic!("expected a project, got {:?}", entries[3]);
    };
    assert!(!*has_worktrees);
    assert!(
        !*can_create_worktree,
        "nothing is known about an unlisted project"
    );
}

#[test]
fn a_collapsed_project_hides_its_worktrees_except_the_active_one() {
    let fixture = Fixture {
        roots: vec![root("api")],
        listings: HashMap::from([(
            root("api"),
            listing(vec![
                listed(worktree_path("api", "fix"), "fix"),
                listed(worktree_path("api", "feature"), "feature"),
            ]),
        )]),
        collapsed: HashSet::from([root("api")]),
        groups: Vec::new(),
    };

    assert_eq!(
        groups_of(&fixture.entries(&TabGroup::Main)),
        [TabGroup::Main, TabGroup::Project(root("api"))]
    );
    assert_eq!(
        groups_of(&fixture.entries(&worktree_group("api", "feature"))),
        [
            TabGroup::Main,
            TabGroup::Project(root("api")),
            worktree_group("api", "feature"),
        ]
    );
}

#[test]
fn a_worktree_with_tabs_is_listed_before_the_listing_arrives() {
    let fixture = Fixture {
        roots: vec![root("api")],
        listings: HashMap::new(),
        collapsed: HashSet::new(),
        groups: vec![worktree_group("api", "fix")],
    };

    let entries = fixture.entries(&TabGroup::Main);

    assert_eq!(
        groups_of(&entries),
        [
            TabGroup::Main,
            TabGroup::Project(root("api")),
            worktree_group("api", "fix"),
        ]
    );
    let SidebarEntry::Worktree {
        worktree,
        tab_count,
        ..
    } = &entries[2]
    else {
        panic!("expected a worktree, got {:?}", entries[2]);
    };
    assert_eq!(worktree.label, "fix");
    assert_eq!(*tab_count, 1);
    assert!(
        !worktree.is_managed(),
        "nothing offers done or abort until git has been asked"
    );
}

#[test]
fn groups_the_registry_no_longer_lists_stay_reachable_in_a_stable_order() {
    let fixture = Fixture {
        roots: vec![root("api")],
        listings: HashMap::new(),
        collapsed: HashSet::new(),
        groups: vec![
            TabGroup::Project(root("zeta")),
            worktree_group("gone", "fix"),
            TabGroup::Project(root("alpha")),
        ],
    };

    assert_eq!(
        groups_of(&fixture.entries(&TabGroup::Main)),
        [
            TabGroup::Main,
            TabGroup::Project(root("api")),
            TabGroup::Project(root("alpha")),
            worktree_group("gone", "fix"),
            TabGroup::Project(root("zeta")),
        ]
    );
}

#[test]
fn a_project_that_is_not_a_repository_offers_no_worktree() {
    let fixture = Fixture {
        roots: vec![root("notes")],
        listings: HashMap::from([(
            root("notes"),
            ProjectWorktrees {
                unavailable: Some("not a git repository".to_owned()),
                ..ProjectWorktrees::default()
            },
        )]),
        collapsed: HashSet::new(),
        groups: Vec::new(),
    };

    let entries = fixture.entries(&TabGroup::Main);
    let SidebarEntry::Project {
        can_create_worktree,
        unavailable,
        ..
    } = &entries[1]
    else {
        panic!("expected a project, got {:?}", entries[1]);
    };
    assert!(!*can_create_worktree);
    assert_eq!(unavailable.as_deref(), Some("not a git repository"));
}

fn stop(group: TabGroup, tab_count: usize) -> SidebarEntry {
    match group {
        TabGroup::Main => SidebarEntry::Main {
            tab_count,
            active: false,
        },
        group => SidebarEntry::Unlisted {
            group,
            tab_count,
            active: false,
        },
    }
}

#[test]
fn cycling_visits_groups_with_tabs_in_sidebar_order_and_wraps() {
    let entries = [
        stop(TabGroup::Main, 1),
        stop(TabGroup::Project(root("api")), 2),
        stop(worktree_group("api", "fix"), 1),
        stop(TabGroup::Project(root("web")), 1),
    ];

    assert_eq!(
        adjacent_tab_group(&entries, &TabGroup::Main, true),
        Some(TabGroup::Project(root("api")))
    );
    assert_eq!(
        adjacent_tab_group(&entries, &TabGroup::Project(root("api")), true),
        Some(worktree_group("api", "fix"))
    );
    assert_eq!(
        adjacent_tab_group(&entries, &TabGroup::Project(root("web")), true),
        Some(TabGroup::Main)
    );
    assert_eq!(
        adjacent_tab_group(&entries, &TabGroup::Main, false),
        Some(TabGroup::Project(root("web")))
    );
}

#[test]
fn cycling_skips_empty_groups_but_always_stops_at_main() {
    let entries = [
        stop(TabGroup::Main, 0),
        stop(TabGroup::Project(root("api")), 0),
        stop(TabGroup::Project(root("web")), 3),
        stop(worktree_group("web", "fix"), 0),
    ];

    assert_eq!(
        adjacent_tab_group(&entries, &TabGroup::Project(root("web")), true),
        Some(TabGroup::Main)
    );
    assert_eq!(
        adjacent_tab_group(&entries, &TabGroup::Main, true),
        Some(TabGroup::Project(root("web")))
    );
    assert_eq!(
        adjacent_tab_group(&entries, &TabGroup::Main, false),
        Some(TabGroup::Project(root("web")))
    );
}

#[test]
fn cycling_with_nowhere_else_to_go_stays_put() {
    let entries = [
        stop(TabGroup::Main, 4),
        stop(TabGroup::Project(root("api")), 0),
    ];

    assert_eq!(adjacent_tab_group(&entries, &TabGroup::Main, true), None);
    assert_eq!(adjacent_tab_group(&entries, &TabGroup::Main, false), None);
}
