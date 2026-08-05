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

#[test]
fn input_context_resume_requests_are_coalesced() {
    let gate = InputContextGate::new(false);

    assert!(gate.request_resume());
    assert!(!gate.request_resume());

    gate.finish_resume();
    assert!(gate.request_resume());
}

#[test]
fn input_context_resume_retries_are_bounded() {
    let gate = InputContextGate::new(false);
    assert!(gate.request_resume());

    for _ in 0..MAX_INPUT_CONTEXT_RESUME_RETRIES {
        assert!(gate.should_retry_resume());
    }
    assert!(!gate.should_retry_resume());
}

#[test]
fn suspending_input_context_cancels_pending_resume() {
    let gate = InputContextGate::new(false);

    assert!(gate.request_resume());
    gate.suspend();

    assert!(gate.request_resume());
}
