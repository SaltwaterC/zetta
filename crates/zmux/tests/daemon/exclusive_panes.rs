//! Explicit pane closure must remove the PTY rather than leave it available
//! for a later background-session restore.

use super::*;

#[test]
fn closed_panes_do_not_return_when_the_remaining_session_is_restored() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let mut panes = Vec::new();
    let mut session_id = None;
    for _ in 0..4 {
        let pane = client
            .spawn(spawn_request(session_id, "printf ready; exec sleep 120"))
            .unwrap();
        session_id = Some(pane.session_id);
        let descriptor = std::fs::File::from(pane.descriptor);
        read_until(&descriptor, "ready");
        panes.push((pane.pane_id, pane.child_pid, descriptor));
    }
    let session_id = session_id.unwrap();
    // Only the two side-by-side panes remain in the window. The closed panes
    // used to stay daemon-owned and be appended as extra horizontal splits.
    for (pane_id, _, _) in &panes[2..] {
        client.discard_pane(session_id, *pane_id).unwrap();
    }
    let mut saved = summary(session_id, panes[0].0);
    saved.panes.push(pane_summary(panes[1].0));
    saved.active_pane = panes[1].0;
    saved.layout = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 630,
        first: Box::new(BackgroundPaneLayout::Pane {
            pane_id: panes[0].0,
        }),
        second: Box::new(BackgroundPaneLayout::Pane {
            pane_id: panes[1].0,
        }),
    };
    client
        .detach(
            session_id,
            saved.clone(),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    let AttachOutcome::Attached { summary, .. } = client.attach(session_id, None, None).unwrap()
    else {
        panic!("a detached exclusive session must attach exclusively")
    };
    assert_eq!(summary.panes, saved.panes);
    assert_eq!(summary.layout, saved.layout);
    assert_eq!(summary.active_pane, saved.active_pane);
    for (pane_id, _, _) in &panes[2..] {
        assert!(client.attach(session_id, Some(*pane_id), None).is_err());
    }
}

#[test]
fn discarding_the_last_pane_removes_a_previously_kept_session() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client.spawn(spawn_request(None, "exec sleep 120")).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    let AttachOutcome::Attached { pane: attached, .. } =
        client.attach(pane.session_id, None, None).unwrap()
    else {
        panic!("the kept pane must attach exclusively")
    };
    client.discard_pane(pane.session_id, pane.pane_id).unwrap();
    assert!(client.list().unwrap().is_empty());
    assert!(client.attach(pane.session_id, None, None).is_err());
    drop(attached);
    drop(pane);
}

#[test]
fn releasing_a_kept_pane_preserves_it_for_reattachment() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let pane = client.spawn(spawn_request(None, "exec sleep 120")).unwrap();
    client
        .detach(
            pane.session_id,
            summary(pane.session_id, pane.pane_id),
            serde_json::Value::Null,
            None,
            Vec::new(),
        )
        .unwrap();
    let AttachOutcome::Attached { pane: attached, .. } =
        client.attach(pane.session_id, None, None).unwrap()
    else {
        panic!("the kept pane must attach exclusively")
    };
    client.close_pane(pane.session_id, pane.pane_id).unwrap();
    let AttachOutcome::Attached { pane: restored, .. } =
        client.attach(pane.session_id, None, None).unwrap()
    else {
        panic!("releasing an attachment must preserve its kept pane")
    };
    assert_eq!(restored.pane_id, pane.pane_id);
    assert_eq!(restored.child_pid, pane.child_pid);
    drop(attached);
}

#[test]
fn discarding_requires_the_panes_exclusive_holder() {
    let daemon = TestDaemon::start();
    let client = daemon.client();
    let holder = Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let pane = client
        .spawn(spawn_request_as(None, "exec sleep 120", holder.id()))
        .unwrap();
    let refused = client.discard_pane(pane.session_id, pane.pane_id);
    reap(holder);
    assert!(refused.unwrap_err().to_string().contains("exclusively"));
}
