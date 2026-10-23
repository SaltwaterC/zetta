use super::*;

#[test]
fn resumed_identifiers_never_collide_with_what_was_adopted() {
    // The failure this guards against: a handover claiming the next session is
    // 1 while session 1 is right there in it. The next spawn then reuses the
    // identifier, and the multiplexer holds two sessions no client can tell
    // apart — they list identically, and attaching one is a coin toss.
    let (session, pane) = next_ids_after(&[1], &[1], 1, 1);
    assert_eq!(session, 2);
    assert_eq!(pane, 2);

    // A handover that is ahead of what it carries is still respected: it knows
    // about identifiers that were issued and then released.
    let (session, pane) = next_ids_after(&[1], &[1], 9, 7);
    assert_eq!(session, 9);
    assert_eq!(pane, 7);

    // Several sessions, and panes numbered independently of them.
    let (session, pane) = next_ids_after(&[3, 1, 2], &[5, 9, 2], 0, 0);
    assert_eq!(session, 4);
    assert_eq!(pane, 10);

    // Nothing adopted: start where the handover says.
    assert_eq!(next_ids_after(&[], &[], 4, 6), (4, 6));
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[test]
fn an_owner_is_adopted_as_the_previous_image_recorded_it() {
    use crate::upgrade::{HANDOVER_VERSION, PID_OWNER_HANDOVER_VERSION};

    let this = ProcessIdentity::of(std::process::id());
    let started = this.started.expect("this platform reports start times");

    // Taken as written, including a start time that no longer matches: the
    // owner it named has gone, whoever holds its PID now.
    let gone = adopt_owner(Some(this.pid), Some(started + 1), HANDOVER_VERSION).unwrap();
    assert!(!gone.is(this.pid));
    assert_eq!(adopt_owner(None, None, HANDOVER_VERSION), None);

    // An image from before start times sent the PID alone; the process holding
    // it now is what that image would itself have accepted.
    let previous = adopt_owner(Some(this.pid), None, PID_OWNER_HANDOVER_VERSION).unwrap();
    assert_eq!(previous, this);
}
