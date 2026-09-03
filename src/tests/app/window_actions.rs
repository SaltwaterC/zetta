use super::*;

#[test]
fn application_menu_navigation_wraps_in_both_directions() {
    assert_eq!(
        adjacent_application_menu_index(2, 0, ApplicationMenuDirection::Left),
        1
    );
    assert_eq!(
        adjacent_application_menu_index(2, 1, ApplicationMenuDirection::Right),
        0
    );
    assert_eq!(
        adjacent_application_menu_index(3, 1, ApplicationMenuDirection::Left),
        0
    );
    assert_eq!(
        adjacent_application_menu_index(3, 1, ApplicationMenuDirection::Right),
        2
    );
}
