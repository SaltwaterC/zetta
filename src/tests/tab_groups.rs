use super::*;

fn project(name: &str) -> TabGroup {
    TabGroup::Project(PathBuf::from(format!("/src/{name}")))
}

fn worktree(project: &str, name: &str) -> TabGroup {
    TabGroup::Worktree {
        project: PathBuf::from(format!("/src/{project}")),
        root: PathBuf::from(format!("/src/{project}.worktrees/{name}")),
    }
}

#[gpui::test]
fn opening_a_project_reveals_its_group_even_when_the_sidebar_was_disabled(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear(); // No shell or daemon is started by this test.
        let mut zetta = Zetta::new(
            config,
            None,
            ZettaLaunchOptions {
                no_mux: true,
                ..Default::default()
            },
            window,
            cx,
        );
        zetta.projects.registry = crate::project::ProjectRegistry::empty();
        zetta
    });
    zetta.update(cx, |zetta, cx| {
        let project = ProjectConfig {
            pending_approval: None,
            root: PathBuf::from("/src/api"),
            effective: zetta.launch_config.clone(),
            theme: None,
            dark_theme: None,
            environment: HashMap::new(),
            commands: Default::default(),
            initial_split: None,
        };
        for hidden_by_setting in [false, true] {
            for already_open in [false, true] {
                for explicit in [None, Some(worktree("api", "fix"))] {
                    zetta.launch_config.hide_project_sidebar = hidden_by_setting;
                    zetta.sidebar.open = already_open;
                    zetta.sidebar.collapsed.insert(project.root.clone());
                    zetta.tab_groups.opening = explicit.clone();

                    let group =
                        zetta.group_for_new_tab(NewTabOrigin::ProjectEntry, Some(&project), cx);

                    assert_eq!(
                        group,
                        explicit.unwrap_or_else(|| TabGroup::Project(project.root.clone()))
                    );
                    assert!(zetta.sidebar_visible());
                    assert!(!zetta.sidebar.collapsed.contains(&project.root));
                    assert!(zetta.tab_groups.opening.is_none());

                    // Closing the sidebar must last through ordinary new tabs,
                    // including those that inherit a project context.
                    zetta.launch_config.hide_project_sidebar = hidden_by_setting;
                    zetta.sidebar.open = false;
                    zetta.group_for_new_tab(NewTabOrigin::CurrentSession, Some(&project), cx);
                    assert!(!zetta.sidebar_visible());
                    assert_eq!(zetta.launch_config.hide_project_sidebar, hidden_by_setting);
                }
            }
        }
    });
}

#[test]
fn a_group_lists_only_its_own_tabs_in_order() {
    let groups = [
        TabGroup::Main,
        project("api"),
        TabGroup::Main,
        worktree("api", "fix"),
        project("api"),
    ];

    assert_eq!(
        indices_in_group(&groups, Some(&TabGroup::Main), |group| group),
        [0, 2]
    );
    assert_eq!(
        indices_in_group(&groups, Some(&project("api")), |group| group),
        [1, 4]
    );
    assert_eq!(
        indices_in_group(&groups, Some(&worktree("api", "fix")), |group| group),
        [3]
    );
    assert_eq!(
        indices_in_group(&groups, None, |group| group),
        [0, 1, 2, 3, 4]
    );
}

#[test]
fn cycling_wraps_within_the_view() {
    let view = [1, 4, 6];

    assert_eq!(cycle_in_view(&view, 1, true), Some(4));
    assert_eq!(cycle_in_view(&view, 6, true), Some(1));
    assert_eq!(cycle_in_view(&view, 1, false), Some(6));
    assert_eq!(cycle_in_view(&view, 4, false), Some(1));
    assert_eq!(cycle_in_view(&[3], 3, true), Some(3));
}

#[test]
fn cycling_from_a_tab_outside_the_view_leaves_the_selection_alone() {
    assert_eq!(cycle_in_view(&[1, 4], 2, true), None);
    assert_eq!(cycle_in_view(&[], 0, true), None);
}

#[test]
fn closing_a_tab_before_the_active_one_only_shifts_it() {
    let after = [TabGroup::Main, project("api"), project("api")];

    assert_eq!(
        active_index_after_removal(&after, 0, 2, Some(&project("api")), None, |group| group),
        1
    );
    assert_eq!(
        active_index_after_removal(&after, 3, 2, Some(&project("api")), None, |group| group),
        2
    );
}

#[test]
fn closing_the_active_tab_stays_in_its_group() {
    // Removed: index 1, a project tab, between Main and the project's other.
    let after = [TabGroup::Main, TabGroup::Main, project("api")];

    assert_eq!(
        active_index_after_removal(&after, 1, 1, Some(&project("api")), None, |group| group),
        2,
        "the next tab in the group, not the Main tab that slid into its place"
    );

    let after = [project("api"), TabGroup::Main, TabGroup::Main];
    assert_eq!(
        active_index_after_removal(&after, 3, 3, Some(&project("api")), None, |group| group),
        0,
        "the previous tab in the group when it was the last"
    );
}

#[test]
fn closing_a_groups_last_tab_falls_back_then_prefers_main() {
    let after = [project("web"), TabGroup::Main, project("web")];

    assert_eq!(
        active_index_after_removal(&after, 1, 1, Some(&project("api")), Some(2), |group| group),
        2,
        "the remembered fallback"
    );
    assert_eq!(
        active_index_after_removal(&after, 1, 1, Some(&project("api")), None, |group| group),
        1,
        "Main when nothing is remembered"
    );
    assert_eq!(
        active_index_after_removal(&after, 1, 1, Some(&project("api")), Some(9), |group| group),
        1,
        "a fallback that no longer exists is ignored"
    );

    let after = [project("web"), project("web")];
    assert_eq!(
        active_index_after_removal(&after, 2, 2, Some(&project("api")), None, |group| group),
        1,
        "the nearest tab when there is no Main"
    );
}

#[test]
fn closing_without_groups_keeps_the_ungrouped_selection() {
    let after = [project("web"), TabGroup::Main];

    assert_eq!(
        active_index_after_removal(&after, 1, 1, None, None, |group| group),
        1
    );
    assert_eq!(
        active_index_after_removal(&after, 2, 2, None, None, |group| group),
        1
    );
    let empty: [TabGroup; 0] = [];
    assert_eq!(
        active_index_after_removal(&empty, 0, 0, None, None, |group| group),
        0
    );
}

#[test]
fn entering_an_empty_group_opens_its_first_tab() {
    let groups = [TabGroup::Main, project("api")];

    assert_eq!(
        group_entry(&groups, 0, None, &project("web"), |group| group),
        GroupEntry::OpenFirstTab
    );
    assert_eq!(
        group_entry(&groups, 0, Some(1), &project("web"), |group| group),
        GroupEntry::OpenFirstTab,
        "a remembered tab that has left the group does not count as one of its tabs"
    );
    let empty: [TabGroup; 0] = [];
    assert_eq!(
        group_entry(&empty, 0, None, &TabGroup::Main, |group| group),
        GroupEntry::OpenFirstTab
    );
}

#[test]
fn entering_the_group_the_user_is_in_opens_nothing() {
    let groups = [TabGroup::Main, project("api"), project("api")];

    assert_eq!(
        group_entry(&groups, 1, None, &project("api"), |group| group),
        GroupEntry::AlreadyThere
    );
    assert_eq!(
        group_entry(&groups, 2, Some(1), &project("api"), |group| group),
        GroupEntry::AlreadyThere,
        "not even a switch back to the remembered tab"
    );
}

#[test]
fn entering_a_group_with_tabs_returns_to_one_rather_than_opening_another() {
    let groups = [
        TabGroup::Main,
        project("api"),
        worktree("api", "fix"),
        project("api"),
    ];

    assert_eq!(
        group_entry(&groups, 0, Some(3), &project("api"), |group| group),
        GroupEntry::Switch(3),
        "the tab it was last on"
    );
    assert_eq!(
        group_entry(&groups, 0, None, &project("api"), |group| group),
        GroupEntry::Switch(1),
        "else its first"
    );
    assert_eq!(
        group_entry(&groups, 1, Some(2), &project("api"), |group| group),
        GroupEntry::AlreadyThere
    );
    assert_eq!(
        group_entry(&groups, 1, Some(2), &TabGroup::Main, |group| group),
        GroupEntry::Switch(0),
        "a remembered tab from another group is ignored"
    );
}

#[test]
fn a_group_remembers_its_tab_until_the_tab_closes() {
    let mut state = TabGroupState::default();
    state.remember(project("api"), 7);
    state.remember(TabGroup::Main, 3);

    assert_eq!(state.remembered(&project("api")), Some(7));
    state.forget_tab(7);
    assert_eq!(state.remembered(&project("api")), None);
    assert_eq!(state.remembered(&TabGroup::Main), Some(3));
}

#[test]
fn groups_name_their_project_and_worktree() {
    let group = worktree("api", "fix");

    assert_eq!(group.label(), "fix");
    assert_eq!(group.project_root(), Some(Path::new("/src/api")));
    assert_eq!(
        group.worktree_root(),
        Some(Path::new("/src/api.worktrees/fix"))
    );
    assert_eq!(project("api").label(), "api");
    assert_eq!(project("api").worktree_root(), None);
    assert_eq!(TabGroup::Main.label(), "Main");
    assert_eq!(TabGroup::Main.project_root(), None);
}

fn listing(paths: &[&str]) -> ProjectWorktrees {
    ProjectWorktrees {
        worktrees: paths
            .iter()
            .map(|path| crate::sidebar::SidebarWorktree {
                path: PathBuf::from(path),
                label: project_display_name(Path::new(path)).to_owned(),
                branch: None,
                source_branch: None,
            })
            .collect(),
        ..ProjectWorktrees::default()
    }
}

fn detected<'a>(
    reported: &'a str,
    registered_root: Option<&'a str>,
    managed_worktree: Option<&'a str>,
) -> DetectedDirectory<'a> {
    DetectedDirectory {
        reported: Path::new(reported),
        canonical: None,
        registered_root: registered_root.map(Path::new),
        managed_worktree: managed_worktree.map(Path::new),
    }
}

#[test]
fn an_arriving_tab_in_a_managed_worktree_returns_to_its_group() {
    let directory = detected(
        "/src/api.worktrees/fix/src",
        Some("/src/api"),
        Some("/src/api.worktrees/fix"),
    );

    assert_eq!(
        group_for_detected_directory(&directory, &HashMap::new()),
        worktree("api", "fix")
    );
}

#[test]
fn a_managed_worktree_takes_the_spelling_its_listing_uses() {
    let listings = HashMap::from([(
        PathBuf::from("/src/api"),
        listing(&["/src/api.worktrees/fix"]),
    )]);
    let directory = detected(
        "/src/api.worktrees/fix",
        Some("/src/api"),
        Some("/src/api.worktrees/fix"),
    );

    assert_eq!(
        group_for_detected_directory(&directory, &listings),
        worktree("api", "fix")
    );
}

#[test]
fn an_unmanaged_worktree_is_recognised_from_its_projects_listing() {
    let listings = HashMap::from([(
        PathBuf::from("/src/api"),
        listing(&["/src/api.worktrees/fix"]),
    )]);

    assert_eq!(
        group_for_detected_directory(
            &detected("/src/api.worktrees/fix/docs", None, None),
            &listings
        ),
        worktree("api", "fix")
    );
}

#[test]
fn the_innermost_listed_worktree_wins_over_the_project_it_is_nested_in() {
    let listings = HashMap::from([(
        PathBuf::from("/src/api"),
        listing(&["/src/api/.worktrees/fix"]),
    )]);

    assert_eq!(
        group_for_detected_directory(
            &detected("/src/api/.worktrees/fix/src", Some("/src/api"), None),
            &listings
        ),
        TabGroup::Worktree {
            project: PathBuf::from("/src/api"),
            root: PathBuf::from("/src/api/.worktrees/fix"),
        }
    );
    assert_eq!(
        group_for_detected_directory(&detected("/src/api/src", Some("/src/api"), None), &listings),
        project("api")
    );
}

#[test]
fn another_projects_listing_does_not_claim_a_directory_in_this_one() {
    let listings = HashMap::from([(PathBuf::from("/src/web"), listing(&["/src/api/vendor"]))]);

    assert_eq!(
        group_for_detected_directory(
            &detected("/src/api/vendor/lib", Some("/src/api"), None),
            &listings
        ),
        project("api")
    );
}

#[test]
fn the_canonical_spelling_matches_a_listing_the_reported_one_misses() {
    let listings = HashMap::from([(
        PathBuf::from("/src/api"),
        listing(&["/src/api.worktrees/fix"]),
    )]);
    let directory = DetectedDirectory {
        canonical: Some(Path::new("/src/api.worktrees/fix/src")),
        ..detected("/home/me/fix/src", None, None)
    };

    assert_eq!(
        group_for_detected_directory(&directory, &listings),
        worktree("api", "fix")
    );
}

#[test]
fn a_directory_in_no_project_returns_to_main() {
    let listings = HashMap::from([(
        PathBuf::from("/src/api"),
        listing(&["/src/api.worktrees/fix"]),
    )]);

    assert_eq!(
        group_for_detected_directory(&detected("/tmp", None, None), &listings),
        TabGroup::Main
    );
    // A sibling whose name only starts with the worktree's is not inside it.
    assert_eq!(
        group_for_detected_directory(&detected("/src/api.worktrees/fix-2", None, None), &listings),
        TabGroup::Main
    );
}

#[test]
fn a_tab_waits_to_be_placed_only_until_it_arrives_or_closes() {
    let mut state = TabGroupState::default();
    state.arriving.insert(7);
    state.arriving.insert(8);

    assert!(state.arrived(7));
    assert!(!state.arrived(7));
    state.forget_tab(8);
    assert!(!state.arrived(8));
}

#[test]
fn every_directory_must_agree_before_a_tab_restores_to_a_project() {
    let api = project("api");
    let web = project("web");
    let fix = worktree("api", "fix");
    assert_eq!(
        common_tab_group([Some(&api), Some(&api)]),
        Some(api.clone())
    );
    assert_eq!(
        common_tab_group([Some(&fix), Some(&fix)]),
        Some(fix.clone())
    );
    for groups in [
        [Some(&api), Some(&web)],
        [Some(&web), Some(&api)],
        [Some(&api), Some(&fix)],
        [Some(&api), Some(&TabGroup::Main)],
        [Some(&TabGroup::Main), Some(&api)],
    ] {
        assert_eq!(common_tab_group(groups), Some(TabGroup::Main));
    }
    assert_eq!(common_tab_group([Some(&api), None]), None);
    assert_eq!(common_tab_group([None, Some(&api)]), None);
    assert_eq!(common_tab_group([None, None]), None);
    // A known outside directory rules out a project even before the others arrive.
    assert_eq!(
        common_tab_group([None, Some(&TabGroup::Main)]),
        Some(TabGroup::Main)
    );
}

#[test]
fn a_tab_group_name_must_not_be_blank() {
    assert!(validate_tab_group_name(&[], "", None).is_err());
    assert!(validate_tab_group_name(&[], "   ", None).is_err());
    assert_eq!(
        validate_tab_group_name(&[], "  scratch ", None),
        Ok("scratch".to_owned()),
        "surrounding whitespace is not part of the name"
    );
}

#[test]
fn a_tab_group_name_must_not_be_taken_ignoring_case() {
    let existing = ["Scratch".to_owned(), "Deploys".to_owned()];

    assert!(validate_tab_group_name(&existing, "scratch", None).is_err());
    assert!(validate_tab_group_name(&existing, "main", None).is_err());
    assert!(validate_tab_group_name(&existing, "Ünïcode", None).is_ok());
    assert!(validate_tab_group_name(&["ÜNÏCODE".to_owned()], "ünïcode", None).is_err());
    assert_eq!(
        validate_tab_group_name(&existing, "scratch", Some("Scratch")),
        Ok("scratch".to_owned()),
        "a rename may change only the case of its own name"
    );
    assert!(validate_tab_group_name(&existing, "deploys", Some("Scratch")).is_err());
}

#[test]
fn renaming_a_tab_group_keeps_its_place_and_remembered_tab() {
    let mut state = TabGroupState::default();
    state.add_custom("one".to_owned());
    state.add_custom("two".to_owned());
    state.remember(TabGroup::Custom("one".to_owned()), 9);

    state.rename_custom("one", "first".to_owned());

    assert_eq!(state.custom_groups(), ["first", "two"]);
    assert_eq!(
        state.remembered(&TabGroup::Custom("first".to_owned())),
        Some(9)
    );
    assert_eq!(state.remembered(&TabGroup::Custom("one".to_owned())), None);

    state.remove_custom("first");
    assert_eq!(state.custom_groups(), ["two"]);
    assert_eq!(
        state.remembered(&TabGroup::Custom("first".to_owned())),
        None
    );
}

#[test]
fn a_named_group_is_not_a_project() {
    let group = TabGroup::Custom("scratch".to_owned());

    assert_eq!(group.label(), "scratch");
    assert_eq!(group.project_root(), None);
    assert_eq!(group.worktree_root(), None);
}
