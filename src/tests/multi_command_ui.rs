use super::*;

/// A tiled multi-command replaces the active pane and tiles the rest beside it,
/// so `n` expansions cost `n - 1` new panes. Counting `n` would refuse a
/// command that exactly fills the tab.
#[test]
fn a_tiled_command_costs_one_pane_fewer_than_it_expands_to() {
    assert!(multi_command_pane_budget(1, MAX_PANES_PER_TAB).is_ok());
    assert!(
        multi_command_pane_budget(1, MAX_PANES_PER_TAB + 1).is_err(),
        "one past a full tab has to be refused"
    );
}

#[test]
fn a_command_that_fits_in_the_remaining_panes_is_allowed() {
    assert!(multi_command_pane_budget(MAX_PANES_PER_TAB - 1, 2).is_ok());
    assert!(multi_command_pane_budget(MAX_PANES_PER_TAB, 1).is_ok());
}

/// The message names both numbers, because "it does not fit" alone leaves the
/// user guessing how many panes they would have to close.
#[test]
fn a_command_that_does_not_fit_reports_both_the_tab_and_the_limit() {
    let existing = MAX_PANES_PER_TAB;
    let message = multi_command_pane_budget(existing, 2).unwrap_err();
    assert!(message.contains(&existing.to_string()));
    assert!(message.contains(&MAX_PANES_PER_TAB.to_string()));
}

/// `expansions` comes from the expander, which never returns an empty list, but
/// the subtraction must not wrap if it ever did.
#[test]
fn an_empty_expansion_costs_no_panes_rather_than_wrapping() {
    assert!(multi_command_pane_budget(MAX_PANES_PER_TAB, 0).is_ok());
}
