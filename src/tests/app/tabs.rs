use super::*;

fn pin_test_tab(id: u64, pinned: bool) -> Tab {
    let profile = Profile {
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
        shared: false,
        custom_title: None,
        worktree_seed_title: None,
        process_title: None,
        icon: Some(IconName::Terminal),
        icon_override: TabIconOverride::None,
        pinned,
        renaming_pane: None,
        rename_buffer: None,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_style_picker: None,
    }
}

fn tab_ids(tabs: &[Tab]) -> Vec<u64> {
    tabs.iter().map(|tab| tab.id).collect()
}

#[test]
fn tab_move_mode_moves_adjacent_tabs_without_wrapping() {
    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        move_item_by_id(&mut ids, 3, TabMoveDirection::Left, 3, true, |id| *id),
        Some(1)
    );
    assert_eq!(ids, vec![1, 3, 2, 4]);

    assert_eq!(
        move_item_by_id(&mut ids, 3, TabMoveDirection::Right, 3, true, |id| *id),
        Some(2)
    );
    assert_eq!(ids, vec![1, 2, 3, 4]);
}

#[test]
fn tab_move_mode_stops_at_boundaries_and_when_disabled() {
    let mut ids = vec![1, 2, 3];
    assert_eq!(
        move_item_by_id(&mut ids, 1, TabMoveDirection::Left, 1, true, |id| *id),
        None
    );
    assert_eq!(
        move_item_by_id(&mut ids, 3, TabMoveDirection::Right, 3, true, |id| *id),
        None
    );
    assert_eq!(
        move_item_by_id(&mut ids, 2, TabMoveDirection::Right, 2, false, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);
}

#[test]
fn visual_pinning_toggles_at_the_pinned_prefix_without_losing_the_active_tab() {
    let mut tabs = vec![
        pin_test_tab(1, true),
        pin_test_tab(2, false),
        pin_test_tab(3, false),
    ];

    let active_index = toggle_tab_pinning_in_order(&mut tabs, 2);
    assert_eq!(active_index, Some(1));
    assert_eq!(tab_ids(&tabs), vec![1, 3, 2]);
    assert!(tabs[1].pinned);

    let active_index = toggle_tab_pinning_in_order(&mut tabs, 0);
    assert_eq!(active_index, Some(1));
    assert_eq!(tab_ids(&tabs), vec![3, 1, 2]);
    assert!(!tabs[1].pinned);
    assert_eq!(pinned_tab_count(&tabs), 1);
}

#[test]
fn reconnected_tabs_reenter_the_pinned_prefix() {
    let mut tabs = vec![pin_test_tab(1, true), pin_test_tab(2, false)];
    let pinned_index = insert_tab_in_pin_order(&mut tabs, pin_test_tab(3, true));
    assert_eq!(pinned_index, 1);
    assert_eq!(tab_ids(&tabs), vec![1, 3, 2]);

    let unpinned_index = insert_tab_in_pin_order(&mut tabs, pin_test_tab(4, false));
    assert_eq!(unpinned_index, 3);
    assert_eq!(tab_ids(&tabs), vec![1, 3, 2, 4]);
}

#[test]
fn tab_move_and_drag_boundaries_preserve_the_pinned_prefix() {
    let tabs = vec![
        pin_test_tab(1, true),
        pin_test_tab(2, true),
        pin_test_tab(3, false),
        pin_test_tab(4, false),
    ];
    assert!(tab_move_preserves_pinning(&tabs, 1, TabMoveDirection::Left));
    assert!(!tab_move_preserves_pinning(
        &tabs,
        1,
        TabMoveDirection::Right
    ));
    assert!(!tab_move_preserves_pinning(
        &tabs,
        2,
        TabMoveDirection::Left
    ));
    assert!(tab_move_preserves_pinning(
        &tabs,
        2,
        TabMoveDirection::Right
    ));
    assert!(tab_drop_preserves_pinning(
        &tabs,
        1,
        TabDropPosition::Before(2)
    ));
    assert!(!tab_drop_preserves_pinning(
        &tabs,
        1,
        TabDropPosition::After(3)
    ));
    assert!(tab_drop_preserves_pinning(
        &tabs,
        3,
        TabDropPosition::After(4)
    ));
    assert!(!tab_drop_preserves_pinning(
        &tabs,
        3,
        TabDropPosition::Before(2)
    ));
}

#[test]
fn tab_move_mode_preserves_the_logical_active_tab() {
    let mut ids = vec![1, 2, 3, 4];
    let active_index = move_item_by_id(&mut ids, 2, TabMoveDirection::Right, 2, true, |id| *id);
    assert_eq!(ids, vec![1, 3, 2, 4]);
    assert_eq!(active_index, Some(2));
}

#[test]
fn tab_reorder_moves_tabs_before_and_after_targets_in_both_directions() {
    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 3, TabDropPosition::Before(1), 2, |id| *id),
        Some(2)
    );
    assert_eq!(ids, vec![3, 1, 2, 4]);

    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 3, TabDropPosition::After(1), 2, |id| *id),
        Some(2)
    );
    assert_eq!(ids, vec![1, 3, 2, 4]);

    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 1, TabDropPosition::Before(3), 2, |id| *id),
        Some(0)
    );
    assert_eq!(ids, vec![2, 1, 3, 4]);

    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 1, TabDropPosition::After(3), 2, |id| *id),
        Some(0)
    );
    assert_eq!(ids, vec![2, 3, 1, 4]);
}

#[test]
fn tab_reorder_adjusts_the_target_index_after_removing_the_source() {
    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 1, TabDropPosition::After(3), 4, |id| *id),
        Some(3)
    );
    assert_eq!(ids, vec![2, 3, 1, 4]);

    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 4, TabDropPosition::Before(2), 1, |id| *id),
        Some(0)
    );
    assert_eq!(ids, vec![1, 4, 2, 3]);
}

#[test]
fn tab_reorder_preserves_the_active_tab_when_moving_active_or_inactive_tabs() {
    let mut ids = vec![1, 2, 3, 4];
    let active_index = reorder_items_by_id(&mut ids, 3, TabDropPosition::After(1), 3, |id| *id);
    assert_eq!(ids, vec![1, 3, 2, 4]);
    assert_eq!(active_index, Some(1));

    let mut ids = vec![1, 2, 3, 4];
    let active_index = reorder_items_by_id(&mut ids, 1, TabDropPosition::After(4), 3, |id| *id);
    assert_eq!(ids, vec![2, 3, 4, 1]);
    assert_eq!(active_index, Some(1));
}

#[test]
fn tab_reorder_ignores_same_tab_invalid_target_empty_and_outside_drops() {
    let mut ids = vec![1, 2, 3];
    assert_eq!(
        reorder_items_by_id(&mut ids, 2, TabDropPosition::Before(2), 1, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);

    assert_eq!(
        reorder_items_by_id(&mut ids, 2, TabDropPosition::After(99), 1, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);

    assert_eq!(
        reorder_items_by_id(&mut ids, 99, TabDropPosition::After(1), 1, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);

    assert_eq!(
        reorder_items_by_id(
            &mut Vec::<u64>::new(),
            1,
            TabDropPosition::After(2),
            1,
            |id| *id
        ),
        None
    );
    assert_eq!(
        reorder_items_by_id(&mut ids, 2, TabDropPosition::Outside, 1, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);
}

#[test]
fn overflow_selection_side_distinguishes_left_and_right_entries() {
    assert_eq!(tab_overflow_selection_side(1, 4), Some(false));
    assert_eq!(tab_overflow_selection_side(7, 4), Some(true));
    assert_eq!(tab_overflow_selection_side(4, 4), None);
}
