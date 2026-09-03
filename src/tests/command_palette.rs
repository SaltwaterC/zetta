use super::*;
use crate::{
    NewTab, OpenProject, ToggleAutoBackgroundTab, ToggleTabMoveMode, ToggleTabPinning,
    ToggleTabSharing,
};
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
fn lifecycle_actions_are_exposed_only_in_their_launch_mode() {
    assert!(action_available_in_launch_mode(
        ToggleTabSharing.name(),
        false
    ));
    assert!(!action_available_in_launch_mode(
        ToggleAutoBackgroundTab.name(),
        false
    ));
    assert!(action_available_in_launch_mode(
        ToggleAutoBackgroundTab.name(),
        true
    ));
    assert!(!action_available_in_launch_mode(
        ToggleTabSharing.name(),
        true
    ));
    assert!(action_available_in_launch_mode(NewTab.name(), false));
    assert!(action_available_in_launch_mode(NewTab.name(), true));
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

    palette.query = TextField::new("paste");
    palette.refresh_matches();
    assert_eq!(palette.matches(), &[0]);
    assert_eq!(
        palette.commands[palette.matches()[0]].name,
        "terminal: paste"
    );
}

#[test]
fn registered_projects_become_palette_commands_that_carry_their_root() {
    let roots = [
        PathBuf::from("/home/user/source/zetta"),
        PathBuf::from("/home/user/work/zetta"),
    ];
    let commands = project_palette_commands(&roots);

    assert_eq!(
        commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>(),
        [
            "zetta: open project: zetta (/home/user/source/zetta)",
            "zetta: open project: zetta (/home/user/work/zetta)",
        ]
    );
    assert!(commands.iter().all(|command| command.shortcut.is_none()));
    for (command, root) in commands.iter().zip(&roots) {
        assert!(
            command
                .action
                .partial_eq(&OpenProject { root: root.clone() })
        );
    }

    // Two projects can share a directory name, and `CommandPalette::new` drops
    // duplicate names, so the path in the label is what keeps both reachable.
    let palette = CommandPalette::new(project_palette_commands(&roots));
    assert_eq!(palette.matches().len(), 2);
}

#[test]
fn project_commands_are_found_by_their_directory_name() {
    let mut palette = CommandPalette::new(project_palette_commands(&[PathBuf::from(
        "/home/user/source/zetta",
    )]));
    palette.query = TextField::new("zetta");
    palette.refresh_matches();

    assert_eq!(palette.matches().len(), 1);
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
