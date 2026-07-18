use super::*;
gpui::actions!(command_palette_test, [First, Second]);

#[test]
fn humanizes_action_names() {
    assert_eq!(humanize_action_name("zetta::NewTab"), "zetta: new tab");
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
