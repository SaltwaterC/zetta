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

fn size(columns: u16, lines: u16) -> TerminalSize {
    TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    }
}

#[test]
fn shared_clients_are_arbitrated_down_to_the_smallest_of_them() {
    // The pane has to fit inside every viewer, so each axis is taken
    // independently: a viewer that is wider but shorter constrains the height
    // only, and taking one viewer's size wholesale would overflow the other.
    assert_eq!(
        smallest_size([(120, 30), (80, 50)].into_iter(), size(200, 60)),
        (80, 30)
    );
    // One viewer: its size, not the size the daemon last applied.
    assert_eq!(
        smallest_size([(90, 20)].into_iter(), size(200, 60)),
        (90, 20)
    );
    // None: whatever the pane is already running at. A shared set can be empty
    // between a revoke being committed and the first viewer joining.
    assert_eq!(smallest_size([].into_iter(), size(200, 60)), (200, 60));
}

#[test]
fn a_shared_pane_with_no_clients_left_is_unheld() {
    // Only the explicit "a client left" path used to do this, so a viewer
    // dropped for being unwritable — wedged past the relay's write timeout —
    // left the pane shared with nobody: still drained, but never exclusively
    // attachable again, and never pruned, because both need `Attachment::None`.
    let mut attachment = Attachment::Shared(Vec::new());
    collapse_empty_shared(&mut attachment);
    assert!(attachment.is_none());

    // A pane nobody was ever sharing is left exactly as it is.
    let mut exclusive = Attachment::Exclusive(7);
    collapse_empty_shared(&mut exclusive);
    assert!(matches!(exclusive, Attachment::Exclusive(7)));
}

#[test]
fn an_empty_shared_set_reports_no_input() {
    // The attribution a pane's exit carries. With no viewers there is nobody to
    // have typed, and an exclusive holder's own keystrokes are the truth for it
    // rather than something the daemon can see.
    assert!(!shared_input_sent(&Attachment::Shared(Vec::new())));
    assert!(!shared_input_sent(&Attachment::Exclusive(7)));
    assert!(!shared_input_sent(&Attachment::None));
    assert!(!shared_input_sent(&Attachment::Revoking { holder: 7 }));
}
