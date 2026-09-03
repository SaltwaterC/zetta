use super::*;

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
            key_envelope: None,
        },
        state: serde_json::Value::Null,
        authentication: protected.then(|| SessionAuthentication::create("correct horse").unwrap()),
        key_envelope: None,
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
    let mut session = session_owned_by(4321, true);

    assert!(
        session_control_authorized(&mut session, Some(4321), None),
        "the owner, vouched for, must still be authorized"
    );
    assert!(
        !session_control_authorized(&mut session, Some(9999), None),
        "another process must not be authorized"
    );
    assert!(
        !session_control_authorized(&mut session, None, None),
        "an unvouched-for peer must not be authorized, whatever it claims"
    );
    // Holder-only controls — resize, palette — are stricter still: not even the
    // owner qualifies, and an unattested peer never does.
    assert!(!protected_holder_authorized(&session, Some(4321)));
    assert!(!protected_holder_authorized(&session, None));

    // A session with no secret is a different question: any same-user process
    // can attach one for itself, so confirming which process is asking would
    // protect nothing, and controls stay open as they always were.
    let mut open = session_owned_by(4321, false);
    assert!(session_control_authorized(&mut open, None, None));
}
