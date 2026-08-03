use super::*;

fn raw_selection_serial(tracker: &SerialTracker) -> Option<u32> {
    tracker.selection_serial().map(SelectionSerial::as_raw)
}

#[test]
fn selection_serial_uses_observation_order_after_serial_wraparound() {
    let mut tracker = SerialTracker::new();
    tracker.update(SerialKind::KeyPress, u32::MAX);
    tracker.update(SerialKind::MousePress, 1);

    assert_eq!(raw_selection_serial(&tracker), Some(1));
}

#[test]
fn selection_serial_ignores_newer_unrelated_serial_kinds() {
    let mut tracker = SerialTracker::new();
    tracker.update(SerialKind::MousePress, 10);
    tracker.update(SerialKind::InputMethod, 20);
    tracker.update(SerialKind::DataDevice, 30);

    assert_eq!(raw_selection_serial(&tracker), Some(10));
}

#[test]
fn selection_serial_is_unavailable_without_an_input_press() {
    let mut tracker = SerialTracker::new();
    tracker.update(SerialKind::InputMethod, 20);
    tracker.update(SerialKind::MouseEnter, 30);
    tracker.update(SerialKind::DataDevice, 40);

    assert_eq!(raw_selection_serial(&tracker), None);
}

#[test]
fn zero_is_a_valid_selection_serial_after_rollover() {
    let mut tracker = SerialTracker::new();
    tracker.update(SerialKind::KeyPress, 0);

    assert_eq!(raw_selection_serial(&tracker), Some(0));
}
