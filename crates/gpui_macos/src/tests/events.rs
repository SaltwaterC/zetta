use super::*;

#[test]
fn mouse_down_modifiers_restore_shift_from_current_state() {
    let normalized = normalize_mouse_down_modifiers(
        Modifiers::default(),
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );

    assert!(normalized.shift);
}

#[test]
fn mouse_down_modifier_normalization_preserves_event_modifiers() {
    let event_modifiers = Modifiers {
        control: true,
        alt: true,
        shift: true,
        platform: true,
        function: true,
    };

    assert_eq!(
        normalize_mouse_down_modifiers(event_modifiers, Modifiers::default()),
        event_modifiers
    );
}

#[test]
fn mouse_down_modifier_normalization_does_not_add_shift_without_current_shift() {
    let normalized = normalize_mouse_down_modifiers(Modifiers::default(), Modifiers::default());

    assert!(!normalized.shift);
}
