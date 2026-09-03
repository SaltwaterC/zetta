use super::*;

#[test]
fn process_quits_only_without_windows_or_dormant_session_runners() {
    assert!(should_quit_after_window_closed(0, 0));
    assert!(!should_quit_after_window_closed(0, 1));
    assert!(!should_quit_after_window_closed(1, 0));
}

#[test]
fn live_windows_are_selected_before_dormant_sessions() {
    let window_id = WindowId::from(7);

    assert_eq!(
        select_window_open_target(Some(window_id), true),
        WindowOpenTarget::Existing(window_id)
    );
    assert_eq!(
        select_window_open_target(None, true),
        WindowOpenTarget::Dormant
    );
}

#[test]
fn window_open_selection_falls_back_to_a_fresh_window_without_reopen_state() {
    assert_eq!(
        select_window_open_target(None, false),
        WindowOpenTarget::Fresh
    );
}

#[test]
fn application_shutdown_is_managed_by_the_session_runner() {
    assert_eq!(zetta_quit_mode(), gpui::QuitMode::Explicit);
}
