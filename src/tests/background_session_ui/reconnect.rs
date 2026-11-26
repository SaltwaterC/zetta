use super::*;
use std::sync::mpsc;

#[test]
fn a_cancelled_reconnect_reports_rejected_once() {
    let (sender, receiver) = mpsc::channel();
    {
        let _completion = ReconnectCompletion::new(sender);
    }

    assert_eq!(receiver.recv().unwrap(), ReconnectSessionResult::Rejected);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn a_reconnect_result_wins_over_the_cancellation_fallback() {
    let (sender, receiver) = mpsc::channel();
    let mut completion = ReconnectCompletion::new(sender);
    completion.send(ReconnectSessionResult::Reconnected);
    drop(completion);

    assert_eq!(
        receiver.recv().unwrap(),
        ReconnectSessionResult::Reconnected
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn a_reconnect_acknowledgement_waits_for_catalog_publication() {
    let timeout = std::time::Duration::from_secs(10);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    crate::background_sessions::after_pending_publications(move || {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(timeout).unwrap();
    });
    started_rx.recv_timeout(timeout).unwrap();

    let (sender, receiver) = mpsc::channel();
    ReconnectCompletion::new(sender).send_after_publication(ReconnectSessionResult::Reconnected);
    assert!(receiver.try_recv().is_err());
    release_tx.send(()).unwrap();
    assert_eq!(
        receiver.recv_timeout(timeout).unwrap(),
        ReconnectSessionResult::Reconnected
    );
    assert!(receiver.try_recv().is_err());
}

#[cfg(feature = "session-persistence")]
fn split_summary(first: u64, second: u64) -> BackgroundSessionSummary {
    let pane = |id: u64| zmux::protocol::BackgroundPaneSummary {
        id,
        label: String::new(),
        profile: String::new(),
        configured_command: "sh".to_owned(),
        application: "sh".to_owned(),
        foreground_command: None,
        terminal_title: None,
        working_directory: None,
        state: zmux::protocol::BackgroundPaneState::Running,
        exit: None,
    };
    BackgroundSessionSummary {
        id: 1,
        title: "zsh".to_owned(),
        authentication_required: false,
        active_pane: second,
        layout: zmux::protocol::BackgroundPaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: zmux::protocol::DEFAULT_BACKGROUND_PANE_SPLIT_RATIO,
            first: Box::new(zmux::protocol::BackgroundPaneLayout::Pane { pane_id: first }),
            second: Box::new(zmux::protocol::BackgroundPaneLayout::Pane { pane_id: second }),
        },
        panes: vec![pane(first), pane(second)],
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

/// The record behind "invalid type: null, expected struct TabState": written
/// for a tab no window ever published a layout for. It restores from the
/// summary instead of being refused.
#[cfg(feature = "session-persistence")]
#[test]
fn a_disk_record_without_tab_state_is_rebuilt_from_its_summary() {
    let summary = split_summary(3, 5);

    let state = restored_disk_tab_state(serde_json::Value::Null, None, &summary).unwrap();

    let mux_ids = state
        .panes
        .iter()
        .map(|pane| pane.mux_pane_id)
        .collect::<Vec<_>>();
    assert_eq!(mux_ids, vec![Some(3), Some(5)]);
    assert_eq!(state.active_pane, 5);
}

#[cfg(feature = "session-persistence")]
#[test]
fn a_headless_shared_record_is_rebuilt_from_its_canonical_summary() {
    // `zmux create` persists only the canonical shared state. Its summary is
    // the authority, not the catalog copy beside it.
    let stale = split_summary(3, 5);
    let canonical =
        zmux::messages::SharedSessionState::new(1, split_summary(7, 8), serde_json::Value::Null);

    let state = restored_disk_tab_state(serde_json::Value::Null, Some(&canonical), &stale).unwrap();

    let mux_ids = state
        .panes
        .iter()
        .map(|pane| pane.mux_pane_id)
        .collect::<Vec<_>>();
    assert_eq!(mux_ids, vec![Some(7), Some(8)]);
}

#[cfg(feature = "session-persistence")]
#[test]
fn a_disk_record_with_nothing_to_rebuild_from_is_still_refused() {
    let mut summary = split_summary(3, 5);
    summary.panes.clear();

    assert!(restored_disk_tab_state(serde_json::Value::Null, None, &summary).is_err());
}
