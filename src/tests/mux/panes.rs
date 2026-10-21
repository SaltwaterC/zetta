use super::*;

#[test]
fn pending_closes_belong_to_their_tab_and_survive_until_acknowledged() {
    let mut panes = MuxPanes::default();
    let (first, pending) = futures::channel::oneshot::channel();
    panes.record_pending_close(1, pending.shared());
    assert_eq!(panes.pending_closes(1).len(), 1);
    assert!(panes.pending_closes(2).is_empty());

    first.send(Ok(())).unwrap();
    let (_second, pending) = futures::channel::oneshot::channel();
    panes.record_pending_close(1, pending.shared());
    assert_eq!(panes.pending_closes(1).len(), 1);
    panes.forget_tab(1);
    assert!(panes.pending_closes(1).is_empty());
}

#[test]
fn a_failed_close_is_kept_for_the_next_publication_to_report() {
    let mut panes = MuxPanes::default();
    let (first, pending) = futures::channel::oneshot::channel();
    panes.record_pending_close(1, pending.shared());
    first.send(Err("refused".to_owned())).unwrap();
    let (_second, pending) = futures::channel::oneshot::channel();
    panes.record_pending_close(1, pending.shared());
    assert_eq!(panes.pending_closes(1).len(), 2);
}
