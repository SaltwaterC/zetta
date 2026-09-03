use super::*;

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
        first: Box::new(PaneSplitTemplate::Pane(Box::default())),
        second: Box::new(PaneSplitTemplate::Pane(Box::default())),
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
        first: Box::new(PaneSplitTemplate::Pane(Box::new(PaneSplitPane {
            label: Some("left".to_owned()),
            ..PaneSplitPane::default()
        }))),
        second: Box::new(PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Horizontal,
            first: Box::new(PaneSplitTemplate::Pane(Box::new(PaneSplitPane {
                label: Some("top-right".to_owned()),
                ..PaneSplitPane::default()
            }))),
            second: Box::new(PaneSplitTemplate::Pane(Box::new(PaneSplitPane {
                label: Some("bottom-right".to_owned()),
                ..PaneSplitPane::default()
            }))),
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
fn configured_template_layout_is_built_through_a_borrow() {
    let templates = HashMap::from([(
        "two".to_owned(),
        PaneSplitTemplateConfig {
            layout: PaneSplitTemplate::Split {
                axis: PaneSplitAxis::Vertical,
                first: Box::new(PaneSplitTemplate::Pane(Box::default())),
                second: Box::new(PaneSplitTemplate::Pane(Box::default())),
            },
            env: HashMap::new(),
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
            first: Box::new(PaneSplitTemplate::Pane(Box::default())),
            second: Box::new(PaneSplitTemplate::Pane(Box::default())),
        }),
        second: Box::new(PaneSplitTemplate::Split {
            axis: PaneSplitAxis::Vertical,
            first: Box::new(PaneSplitTemplate::Pane(Box::default())),
            second: Box::new(PaneSplitTemplate::Pane(Box::default())),
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
fn directional_focus_moves_between_quarter_panes() {
    let mut layout = PaneLayout::Pane(1);
    assert!(layout.split(1, SplitAxis::Horizontal, 2, SplitPosition::After));
    assert!(layout.split(1, SplitAxis::Vertical, 3, SplitPosition::After));
    assert!(layout.split(2, SplitAxis::Vertical, 4, SplitPosition::After));

    assert_eq!(layout.adjacent_pane(1, PaneDirection::Right, &[]), Some(3));
    assert_eq!(layout.adjacent_pane(1, PaneDirection::Down, &[]), Some(2));
    assert_eq!(layout.adjacent_pane(3, PaneDirection::Down, &[]), Some(4));
    assert_eq!(layout.adjacent_pane(4, PaneDirection::Left, &[]), Some(2));
    assert_eq!(layout.adjacent_pane(4, PaneDirection::Up, &[]), Some(3));
    assert_eq!(layout.regions().len(), 4);
}

#[test]
fn directional_focus_defaults_to_topmost_candidate_when_tied() {
    // left | top-right
    //      | bottom-right
    let mut layout = PaneLayout::Pane(1);
    assert!(layout.split(1, SplitAxis::Vertical, 2, SplitPosition::After));
    assert!(layout.split(2, SplitAxis::Horizontal, 3, SplitPosition::After));

    // Moving right out of the full-height left pane ties between top-right
    // (2) and bottom-right (3); with no history, the first one in tree order
    // wins.
    assert_eq!(layout.adjacent_pane(1, PaneDirection::Right, &[]), Some(2));
}

#[test]
fn directional_focus_retains_last_focused_pane_in_a_column() {
    // left | top-right
    //      | bottom-right
    let mut layout = PaneLayout::Pane(1);
    assert!(layout.split(1, SplitAxis::Vertical, 2, SplitPosition::After));
    assert!(layout.split(2, SplitAxis::Horizontal, 3, SplitPosition::After));

    // Having previously focused bottom-right (3) more recently than
    // top-right (2), moving right out of the left pane should return to
    // bottom-right rather than resetting to the topmost candidate.
    let recent = [2, 3];
    assert_eq!(
        layout.adjacent_pane(1, PaneDirection::Right, &recent),
        Some(3)
    );

    // The reverse history should restore top-right instead.
    let recent = [3, 2];
    assert_eq!(
        layout.adjacent_pane(1, PaneDirection::Right, &recent),
        Some(2)
    );
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
