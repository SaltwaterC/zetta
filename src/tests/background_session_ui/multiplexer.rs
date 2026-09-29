use super::*;

use crate::session_state::{AxisState, LayoutState, PaneState, TabState};

fn pane_summary(id: u64) -> BackgroundPaneSummary {
    BackgroundPaneSummary {
        id,
        label: format!("Pane {id}"),
        profile: "System".to_owned(),
        configured_command: "sh".to_owned(),
        application: "sh".to_owned(),
        foreground_command: None,
        terminal_title: None,
        working_directory: None,
        state: BackgroundPaneState::Running,
        exit: None,
    }
}

fn summary(
    panes: Vec<u64>,
    layout: BackgroundPaneLayout,
    active_pane: u64,
) -> BackgroundSessionSummary {
    BackgroundSessionSummary {
        id: 9,
        title: "shared".to_owned(),
        authentication_required: false,
        active_pane,
        layout,
        panes: panes.into_iter().map(pane_summary).collect(),
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

fn pane_state(id: u64, mux_pane_id: Option<u64>) -> PaneState {
    PaneState {
        id,
        mux_pane_id,
        label_number: id as usize,
        generated_label: Some(format!("generated-{id}")),
        custom_label: None,
        profile: "System".to_owned(),
        theme_override: None,
        environment_overrides: HashMap::new(),
        overlay: None,
        exit: None,
        base_exited: false,
        pending_command: None,
        active_command: None,
        detected_worktree_title: None,
        stack: Vec::new(),
        selected_stacked: None,
    }
}

fn tab_state(panes: Vec<PaneState>) -> TabState {
    let first = panes.first().map_or(0, |pane| pane.id);
    TabState {
        pane_theme_source: None,
        attention_id: 17,
        next_pane_label: 20,
        layout: LayoutState::Pane { pane_id: first },
        active_pane: first,
        focus_history: vec![first],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: true,
        silent_mode: true,
        keep_running: true,
        shared: true,
        custom_title: Some("durable title".to_owned()),
        worktree_seed_title: Some("durable worktree".to_owned()),
        process_title: Some("durable process".to_owned()),
        icon: Some("terminal".to_owned()),
        icon_override: None,
        pinned: true,
        panes,
        theme_override: Some("Ayu Dark".to_owned()),
    }
}

fn canonical(
    panes: Vec<u64>,
    layout: BackgroundPaneLayout,
    active_pane: u64,
    state: TabState,
) -> zmux::messages::SharedSessionState {
    let mut canonical = zmux::messages::SharedSessionState::new(
        9,
        summary(panes, layout.clone(), active_pane),
        serde_json::to_value(state).unwrap(),
    );
    canonical.presentation.layout = layout;
    canonical.presentation.active_pane = active_pane;
    canonical
}

#[test]
fn stale_four_pane_blob_is_reduced_to_the_one_live_pane() {
    let stale = tab_state(vec![
        pane_state(101, Some(11)),
        pane_state(102, Some(12)),
        pane_state(103, Some(13)),
        pane_state(104, Some(14)),
    ]);
    let live = canonical(
        vec![14],
        BackgroundPaneLayout::Pane { pane_id: 14 },
        14,
        stale,
    );

    let (repaired, changed) =
        reconcile_shared_attach_state(serde_json::from_value(live.state.clone()).unwrap(), &live)
            .unwrap();

    assert!(changed);
    assert_eq!(repaired.panes.len(), 1);
    assert_eq!(repaired.panes[0].mux_pane_id, Some(14));
    assert_eq!(repaired.panes[0].id, 14);
    assert_eq!(repaired.layout, LayoutState::Pane { pane_id: 14 });
    assert_eq!(repaired.active_pane, 14);
}

#[test]
fn detached_attach_drops_an_ended_pane_and_keeps_the_survivor() {
    let mut stale = tab_state(vec![pane_state(101, Some(11)), pane_state(102, Some(12))]);
    stale.shared = false;
    stale.layout = LayoutState::Split {
        axis: AxisState::Vertical,
        first_ratio: 500,
        first: Box::new(LayoutState::Pane { pane_id: 101 }),
        second: Box::new(LayoutState::Pane { pane_id: 102 }),
    };
    stale.active_pane = 101;
    let live = canonical(
        vec![12],
        BackgroundPaneLayout::Pane { pane_id: 12 },
        12,
        stale,
    );

    let (repaired, changed) = reconcile_attached_state(
        serde_json::from_value(live.state.clone()).unwrap(),
        &live,
        false,
    )
    .unwrap();

    assert!(changed);
    assert!(!repaired.shared);
    assert_eq!(repaired.panes.len(), 1);
    assert_eq!(repaired.panes[0].mux_pane_id, Some(12));
    assert_eq!(repaired.layout, LayoutState::Pane { pane_id: 12 });
    assert_eq!(repaired.active_pane, 12);
}

#[test]
fn surviving_metadata_and_durable_tab_fields_are_preserved_by_mux_id() {
    let mut stale_survivor = pane_state(204, Some(24));
    stale_survivor.custom_label = Some("kept label".to_owned());
    stale_survivor.theme_override = Some("Dracula".to_owned());
    stale_survivor
        .environment_overrides
        .insert("KEEP".to_owned(), "yes".to_owned());
    let stale = tab_state(vec![pane_state(201, Some(21)), stale_survivor]);
    let canonical = canonical(
        vec![24],
        BackgroundPaneLayout::Pane { pane_id: 24 },
        24,
        stale,
    );

    let (repaired, _) = reconcile_shared_attach_state(
        serde_json::from_value(canonical.state.clone()).unwrap(),
        &canonical,
    )
    .unwrap();

    let survivor = &repaired.panes[0];
    assert_eq!(survivor.custom_label.as_deref(), Some("kept label"));
    assert_eq!(survivor.theme_override.as_deref(), Some("Dracula"));
    assert_eq!(
        survivor
            .environment_overrides
            .get("KEEP")
            .map(String::as_str),
        Some("yes")
    );
    assert_eq!(repaired.custom_title.as_deref(), Some("durable title"));
    assert_eq!(repaired.theme_override.as_deref(), Some("Ayu Dark"));
    assert!(repaired.keep_running);
    assert!(repaired.pinned);
}

#[test]
fn canonical_presentation_repairs_layout_focus_and_visibility() {
    let mut stale = tab_state(vec![pane_state(301, Some(31)), pane_state(302, Some(32))]);
    stale.active_pane = 301;
    stale.focus_history = vec![301, 302];
    stale.maximized_pane = Some(301);
    stale.minimized_panes = vec![302];
    stale.selected_minimized_pane = Some(302);
    let layout = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 300,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 32 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 31 }),
    };
    let canonical = canonical(vec![31, 32], layout, 32, stale);

    let (repaired, _) = reconcile_shared_attach_state(
        serde_json::from_value(canonical.state.clone()).unwrap(),
        &canonical,
    )
    .unwrap();

    assert_eq!(
        repaired.layout,
        LayoutState::Split {
            axis: AxisState::Vertical,
            first_ratio: 300,
            first: Box::new(LayoutState::Pane { pane_id: 32 }),
            second: Box::new(LayoutState::Pane { pane_id: 31 }),
        }
    );
    assert_eq!(repaired.active_pane, 32);
    assert_eq!(repaired.focus_history, vec![31, 32]);
    assert_eq!(repaired.maximized_pane, None);
    assert_eq!(repaired.minimized_panes, Vec::<u64>::new());
    assert_eq!(repaired.selected_minimized_pane, None);
}

#[test]
fn a_live_pane_missing_from_the_blob_is_synthesized_from_the_summary() {
    let stale = tab_state(vec![pane_state(401, Some(41))]);
    let layout = BackgroundPaneLayout::Split {
        axis: "horizontal".to_owned(),
        first_ratio: 700,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 41 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 42 }),
    };
    let canonical = canonical(vec![41, 42], layout, 42, stale);

    let (repaired, changed) = reconcile_shared_attach_state(
        serde_json::from_value(canonical.state.clone()).unwrap(),
        &canonical,
    )
    .unwrap();

    assert!(changed);
    let synthesized = repaired.panes.iter().find(|pane| pane.id == 42).unwrap();
    assert_eq!(synthesized.mux_pane_id, Some(42));
    assert_eq!(synthesized.profile, "System");
    assert_eq!(synthesized.generated_label.as_deref(), Some("Pane 42"));
    assert_eq!(repaired.active_pane, 42);
}

fn publication_profile() -> Profile {
    Profile {
        name: "System".into(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::default(),
    }
}

#[test]
fn disk_restore_metadata_joins_by_mux_id_but_keeps_routing_ids() {
    let state = tab_state(vec![
        pane_state(2, Some(1)),
        pane_state(1, Some(2)),
        pane_state(3, None),
    ]);
    let mut summary = summary(vec![1, 2, 3], BackgroundPaneLayout::Pane { pane_id: 1 }, 1);
    for pane in &mut summary.panes {
        pane.working_directory = Some(PathBuf::from(format!("directory-{}", pane.id)));
    }
    let metadata = restored_pane_metadata(&state, &summary);
    assert_eq!(
        metadata[0],
        (2, "System".into(), Some(PathBuf::from("directory-1")))
    );
    assert_eq!(
        metadata[1],
        (1, "System".into(), Some(PathBuf::from("directory-2")))
    );
    assert_eq!(
        metadata[2],
        (3, "System".into(), Some(PathBuf::from("directory-3")))
    );
}

#[gpui::test]
fn detach_publication_preserves_layout_cwd_and_project_theme(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(
            theme::LoadThemes::All(Box::new(crate::zetta_assets::ZettaAssets)),
            cx,
        );
        let registry = ThemeRegistry::global(cx);
        theme_settings::load_bundled_themes(&registry);
        theme::GlobalTheme::update_theme(cx, registry.get("One Light").unwrap());
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        Zetta::new(
            config,
            None,
            crate::ZettaLaunchOptions {
                no_mux: true,
                ..Default::default()
            },
            window,
            cx,
        )
    });
    let temporary = tempfile::tempdir().unwrap();
    let mut roots = Vec::new();
    for (name, theme) in [
        ("first", "One Dark"),
        ("second", "Solarized Light"),
        ("destination", "Gruvbox Dark"),
    ] {
        let root = temporary.path().join(name);
        std::fs::create_dir_all(root.join(".zetta")).unwrap();
        std::fs::write(
            ProjectConfig::path_for(&root),
            format!(r#"{{"theme":"{theme}"}}"#),
        )
        .unwrap();
        roots.push(std::fs::canonicalize(root).unwrap());
    }
    let directories = [roots[0].clone(), roots[1].clone(), roots[0].join("src")];
    std::fs::create_dir_all(&directories[2]).unwrap();
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.projects.registry =
            ProjectRegistry::load_from(temporary.path().join("registry.json")).unwrap();
        for root in &roots {
            zetta.projects.registry.add(root).unwrap();
        }
        let mut destination = tab_state(vec![pane_state(99, None)])
            .into_tab(99, |_| publication_profile())
            .unwrap();
        destination.theme_override = None;
        zetta
            .projects
            .insert_config(ProjectConfig::load(&roots[2], &zetta.launch_config).unwrap());
        zetta.projects.pane_roots.insert(99, roots[2].clone());
        zetta.tabs.push(destination);
        zetta.active_tab = 0;
        zetta.next_pane_id = 100;

        let mut state = tab_state(vec![
            pane_state(2, Some(1)),
            pane_state(1, Some(3)),
            pane_state(3, Some(2)),
        ]);
        state.shared = false;
        state.theme_override = None;
        state.active_pane = 1;
        state.layout = LayoutState::Split {
            axis: AxisState::Vertical,
            first_ratio: 270,
            first: Box::new(LayoutState::Pane { pane_id: 3 }),
            second: Box::new(LayoutState::Split {
                axis: AxisState::Horizontal,
                first_ratio: 630,
                first: Box::new(LayoutState::Pane { pane_id: 2 }),
                second: Box::new(LayoutState::Pane { pane_id: 1 }),
            }),
        };
        let mut tab = state.into_tab(9, |_| publication_profile()).unwrap();
        for (pane, mux_id) in tab.panes.iter().zip([1, 3, 2]) {
            zetta.mux_panes.record(pane.id, mux_id);
        }
        for (index, pane) in tab.panes.iter_mut().enumerate() {
            let builder = terminal::TerminalBuilder::new_display_only(
                terminal::terminal_settings::CursorShape::Block,
                terminal::terminal_settings::AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                util::paths::PathStyle::local(),
            )
            .with_working_directory(Some(directories[index].clone()));
            pane.terminal = Some(cx.new(|cx| builder.subscribe(cx)));
        }
        // Reconnect must read the latest configuration, even if this window
        // already cached the project before it was edited.
        zetta
            .projects
            .insert_config(ProjectConfig::load(&roots[0], &zetta.launch_config).unwrap());
        std::fs::write(
            ProjectConfig::path_for(&roots[0]),
            r#"{"theme":"Solarized Dark"}"#,
        )
        .unwrap();
        for _ in 0..2 {
            let (summary, opaque) = zetta.session_publication(&tab, 9, false, cx).unwrap();
            assert_eq!(summary.active_pane, 3);
            assert_eq!(
                summary.panes.iter().map(|pane| pane.id).collect::<Vec<_>>(),
                vec![1, 3, 2]
            );
            let expected = BackgroundPaneLayout::Split {
                axis: "vertical".into(),
                first_ratio: 270,
                first: Box::new(BackgroundPaneLayout::Pane { pane_id: 2 }),
                second: Box::new(BackgroundPaneLayout::Split {
                    axis: "horizontal".into(),
                    first_ratio: 630,
                    first: Box::new(BackgroundPaneLayout::Pane { pane_id: 1 }),
                    second: Box::new(BackgroundPaneLayout::Pane { pane_id: 3 }),
                }),
            };
            assert_eq!(summary.layout, expected);
            for (index, pane) in summary.panes.iter().enumerate() {
                assert_eq!(pane.working_directory.as_ref(), Some(&directories[index]));
            }
            let saved: TabState = serde_json::from_value(opaque.clone()).unwrap();
            assert_eq!(saved.panes[0].id, tab.panes[0].id);
            let canonical = zmux::messages::SharedSessionState::new(9, summary.clone(), opaque);
            let (state, _) = reconcile_attached_state(saved, &canonical, false).unwrap();
            let metadata = zetta.prepare_restored_panes(
                restored_pane_metadata(&state, &summary),
                ProjectContextPolicy::Local,
            );
            tab = state.into_tab(9, |_| publication_profile()).unwrap();
            let mappings = tab.reassign_ids(9, &mut zetta.next_pane_id);
            zetta.bind_restored_projects(&tab, &metadata);
            for mux_id in [1, 3, 2] {
                zetta.mux_panes.record(mappings[&mux_id], mux_id);
            }
            restore_test_views(zetta, &mut tab, &metadata, &roots, window, cx);
            let remote = zetta.prepare_restored_panes(
                restored_pane_metadata(&TabState::from_tab(&tab, zetta.mux_panes.ids()), &summary),
                ProjectContextPolicy::Remote,
            );
            assert!(
                remote
                    .panes
                    .values()
                    .all(|pane| pane.project_root.is_none())
            );
        }
        zetta.mux_panes.forget_pane(tab.panes[0].id);
        assert!(zetta.session_publication(&tab, 9, false, cx).is_err());
    });
}

fn restore_test_views(
    zetta: &mut Zetta,
    tab: &mut Tab,
    metadata: &RestoredPaneMetadata,
    roots: &[PathBuf],
    window: &mut Window,
    cx: &mut Context<Zetta>,
) {
    for (index, pane) in tab.panes.iter_mut().enumerate() {
        let project = zetta.projects.config_for_pane(pane.id).cloned().unwrap();
        assert_eq!(project.root, roots[index % 2]);
        let theme = zetta.restored_terminal_theme(
            None,
            None,
            &pane.profile,
            Some(&project),
            ProjectContextPolicy::Local,
            cx,
        );
        let builder = terminal::TerminalBuilder::new_display_only(
            terminal::terminal_settings::CursorShape::Block,
            terminal::terminal_settings::AlternateScroll::On,
            None,
            0,
            cx.background_executor(),
            util::paths::PathStyle::local(),
        )
        .with_working_directory(metadata.working_directory(pane.routing_id));
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view = cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), theme, window, cx));
        assert_eq!(
            view.read(cx).theme().unwrap().name.as_ref(),
            ["Solarized Dark", "Solarized Light"][index % 2]
        );
        assert!(terminal.read(cx).reported_working_directory().is_none());
        pane.terminal = Some(terminal);
        pane.view = Some(view);
        for (pane_override, tab_override, expected) in [
            (Some("One Light"), Some("One Dark"), "One Light"),
            (None, Some("One Light"), "One Light"),
        ] {
            assert_eq!(
                zetta
                    .restored_terminal_theme(
                        pane_override,
                        tab_override,
                        &pane.profile,
                        Some(&project),
                        ProjectContextPolicy::Local,
                        cx
                    )
                    .unwrap()
                    .name
                    .as_ref(),
                expected
            );
        }
    }
}

#[test]
fn mosh_carried_and_ssh_attached_panes_keep_their_layout_order() {
    // (local pane, multiplexer pane); 20 and 40 came up on Mosh.
    let remaining = [(2, 20), (3, 30), (4, 40), (5, 50)];
    let attached = HashMap::from([(3, "ssh 30".to_owned()), (5, "ssh 50".to_owned())]);

    let merged = merge_remaining_panes(
        &remaining,
        |mux_pane_id| matches!(mux_pane_id, 20 | 40),
        attached,
        |mux_pane_id| format!("mosh {mux_pane_id}"),
    );

    assert_eq!(
        merged,
        [
            (2, "mosh 20".to_owned()),
            (3, "ssh 30".to_owned()),
            (4, "mosh 40".to_owned()),
            (5, "ssh 50".to_owned()),
        ]
    );
}

#[test]
fn a_pane_refused_over_ssh_ends_the_tab_where_it_would_have_been() {
    let remaining = [(2, 20), (3, 30), (4, 40)];
    // 30 fell back to SSH and was refused: the session was taken meanwhile.
    let merged = merge_remaining_panes(
        &remaining,
        |mux_pane_id| mux_pane_id != 30,
        HashMap::<u64, String>::new(),
        |mux_pane_id| format!("mosh {mux_pane_id}"),
    );

    assert_eq!(merged, [(2, "mosh 20".to_owned())]);
}
