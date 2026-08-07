use super::*;

#[test]
fn minimized_shelf_capacity_fits_only_complete_entries() {
    assert_eq!(minimized_pane_capacity(px(0.), 4), 1);
    assert_eq!(minimized_pane_capacity(px(180.), 4), 1);
    assert_eq!(minimized_pane_capacity(px(363.), 4), 1);
    assert_eq!(minimized_pane_capacity(px(364.), 4), 2);
    assert_eq!(minimized_pane_capacity(px(1000.), 4), 4);
    assert_eq!(minimized_pane_capacity(px(1000.), 0), 0);
}

#[test]
fn minimized_shelf_keeps_selection_in_a_full_visible_page() {
    assert_eq!(visible_minimized_pane_range(5, 0, 3), 0..3);
    assert_eq!(visible_minimized_pane_range(5, 2, 3), 0..3);
    assert_eq!(visible_minimized_pane_range(5, 3, 3), 2..5);
    assert_eq!(visible_minimized_pane_range(5, 4, 3), 2..5);
}

#[test]
fn minimized_shelf_resolves_metadata_only_for_visible_entries() {
    let mut resolved = Vec::new();
    let entries = resolve_visible_minimized_panes(63, 62, 3, |index| {
        resolved.push(index);
        Some(index)
    });

    assert_eq!(entries, [60, 61, 62]);
    assert_eq!(resolved, [60, 61, 62]);
}
