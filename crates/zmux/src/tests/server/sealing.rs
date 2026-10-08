use super::*;

fn recipients(name: &str) -> Vec<String> {
    vec![name.to_owned()]
}

fn all_running(_: u32) -> bool {
    true
}

#[test]
fn startup_recipients_are_trusted_only_until_a_client_configures() {
    let mut grants = RecipientGrants::starting_with(Some(recipients("startup")));
    assert_eq!(
        grants.for_session(Some(7), None),
        Some(recipients("startup"))
    );
    assert_eq!(grants.for_session(None, None), Some(recipients("startup")));

    // Unvouched-for, a Configure grants nothing, but the store it describes is
    // no longer the one the daemon was started with.
    grants.configured(None, &recipients("anyone"), all_running);
    assert_eq!(grants.for_session(Some(7), None), None);
    assert_eq!(grants.for_session(None, None), None);
}

#[test]
fn a_peer_is_granted_only_what_it_or_the_sessions_owner_configured() {
    let mut grants = RecipientGrants::default();
    grants.configured(Some(7), &recipients("owner"), all_running);
    grants.configured(Some(8), &recipients("other"), all_running);
    assert_eq!(grants.for_session(Some(7), None), Some(recipients("owner")));
    assert_eq!(
        grants.for_session(Some(8), Some(7)),
        Some(recipients("other"))
    );
    assert_eq!(grants.for_session(Some(9), None), None);
    // `zmux share` configures nothing, so the owner's choice stands in.
    assert_eq!(
        grants.for_session(Some(9), Some(7)),
        Some(recipients("owner"))
    );

    // Configuring memory retention says nothing about recipients to seal to.
    grants.configured(Some(7), &[], all_running);
    assert_eq!(grants.for_session(Some(7), None), None);
}

#[test]
fn a_process_that_exited_leaves_nothing_for_its_id_to_inherit() {
    let mut grants = RecipientGrants::default();
    grants.configured(Some(7), &recipients("exited"), all_running);
    grants.configured(Some(8), &recipients("running"), |process| process != 7);
    assert_eq!(grants.for_session(Some(7), None), None);
    assert_eq!(
        grants.for_session(Some(8), None),
        Some(recipients("running"))
    );
}

#[test]
fn grants_are_bounded_oldest_first() {
    let mut grants = RecipientGrants::default();
    for process in 0..=MAX_GRANTS as u32 {
        grants.configured(Some(process), &recipients("r"), all_running);
    }
    assert_eq!(grants.by_process.len(), MAX_GRANTS);
    assert_eq!(grants.for_session(Some(0), None), None);
    assert_eq!(
        grants.for_session(Some(MAX_GRANTS as u32), None),
        Some(recipients("r"))
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
mod sessions {
    use super::*;
    use crate::server::secret_check::tests::fixtures::*;

    /// A reload of the window's configuration re-sends its recipients, and
    /// anything holding the control token can both edit that file and ask for
    /// the reload; so configuring pins, but never re-pins.
    #[test]
    fn configuring_pins_only_unpinned_sessions_the_peer_controls() {
        let (daemon, _directory) = test_daemon();
        let mut sessions = vec![
            protected_session(1, Some(7)),
            protected_session(2, Some(8)),
            protected_session(3, Some(7)),
        ];
        sessions[1].sealed_to = Some(recipients("owner of 2"));
        sessions[2].sealed_to = Some(recipients("owner of 3"));

        configure_seals(&daemon, &mut sessions, Some(7), &recipients("seven"));
        assert_eq!(sessions[0].sealed_to, Some(recipients("seven")));
        assert_eq!(sessions[1].sealed_to, Some(recipients("owner of 2")));
        assert_eq!(sessions[2].sealed_to, Some(recipients("owner of 3")));

        // Somebody who owns none of them changes none of them.
        let mut unpinned = vec![protected_session(4, Some(7))];
        configure_seals(&daemon, &mut unpinned, Some(9), &recipients("stranger"));
        assert_eq!(unpinned[0].sealed_to, None);
    }

    #[cfg(feature = "session-persistence")]
    #[test]
    fn a_protected_session_nobody_vouched_for_configured_is_withheld() {
        use crate::persistence::Seal;
        let (daemon, _directory) = test_daemon();
        // A process that is running, since grants of exited ones are dropped.
        let window = std::process::id();
        let mut session = protected_session(1, Some(window));
        assert_eq!(seal_for(&session), Seal::Withheld);

        protected_by(&daemon, &mut session, Some(window));
        assert_eq!(seal_for(&session), Seal::Withheld);

        lock_grants(&daemon).configured(Some(window), &recipients("window"), all_running);
        protected_by(&daemon, &mut session, Some(window));
        assert_eq!(seal_for(&session), Seal::Recipients(recipients("window")));

        session.authentication = None;
        assert_eq!(seal_for(&session), Seal::Store);
    }

    #[test]
    fn an_older_handover_pins_its_protected_sessions_to_its_store_once() {
        let mut sessions = vec![protected_session(1, Some(7))];
        let grants = adopted_grants(None, &mut sessions, Some(&recipients("store")));
        assert_eq!(sessions[0].sealed_to, Some(recipients("store")));
        assert_eq!(grants.for_session(Some(9), None), Some(recipients("store")));

        // A current one is taken as it is, unpinned sessions included.
        let mut sessions = vec![protected_session(1, Some(7))];
        let carried = RecipientGrants::default();
        let grants = adopted_grants(Some(carried), &mut sessions, Some(&recipients("store")));
        assert_eq!(sessions[0].sealed_to, None);
        assert_eq!(grants.for_session(Some(9), None), None);
    }
}
