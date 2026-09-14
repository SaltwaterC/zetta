use super::*;
use gpui::{KeyDownEvent, Keystroke, Modifiers};

fn key_event(key: &str) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: Keystroke {
            modifiers: Modifiers::default(),
            key: key.to_owned(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    }
}

fn shifted_key_event(key: &str) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
            key: key.to_owned(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    }
}

#[test]
fn fuzzy_filtering_is_case_insensitive_and_keeps_display_order() {
    let options = vec![
        "One Dark".to_owned(),
        "Solarized Light".to_owned(),
        "Use application theme".to_owned(),
    ];

    let (rows, widest) = dropdown_snapshot_rows(&options, "SL");

    assert_eq!(rows.as_ref(), [1, 2]);
    assert_eq!(widest, Some(1));
}

#[test]
fn filtered_row_selection_moves_through_matching_options() {
    let mut dropdown = SearchableDropdown::default();
    dropdown.open(
        Arc::from([
            "Alpha One".to_owned(),
            "Beta".to_owned(),
            "Alpha Two".to_owned(),
        ]),
        0,
        Point::default(),
    );
    dropdown.set_query("alpha");

    assert_eq!(dropdown.rows.as_ref(), [0, 2]);
    assert_eq!(dropdown.selected_index, 0);
    dropdown.move_selection(1);
    assert_eq!(dropdown.selected_index, 2);
    assert_eq!(dropdown.selected_value().as_deref(), Some("Alpha Two"));
}

#[test]
fn navigation_wraps_in_both_directions() {
    let mut dropdown = SearchableDropdown::default();
    dropdown.open(
        Arc::from(["One".to_owned(), "Two".to_owned(), "Three".to_owned()]),
        0,
        Point::default(),
    );

    dropdown.move_selection(-1);
    assert_eq!(dropdown.selected_index, 2);
    dropdown.move_selection(1);
    assert_eq!(dropdown.selected_index, 0);
}

#[test]
fn a_no_match_query_cannot_commit() {
    let mut dropdown = SearchableDropdown::default();
    dropdown.open(
        Arc::from(["System".to_owned(), "Workspace".to_owned()]),
        0,
        Point::default(),
    );
    dropdown.set_query("missing");

    assert!(dropdown.rows.is_empty());
    assert_eq!(dropdown.selected_value(), None);
}

#[test]
fn keyboard_actions_keep_no_match_open_and_report_escape_and_tab() {
    let mut dropdown = SearchableDropdown::default();
    dropdown.open(
        Arc::from(["System".to_owned(), "Workspace".to_owned()]),
        0,
        Point::default(),
    );
    dropdown.set_query("missing");

    assert_eq!(
        dropdown.key_down(&key_event("enter"), false),
        SearchableDropdownAction::Commit(None)
    );
    assert_eq!(
        dropdown.key_down(&key_event("escape"), false),
        SearchableDropdownAction::Close
    );
    assert_eq!(
        dropdown.key_down(&key_event("tab"), false),
        SearchableDropdownAction::Tab { reverse: false }
    );
    assert_eq!(
        dropdown.key_down(&shifted_key_event("tab"), false),
        SearchableDropdownAction::Tab { reverse: true }
    );
}

#[test]
fn opening_preserves_the_current_selection() {
    let mut dropdown = SearchableDropdown::default();
    dropdown.open(
        Arc::from(["System".to_owned(), "Workspace".to_owned()]),
        1,
        Point::default(),
    );

    assert_eq!(dropdown.selected_index, 1);
    assert_eq!(dropdown.selected_value().as_deref(), Some("Workspace"));
}
