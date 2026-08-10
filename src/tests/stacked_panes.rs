use super::*;

fn profile() -> Profile {
    Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        icon: ProfileIcon::Zetta,
    }
}

fn entry(id: u64) -> StackedPane {
    StackedPane::new(id, format!("command-{id}"), profile(), None, None)
}

#[test]
fn stack_cycles_through_base_and_entries_with_wraparound() {
    let mut stack = PaneStack::default();
    assert!(stack.push(entry(1)));
    assert!(stack.push(entry(2)));

    assert_eq!(stack.selected, PaneStackSelection::Stacked(2));
    assert_eq!(stack.cycle(true), Some(PaneStackSelection::Base));
    assert_eq!(stack.cycle(true), Some(PaneStackSelection::Stacked(1)));
    assert_eq!(stack.cycle(false), Some(PaneStackSelection::Base));
    assert_eq!(stack.cycle(false), Some(PaneStackSelection::Stacked(2)));
}

#[test]
fn removing_selected_entry_prefers_the_next_entry_then_previous_then_base() {
    let mut stack = PaneStack::default();
    assert!(stack.push(entry(1)));
    assert!(stack.push(entry(2)));
    assert!(stack.push(entry(3)));

    assert_eq!(stack.remove(2).unwrap().id, 2);
    assert_eq!(stack.selected, PaneStackSelection::Stacked(3));
    assert_eq!(stack.remove(3).unwrap().id, 3);
    assert_eq!(stack.selected, PaneStackSelection::Stacked(1));
    assert_eq!(stack.remove(1).unwrap().id, 1);
    assert_eq!(stack.selected, PaneStackSelection::Base);
}

#[test]
fn selection_and_entries_are_isolated_per_host_stack() {
    let mut first = PaneStack::default();
    let mut second = PaneStack::default();
    assert!(first.push(entry(1)));
    assert!(second.push(entry(2)));

    assert!(first.select(PaneStackSelection::Base));
    assert_eq!(first.selected, PaneStackSelection::Base);
    assert_eq!(second.selected, PaneStackSelection::Stacked(2));
    assert_eq!(first.entries[0].id, 1);
    assert_eq!(second.entries[0].id, 2);
}

#[test]
fn invalid_selection_repairs_to_the_last_remaining_entry() {
    let mut stack = PaneStack::default();
    assert!(stack.push(entry(1)));
    assert!(stack.push(entry(2)));
    stack.selected = PaneStackSelection::Stacked(99);

    stack.repair_selection();

    assert_eq!(stack.selected, PaneStackSelection::Stacked(2));
}

#[test]
fn base_exit_preserves_stack_selection_and_moves_foreground_to_a_command() {
    let mut stack = PaneStack::default();
    assert!(stack.push(entry(1)));
    assert!(stack.push(entry(2)));

    stack.selected = PaneStackSelection::Base;
    stack.select_after_base_exit();
    assert_eq!(stack.selected, PaneStackSelection::Stacked(1));

    stack.selected = PaneStackSelection::Stacked(2);
    stack.select_after_base_exit();
    assert_eq!(stack.selected, PaneStackSelection::Stacked(2));
}

#[test]
fn stack_capacity_leaves_room_for_the_host_terminal() {
    let mut stack = PaneStack::default();
    for id in 0..MAX_PANES_PER_TAB.saturating_sub(1) as u64 {
        assert!(stack.push(entry(id)));
    }

    assert_eq!(stack.entries.len(), MAX_PANES_PER_TAB - 1);
    assert!(!stack.push(entry(MAX_PANES_PER_TAB as u64)));
}
