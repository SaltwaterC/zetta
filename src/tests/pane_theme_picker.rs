use super::*;

fn names(picker: &CommandPalette) -> Vec<&str> {
    picker
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect()
}

/// The reset entry stays first whatever it is called, because it is the one
/// entry that is not a theme — sorting it alphabetically would bury it among
/// the themes starting with `R`.
#[test]
fn the_reset_entry_is_pinned_above_the_themes() {
    let installed = vec!["Solarized".to_owned(), "Ayu".to_owned()];
    let pane = theme_picker_palette(ThemeScope::Pane, installed.clone(), None);
    assert_eq!(names(&pane), ["Reset to tab default", "Ayu", "Solarized"]);

    let tab = theme_picker_palette(ThemeScope::Tab, installed, None);
    assert_eq!(
        names(&tab),
        ["Reset to configured theme", "Ayu", "Solarized"],
        "the two scopes reset to different things and say so"
    );
}

/// Two extensions can ship a theme of the same name, and the registry lists
/// both; offering it twice would give the picker two rows that do the same
/// thing.
#[test]
fn a_theme_installed_twice_is_offered_once() {
    let picker = theme_picker_palette(
        ThemeScope::Pane,
        vec![
            "One Dark".to_owned(),
            "Ayu".to_owned(),
            "One Dark".to_owned(),
        ],
        None,
    );
    assert_eq!(names(&picker), ["Reset to tab default", "Ayu", "One Dark"]);
}

/// The picker opens on whatever theme the pane is actually showing, so `enter`
/// without moving is a no-op rather than a change.
#[test]
fn the_selection_opens_on_the_theme_that_is_showing() {
    let installed = vec!["Ayu".to_owned(), "One Dark".to_owned(), "Rose".to_owned()];
    let picker = theme_picker_palette(ThemeScope::Pane, installed.clone(), Some("One Dark"));
    assert_eq!(picker.commands[picker.selected].name, "One Dark");

    let unknown = theme_picker_palette(ThemeScope::Pane, installed, Some("Uninstalled"));
    assert_eq!(
        unknown.selected, 0,
        "a theme that is no longer installed leaves the selection on the reset entry"
    );
}

#[test]
fn an_empty_registry_still_offers_the_reset_entry() {
    let picker = theme_picker_palette(ThemeScope::Tab, Vec::new(), None);
    assert_eq!(names(&picker), ["Reset to configured theme"]);
    assert_eq!(picker.matches(), [0]);
}
