use super::*;

#[test]
fn tab_move_context_menu_is_only_available_with_two_tabs() {
    assert!(!tab_move_menu_entry_available(0));
    assert!(!tab_move_menu_entry_available(1));
    assert!(tab_move_menu_entry_available(2));
}

#[test]
fn active_tab_shape_requires_compact_mode_and_selection() {
    assert!(active_tab_shape_visible(true, true));
    assert!(!active_tab_shape_visible(false, true));
    assert!(!active_tab_shape_visible(true, false));
}
