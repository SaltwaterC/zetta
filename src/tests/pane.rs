use super::*;

#[test]
fn terminal_size_label_uses_columns_before_rows() {
    assert_eq!(terminal_size_label(120, 40), "120 × 40");
}

#[test]
fn two_pane_layout_rotates_clockwise_and_counter_clockwise() {
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };

    assert!(layout.rotate_pane(1, PaneRotationDirection::Clockwise));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(1)),
        }
    );
    assert!(layout.rotate_pane(1, PaneRotationDirection::CounterClockwise));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(2)),
        }
    );

    let mut clockwise = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    assert!(clockwise.rotate_pane(1, PaneRotationDirection::Clockwise));
    assert!(clockwise.rotate_pane(1, PaneRotationDirection::Clockwise));
    assert_eq!(clockwise.first_pane(), 2);

    let mut counter_clockwise = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    assert!(counter_clockwise.rotate_pane(1, PaneRotationDirection::CounterClockwise));
    assert!(counter_clockwise.rotate_pane(1, PaneRotationDirection::CounterClockwise));
    assert_eq!(counter_clockwise.first_pane(), 2);
}

#[test]
fn pane_rotation_rejects_missing_panes() {
    let mut single = PaneLayout::Pane(1);
    let layout = PaneLayout::tiled(&[1, 2, 3]).unwrap();

    assert!(!single.rotate_pane(1, PaneRotationDirection::Clockwise));
    assert!(
        !layout
            .clone()
            .rotate_pane(99, PaneRotationDirection::Clockwise)
    );
    assert_eq!(single, PaneLayout::Pane(1));
}

#[test]
fn pane_rotation_stays_with_the_equal_local_pair() {
    let mut layout = PaneLayout::tiled(&[1, 2, 3]).unwrap();
    let outer = layout.clone();

    assert!(layout.rotate_pane(2, PaneRotationDirection::Clockwise));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Vertical,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(3)),
                second: Box::new(PaneLayout::Pane(2)),
            }),
        }
    );

    assert!(layout.rotate_pane(2, PaneRotationDirection::CounterClockwise));
    assert_eq!(layout, outer);
}

#[test]
fn pane_rotation_preserves_resized_two_pane_support() {
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: 700,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };

    assert!(layout.rotate_pane(2, PaneRotationDirection::Clockwise));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: 700,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(2)),
        }
    );
}

#[test]
fn pane_rotation_rotates_a_focused_dominant_three_pane_group() {
    let mut layout = PaneLayout::tiled(&[1, 2, 3]).unwrap();

    assert!(layout.rotate_pane(1, PaneRotationDirection::Clockwise));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Vertical,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(3)),
                second: Box::new(PaneLayout::Pane(2)),
            }),
        }
    );
}

#[test]
fn pane_rotation_rotates_equal_quarters_as_one_group() {
    let mut layout = PaneLayout::tiled(&[1, 2, 3, 4]).unwrap();

    assert!(layout.rotate_pane(1, PaneRotationDirection::Clockwise));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Split {
                axis: SplitAxis::Vertical,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(2)),
                second: Box::new(PaneLayout::Pane(1)),
            }),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Vertical,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(4)),
                second: Box::new(PaneLayout::Pane(3)),
            }),
        }
    );

    assert!(layout.rotate_pane(1, PaneRotationDirection::CounterClockwise));
    assert_eq!(layout, PaneLayout::tiled(&[1, 2, 3, 4]).unwrap());
}

#[test]
fn pane_rotation_recurses_only_into_the_active_group() {
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::tiled(&[1, 2, 3]).unwrap()),
        second: Box::new(PaneLayout::Pane(4)),
    };

    assert!(layout.rotate_pane(2, PaneRotationDirection::Clockwise));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Split {
                axis: SplitAxis::Vertical,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(1)),
                second: Box::new(PaneLayout::Split {
                    axis: SplitAxis::Vertical,
                    first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                    first: Box::new(PaneLayout::Pane(3)),
                    second: Box::new(PaneLayout::Pane(2)),
                }),
            }),
            second: Box::new(PaneLayout::Pane(4)),
        }
    );
}

#[test]
fn pane_template_replaces_only_the_target_leaf() {
    let template = PaneSplitTemplate::Split {
        axis: PaneSplitAxis::Horizontal,
        first: Box::new(PaneSplitTemplate::Pane(None)),
        second: Box::new(PaneSplitTemplate::Pane(None)),
    };
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    let replacement = PaneLayout::from_template(&template, &mut [2, 3].into_iter());

    assert!(layout.replace(2, replacement));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(2)),
                second: Box::new(PaneLayout::Pane(3)),
            }),
        }
    );
}

#[test]
fn pane_template_labels_follow_the_materialized_leaf_order() {
    let template = PaneSplitTemplate::Split {
        axis: PaneSplitAxis::Vertical,
        first: Box::new(PaneSplitTemplate::Pane(Some("left".to_owned()))),
        second: Box::new(PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Horizontal,
            first: Box::new(PaneSplitTemplate::Pane(Some("top-right".to_owned()))),
            second: Box::new(PaneSplitTemplate::Pane(Some("bottom-right".to_owned()))),
        }),
    };

    let layout = PaneLayout::from_template(&template, &mut [10, 11, 12].into_iter());

    assert_eq!(
        template.pane_labels(),
        vec![
            Some("left".to_owned()),
            Some("top-right".to_owned()),
            Some("bottom-right".to_owned()),
        ]
    );
    assert_eq!(layout.first_pane(), 10);
    assert_eq!(
        layout
            .regions()
            .iter()
            .map(|region| region.id)
            .collect::<Vec<_>>(),
        [10, 11, 12]
    );
}

#[test]
fn pane_layout_replacement_moves_the_tree_without_cloning_it() {
    let replacement = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(10)),
        second: Box::new(PaneLayout::Pane(11)),
    };
    let original_first_child = match &replacement {
        PaneLayout::Split { first, .. } => first.as_ref() as *const PaneLayout,
        PaneLayout::Pane(_) => unreachable!(),
    };
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
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
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(1)),
                second: Box::new(PaneLayout::Pane(2)),
            }),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
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
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
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
            first: Box::new(PaneSplitTemplate::Pane(None)),
            second: Box::new(PaneSplitTemplate::Pane(None)),
        },
    )]);
    let layout = pane_layout_from_configured_template(&templates, "two", &mut [10, 11].into_iter());

    assert_eq!(
        layout,
        Some(PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(10)),
            second: Box::new(PaneLayout::Pane(11)),
        })
    );
    assert!(templates.contains_key("two"));
}

#[test]
fn four_vertical_template_materializes_left_to_right_equal_columns() {
    let template = PaneSplitTemplate::Split {
        axis: PaneSplitAxis::Vertical,
        first: Box::new(PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Vertical,
            first: Box::new(PaneSplitTemplate::Pane(None)),
            second: Box::new(PaneSplitTemplate::Pane(None)),
        }),
        second: Box::new(PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Vertical,
            first: Box::new(PaneSplitTemplate::Pane(None)),
            second: Box::new(PaneSplitTemplate::Pane(None)),
        }),
    };
    let layout = PaneLayout::from_template(&template, &mut [1, 2, 3, 4].into_iter());

    assert_eq!(
        layout.regions(),
        vec![
            PaneRegion {
                id: 1,
                left: 0.,
                right: 0.25,
                top: 0.,
                bottom: 1.,
            },
            PaneRegion {
                id: 2,
                left: 0.25,
                right: 0.5,
                top: 0.,
                bottom: 1.,
            },
            PaneRegion {
                id: 3,
                left: 0.5,
                right: 0.75,
                top: 0.,
                bottom: 1.,
            },
            PaneRegion {
                id: 4,
                left: 0.75,
                right: 1.,
                top: 0.,
                bottom: 1.,
            },
        ]
    );
}

#[test]
fn tab_pane_index_resolves_panes_without_scanning() {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        icon: ProfileIcon::Zetta,
    };
    let panes = [1, 2, 3]
        .into_iter()
        .map(|id| TerminalPane {
            id,
            label_number: id as usize,
            generated_label: None,
            custom_label: None,
            overlay_text: None,
            overlay_font_size: None,
            overlay_opacity: None,
            overlay_color: None,
            profile: profile.clone(),
            terminal: None,
            view: None,
            error: None,
            base_exited: false,
            wsl_cwd_file: None,
            pending_command: None,
            stack: PaneStack::default(),
        })
        .collect::<Vec<_>>();
    let mut tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
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
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        custom_title: None,
        icon: Some(IconName::Terminal),
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
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
        overlay_text: None,
        overlay_font_size: None,
        overlay_opacity: None,
        overlay_color: None,
        profile,
        terminal: None,
        view: None,
        error: None,
        base_exited: false,
        wsl_cwd_file: None,
        pending_command: None,
        stack: PaneStack::default(),
    });
    assert_eq!(tab.pane(4).map(|pane| pane.id), Some(4));
}

#[test]
fn nested_pane_layouts_split_and_collapse() {
    let mut layout = PaneLayout::Pane(1);
    assert!(layout.split(1, SplitAxis::Horizontal, 2, SplitPosition::After));
    assert!(layout.split(2, SplitAxis::Vertical, 3, SplitPosition::After));
    assert!(!layout.split(99, SplitAxis::Vertical, 4, SplitPosition::After));

    let layout = layout.without(2).unwrap();
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(3)),
        }
    );
}

#[test]
fn pane_layouts_can_insert_new_panes_before_the_active_pane() {
    let mut layout = PaneLayout::Pane(1);

    assert!(layout.split(1, SplitAxis::Vertical, 2, SplitPosition::Before));
    assert!(layout.split(1, SplitAxis::Horizontal, 3, SplitPosition::Before));

    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(3)),
                second: Box::new(PaneLayout::Pane(1)),
            }),
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
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
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
        icon: ProfileIcon::Zetta,
    };
    let zsh = Profile {
        name: "Zsh".to_owned(),
        command: task::Shell::Program("zsh".to_owned()),
        theme: Some("One Light".to_owned()),
        icon: ProfileIcon::Zsh,
    };
    let tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
        panes: vec![
            TerminalPane {
                id: 1,
                label_number: 1,
                generated_label: None,
                custom_label: None,
                overlay_text: None,
                overlay_font_size: None,
                overlay_opacity: None,
                overlay_color: None,
                profile: system,
                terminal: None,
                view: None,
                error: None,
                base_exited: false,
                wsl_cwd_file: None,
                pending_command: None,
                stack: PaneStack::default(),
            },
            TerminalPane {
                id: 2,
                label_number: 2,
                generated_label: None,
                custom_label: None,
                overlay_text: None,
                overlay_font_size: None,
                overlay_opacity: None,
                overlay_color: None,
                profile: zsh,
                terminal: None,
                view: None,
                error: None,
                base_exited: false,
                wsl_cwd_file: None,
                pending_command: None,
                stack: PaneStack::default(),
            },
        ],
        pane_indices: HashMap::from([(1, 0), (2, 1)]),
        next_pane_label: 3,
        layout: PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(2)),
        },
        active_pane: 2,
        focus_history: vec![1, 2],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        custom_title: None,
        icon: Some(IconName::Terminal),
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
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
        icon: ProfileIcon::Zetta,
    };
    let pane = |id| TerminalPane {
        id,
        label_number: id as usize,
        generated_label: None,
        custom_label: None,
        overlay_text: None,
        overlay_font_size: None,
        overlay_opacity: None,
        overlay_color: None,
        profile: profile.clone(),
        terminal: None,
        view: None,
        error: None,
        base_exited: false,
        wsl_cwd_file: None,
        pending_command: None,
        stack: PaneStack::default(),
    };
    let mut tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
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
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        custom_title: None,
        icon: Some(IconName::Terminal),
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
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
        icon: ProfileIcon::Zetta,
    };
    let pane = |id| TerminalPane {
        id,
        label_number: id as usize,
        generated_label: None,
        custom_label: None,
        overlay_text: None,
        overlay_font_size: None,
        overlay_opacity: None,
        overlay_color: None,
        profile: profile.clone(),
        terminal: None,
        view: None,
        error: None,
        base_exited: false,
        wsl_cwd_file: None,
        pending_command: None,
        stack: PaneStack::default(),
    };
    let mut tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
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
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        custom_title: None,
        icon: Some(IconName::Terminal),
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
    };

    tab.remove_pane(1);
    tab.restore_focus_after_close(1, 2);

    assert_eq!(tab.active_pane, 3);
    assert_eq!(tab.focus_history, vec![2, 3]);
}

#[test]
fn directional_focus_moves_between_quarter_panes() {
    let mut layout = PaneLayout::Pane(1);
    assert!(layout.split(1, SplitAxis::Horizontal, 2, SplitPosition::After));
    assert!(layout.split(1, SplitAxis::Vertical, 3, SplitPosition::After));
    assert!(layout.split(2, SplitAxis::Vertical, 4, SplitPosition::After));

    assert_eq!(layout.adjacent_pane(1, PaneDirection::Right), Some(3));
    assert_eq!(layout.adjacent_pane(1, PaneDirection::Down), Some(2));
    assert_eq!(layout.adjacent_pane(3, PaneDirection::Down), Some(4));
    assert_eq!(layout.adjacent_pane(4, PaneDirection::Left), Some(2));
    assert_eq!(layout.adjacent_pane(4, PaneDirection::Up), Some(3));
    assert_eq!(layout.regions().len(), 4);
}

#[test]
fn pane_resize_boundary_moves_the_nearest_matching_split() {
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(3)),
        }),
    };

    let boundary = layout.resize_boundary(1, SplitAxis::Vertical).unwrap();
    assert_eq!(boundary.parent_fraction, 1.);
    assert!(boundary.active_is_first);
    assert_eq!(boundary.sibling_panes, [2, 3]);
    assert!(layout.adjust_resize_boundary(1, SplitAxis::Vertical, 0.1));

    let first = layout
        .regions()
        .into_iter()
        .find(|region| region.id == 1)
        .unwrap();
    assert!((first.right - first.left - 0.6).abs() < f32::EPSILON);

    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    assert!(layout.adjust_resize_boundary(2, SplitAxis::Vertical, 0.1));
    let second = layout
        .regions()
        .into_iter()
        .find(|region| region.id == 2)
        .unwrap();
    assert!((second.right - second.left - 0.6).abs() < f32::EPSILON);

    let boundary = layout.resize_boundary(2, SplitAxis::Vertical).unwrap();
    assert!(!boundary.active_is_first);
}

#[test]
fn moving_into_a_two_pane_split_swaps_the_leaves() {
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: 700,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };

    assert!(layout.move_pane(1, PaneDirection::Right));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: PANE_SPLIT_RATIO_SCALE - 700,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(1)),
        }
    );

    // 1 is already the rightmost pane, so moving it further right is a no-op.
    assert!(!layout.move_pane(1, PaneDirection::Right));

    assert!(layout.move_pane(1, PaneDirection::Left));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: 700,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(2)),
        }
    );
}

#[test]
fn moving_a_small_pane_flips_the_three_pane_layout() {
    // One big pane on the left, two small panes stacked on the right - the
    // default three-pane layout from `PaneLayout::tiled`.
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: 700,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(3)),
        }),
    };

    // The inner split's axis never matches a left/right move, so the search
    // bubbles up and swaps the whole stack with the big pane.
    assert!(layout.move_pane(2, PaneDirection::Left));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: PANE_SPLIT_RATIO_SCALE - 700,
            first: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(2)),
                second: Box::new(PaneLayout::Pane(3)),
            }),
            second: Box::new(PaneLayout::Pane(1)),
        }
    );
}

#[test]
fn moving_prefers_the_innermost_matching_boundary_before_bubbling_up() {
    // Quarters layout: left column {1 over 2}, right column {3 over 4}.
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: 300,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(2)),
        }),
        second: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(3)),
            second: Box::new(PaneLayout::Pane(4)),
        }),
    };

    // Moving down swaps pane 1 with its own column sibling only; the right
    // column is untouched.
    assert!(layout.move_pane(1, PaneDirection::Down));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: PANE_SPLIT_RATIO_SCALE - 300,
                first: Box::new(PaneLayout::Pane(2)),
                second: Box::new(PaneLayout::Pane(1)),
            }),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(3)),
                second: Box::new(PaneLayout::Pane(4)),
            }),
        }
    );

    // Moving right has no local boundary to use (the only vertical-axis
    // split is the outer one), so it swaps the whole column - pane 2 (now
    // on top of the left column) travels along with pane 1.
    assert!(layout.move_pane(1, PaneDirection::Right));
    assert_eq!(layout.first_pane(), 3);
}

#[test]
fn moving_is_a_no_op_when_no_matching_axis_ancestor_exists() {
    let mut single = PaneLayout::Pane(1);
    assert!(!single.move_pane(1, PaneDirection::Left));
    assert!(!single.move_pane(1, PaneDirection::Up));

    // Two panes side by side have no horizontal-axis split, so vertical
    // moves are no-ops, as is moving further past an edge.
    let mut side_by_side = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    assert!(!side_by_side.move_pane(1, PaneDirection::Up));
    assert!(!side_by_side.move_pane(1, PaneDirection::Down));
    assert!(!side_by_side.move_pane(2, PaneDirection::Right));

    // A pane id that isn't in the layout is a no-op rather than a panic.
    assert!(!side_by_side.move_pane(99, PaneDirection::Left));
}

#[test]
fn swap_panes_exchanges_leaves_anywhere_in_the_tree() {
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: 700,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(3)),
        }),
    };

    assert!(layout.swap_panes(1, 3));
    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: 700,
            first: Box::new(PaneLayout::Pane(3)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(2)),
                second: Box::new(PaneLayout::Pane(1)),
            }),
        }
    );
}

#[test]
fn swap_panes_rejects_missing_or_identical_ids() {
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    let original = layout.clone();

    assert!(!layout.swap_panes(1, 1));
    assert!(!layout.swap_panes(1, 99));
    assert_eq!(layout, original);
}

#[test]
fn pane_resize_gutter_targets_its_exact_split() {
    let mut layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(2)),
        }),
        second: Box::new(PaneLayout::Pane(3)),
    };

    assert_eq!(
        layout.split_panes(1, 3, SplitAxis::Vertical),
        Some((vec![1, 2], vec![3]))
    );
    assert!(layout.adjust_split_ratio(1, 3, SplitAxis::Vertical, 0.1));

    assert_eq!(
        layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: 600,
            first: Box::new(PaneLayout::Split {
                axis: SplitAxis::Vertical,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(1)),
                second: Box::new(PaneLayout::Pane(2)),
            }),
            second: Box::new(PaneLayout::Pane(3)),
        }
    );
}

fn pane_management_tab() -> Tab {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = |id| TerminalPane {
        id,
        label_number: id as usize,
        generated_label: None,
        custom_label: None,
        overlay_text: None,
        overlay_font_size: None,
        overlay_opacity: None,
        overlay_color: None,
        profile: profile.clone(),
        terminal: None,
        view: None,
        error: None,
        base_exited: false,
        wsl_cwd_file: None,
        pending_command: None,
        stack: PaneStack::default(),
    };
    let layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(3)),
        }),
    };
    Tab {
        id: 1,
        attention_id: 1,
        attention: None,
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
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        custom_title: None,
        icon: Some(IconName::Terminal),
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
    }
}

#[test]
fn transferred_tabs_receive_target_window_ids_consistently() {
    let mut tab = pane_management_tab();
    let profile = tab.pane(2).unwrap().profile.clone();
    let pane = tab.pane_mut(2).unwrap();
    assert!(pane.stack.push(StackedPane::new(
        7,
        "first".to_owned(),
        profile.clone(),
        None,
        None
    )));
    assert!(pane.stack.push(StackedPane::new(
        8,
        "second".to_owned(),
        profile,
        None,
        None
    )));
    tab.attention_id = 99;
    tab.attention = Some(TabAttention {
        summary: "Build finished".to_owned(),
        body: Some("All tests passed".to_owned()),
    });
    tab.maximized_pane = Some(2);
    tab.minimized_panes = vec![1, 3];
    tab.selected_minimized_pane = Some(3);
    let mut next_pane_id = 20;

    tab.reassign_ids(10, &mut next_pane_id);

    assert_eq!(tab.id, 10);
    assert_eq!(tab.attention_id, 99);
    assert_eq!(
        tab.attention.as_ref().unwrap().tooltip_text(),
        "Build finished\nAll tests passed"
    );
    assert_eq!(next_pane_id, 25);
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
        tab.pane(21)
            .unwrap()
            .stack
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        [23, 24]
    );
    assert_eq!(
        tab.pane(21).unwrap().stack.selected,
        PaneStackSelection::Stacked(24)
    );
    assert_eq!(
        tab.layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(20)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(21)),
                second: Box::new(PaneLayout::Pane(22)),
            }),
        }
    );
}

#[test]
fn tab_close_policy_distinguishes_close_from_unprotected_backgrounding() {
    assert!(TabClosePolicy::Close.background_authentication().is_none());
    assert!(
        TabClosePolicy::Background {
            authentication: None,
        }
        .background_authentication()
        .is_some_and(|authentication| authentication.is_none())
    );

    let authentication = SessionAuthentication::create("correct horse battery staple").unwrap();
    let selected = TabClosePolicy::Background {
        authentication: Some(authentication),
    }
    .background_authentication()
    .flatten()
    .unwrap();
    assert!(selected.verify("correct horse battery staple"));
    assert!(!selected.verify("wrong"));
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
fn maximizing_the_only_visible_pane_is_a_no_op() {
    let mut tab = pane_management_tab();

    assert!(tab.minimize(2));
    assert!(tab.minimize(3));
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(1)));

    assert!(!tab.toggle_maximize(1));
    assert_eq!(tab.maximized_pane, None);
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(1)));
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
        overlay_text: None,
        overlay_font_size: None,
        overlay_opacity: None,
        overlay_color: None,
        profile,
        terminal: None,
        view: None,
        error: None,
        base_exited: false,
        wsl_cwd_file: None,
        pending_command: None,
        stack: PaneStack::default(),
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
fn template_labels_replace_generated_labels_but_preserve_manual_labels() {
    let mut tab = pane_management_tab();
    tab.pane_mut(1).unwrap().generated_label = Some("old-left".to_owned());
    tab.pane_mut(2).unwrap().generated_label = Some("old-middle".to_owned());
    tab.pane_mut(2).unwrap().custom_label = Some("Manual".to_owned());
    tab.pane_mut(3).unwrap().generated_label = Some("old-right".to_owned());

    tab.apply_generated_labels([
        (1, Some("top-left".to_owned())),
        (2, Some("top-right".to_owned())),
        (3, None),
    ]);

    assert_eq!(tab.pane(1).unwrap().label(), "top-left");
    assert_eq!(tab.pane(2).unwrap().label(), "Manual");
    assert_eq!(
        tab.pane(2).unwrap().generated_label.as_deref(),
        Some("top-right")
    );
    assert_eq!(tab.pane(3).unwrap().label(), "Pane 3");
    assert_eq!(tab.pane(3).unwrap().generated_label, None);
}

#[test]
fn pane_overlay_is_hidden_by_default_and_renders_while_editing() {
    let mut tab = pane_management_tab();

    assert_eq!(tab.displayed_pane_overlay(2), None);

    tab.pane_mut(2).unwrap().overlay_text = Some("Prod".to_owned());
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("Prod"));

    tab.editing_overlay_pane = Some(2);
    tab.overlay_buffer = Some("Staging".to_owned());
    tab.overlay_cursor = 4;
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("Stag|ing"));

    tab.overlay_select_all = true;
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("Staging"));

    tab.overlay_buffer = Some(String::new());
    tab.overlay_cursor = 0;
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("|"));

    tab.editing_overlay_pane = None;
    tab.overlay_buffer = None;
    tab.overlay_select_all = false;
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("Prod"));

    tab.pane_mut(2).unwrap().overlay_text = None;
    assert_eq!(tab.displayed_pane_overlay(2), None);
}

#[test]
fn overlay_style_picker_percent_snaps_to_five_percent_steps() {
    let default = OverlayStylePicker::percent_for_opacity(None);
    assert_eq!(DEFAULT_OVERLAY_OPACITY, 0.85);
    assert_eq!(default, 85);

    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(0.)), 0);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(0.5)), 50);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(1.)), 100);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(0.37)), 35);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(1.4)), 100);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(-0.2)), 0);
}

#[test]
fn overlay_style_picker_preset_cursor_clamps_to_the_six_column_grid() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::ColorPresets,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };

    assert_eq!(OVERLAY_COLOR_PRESET_COLUMNS, 6);
    picker.move_preset_cursor(0, -1);
    assert_eq!(picker.preset_index, 0);
    picker.move_preset_cursor(-1, 0);
    assert_eq!(picker.preset_index, 0);

    picker.move_preset_cursor(0, 1);
    assert_eq!(picker.preset_index, 1);
    picker.set_preset_index(5);
    picker.move_preset_cursor(0, 1);
    assert_eq!(picker.preset_index, 5);
    picker.move_preset_cursor(1, 0);
    assert_eq!(picker.preset_index, 11);
    picker.move_preset_cursor(1, 0);
    assert_eq!(picker.preset_index, 11);

    picker.move_preset_cursor(0, -1);
    assert_eq!(picker.preset_index, 10);
    picker.move_preset_cursor(-1, 0);
    assert_eq!(picker.preset_index, 4);
    picker.set_preset_index(6);
    picker.move_preset_cursor(0, -1);
    assert_eq!(picker.preset_index, 6);
    picker.set_preset_index(usize::MAX);
    assert_eq!(picker.preset_index, 11);
}

#[test]
fn overlay_style_picker_preset_cursor_starts_at_a_matching_preset_or_first() {
    let red = overlay_color_from_hex("#ff0000").unwrap();
    assert_eq!(OverlayStylePicker::preset_index_for_color(red), 3);

    let custom = overlay_color_from_hex("#123456").unwrap();
    assert_eq!(OverlayStylePicker::preset_index_for_color(custom), 0);
}

#[test]
fn overlay_color_presets_parse_to_their_canonical_opaque_values() {
    let expected = [
        ("black", "#000000"),
        ("white", "#ffffff"),
        ("gray", "#808080"),
        ("red", "#ff0000"),
        ("orange", "#ffa500"),
        ("yellow", "#ffff00"),
        ("green", "#008000"),
        ("cyan", "#00ffff"),
        ("blue", "#0000ff"),
        ("purple", "#800080"),
        ("magenta", "#ff00ff"),
        ("pink", "#ffc0cb"),
    ];

    assert_eq!(OVERLAY_COLOR_PRESETS.len(), expected.len());
    for (preset, (name, hex)) in OVERLAY_COLOR_PRESETS.iter().zip(expected) {
        assert_eq!((preset.name, preset.hex), (name, hex));
        let color = overlay_color_from_value(preset.name).expect("preset should parse");
        assert_eq!(overlay_color_to_hex(color), hex);
        assert!((color.a - 1.).abs() < f32::EPSILON);
    }

    assert_eq!(
        overlay_color_to_hex(overlay_color_from_value("  ReD  ").unwrap()),
        "#ff0000"
    );
    assert_eq!(
        overlay_color_to_hex(overlay_color_from_value("YELLOW").unwrap()),
        "#ffff00"
    );
}

#[test]
fn overlay_color_value_parser_preserves_hex_forms_and_rejects_invalid_values() {
    for (value, expected_hex, expected_alpha) in [
        ("f0a", "#ff00aa", 1.),
        ("#f0a8", "#ff00aa", 136. / 255.),
        ("112233", "#112233", 1.),
        ("#11223344", "#112233", 68. / 255.),
    ] {
        let color = overlay_color_from_value(value).expect("hex colour should parse");
        assert_eq!(overlay_color_to_hex(color), expected_hex);
        assert!((color.a - expected_alpha).abs() < 1e-6);
    }

    for value in ["", "grey", "not-a-color", "#12", "#ggg", "#12345"] {
        assert!(
            overlay_color_from_value(value).is_none(),
            "expected {value:?} to be rejected"
        );
    }
}

#[test]
fn overlay_style_picker_round_trips_hsl_to_hsv() {
    for hsla in [
        gpui::hsla(0., 1., 0.5, 1.),   // red
        gpui::hsla(0.5, 1., 0.5, 1.),  // cyan
        gpui::hsla(0.3, 0.4, 0.2, 1.), // dark olive
        gpui::hsla(0., 0., 1., 1.),    // white
        gpui::hsla(0., 0., 0., 1.),    // black
    ] {
        let (hue, saturation, value) = hsla_to_hsv(hsla);
        let back = hsv_to_hsla(hue, saturation, value);
        assert!((back.h - hsla.h).abs() < 1e-4);
        assert!((back.s - hsla.s).abs() < 1e-4);
        assert!((back.l - hsla.l).abs() < 1e-4);
    }
}

#[test]
fn overlay_style_picker_hex_field_colors_the_selection() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };
    picker.refresh_hex();
    assert_eq!(picker.hex_buffer, "#ffffff");

    // A complete buffer is replaced by a fresh entry instead of growing.
    assert!(!picker.hex_input('#'));
    assert!(!picker.hex_input('f'));
    assert!(!picker.hex_input('0'));
    assert_eq!(picker.hex_buffer, "#f0");

    picker.refresh_hex();
    for ch in ['f', 'f', '0', '0', '0', '0'] {
        picker.hex_input(ch);
    }
    assert_eq!(picker.hex_buffer, "#ff0000");
    let color = picker.color();
    assert!(color.h.abs() < 1e-3 || (color.h - 1.).abs() < 1e-3);
    assert!(color.s > 0.99);
}

#[test]
fn overlay_style_picker_preset_selection_keeps_hsv_and_hex_editing_live() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };

    let orange = OVERLAY_COLOR_PRESETS
        .iter()
        .find(|preset| preset.name == "orange")
        .copied()
        .unwrap();
    picker.set_color_preset(orange);
    assert_eq!(picker.preset_index, 4);
    assert_eq!(picker.hex_buffer, orange.hex);
    assert_eq!(overlay_color_to_hex(picker.color()), orange.hex);

    picker.adjust_value(-0.2);
    assert!(picker.value < 1.);
    assert_ne!(picker.hex_buffer, orange.hex);
    assert_eq!(picker.preset_index, 4);

    picker.hex_buffer = "#".to_owned();
    for digit in ['0', '0', '8', '0', '0', '0'] {
        picker.hex_input(digit);
    }
    assert_eq!(picker.hex_buffer, "#008000");
    assert_eq!(overlay_color_to_hex(picker.color()), "#008000");
    assert_eq!(picker.preset_index, 4);
}

#[test]
fn overlay_style_picker_hex_accepts_three_digit_codes() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };
    picker.refresh_hex();

    // `#f00` commits as soon as the third digit lands and keeps its literal
    // buffer form so a longer code can keep being typed.
    assert!(!picker.hex_input('f'));
    assert!(!picker.hex_input('0'));
    assert!(picker.hex_input('0'));
    assert_eq!(picker.hex_buffer, "#f00");
    let red = picker.color();
    assert!(red.h.abs() < 1e-3 || (red.h - 1.).abs() < 1e-3);
    assert!(red.s > 0.99);

    // A six-digit code typed straight through is not interrupted by the
    // short-form commit.
    picker.refresh_hex();
    for ch in ['f', 'f', '0', '0', '0', '0'] {
        picker.hex_input(ch);
    }
    assert_eq!(picker.hex_buffer, "#ff0000");
    let red = picker.color();
    assert!(red.h.abs() < 1e-3 || (red.h - 1.).abs() < 1e-3);
    assert!(red.s > 0.99);
}

#[test]
fn overlay_style_picker_hex_backspace_keeps_the_hash() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };
    picker.refresh_hex();
    picker.hex_input('a');
    picker.hex_input('b');
    picker.hex_input('c');

    // Backspace keeps the leading `#`; repeated backspaces never clear it.
    assert!(!picker.hex_backspace());
    picker.hex_backspace();
    picker.hex_backspace();
    assert_eq!(picker.hex_buffer, "#");
    assert!(!picker.hex_backspace());
    assert_eq!(picker.hex_buffer, "#");
}

#[test]
fn overlay_style_picker_seeds_a_pleasant_hue_for_achromatic_colors() {
    let white = gpui::hsla(0., 0., 1., 1.);
    let (hue, saturation, value) = overlay_picker_hsv_from_hsla(white);
    assert_eq!(hue, DEFAULT_PICKER_HUE);
    assert!(saturation < 0.05);
    assert!(value > 0.95);

    let blue = gpui::hsla(0.6, 1., 0.5, 1.);
    let (hue, saturation, value) = overlay_picker_hsv_from_hsla(blue);
    assert!((hue - 0.6).abs() < 1e-3);
    assert!(saturation > 0.99);
    assert!(value > 0.4);
}

#[test]
fn overlay_font_size_steps_wrap_around_the_ends() {
    assert_eq!(
        OverlayFontSize::Small.step(-1),
        OverlayFontSize::ExtraExtraExtraLarge
    );
    assert_eq!(
        OverlayFontSize::ExtraExtraExtraLarge.step(1),
        OverlayFontSize::Small
    );
    assert_eq!(
        OverlayFontSize::ExtraLarge.step(1),
        OverlayFontSize::ExtraExtraLarge
    );
    assert_eq!(OverlayFontSize::ExtraLarge.step(-2), OverlayFontSize::Base);
}

#[test]
fn overlay_picker_section_steps_wrap_around_the_ends() {
    assert_eq!(
        OverlayPickerSection::FontSize.step(-1),
        OverlayPickerSection::ColorPresets
    );
    assert_eq!(
        OverlayPickerSection::FontSize.step(1),
        OverlayPickerSection::Opacity
    );
    assert_eq!(
        OverlayPickerSection::Opacity.step(1),
        OverlayPickerSection::Color
    );
    assert_eq!(
        OverlayPickerSection::Color.step(1),
        OverlayPickerSection::ColorPresets
    );
    assert_eq!(
        OverlayPickerSection::ColorPresets.step(1),
        OverlayPickerSection::FontSize
    );
    assert_eq!(
        OverlayPickerSection::Opacity.step(-1),
        OverlayPickerSection::FontSize
    );
    assert_eq!(
        OverlayPickerSection::Color.step(-1),
        OverlayPickerSection::Opacity
    );
    assert_eq!(
        OverlayPickerSection::ColorPresets.step(-1),
        OverlayPickerSection::Color
    );
}

#[test]
fn overlay_style_picker_preserves_committed_text_and_holds_values() {
    let mut tab = pane_management_tab();
    let pane_id = 2;
    tab.pane_mut(pane_id).unwrap().overlay_text = Some("Prod".to_owned());
    tab.editing_overlay_pane = None;
    tab.overlay_buffer = None;

    tab.overlay_style_picker = Some(OverlayStylePicker {
        pane_id,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::Large,
        original_font_size: None,
        hue: 0.6,
        saturation: 0.7,
        value: 0.4,
        original_color: None,
        preset_index: 0,
        opacity_percent: 60,
        original_opacity: Some(0.6),
        hex_buffer: String::new(),
    });
    let picker = tab.overlay_style_picker.as_ref().unwrap();

    assert_eq!(picker.opacity_percent, 60);
    assert_eq!(picker.section, OverlayPickerSection::Color);
    assert_eq!(picker.font_size, OverlayFontSize::Large);
    assert_eq!(tab.displayed_pane_overlay(pane_id).as_deref(), Some("Prod"));
    assert_eq!(tab.editing_overlay_pane, None);
}

#[test]
fn tabs_use_the_terminal_icon_by_default() {
    assert_eq!(pane_management_tab().icon, Some(IconName::Terminal));
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
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
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
