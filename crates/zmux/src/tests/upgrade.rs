use super::*;
use std::os::fd::IntoRawFd as _;

fn handover() -> Handover {
    Handover {
        version: HANDOVER_VERSION,
        next_session_id: 5,
        next_pane_id: 9,
        sessions: vec![SessionHandover {
            id: 1,
            summary: crate::protocol::BackgroundSessionSummary {
                id: 1,
                title: "held".to_owned(),
                authentication_required: true,
                active_pane: 2,
                layout: crate::protocol::BackgroundPaneLayout::Pane { pane_id: 2 },
                panes: Vec::new(),
                held: false,
                scoped_to: None,
            },
            state: serde_json::json!({"tab": 1}),
            keep: true,
            offered: true,
            owner: None,
            verifier: Some("$argon2id$v=19$m=1,t=1,p=1$c2FsdA$aGFzaA".to_owned()),
            failed_authentications: 3,
            refuse_for: Some(Duration::from_secs(4)),
            panes: vec![PaneHandover {
                id: 2,
                descriptor: 7,
                child_pid: 1234,
                attachment: AttachmentHandover::Shared {
                    clients: vec![SharedClientHandover {
                        process_id: 4321,
                        columns: 100,
                        lines: 30,
                        input_sent: true,
                    }],
                },
                columns: 100,
                lines: 30,
                exited: false,
                exit_status: None,
                retained: b"output".to_vec(),
            }],
        }],
    }
}

#[test]
fn a_handover_round_trips_through_its_anonymous_file() {
    let file = write_handover(&handover()).unwrap();
    let restored = read_handover(file.into_raw_fd()).unwrap();

    assert_eq!(restored.next_session_id, 5);
    assert_eq!(restored.sessions[0].panes[0].descriptor, 7);
    assert_eq!(restored.sessions[0].panes[0].retained, b"output");
}

#[test]
fn a_protected_session_keeps_its_verifier_and_its_backoff() {
    let file = write_handover(&handover()).unwrap();
    let restored = read_handover(file.into_raw_fd()).unwrap();
    let session = &restored.sessions[0];

    assert!(session.verifier.is_some(), "protection must survive");
    // Dropping these would make `--upgrade` a way to clear a session's rate
    // limit, which is the thing the backoff exists to prevent.
    assert_eq!(session.failed_authentications, 3);
    assert_eq!(session.refuse_for, Some(Duration::from_secs(4)));
}

#[test]
fn a_session_offered_to_other_windows_is_still_offered_afterwards() {
    let file = write_handover(&handover()).unwrap();
    let restored = read_handover(file.into_raw_fd()).unwrap();

    // Independent of `keep`, and carried for the same reason: dropping it would
    // make `--upgrade` silently withdraw every session that was being shared,
    // and the windows showing them have no way to notice — they are not the ones
    // that would be refused, a window trying to join them is.
    assert!(restored.sessions[0].offered);
}

#[test]
fn a_panes_mode_and_size_survive_the_handover() {
    let file = write_handover(&handover()).unwrap();
    let restored = read_handover(file.into_raw_fd()).unwrap();
    let pane = &restored.sessions[0].panes[0];

    // Collapsing anything that was not an exclusive hold to "no holder" made a
    // shared pane come back exclusive-capable, so the first viewer to reconnect
    // was handed the descriptor and the rest were left reading a terminal
    // nobody was relaying.
    match &pane.attachment {
        AttachmentHandover::Shared { clients } => {
            assert_eq!(clients.len(), 1);
            assert_eq!(clients[0].process_id, 4321);
            assert_eq!((clients[0].columns, clients[0].lines), (100, 30));
            // A pane's exit reports which viewers typed into it; an upgrade must
            // not launder that away.
            assert!(clients[0].input_sent);
        }
        other => panic!("a shared pane came back as {other:?}"),
    }
    // Restarting from a default silently resized every adopted pane.
    assert_eq!((pane.columns, pane.lines), (100, 30));
}

#[test]
fn a_revoking_pane_is_still_being_handed_over_afterwards() {
    let mut mid_handover = handover();
    mid_handover.sessions[0].panes[0].attachment = AttachmentHandover::Revoking { holder: 99 };
    let file = write_handover(&mid_handover).unwrap();
    let restored = read_handover(file.into_raw_fd()).unwrap();

    // The holder still has the descriptor and its snapshot is still expected.
    // Forgetting that let the drain thread read a terminal the holder was also
    // reading, and made the holder's snapshot arrive at a pane that was no
    // longer being handed over — so the handover failed and left it inert.
    assert!(matches!(
        restored.sessions[0].panes[0].attachment,
        AttachmentHandover::Revoking { holder: 99 }
    ));
}

#[test]
fn an_already_exited_panes_status_survives_the_handover() {
    let mut ended = handover();
    ended.sessions[0].panes[0].exited = true;
    ended.sessions[0].panes[0].exit_status = Some(256);
    let file = write_handover(&ended).unwrap();
    let restored = read_handover(file.into_raw_fd()).unwrap();

    // Only the parent can observe a status, and it observes it once. A client
    // that asks the replacement what it missed has no other route back to it, so
    // dropping it here would turn a real exit code into "status unavailable".
    assert!(restored.sessions[0].panes[0].exited);
    assert_eq!(restored.sessions[0].panes[0].exit_status, Some(256));
}

#[test]
fn a_handover_from_an_unknown_version_is_refused() {
    let mut future = handover();
    future.version = HANDOVER_VERSION + 1;
    let file = write_handover(&future).unwrap();

    // Guessing at the layout of a session it is about to take responsibility
    // for is worse than refusing to start.
    assert!(read_handover(file.into_raw_fd()).is_err());
}

#[test]
fn the_handover_never_reaches_a_named_path() {
    // It carries session verifiers, which the security model says are never
    // written to the filesystem.
    let file = write_handover(&handover()).unwrap();
    let descriptor = file.into_raw_fd();
    let link = std::fs::read_link(format!("/proc/self/fd/{descriptor}"));

    if let Ok(link) = link {
        let shown = link.display().to_string();
        assert!(
            shown.contains("memfd:") || shown.contains("(deleted)"),
            "the handover is reachable at {shown}"
        );
    }
}

#[test]
fn descriptors_marked_to_survive_are_no_longer_close_on_exec() {
    let file = tempfile::tempfile().unwrap();
    // A descriptor left close-on-exec is silently gone after the exec, and the
    // first sign of it would be a session whose terminal stops responding.
    keep_across_exec(&file).unwrap();

    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, 0);
}

#[test]
fn a_replacement_that_cannot_take_over_is_detected_before_it_is_run() {
    // `execv` is irreversible: a daemon that skipped this check and executed a
    // replacement which then refused the handover would have destroyed its own
    // sessions to find out.
    assert!(!replacement_accepts_handover(std::path::Path::new("/bin/false")).unwrap());
    assert!(replacement_accepts_handover(std::path::Path::new("/bin/true")).unwrap());
}
