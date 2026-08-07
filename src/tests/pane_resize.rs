use super::*;

#[test]
fn pane_resize_combines_window_dimension_changes() {
    let mut resize = WindowResize::default();
    resize.add(SplitAxis::Vertical, 120.);
    resize.add(SplitAxis::Horizontal, 48.);
    resize.add(SplitAxis::Vertical, -8.);

    assert_eq!(resize.width_delta, 112.);
    assert_eq!(resize.height_delta, 48.);
}

#[test]
fn pane_resize_keeps_the_window_large_enough_for_client_controls() {
    assert_eq!(minimum_resized_window_extent(720., 100., px(520.)), 520.);
    assert_eq!(minimum_resized_window_extent(400., 100., px(520.)), 400.);
}

#[test]
fn pane_resize_arrows_follow_the_active_pane_edge() {
    let vertical = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    assert_eq!(
        pane_resize_cell_delta(&vertical, 1, SplitAxis::Vertical, -1),
        -1
    );
    assert_eq!(
        pane_resize_cell_delta(&vertical, 2, SplitAxis::Vertical, -1),
        1
    );

    let horizontal = PaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Pane(2)),
    };
    assert_eq!(
        pane_resize_cell_delta(&horizontal, 1, SplitAxis::Horizontal, -1),
        -1
    );
    assert_eq!(
        pane_resize_cell_delta(&horizontal, 2, SplitAxis::Horizontal, -1),
        1
    );
}

#[test]
fn held_pane_resize_arrows_repeat_on_both_axes() {
    let mut keys = PaneResizeKeys::default();

    assert!(keys.press(PaneResizeDirection::Down));
    assert_eq!(keys.len(), 1);
    assert_eq!(keys.delta(), (0, 1));
    assert!(keys.press(PaneResizeDirection::Right));
    assert_eq!(keys.len(), 2);
    assert_eq!(keys.delta(), (1, 1));

    keys.release(PaneResizeDirection::Right);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys.delta(), (0, 1));
    assert!(!keys.press(PaneResizeDirection::Down));
}

#[test]
fn opposite_held_pane_resize_arrows_cancel_each_other() {
    let mut keys = PaneResizeKeys::default();

    assert!(keys.press(PaneResizeDirection::Left));
    assert!(keys.press(PaneResizeDirection::Right));
    assert!(keys.press(PaneResizeDirection::Down));
    assert_eq!(keys.delta(), (0, 1));
}
