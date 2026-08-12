use super::*;

#[test]
fn keyboard_confirmation_actions_are_explicit() {
    assert_eq!(
        close_confirmation_action("enter"),
        CloseConfirmationAction::Confirm
    );
    assert_eq!(
        close_confirmation_action("escape"),
        CloseConfirmationAction::Dismiss
    );
    assert_eq!(
        close_confirmation_action("tab"),
        CloseConfirmationAction::Ignore
    );
}

#[test]
fn confirmation_targets_only_its_recorded_tab_id() {
    let confirmation = CloseTabConfirmation { tab_id: 42 };
    assert!(close_confirmation_targets_tab(&confirmation, 42));
    assert!(!close_confirmation_targets_tab(&confirmation, 7));
}
