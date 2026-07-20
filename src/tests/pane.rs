use super::*;

#[test]
fn pane_template_replaces_only_the_target_leaf() {
    let template = PaneSplitTemplate::Split {
        axis: PaneSplitAxis::Horizontal,
        first: Box::new(PaneSplitTemplate::Pane),
        second: Box::new(PaneSplitTemplate::Pane),
    };
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    let replacement = PaneLayout::from_template(&template, &mut [2, 3].into_iter());

    assert!(layout.replace(2, replacement));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(PaneLayout::Pane(2)),
                second: Box::new(PaneLayout::Pane(3)),
            }),
        }
    );
}

#[test]
fn pane_layout_replacement_moves_the_tree_without_cloning_it() {
    let replacement = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: Box::new(PaneLayout::Pane(10)),
        second: Box::new(PaneLayout::Pane(11)),
    };
    let original_first_child = match &replacement {
        PaneLayout::Split { first, .. } => first.as_ref() as *const PaneLayout,
        PaneLayout::Pane(_) => unreachable!(),
    };
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };

    assert!(layout.replace(1, replacement));
    let inserted_first_child = match &layout {
        PaneLayout::Split { first, .. } => match first.as_ref() {
            PaneLayout::Split { first, .. } => first.as_ref() as *const PaneLayout,
            PaneLayout::Pane(_) => unreachable!(),
        },
        PaneLayout::Pane(_) => unreachable!(),
    };
    assert_eq!(inserted_first_child, original_first_child);
}

#[test]
fn four_commands_tile_into_quarters() {
    assert_eq!(
        PaneLayout::tiled(&[1, 2, 3, 4]),
        Some(PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(PaneLayout::Pane(1)),
                second: Box::new(PaneLayout::Pane(2)),
            }),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(PaneLayout::Pane(3)),
                second: Box::new(PaneLayout::Pane(4)),
            }),
        })
    );
}

#[test]
fn three_commands_use_the_three_right_layout() {
    assert_eq!(
        PaneLayout::tiled(&[1, 2, 3]),
        Some(PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(PaneLayout::Pane(2)),
                second: Box::new(PaneLayout::Pane(3)),
            }),
        })
    );
}

#[test]
fn tiled_layout_rejects_an_empty_pane_list() {
    assert_eq!(PaneLayout::tiled(&[]), None);
}

#[test]
fn pane_limit_applies_to_total_tab_panes() {
    assert!(can_add_panes(1, MAX_PANES_PER_TAB - 1));
    assert!(!can_add_panes(2, MAX_PANES_PER_TAB - 1));
    assert!(!can_add_panes(usize::MAX, 1));
}

#[test]
fn terminal_spawn_notifications_are_coalesced() {
    let mut pending = false;
    assert!(begin_coalesced_notification(&mut pending));
    assert!(!begin_coalesced_notification(&mut pending));
    assert!(!begin_coalesced_notification(&mut pending));
    pending = false;
    assert!(begin_coalesced_notification(&mut pending));
}

#[test]
fn pane_output_save_guard_blocks_until_the_active_save_finishes() {
    let mut in_progress = false;

    assert!(begin_pane_output_save(&mut in_progress));
    assert!(in_progress);
    assert!(!begin_pane_output_save(&mut in_progress));

    finish_pane_output_save(&mut in_progress);
    assert!(!in_progress);
    assert!(begin_pane_output_save(&mut in_progress));
}

#[test]
fn bounded_launch_queue_applies_backpressure_and_preserves_order() {
    let mut queue = BoundedLaunchQueue::new(2);
    queue.extend([1, 2, 3, 4]);

    assert_eq!(queue.pop_ready(), Some(1));
    assert_eq!(queue.pop_ready(), Some(2));
    assert_eq!(queue.pop_ready(), None);

    queue.complete();
    assert_eq!(queue.pop_ready(), Some(3));
    assert_eq!(queue.pop_ready(), None);

    queue.complete();
    assert_eq!(queue.pop_ready(), Some(4));
}

#[test]
fn pane_launch_metadata_is_prepared_once_per_pane() {
    let mut preparations = 0;
    let launches = prepare_pane_launches([2, 3, 4], |pane_id| {
        preparations += 1;
        format!("tracking-{pane_id}")
    });

    assert_eq!(preparations, 3);
    assert_eq!(
        launches,
        [
            (2, "tracking-2".to_owned()),
            (3, "tracking-3".to_owned()),
            (4, "tracking-4".to_owned()),
        ]
    );
}

#[test]
fn terminal_regexes_are_cloned_then_moved_into_the_final_spawn() {
    let mut regexes = vec!["first".to_owned(), "second".to_owned()];
    let original_buffer = regexes[0].as_ptr();

    let earlier_spawn = clone_or_take_for_final_spawn(&mut regexes, false);
    assert_ne!(earlier_spawn[0].as_ptr(), original_buffer);
    assert_eq!(regexes[0].as_ptr(), original_buffer);

    let final_spawn = clone_or_take_for_final_spawn(&mut regexes, true);
    assert_eq!(final_spawn[0].as_ptr(), original_buffer);
    assert!(regexes.is_empty());
}

#[test]
fn configured_template_layout_is_built_through_a_borrow() {
    let templates = HashMap::from([(
        "two".to_owned(),
        PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Vertical,
            first: Box::new(PaneSplitTemplate::Pane),
            second: Box::new(PaneSplitTemplate::Pane),
        },
    )]);
    let layout = pane_layout_from_configured_template(&templates, "two", &mut [10, 11].into_iter());

    assert_eq!(
        layout,
        Some(PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: Box::new(PaneLayout::Pane(10)),
            second: Box::new(PaneLayout::Pane(11)),
        })
    );
    assert!(templates.contains_key("two"));
}

#[test]
fn tab_pane_index_resolves_panes_without_scanning() {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
    };
    let panes = [1, 2, 3]
        .into_iter()
        .map(|id| TerminalPane {
            id,
            label_number: id as usize,
            generated_label: None,
            custom_label: None,
            profile: profile.clone(),
            terminal: None,
            view: None,
            error: None,
            wsl_cwd_file: None,
            pending_command: None,
        })
        .collect::<Vec<_>>();
    let mut tab = Tab {
        id: 1,
        panes,
        pane_indices: HashMap::from([(1, 0), (2, 1), (3, 2)]),
        next_pane_label: 4,
        layout: PaneLayout::Pane(1),
        active_pane: 1,
        focus_history: vec![1],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        custom_title: None,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
    };
    for pane in &tab.panes {
        assert!(std::ptr::eq(tab.pane(pane.id).unwrap(), pane));
    }
    assert!(tab.pane(99).is_none());

    tab.remove_pane(1);
    assert_eq!(tab.pane(2).map(|pane| pane.id), Some(2));
    assert_eq!(tab.pane(3).map(|pane| pane.id), Some(3));
    tab.push_pane(TerminalPane {
        id: 4,
        label_number: 0,
        generated_label: None,
        custom_label: None,
        profile,
        terminal: None,
        view: None,
        error: None,
        wsl_cwd_file: None,
        pending_command: None,
    });
    assert_eq!(tab.pane(4).map(|pane| pane.id), Some(4));
}

#[test]
fn nested_pane_layouts_split_and_collapse() {
    let mut layout = PaneLayout::Pane(1);
    assert!(layout.split(1, SplitAxis::Horizontal, 2));
    assert!(layout.split(2, SplitAxis::Vertical, 3));
    assert!(!layout.split(99, SplitAxis::Vertical, 4));

    let layout = layout.without(2).unwrap();
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(3)),
        }
    );
}

#[test]
fn layout_removes_multiple_panes_in_one_traversal() {
    let layout = PaneLayout::tiled(&[1, 2, 3, 4]).unwrap();
    let minimized = HashSet::from([2, 3]);

    assert_eq!(
        layout.without_all(&minimized),
        Some(PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(4)),
        })
    );
    assert_eq!(layout.without_all(&HashSet::from([1, 2, 3, 4])), None);
}

#[test]
fn split_profile_comes_from_the_active_pane() {
    let system = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
    };
    let zsh = Profile {
        name: "Zsh".to_owned(),
        command: task::Shell::Program("zsh".to_owned()),
        theme: Some("One Light".to_owned()),
    };
    let tab = Tab {
        id: 1,
        panes: vec![
            TerminalPane {
                id: 1,
                label_number: 1,
                generated_label: None,
                custom_label: None,
                profile: system,
                terminal: None,
                view: None,
                error: None,
                wsl_cwd_file: None,
                pending_command: None,
            },
            TerminalPane {
                id: 2,
                label_number: 2,
                generated_label: None,
                custom_label: None,
                profile: zsh,
                terminal: None,
                view: None,
                error: None,
                wsl_cwd_file: None,
                pending_command: None,
            },
        ],
        pane_indices: HashMap::from([(1, 0), (2, 1)]),
        next_pane_label: 3,
        layout: PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(2)),
        },
        active_pane: 2,
        focus_history: vec![1, 2],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        custom_title: None,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
    };

    let profile = tab.active_profile().unwrap();
    assert_eq!(profile.name, "Zsh");
    assert_eq!(profile.theme.as_deref(), Some("One Light"));
}

#[test]
fn closing_active_pane_restores_previous_focus() {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
    };
    let pane = |id| TerminalPane {
        id,
        label_number: id as usize,
        generated_label: None,
        custom_label: None,
        profile: profile.clone(),
        terminal: None,
        view: None,
        error: None,
        wsl_cwd_file: None,
        pending_command: None,
    };
    let mut tab = Tab {
        id: 1,
        panes: vec![pane(1), pane(2), pane(3)],
        pane_indices: HashMap::from([(1, 0), (2, 1), (3, 2)]),
        next_pane_label: 4,
        layout: PaneLayout::Pane(1),
        active_pane: 3,
        focus_history: vec![1, 2, 3],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        custom_title: None,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
    };

    tab.remove_pane(3);
    tab.restore_focus_after_close(3, 1);

    assert_eq!(tab.active_pane, 2);
    assert_eq!(tab.focus_history, vec![1, 2]);
}

#[test]
fn closing_inactive_pane_preserves_focus() {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
    };
    let pane = |id| TerminalPane {
        id,
        label_number: id as usize,
        generated_label: None,
        custom_label: None,
        profile: profile.clone(),
        terminal: None,
        view: None,
        error: None,
        wsl_cwd_file: None,
        pending_command: None,
    };
    let mut tab = Tab {
        id: 1,
        panes: vec![pane(1), pane(2), pane(3)],
        pane_indices: HashMap::from([(1, 0), (2, 1), (3, 2)]),
        next_pane_label: 4,
        layout: PaneLayout::Pane(1),
        active_pane: 3,
        focus_history: vec![1, 2, 3],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        custom_title: None,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
    };

    tab.remove_pane(1);
    tab.restore_focus_after_close(1, 2);

    assert_eq!(tab.active_pane, 3);
    assert_eq!(tab.focus_history, vec![2, 3]);
}

#[test]
fn directional_focus_moves_between_quarter_panes() {
    let mut layout = PaneLayout::Pane(1);
    assert!(layout.split(1, SplitAxis::Horizontal, 2));
    assert!(layout.split(1, SplitAxis::Vertical, 3));
    assert!(layout.split(2, SplitAxis::Vertical, 4));

    assert_eq!(layout.adjacent_pane(1, PaneDirection::Right), Some(3));
    assert_eq!(layout.adjacent_pane(1, PaneDirection::Down), Some(2));
    assert_eq!(layout.adjacent_pane(3, PaneDirection::Down), Some(4));
    assert_eq!(layout.adjacent_pane(4, PaneDirection::Left), Some(2));
    assert_eq!(layout.adjacent_pane(4, PaneDirection::Up), Some(3));
    assert_eq!(layout.regions().len(), 4);
}

fn pane_management_tab() -> Tab {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
    };
    let pane = |id| TerminalPane {
        id,
        label_number: id as usize,
        generated_label: None,
        custom_label: None,
        profile: profile.clone(),
        terminal: None,
        view: None,
        error: None,
        wsl_cwd_file: None,
        pending_command: None,
    };
    let layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(3)),
        }),
    };
    Tab {
        id: 1,
        panes: vec![pane(1), pane(2), pane(3)],
        pane_indices: HashMap::from([(1, 0), (2, 1), (3, 2)]),
        next_pane_label: 4,
        layout,
        active_pane: 2,
        focus_history: vec![1, 3, 2],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        custom_title: None,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
    }
}

#[test]
fn transferred_tabs_receive_target_window_ids_consistently() {
    let mut tab = pane_management_tab();
    tab.maximized_pane = Some(2);
    tab.minimized_panes = vec![1, 3];
    tab.selected_minimized_pane = Some(3);
    let mut next_pane_id = 20;

    tab.reassign_ids(10, &mut next_pane_id);

    assert_eq!(tab.id, 10);
    assert_eq!(next_pane_id, 23);
    assert_eq!(
        tab.panes.iter().map(|pane| pane.id).collect::<Vec<_>>(),
        [20, 21, 22]
    );
    assert_eq!(tab.active_pane, 21);
    assert_eq!(tab.focus_history, [20, 22, 21]);
    assert_eq!(tab.maximized_pane, Some(21));
    assert_eq!(tab.minimized_panes, [20, 22]);
    assert_eq!(tab.selected_minimized_pane, Some(22));
    assert_eq!(
        tab.layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: Box::new(PaneLayout::Pane(20)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(PaneLayout::Pane(21)),
                second: Box::new(PaneLayout::Pane(22)),
            }),
        }
    );
}

#[test]
fn maximizing_and_restoring_preserves_the_original_layout() {
    let mut tab = pane_management_tab();
    let original = tab.layout.clone();

    assert!(tab.toggle_maximize(2));
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(2)));
    assert_eq!(tab.layout, original);

    assert!(tab.toggle_maximize(2));
    assert_eq!(tab.visible_layout(), Some(original.clone()));
    assert_eq!(tab.layout, original);
}

#[test]
fn pane_labels_remain_stable_and_are_not_reused() {
    let mut tab = pane_management_tab();

    assert_eq!(tab.pane(1).unwrap().label(), "Pane 1");
    assert_eq!(tab.pane(2).unwrap().label(), "Pane 2");
    assert_eq!(tab.pane(3).unwrap().label(), "Pane 3");

    let profile = tab.pane(1).unwrap().profile.clone();
    tab.remove_pane(2);
    tab.push_pane(TerminalPane {
        id: 4,
        label_number: 0,
        generated_label: None,
        custom_label: None,
        profile,
        terminal: None,
        view: None,
        error: None,
        wsl_cwd_file: None,
        pending_command: None,
    });

    assert_eq!(tab.pane(1).unwrap().label(), "Pane 1");
    assert_eq!(tab.pane(3).unwrap().label(), "Pane 3");
    assert_eq!(tab.pane(4).unwrap().label(), "Pane 4");
}

#[test]
fn custom_pane_labels_replace_the_fallback_and_render_while_editing() {
    let mut tab = pane_management_tab();

    tab.pane_mut(2).unwrap().generated_label = Some("dev · eu-west".to_owned());
    assert_eq!(tab.pane(2).unwrap().label(), "dev · eu-west");

    tab.pane_mut(2).unwrap().custom_label = Some("API server".to_owned());
    assert_eq!(tab.pane(2).unwrap().label(), "API server");

    tab.renaming_pane = Some(2);
    tab.rename_buffer = Some("Database".to_owned());
    tab.rename_cursor = 4;
    assert_eq!(tab.displayed_pane_label(2).as_deref(), Some("Data|base"));

    tab.pane_mut(2).unwrap().custom_label = None;
    tab.renaming_pane = None;
    tab.rename_buffer = None;
    assert_eq!(tab.pane(2).unwrap().label(), "dev · eu-west");
}

#[test]
fn minimizing_and_restoring_preserves_the_nested_split_position() {
    let mut tab = pane_management_tab();
    let original = tab.layout.clone();

    assert!(tab.minimize(2));
    assert_eq!(tab.minimized_panes, vec![2]);
    assert_eq!(tab.selected_minimized_pane, Some(2));
    assert_eq!(tab.active_pane, 3);
    assert_eq!(
        tab.visible_layout(),
        Some(PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(3)),
        })
    );
    assert_eq!(tab.layout, original);

    assert!(tab.restore_minimized(2));
    assert_eq!(tab.selected_minimized_pane, None);
    assert_eq!(tab.active_pane, 2);
    assert_eq!(tab.visible_layout(), Some(original.clone()));
    assert_eq!(tab.layout, original);
}

#[test]
fn minimized_pane_selection_wraps_and_restore_uses_the_selection() {
    let mut tab = pane_management_tab();

    assert!(tab.minimize(2));
    assert!(tab.minimize(3));
    assert_eq!(tab.selected_minimized_pane, Some(3));

    assert!(tab.select_previous_minimized());
    assert_eq!(tab.selected_minimized_pane, Some(2));
    assert!(tab.select_previous_minimized());
    assert_eq!(tab.selected_minimized_pane, Some(3));
    assert!(tab.select_next_minimized());
    assert_eq!(tab.selected_minimized_pane, Some(2));

    assert!(tab.restore_last_minimized());
    assert_eq!(tab.active_pane, 2);
    assert_eq!(tab.minimized_panes, vec![3]);
    assert_eq!(tab.selected_minimized_pane, Some(3));
}

#[test]
fn closing_the_selected_minimized_pane_selects_a_surviving_item() {
    let mut tab = pane_management_tab();
    assert!(tab.minimize(2));
    assert!(tab.minimize(3));

    tab.remove_pane(3);
    let layout = tab.layout.clone().without(3).unwrap();
    tab.restore_focus_after_close(3, layout.first_pane());
    tab.layout = layout;

    assert_eq!(tab.minimized_panes, vec![2]);
    assert_eq!(tab.selected_minimized_pane, Some(2));
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(1)));
}

#[test]
fn closing_the_only_visible_pane_restores_the_most_recently_minimized_pane() {
    let mut tab = pane_management_tab();
    assert!(tab.minimize(2));
    assert!(tab.minimize(3));
    assert!(tab.select_previous_minimized());
    assert_eq!(tab.selected_minimized_pane, Some(2));

    tab.remove_pane(1);
    tab.layout = tab.layout.clone().without(1).unwrap();
    tab.restore_focus_after_close(1, tab.layout.first_pane());

    assert_eq!(tab.minimized_panes, vec![2]);
    assert_eq!(tab.selected_minimized_pane, Some(2));
    assert_eq!(tab.active_pane, 3);
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(3)));
}

#[test]
fn closing_the_only_visible_pane_restores_a_sole_minimized_pane() {
    let mut tab = pane_management_tab();
    tab.remove_pane(3);
    tab.layout = tab.layout.clone().without(3).unwrap();
    tab.restore_focus_after_close(3, tab.layout.first_pane());
    assert!(tab.minimize(2));

    tab.remove_pane(1);
    tab.layout = tab.layout.clone().without(1).unwrap();
    tab.restore_focus_after_close(1, tab.layout.first_pane());

    assert!(tab.minimized_panes.is_empty());
    assert_eq!(tab.selected_minimized_pane, None);
    assert_eq!(tab.active_pane, 2);
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(2)));
    assert!(!tab.restore_last_minimized());
}

#[test]
fn at_least_one_pane_must_remain_visible() {
    let mut tab = pane_management_tab();

    assert!(tab.minimize(2));
    assert!(tab.minimize(3));
    assert!(!tab.minimize(1));
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(1)));
    assert!(tab.restore_last_minimized());
    assert_eq!(tab.active_pane, 3);
}

#[test]
fn terminal_environment_identifies_zetta() {
    let mut env = HashMap::from([("ZED_TERM".to_string(), "true".to_string())]);

    terminal::insert_zetta_terminal_env(&mut env, &"0.1.0");

    assert_eq!(env.get("ZETTA_TERM").map(String::as_str), Some("true"));
    assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("zetta"));
    assert_eq!(
        env.get("TERM_PROGRAM_VERSION").map(String::as_str),
        Some("0.1.0")
    );
    assert!(!env.contains_key("ZED_TERM"));
}
