use super::*;

#[test]
fn pane_resize_mode_uses_a_twenty_pixel_mouse_gutter() {
    assert_eq!(PANE_RESIZE_GUTTER_SIZE, px(20.));
}

#[test]
fn pane_resize_menu_entry_requires_at_least_two_panes() {
    assert!(!pane_resize_menu_entry_available(0));
    assert!(!pane_resize_menu_entry_available(1));
    assert!(pane_resize_menu_entry_available(2));
    assert!(pane_resize_menu_entry_available(3));
}

#[test]
fn pane_move_menu_entry_requires_at_least_two_panes() {
    assert!(!pane_move_menu_entry_available(0));
    assert!(!pane_move_menu_entry_available(1));
    assert!(pane_move_menu_entry_available(2));
    assert!(pane_move_menu_entry_available(3));
}

#[test]
fn pane_window_edges_follow_split_direction() {
    let edges = PaneWindowEdges::all();
    assert!(!edges.with_bottom(false).bottom);

    let top = edges.first(SplitAxis::Horizontal);
    let bottom = edges.second(SplitAxis::Horizontal);
    assert!(top.left && top.right && !top.bottom);
    assert!(bottom.left && bottom.right && bottom.bottom);

    let left = edges.first(SplitAxis::Vertical);
    let right = edges.second(SplitAxis::Vertical);
    assert!(left.left && left.bottom && !left.right);
    assert!(!right.left && right.bottom && right.right);
}
