use super::*;

#[test]
fn settings_control_navigation_wraps_and_starts_at_the_expected_end() {
    assert_eq!(adjacent_settings_control_index(0, None, false), None);
    assert_eq!(adjacent_settings_control_index(3, Some(0), true), Some(2));
    assert_eq!(adjacent_settings_control_index(3, Some(2), false), Some(0));
    assert_eq!(adjacent_settings_control_index(3, None, false), Some(0));
    assert_eq!(adjacent_settings_control_index(3, None, true), Some(2));
}
