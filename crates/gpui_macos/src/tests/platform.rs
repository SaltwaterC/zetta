use super::*;
use std::cell::Cell;

#[test]
fn input_context_gate_stays_suspended_until_the_app_and_input_source_are_ready() {
    let gate = InputContextGate::new(true);
    assert!(gate.is_enabled());

    gate.suspend();
    assert!(!gate.is_enabled());

    let source_was_checked = Cell::new(false);
    gate.resume_if_active(false, || {
        source_was_checked.set(true);
        true
    });
    assert!(!source_was_checked.get());
    assert!(!gate.is_enabled());

    gate.resume_if_active(true, || false);
    assert!(!gate.is_enabled());

    gate.resume_if_active(true, || true);
    assert!(gate.is_enabled());
}

#[test]
fn input_context_gate_can_start_suspended_when_the_input_source_is_unavailable() {
    let gate = InputContextGate::new(false);

    assert!(!gate.is_enabled());
}
