use super::*;

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
    collapse_empty_shared(&mut attachment, 0);
    assert!(attachment.is_none());

    // A pane nobody was ever sharing is left exactly as it is.
    let mut exclusive = Attachment::Exclusive(7);
    collapse_empty_shared(&mut exclusive, 0);
    assert!(matches!(exclusive, Attachment::Exclusive(7)));

    // A revoke handover owns the empty shared state until its waiter joins;
    // output and size eviction must not turn it back into an exclusive pane.
    let mut handover = Attachment::Shared(Vec::new());
    collapse_empty_shared(&mut handover, 1);
    assert!(matches!(handover, Attachment::Shared(clients) if clients.is_empty()));
}
