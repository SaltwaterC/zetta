use super::*;

#[test]
fn a_tabs_panes_join_one_session() {
    // A tab is a session: the first pane to reach the multiplexer creates it
    // and the rest join, or a split tab would come back as several sessions.
    let mut panes = MuxPanes::default();
    let first = panes.session_for_tab(7);
    let second = panes.session_for_tab(7);

    assert_eq!(first.id(), None);
    first.set_id(900);
    assert_eq!(
        second.id(),
        Some(900),
        "a pane spawned later must join the session the first one created"
    );
    assert_eq!(panes.session_id(7), Some(900));
}

#[test]
fn separate_tabs_get_separate_sessions() {
    let mut panes = MuxPanes::default();
    let first = panes.session_for_tab(1);
    let second = panes.session_for_tab(2);
    first.set_id(900);

    assert_eq!(second.id(), None);
    assert_eq!(panes.session_id(2), None);
}

#[test]
fn an_attached_session_is_adopted_by_the_tab_showing_it() {
    let mut panes = MuxPanes::default();
    panes.adopt_session(3, 42);

    assert_eq!(panes.session_id(3), Some(42));
    // A pane spawned into the attached tab joins the session it came from
    // rather than starting a second one.
    assert_eq!(panes.session_for_tab(3).id(), Some(42));
}

#[test]
fn pane_identifiers_map_both_ways_and_can_be_forgotten() {
    let mut panes = MuxPanes::default();
    panes.record(10, 900);
    panes.record(11, 901);

    assert_eq!(panes.mux_pane_id(10), Some(900));
    assert_eq!(panes.ids().len(), 2);

    panes.forget_pane(10);
    assert_eq!(panes.mux_pane_id(10), None);
    assert_eq!(panes.mux_pane_id(11), Some(901));

    panes.forget_tab(3);
    assert_eq!(panes.session_id(3), None);
}

#[test]
fn a_window_knows_which_sessions_it_is_already_showing() {
    // What keeps the reconnect picker from offering a window its own shared
    // session. The multiplexer hands a pane straight back to the process that
    // already holds it, so taking that offer would open a second tab reading
    // the same pty as the first, and the two would split its output.
    let mut panes = MuxPanes::default();
    panes.session_for_tab(1).set_id(900);
    panes.session_for_tab(2);

    assert!(panes.holds_session(900));
    assert!(!panes.holds_session(901));
    // A tab whose first pane has not reached the multiplexer yet holds no
    // session, and must not be mistaken for holding session zero.
    assert!(!panes.holds_session(0));

    panes.forget_tab(1);
    assert!(
        !panes.holds_session(900),
        "a detached tab's session is available again"
    );
}
