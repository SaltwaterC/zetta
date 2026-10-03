use super::*;

use std::time::Duration;

/// Whether a thread entering the gate for `session_id` is still waiting after
/// a moment — long enough for it to have got in, were it free to.
fn entry_waits(gate: &Arc<VerificationGate>, session_id: u64) -> bool {
    let (entered, entry) = std::sync::mpsc::channel();
    let waiter = Arc::clone(gate);
    thread::spawn(move || {
        let _ticket = waiter.enter(session_id);
        let _ = entered.send(());
    });
    entry.recv_timeout(Duration::from_millis(200)).is_err()
}

#[test]
fn one_check_per_session_is_admitted_at_a_time() {
    let gate = Arc::new(VerificationGate::default());
    let ticket = gate.enter(1);

    assert!(
        entry_waits(&gate, 1),
        "a second check of the same session must wait for the first's outcome"
    );
    assert!(
        !entry_waits(&gate, 2),
        "another session's check must not wait behind this one"
    );
    drop(ticket);
}

#[test]
fn checks_across_sessions_are_bounded() {
    let gate = Arc::new(VerificationGate::default());
    let tickets: Vec<_> = (0..MAX_CONCURRENT_CHECKS as u64)
        .map(|session_id| gate.enter(session_id))
        .collect();

    assert!(
        entry_waits(&gate, u64::MAX),
        "a check past the bound must wait for a slot"
    );
    drop(tickets);
    assert!(!entry_waits(&gate, u64::MAX));
}

#[test]
fn a_ticket_is_given_back_when_its_check_panics() {
    let gate = Arc::new(VerificationGate::default());
    let panicking = Arc::clone(&gate);
    let _ = thread::spawn(move || {
        let _ticket = panicking.enter(1);
        panic!("Argon2 failed");
    })
    .join();

    assert!(
        !entry_waits(&gate, 1),
        "a panic mid-check must not lock the session's secret out"
    );
}

#[test]
fn a_proof_does_not_survive_its_verifier_being_replaced() {
    let original = SessionAuthentication::create("correct horse").unwrap();
    let proof = SecretProof(vec![original.verify("correct horse").unwrap()]);
    assert!(proof.authorizes(&original));
    assert!(proof.authorizes(&original.clone()));

    // Reprotected with the very same secret is still a replacement: the proof
    // was of the old verifier, and honouring it would be a cached answer.
    let replacement = SessionAuthentication::create("correct horse").unwrap();
    assert!(!proof.authorizes(&replacement));
    assert!(!SecretProof::NONE.authorizes(&original));
}

/// A daemon holding protected sessions, shared with the attach tests, which
/// check the same secret through `authorize_attach`.
#[cfg(all(unix, not(target_os = "macos")))]
pub(in crate::server) mod fixtures {
    use super::*;

    pub(in crate::server) const SECRET: &str = "correct horse";

    pub(in crate::server) fn test_daemon() -> (Arc<Daemon>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let image_directory = directory.path().join("clipboard");
        create_private_dir(&image_directory).unwrap();
        let daemon = Arc::new(Daemon::new(
            directory.path(),
            Retention::None,
            image_directory,
            #[cfg(feature = "session-persistence")]
            None,
            1,
            1,
            -1,
        ));
        (daemon, directory)
    }

    pub(in crate::server) fn protected_session(id: u64, owner: Option<u32>) -> Session {
        Session {
            id,
            summary: BackgroundSessionSummary {
                id,
                title: "shell".to_owned(),
                authentication_required: true,
                active_pane: 1,
                layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
                held: false,
                scoped_to: None,
                key_envelope: None,
            },
            state: serde_json::Value::Null,
            shared_state: None,
            authentication: Some(SessionAuthentication::create(SECRET).unwrap()),
            key_envelope: None,
            failed_authentications: 0,
            refuse_until: None,
            panes: Vec::new(),
            keep: true,
            offered: false,
            owner,
        }
    }

    pub(in crate::server) fn daemon_with(
        sessions: Vec<Session>,
    ) -> (Arc<Daemon>, tempfile::TempDir) {
        let (daemon, directory) = test_daemon();
        *daemon.sessions.lock().unwrap() = sessions;
        (daemon, directory)
    }

    pub(in crate::server) fn failures(daemon: &Daemon, session_id: u64) -> u32 {
        daemon
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|session| session.id == session_id)
            .unwrap()
            .failed_authentications
    }

    /// Whether a secret check is between entering the gate and leaving it.
    pub(in crate::server) fn check_in_flight(daemon: &Daemon) -> bool {
        !daemon
            .verification_gate
            .in_flight
            .lock()
            .unwrap()
            .is_empty()
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod with_daemon {
    use super::{fixtures::*, *};

    #[test]
    fn the_sessions_lock_is_free_while_a_secret_is_checked() {
        let (daemon, _directory) = daemon_with(vec![protected_session(1, None)]);
        // How long one check takes in this build, so the assertion below scales
        // with it rather than guessing a constant.
        let started = Instant::now();
        let _ = SessionAuthentication::create(SECRET)
            .unwrap()
            .verify(SECRET);
        let one_check = started.elapsed();

        let checker = {
            let daemon = Arc::clone(&daemon);
            thread::spawn(move || check_session_secret(&daemon, 1, SECRET))
        };
        // What an unrelated pane's drain would see: the longest it had to wait
        // for the lock at any point while the check ran.
        let mut longest_wait = Duration::ZERO;
        while !checker.is_finished() {
            let started = Instant::now();
            drop(daemon.sessions.lock().unwrap());
            longest_wait = longest_wait.max(started.elapsed());
            thread::sleep(Duration::from_millis(1));
        }

        assert!(matches!(checker.join().unwrap(), SecretCheck::Verified(_)));
        assert!(
            longest_wait < one_check / 4,
            "the sessions lock was held for {longest_wait:?} of a {one_check:?} check"
        );
    }

    #[test]
    fn concurrent_wrong_secrets_meet_the_refusal_window_one_at_a_time() {
        let (daemon, _directory) = daemon_with(vec![protected_session(1, None)]);
        let attempts: Vec<_> = (0..4)
            .map(|_| {
                let daemon = Arc::clone(&daemon);
                thread::spawn(move || check_session_secret(&daemon, 1, "wrong"))
            })
            .collect();
        for attempt in attempts {
            assert!(matches!(attempt.join().unwrap(), SecretCheck::Failed));
        }

        // Only the first was evaluated. The rest queued behind it and then met
        // the window it opened — the guessing rate the backoff promises. Racing
        // them all through Argon2 at once would have evaluated, and counted,
        // four guesses.
        assert_eq!(failures(&daemon, 1), 1);
    }

    #[test]
    fn a_right_secret_inside_the_refusal_window_is_refused_like_a_wrong_one() {
        let (daemon, _directory) = daemon_with(vec![protected_session(1, None)]);
        assert!(matches!(
            check_session_secret(&daemon, 1, "wrong"),
            SecretCheck::Failed
        ));
        assert!(
            matches!(
                check_session_secret(&daemon, 1, SECRET),
                SecretCheck::Failed
            ),
            "the window must not be probeable with the right secret"
        );

        daemon.sessions.lock().unwrap()[0].refuse_until = None;
        assert!(matches!(
            check_session_secret(&daemon, 1, SECRET),
            SecretCheck::Verified(_)
        ));
        assert_eq!(failures(&daemon, 1), 0, "a success resets the count");
    }

    #[test]
    fn a_session_that_has_gone_has_nothing_to_check() {
        let (daemon, _directory) = daemon_with(Vec::new());
        assert!(matches!(
            check_session_secret(&daemon, 1, SECRET),
            SecretCheck::NotApplicable
        ));
    }

    #[test]
    fn a_proof_is_skipped_for_sessions_the_peer_controls_by_identity() {
        let (daemon, _directory) = daemon_with(vec![
            protected_session(1, Some(4321)),
            protected_session(2, None),
        ]);
        let proof = prove_session_secret(&daemon, Some(4321), Some("wrong"), |_| true);

        // The owner's own session was never checked, so a wrong secret cost it
        // nothing; the other session was, and was charged for it.
        assert!(proof.0.is_empty());
        assert_eq!(failures(&daemon, 1), 0);
        assert_eq!(failures(&daemon, 2), 1);
    }
}
