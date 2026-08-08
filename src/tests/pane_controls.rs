use super::*;

#[test]
fn pane_controls_idle_delay_resets_and_expires() {
    let start = Instant::now();

    assert_eq!(
        pane_controls_hide_delay(start, start + Duration::from_millis(200)),
        Some(Duration::from_millis(1000))
    );
    assert_eq!(
        pane_controls_hide_delay(start, start + PANE_CONTROLS_IDLE_DELAY),
        None
    );
    assert_eq!(
        pane_controls_hide_delay(start, start + Duration::from_secs(5)),
        None
    );
}

#[test]
fn pane_controls_toggle_hides_and_restores_each_requested_pane() {
    let mut hidden_panes = HashSet::from([1]);

    assert!(toggle_hidden_pane_controls(&mut hidden_panes, &[1, 2]));
    assert_eq!(hidden_panes, HashSet::from([1, 2]));

    assert!(!toggle_hidden_pane_controls(&mut hidden_panes, &[1, 2]));
    assert!(hidden_panes.is_empty());
}

#[test]
fn pane_controls_toggle_keeps_other_tabs_unchanged() {
    let mut hidden_panes = HashSet::from([1, 2, 3]);

    assert!(!toggle_hidden_pane_controls(&mut hidden_panes, &[1, 2]));
    assert_eq!(hidden_panes, HashSet::from([3]));
}

#[test]
fn pane_controls_can_start_hidden_without_disabling_toggles() {
    let mut hidden_panes = default_hidden_pane_controls(true, [1, 2]);

    assert_eq!(hidden_panes, HashSet::from([1, 2]));
    assert!(!toggle_hidden_pane_controls(&mut hidden_panes, &[1, 2]));
    assert!(hidden_panes.is_empty());
    assert!(default_hidden_pane_controls(false, [3]).is_empty());
}

#[test]
fn reloading_changed_pane_controls_default_resets_open_panes() {
    let mut hidden_panes = HashSet::from([1, 3]);

    reset_pane_controls_visibility(&mut hidden_panes, true, [1, 2]);
    assert_eq!(hidden_panes, HashSet::from([1, 2, 3]));

    reset_pane_controls_visibility(&mut hidden_panes, false, [1, 2]);
    assert_eq!(hidden_panes, HashSet::from([3]));
}
