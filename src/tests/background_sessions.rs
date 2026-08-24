use super::*;

fn catalog(process_id: u32, runner_id: u64, session_ids: &[u64]) -> BackgroundSessionCatalog {
    scoped_catalog(process_id, runner_id, session_ids, None)
}

/// A catalog whose sessions are all scoped to `scoped_to`.
fn scoped_catalog(
    process_id: u32,
    runner_id: u64,
    session_ids: &[u64],
    scoped_to: Option<u32>,
) -> BackgroundSessionCatalog {
    BackgroundSessionCatalog {
        version: 3,
        process_id,
        runner_id,
        sessions: session_ids
            .iter()
            .map(|id| BackgroundSessionSummary {
                id: *id,
                title: format!("Session {id}"),
                authentication_required: false,
                active_pane: 1,
                layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
                held: false,
                scoped_to,
                key_envelope: None,
            })
            .collect(),
    }
}

#[test]
fn mux_held_sessions_skip_zetta_process_catalogs() {
    let catalogs = [
        // The multiplexer: a process with no Zetta control endpoint.
        catalog(1000, 7, &[1, 2]),
        // A Zetta process that kept a session in memory because the mux was
        // unreachable: it publishes a catalog too, but it has a control
        // endpoint, so its sessions are not the daemon's to attach.
        catalog(2000, 8, &[3]),
        catalog(3000, 9, &[4]),
    ];
    let is_zetta = |process_id| matches!(process_id, 2000 | 3000);

    let held = multiplexer_held_catalog_sessions(&catalogs, is_zetta, 4242)
        .map(|(catalog, session)| (catalog.runner_id, session.id))
        .collect::<Vec<_>>();

    assert_eq!(held, vec![(7, 1), (7, 2)]);
}

#[test]
fn mux_held_sessions_attach_keeps_the_runners_distinct() {
    let catalogs = [catalog(1000, 7, &[1]), catalog(1001, 8, &[2])];

    let held = multiplexer_held_catalog_sessions(&catalogs, |_| false, 4242)
        .map(|(catalog, session)| (catalog.runner_id, session.id))
        .collect::<Vec<_>>();

    assert_eq!(held, vec![(7, 1), (8, 2)]);
}

/// Backgrounding a tab keeps its session to the window that did it.
///
/// The catalog is one file that every Zetta process reads, so the entries have
/// to say whose they are: without this a second process listed — and offered to
/// attach — a session the multiplexer would refuse it, which is the behaviour
/// backgrounding had before the multiplexer held these sessions at all.
#[test]
fn mux_held_sessions_skip_another_processes_scoped_sessions() {
    let catalogs = [
        scoped_catalog(1000, 7, &[1], Some(4242)),
        scoped_catalog(1000, 8, &[2], Some(9999)),
        // Shared: nobody's in particular, so everybody's.
        scoped_catalog(1000, 9, &[3], None),
    ];

    let held = multiplexer_held_catalog_sessions(&catalogs, |_| false, 4242)
        .map(|(_, session)| session.id)
        .collect::<Vec<_>>();

    assert_eq!(held, vec![1, 3]);
}

#[test]
fn reconnects_most_recently_detached_session_first() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach("build", None);
    runner.detach("server", None);

    assert_eq!(runner.len(), 2);
    assert_eq!(runner.reconnect_at(runner.len() - 1), Some("server"));
    assert_eq!(runner.reconnect_at(runner.len() - 1), Some("build"));
    assert_eq!(runner.reconnect_at(0), None);
}

#[test]
fn reconnects_a_selected_session_without_reordering_the_others() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach("build", None);
    runner.detach("server", None);
    runner.detach("editor", None);

    assert_eq!(runner.reconnect_at(1), Some("server"));
    assert_eq!(
        runner.iter().copied().collect::<Vec<_>>(),
        ["build", "editor"]
    );
    assert_eq!(runner.reconnect_at(2), None);
}

#[test]
fn a_wrong_secret_opens_a_refusal_window_scoped_to_that_session() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach(
        "privileged",
        Some(SessionAuthentication::create("secret").unwrap()),
    );
    runner.detach(
        "other",
        Some(SessionAuthentication::create("secret").unwrap()),
    );

    assert!(!runner.authentication_is_refused_at(0));
    runner.record_failed_authentication_at(0);
    assert!(runner.authentication_is_refused_at(0));
    // Backoff is per session: guessing at one must not lock the user out of
    // another, and must not be shared state an attacker can pile onto.
    assert!(!runner.authentication_is_refused_at(1));

    // A correct secret clears the penalty.
    runner.clear_failed_authentications_at(0);
    assert!(!runner.authentication_is_refused_at(0));

    // An out-of-range index is inert rather than panicking.
    runner.record_failed_authentication_at(99);
    assert!(!runner.authentication_is_refused_at(99));
}

#[test]
fn authentication_is_attached_only_to_the_selected_session() {
    let mut runner = BackgroundSessionRunner::default();
    runner.detach("ordinary", None);
    runner.detach(
        "sensitive",
        Some(SessionAuthentication::create("secret").unwrap()),
    );

    assert!(runner.authentication_at(0).is_none());
    assert!(
        runner
            .authentication_at(1)
            .unwrap()
            .verify("secret")
            .is_some()
    );
    assert_eq!(
        runner.iter().copied().collect::<Vec<_>>(),
        ["ordinary", "sensitive"]
    );
}

#[test]
fn unexpected_exit_metadata_is_sanitized_and_actionable() {
    let event = TerminalExited {
        exit_code: Some(1),
        source: TerminalExitSource::Child,
        child_pid: Some(77),
        input_sent: true,
        foreground_is_shell: Some(false),
        foreground_command: Some("htop".to_owned()),
    };
    let exit = background_pane_exit_from_terminal(&event).unwrap();
    assert_eq!(exit.reason, BackgroundPaneExitReason::ForegroundCommand);
    assert_eq!(exit.foreground_command.as_deref(), Some("htop"));
    assert!(exit.reason_text().contains("htop"));
    assert!(exit.reason_text().contains("child PID 77"));

    let unsafe_event = TerminalExited {
        foreground_command: Some("sh -c secret=value".to_owned()),
        ..event
    };
    let sanitized = background_pane_exit_from_terminal(&unsafe_event).unwrap();
    assert_eq!(sanitized.foreground_command, None);
    assert!(!sanitized.reason_text().contains("secret=value"));

    let unknown_status = TerminalExited {
        exit_code: None,
        ..unsafe_event
    };
    assert_eq!(
        background_pane_exit_from_terminal(&unknown_status)
            .unwrap()
            .reason,
        BackgroundPaneExitReason::StatusUnavailable
    );
}

#[test]
fn a_clean_exit_is_never_an_unexpected_exit() {
    let event = TerminalExited {
        exit_code: Some(0),
        source: TerminalExitSource::Child,
        child_pid: Some(77),
        input_sent: true,
        foreground_is_shell: Some(false),
        foreground_command: Some("htop".to_owned()),
    };
    assert!(
        background_pane_exit_from_terminal(&event).is_none(),
        "a process exiting with status 0 closed the pane itself; nothing to retain"
    );

    let clean_exit_before_input = TerminalExited {
        input_sent: false,
        ..event
    };
    assert!(
        background_pane_exit_from_terminal(&clean_exit_before_input).is_none(),
        "a clean exit is an ordinary close even before any input was sent"
    );
}
