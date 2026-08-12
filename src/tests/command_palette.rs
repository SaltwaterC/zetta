use super::*;
use crate::{ToggleTabMoveMode, ToggleTabPinning};
gpui::actions!(command_palette_test, [First, Second]);

#[test]
fn humanizes_action_names() {
    assert_eq!(humanize_action_name("zetta::NewTab"), "zetta: new tab");
    assert_eq!(
        humanize_action_name(ToggleTabMoveMode.name()),
        "zetta: toggle tab move mode"
    );
    assert_eq!(
        humanize_action_name(ToggleTabPinning.name()),
        "zetta: toggle tab pinning"
    );
    assert_eq!(
        humanize_action_name("editor::OpenURLParser"),
        "editor: open URL parser"
    );
    assert_eq!(
        humanize_action_name("go_to_line::Deploy"),
        "go to line: deploy"
    );
}

#[test]
fn fuzzy_matching_finds_subsequences() {
    assert!(fuzzy_score("terminal: paste trimmed", "paste trim").is_some());
    assert!(fuzzy_score("terminal: paste", "missing").is_none());
}

#[test]
fn matches_are_cached_until_the_query_changes() {
    let mut palette = CommandPalette::new(vec![
        PaletteCommand {
            name: "terminal: paste".into(),
            shortcut: None,
            action: Box::new(First),
        },
        PaletteCommand {
            name: "window: new tab".into(),
            shortcut: None,
            action: Box::new(Second),
        },
    ]);
    assert_eq!(palette.matches(), &[0, 1]);

    palette.query = "paste".into();
    palette.refresh_matches();
    assert_eq!(palette.matches(), &[0]);
    assert_eq!(
        palette.commands[palette.matches()[0]].name,
        "terminal: paste"
    );
}

#[test]
fn pinned_first_command_stays_ahead_of_the_alphabetized_rest() {
    let palette = CommandPalette::with_pinned_first(
        PaletteCommand {
            name: "Reset to profile default".into(),
            shortcut: None,
            action: Box::new(First),
        },
        vec![
            PaletteCommand {
                name: "Zenburn".into(),
                shortcut: None,
                action: Box::new(Second),
            },
            PaletteCommand {
                name: "Dracula".into(),
                shortcut: None,
                action: Box::new(Second),
            },
        ],
    );
    let names = palette
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["Reset to profile default", "Dracula", "Zenburn"]);
}

#[test]
fn pinned_first_command_replaces_a_same_named_entry_in_the_rest() {
    let palette = CommandPalette::with_pinned_first(
        PaletteCommand {
            name: "Dracula".into(),
            shortcut: None,
            action: Box::new(First),
        },
        vec![PaletteCommand {
            name: "Dracula".into(),
            shortcut: None,
            action: Box::new(Second),
        }],
    );
    assert_eq!(palette.commands.len(), 1);
    assert_eq!(palette.commands[0].name, "Dracula");
}
