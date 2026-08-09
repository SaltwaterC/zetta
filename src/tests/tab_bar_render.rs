use super::*;

#[test]
fn tab_move_context_menu_is_only_available_with_two_tabs() {
    assert!(!tab_move_menu_entry_available(0));
    assert!(!tab_move_menu_entry_available(1));
    assert!(tab_move_menu_entry_available(2));
}
