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

fn session_owned_by(owner: u32, protected: bool) -> Session {
    Session {
        id: 1,
        summary: BackgroundSessionSummary {
            id: 1,
            title: "shell".to_owned(),
            authentication_required: protected,
            active_pane: 1,
            layout: BackgroundPaneLayout::Pane { pane_id: 1 },
            panes: Vec::new(),
            held: false,
            scoped_to: None,
        },
        state: serde_json::Value::Null,
        authentication: protected.then(|| SessionAuthentication::create("correct horse").unwrap()),
        failed_authentications: 0,
        refuse_until: None,
        panes: Vec::new(),
        keep: true,
        offered: false,
        owner: Some(owner),
    }
}

#[test]
fn a_protected_sessions_owner_has_to_be_vouched_for_not_claimed() {
    // The hole this closes: where the platform reports no peer credentials, the
    // owner used to be whatever the envelope said it was. Anyone able to read
    // the endpoint token could then name the real owner and detach, kill or
    // rescope a protected session without ever presenting its secret.
    let session = session_owned_by(4321, true);

    assert!(
        session_control_authorized(&session, Some(4321)),
        "the owner, vouched for, must still be authorized"
    );
    assert!(
        !session_control_authorized(&session, Some(9999)),
        "another process must not be authorized"
    );
    assert!(
        !session_control_authorized(&session, None),
        "an unvouched-for peer must not be authorized, whatever it claims"
    );
    // Holder-only controls — resize, palette — are stricter still: not even the
    // owner qualifies, and an unattested peer never does.
    assert!(!protected_holder_authorized(&session, Some(4321)));
    assert!(!protected_holder_authorized(&session, None));

    // A session with no secret is a different question: any same-user process
    // can attach one for itself, so confirming which process is asking would
    // protect nothing, and controls stay open as they always were.
    let open = session_owned_by(4321, false);
    assert!(session_control_authorized(&open, None));
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
