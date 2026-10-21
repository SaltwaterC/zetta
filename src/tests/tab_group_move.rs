use super::*;

use crate::project::ProjectRegistry;
use crate::sidebar::{ProjectWorktrees, SidebarSources, SidebarWorktree, sidebar_entries};

#[test]
fn tab_group_destinations_include_empty_groups_but_not_the_source() {
    let project = PathBuf::from("/src/api");
    let worktree = PathBuf::from("/src/api.worktrees/fix");
    let current = TabGroup::Custom("scratch".to_owned());
    let entries = sidebar_entries(&SidebarSources {
        custom: &["scratch".to_owned(), "empty".to_owned()],
        roots: std::slice::from_ref(&project),
        listings: &HashMap::from([(
            project.clone(),
            ProjectWorktrees {
                worktrees: vec![SidebarWorktree {
                    path: worktree.clone(),
                    label: "fix".to_owned(),
                    branch: None,
                    source_branch: None,
                }],
                ..Default::default()
            },
        )]),
        collapsed: &HashSet::new(),
        counts: &HashMap::new(),
        active: &current,
    });
    let palette = destination_palette(&entries, 7, &current);
    assert!(palette.kind == PaletteKind::TabGroupDestination);
    let actions = palette
        .commands
        .iter()
        .map(|command| {
            command
                .action
                .as_any()
                .downcast_ref::<MoveTabToGroupDestination>()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(actions.len(), 4);
    for group in [
        TabGroup::Main,
        TabGroup::Custom("empty".to_owned()),
        TabGroup::Project(project.clone()),
        TabGroup::Worktree {
            project,
            root: worktree,
        },
    ] {
        assert!(
            actions
                .iter()
                .any(|action| action.group == group && action.tab_id == 7)
        );
    }
    assert!(actions.iter().all(|action| action.group != current));
}

#[test]
fn tab_group_destination_labels_distinguish_duplicate_names() {
    let groups = [
        TabGroup::Main,
        TabGroup::Custom("api".to_owned()),
        TabGroup::Project(PathBuf::from("/one/api")),
        TabGroup::Project(PathBuf::from("/two/api")),
        TabGroup::Worktree {
            project: PathBuf::from("/one/api"),
            root: PathBuf::from("/trees/api"),
        },
    ];
    let labels = groups.iter().map(destination_label).collect::<HashSet<_>>();
    assert_eq!(labels.len(), groups.len());
}

fn test_window(cx: &mut gpui::TestAppContext) -> (Entity<Zetta>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear(); // No shell or daemon is started by these tests.
        config.hide_project_sidebar = false;
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
        zetta.projects.registry = ProjectRegistry::empty();
        zetta.tabs = vec![test_tab(7, TabGroup::Main), test_tab(8, TabGroup::Main)];
        zetta.tab_groups.add_custom("scratch".to_owned());
        zetta
    });
    cx.run_until_parked();
    (zetta, cx)
}

#[gpui::test]
fn tab_group_move_palette_command_opens_a_searchable_picker_and_dispatches_the_move(
    cx: &mut gpui::TestAppContext,
) {
    let (zetta, cx) = test_window(cx);
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.toggle_command_palette(&ToggleCommandPalette, window, cx);
        let palette = zetta.command_palette.as_ref().unwrap();
        let command = palette
            .commands
            .iter()
            .position(|command| command.action.name() == MoveTabToGroup.name())
            .expect("the move action is available in the command palette");
        zetta.run_palette_command(command, window, cx);
    });
    cx.run_until_parked();
    zetta.update_in(cx, |zetta, window, cx| {
        let palette = zetta.command_palette.as_mut().unwrap();
        assert!(palette.kind == PaletteKind::TabGroupDestination);
        palette.query.text = "scratch".to_owned();
        palette.query_edited();
        assert_eq!(palette.matches().len(), 1);
        let command = palette.matches()[0];
        zetta.run_palette_command(command, window, cx);
    });
    cx.run_until_parked();
    zetta.read_with(cx, |zetta, _| {
        assert!(zetta.command_palette.is_none());
        assert_eq!(zetta.tabs[zetta.active_tab].id, 7);
        assert_eq!(
            zetta.active_tab_group(),
            TabGroup::Custom("scratch".to_owned())
        );
    });
}

#[gpui::test]
fn tab_group_move_escape_cancels_without_changing_membership(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = test_window(cx);
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.move_tab_to_group(&MoveTabToGroup, window, cx);
        zetta.command_palette_key_down(
            &KeyDownEvent {
                keystroke: gpui::Keystroke::parse("escape").unwrap(),
                is_held: false,
                prefer_character_input: false,
            },
            window,
            cx,
        );
        assert!(zetta.command_palette.is_none());
        assert_eq!(zetta.active_tab, 0);
        assert!(zetta.tabs.iter().all(|tab| tab.group == TabGroup::Main));

        zetta.launch_config.hide_project_sidebar = true;
        zetta.toggle_command_palette(&ToggleCommandPalette, window, cx);
        assert!(
            zetta
                .command_palette
                .as_ref()
                .unwrap()
                .commands
                .iter()
                .all(|command| { command.action.name() != MoveTabToGroup.name() })
        );
    });
}

#[gpui::test]
fn tab_group_move_follows_the_original_tab_and_preserves_its_contents(
    cx: &mut gpui::TestAppContext,
) {
    let (zetta, cx) = test_window(cx);
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.tab_groups.remember(TabGroup::Main, 7);
        zetta.move_tab_to_group(&MoveTabToGroup, window, cx);
        let action = zetta.command_palette.as_ref().unwrap().commands[0]
            .action
            .boxed_clone();
        let action = action
            .as_any()
            .downcast_ref::<MoveTabToGroupDestination>()
            .unwrap();
        assert_eq!(action.tab_id, 7);
        zetta.command_palette = None;
        // A close or reorder while the picker is open must not retarget it.
        zetta.tabs.swap(0, 1);
        zetta.active_tab = 0;
        zetta.move_tab_to_group_destination(action, window, cx);
        assert_eq!(zetta.active_tab, 1);
        assert_eq!(zetta.tabs_in_view(), [1]);
        assert_eq!(zetta.tabs[0].group, TabGroup::Main);
        let tab = &zetta.tabs[1];
        assert_eq!(tab.id, 7);
        assert_eq!(tab.group, TabGroup::Custom("scratch".to_owned()));
        assert!(tab.pinned);
        assert_eq!(tab.panes.len(), 1);
        assert_eq!(tab.panes[0].id, 7);
        assert_eq!(tab.active_pane, 7);
        assert_eq!(zetta.tab_groups.remembered(&TabGroup::Main), None);
        assert_eq!(zetta.tab_groups.remembered(&tab.group), Some(7));
    });
}

#[gpui::test]
fn tab_group_move_reaches_projects_and_collapsed_worktrees(cx: &mut gpui::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().canonicalize().unwrap();
    let worktree = project.join("fix");
    let (zetta, cx) = test_window(cx);
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.projects.registry.add(&project).unwrap();
        zetta.sidebar.worktrees.insert(
            project.clone(),
            ProjectWorktrees {
                worktrees: vec![SidebarWorktree {
                    path: worktree.clone(),
                    label: "fix".to_owned(),
                    branch: None,
                    source_branch: None,
                }],
                ..Default::default()
            },
        );
        zetta.sidebar.collapsed.insert(project.clone());
        zetta.move_tab_to_group(&MoveTabToGroup, window, cx);
        assert_eq!(zetta.command_palette.as_ref().unwrap().commands.len(), 3);
        // Project-context refreshes must preserve a destination picker.
        zetta.refresh_open_command_palette(window, cx);
        assert!(zetta.command_palette.as_ref().unwrap().kind == PaletteKind::TabGroupDestination);
        zetta.dismiss_command_palette(window, cx);
        for group in [
            TabGroup::Project(project.clone()),
            TabGroup::Worktree {
                project: project.clone(),
                root: worktree.clone(),
            },
            TabGroup::Custom("scratch".to_owned()),
            TabGroup::Main,
        ] {
            zetta.move_tab_to_group_destination(
                &MoveTabToGroupDestination {
                    tab_id: 7,
                    group: group.clone(),
                },
                window,
                cx,
            );
            assert_eq!(zetta.active_tab_group(), group);
            assert_eq!(zetta.tabs[0].panes[0].id, 7);
        }
        assert!(!zetta.sidebar.collapsed.contains(&project));
    });
}

#[gpui::test]
fn tab_group_move_ignores_stale_destinations_closed_tabs_and_disabled_grouping(
    cx: &mut gpui::TestAppContext,
) {
    let (zetta, cx) = test_window(cx);
    zetta.update_in(cx, |zetta, window, cx| {
        let action = MoveTabToGroupDestination {
            tab_id: 7,
            group: TabGroup::Custom("scratch".to_owned()),
        };
        zetta.tab_groups.remove_custom("scratch");
        zetta.move_tab_to_group_destination(&action, window, cx);
        assert_eq!(zetta.tabs[0].group, TabGroup::Main);
        zetta.tab_groups.add_custom("scratch".to_owned());
        zetta.tabs.remove(0);
        zetta.move_tab_to_group_destination(&action, window, cx);
        assert_eq!(zetta.tabs[0].id, 8);
        assert_eq!(zetta.tabs[0].group, TabGroup::Main);
        zetta.launch_config.hide_project_sidebar = true;
        zetta.move_tab_to_group_destination(
            &MoveTabToGroupDestination {
                tab_id: 8,
                ..action
            },
            window,
            cx,
        );
        zetta.move_tab_to_group(&MoveTabToGroup, window, cx);
        assert!(zetta.command_palette.is_none());
        assert_eq!(zetta.tabs[0].group, TabGroup::Main);
    });
}

fn test_tab(id: u64, group: TabGroup) -> Tab {
    let profile = Profile {
        console_backend: Default::default(),
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = TerminalPane::new(id, profile).with_label_number(1);
    Tab {
        id,
        attention_id: id,
        attention: None,
        panes: vec![pane],
        pane_indices: HashMap::from([(id, 0)]),
        next_pane_label: 2,
        theme_override: None,
        layout: PaneLayout::Pane(id),
        active_pane: id,
        focus_history: vec![id],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        protected: false,
        shared: false,
        custom_title: None,
        worktree_seed_title: None,
        process_title: None,
        icon: Some(IconName::Terminal),
        icon_override: TabIconOverride::None,
        pinned: true,
        renaming_pane: None,
        rename_buffer: None,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_style_picker: None,
        group,
    }
}
