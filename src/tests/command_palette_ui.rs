use super::*;

use gpui::{Keystroke, Modifiers, NoAction};

fn palette(names: &[&str]) -> CommandPalette {
    CommandPalette::new(
        names
            .iter()
            .map(|name| PaletteCommand {
                name: (*name).to_owned(),
                shortcut: None,
                action: Box::new(NoAction),
            })
            .collect(),
    )
}

/// A named key with no character behind it — an arrow, `enter`, `escape`.
fn key(key: &str) -> Keystroke {
    Keystroke {
        modifiers: Modifiers::default(),
        key: key.to_owned(),
        key_char: None,
    }
}

/// A keystroke the platform resolved to a character, which is what the query
/// takes as text.
fn typed(character: &str) -> Keystroke {
    Keystroke {
        modifiers: Modifiers::default(),
        key: character.to_owned(),
        key_char: Some(character.to_owned()),
    }
}

fn selected_name(palette: &CommandPalette) -> &str {
    let index = palette.matches()[palette.selected];
    &palette.commands[index].name
}

/// `command_palette_key_down` and `theme_picker_key_down` both delegate the
/// list half to [`CommandPalette::apply_key`] and keep only `escape` and what
/// `enter` runs. These pin the half they share.
#[test]
fn the_arrow_keys_move_the_selection_and_stop_at_the_ends() {
    let mut palette = palette(&["alpha", "beta", "gamma"]);
    assert_eq!(selected_name(&palette), "alpha");

    assert_eq!(palette.apply_key(&key("up")), PaletteKey::Redraw);
    assert_eq!(
        selected_name(&palette),
        "alpha",
        "up on the first match must not wrap to the last"
    );

    palette.apply_key(&key("down"));
    assert_eq!(selected_name(&palette), "beta");
    palette.apply_key(&key("down"));
    palette.apply_key(&key("down"));
    assert_eq!(
        selected_name(&palette),
        "gamma",
        "down on the last match must not wrap to the first"
    );
    palette.apply_key(&key("up"));
    assert_eq!(selected_name(&palette), "beta");
}

#[test]
fn enter_accepts_the_selected_match_rather_than_running_it() {
    let mut palette = palette(&["alpha", "beta"]);
    palette.apply_key(&key("down"));
    let PaletteKey::Accept(command) = palette.apply_key(&key("enter")) else {
        panic!("enter on a match should accept it");
    };
    assert_eq!(palette.commands[command].name, "beta");
}

/// Both surfaces stay open on `enter` with nothing matched: the keystroke did
/// nothing, so dismissing on it would lose a half-typed query.
#[test]
fn enter_with_no_matches_is_a_redraw_rather_than_an_accept() {
    let mut palette = palette(&["alpha"]);
    for character in "zzz".chars() {
        palette.apply_key(&typed(&character.to_string()));
    }
    assert!(palette.matches().is_empty());
    assert_eq!(palette.apply_key(&key("enter")), PaletteKey::Redraw);
}

/// Typing re-filters, so the old selection index would point into a list that
/// no longer exists.
#[test]
fn typing_refilters_and_returns_the_selection_to_the_first_match() {
    let mut palette = palette(&["alpha", "gamma", "gate"]);
    palette.apply_key(&key("down"));
    palette.apply_key(&key("down"));
    assert_eq!(selected_name(&palette), "gate");

    assert_eq!(palette.apply_key(&typed("g")), PaletteKey::Redraw);
    assert_eq!(palette.selected, 0);
    assert!(
        palette
            .matches()
            .iter()
            .all(|index| palette.commands[*index].name.starts_with('g'))
    );
}

/// `escape` is deliberately not answered here: the palette dismisses itself and
/// the theme picker restores the pane's previous theme, so each surface keeps
/// it.
#[test]
fn escape_is_left_to_the_surface_that_owns_the_list() {
    let mut palette = palette(&["alpha"]);
    assert_eq!(palette.apply_key(&key("escape")), PaletteKey::Ignored);
    assert_eq!(
        palette.query.text, "",
        "escape must not type into the query"
    );
}
